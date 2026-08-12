//! Environments management view.
//!
//! Shown inside a Dialog opened from the title bar. Lets the user pick an
//! environment, edit its variables (kv table), add/remove environments, and
//! set the active one. Changes write back to the shared [`AppState`].

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{
    ActiveTheme, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::state::models::{Environment, KeyValue};
use crate::state::{AppEvent, AppState};
use crate::ui::kv_table::{self, KvHandlers, KvRow};

/// Which item in the left sidebar is selected for editing.
#[derive(Clone, PartialEq, Eq)]
pub enum SettingsSection {
    /// An environment by id.
    Env(String),
    /// A project-global KV scope.
    Global(crate::ui::kv_manager_view::KvScope),
}

pub struct EnvironmentsView {
    pub state: Entity<AppState>,
    pub env_select: Entity<SelectState<Vec<String>>>,
    pub name_input: Entity<InputState>,
    /// Kv rows for the currently-selected environment.
    pub rows: Vec<KvRow>,
    pub selected_env_id: Option<String>,
    /// The env id whose rows are currently in `rows`; used to detect when a
    /// reload is needed.
    pub last_built_id: Option<String>,
    /// Which sidebar section is active (drives the right panel content).
    pub active_section: Option<SettingsSection>,
    /// Signature of the last committed substitution-relevant content, so we
    /// only emit [`AppEvent::EnvironmentChanged`] when values actually change
    /// (not on every render).
    last_subst_sig: Option<u64>,
    _subs: Vec<gpui::Subscription>,
}

impl EnvironmentsView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Build the env-name options from the active project.
        let names: Vec<String> = state
            .read(cx)
            .active_project()
            .map(|p| {
                let mut v = vec!["+ New environment".to_string()];
                v.extend(p.environments.iter().map(|e| e.name.clone()));
                v
            })
            .unwrap_or_default();
        let env_select = cx.new(|cx| {
            SelectState::new(
                names,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Environment name"));
        let mut view = Self {
            state: state.clone(),
            env_select,
            name_input,
            rows: Vec::new(),
            selected_env_id: None,
            last_built_id: None,
            active_section: None,
            last_subst_sig: None,
            _subs: Vec::new(),
        };
        // Load the active environment (if any) by default.
        let active_id = state
            .read(cx)
            .active_project()
            .and_then(|p| p.active_environment.clone());
        if let Some(id) = active_id {
            view.selected_env_id = Some(id.clone());
            view.active_section = Some(SettingsSection::Env(id.clone()));
            view.load_env(Some(id.clone()), window, cx);
            // Reflect in the select.
            if let Some(name) = state.read(cx).active_project().and_then(|p| {
                p.environments
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.name.clone())
            }) {
                view.env_select
                    .update(cx, |s, cx| s.set_selected_value(&name, window, cx));
            }
        }
        let sub = cx.subscribe(&view.env_select.clone(), Self::on_env_change);
        view._subs = vec![sub];
        view
    }

    fn on_env_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let chosen = src.read(cx).selected_value().cloned();
        if chosen.as_deref() == Some("+ New environment") {
            // Create a fresh env.
            let env = Environment::new("New Environment");
            let id = env.id.clone();
            self.state.update(cx, |s, cx| {
                if let Some(p) = s.active_project_mut() {
                    p.environments.push(env);
                    s.notify_workspace(cx);
                }
            });
            self.selected_env_id = Some(id);
            cx.notify();
            return;
        }
        // Find the env id by name.
        let id = self.state.read(cx).active_project().and_then(|p| {
            chosen.as_deref().and_then(|n| {
                p.environments
                    .iter()
                    .find(|e| e.name == n)
                    .map(|e| e.id.clone())
            })
        });
        if let Some(id) = id {
            self.selected_env_id = Some(id.clone());
            self.active_section = Some(SettingsSection::Env(id));
            cx.notify();
        }
    }

    fn load_env(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let vars = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| {
                id.as_ref()
                    .and_then(|i| p.environments.iter().find(|e| e.id == *i))
                    .map(|e| e.variables.clone())
            })
            .unwrap_or_default();
        self.rows = kv_table::rows_from_pairs(&vars, window, cx);
        self.name_input.update(cx, |s, cx| {
            let name = self
                .state
                .read(cx)
                .active_project()
                .and_then(|p| {
                    id.as_ref().and_then(|i| {
                        p.environments
                            .iter()
                            .find(|e| e.id == *i)
                            .map(|e| e.name.clone())
                    })
                })
                .unwrap_or_default();
            s.set_value(name, window, cx);
        });
    }

    /// Commit the current rows (+ name for environments) back into the active
    /// section. Dispatches between the selected environment and a global scope.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let pairs = kv_table::pairs_from_rows(&self.rows, cx);
        // Track whether the committed change should trigger a cross-panel
        // variable refresh. Environment variable edits and global variable
        // edits both affect URL/header substitution in the request panel.
        let mut affects_substitution = false;
        match self.active_section.clone() {
            Some(SettingsSection::Env(id)) => {
                let name = self.name_input.read(cx).value().to_string();
                let name = if name.trim().is_empty() {
                    "Environment".to_string()
                } else {
                    name
                };
                self.state.update(cx, |s, cx| {
                    if let Some(p) = s.active_project_mut() {
                        if let Some(e) = p.environments.iter_mut().find(|e| e.id == id) {
                            e.variables = pairs;
                            e.name = name;
                        }
                    }
                    s.notify_edited(cx);
                });
                // Any environment change affects substitution if it's the
                // active environment (its values are what get injected).
                let is_active = self
                    .state
                    .read(cx)
                    .active_project()
                    .and_then(|p| p.active_environment.as_ref())
                    .map(|active| active == &id)
                    .unwrap_or(false);
                affects_substitution = is_active;
            }
            Some(SettingsSection::Global(scope)) => {
                self.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        match scope {
                            crate::ui::kv_manager_view::KvScope::GlobalParams => {
                                project.global_params = pairs
                            }
                            crate::ui::kv_manager_view::KvScope::GlobalHeaders => {
                                project.global_headers = pairs
                            }
                            crate::ui::kv_manager_view::KvScope::GlobalVariables => {
                                project.global_variables = pairs
                            }
                            crate::ui::kv_manager_view::KvScope::GlobalCookies => {
                                project.global_cookies = pairs
                            }
                        }
                    }
                    s.notify_edited(cx);
                });
                affects_substitution =
                    scope == crate::ui::kv_manager_view::KvScope::GlobalVariables;
            }
            None => {}
        }
        // Notify other panels (request panel, folder settings) that the
        // effective variable values changed so they re-render substituted
        // URLs / base-URL displays. Use a signature so we only emit when the
        // committed content actually differs from the last emit — this avoids
        // re-render storms from the per-render commit() call.
        if affects_substitution {
            let sig = kv_signature(&kv_table::pairs_from_rows(&self.rows, cx));
            if self.last_subst_sig != Some(sig) {
                self.last_subst_sig = Some(sig);
                self.state.update(cx, |_s, cx| {
                    cx.emit(AppEvent::EnvironmentChanged);
                });
            }
        } else {
            // Reset the signature when switching away from a
            // substitution-relevant section so the next edit re-emits.
            self.last_subst_sig = None;
        }
    }

    /// Set this environment as active and persist.
    fn set_active(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_env_id.clone() {
            self.commit(cx);
            self.state.update(cx, |s, cx| {
                if let Some(p) = s.active_project_mut() {
                    p.active_environment = Some(id);
                    cx.emit(AppEvent::EnvironmentChanged);
                }
                s.persist(cx);
            });
        }
    }

    /// Request deletion of the given env — opens a real confirmation dialog.
    fn request_delete_env(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // Resolve the env name for the prompt.
        let env_name = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| {
                p.environments
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.name.clone())
            })
            .unwrap_or_else(|| "该环境".to_string());
        let state = self.state.clone();
        let id_for_confirm = id.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let state_footer = state.clone();
            let id_footer = id_for_confirm.clone();
            let name_for_prompt = env_name.clone();
            dialog
                .title("确认删除")
                .content(move |content, _, _| {
                    // Clone per-render (content is a Fn).
                    let name = name_for_prompt.clone();
                    content.child(
                        v_flex().p_4().w(px(360.)).gap_2().child(
                            div()
                                .text_sm()
                                .child(format!("确定要删除环境「{}」吗？此操作不可撤销。", name)),
                        ),
                    )
                })
                .footer({
                    let state_del = state_footer.clone();
                    let id_del = id_footer.clone();
                    gpui_component::button::Button::new("confirm-env-delete")
                        .primary()
                        .small()
                        .label("删除")
                        .on_click(move |_, window, cx| {
                            let id_del = id_del.clone();
                            let _ = state_del.update(cx, |s, cx| {
                                if let Some(p) = s.active_project_mut() {
                                    p.environments.retain(|e| e.id != id_del);
                                    if p.active_environment.as_deref() == Some(&id_del) {
                                        p.active_environment = None;
                                    }
                                    s.notify_workspace(cx);
                                }
                            });
                            window.close_dialog(cx);
                        })
                })
        });
    }

    /// Create a fresh environment and select it for editing.
    fn add_env(&mut self, cx: &mut Context<Self>) {
        let env = Environment::new("New Environment");
        let id = env.id.clone();
        self.state.update(cx, |s, cx| {
            if let Some(p) = s.active_project_mut() {
                p.environments.push(env);
                s.notify_workspace(cx);
            }
        });
        self.selected_env_id = Some(id.clone());
        self.active_section = Some(SettingsSection::Env(id));
        self.last_built_id = None; // force reload on next render
        cx.notify();
    }

    /// Select an environment by id for editing (left-sidebar click).
    fn select_env(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected_env_id = Some(id.clone());
        self.active_section = Some(SettingsSection::Env(id));
        self.last_built_id = None; // force reload
        cx.notify();
    }

    /// Select a project-global KV scope for editing (left-sidebar click).
    fn select_global(
        &mut self,
        scope: crate::ui::kv_manager_view::KvScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_section = Some(SettingsSection::Global(scope));
        // Reload rows from the chosen global scope.
        let pairs = read_global(&self.state, scope, cx);
        self.rows = kv_table::rows_from_pairs(&pairs, window, cx);
        self.last_built_id = Some(format!("global-{:?}", scope));
        cx.notify();
    }
}

