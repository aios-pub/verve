//! All view title bars: the API workbench bar (workspace/project/env
//! switchers, sync, import/export, update, language), the shared rail
//! toggle / update / export / language pickers, and the per-view bars
//! (SSH, Docker, K8s, Notes, JSON, Relocate, PDF, Markdown, AutoTest).

use gpui::{img, *};
use gpui::prelude::FluentBuilder as _;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable as _, Selectable as _, Disableable as _,
    button::{Button, ButtonVariants as _}, h_flex, v_flex, popover::Popover, WindowExt};
use crate::assets::{BRACES, BRACES_JSON, DOCKER as DOCKER_ICON, DOCS, EXPORT, FILE_PDF,
    HISTORY, IMPORT, K8S as K8S_ICON, KANBAN, MARKDOWN, NOTEBOOK, REFRESH_CW, SAVE,
    SAVE_AS, SERVER, SHARE};
use crate::share::models::ShareScope;
use crate::ui::k8s_panel::View as K8sView;
use crate::state::{AppEvent, AppState};
use super::widgets::{menu_item, menu_separator, vicon};
use super::{PendingDialog, SideView, VerveApp};

impl VerveApp {
    pub(super) fn render_title_bar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.active_view {
            SideView::Ssh => self.render_ssh_title_bar(cx).into_any_element(),
            SideView::Docker => self.render_docker_title_bar(cx).into_any_element(),
            SideView::K8s => self.render_k8s_title_bar(cx).into_any_element(),
            SideView::Notes => self.render_notes_title_bar(cx).into_any_element(),
            SideView::MarkdownEditor => self.render_markdown_title_bar(cx).into_any_element(),
            SideView::JsonFormat => self.render_json_title_bar(cx).into_any_element(),
            SideView::PdfEditor => self.render_pdf_title_bar(cx).into_any_element(),
            SideView::AutoTest => self.render_autotest_title_bar(cx).into_any_element(),
            SideView::Relocate => self.render_relocate_title_bar(cx).into_any_element(),
            _ => self.render_api_title_bar(cx).into_any_element(),
        }
    }

    pub(super) fn render_api_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let st = self.state.read(cx);
        let project_name = st
            .active_project()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Verve".to_string());
        // Active environment name + the list of (id, name) for the env popover.
        let active_env_name = st
            .active_project()
            .and_then(|p| {
                p.active_environment
                    .as_ref()
                    .and_then(|id| p.environments.iter().find(|e| &e.id == id))
                    .map(|e| e.name.clone())
            })
            .unwrap_or_else(|| "No environment".to_string());
        let active_project_envs: Vec<(String, String)> = st
            .active_project()
            .map(|p| {
                p.environments
                    .iter()
                    .map(|e| (e.id.clone(), e.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let active_env_id = st
            .active_project()
            .and_then(|p| p.active_environment.clone());
        let _ = theme.clone();

        h_flex()
            .h(px(40.))
            // On macOS the (transparent) titlebar hosts the traffic-light
            // buttons on the left; pad the content past them.
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            // Far-left activity rail collapse/expand toggle.
            .child(self.render_rail_toggle(cx))
            // Sidebar (tree) collapse/expand toggle.
            .child(
                Button::new("sidebar-toggle")
                    .ghost()
                    .small()
                    .icon(if self.sidebar_collapsed {
                        IconName::PanelLeftOpen
                    } else {
                        IconName::PanelLeft
                    })
                    .selected(!self.sidebar_collapsed)
                    .tooltip(if self.sidebar_collapsed {
                        "展开接口列表"
                    } else {
                        "收起接口列表"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar_collapsed = !this.sidebar_collapsed;
                        cx.notify();
                    })),
            )
            // Workspace switcher — lists all workspaces, allows switching,
            // creating, and deleting (except the default).
            .child(
                div()
                    .w(px(150.))
                    .child({
                        let ws_idx = crate::state::persistence::load_workspaces_index();
                        let ws_name = ws_idx
                            .active_meta()
                            .map(|w| w.name.clone())
                            .unwrap_or_else(|| "Default".to_string());
                        let workspaces: Vec<(String, String, bool)> = ws_idx
                            .workspaces
                            .iter()
                            .map(|w| (w.id.clone(), w.name.clone(), w.is_default))
                            .collect();
                        let ent = cx.entity();
                        Popover::new("workspace-popover")
                            .anchor(gpui::Anchor::BottomLeft)
                            .open(self.workspace_popover_open)
                            .on_open_change(cx.listener(|this, open, _, cx| {
                                this.workspace_popover_open = *open;
                                cx.notify();
                            }))
                            .trigger(
                                Button::new("workspace-trigger")
                                    .ghost()
                                    .small()
                                    .icon(IconName::LayoutDashboard)
                                    .label(ws_name)
                                    .icon(IconName::ChevronDown)
                                    .w_full(),
                            )
                            .p(px(4.))
                            .child(
                                v_flex()
                                    .w(px(200.))
                                    .gap(px(1.))
                                    .children(workspaces.iter().map(|(id, name, is_default)| {
                                        let id = id.clone();
                                        let name = name.clone();
                                        let can_delete = !*is_default;
                                        let ent = ent.clone();
                                        let id_del = id.clone();
                                        let ent_del = ent.clone();
                                        h_flex()
                                            .id(format!("ws-{id}"))
                                            .w_full()
                                            .px(px(8.))
                                            .py(px(5.))
                                            .gap(px(6.))
                                            .items_center()
                                            .rounded(px(4.))
                                            .cursor_pointer()
                                            .hover(|d| d.bg(theme.muted))
                                            .child(div().flex_1().text_size(px(13.)).child(name.clone()))
                                            .when(can_delete, |d| {
                                                d.child(
                                                    div()
                                                        .id(format!("ws-del-{id_del}"))
                                                        .cursor_pointer()
                                                        .text_color(theme.muted_foreground)
                                                        .hover(|h| h.text_color(theme.danger))
                                                        .child(IconName::Delete)
                                                        .on_click(move |_, _w, cx: &mut App| {
                                                            let _ = ent_del.update(cx, |this, cx| {
                                                                this.workspace_popover_open = false;
                                                                this.delete_workspace(id_del.clone(), cx);
                                                            });
                                                        }),
                                                )
                                            })
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.workspace_popover_open = false;
                                                    this.switch_workspace(id.clone(), cx);
                                                });
                                            })
                                    }))
                                    .child(div().h(px(1.)).w_full().bg(theme.border))
                                    .child(
                                        div()
                                            .id("ws-new")
                                            .w_full()
                                            .px(px(8.))
                                            .py(px(5.))
                                            .text_size(px(13.))
                                            .rounded(px(4.))
                                            .cursor_pointer()
                                            .text_color(theme.primary)
                                            .hover(|d| d.bg(theme.muted))
                                            .child("+ 新建工作空间")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.workspace_popover_open = false;
                                                this.open_new_workspace(window, cx);
                                            })),
                                    ),
                            )
                    }),
            )
            // Project switcher — a popover listing the active workspace's
            // projects plus a "新建项目" entry.
            .child(
                div()
                    .w(px(180.))
                    .child(
                        Popover::new("project-popover")
                            .anchor(gpui::Anchor::BottomLeft)
                            .open(self.project_popover_open)
                            .on_open_change(cx.listener(|this, open, _, cx| {
                                this.project_popover_open = *open;
                                cx.notify();
                            }))
                            .trigger(
                                Button::new("project-trigger")
                                    .ghost()
                                    .small()
                                    .label(project_name.clone())
                                    .icon(IconName::ChevronDown)
                                    .w_full(),
                            )
                            .p(px(4.))
                            .child({
                                // Build the project list + management menu.
                                let projects: Vec<(String, usize)> = st
                                    .data
                                    .projects
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| (p.name.clone(), i))
                                    .collect();
                                let active_idx = st.active_project;
                                let ent = cx.entity();
                                v_flex()
                                    .w(px(200.))
                                    .gap(px(1.))
                                    .children(projects.iter().map(|(name, idx)| {
                                        let name = name.clone();
                                        let idx = *idx;
                                        let is_active = idx == active_idx;
                                        let ent = ent.clone();
                                        let ent_set = ent.clone();
                                        let ent_del = ent.clone();
                                        menu_item(
                                            format!("proj-{}", idx),
                                            name,
                                            is_active,
                                            None,
                                            // Hover-revealed delete → confirm dialog.
                                            Some(Box::new(move |cx| {
                                                let _ = ent_del.update(cx, |this, cx| {
                                                    this.project_popover_open = false;
                                                    this.pending_dialog =
                                                        Some(PendingDialog::DeleteProject(idx));
                                                    cx.notify();
                                                });
                                            })),
                                            // Hover-revealed gear → open project settings.
                                            Some(Box::new(move |cx| {
                                                let _ = ent_set.update(cx, |this, cx| {
                                                    this.project_popover_open = false;
                                                    this.pending_dialog =
                                                        Some(PendingDialog::ProjectSettings(idx));
                                                    cx.notify();
                                                });
                                            })),
                                            move |_, cx| {
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.project_popover_open = false;
                                                    this.state.update(cx, |s, cx| {
                                                        if s.active_project != idx {
                                                            s.active_project = idx;
                                                            s.data.active_project_id = s.data.projects.get(idx).map(|p| p.id.clone());
                                                            s.selected_request = None;
                                                            s.selected_folder = None;
                                                            s.open_request_ids.clear();
                                                            s.active_tab_id = None;
                                                            cx.emit(AppEvent::WorkspaceChanged);
                                                            cx.emit(AppEvent::SelectionChanged);
                                                        }
                                                    });
                                                    cx.notify();
                                                });
                                            },
                                            &theme,
                                        )
                                    }))
                                    // Separator + the single "+ 新建" entry.
                                    .child(menu_separator(&theme))
                                    .child({
                                        let ent = ent.clone();
                                        menu_item(
                                            "proj-new".to_string(),
                                            "新建项目".to_string(),
                                            false,
                                            Some(IconName::Plus),
                                            None,
                                            None,
                                            move |_, cx| {
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.project_popover_open = false;
                                                    this.pending_new_project = true;
                                                    cx.notify();
                                                });
                                            },
                                            &theme,
                                        )
                                    })
                            }),
                    ),
            )
            // Share the whole project's docs (scope = Project).
            .child(
                Button::new("share-project")
                    .ghost()
                    .small()
                    .icon(vicon(SHARE))
                    .tooltip("分享API文档")
                    .on_click(cx.listener(
                        |this, _ev, window, cx| {
                            this.open_share_dialog(
                                ShareScope::Project,
                                None,
                                None,
                                window,
                                cx,
                            );
                        },
                    )),
            )
            .child(div().flex_1())
            // Environment switcher — a popover listing environments plus
            // management entries (环境管理/Cookie/全局参数/全局变量/新建).
            .child(
                div()
                    .w(px(160.))
                    .child(
                        Popover::new("env-popover")
                            .anchor(gpui::Anchor::BottomLeft)
                            .open(self.env_popover_open)
                            .on_open_change(cx.listener(|this, open, _, cx| {
                                this.env_popover_open = *open;
                                cx.notify();
                            }))
                            .trigger(
                                Button::new("env-trigger")
                                    .ghost()
                                    .small()
                                    .label(active_env_name.clone())
                                    .icon(IconName::ChevronDown)
                                    .w_full(),
                            )
                            .p(px(4.))
                            .child({
                                // Build env list + a single "+ 新建" entry. Each
                                // environment row reveals a settings (gear) button
                                // on hover that opens environment management.
                                let envs: Vec<(String, String)> = active_project_envs
                                    .iter()
                                    .map(|(id, name)| (name.clone(), id.clone()))
                                    .collect();
                                let active_env = active_env_id.clone();
                                let ent = cx.entity();
                                v_flex()
                                    .w(px(220.))
                                    .gap(px(1.))
                                    .child(menu_item(
                                        "env-none".to_string(),
                                        "No environment".to_string(),
                                        active_env.is_none(),
                                        None,
                                        None,
                                        None,
                                        {
                                            let ent = ent.clone();
                                            move |_, cx| {
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.env_popover_open = false;
                                                    this.state.update(cx, |s, cx| {
                                                        if let Some(project) =
                                                            s.active_project_mut()
                                                        {
                                                            project.active_environment = None;
                                                            cx.emit(AppEvent::EnvironmentChanged);
                                                        }
                                                    });
                                                    cx.notify();
                                                });
                                            }
                                        },
                                        &theme,
                                    ))
                                    .children(envs.iter().map(|(name, id)| {
                                        let name = name.clone();
                                        let id = id.clone();
                                        let id_del = id.clone();
                                        let is_active = Some(&id) == active_env.as_ref();
                                        let ent = ent.clone();
                                        let ent_set = ent.clone();
                                        let ent_del = ent.clone();
                                        menu_item(
                                            format!("env-{}", id),
                                            name,
                                            is_active,
                                            None,
                                            // Hover-revealed delete → confirm dialog.
                                            Some(Box::new(move |cx| {
                                                let _ = ent_del.update(cx, |this, cx| {
                                                    this.env_popover_open = false;
                                                    this.pending_dialog =
                                                        Some(PendingDialog::DeleteEnv(id_del.clone()));
                                                    cx.notify();
                                                });
                                            })),
                                            // Hover-revealed gear → open env management.
                                            Some(Box::new(move |cx| {
                                                let _ = ent_set.update(cx, |this, cx| {
                                                    this.env_popover_open = false;
                                                    this.pending_dialog =
                                                        Some(PendingDialog::Environments);
                                                    cx.notify();
                                                });
                                            })),
                                            move |_, cx| {
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.env_popover_open = false;
                                                    this.state.update(cx, |s, cx| {
                                                        if let Some(project) =
                                                            s.active_project_mut()
                                                        {
                                                            project.active_environment =
                                                                Some(id.clone());
                                                            cx.emit(AppEvent::EnvironmentChanged);
                                                        }
                                                    });
                                                    cx.notify();
                                                });
                                            },
                                            &theme,
                                        )
                                    }))
                                    // Separator + the single "+ 新建" entry.
                                    .child(menu_separator(&theme))
                                    .child({
                                        let ent = ent.clone();
                                        menu_item(
                                            "env-new".to_string(),
                                            "新建环境".to_string(),
                                            false,
                                            Some(IconName::Plus),
                                            None,
                                            None,
                                            move |_, cx| {
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.env_popover_open = false;
                                                    this.pending_new_env = true;
                                                    cx.notify();
                                                });
                                            },
                                            &theme,
                                        )
                                    })
                            }),
                    ),
            )
            // Sync (save to disk) button — persists the workspace, then runs
            // a git sync (commit dirty → pull/push). Shows a label while busy
            // and a transient success/failure chip after, so the user always
            // gets feedback (the original "no response" complaint).
            .child({
                let g = self.git.read(cx);
                let theme = cx.theme().clone();
                let busy_label = g.busy.clone();
                let busy = g.is_busy();
                let branch = g.status.branch.clone().unwrap_or_else(|| "—".to_string());
                let dirty = g.status.dirty;
                let tooltip = if g.initialized {
                    format!("Git 同步 · {} · {} 待提交", branch, dirty)
                } else {
                    "同步/保存到本地".to_string()
                };
                let last_result = g.last_result.clone();
                let git_for_click = self.git.clone();
                let git_for_dismiss = self.git.clone();
                let muted_fg = theme.muted_foreground;
                let fg = theme.foreground;
                let accent = theme.accent;
                let danger = theme.danger;
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("sync")
                            .ghost()
                            .small()
                            .icon(IconName::Replace)
                            .selected(busy)
                            .tooltip(tooltip)
                            // Show "同步中" while a git op is in flight, so the
                            // click is visibly acknowledged.
                            .when_some(busy_label, |btn, label| {
                                btn.label(label).disabled(true)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                log::info!("sync 按钮：点击 — 先持久化 workspace.json");
                                // Persist first so the latest workspace.json is on disk,
                                // then commit + sync.
                                this.state.update(cx, |s, cx| s.persist(cx));
                                let _ = git_for_click.update(cx, |g, cx| {
                                    log::info!("sync 按钮：调用 sync_async（若已有操作进行中会被 is_busy 挡掉，由 Persisted 自动同步接管）");
                                    g.sync_async(None, cx)
                                });
                            })),
                    )
                    // Transient result chip: green ✓ on success, red message on
                    // failure. Dismissable via the × control.
                    .when_some(last_result, |col, (msg, ok)| {
                        col.child(
                            div()
                                .text_size(px(11.))
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .max_w(px(260.))
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .when(ok, |d| d.bg(accent.opacity(0.2)).text_color(fg))
                                .when(!ok, |d| d.bg(danger.opacity(0.2)).text_color(danger))
                                .child(if ok { format!("✓ {msg}") } else { format!("⚠ {msg}") }),
                        )
                    })
                    // A small dismiss (×) control to clear the chip.
                    .when_some(self.git.read(cx).last_result.clone(), |col, _| {
                        col.child(
                            div()
                                .id("sync-dismiss")
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(muted_fg)
                                .hover(|d| d.text_color(fg))
                                .child("×")
                                .on_click(move |_, _w, cx: &mut App| {
                                    let _ = git_for_dismiss.update(cx, |g, cx| {
                                        g.last_result = None;
                                        cx.emit(crate::git::GitEvent::Updated);
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
            .child(
                Button::new("import")
                    .ghost()
                    .small()
                    .icon(vicon(IMPORT))
                    .tooltip("导入集合 (Postman/OpenAPI JSON)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.import_collection(cx);
                    })),
            )
            .child(self.render_export_picker(cx))
            // Update check button: shows a spinner while checking, a download
            // icon when an update is available, or a refresh icon otherwise.
            .child(self.render_update_button(cx))
            // Language switcher (top-right).
            .child(self.render_lang_picker(cx))
    }

    pub(super) fn render_rail_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        Button::new("rail-toggle")
            .ghost()
            .small()
            .icon(if self.rail_collapsed {
                IconName::PanelLeftOpen
            } else {
                IconName::PanelLeftClose
            })
            .selected(!self.rail_collapsed)
            .tooltip(if self.rail_collapsed {
                "展开左侧导航栏"
            } else {
                "收起左侧导航栏"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.rail_collapsed = !this.rail_collapsed;
                cx.notify();
            }))
    }

    pub(super) fn render_update_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let checking = self.update_checking;
        let has_update = self.update_info.is_some();
        let update_info = self.update_info.clone();
        let icon: Icon = if checking {
            IconName::LoaderCircle.into()
        } else if has_update {
            IconName::ExternalLink.into()
        } else {
            vicon(REFRESH_CW)
        };
        let tooltip = if checking {
            "正在检查更新…".to_string()
        } else if let Some(info) = &update_info {
            format!("发现新版本 v{}，点击前往下载", info.version)
        } else {
            "检查更新".to_string()
        };
        Button::new("update-check")
            .ghost()
            .small()
            .icon(icon)
            .tooltip(tooltip)
            .when(checking, |b| b.disabled(true))
            .on_click(cx.listener(move |this, _ev, window, cx| {
                if this.update_info.is_some() {
                    this.open_update_download(window, cx);
                } else {
                    this.check_updates_manual(window, cx);
                }
            }))
    }

    pub(super) fn render_export_picker(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let open = self.export_popover_open;
        Popover::new("export-picker")
            .anchor(gpui::Anchor::BottomRight)
            .open(open)
            .on_open_change(cx.listener(|this, open, _, cx| {
                this.export_popover_open = *open;
                cx.notify();
            }))
            .trigger(
                Button::new("export-trigger")
                    .ghost()
                    .small()
                    .icon(vicon(EXPORT))
                    .tooltip("导出项目"),
            )
            .p(px(4.))
            .child(v_flex().w(px(200.)).gap(px(2.)).children(
                crate::export::Format::ALL.iter().map(|&f| {
                    let ent = cx.entity();
                    div()
                        .id(format!("export-fmt-{}", f.label()))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.muted))
                        .child(div().text_sm().text_color(theme.foreground).child(format!(
                            "{} (.{})",
                            f.label(),
                            f.extension()
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.export_popover_open = false;
                            this.export_project(f, cx);
                        }))
                }),
            ))
    }

    pub(super) fn render_lang_picker(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let open = self.lang_popover_open;
        let cur = rust_i18n::locale().to_string();
        let lang_label = if cur.starts_with("en") { "EN" } else { "中" };
        Popover::new("lang-picker")
            .anchor(gpui::Anchor::BottomRight)
            .open(open)
            .on_open_change(cx.listener(|this, open, _, cx| {
                this.lang_popover_open = *open;
                cx.notify();
            }))
            .trigger(
                Button::new("rail-lang")
                    .ghost()
                    .small()
                    .label(lang_label)
                    .icon(IconName::Globe)
                    .tooltip("语言 / Language"),
            )
            .p(px(4.))
            .child(
                v_flex()
                    .w(px(140.))
                    .gap(px(2.))
                    .child(
                        div()
                            .id("lang-opt-zh")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.muted))
                            .when(cur.starts_with("zh") || cur == "zh-CN", |d| {
                                d.bg(theme.accent.opacity(0.5))
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child("简体中文"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                rust_i18n::set_locale("zh-CN");
                                let mut l =
                                    crate::state::persistence::load_layout().unwrap_or_default();
                                l.locale = Some("zh-CN".into());
                                let _ = crate::state::persistence::save_layout(&l);
                                this.lang_popover_open = false;
                                window.refresh();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("lang-opt-en")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.muted))
                            .when(cur == "en" || cur.starts_with("en"), |d| {
                                d.bg(theme.accent.opacity(0.5))
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child("English"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                rust_i18n::set_locale("en");
                                let mut l =
                                    crate::state::persistence::load_layout().unwrap_or_default();
                                l.locale = Some("en".into());
                                let _ = crate::state::persistence::save_layout(&l);
                                this.lang_popover_open = false;
                                window.refresh();
                                cx.notify();
                            })),
                    ),
            )
    }

    /// The title bar shown while the SSH manager is active. SSH hosts live in
    /// a global store (`~/.verve/ssh_hosts.json`), not in any workspace, so
    /// the API bar's workspace/project/environment controls don't apply here.
    /// This bar carries the SSH context instead — title, saved-host count,
    /// and the primary "new host" action — plus the app-level update/language
    /// controls. Height, padding, and the leading rail toggle match
    /// `render_api_title_bar` exactly, so nothing shifts on view switch.
    pub(super) fn render_ssh_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let host_count = self.ssh.read(cx).host_count();
        let ssh = self.ssh.clone();

        h_flex()
            .h(px(40.))
            // On macOS the (transparent) titlebar hosts the traffic-light
            // buttons on the left; pad the content past them.
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            // SSH context: terminal icon + title + saved-host count badge.
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(IconName::SquareTerminal),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("SSH 管理"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme.accent.opacity(0.2))
                            .text_color(theme.foreground)
                            .child(format!("{host_count} 台主机")),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("ssh-title-new-host")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("新建主机")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let _ = ssh.update(cx, |panel, cx| {
                            panel.open_new_host(window, cx);
                        });
                    })),
            )
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }

    /// The title bar shown while the Docker manager is active. Mirrors the SSH
    /// title-bar pattern: an icon + title + host-count badge on the left,
    /// sub-view tabs (Containers/Images/Hosts) and a refresh button in the
    /// middle, and a primary "add host" action on the right.
    pub(super) fn render_docker_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let docker = self.docker.clone();
        let (host_count, active_host, cur_view) = {
            let r = self.docker.read(cx);
            (
                r.host_count(),
                r.active_host_name(),
                r.current_view().to_string(),
            )
        };
        let cur_view: &str = &cur_view;

        let docker_containers = docker.clone();
        let docker_images = docker.clone();
        let docker_hosts = docker.clone();
        let docker_reload = docker.clone();
        let docker_add = docker.clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            // Docker context: icon + title + host badge.
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_color(theme.primary).child(vicon(DOCKER_ICON)))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Docker"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme.accent.opacity(0.2))
                            .text_color(theme.foreground)
                            .child(active_host),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(format!("{host_count} 台主机")),
                    ),
            )
            .child(div().w(px(12.)))
            // Sub-view tabs: Containers / Images / Hosts.
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("docker-title-containers")
                            .ghost()
                            .small()
                            .label("Containers")
                            .when(cur_view == "containers", |b| b.selected(true))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                docker_containers.update(cx, |p, cx| p.show_containers(cx));
                            })),
                    )
                    .child(
                        Button::new("docker-title-images")
                            .ghost()
                            .small()
                            .label("Images")
                            .when(cur_view == "images", |b| b.selected(true))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                docker_images.update(cx, |p, cx| p.show_images(cx));
                            })),
                    )
                    .child(
                        Button::new("docker-title-hosts")
                            .ghost()
                            .small()
                            .label("主机")
                            .when(cur_view == "hosts", |b| b.selected(true))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                docker_hosts.update(cx, |p, cx| p.show_hosts(cx));
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("docker-title-reload")
                    .ghost()
                    .small()
                    .icon(IconName::Loader)
                    .label("刷新")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        docker_reload.update(cx, |p, cx| p.refresh_current(cx));
                    })),
            )
            .child(
                Button::new("docker-title-add-host")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("添加主机")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        docker_add.update(cx, |p, cx| p.open_add_host(window, cx));
                    })),
            )
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }

    /// The title bar shown while the K8s manager is active. Mirrors the SSH
    /// title-bar pattern: an icon + title + context badge on the left, sub-view
    /// tabs (Pods/Contexts/Forwards) and a refresh button in the middle, and
    /// primary actions (import kubeconfig / add cluster) on the right.
    pub(super) fn render_k8s_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let k8s = self.k8s.clone();
        let (ctx_name, ctx_count, fwd_count, cur_view_str) = {
            let r = self.k8s.read(cx);
            (
                r.active_context_display(),
                r.context_count(),
                r.forward_count(),
                r.current_view().to_string(),
            )
        };
        let cur_view: &str = &cur_view_str;

        let k8s_pods = k8s.clone();
        let k8s_ctx = k8s.clone();
        let k8s_fwd = k8s.clone();
        let k8s_reload = k8s.clone();
        let k8s_add = k8s.clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            // K8s context: icon + title + current context badge.
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_color(theme.primary).child(vicon(K8S_ICON)))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Kubernetes"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme.accent.opacity(0.2))
                            .text_color(theme.foreground)
                            .child(ctx_name),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(format!("{ctx_count} 个 context")),
                    ),
            )
            .child(div().w(px(12.)))
            // Sub-view tabs: Pods / Contexts / Forwards.
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("k8s-title-pods")
                            .ghost()
                            .small()
                            .label("Pods")
                            .when(cur_view == "pods", |b| b.selected(true))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                k8s_pods.update(cx, |p, cx| {
                                    // show_pods needs &Window; skip explicit namespace load here,
                                    // it will load on first render in render_pods
                                    p.set_view(K8sView::Pods, cx);
                                    if p.pods.is_empty() && !p.pods_loading {
                                        p.reload_pods(cx);
                                    }
                                });
                            })),
                    )
                    .child(
                        Button::new("k8s-title-ctx")
                            .ghost()
                            .small()
                            .label("Contexts")
                            .when(cur_view == "contexts", |b| b.selected(true))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                k8s_ctx.update(cx, |p, cx| p.show_contexts(cx));
                            })),
                    )
                    .child(
                        Button::new("k8s-title-fwd")
                            .ghost()
                            .small()
                            .label(format!("转发 ({fwd_count})"))
                            .when(cur_view == "forwards", |b| b.selected(true))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                k8s_fwd.update(cx, |p, cx| p.show_forwards(cx));
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("k8s-title-reload")
                    .ghost()
                    .small()
                    .icon(IconName::Loader)
                    .label("刷新")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        k8s_reload.update(cx, |p, cx| p.refresh_current(cx));
                    })),
            )
            .child(
                Button::new("k8s-title-add-cluster")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("添加集群")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let panel = k8s_add.clone();
                        // k8s_cluster_dialog::open_dialog takes &mut App.
                        window.defer(cx, move |window, cx| {
                            let panel_for_cb = panel.clone();
                            crate::ui::k8s_cluster_dialog::open_dialog(
                                None,
                                move |_window, cx| {
                                    let _ = panel_for_cb.update(cx, |p, cx| p.refresh_current(cx));
                                },
                                window,
                                cx,
                            );
                        });
                    })),
            )
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }
}

