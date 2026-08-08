//! Workspace / project / environment management actions: create, switch,
//! delete workspaces (each maps to a git branch), project & env dialogs,
//! mock-rule generation, project export, and collection import.

use super::sanitize;
use super::{PendingDialog, VerveApp};
use crate::share::models::ShareScope;
use crate::state::{AppEvent, AppState};
use gpui::prelude::FluentBuilder as _;
use gpui::{img, *};
use gpui_component::{
    ActiveTheme, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

impl VerveApp {
    pub(super) fn open_kv_manager(
        &mut self,
        scope: crate::ui::kv_manager_view::KvScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = scope.title().to_string();
        let view = cx.new(|cx| {
            crate::ui::kv_manager_view::KvManagerView::new(self.state.clone(), scope, window, cx)
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            dialog
                .title(title.clone())
                .content(move |content, _, _| content.child(div().p_4().child(view.clone())))
        });
    }

    pub(super) fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| gpui_component::input::InputState::new(window, cx).placeholder("项目名称"));
        let state = self.state.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            // Clone per-closure so content and footer each own a handle.
            let input_content = input.clone();
            let input_footer = input.clone();
            let state_footer = state.clone();
            dialog
                .title("新建项目")
                .content(move |content, _, _| {
                    content.child(
                        div()
                            .p_4()
                            .w(px(360.))
                            .child(gpui_component::input::Input::new(&input_content)),
                    )
                })
                .footer({
                    // The footer is a plain element; the confirm button reads the
                    // input value, creates the project, and closes the dialog.
                    gpui_component::button::Button::new("confirm-project")
                        .primary()
                        .small()
                        .label("创建")
                        .on_click(move |_, window, cx| {
                            let name = input_footer.read(cx).value().to_string();
                            if !name.trim().is_empty() {
                                let _ = state_footer.update(cx, |s, cx| {
                                    s.data
                                        .projects
                                        .push(crate::state::models::Project::new(name));
                                    s.active_project = s.data.projects.len() - 1;
                                    s.selected_request = None;
                                    s.selected_folder = None;
                                    s.open_request_ids.clear();
                                    s.active_tab_id = None;
                                    s.notify_workspace(cx);
                                });
                            }
                            window.close_dialog(cx);
                        })
                })
        });
    }

    pub(super) fn switch_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        log::info!("switch_workspace: 切换到 {id}");
        // 1. Persist the current workspace's data to disk (its branch).
        self.state.update(cx, |s, cx| s.persist(cx));
        // 2. If git is initialized, switch the branch (commits dirty first).
        let git_ready = self.git.read(cx).initialized;
        let target_branch = {
            let idx = crate::state::persistence::load_workspaces_index();
            idx.find(&id).map(|w| w.branch.clone())
        };
        if git_ready {
            if let Some(branch) = target_branch.clone() {
                // Commit current changes on the old branch before switching.
                self.git.update(cx, |g, cx| {
                    // switch_branch_async does create_or_checkout.
                    g.switch_branch_async(branch.clone(), cx);
                });
                // The actual data reload happens after the git op completes
                // (detected via busy clearing). For simplicity, schedule a
                // reload after a short delay — the branch switch is fast (local).
                let state = self.state.clone();
                let wid = id.clone();
                cx.spawn(async move |this, cx| {
                    // Wait for the git op to settle (branch switch + refresh).
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(800))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        crate::state::persistence::set_active_workspace(&wid);
                        this.state.update(cx, |s, cx| {
                            let active = crate::state::persistence::load_workspaces_index().active;
                            s.reload_from_disk(active);
                            cx.emit(crate::state::AppEvent::WorkspaceSwitched);
                            cx.emit(crate::state::AppEvent::WorkspaceChanged);
                        });
                        this.pending_switcher_refresh = true;
                        cx.notify();
                    });
                    let _ = state;
                })
                .detach();
                return;
            }
        }
        // 3. Git not ready: just swap the active marker + reload (degraded mode).
        crate::state::persistence::set_active_workspace(&id);
        self.state.update(cx, |s, cx| {
            let active = crate::state::persistence::load_workspaces_index().active;
            s.reload_from_disk(active);
            cx.emit(crate::state::AppEvent::WorkspaceSwitched);
            cx.emit(crate::state::AppEvent::WorkspaceChanged);
        });
        self.pending_switcher_refresh = true;
        cx.notify();
    }

    pub(super) fn create_workspace(&mut self, name: String, cx: &mut Context<Self>) {
        log::info!("create_workspace: 创建 {name}");
        let meta = crate::state::models::WorkspaceMeta::new(name.clone());
        let branch = meta.branch.clone();
        let id = meta.id.clone();
        // 1. Persist current workspace data.
        self.state.update(cx, |s, cx| s.persist(cx));
        let git_ready = self.git.read(cx).initialized;
        if git_ready {
            // 2. Create + checkout the new branch.
            self.git.update(cx, |g, cx| {
                g.switch_branch_async(branch.clone(), cx);
            });
            let state = self.state.clone();
            let meta_clone = meta.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(800))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    // 3. Write empty workspace data for the new workspace.
                    let empty = crate::state::models::WorkspaceData::default();
                    let _ = crate::state::persistence::save(&empty);
                    // 4. Commit the empty workspace on the new branch.
                    if let Ok(repo) = crate::git::ops::ensure_repo(
                        &crate::state::persistence::data_dir().unwrap_or_default(),
                    ) {
                        let _ = crate::git::ops::commit(
                            &repo,
                            &format!("初始化工作空间 {}", meta_clone.name),
                        );
                    }
                    // 5. Update the index + reload.
                    let mut idx = crate::state::persistence::load_workspaces_index();
                    idx.workspaces.push(meta_clone.clone());
                    idx.active = Some(meta_clone.id.clone());
                    let _ = crate::state::persistence::save_workspaces_index(&idx);
                    this.state.update(cx, |s, cx| {
                        let active = crate::state::persistence::load_workspaces_index().active;
                        s.reload_from_disk(active);
                        cx.emit(crate::state::AppEvent::WorkspaceSwitched);
                        cx.emit(crate::state::AppEvent::WorkspaceChanged);
                    });
                    this.pending_switcher_refresh = true;
                    cx.notify();
                });
                let _ = state;
            })
            .detach();
        } else {
            // Git not ready: just add to index + reload (degraded mode).
            let mut idx = crate::state::persistence::load_workspaces_index();
            idx.workspaces.push(meta.clone());
            idx.active = Some(id);
            let _ = crate::state::persistence::save_workspaces_index(&idx);
            self.state.update(cx, |s, cx| {
                let active = crate::state::persistence::load_workspaces_index().active;
                s.reload_from_disk(active);
                cx.emit(crate::state::AppEvent::WorkspaceSwitched);
                cx.emit(crate::state::AppEvent::WorkspaceChanged);
            });
            self.pending_switcher_refresh = true;
            cx.notify();
        }
    }

    pub(super) fn delete_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        let idx = crate::state::persistence::load_workspaces_index();
        let Some(meta) = idx.find(&id).cloned() else {
            log::warn!("delete_workspace: {id} 不存在");
            return;
        };
        if meta.is_default {
            log::warn!("delete_workspace: 不能删除 default workspace");
            return;
        }
        log::info!("delete_workspace: 删除 {} ({})", meta.name, meta.branch);
        let branch = meta.branch.clone();
        let is_active = idx.active.as_deref() == Some(&id);
        let git_ready = self.git.read(cx).initialized;

        // If deleting the active workspace, switch to default first.
        if is_active {
            self.switch_workspace("default".to_string(), cx);
        }
        // Delete the git branch (must have switched off it first).
        if git_ready {
            self.git.update(cx, |g, cx| {
                g.delete_branch_async(branch.clone(), cx);
            });
        }
        // Remove from index.
        let mut idx = crate::state::persistence::load_workspaces_index();
        idx.workspaces.retain(|w| w.id != id);
        if idx.active.as_deref() == Some(&id) {
            idx.active = Some("default".to_string());
        }
        let _ = crate::state::persistence::save_workspaces_index(&idx);
        self.pending_switcher_refresh = true;
        cx.notify();
    }

    pub(super) fn open_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx).placeholder("工作空间名称")
        });
        // Store the input so the confirm handler (a pending flag reconciled in
        // render) can read it after the dialog closes.
        self.pending_workspace_name_input = Some(input.clone());
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let input_content = input.clone();
            let input_footer = input.clone();
            dialog
                .title("新建工作空间")
                .content(move |content, _, _| {
                    content.child(
                        div()
                            .p_4()
                            .w(px(360.))
                            .child(gpui_component::input::Input::new(&input_content)),
                    )
                })
                .footer({
                    gpui_component::button::Button::new("confirm-workspace")
                        .primary()
                        .small()
                        .label("创建")
                        .on_click(move |_, window, cx| {
                            // Signal that a workspace creation is pending; the name
                            // is read from the stored input entity on reconciliation.
                            let _ = input_footer.read(cx).value().to_string();
                            window.close_dialog(cx);
                        })
                })
        });
        // Mark pending so render() picks it up and calls create_workspace.
        self.pending_new_workspace = true;
        cx.notify();
    }

    pub(super) fn open_new_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| gpui_component::input::InputState::new(window, cx).placeholder("环境名称"));
        let state = self.state.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            // Clone per-closure so content and footer each own a handle.
            let input_content = input.clone();
            let input_footer = input.clone();
            let state_footer = state.clone();
            dialog
                .title("新建环境")
                .content(move |content, _, _| {
                    content.child(
                        div()
                            .p_4()
                            .w(px(360.))
                            .child(gpui_component::input::Input::new(&input_content)),
                    )
                })
                .footer({
                    gpui_component::button::Button::new("confirm-env")
                        .primary()
                        .small()
                        .label("创建")
                        .on_click(move |_, window, cx| {
                            let name = input_footer.read(cx).value().to_string();
                            if !name.trim().is_empty() {
                                let _ = state_footer.update(cx, |s, cx| {
                                    if let Some(project) = s.active_project_mut() {
                                        project
                                            .environments
                                            .push(crate::state::models::Environment::new(name));
                                    }
                                    s.notify_workspace(cx);
                                });
                            }
                            window.close_dialog(cx);
                        })
                })
        });
    }

    pub(super) fn open_project_settings(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_name = self
            .state
            .read(cx)
            .data
            .projects
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let theme = cx.theme().clone();
        let theme_border = theme.border;
        let theme_muted = theme.muted_foreground;
        let input = cx.new(|cx| {
            let mut s = gpui_component::input::InputState::new(window, cx).placeholder("项目名称");
            s.set_value(current_name, window, cx);
            s
        });
        let state = self.state.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let input_content = input.clone();
            let input_footer = input.clone();
            let state_footer = state.clone();
            let state_for_content = state.clone();
            dialog.title("项目设置").content(move |content, _, _| {
                // Clone per-render so the delete button's on_click (a Fn) can
                // own its own handle each time content is built.
                let state_del = state_for_content.clone();
                content.child(
                    v_flex()
                        .p_4()
                        .w(px(400.))
                        .gap_3()
                        .child(
                            v_flex().gap_1().child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("项目名称"),
                            ).child(gpui_component::input::Input::new(&input_content)),
                        )
                        // Danger zone: delete the project.
                        .child(
                            div()
                                .h(px(1.))
                                .w_full()
                                .bg(theme_border),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .text_color(theme_muted)
                                        .child("删除项目将移除其下所有接口与目录，且不可恢复。"),
                                )
                                .child(
                                    gpui_component::button::Button::new("project-delete")
                                        .ghost()
                                        .small()
                                        .text_color(gpui::red())
                                        .label("删除项目")
                                        .on_click(move |_, window, cx| {
                                            // Close this settings dialog first, then
                                            // open a confirmation dialog.
                                            window.close_dialog(cx);
                                            let state_del = state_del.clone();
                                            window.open_dialog(cx, move |dialog, _window, _cx| {
                                                dialog.title("确认删除").content(move |content, _, _| {
                                                    content.child(
                                                        v_flex()
                                                            .p_4()
                                                            .w(px(360.))
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .text_color(theme_muted)
                                                                    .child("确定要删除该项目吗？所有接口与目录将被移除，且不可撤销。"),
                                                            ),
                                                    )
                                                })
                                                .footer({
                                                    let state_confirm = state_del.clone();
                                                    gpui_component::button::Button::new("confirm-project-delete")
                                                        .primary()
                                                        .small()
                                                        .label("确认删除")
                                                        .on_click(move |_, window, cx| {
                                                            let _ = state_confirm.update(cx, |s, cx| {
                                                                if s.data.projects.len() > idx {
                                                                    s.data.projects.remove(idx);
                                                                    if s.active_project >= s.data.projects.len() {
                                                                        s.active_project = s.data.projects.len().saturating_sub(1);
                                                                    }
                                                                    s.selected_request = None;
                                                                    s.selected_folder = None;
                                                                    s.open_request_ids.clear();
                                                                    s.active_tab_id = None;
                                                                    s.notify_workspace(cx);
                                                                }
                                                            });
                                                            window.close_dialog(cx);
                                                        })
                                                })
                                            });
                                        }),
                                ),
                        ),
                )
            })
            .footer({
                gpui_component::button::Button::new("confirm-project-rename")
                    .primary()
                    .small()
                    .label("保存")
                    .on_click(move |_, window, cx| {
                        let name = input_footer.read(cx).value().to_string();
                        if !name.trim().is_empty() {
                            let _ = state_footer.update(cx, |s, cx| {
                                if let Some(project) = s.data.projects.get_mut(idx) {
                                    project.name = name;
                                }
                                s.notify_workspace(cx);
                            });
                        }
                        window.close_dialog(cx);
                    })
            })
        });
    }

    pub(super) fn confirm_delete_project(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project_name = self
            .state
            .read(cx)
            .data
            .projects
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "该项目".to_string());
        let theme = cx.theme().clone();
        let theme_muted = theme.muted_foreground;
        let state = self.state.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let name = project_name.clone();
            dialog
                .title("确认删除")
                .content(move |content, _, _| {
                    content.child(v_flex().p_4().w(px(360.)).gap_2().child(
                        div().text_sm().text_color(theme_muted).child(format!(
                            "确定要删除项目「{}」吗？所有接口与目录将被移除，且不可撤销。",
                            name
                        )),
                    ))
                })
                .footer({
                    let state_del = state.clone();
                    gpui_component::button::Button::new("confirm-project-delete")
                        .primary()
                        .small()
                        .label("删除")
                        .on_click(move |_, window, cx| {
                            let _ = state_del.update(cx, |s, cx| {
                                if s.data.projects.len() > idx {
                                    s.data.projects.remove(idx);
                                    if s.active_project >= s.data.projects.len() {
                                        s.active_project = s.data.projects.len().saturating_sub(1);
                                    }
                                    s.selected_request = None;
                                    s.selected_folder = None;
                                    s.open_request_ids.clear();
                                    s.active_tab_id = None;
                                    s.notify_workspace(cx);
                                }
                            });
                            window.close_dialog(cx);
                        })
                })
        });
    }

    pub(super) fn confirm_delete_env(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let theme = cx.theme().clone();
        let theme_muted = theme.muted_foreground;
        let state = self.state.clone();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let name = env_name.clone();
            let id_del = id.clone();
            dialog
                .title("确认删除")
                .content(move |content, _, _| {
                    content.child(
                        v_flex().p_4().w(px(360.)).gap_2().child(
                            div()
                                .text_sm()
                                .text_color(theme_muted)
                                .child(format!("确定要删除环境「{}」吗？此操作不可撤销。", name)),
                        ),
                    )
                })
                .footer({
                    let state_del = state.clone();
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

    pub(super) fn generate_mocks(&mut self, cx: &mut Context<Self>) {
        let app = self.state.read(cx);
        let idx = app.active_project;
        let mut s = app.data.clone();
        let _ = app;
        let Some(p) = s.projects.get_mut(idx) else {
            return;
        };
        let generated = crate::mock::generate_missing(p);
        if generated.is_empty() {
            return;
        }
        let total = generated.len();
        let mut map: std::collections::HashMap<String, crate::state::models::MockRule> =
            generated.into_iter().collect();
        for r in p.requests.iter_mut() {
            if let Some(m) = map.remove(&r.id) {
                r.mock = Some(m);
            }
        }
        fn walk(
            folders: &mut [crate::state::models::Folder],
            map: &mut std::collections::HashMap<String, crate::state::models::MockRule>,
        ) {
            for r in folders.iter_mut().flat_map(|f| f.requests.iter_mut()) {
                if let Some(m) = map.remove(&r.id) {
                    r.mock = Some(m);
                }
            }
            for sub in folders.iter_mut() {
                walk(&mut sub.folders, map);
            }
        }
        walk(&mut p.folders, &mut map);
        self.state.update(cx, |app, cx| {
            app.data = s;
            app.notify_workspace(cx);
        });
        log::info!("mock: generated defaults for {total} request(s)");

        // Show friendly success notification with next steps.
        let remote_cfg = crate::state::persistence::load_remote_config();
        let mock_url = crate::share::remote::mock_server_url(&remote_cfg);
        let total_copy = total;
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |_this, window, cx| {
                window.push_notification(
                    gpui_component::notification::Notification::new()
                        .title("Mock 规则生成成功")
                        .message(format!(
                            "✅ 已为 {} 个接口生成默认Mock规则\n\n👉 下一步：在URL中使用 {{{{mock_server}}}} 变量，或直接将API基础地址替换为 {} 即可调用\n📝 可在接口详情的Mock标签页自定义状态码/延迟/响应体",
                            total_copy, mock_url
                        ))
                        .autohide(false),
                    cx,
                );
            });
        })
        .detach();
    }

    pub(super) fn export_project(&mut self, format: crate::export::Format, cx: &mut Context<Self>) {
        let project = match self.state.read(cx).active_project().cloned() {
            Some(p) => p,
            None => return,
        };
        let content = match format {
            crate::export::Format::Markdown => crate::export::project_to_markdown(&project),
            crate::export::Format::Json => match crate::export::project_to_json(&project) {
                Ok(s) => s,
                Err(_) => return,
            },
            crate::export::Format::Apipost => match crate::export::project_to_apipost(&project) {
                Ok(s) => s,
                Err(_) => return,
            },
            crate::export::Format::PostmanV2_1 => {
                match crate::export::project_to_postman_v2_1(&project) {
                    Ok(s) => s,
                    Err(_) => return,
                }
            }
            crate::export::Format::Swagger => match crate::export::project_to_swagger(&project) {
                Ok(s) => s,
                Err(_) => return,
            },
            crate::export::Format::OpenApi3 => match crate::export::project_to_openapi(&project) {
                Ok(s) => s,
                Err(_) => return,
            },
        };
        // Write into the user data dir so the export is always reachable.
        let dir = match crate::state::persistence::data_dir() {
            Ok(d) => d.join("exports"),
            Err(_) => return,
        };
        let _ = std::fs::create_dir_all(&dir);
        let filename = format!("{}.{}", sanitize(&project.name), format.extension());
        let path = dir.join(filename);
        if let Err(e) = std::fs::write(&path, content) {
            log::error!("export write failed: {e:?}");
        } else {
            log::info!("exported to {:?}", path);
        }
    }

    pub(super) fn import_collection(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                "Import collection (Apipost / Postman v2.1 / Swagger / OpenAPI 3 JSON)".into(),
            ),
        });
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await {
                if let Some(path) = paths.first() {
                    if let Ok(contents) = std::fs::read_to_string(path) {
                        let project = if contents.contains("\"openapi\"") {
                            crate::import::openapi_v3(&contents)
                        } else if contents.contains("\"swagger\"") {
                            crate::import::swagger_v2(&contents)
                        } else if contents.contains("\"project_id\"")
                            && contents.contains("\"target_type\"")
                        {
                            // postman export format.
                            crate::import::postman(&contents)
                        } else {
                            crate::import::postman_v2_1(&contents)
                        };
                        if let Ok(project) = project {
                            let _ = state.update(cx, |s, cx| {
                                s.active_project = s.data.projects.len();
                                s.data.active_project_id = Some(project.id.clone());
                                s.data.projects.push(project);
                                s.notify_workspace(cx);
                            });
                        }
                    }
                }
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}
