//! Project management panel — a full-bleed management surface shown when the
//! activity rail's "项目管理" button is active. It REPLACES the entire API
//! workbench (tree + request/response) with its own layout:
//!
//!   ┌─ activity rail ┬─ secondary nav ┬─ section content ─┐
//!   │  (in app.rs)   │  基本设置        │                   │
//!   │                │  迭代分支        │  (the selected    │
//!   │                │  合并请求        │   section's body) │
//!   │                │  分支管理        │                   │
//!   │                │  Mock 服务       │                   │
//!   │                │  ...            │                   │
//!   └────────────────┴────────────────┴───────────────────┘
//!
//! Sections map to postman's project-management areas. Git operations live
//! under "分支管理" (Branch Management); basic project metadata under
//! "基本设置" (Basic Settings).

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::assets::{EXPORT, IMPORT};
use crate::git::state::{GitEvent, GitState};

/// Build an icon from a Verve-custom Lucide SVG path.
fn vicon(path: &'static str) -> Icon {
    Icon::from(IconName::Redo).path(path)
}
use crate::state::models::{ApiToken, MergeRequest, RequestMethod, StatusCodeEntry};
use crate::state::persistence::{self, GitConfig};
use crate::state::{AppEvent, AppState};

/// A selectable management section in the secondary nav.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ManageSection {
    BasicSettings,
    IterationBranch,
    MergeRequests,
    BranchManage,
    MockService,
    PublicResources,
    IfaceProps,
    IfaceStatus,
    ExternalCaps,
    ImportExport,
}

impl ManageSection {
    /// The flat list of (id, label, optional group label) entries shown in the
    /// secondary nav, top to bottom.
    fn all() -> &'static [(&'static str, &'static str, ManageSection)] {
        &[
            ("basic", "基本设置", ManageSection::BasicSettings),
            ("iter", "迭代分支", ManageSection::IterationBranch),
            ("merge", "合并请求", ManageSection::MergeRequests),
            ("branch", "分支管理", ManageSection::BranchManage),
            ("mock", "Mock 服务", ManageSection::MockService),
            ("public", "公共资源维护", ManageSection::PublicResources),
            ("iface-props", "接口属性", ManageSection::IfaceProps),
            ("iface-status", "接口状态", ManageSection::IfaceStatus),
            ("external", "对外能力", ManageSection::ExternalCaps),
            ("io", "导入 / 导出", ManageSection::ImportExport),
        ]
    }
}

/// Events the management panel can emit upward (consumed by VerveApp).
#[derive(Clone, Debug)]
pub enum ManageEvent {
    /// User clicked "import collection".
    Import,
    /// User clicked one of the export buttons.
    Export(crate::export::Format),
    /// User clicked "一键生成 Mock" — create default mock rules for requests that lack them.
    GenerateMocks,
}

pub struct ProjectManagePanel {
    pub git: Entity<GitState>,
    pub state: Entity<AppState>,
    /// Active section in the secondary nav.
    pub active_section: ManageSection,
    /// Queued input dialog (commit message / new branch / remote / auth / ...).
    pub pending: Option<PendingInput>,
    // --- inputs reused across dialogs ---
    pub token_input: Entity<InputState>,
    pub username_input: Entity<InputState>,
    pub remote_input: Entity<InputState>,
    pub message_input: Entity<InputState>,
    pub branch_input: Entity<InputState>,
    // --- basic-settings inputs ---
    pub project_name_input: Entity<InputState>,
    pub project_desc_input: Entity<InputState>,
    // --- merge-request dialog inputs ---
    pub mr_title_input: Entity<InputState>,
    pub mr_source_input: Entity<InputState>,
    pub mr_target_input: Entity<InputState>,
    // --- status-code dialog inputs ---
    pub sc_code_input: Entity<InputState>,
    pub sc_name_input: Entity<InputState>,
    pub sc_desc_input: Entity<InputState>,
    // --- api-token dialog input ---
    pub token_label_input: Entity<InputState>,
    /// Reseed inputs from git/state on the next render.
    pub pending_reseed: bool,
    /// Reseed basic-settings inputs when the active project changes.
    pub pending_project_reseed: bool,
    focus_handle: FocusHandle,
    _subs: Vec<gpui::Subscription>,
}

/// Which input dialog is queued for opening (needs a Window, opened in render).
#[derive(Clone, Debug)]
pub enum PendingInput {
    Commit,
    NewBranch,
    EditRemote,
    EditAuth,
    /// Create a merge request (title / source / target pre-filled from inputs).
    NewMergeRequest,
    /// Add a status-code dictionary entry.
    NewStatusCode,
    /// Create an API token (label).
    NewApiToken,
}