impl VerveApp {
    /// The title bar shown while the Notes panel is active. Mirrors
    /// `render_ssh_title_bar`: rail toggle, identity block (icon + title + count
    /// badge), spacer, then import / export / live-source toggle / lang.
    pub(super) fn render_notes_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let notes = self.notes.clone();

        // Snapshot note count and current view mode.
        let (note_count, list_collapsed) = {
            let r = notes.read(cx);
            (r.note_count(), r.is_list_collapsed())
        };

        let notes_import = notes.clone();
        let notes_export = notes.clone();
        let notes_collapse = notes.clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            // Far-left rail toggle (shared across views).
            .child(self.render_rail_toggle(cx))
            // Sidebar collapse button — "收起笔记列表" / "展开笔记列表".
            .child(
                Button::new("sidebar-toggle")
                    .ghost()
                    .small()
                    .icon(if list_collapsed {
                        IconName::PanelLeftOpen
                    } else {
                        IconName::PanelLeft
                    })
                    .selected(!list_collapsed)
                    .tooltip(if list_collapsed {
                        rust_i18n::t!("notes.expand_list").to_string()
                    } else {
                        rust_i18n::t!("notes.collapse_list").to_string()
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = notes_collapse.update(cx, |notes, cx| {
                            notes.toggle_list_collapsed(cx);
                        });
                    })),
            )
            // Identity block: notebook icon + "笔记" + note-count badge.
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(vicon(crate::assets::NOTEBOOK)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("notes.title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme.accent.opacity(0.2))
                            .text_color(theme.foreground)
                            .child(
                                rust_i18n::t!("notes.note_count", count = note_count).to_string(),
                            ),
                    ),
            )
            .child(div().flex_1())
            // WYSIWYG formatting toolbar — right-aligned with the source/import/
            // export actions so it sits next to the source toggle. (Previously
            // it was on the far left, far from the source toggle.)
            .child({
                let editor = notes.read(cx).editor().clone();
                self.render_editor_toolbar(&editor, cx)
            })
            // View-mode toggle (Rendered ↔ Source) — right-aligned with the
            // other action buttons, same affordance as the markdown title bar.
            .child({
                let editor = notes.read(cx).editor().clone();
                let view_mode = editor.read(cx).view_mode();
                let notes_vm = notes.clone();
                Button::new("notes-title-view-mode")
                    .small()
                    .ghost()
                    .selected(view_mode == crate::wysiwyg::editor::ViewMode::Source)
                    .icon(IconName::Eye)
                    .tooltip(if view_mode == crate::wysiwyg::editor::ViewMode::Source {
                        rust_i18n::t!("markdown.switch_to_rendered").to_string()
                    } else {
                        rust_i18n::t!("markdown.switch_to_source").to_string()
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = notes_vm.update(cx, |p, cx| p.toggle_view_mode(cx));
                    }))
            })
            // Import button.
            .child(
                Button::new("notes-title-import")
                    .ghost()
                    .small()
                    .icon(vicon(IMPORT))
                    .label(rust_i18n::t!("notes.import").to_string())
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let _ = notes_import.update(cx, |p, cx| {
                            p.import_markdown_dialog(window, cx);
                        });
                    })),
            )
            // Export button.
            .child(
                Button::new("notes-title-export")
                    .ghost()
                    .small()
                    .icon(vicon(EXPORT))
                    .label(rust_i18n::t!("notes.export").to_string())
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let _ = notes_export.update(cx, |p, cx| {
                            p.export_active_note_dialog(window, cx);
                        });
                    })),
            )
            .child(self.render_lang_picker(cx))
    }

    /// cmd-o — open a Markdown file in the markdown editor view.
    pub(super) fn render_json_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            // Icon + title
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_color(theme.primary).child(vicon(BRACES_JSON)))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("json.title").to_string()),
                    ),
            )
            .child(div().flex_1())
            // Toolbar buttons that trigger actions on the json panel
            .child({
                let json = self.json.clone();
                Button::new("json-title-format")
                    .small()
                    .ghost()
                    .icon(IconName::Check)
                    .label(rust_i18n::t!("json.format_btn").to_string())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = json.update(cx, |panel, cx| panel.format_from_title(cx));
                    }))
            })
            .child({
                let json = self.json.clone();
                let active = json.read(cx).is_compact_active();
                Button::new("json-title-compact")
                    .small()
                    .ghost()
                    .label(rust_i18n::t!("json.compact_btn").to_string())
                    .when(active, |b| b.selected(true))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = json.update(cx, |panel, cx| panel.toggle_compact(cx));
                    }))
            })
            .child({
                let json = self.json.clone();
                Button::new("json-title-expand")
                    .small()
                    .ghost()
                    .label(rust_i18n::t!("json.expand_all").to_string())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = json.update(cx, |panel, cx| panel.expand_all(cx));
                    }))
            })
            .child({
                let json = self.json.clone();
                Button::new("json-title-collapse")
                    .small()
                    .ghost()
                    .label(rust_i18n::t!("json.collapse_all").to_string())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = json.update(cx, |panel, cx| panel.collapse_all(cx));
                    }))
            })
            .child({
                let json = self.json.clone();
                Button::new("json-title-copy")
                    .small()
                    .ghost()
                    .icon(IconName::Copy)
                    .label(rust_i18n::t!("json.copy_btn").to_string())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = json.update(cx, |panel, cx| panel.copy_result(cx));
                    }))
            })
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }

    /// Relocate title bar: rail toggle + disk icon + title + quick actions.
    /// The heavy controls live in the panel body; the title bar keeps a light
    /// summary (target disk + reclaimed space) and a "选择磁盘" shortcut.
    pub(super) fn render_relocate_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let eng = self.relocate.read(cx).engine.read(cx);
        let target_label = eng
            .target_root
            .as_ref()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "未选择磁盘".to_string());
        let reclaimed = crate::relocate::types::human_bytes(eng.reclaimed_bytes());

        let relocate = self.relocate.clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_color(theme.primary).child(IconName::HardDrive))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("磁盘迁移"),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(theme.accent.opacity(0.18))
                    .text_color(theme.foreground)
                    .child(format!("已释放 {}", reclaimed)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(format!("目标：{}", target_label)),
            )
            .child(
                Button::new("relocate-title-rescan")
                    .ghost()
                    .small()
                    .icon(IconName::Redo)
                    .label("重新扫描")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        relocate.update(cx, |p, cx| p.engine.update(cx, |e, cx| e.scan_async(cx)));
                    })),
            )
    }

    /// PDF Editor title bar: rail toggle + file icon + title.
    pub(super) fn render_pdf_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            // Icon + title
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_color(theme.primary).child(vicon(FILE_PDF)))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("pdf.title").to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }

    /// Markdown Editor title bar: rail toggle + filename (with dirty marker) +
    /// New / Open (recent-files popover) / Save / Save-As / Outline / Export.
    pub(super) fn render_markdown_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_component::button::ButtonVariants as _;
        use gpui_component::popover::Popover;
        use gpui_component::{h_flex, v_flex};

        let theme = cx.theme().clone();
        let md = self.markdown.clone();
        let (
            title,
            dirty,
            file_path,
            outline_open,
            view_mode,
            word_total,
            word_sel,
            open_popover,
            export_popover,
            recents,
        ) = {
            let r = md.read(cx);
            let (wt, ws) = r.word_count(cx);
            (
                r.title(cx),
                r.dirty(cx),
                r.file_path(cx),
                r.outline_open(cx),
                r.view_mode(cx),
                wt,
                ws,
                r.open_popover_open(),
                r.export_popover_open(),
                r.recent_files().to_vec(),
            )
        };

        // Button-target clones.
        let md_new = md.clone();
        let md_open = md.clone();
        let md_save = md.clone();
        let md_save_as = md.clone();
        let md_outline = md.clone();
        let md_view_mode = md.clone();
        let md_export_html = md.clone();
        let md_export_pdf = md.clone();
        let md_open_pop = md.clone();
        let md_export_pop = md.clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            // Far-left rail toggle (shared across views, so the window chrome
            // never shifts when switching views).
            .child(self.render_rail_toggle(cx))
            // Outline toggle — opens the document-outline drawer inside the
            // editor. Same leading position + icon/selected/tooltip pattern as
            // the Notes title bar's sidebar-toggle, so the two views read as
            // one consistent sidebar affordance.
            .child(
                Button::new("md-title-outline")
                    .ghost()
                    .small()
                    .icon(if outline_open {
                        IconName::PanelLeft
                    } else {
                        IconName::PanelLeftOpen
                    })
                    .selected(outline_open)
                    .tooltip(if outline_open {
                        rust_i18n::t!("markdown.collapse_outline").to_string()
                    } else {
                        rust_i18n::t!("markdown.outline").to_string()
                    })
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let _ = md_outline.update(cx, |p, cx| p.toggle_outline(window, cx));
                    })),
            )
            // Icon + filename + dirty marker. The whole block is width-bounded
            // (`min_w_0` + `max_w`) so a very long filename/path can ellipsize
            // instead of growing unbounded and starving the right-side menu off
            // screen. `min_w_0` is what lets the flex child actually shrink below
            // its content size so truncation takes effect.
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .min_w_0()
                    .max_w(px(320.))
                    .overflow_hidden()
                    .child(div().flex_shrink_0().text_color(theme.primary).child(vicon(DOCS)))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(title),
                    )
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_color(theme.primary)
                                .child("●"),
                        )
                    })
                    .when_some(file_path, |this, path| {
                        // Click the path to copy the full path to the clipboard
                        // (it's the full display path, already available here).
                        let path_copy = path.clone();
                        let copy_tip = rust_i18n::t!("markdown.copy_path").to_string();
                        let copied_tip = rust_i18n::t!("markdown.path_copied").to_string();
                        this.child(
                            div()
                                .id("md-title-path")
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                // Plain `div` (not `Button`) takes a tooltip via a
                                // closure that builds the tooltip view.
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(copy_tip.clone())
                                        .build(window, cx)
                                })
                                .child(path)
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        path_copy.clone(),
                                    ));
                                    window.push_notification(
                                        gpui_component::notification::Notification::new()
                                            .title(copied_tip.clone())
                                            .autohide(true),
                                        cx,
                                    );
                                }),
                        )
                    }),
            )
            .child(div().flex_1())
            // WYSIWYG formatting toolbar (H1-3 / B I S U / Paragraph / Format).
            .child({
                let editor = md.read(cx).editor().clone();
                self.render_editor_toolbar(&editor, cx)
            })
            // New
            .child(
                Button::new("md-title-new")
                    .small()
                    .ghost()
                    .icon(IconName::Plus)
                    .tooltip(rust_i18n::t!("markdown.new").to_string())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = md_new.update(cx, |p, cx| p.new_document(cx));
                    })),
            )
            // Open (with recent-files popover)
            .child(
                Popover::new("md-open-pop")
                    .anchor(gpui::Anchor::BottomRight)
                    .open(open_popover)
                    .on_open_change(cx.listener(move |_, open: &bool, _, cx| {
                        let _ = md_open_pop.update(cx, |p, cx| p.set_open_popover(*open, cx));
                    }))
                    .trigger(
                        Button::new("md-title-open")
                            .small()
                            .ghost()
                            .icon(IconName::Folder)
                            .tooltip(rust_i18n::t!("markdown.open").to_string()),
                    )
                    .p(px(4.))
                    .child({
                        let md_pick = md_open.clone();
                        let md_dialog = md_open.clone();
                        v_flex()
                            .w(px(260.))
                            .gap(px(2.))
                            .child(if recents.is_empty() {
                                div()
                                    .px_2()
                                    .py_2()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(rust_i18n::t!("markdown.no_recent").to_string())
                                    .into_any_element()
                            } else {
                                v_flex()
                                    .gap(px(2.))
                                    .children(recents.into_iter().enumerate().map(|(i, p)| {
                                        let md_pick = md_pick.clone();
                                        let label = p.display().to_string();
                                        let path = p.clone();
                                        div()
                                            .id(gpui::ElementId::Name(
                                                format!("md-recent-{i}").into(),
                                            ))
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .cursor_pointer()
                                            .hover(|s| s.bg(theme.muted))
                                            .child(div().text_xs().truncate().child(label))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |_, _, _, cx| {
                                                    let _ = md_pick.update(cx, |panel, cx| {
                                                        panel.set_open_popover(false, cx);
                                                        panel.open_path(&path, cx);
                                                    });
                                                }),
                                            )
                                            .into_any_element()
                                    }))
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.muted))
                                    .child(
                                        div()
                                            .text_xs()
                                            .child(rust_i18n::t!("markdown.browse").to_string()),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_, _, _, cx| {
                                            let _ = md_dialog.update(cx, |p, cx| {
                                                p.set_open_popover(false, cx);
                                                p.open_file_dialog(cx);
                                            });
                                        }),
                                    ),
                            )
                    }),
            )
            // Save
            .child(
                Button::new("md-title-save")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::File).path(SAVE))
                    .tooltip(rust_i18n::t!("markdown.save").to_string())
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let _ = md_save.update(cx, |p, cx| p.save(window, cx));
                    })),
            )
            // Save As
            .child(
                Button::new("md-title-save-as")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::File).path(SAVE_AS))
                    .tooltip(rust_i18n::t!("markdown.save_as").to_string())
                    .on_click(cx.listener(move |_, _, window, cx| {
                        let _ = md_save_as.update(cx, |p, cx| p.save_as(window, cx));
                    })),
            )
            // View-mode toggle (Rendered ↔ Source), moved up from the status
            // bar. Single Eye icon + .selected (lit in Source mode) + a tooltip
            // naming the target mode — same affordance pattern as the outline
            // toggle above.
            .child(
                Button::new("md-title-view-mode")
                    .small()
                    .ghost()
                    .selected(view_mode == crate::wysiwyg::editor::ViewMode::Source)
                    .icon(IconName::Eye)
                    .tooltip(if view_mode == crate::wysiwyg::editor::ViewMode::Source {
                        rust_i18n::t!("markdown.switch_to_rendered").to_string()
                    } else {
                        rust_i18n::t!("markdown.switch_to_source").to_string()
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = md_view_mode.update(cx, |p, cx| p.toggle_view_mode(cx));
                    })),
            )
            // Word count. Reads "N 字" normally, or "sel / total 字" with a
            // selection. Plain muted text label, no interaction.
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(if let Some(sel) = word_sel {
                        format!(
                            "{} / {} {}",
                            sel,
                            word_total,
                            rust_i18n::t!("markdown.word_suffix")
                        )
                    } else {
                        format!("{} {}", word_total, rust_i18n::t!("markdown.word_suffix"))
                    }),
            )
            // Export popover (HTML / PDF)
            .child(
                Popover::new("md-export-pop")
                    .anchor(gpui::Anchor::BottomRight)
                    .open(export_popover)
                    .on_open_change(cx.listener(move |_, open: &bool, _, cx| {
                        let _ = md_export_pop.update(cx, |p, cx| p.set_export_popover(*open, cx));
                    }))
                    .trigger(
                        Button::new("md-title-export")
                            .small()
                            .ghost()
                            .icon({
                                let mut icon = Icon::new(IconName::ExternalLink);
                                icon = icon.path(EXPORT);
                                icon
                            })
                            .tooltip(rust_i18n::t!("notes.export").to_string()),
                    )
                    .p(px(4.))
                    .child(
                        v_flex()
                            .w(px(170.))
                            .gap(px(2.))
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.muted))
                                    .child(
                                        div().text_size(px(12.)).child(
                                            rust_i18n::t!("markdown.export_html").to_string(),
                                        ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_, _, window, cx| {
                                            let _ = md_export_html
                                                .update(cx, |p, cx| p.export_html(window, cx));
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.muted))
                                    .child(
                                        div().text_size(px(12.)).child(
                                            rust_i18n::t!("markdown.export_pdf").to_string(),
                                        ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_, _, window, cx| {
                                            let _ = md_export_pdf
                                                .update(cx, |p, cx| p.export_pdf(window, cx));
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }

    /// AutoTest title bar: rail toggle + icon/title + suite count +
    /// add-suite/add-case/run actions. The body renders no internal header.
    pub(super) fn render_autotest_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let at = self.autotest.clone();
        let (has_project, suite_count, running, has_case) = {
            let r = at.read(cx);
            (
                r.has_active_project(cx),
                r.suite_count(cx),
                r.running,
                r.selected_case.is_some(),
            )
        };

        let at_add_suite = at.clone();
        let at_add_case = at.clone();
        let at_run = at.clone();

        h_flex()
            .h(px(40.))
            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
            .when(!cfg!(target_os = "macos"), |this| this.pl_3())
            .pr_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .child(self.render_rail_toggle(cx))
            // Icon + title + suite count badge
            .child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .child(div().text_color(theme.primary).child(IconName::Play))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("view.autotest").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme.accent.opacity(0.2))
                            .text_color(theme.foreground)
                            .child(
                                rust_i18n::t!("autotest.suite_count", count = suite_count)
                                    .to_string(),
                            ),
                    ),
            )
            .child(div().flex_1())
            // Add suite
            .child(
                Button::new("autotest-title-add-suite")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label("新建套件")
                    .disabled(!has_project)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = at_add_suite.update(cx, |p, cx| p.add_suite_from_title(cx));
                    })),
            )
            // Add case
            .child(
                Button::new("autotest-title-add-case")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label("新建用例")
                    .disabled(!has_project || suite_count == 0)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = at_add_case.update(cx, |p, cx| p.add_case_from_title(cx));
                    })),
            )
            // Run
            .child(
                Button::new("autotest-title-run")
                    .primary()
                    .small()
                    .icon(if running {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .label(if running {
                        "运行中…"
                    } else {
                        "运行用例"
                    })
                    .disabled(!has_project || !has_case || running)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = at_run.update(cx, |p, cx| p.run_selected_from_title(cx));
                    })),
            )
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }
}