/// Read a project-global KV slice (mirrors KvManagerView::read_scope).
fn read_global(
    state: &Entity<AppState>,
    scope: crate::ui::kv_manager_view::KvScope,
    cx: &App,
) -> Vec<crate::state::models::KeyValue> {
    use crate::ui::kv_manager_view::KvScope;
    let s = state.read(cx);
    let project = match s.active_project() {
        Some(p) => p,
        None => return Vec::new(),
    };
    match scope {
        KvScope::GlobalParams => project.global_params.clone(),
        KvScope::GlobalHeaders => project.global_headers.clone(),
        KvScope::GlobalVariables => project.global_variables.clone(),
        KvScope::GlobalCookies => project.global_cookies.clone(),
    }
}

impl Render for EnvironmentsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Reconcile rows for the active section (env reload on selection change).
        self.reload_if_needed(window, cx);
        // Commit current row edits before rendering so text changes are
        // persisted to the model on every render cycle. This ensures
        // environment variable edits take effect immediately.
        self.commit(cx);
        let theme = cx.theme().clone();
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_req = cx.entity();
        let handlers = KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    if let Some(row) = this.rows.get_mut(ix) {
                        row.enabled = val;
                    }
                    this.commit(cx);
                });
            }),
            on_delete: Arc::new(move |ix, window, cx: &mut App| {
                // Empty rows (e.g. the trailing add-slot) delete immediately;
                // rows with content ask for confirmation first.
                let row_info = view_delete.read(cx).rows.get(ix).map(|r| {
                    (
                        r.key.read(cx).value().to_string(),
                        r.value.read(cx).value().to_string(),
                    )
                });
                let has_content = row_info
                    .as_ref()
                    .map(|(k, v)| !k.trim().is_empty() || !v.trim().is_empty())
                    .unwrap_or(false);
                if !has_content {
                    let _ = view_delete.update(cx, |this, cx| {
                        if ix < this.rows.len() {
                            this.rows.remove(ix);
                        }
                        this.commit(cx);
                    });
                    return;
                }
                let key_label = row_info
                    .as_ref()
                    .map(|(k, _)| k.trim().to_string())
                    .filter(|k| !k.is_empty());
                let view_for_dialog = view_delete.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let key_for_content = key_label.clone();
                    let view_del = view_for_dialog.clone();
                    dialog
                        .title("确认删除")
                        .content(move |content, _, _| {
                            let msg = match key_for_content.as_ref() {
                                Some(k) => {
                                    format!("确定要删除变量「{}」吗？此操作不可撤销。", k)
                                }
                                None => "确定要删除该变量吗？此操作不可撤销。".to_string(),
                            };
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w(px(360.))
                                    .gap_2()
                                    .child(div().text_sm().child(msg)),
                            )
                        })
                        .footer(
                            gpui_component::button::Button::new("confirm-var-delete")
                                .primary()
                                .small()
                                .label("删除")
                                .on_click(move |_, window, cx| {
                                    let _ = view_del.update(cx, |this, cx| {
                                        if ix < this.rows.len() {
                                            this.rows.remove(ix);
                                        }
                                        this.commit(cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }),
            on_add: Arc::new(move |window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.rows.push(KvRow::empty(window, cx));
                    this.commit(cx);
                });
            }),
            on_type_change: Arc::new(|_, _, _, _| {}),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    if let Some(row) = this.rows.get_mut(ix) {
                        row.required = val;
                    }
                    this.commit(cx);
                });
            }),
            on_file_pick: Arc::new(|_, _, _| {}),
        };

        let active_env = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| p.active_environment.clone());
        // Snapshot the environment list (id, name) for the left sidebar.
        let env_list: Vec<(String, String)> = self
            .state
            .read(cx)
            .active_project()
            .map(|p| {
                p.environments
                    .iter()
                    .map(|e| (e.id.clone(), e.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let active_section = self.active_section.clone();

        // The global entries shown below the env list in the sidebar.
        let global_entries = [
            (
                crate::ui::kv_manager_view::KvScope::GlobalVariables,
                "全局变量",
            ),
            (
                crate::ui::kv_manager_view::KvScope::GlobalParams,
                "全局参数",
            ),
            (
                crate::ui::kv_manager_view::KvScope::GlobalHeaders,
                "全局请求头",
            ),
            (
                crate::ui::kv_manager_view::KvScope::GlobalCookies,
                "Cookie 管理器",
            ),
        ];

        // Helper: is a given section the active one?
        let section_is_active = |s: &SettingsSection| active_section.as_ref() == Some(s);

        // Two-column layout: left sidebar (env list + global entries) + right
        // detail panel (switches by active_section). Fill the host window's
        // content area (the OS window is resizable) instead of a fixed size so
        // no dark background leaks through at the bottom/right edges.
        h_flex()
            .size_full()
            .relative()
            .gap_0()
            .overflow_hidden()
            // --- Left sidebar.
            .child(
                v_flex()
                    .w(px(220.))
                    .h_full()
                    .min_h_0()
                    .border_r_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    // Header: title + 新建 button.
                    .child(
                        h_flex()
                            .px_2()
                            .py_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .flex_1()
                                    .child("环境"),
                            )
                            .child(
                                Button::new("env-add")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .label("新建")
                                    .tooltip("新建环境")
                                    .on_click(cx.listener(|this, _, _, cx| this.add_env(cx))),
                            ),
                    )
                    // Scrollable environment list + global entries.
                    .child(
                        v_flex()
                            .id("env-list-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_1()
                            .gap(px(1.))
                            .children(env_list.iter().map(|(id, name)| {
                                let id = id.clone();
                                let id_del = id.clone();
                                let is_selected =
                                    section_is_active(&SettingsSection::Env(id.clone()));
                                let is_active_env = Some(&id) == active_env.as_ref();
                                let view = cx.entity();
                                let view_del = cx.entity();
                                let group = format!("env-row-{}", id);
                                let theme_r = theme.clone();
                                div()
                                    .id(format!("env-side-{}", id))
                                    .w_full()
                                    .px_2()
                                    .py(px(5.))
                                    .rounded(theme.radius)
                                    .flex()
                                    .gap_1()
                                    .items_center()
                                    .text_sm()
                                    .group(group.clone())
                                    .when(is_selected, |d| {
                                        d.bg(theme.primary.opacity(0.18))
                                            .text_color(theme.foreground)
                                    })
                                    .when(!is_selected, |d| d.text_color(theme.muted_foreground))
                                    .hover(|d| d.bg(theme.accent.opacity(0.4)))
                                    .child(div().flex_1().child(name.clone()))
                                    .when(is_active_env, |d| {
                                        d.child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(theme.primary)
                                                .child("●"),
                                        )
                                    })
                                    // Hover-reveal delete button.
                                    .child(
                                        div()
                                            .id(format!("env-side-del-{}", id))
                                            .w(px(18.))
                                            .h(px(18.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(4.))
                                            .text_color(theme_r.danger.opacity(0.8))
                                            .opacity(0.0)
                                            .group_hover(group.clone(), |d| d.opacity(1.0))
                                            .hover(|d| d.bg(theme_r.danger.opacity(0.2)))
                                            .child(IconName::Delete)
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                move |_, window, cx: &mut App| {
                                                    cx.stop_propagation();
                                                    let _ = view_del.update(cx, |this, cx| {
                                                        this.request_delete_env(
                                                            id_del.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                },
                                            ),
                                    )
                                    .on_click(move |_, _window, cx: &mut App| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.select_env(id.clone(), cx);
                                        });
                                    })
                            }))
                            // Separator between environments and global entries.
                            .child(div().w_full().h(px(1.)).my(px(4.)).bg(theme.border))
                            .children(global_entries.iter().map(|(scope, label)| {
                                let scope = *scope;
                                let label = (*label).to_string();
                                let is_selected =
                                    section_is_active(&SettingsSection::Global(scope));
                                let view = cx.entity();
                                div()
                                    .id(format!("gentry-{:?}", scope))
                                    .w_full()
                                    .px_2()
                                    .py(px(5.))
                                    .rounded(theme.radius)
                                    .flex()
                                    .items_center()
                                    .text_sm()
                                    .when(is_selected, |d| {
                                        d.bg(theme.primary.opacity(0.18))
                                            .text_color(theme.foreground)
                                    })
                                    .when(!is_selected, |d| d.text_color(theme.muted_foreground))
                                    .hover(|d| d.bg(theme.accent.opacity(0.4)))
                                    .child(div().flex_1().child(label))
                                    .on_click(move |_, window, cx: &mut App| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.select_global(scope, window, cx);
                                        });
                                    })
                            })),
                    ),
            )
            // --- Right detail panel: switches by active_section.
            .child(
                v_flex().flex_1().min_w_0().h_full().p_3().gap_2().child(
                    match active_section.clone() {
                        None => v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .text_color(theme.muted_foreground)
                            .child("选择左侧的环境或全局项进行编辑，或点击「新建」创建一个环境。")
                            .into_any_element(),
                        Some(SettingsSection::Env(id)) => {
                            let is_env = self
                                .state
                                .read(cx)
                                .active_project()
                                .map_or(false, |p| p.environments.iter().any(|e| e.id == id));
                            if !is_env {
                                v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .text_color(theme.muted_foreground)
                                    .child("该环境已删除。")
                                    .into_any_element()
                            } else {
                                v_flex()
                                    .gap_2()
                                    .size_full()
                                    .min_h_0()
                                    // Header: toolbar + description. Structurally parallel
                                    // to the global-section header (same row heights) so the
                                    // variables table area lines up across the two views.
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .flex_shrink_0()
                                            .child(
                                                h_flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .min_h(px(30.))
                                                    .child(
                                                        div().flex_1().child(
                                                            Input::new(&self.name_input)
                                                                .small()
                                                                .prefix(IconName::Settings),
                                                        ),
                                                    )
                                                    .child(
                                                        Button::new("env-set-active")
                                                            .primary()
                                                            .small()
                                                            .label("设为当前")
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.set_active(cx)
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("env-delete")
                                                            .ghost()
                                                            .small()
                                                            .icon(IconName::Delete)
                                                            .tooltip("删除环境")
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    if let Some(id) =
                                                                        this.selected_env_id.clone()
                                                                    {
                                                                        this.request_delete_env(
                                                                            id, window, cx,
                                                                        )
                                                                    }
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child("环境变量在该环境激活时用于变量替换"),
                                            ),
                                    )
                                    // Variables table (scrollable).
                                    .child(
                                        div()
                                            .id("env-vars-scroll")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .overflow_x_scroll()
                                            .child(
                                                div().w_full().min_w(px(640.)).child(
                                                    crate::ui::kv_table::KvTable::new(
                                                        "env-vars",
                                                        self.rows.clone(),
                                                        handlers,
                                                    )
                                                    .show_description(false)
                                                    .value_width(px(360.))
                                                    .description_flex(true)
                                                    .show_enabled(false),
                                                ),
                                            ),
                                    )
                                    .into_any_element()
                            }
                        }
                        Some(SettingsSection::Global(scope)) => {
                            let title = scope.title().to_string();
                            let desc = scope.description().to_string();
                            v_flex()
                                .gap_2()
                                .size_full()
                                .min_h_0()
                                // Header: title + description. Structurally parallel to the
                                // environment header (same row heights) so the variables
                                // table area lines up across the two views.
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .flex_shrink_0()
                                        .child(
                                            h_flex().items_center().gap_2().min_h(px(30.)).child(
                                                div()
                                                    .flex_1()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child(title),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(desc),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("global-vars-scroll")
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .overflow_x_scroll()
                                        .child(
                                            div().w_full().min_w(px(640.)).child(
                                                crate::ui::kv_table::KvTable::new(
                                                    "global-vars",
                                                    self.rows.clone(),
                                                    handlers,
                                                )
                                                .show_description(false)
                                                .value_width(px(360.))
                                                .description_flex(true)
                                                .show_enabled(false),
                                            ),
                                        ),
                                )
                                .into_any_element()
                        }
                    },
                ),
            )
    }
}