impl ProjectManagePanel {
    pub fn new(
        git: Entity<GitState>,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub_git = cx.subscribe(&git, |this, _src, _ev: &GitEvent, cx| {
            this.pending_reseed = true;
            cx.notify();
        });
        // Reseed basic-settings when the active project changes.
        let sub_state = cx.subscribe(&state, |this, _src, ev: &AppEvent, cx| {
            if matches!(ev, AppEvent::WorkspaceChanged) {
                this.pending_project_reseed = true;
                cx.notify();
            }
        });

        let token_input = cx.new(|cx| InputState::new(window, cx).placeholder("access token"));
        let username_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("用户名（默认 git）"));
        let remote_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/..."));
        let message_input = cx.new(|cx| InputState::new(window, cx).placeholder("提交信息"));
        let branch_input = cx.new(|cx| InputState::new(window, cx).placeholder("分支名"));
        let project_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("项目名称"));
        let project_desc_input = cx.new(|cx| InputState::new(window, cx).placeholder("项目描述"));
        // Merge-request dialog inputs.
        let mr_title_input = cx.new(|cx| InputState::new(window, cx).placeholder("合并请求标题"));
        let mr_source_input = cx.new(|cx| InputState::new(window, cx).placeholder("源分支"));
        let mr_target_input = cx.new(|cx| InputState::new(window, cx).placeholder("目标分支"));
        // Status-code dialog inputs.
        let sc_code_input = cx.new(|cx| InputState::new(window, cx).placeholder("如 200"));
        let sc_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("如 OK"));
        let sc_desc_input = cx.new(|cx| InputState::new(window, cx).placeholder("状态码说明"));
        // API token dialog input.
        let token_label_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("备注名，如 CI 同步"));

        let mut panel = Self {
            git,
            state,
            active_section: ManageSection::BasicSettings,
            pending: None,
            token_input,
            username_input,
            remote_input,
            message_input,
            branch_input,
            project_name_input,
            project_desc_input,
            mr_title_input,
            mr_source_input,
            mr_target_input,
            sc_code_input,
            sc_name_input,
            sc_desc_input,
            token_label_input,
            pending_reseed: true,
            pending_project_reseed: true,
            focus_handle: cx.focus_handle(),
            _subs: vec![sub_git, sub_state],
        };
        panel.reseed_git_inputs(window, cx);
        panel.reseed_project_inputs(window, cx);
        panel
    }

    fn reseed_git_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.git.read(cx).auth.token.clone();
        let username = self.git.read(cx).auth.username.clone();
        let remote = self.git.read(cx).remote.clone().unwrap_or_default();
        self.token_input
            .update(cx, |s, cx| s.set_value(token, window, cx));
        self.username_input
            .update(cx, |s, cx| s.set_value(username, window, cx));
        self.remote_input
            .update(cx, |s, cx| s.set_value(remote, window, cx));
    }

    fn reseed_project_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, desc) = self
            .state
            .read(cx)
            .active_project()
            .map(|p| (p.name.clone(), p.description.clone()))
            .unwrap_or_default();
        self.project_name_input
            .update(cx, |s, cx| s.set_value(name, window, cx));
        self.project_desc_input
            .update(cx, |s, cx| s.set_value(desc, window, cx));
    }

    /// Persist the current git config fields back to layout.json.
    fn persist_git_config(&self, cx: &mut Context<Self>) {
        let g = self.git.read(cx);
        let cfg = GitConfig {
            auto_commit: g.auto_commit,
            auto_push: g.auto_push,
            remote: g.remote.clone(),
            username: g.auth.username.clone(),
            token: g.auth.token.clone(),
            sync_interval_minutes: persistence::load_sync_interval_minutes().into(),
        };
        persistence::save_git_config(&cfg);
        let _ = cx;
    }

    fn open_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            PendingInput::Commit => {
                let git = self.git.clone();
                let input = self.message_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let input_c = input.clone();
                    let input_f = input.clone();
                    let git_f = git.clone();
                    dialog
                        .title("提交更改")
                        .w(px(480.))
                        .content(move |content, _w, _cx| {
                            content.child(v_flex().p_4().w_full().child(Input::new(&input_c)))
                        })
                        .footer(
                            Button::new("ok-commit")
                                .primary()
                                .small()
                                .label("提交")
                                .on_click(move |_, window, cx| {
                                    let msg = input_f.read(cx).value().to_string();
                                    let msg = if msg.trim().is_empty() {
                                        "Verve 提交".to_string()
                                    } else {
                                        msg
                                    };
                                    let _ = git_f.update(cx, |g, cx| g.commit_async(msg, cx));
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
            PendingInput::NewBranch => {
                let git = self.git.clone();
                let input = self.branch_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let input_c = input.clone();
                    let input_f = input.clone();
                    let git_f = git.clone();
                    dialog
                        .title("新建分支")
                        .w(px(480.))
                        .content(move |content, _w, _cx| {
                            content.child(v_flex().p_4().w_full().child(Input::new(&input_c)))
                        })
                        .footer(
                            Button::new("ok-branch")
                                .primary()
                                .small()
                                .label("创建并切换")
                                .on_click(move |_, window, cx| {
                                    let name = input_f.read(cx).value().to_string();
                                    if !name.trim().is_empty() {
                                        let _ = git_f
                                            .update(cx, |g, cx| g.create_branch_async(name, cx));
                                    }
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
            PendingInput::EditRemote => {
                let git = self.git.clone();
                let panel = cx.entity().downgrade();
                let input = self.remote_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let input_c = input.clone();
                    let input_f = input.clone();
                    let git_f = git.clone();
                    let panel_f = panel.clone();
                    dialog
                        .title("远程仓库")
                        .w(px(520.))
                        .content(move |content, _w, _cx| {
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w_full()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(gpui::rgb(0xcccccc))
                                            .child("origin URL（HTTPS，可嵌入 token）"),
                                    )
                                    .child(Input::new(&input_c)),
                            )
                        })
                        .footer(
                            Button::new("ok-remote")
                                .primary()
                                .small()
                                .label("保存")
                                .on_click(move |_, window, cx| {
                                    let url = input_f.read(cx).value().to_string();
                                    if !url.trim().is_empty() {
                                        let _ =
                                            git_f.update(cx, |g, cx| g.set_remote_async(url, cx));
                                        let _ = panel_f
                                            .update(cx, |this, cx| this.persist_git_config(cx));
                                    }
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
            PendingInput::EditAuth => {
                let git = self.git.clone();
                let panel = cx.entity().downgrade();
                let user = self.username_input.clone();
                let tok = self.token_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let user_c = user.clone();
                    let user_f = user.clone();
                    let tok_c = tok.clone();
                    let tok_f = tok.clone();
                    let git_f = git.clone();
                    let panel_f = panel.clone();
                    dialog
                        .title("认证配置")
                        .w(px(520.))
                        .content(move |content, _w, _cx| {
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w_full()
                                    .gap_3()
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_size(px(12.)).child("用户名"))
                                            .child(Input::new(&user_c)),
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_size(px(12.)).child("Access Token"))
                                            .child(Input::new(&tok_c)),
                                    ),
                            )
                        })
                        .footer(
                            Button::new("ok-auth")
                                .primary()
                                .small()
                                .label("保存")
                                .on_click(move |_, window, cx| {
                                    let username = user_f.read(cx).value().to_string();
                                    let token = tok_f.read(cx).value().to_string();
                                    let _ = git_f.update(cx, |g, cx| {
                                        g.auth.username = username;
                                        g.auth.token = token;
                                        cx.notify();
                                    });
                                    let _ =
                                        panel_f.update(cx, |this, cx| this.persist_git_config(cx));
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
            PendingInput::NewMergeRequest => {
                let state = self.state.clone();
                let title = self.mr_title_input.clone();
                let source = self.mr_source_input.clone();
                let target = self.mr_target_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let title_c = title.clone();
                    let source_c = source.clone();
                    let target_c = target.clone();
                    let title_f = title.clone();
                    let source_f = source.clone();
                    let target_f = target.clone();
                    let state_f = state.clone();
                    dialog
                        .title("新建合并请求")
                        .w(px(520.))
                        .content(move |content, _w, _cx| {
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w_full()
                                    .gap_3()
                                    .child(field("标题", Input::new(&title_c)))
                                    .child(field("源分支", Input::new(&source_c)))
                                    .child(field("目标分支", Input::new(&target_c))),
                            )
                        })
                        .footer(
                            Button::new("ok-mr")
                                .primary()
                                .small()
                                .label("创建")
                                .on_click(move |_, window, cx| {
                                    let title = title_f.read(cx).value().to_string();
                                    let source = source_f.read(cx).value().to_string();
                                    let target = target_f.read(cx).value().to_string();
                                    if !title.trim().is_empty() {
                                        let source = if source.trim().is_empty() {
                                            "feature".to_string()
                                        } else {
                                            source
                                        };
                                        let target = if target.trim().is_empty() {
                                            "main".to_string()
                                        } else {
                                            target
                                        };
                                        let _ = state_f.update(cx, |s, cx| {
                                            if let Some(p) = s.active_project_mut() {
                                                p.merge_requests
                                                    .push(MergeRequest::new(title, source, target));
                                            }
                                            s.notify_workspace(cx);
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
            PendingInput::NewStatusCode => {
                let state = self.state.clone();
                let code = self.sc_code_input.clone();
                let name = self.sc_name_input.clone();
                let desc = self.sc_desc_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let code_c = code.clone();
                    let name_c = name.clone();
                    let desc_c = desc.clone();
                    let code_f = code.clone();
                    let name_f = name.clone();
                    let desc_f = desc.clone();
                    let state_f = state.clone();
                    dialog
                        .title("新增状态码")
                        .w(px(520.))
                        .content(move |content, _w, _cx| {
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w_full()
                                    .gap_3()
                                    .child(field("状态码", Input::new(&code_c)))
                                    .child(field("名称", Input::new(&name_c)))
                                    .child(field("描述", Input::new(&desc_c))),
                            )
                        })
                        .footer(
                            Button::new("ok-sc")
                                .primary()
                                .small()
                                .label("添加")
                                .on_click(move |_, window, cx| {
                                    let code_raw = code_f.read(cx).value().to_string();
                                    let name = name_f.read(cx).value().to_string();
                                    let desc = desc_f.read(cx).value().to_string();
                                    if let Ok(c) = code_raw.trim().parse::<u16>() {
                                        let _ = state_f.update(cx, |s, cx| {
                                            if let Some(p) = s.active_project_mut() {
                                                p.status_codes.push(StatusCodeEntry {
                                                    code: c,
                                                    name: if name.trim().is_empty() {
                                                        c.to_string()
                                                    } else {
                                                        name
                                                    },
                                                    description: desc,
                                                });
                                            }
                                            s.notify_workspace(cx);
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
            PendingInput::NewApiToken => {
                let state = self.state.clone();
                let label = self.token_label_input.clone();
                window.open_dialog(cx, move |dialog, _w, _cx| {
                    let label_c = label.clone();
                    let label_f = label.clone();
                    let state_f = state.clone();
                    dialog
                        .title("新建 API Token")
                        .w(px(520.))
                        .content(move |content, _w, _cx| {
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w_full()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(gpui::rgb(0xcccccc))
                                            .child("为该 token 取一个备注名以便区分用途"),
                                    )
                                    .child(Input::new(&label_c)),
                            )
                        })
                        .footer(
                            Button::new("ok-token")
                                .primary()
                                .small()
                                .label("创建")
                                .on_click(move |_, window, cx| {
                                    let label = label_f.read(cx).value().to_string();
                                    let label = if label.trim().is_empty() {
                                        "默认".to_string()
                                    } else {
                                        label
                                    };
                                    let _ = state_f.update(cx, |s, cx| {
                                        if let Some(p) = s.active_project_mut() {
                                            p.api_tokens.push(ApiToken::new(label));
                                        }
                                        s.notify_workspace(cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                });
            }
        }
    }

    /// Save the basic-settings (project name + description) back to AppState.
    fn _save_basic_settings(&mut self, cx: &mut Context<Self>) {
        let name = self.project_name_input.read(cx).value().to_string();
        let desc = self.project_desc_input.read(cx).value().to_string();
        self.state.update(cx, |s, cx| {
            if let Some(p) = s.active_project_mut() {
                p.name = name;
                p.description = desc;
            }
            s.notify_workspace(cx);
        });
    }
}

impl EventEmitter<ManageEvent> for ProjectManagePanel {}

impl Render for ProjectManagePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_reseed {
            self.pending_reseed = false;
            self.reseed_git_inputs(window, cx);
        }
        if self.pending_project_reseed {
            self.pending_project_reseed = false;
            self.reseed_project_inputs(window, cx);
        }
        if self.pending.is_some() {
            self.open_pending(window, cx);
        }

        let theme = cx.theme().clone();
        let active = self.active_section;

        h_flex()
            .size_full()
            .bg(theme.background)
            .overflow_hidden()
            // ---- Secondary nav (left) ----
            .child(self.render_secondary_nav(active, &theme, cx))
            // ---- Section content (right) ----
            .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(
                match self.active_section {
                    ManageSection::BasicSettings => {
                        self.render_basic_settings(&theme, cx).into_any_element()
                    }
                    ManageSection::IterationBranch => {
                        self.render_iteration_branch(&theme, cx).into_any_element()
                    }
                    ManageSection::MergeRequests => {
                        self.render_merge_requests(&theme, cx).into_any_element()
                    }
                    ManageSection::BranchManage => {
                        self.render_branch_manage(&theme, cx).into_any_element()
                    }
                    ManageSection::MockService => {
                        self.render_mock_service(&theme, cx).into_any_element()
                    }
                    ManageSection::PublicResources => {
                        self.render_public_resources(&theme, cx).into_any_element()
                    }
                    ManageSection::IfaceProps => {
                        self.render_iface_props(&theme, cx).into_any_element()
                    }
                    ManageSection::IfaceStatus => {
                        self.render_iface_status(&theme, cx).into_any_element()
                    }
                    ManageSection::ExternalCaps => {
                        self.render_external_caps(&theme, cx).into_any_element()
                    }
                    ManageSection::ImportExport => {
                        self.render_import_export(&theme, cx).into_any_element()
                    }
                },
            ))
    }
}

impl ProjectManagePanel {
    fn render_secondary_nav(
        &mut self,
        active: ManageSection,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let ent = cx.entity();
        v_flex()
            .w(px(200.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .py_2()
            .gap(px(1.))
            .children(ManageSection::all().iter().map(|(id, label, section)| {
                let is_active = *section == active;
                let ent = ent.clone();
                div()
                    .id(format!("nav-{}", id))
                    .w_full()
                    .px_3()
                    .py(px(7.))
                    .text_size(px(13.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(if is_active {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .when(is_active, |d| {
                        d.bg(theme.primary.opacity(0.18))
                            .font_weight(FontWeight::SEMIBOLD)
                            .border_l_2()
                            .border_color(theme.primary)
                    })
                    .hover(|d| d.bg(theme.accent.opacity(0.25)))
                    .child(label.to_string())
                    .on_click(move |_, _w, _cx: &mut App| {
                        let _ = ent.update(_cx, |this, cx| {
                            this.active_section = *section;
                            cx.notify();
                        });
                    })
            }))
    }

    // -----------------------------------------------------------------
    // Section: 基本设置 (Basic Settings)
    // -----------------------------------------------------------------
    fn render_basic_settings(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let name_input = self.project_name_input.clone();
        let desc_input = self.project_desc_input.clone();
        let ent = cx.entity();
        let project_id = self
            .state
            .read(cx)
            .active_project()
            .map(|p| p.id.clone())
            .unwrap_or_default();

        v_flex()
            .size_full()
            .id("basic-settings-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(640.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("基本设置"),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_size(px(13.)).child("项目名称"))
                            .child(Input::new(&name_input)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_size(px(13.)).child("项目描述"))
                            .child(Input::new(&desc_input)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_size(px(13.)).child("项目 ID"))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(project_id),
                            ),
                    )
                    .child(
                        h_flex().gap_2().child(
                            Button::new("save-basic")
                                .primary()
                                .small()
                                .label("保存")
                                .on_click(move |_, _w, cx: &mut App| {
                                    let _ = ent.update(cx, |this, cx| {
                                        let name =
                                            this.project_name_input.read(cx).value().to_string();
                                        let desc =
                                            this.project_desc_input.read(cx).value().to_string();
                                        this.state.update(cx, |s, cx| {
                                            if let Some(p) = s.active_project_mut() {
                                                p.name = name;
                                                p.description = desc;
                                            }
                                            s.notify_workspace(cx);
                                        });
                                        cx.notify();
                                    });
                                }),
                        ),
                    ),
            )
    }

    // -----------------------------------------------------------------
    // Section: 分支管理 (Branch Management) — the Git surface
    // -----------------------------------------------------------------
    fn render_branch_manage(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let g = self.git.read(cx);
        let initialized = g.initialized;
        let busy = g.busy.clone();
        let branch = g.status.branch.clone();
        let dirty = g.status.dirty;
        let ahead = g.status.ahead;
        let auto_commit = g.auto_commit;
        let auto_push = g.auto_push;
        let remote = g.remote.clone();
        let auth_set = !g.auth.token.is_empty();
        let last_result = g.last_result.clone();
        let commits = g.commits.clone();
        let branches = g.branches.clone();

        // Clone-per-closure handles (Entity isn't Copy).
        let git_pull = self.git.clone();
        let git_push = self.git.clone();
        let git_sync = self.git.clone();
        let git_refresh = self.git.clone();
        let git_checkout = self.git.clone();
        let git_init = self.git.clone();
        let git_branch_toggle = self.git.clone();
        let git_auto_push = self.git.clone();
        let git_delete = self.git.clone();
        let panel_ent = cx.entity();

        let busy_disabled = busy.is_some();

        let mut content = v_flex().size_full().overflow_hidden();

        // Not initialised: show the init CTA instead of the rest.
        if !initialized {
            return content
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("尚未初始化 Git 仓库"),
                )
                .child(
                    Button::new("git-init")
                        .primary()
                        .label("初始化 Git 仓库")
                        .icon(IconName::Github)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let _ = git_init.update(cx, |g, cx| g.init_repo_async(cx));
                        })),
                )
                .into_any_element();
        }

        content = content
            // Header
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("分支管理"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} · {} 待提交{}",
                                branch.as_deref().unwrap_or("—"),
                                dirty,
                                if ahead > 0 {
                                    format!(" · {} 待推送", ahead)
                                } else {
                                    String::new()
                                }
                            )),
                    )
                    .when_some(busy, |col, label| {
                        col.child(
                            div()
                                .text_size(px(12.))
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .bg(theme.accent.opacity(0.3))
                                .child(label),
                        )
                    }),
            )
            // Action buttons
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_2()
                    .flex_wrap()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("act-commit")
                            .small()
                            .ghost()
                            .label("提交")
                            .icon(IconName::ArrowUp)
                            .disabled(busy_disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending = Some(PendingInput::Commit);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("act-pull")
                            .small()
                            .ghost()
                            .label("拉取")
                            .icon(IconName::ArrowDown)
                            .disabled(busy_disabled)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = git_pull.update(cx, |g, cx| g.pull_async(cx));
                            })),
                    )
                    .child(
                        Button::new("act-push")
                            .small()
                            .ghost()
                            .label("推送")
                            .icon(IconName::ArrowUp)
                            .disabled(busy_disabled)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = git_push.update(cx, |g, cx| g.push_async(cx));
                            })),
                    )
                    .child(
                        Button::new("act-sync")
                            .small()
                            .primary()
                            .label("同步")
                            .icon(IconName::Replace)
                            .disabled(busy_disabled)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = git_sync.update(cx, |g, cx| g.sync_async(None, cx));
                            })),
                    )
                    .child(
                        Button::new("act-refresh")
                            .small()
                            .ghost()
                            .icon(IconName::Redo)
                            .disabled(busy_disabled)
                            .tooltip("刷新")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = git_refresh.update(cx, |g, cx| g.refresh_async(cx));
                            })),
                    ),
            )
            // Auto-sync toggles
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_6()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(toggle_row(
                        "auto-commit",
                        "自动提交",
                        auto_commit,
                        theme.clone(),
                        {
                            let p = panel_ent.downgrade();
                            move |val: bool, cx: &mut App| {
                                let _ = git_branch_toggle.update(cx, |g, cx| {
                                    g.auto_commit = val;
                                    cx.notify();
                                });
                                let _ = p.update(cx, |this, cx| this.persist_git_config(cx));
                            }
                        },
                    ))
                    .child(toggle_row(
                        "auto-push",
                        "自动推送",
                        auto_push,
                        theme.clone(),
                        {
                            let p = panel_ent.downgrade();
                            move |val: bool, cx: &mut App| {
                                let _ = git_auto_push.update(cx, |g, cx| {
                                    g.auto_push = val;
                                    cx.notify();
                                });
                                let _ = p.update(cx, |this, cx| this.persist_git_config(cx));
                            }
                        },
                    )),
            )
            // Last result banner
            .when_some(last_result, |col, (msg, ok)| {
                col.child(
                    div()
                        .mx_6()
                        .my_2()
                        .px_3()
                        .py(px(8.))
                        .text_size(px(12.))
                        .rounded(px(4.))
                        .bg(if ok {
                            theme.accent.opacity(0.2)
                        } else {
                            theme.danger.opacity(0.2)
                        })
                        .text_color(if ok { theme.foreground } else { theme.danger })
                        .child(msg),
                )
            });

        // Two columns: branch list + config (left), commit history (right).
        let branches_col = v_flex()
            .p_6()
            .gap_3()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("分支"),
            )
            .child(
                v_flex()
                    .id("branch-list-scroll")
                    .gap(px(1.))
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .children(branches.iter().map(|b| {
                        let b = b.clone();
                        let is_active = Some(&b) == branch.as_ref();
                        let git_e = git_checkout.clone();
                        let git_del = git_delete.clone();
                        let b_del = b.clone();
                        let theme_c = theme.clone();
                        div()
                            .id(format!("br-{}", b))
                            .w_full()
                            .px(px(8.))
                            .py(px(4.))
                            .text_size(px(12.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .when(is_active, |d| {
                                d.bg(theme.primary.opacity(0.2))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(|d| d.bg(theme.accent.opacity(0.3)))
                            .child(IconName::Github)
                            .child(div().flex_1().child(b.clone()))
                            .when(is_active, |d| d.child(IconName::Check))
                            // Delete button — hidden for the active branch (can't
                            // delete the checked-out branch without extra steps).
                            .when(!is_active, |d| {
                                d.child(
                                    div()
                                        .id(format!("br-del-{}", b_del))
                                        .cursor_pointer()
                                        .text_color(theme_c.muted_foreground)
                                        .hover(|h| h.text_color(theme_c.danger))
                                        .child(IconName::Delete)
                                        .on_click(move |_, _w, cx| {
                                            let name = b_del.clone();
                                            let _ = git_del.update(cx, |g, cx| {
                                                g.delete_branch_async(name, cx)
                                            });
                                        }),
                                )
                            })
                            .on_click(move |_, _w, cx| {
                                if !is_active {
                                    let _ =
                                        git_e.update(cx, |g, cx| g.checkout_async(b.clone(), cx));
                                }
                            })
                    })),
            )
            .child(
                Button::new("branch-new")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label("新建分支")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pending = Some(PendingInput::NewBranch);
                        cx.notify();
                    })),
            )
            .child(div().h(px(1.)).w_full().bg(theme.border))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("远程 / 认证"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .w(px(48.))
                            .text_color(theme.muted_foreground)
                            .child("远程"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(remote.clone().unwrap_or_else(|| "未配置".to_string())),
                    )
                    .child(
                        Button::new("remote-edit")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .tooltip("编辑远程")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending = Some(PendingInput::EditRemote);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .w(px(48.))
                            .text_color(theme.muted_foreground)
                            .child("认证"),
                    )
                    .child(div().flex_1().text_size(px(12.)).child(if auth_set {
                        "已配置"
                    } else {
                        "未配置"
                    }))
                    .child(
                        Button::new("auth-edit")
                            .ghost()
                            .small()
                            .icon(IconName::CircleUser)
                            .tooltip("配置 Token")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending = Some(PendingInput::EditAuth);
                                cx.notify();
                            })),
                    ),
            );

        let history_col = v_flex()
            .p_6()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("提交历史"),
            )
            .when(commits.is_empty(), |c| {
                c.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child("暂无提交"),
                )
            })
            .child(
                v_flex()
                    .id("commit-history-scroll")
                    .gap_1()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(commits.iter().enumerate().map(|(i, c)| {
                        let date = chrono::DateTime::from_timestamp(c.time, 0)
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default();
                        v_flex()
                            .id(format!("commit-{}", i))
                            .w_full()
                            .px_2()
                            .py(px(6.))
                            .gap(px(2.))
                            .rounded(px(4.))
                            .hover(|d| d.bg(theme.muted))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.primary)
                                            .child(c.short_id.clone()),
                                    )
                                    .child(
                                        div().flex_1().text_size(px(13.)).child(c.message.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(div().child(c.author.clone()))
                                    .child(div().flex_1())
                                    .child(div().child(date)),
                            )
                    })),
            );

        content = content.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(
                    v_flex()
                        .w(px(320.))
                        .flex_shrink_0()
                        .h_full()
                        .overflow_hidden()
                        .child(branches_col),
                )
                .child(history_col),
        );

        content.into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: 导入 / 导出
    // -----------------------------------------------------------------
    fn render_import_export(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let ent_import = cx.entity();
        let ent_md = cx.entity();
        let ent_json = cx.entity();
        let ent_apipost = cx.entity();
        let ent_pm21 = cx.entity();
        let ent_swagger = cx.entity();
        let ent_openapi = cx.entity();
        v_flex()
            .size_full()
            .id("io-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(640.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("导入 / 导出"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("导入 Postman v2.1 / Swagger / OpenAPI 3 集合，或将当前工作空间导出为 Markdown / JSON / Apipost / Postman v2.1 / Swagger / OpenAPI 3。"),
                    )
                    .child(
                        Button::new("io-import")
                            .primary()
                            .small()
                            .icon(vicon(IMPORT))
                            .label("导入集合")
                            .on_click(move |_, _w, cx: &mut App| {
                                let _ = ent_import.update(cx, |_, cx| cx.emit(ManageEvent::Import));
                            }),
                    )
                    .child(div().h(px(1.)).w_full().bg(theme.border))
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("io-export-md")
                                    .ghost()
                                    .small()
                                    .icon(vicon(EXPORT))
                                    .label("导出 Markdown")
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_md.update(cx, |_, cx| {
                                            cx.emit(ManageEvent::Export(crate::export::Format::Markdown))
                                        });
                                    }),
                            )
                            .child(
                                Button::new("io-export-json")
                                    .ghost()
                                    .small()
                                    .icon(vicon(EXPORT))
                                    .label("导出 JSON")
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_json.update(cx, |_, cx| {
                                            cx.emit(ManageEvent::Export(crate::export::Format::Json))
                                        });
                                    }),
                            )
                            .child(
                                Button::new("io-export-apipost")
                                    .ghost()
                                    .small()
                                    .icon(vicon(EXPORT))
                                    .label("导出 Apipost")
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_apipost.update(cx, |_, cx| {
                                            cx.emit(ManageEvent::Export(crate::export::Format::Apipost))
                                        });
                                    }),
                            )
                            .child(
                                Button::new("io-export-pm21")
                                    .ghost()
                                    .small()
                                    .icon(vicon(EXPORT))
                                    .label("导出 Postman v2.1")
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_pm21.update(cx, |_, cx| {
                                            cx.emit(ManageEvent::Export(crate::export::Format::PostmanV2_1))
                                        });
                                    }),
                            )
                            .child(
                                Button::new("io-export-swagger")
                                    .ghost()
                                    .small()
                                    .icon(vicon(EXPORT))
                                    .label("导出 Swagger 2.0")
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_swagger.update(cx, |_, cx| {
                                            cx.emit(ManageEvent::Export(crate::export::Format::Swagger))
                                        });
                                    }),
                            )
                            .child(
                                Button::new("io-export-openapi")
                                    .ghost()
                                    .small()
                                    .icon(vicon(EXPORT))
                                    .label("导出 OpenAPI 3")
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_openapi.update(cx, |_, cx| {
                                            cx.emit(ManageEvent::Export(crate::export::Format::OpenApi3))
                                        });
                                    }),
                            ),
                    ),
            )
    }

    // -----------------------------------------------------------------
    // Section: 迭代分支 (Iteration / Branches overview)
    // -----------------------------------------------------------------
    fn render_iteration_branch(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let g = self.git.read(cx);
        let initialized = g.initialized;
        let current = g.status.branch.clone();
        let branches = g.branches.clone();
        let dirty = g.status.dirty;

        let mut body = v_flex()
            .size_full()
            .id("iter-scroll")
            .overflow_y_scroll()
            .child(
            v_flex()
                .p_6()
                .gap_5()
                .max_w(px(820.))
                .child(section_title("迭代分支"))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(
                            "按分支组织迭代。每条分支对应一次开发迭代，可在此查看、创建与切换。",
                        ),
                )
                .child(
                    h_flex().gap_2().child(
                        Button::new("iter-new")
                            .primary()
                            .small()
                            .icon(IconName::Plus)
                            .label("新建迭代分支")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pending = Some(PendingInput::NewBranch);
                                cx.notify();
                            })),
                    ),
                )
                .child(div().h(px(1.)).w_full().bg(theme.border)),
        );

        if !initialized {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("尚未初始化 Git 仓库，无法读取分支。请先到「分支管理」初始化。"),
            );
            return body.into_any_element();
        }

        body = body.child(v_flex().gap(px(1.)).children(branches.iter().map(|b| {
            let b = b.clone();
            let is_active = Some(&b) == current.as_ref();
            let badge = if is_active { "当前" } else { "" };
            let dirty_label = if is_active && dirty > 0 {
                format!("{} 处未提交", dirty)
            } else {
                String::new()
            };
            h_flex()
                .id(format!("iter-br-{}", b))
                .w_full()
                .px_3()
                .py(px(8.))
                .gap_2()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .when(is_active, |d| {
                    d.border_color(theme.primary)
                        .bg(theme.primary.opacity(0.12))
                })
                .when(!is_active, |d| d.border_color(theme.border))
                .hover(|d| d.bg(theme.muted))
                .child(IconName::Github)
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(b.clone()),
                )
                .when(is_active, |d| d.child(badge_chip(badge, &theme)))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(dirty_label),
                )
        })));

        body.into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: 合并请求 (Merge Requests)
    // -----------------------------------------------------------------
    fn render_merge_requests(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let mrs = self
            .state
            .read(cx)
            .active_project()
            .map(|p| p.merge_requests.clone())
            .unwrap_or_default();

        v_flex()
            .size_full()
            .id("mr-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(900.))
                    .child(
                        h_flex()
                            .items_center()
                            .child(section_title("合并请求"))
                            .child(div().flex_1())
                            .child(
                                Button::new("mr-new")
                                    .primary()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("新建合并请求")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pending = Some(PendingInput::NewMergeRequest);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(mr_table(&mrs, &theme, self.state.clone())),
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: Mock 服务 (Mock Service)
    // -----------------------------------------------------------------
    fn render_mock_service(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        // Scan every request; collect those with an enabled mock rule.
        let mocked: Vec<(String, String, String, u16)> = self
            .state
            .read(cx)
            .active_project()
            .map(|p| {
                p.iter_all_requests()
                    .into_iter()
                    .filter_map(|(path, req)| {
                        req.mock.as_ref().filter(|m| m.enabled).map(|m| {
                            (
                                if path.is_empty() {
                                    req.name.clone()
                                } else {
                                    path
                                },
                                req.url.clone(),
                                req.name.clone(),
                                m.status,
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        v_flex()
            .size_full()
            .id("mock-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(900.))
                    .child(section_title("Mock 服务"))
                    .child(
                        div().text_size(px(12.)).text_color(theme.muted_foreground).child(
                            "为接口开启 Mock 后，本地 Mock 服务器会按规则返回响应。下方列出当前已启用 Mock 的接口。",
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .px(px(8.))
                                    .py(px(3.))
                                    .rounded(px(4.))
                                    .bg(theme.primary.opacity(0.18))
                                    .child(format!("已启用 {} 条", mocked.len())),
                            )
                            .child(
                                Button::new("mock-generate")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .label("一键生成 Mock")
                                    .tooltip("为所有还没有 Mock 规则的接口生成默认规则")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        cx.emit(ManageEvent::GenerateMocks);
                                        let _ = this;
                                    })),
                            ),
                    )
                    .child(div().h(px(1.)).w_full().bg(theme.border))
                    .when(mocked.is_empty(), |c| {
                        c.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child("暂无已启用 Mock 的接口。在接口编辑器的 Mock 标签页开启即可。"),
                        )
                    })
                    .child(
                        v_flex()
                            .gap(px(1.))
                            .children(mocked.iter().enumerate().map(|(i, (loc, url, name, status))| {
                                mock_row(i, loc, url, name, *status, &theme)
                            })),
                    ),
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: 公共资源维护 (Public Resources — status code dictionary)
    // -----------------------------------------------------------------
    fn render_public_resources(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let codes = self
            .state
            .read(cx)
            .active_project()
            .map(|p| p.status_codes.clone())
            .unwrap_or_default();

        v_flex()
            .size_full()
            .id("public-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(900.))
                    .child(
                        h_flex()
                            .items_center()
                            .child(section_title("状态码字典"))
                            .child(div().flex_1())
                            .child(
                                Button::new("sc-new")
                                    .primary()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("新增状态码")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pending = Some(PendingInput::NewStatusCode);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(
                                "项目级共享的 HTTP 状态码字典，可在响应文档与 Mock 规则中引用。",
                            ),
                    )
                    .child(status_code_table(&codes, &theme, self.state.clone())),
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: 接口属性 (Interface properties — flat table of all requests)
    // -----------------------------------------------------------------
    fn render_iface_props(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let rows: Vec<(String, String, RequestMethod, String, String, Vec<String>)> = self
            .state
            .read(cx)
            .active_project()
            .map(|p| {
                p.iter_all_requests()
                    .into_iter()
                    .map(|(path, req)| {
                        (
                            path,
                            req.name.clone(),
                            req.method,
                            req.url.clone(),
                            req.status.clone(),
                            req.tags.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let header_labels = ["所属目录", "接口名称", "方法", "请求路径", "状态", "标签"];
        let header_widths = [px(140.), px(160.), px(72.), px(220.), px(90.), px(120.)];
        let header = table_header(&header_labels, &header_widths, &theme);

        v_flex()
            .size_full()
            .id("iface-props-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_4()
                    .max_w(px(1100.))
                    .child(section_title("接口属性"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "共 {} 个接口。展示其所属目录、方法、路径与状态。",
                                rows.len()
                            )),
                    )
                    .child(div().h(px(1.)).w_full().bg(theme.border))
                    .child(header)
                    .child(
                        v_flex().gap(px(1.)).children(
                            rows.iter()
                                .enumerate()
                                .map(|(i, r)| iface_props_row(i, r, &theme)),
                        ),
                    ),
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: 接口状态 (Interface status — grouped statistics)
    // -----------------------------------------------------------------
    fn render_iface_status(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        // Group all requests by their status label (empty → "未设置").
        let mut groups: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        if let Some(p) = self.state.read(cx).active_project() {
            for (_path, req) in p.iter_all_requests() {
                let key = if req.status.trim().is_empty() {
                    "未设置".to_string()
                } else {
                    req.status.clone()
                };
                groups.entry(key).or_default().push(req.name.clone());
            }
        }
        let total: usize = groups.values().map(|v| v.len()).sum();

        v_flex()
            .size_full()
            .id("iface-status-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(900.))
                    .child(section_title("接口状态"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "按状态聚合统计。共 {} 个接口，分布于 {} 个状态。",
                                total,
                                groups.len()
                            )),
                    )
                    .child(
                        h_flex().gap_3().flex_wrap().children(
                            groups
                                .iter()
                                .map(|(status, names)| status_card(status, names.len(), &theme)),
                        ),
                    )
                    .child(div().h(px(1.)).w_full().bg(theme.border))
                    .child(
                        v_flex()
                            .gap_4()
                            .children(groups.iter().map(|(status, names)| {
                                v_flex()
                                    .gap(px(1.))
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(status.clone()),
                                            )
                                            .child(badge_chip(&names.len().to_string(), &theme)),
                                    )
                                    .child(v_flex().gap(px(1.)).children(names.iter().map(|n| {
                                        div()
                                            .px_3()
                                            .py(px(5.))
                                            .text_size(px(12.))
                                            .rounded(px(4.))
                                            .hover(|d| d.bg(theme.muted))
                                            .child(n.clone())
                                    })))
                            })),
                    ),
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------
    // Section: 对外能力 (External capabilities — OpenAPI + API tokens)
    // -----------------------------------------------------------------
    fn render_external_caps(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme.clone();
        let proj = self
            .state
            .read(cx)
            .active_project()
            .map(|p| (p.id.clone(), p.api_tokens.clone()))
            .unwrap_or_default();
        let (project_id, tokens) = proj;

        v_flex()
            .size_full()
            .id("ext-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_5()
                    .max_w(px(900.))
                    .child(section_title("OpenAPI"))
                    .child(
                        div().text_size(px(12.)).text_color(theme.muted_foreground).child(
                            "通过 Open API 可以访问您在 Verve 中的项目数据。使用时需要携带 API token，您可根据用途创建不同的 token。",
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("当前分支 ID :"))
                            .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(if project_id.len() >= 6 { project_id[..6].to_string() } else { project_id })),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .child(section_title("API Token"))
                            .child(div().flex_1())
                            .child(
                                Button::new("token-new")
                                    .primary()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("新建 Token")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pending = Some(PendingInput::NewApiToken);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(token_table(&tokens, &theme, self.state.clone())),
            )
            .into_any_element()
    }
}

// =================================================================
// Free-function table / row helpers used by the new sections.
// =================================================================

/// A labelled form field row used inside dialogs.
fn field(label: &str, input: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_size(px(12.)).child(label.to_string()))
        .child(input)
}

/// A consistent section title (h2-ish).
fn section_title(text: &str) -> impl IntoElement {
    div()
        .text_size(px(18.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.to_string())
}

/// A small coloured pill used for counts / "current" markers.
fn badge_chip(text: &str, theme: &gpui_component::Theme) -> impl IntoElement {
    div()
        .text_size(px(11.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(theme.primary.opacity(0.2))
        .text_color(theme.foreground)
        .child(text.to_string())
}

/// A status group card (used by 接口状态).
fn status_card(status: &str, count: usize, theme: &gpui_component::Theme) -> impl IntoElement {
    v_flex()
        .p_4()
        .min_w(px(160.))
        .gap_1()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme.primary)
                .child(count.to_string()),
        )
        .child(div().text_size(px(13.)).child(status.to_string()))
}

/// Header row for a flat table. `widths` must match the row cell widths.
fn table_header(
    labels: &[&str],
    widths: &[gpui::Pixels],
    theme: &gpui_component::Theme,
) -> impl IntoElement {
    let row = h_flex()
        .w_full()
        .px_3()
        .py(px(8.))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground);
    let mut row = row;
    for (i, label) in labels.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(px(120.));
        row = row.child(div().w(w).flex_shrink_0().child(label.to_string()));
    }
    row
}

/// One row of the merge-request table.
fn mr_table(
    mrs: &[MergeRequest],
    theme: &gpui_component::Theme,
    state_ent: Entity<AppState>,
) -> impl IntoElement {
    let widths = [px(220.), px(120.), px(120.), px(90.), px(140.)];
    let labels = ["标题", "源分支", "目标分支", "状态", "创建时间"];
    let header = table_header(&labels, &widths, theme);

    let body = if mrs.is_empty() {
        v_flex().child(
            div()
                .px_3()
                .py(px(16.))
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child("暂无合并请求。点击右上角「新建合并请求」创建。"),
        )
    } else {
        let mut col = v_flex().gap(px(1.));
        for (i, mr) in mrs.iter().enumerate() {
            let state = mr.state.clone();
            let state_ent = state_ent.clone();
            let mr_id = mr.id.clone();
            let cells = h_flex()
                .w_full()
                .px_3()
                .py(px(8.))
                .text_size(px(12.))
                .border_b_1()
                .border_color(theme.border)
                .hover(|d| d.bg(theme.muted))
                .child(
                    div()
                        .w(widths[0])
                        .flex_shrink_0()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(mr.title.clone()),
                )
                .child(
                    div()
                        .w(widths[1])
                        .flex_shrink_0()
                        .child(mr.source_branch.clone()),
                )
                .child(
                    div()
                        .w(widths[2])
                        .flex_shrink_0()
                        .child(mr.target_branch.clone()),
                )
                .child(
                    div()
                        .w(widths[3])
                        .flex_shrink_0()
                        .child(mr_state_chip(&state, theme)),
                )
                .child(
                    div()
                        .w(widths[4])
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground)
                        .child(mr.created_at.clone()),
                )
                .child(div().flex_1())
                .child(
                    Button::new(("mr-close", i))
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .tooltip("关闭")
                        .on_click(move |_, _w, cx: &mut App| {
                            let id = mr_id.clone();
                            let _ = state_ent.update(cx, |s, cx| {
                                if let Some(p) = s.active_project_mut() {
                                    if let Some(m) =
                                        p.merge_requests.iter_mut().find(|m| m.id == id)
                                    {
                                        m.state = "closed".to_string();
                                    }
                                }
                                s.notify_workspace(cx);
                            });
                        }),
                );
            col = col.child(cells);
        }
        col
    };

    v_flex().child(header).child(body)
}

/// Coloured chip for an MR lifecycle state.
fn mr_state_chip(state: &str, theme: &gpui_component::Theme) -> impl IntoElement {
    let (bg, fg) = match state {
        "merged" => (theme.accent.opacity(0.25), theme.foreground),
        "closed" => (theme.danger.opacity(0.25), theme.danger),
        _ => (theme.primary.opacity(0.25), theme.primary),
    };
    div()
        .text_size(px(11.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(bg)
        .text_color(fg)
        .child(state.to_string())
}

/// One row of the status-code dictionary table.
fn status_code_table(
    codes: &[StatusCodeEntry],
    theme: &gpui_component::Theme,
    state_ent: Entity<AppState>,
) -> impl IntoElement {
    let widths = [px(90.), px(140.), px(360.)];
    let labels = ["状态码", "名称", "描述"];
    let header = table_header(&labels, &widths, theme);

    let body = if codes.is_empty() {
        v_flex().child(
            div()
                .px_3()
                .py(px(16.))
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child("暂无状态码。"),
        )
    } else {
        let mut col = v_flex().gap(px(1.));
        for (i, c) in codes.iter().enumerate() {
            let code = c.code;
            let state_ent = state_ent.clone();
            let cells = h_flex()
                .w_full()
                .px_3()
                .py(px(7.))
                .text_size(px(12.))
                .border_b_1()
                .border_color(theme.border)
                .hover(|d| d.bg(theme.muted))
                .child(
                    div()
                        .w(widths[0])
                        .flex_shrink_0()
                        .child(format!("{}", code)),
                )
                .child(div().w(widths[1]).flex_shrink_0().child(c.name.clone()))
                .child(
                    div()
                        .w(widths[2])
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground)
                        .child(c.description.clone()),
                )
                .child(div().flex_1())
                .child(
                    Button::new(("sc-del", i))
                        .ghost()
                        .small()
                        .icon(IconName::Delete)
                        .tooltip("删除")
                        .on_click(move |_, _w, cx: &mut App| {
                            let _ = state_ent.update(cx, |s, cx| {
                                if let Some(p) = s.active_project_mut() {
                                    p.status_codes.retain(|x| x.code != code);
                                }
                                s.notify_workspace(cx);
                            });
                        }),
                );
            col = col.child(cells);
        }
        col
    };

    v_flex().child(header).child(body)
}

/// One row of the API-token table.
fn token_table(
    tokens: &[ApiToken],
    theme: &gpui_component::Theme,
    state_ent: Entity<AppState>,
) -> impl IntoElement {
    let widths = [px(160.), px(300.), px(160.)];
    let labels = ["备注名", "Token", "创建时间"];
    let header = table_header(&labels, &widths, theme);

    let body = if tokens.is_empty() {
        v_flex().child(
            div()
                .px_3()
                .py(px(16.))
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child("暂无 API Token。点击右上角「新建 Token」生成。"),
        )
    } else {
        let mut col = v_flex().gap(px(1.));
        for (i, t) in tokens.iter().enumerate() {
            let masked = if t.token.len() > 12 {
                format!("{}••••••{}", &t.token[..6], &t.token[t.token.len() - 4..])
            } else {
                t.token.clone()
            };
            let state_ent = state_ent.clone();
            let token_id = t.id.clone();
            let cells = h_flex()
                .w_full()
                .px_3()
                .py(px(7.))
                .text_size(px(12.))
                .border_b_1()
                .border_color(theme.border)
                .hover(|d| d.bg(theme.muted))
                .child(div().w(widths[0]).flex_shrink_0().child(t.label.clone()))
                .child(
                    div()
                        .w(widths[1])
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground)
                        .child(masked),
                )
                .child(
                    div()
                        .w(widths[2])
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground)
                        .child(t.created_at.clone()),
                )
                .child(div().flex_1())
                .child(
                    Button::new(("token-del", i))
                        .ghost()
                        .small()
                        .icon(IconName::Delete)
                        .tooltip("撤销")
                        .on_click(move |_, _w, cx: &mut App| {
                            let id = token_id.clone();
                            let _ = state_ent.update(cx, |s, cx| {
                                if let Some(p) = s.active_project_mut() {
                                    p.api_tokens.retain(|x| x.id != id);
                                }
                                s.notify_workspace(cx);
                            });
                        }),
                );
            col = col.child(cells);
        }
        col
    };

    v_flex().child(header).child(body)
}

/// One row of the 接口属性 flat table.
fn iface_props_row(
    i: usize,
    row: &(String, String, RequestMethod, String, String, Vec<String>),
    theme: &gpui_component::Theme,
) -> impl IntoElement {
    let (path, name, method, url, status, tags) = row;
    let widths = [px(140.), px(160.), px(72.), px(220.), px(90.), px(120.)];
    let status_label = if status.trim().is_empty() {
        "未设置"
    } else {
        status.as_str()
    };
    h_flex()
        .id(("iface-row", i))
        .w_full()
        .px_3()
        .py(px(6.))
        .text_size(px(12.))
        .border_b_1()
        .border_color(theme.border)
        .hover(|d| d.bg(theme.muted))
        .child(
            div()
                .w(widths[0])
                .flex_shrink_0()
                .text_color(theme.muted_foreground)
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(if path.is_empty() {
                    "/".to_string()
                } else {
                    path.clone()
                }),
        )
        .child(
            div()
                .w(widths[1])
                .flex_shrink_0()
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(name.clone()),
        )
        .child(
            div()
                .w(widths[2])
                .flex_shrink_0()
                .child(method_chip(*method, theme)),
        )
        .child(
            div()
                .w(widths[3])
                .flex_shrink_0()
                .text_color(theme.muted_foreground)
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(url.clone()),
        )
        .child(
            div()
                .w(widths[4])
                .flex_shrink_0()
                .child(status_label.to_string()),
        )
        .child(
            div()
                .w(widths[5])
                .flex_shrink_0()
                .text_color(theme.muted_foreground)
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(tags.join(", ")),
        )
}

/// Coloured chip for an HTTP method.
fn method_chip(method: RequestMethod, theme: &gpui_component::Theme) -> impl IntoElement {
    let put_color: gpui::Hsla = gpui::rgb(0xf59e0b).into();
    let (bg, fg) = match method {
        RequestMethod::Get => (theme.accent.opacity(0.25), theme.foreground),
        RequestMethod::Post => (theme.primary.opacity(0.3), theme.primary),
        RequestMethod::Put => (put_color.opacity(0.3), put_color),
        RequestMethod::Delete => (theme.danger.opacity(0.3), theme.danger),
        _ => (theme.muted_foreground.opacity(0.25), theme.foreground),
    };
    div()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .px(px(5.))
        .py(px(1.))
        .rounded(px(3.))
        .bg(bg)
        .text_color(fg)
        .child(method.as_str().to_string())
}

/// One row of the Mock-service list.
fn mock_row(
    i: usize,
    location: &str,
    url: &str,
    name: &str,
    status: u16,
    theme: &gpui_component::Theme,
) -> impl IntoElement {
    h_flex()
        .id(("mock-row", i))
        .w_full()
        .px_3()
        .py(px(7.))
        .gap_2()
        .items_center()
        .rounded(px(4.))
        .border_b_1()
        .border_color(theme.border)
        .hover(|d| d.bg(theme.muted))
        .child(badge_chip(&format!("{}", status), theme))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(name.to_string()),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(url.to_string()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(location.to_string()),
        )
}

/// A small toggle row with an on-change callback.
fn toggle_row(
    id: &'static str,
    label: &str,
    on: bool,
    theme: gpui_component::Theme,
    on_change: impl Fn(bool, &mut App) + 'static,
) -> impl IntoElement {
    let _ = id;
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .id(format!("toggle-{id}"))
                .w(px(28.))
                .h(px(16.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .px(px(2.))
                .cursor_pointer()
                .when(on, |d| d.bg(theme.primary).justify_end())
                .when(!on, |d| {
                    d.bg(theme.muted_foreground.opacity(0.4)).justify_start()
                })
                .child(
                    div()
                        .w(px(12.))
                        .h(px(12.))
                        .rounded_full()
                        .bg(theme.background),
                )
                .on_click(move |_, _w, cx: &mut App| {
                    on_change(!on, cx);
                }),
        )
        .child(div().text_size(px(13.)).child(label.to_string()))
}