impl EnvironmentsView {
    /// Reload rows when the active section no longer matches the rows we're
    /// showing. We track the last-built section via a string marker.
    fn reload_if_needed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let marker = match &self.active_section {
            Some(SettingsSection::Env(id)) => format!("env-{}", id),
            Some(SettingsSection::Global(scope)) => format!("global-{:?}", scope),
            None => String::new(),
        };
        if self.last_built_id.as_deref() != Some(marker.as_str()) {
            self.last_built_id = Some(marker.clone());
            match &self.active_section {
                Some(SettingsSection::Env(id)) => {
                    self.load_env(Some(id.clone()), window, cx);
                }
                Some(SettingsSection::Global(scope)) => {
                    let pairs = read_global(&self.state, *scope, cx);
                    self.rows = kv_table::rows_from_pairs(&pairs, window, cx);
                }
                None => {
                    self.rows.clear();
                }
            }
        }
    }
}

/// Open a global-KV management dialog (Cookie/params/variables) over the given
/// window. Called from the environment-management view's quick-link buttons.
fn open_global_dialog(
    state: Entity<AppState>,
    scope: crate::ui::kv_manager_view::KvScope,
    window: &mut Window,
    cx: &mut App,
) {
    let title = scope.title().to_string();
    let view =
        cx.new(|cx| crate::ui::kv_manager_view::KvManagerView::new(state, scope, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        let view = view.clone();
        dialog
            .title(title.clone())
            .w(px(700.))
            .content(move |content, _, _| content.child(div().p_4().child(view.clone())))
    });
}

/// Compute a stable signature over a list of key/value pairs so we can tell
/// whether the substitution-relevant content actually changed between
/// commits. Used to throttle [`AppEvent::EnvironmentChanged`] emits.
fn kv_signature(pairs: &[KeyValue]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    pairs.len().hash(&mut hasher);
    for kv in pairs {
        kv.key.hash(&mut hasher);
        kv.value.hash(&mut hasher);
        kv.enabled.hash(&mut hasher);
    }
    hasher.finish()
}
