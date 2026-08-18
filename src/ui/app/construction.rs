//! VerveApp construction: the `new()` constructor, the startup update
//! check, the pending-Markdown-file watcher, and the manual update flow.
//! All app-wide wiring (panels, git, share server, subscriptions) happens here.

use super::{SideView, VerveApp};
use crate::git::GitState;
use crate::share::models::ShareScope;
use crate::share::server::{self, ShareServer};
use crate::state::{AppEvent, AppState};
use crate::ui::autotest_panel::AutoTestPanel;
use crate::ui::console_panel::{ConsoleEvent, ConsolePanel};
use crate::ui::docker_panel::DockerPanel;
use crate::ui::hosts_panel::HostsPanel;
use crate::ui::json_panel::JsonPanel;
use crate::ui::k8s_panel::K8sPanel;
use crate::ui::markdown_panel::MarkdownPanel;
use crate::ui::mock_console_panel::{MockConsoleEvent, MockConsolePanel};
use crate::ui::notes_panel::NotesPanel;
use crate::ui::pdf_panel::PdfPanel;
use crate::ui::project_manage_panel::ProjectManagePanel;
use crate::ui::project_tree_panel::ProjectTreePanel;
use crate::ui::proxy_panel::ProxyPanel;
use crate::ui::relocate_panel::RelocatePanel;
use crate::ui::request_panel::RequestPanel;
use crate::ui::response_panel::ResponsePanel;
use crate::ui::share_panel::{ShareEvent, SharePanel};
use crate::ui::ssh_panel::SshPanel;
use crate::ui::stress_panel::StressPanel;
use gpui::prelude::FluentBuilder as _;
use gpui::{img, *};
use gpui_component::resizable::ResizableState;
use gpui_component::{
    ActiveTheme, Sizable as _, WindowExt as _,
    select::{SelectEvent, SelectState},
};

impl VerveApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let tree = cx.new(|cx| ProjectTreePanel::new(state.clone(), window, cx));
        let request = cx.new(|cx| RequestPanel::new(state.clone(), window, cx));
        let response = cx.new(|cx| ResponsePanel::new(state.clone(), window, cx));
        let console = cx.new(|cx| ConsolePanel::new(state.clone(), cx));
        let mock_console = cx.new(|cx| MockConsolePanel::new(state.clone(), cx));

        // Environment switcher.
        let env_names: Vec<String> = state
            .read(cx)
            .active_project()
            .map(|p| {
                let mut v = vec!["No environment".to_string()];
                v.extend(p.environments.iter().map(|e| e.name.clone()));
                v
            })
            .unwrap_or_default();
        let env_select = cx.new(|cx| {
            SelectState::new(
                env_names,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });

        // Project switcher: one entry per project in the workspace.
        let project_names: Vec<String> = state
            .read(cx)
            .data
            .projects
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let project_select = cx.new(|cx| {
            SelectState::new(
                project_names.clone(),
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });

        // Resizable panel groups. Sizes are restored on render from the saved
        // layout via the panel `.size()` builder; `on_resize` persists changes.
        let main_resize = cx.new(|_| ResizableState::default());
        let center_resize = cx.new(|_| ResizableState::default());

        // Git version-control state. Initialised at the workspace data dir
        // (`~/.verve`); loads persisted auth/remote/auto-push config first.
        let data_dir =
            crate::state::persistence::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let git = GitState::init(data_dir.clone(), cx);
        {
            // Ensure the git repo exists (pure-local init, no remote/token
            // needed) and the .gitignore excludes cross-branch files, so every
            // workspace gets its own branch from the start.
            if let Ok(repo) = crate::git::ops::ensure_repo(&data_dir) {
                crate::state::persistence::ensure_gitignore();
                // Checkout the active workspace's branch (create if missing).
                let ws_idx = crate::state::persistence::load_workspaces_index();
                if let Some(branch) = ws_idx.active_meta().map(|m| m.branch.clone()) {
                    let no_auth = crate::git::ops::GitAuth::default();
                    if let Err(e) = crate::git::ops::create_or_checkout(&repo, &branch, &no_auth) {
                        log::warn!("启动时切换到 workspace 分支 {branch} 失败: {e}");
                    }
                }
            }
            let cfg = crate::state::persistence::load_git_config();
            let sync_minutes = cfg
                .sync_interval_minutes
                .unwrap_or(crate::state::persistence::DEFAULT_SYNC_INTERVAL_MINUTES);
            let has_remote = cfg.remote.is_some();
            git.update(cx, |g, cx| {
                g.load_config(
                    cfg.auto_commit,
                    cfg.auto_push,
                    cfg.remote.clone(),
                    cfg.username.clone(),
                    cfg.token.clone(),
                );
                // Trigger an initial refresh now that config is loaded.
                g.refresh_async(cx);
                // On startup, if a remote is configured, pull latest changes.
                if has_remote {
                    g.pull_async(cx);
                }
                // Auto-sync on the configured interval: commit dirty changes +
                // push/pull so work is versioned without manual intervention.
                g.start_auto_sync(
                    cx,
                    std::time::Duration::from_secs((sync_minutes as u64) * 60),
                );
            });
        }
        // Reload workspace data from the now-correct branch.
        state.update(cx, |s, _cx| {
            s.reload_from_disk(crate::state::persistence::load_workspaces_index().active);
        });
        let project_manage =
            cx.new(|cx| ProjectManagePanel::new(git.clone(), state.clone(), window, cx));
        let share = cx.new(|cx| SharePanel::new(state.clone(), window, cx));
        let ssh = cx.new(|cx| SshPanel::new(window, cx));
        let autotest = cx.new(|cx| AutoTestPanel::new(state.clone(), window, cx));
        let stress = cx.new(|cx| StressPanel::new(state.clone(), window, cx));
        let proxy = cx.new(|cx| ProxyPanel::new(window, cx));
        let k8s = cx.new(|cx| K8sPanel::new(window, cx));
        let docker = cx.new(|cx| DockerPanel::new(window, cx));
        let hosts = cx.new(|cx| HostsPanel::new(state.clone(), window, cx));
        let notes = cx.new(|cx| NotesPanel::new(state.clone(), window, cx));
        let markdown = cx.new(|cx| MarkdownPanel::new(state.clone(), window, cx));
        let json = cx.new(|cx| JsonPanel::new(window, cx));
        let pdf = cx.new(|cx| PdfPanel::new(window, cx));
        let relocate = cx.new(|cx| RelocatePanel::new(window, cx));

        // Document-sharing: load configs and start the local HTTP server if any
        // shares already exist. The server resolves projects from disk
        // (`workspace.json`) at request time so edits show up live without a
        // restart — the app persists to disk on a debounce, so a fresh read
        // always reflects current edits.
        let remote_cfg = crate::state::persistence::load_remote_config();
        let share_host = remote_cfg.public_host.clone().unwrap_or_else(|| {
            if remote_cfg.host == "0.0.0.0" {
                "127.0.0.1".to_string()
            } else {
                remote_cfg.host.clone()
            }
        });
        let share_port = remote_cfg.port;
        let active_ws_id = crate::state::persistence::load_workspaces_index().active;
        let tenant_id = remote_cfg
            .multi_tenant
            .then(|| active_ws_id.clone().unwrap_or_default());
        let share_configs = server::config_store(crate::share::persist::load_shares());

        // Build initial mock rules from active project.
        let initial_mock: Vec<crate::mock::RuleEntry> = state
            .read(cx)
            .active_project()
            .map(|p| {
                std::sync::Arc::try_unwrap(crate::mock::rule_map(p))
                    .unwrap_or_else(|arc| (*arc).clone())
            })
            .unwrap_or_default();
        let mock_shared = crate::mock::shared_rules(initial_mock);
        let mock_shared_for_state = mock_shared.clone();
        state.update(cx, move |s, _| {
            s.mock_rules = Some(mock_shared_for_state);
        });

        // Only start local server when not in pure cloud/remote mode (public_host configured).
        let is_cloud_mode = crate::share::remote::is_cloud_mode(&remote_cfg);
        let share_server = if !is_cloud_mode {
            Some(server::start_desktop_with_config(
                remote_cfg.host.clone(),
                remote_cfg.port,
                remote_cfg.auth_token.clone(),
                tenant_id,
                share_configs.clone(),
                |project_id| {
                    let data = crate::state::persistence::load_or_default();
                    data.projects.iter().find(|p| p.id == project_id).cloned()
                },
                Some(mock_shared),
            ))
        } else {
            None
        };

        // Focus the app root so keybinding actions fire even when no panel
        // has been focused/clicked yet (see the field docs on `focus_handle`).
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        let mut app = Self {
            focus_handle,
            state: state.clone(),
            tree,
            request,
            response,
            console,
            mock_console,
            git,
            project_manage,
            share,
            ssh,
            autotest,
            stress,
            proxy,
            k8s,
            docker,
            hosts,
            notes,
            markdown,
            json,
            pdf,
            relocate,
            help_window: None,
            share_server,
            share_configs,
            share_host,
            share_port,
            active_workspace_id_cache: active_ws_id,
            env_select,
            project_select,
            show_console: false,
            active_view: SideView::Api,
            home_view: crate::state::persistence::load_layout()
                .and_then(|l| l.home_view.as_deref().map(SideView::parse))
                .unwrap_or(SideView::Api),
            pending_switcher_refresh: false,
            sidebar_collapsed: false,
            rail_collapsed: false,
            hidden_rails: crate::state::persistence::load_hidden_rails(),
            theme_popover_open: false,
            lang_popover_open: false,
            export_popover_open: false,
            project_popover_open: false,
            workspace_popover_open: false,
            env_popover_open: false,
            pending_new_project: false,
            pending_new_workspace: false,
            pending_workspace_name_input: None,
            pending_new_env: false,
            pending_dialog: None,
            pending_share: None,
            applied_sync_interval: crate::state::persistence::DEFAULT_SYNC_INTERVAL_MINUTES,
            main_resize: main_resize.clone(),
            center_resize: center_resize.clone(),
            saved_layout: crate::state::persistence::load_layout(),
            update_info: None,
            update_checking: false,
            update_check_result: None,
            rail_order: {
                let saved = crate::state::persistence::load_rail_order();
                let default: Vec<String> =
                    SideView::ALL.iter().map(|v| v.name().to_string()).collect();
                let mut order = match saved {
                    None => default,
                    Some(mut order) => {
                        let existing: std::collections::HashSet<_> =
                            order.iter().cloned().collect();
                        // Append new default items missing from saved order
                        for name in &default {
                            if !existing.contains(name) {
                                order.push(name.clone());
                            }
                        }
                        // Remove items no longer in defaults
                        let default_set: std::collections::HashSet<_> =
                            default.into_iter().collect();
                        order.retain(|n| default_set.contains(n));
                        order
                    }
                };
                // One-time reorder: put the fixed-shortcut views (⌘1..⌘5) at
                // the front so the rail matches the key order. Runs once (not
                // every launch) so later drag-to-reorder isn't overridden.
                if !crate::state::persistence::rail_shortcut_migrated() {
                    shortcut_first_rail_order(&mut order);
                    crate::state::persistence::save_rail_order(&order);
                    crate::state::persistence::mark_rail_shortcut_migrated();
                }
                order
            },
            dragging_rail: None,
            rail_drop_target: None,
            _subs: Vec::new(),
        };

        // Persist on workspace edits (debounced by coalescing).
        let sub_edited = cx.subscribe(&state, |this, _src, ev: &AppEvent, cx| {
            match ev {
                // Structural changes persist immediately and may have renamed a
                // project / changed environments, so the switchers must rebuild.
                AppEvent::WorkspaceChanged => {
                    this.state.update(cx, |s, cx| s.persist(cx));
                    this.pending_switcher_refresh = true;
                    cx.notify();
                }
                // A workspace switch reloaded the data; refresh everything.
                AppEvent::WorkspaceSwitched => {
                    this.pending_switcher_refresh = true;
                    cx.notify();
                }
                // Field edits use a debounced save (scheduled in notify_edited).
                AppEvent::RequestEdited => {}
                // A persist just landed on disk — kick a git auto-commit (and
                // optional auto-push) so changes are versioned without the user
                // having to think about it. The Persisted event already coalesces
                // with the 1s debounce, so we don't double-commit.
                AppEvent::Persisted => {
                    let git = this.git.clone();
                    let _ = git.update(cx, |g, cx| {
                        if g.initialized && g.auto_commit && !g.is_busy() {
                            log::info!("Persisted → 自动同步触发 (auto_commit=true)");
                            g.sync_async(None, cx);
                        } else if g.is_busy() {
                            log::info!("Persisted → 已有同步进行中，跳过自动同步");
                        } else if !g.initialized {
                            log::info!("Persisted → git 未初始化，跳过自动同步");
                        }
                    });
                }
                // Share a single API: resolve the request's name, then defer
                // the dialog open to render (where a Window is available).
                AppEvent::ShareRequest(id) => {
                    let (target_name, found) = this
                        .state
                        .read(cx)
                        .active_project()
                        .and_then(|p| p.find_request(id).map(|(_, r)| r.name.clone()))
                        .map(|n| (Some(n), true))
                        .unwrap_or((None, false));
                    if found {
                        this.pending_share = Some((
                            crate::share::models::ShareScope::Request,
                            Some(id.clone()),
                            target_name,
                        ));
                        cx.notify();
                    }
                }
                _ => {}
            }
        });

        // Env switcher: update the active environment when the user picks one.
        let env_select_handle = app.env_select.clone();
        let sub_env = cx.subscribe(
            &app.env_select,
            move |this, _src, _ev: &SelectEvent<Vec<String>>, cx| {
                let chosen = env_select_handle.read(cx).selected_value().cloned();
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        // "No environment" (index 0) maps to None; otherwise match by name.
                        let new_id = match chosen.as_deref() {
                            Some(name) if name != "No environment" => project
                                .environments
                                .iter()
                                .find(|e| e.name == name)
                                .map(|e| e.id.clone()),
                            _ => None,
                        };
                        project.active_environment = new_id;
                        cx.emit(AppEvent::EnvironmentChanged);
                    }
                });
            },
        );

        // Project switcher: change the active project when the user picks one.
        let project_select_handle = app.project_select.clone();
        let sub_project = cx.subscribe(
            &app.project_select,
            move |this, _src, _ev: &SelectEvent<Vec<String>>, cx| {
                let chosen = project_select_handle.read(cx).selected_value().cloned();
                this.state.update(cx, |s, cx| {
                    if let Some(name) = chosen {
                        if let Some(idx) = s.data.projects.iter().position(|p| p.name == name) {
                            if s.active_project != idx {
                                s.active_project = idx;
                                s.data.active_project_id =
                                    s.data.projects.get(idx).map(|p| p.id.clone());
                                s.selected_request = None;
                                s.selected_folder = None;
                                s.open_request_ids.clear();
                                s.active_tab_id = None;
                                cx.emit(AppEvent::WorkspaceChanged);
                                cx.emit(AppEvent::SelectionChanged);
                            }
                        }
                    }
                });
            },
        );

        // Re-render when git state changes (busy flag, dirty count, branches)
        // so the title-bar sync button tooltip stays current.
        let sub_git = cx.subscribe(&app.git, |this, _src, _ev: &crate::git::GitEvent, cx| {
            cx.notify();
            let _ = this;
        });

        // The management panel emits import/export requests upward (it can't
        // call VerveApp's file-prompt methods directly).
        let sub_manage = cx.subscribe(
            &app.project_manage,
            |this, _src, ev: &crate::ui::project_manage_panel::ManageEvent, cx| match ev {
                crate::ui::project_manage_panel::ManageEvent::Import => {
                    this.import_collection(cx);
                }
                crate::ui::project_manage_panel::ManageEvent::Export(fmt) => {
                    this.export_project(*fmt, cx);
                }
                crate::ui::project_manage_panel::ManageEvent::GenerateMocks => {
                    this.generate_mocks(cx);
                }
            },
        );

        // The share panel emits NewShare / Open / Copy / Delete requests upward.
        let sub_share = cx.subscribe(&app.share, |this, _src, ev: &ShareEvent, cx| {
            match ev {
                ShareEvent::NewShare => {
                    // Opening the dialog needs a Window; defer to render.
                    this.pending_share = Some((ShareScope::Project, None, None));
                    cx.notify();
                }
                ShareEvent::Open(id) => {
                    let url = this.build_share_url(id);
                    cx.open_url(&url);
                }
                ShareEvent::Copy(id) => {
                    let url = this.build_share_url(id);
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url));
                }
                ShareEvent::Delete(id) => {
                    this.delete_share(id.clone(), cx);
                }
            }
        });

        // The console (history) panel emits OpenRequest when the user clicks a row.
        let console_handle = app.console.clone();
        let sub_console = cx.subscribe(
            &console_handle,
            |this, _src, ev: &ConsoleEvent, cx| match ev {
                ConsoleEvent::OpenRequest {
                    request_id: Some(req_id),
                    project_id,
                } => {
                    // Switch project if the history entry belongs to a different project.
                    let current_pid = this
                        .state
                        .read(cx)
                        .active_project()
                        .map(|p| p.id.clone())
                        .unwrap_or_default();
                    if !project_id.is_empty() && &current_pid != project_id {
                        if let Some(idx) = this
                            .state
                            .read(cx)
                            .data
                            .projects
                            .iter()
                            .position(|p| &p.id == project_id)
                        {
                            this.state.update(cx, |s, cx| {
                                s.active_project = idx;
                                s.selected_request = None;
                                s.selected_folder = None;
                                s.open_request_ids.clear();
                                s.active_tab_id = None;
                                cx.emit(AppEvent::WorkspaceChanged);
                            });
                        }
                    }

                    // Switch to API view, select the request, ask the tree to reveal it.
                    let exists = this
                        .state
                        .read(cx)
                        .active_project()
                        .map(|p| p.find_request(req_id).is_some())
                        .unwrap_or(false);
                    if exists {
                        this.active_view = SideView::Api;
                        let req_id_emit = req_id.clone();
                        this.state.update(cx, |s, cx| {
                            s.open_or_focus_tab(&req_id_emit, cx);
                            cx.emit(AppEvent::LocateActive);
                        });
                        // Collapse the bottom dock so the request/response is visible.
                        this.show_console = false;
                    }
                    cx.notify();
                }
                ConsoleEvent::OpenRequest {
                    request_id: None, ..
                } => {
                    // Legacy entries without a request id: nothing to navigate to.
                }
            },
        );

        // Subscribe to mock console events (GenerateAll).
        let mock_console_handle = app.mock_console.clone();
        let sub_mock = cx.subscribe(
            &mock_console_handle,
            |this, _src, ev: &MockConsoleEvent, cx| match ev {
                MockConsoleEvent::GenerateAll => {
                    this.generate_mocks(cx);
                }
            },
        );

        // Subscribe to tree events (share request).
        let tree_handle = app.tree.clone();
        let sub_tree = cx.subscribe(
            &tree_handle,
            |this, _src, ev: &crate::ui::project_tree_panel::TreeEvent, _cx| match ev {
                crate::ui::project_tree_panel::TreeEvent::ShareRequest(id, name) => {
                    this.pending_share = Some((
                        crate::share::models::ShareScope::Request,
                        Some(id.clone()),
                        Some(name.clone()),
                    ));
                }
            },
        );

        // The formatting toolbar lives in the host title bar (rendered by
        // VerveApp via `render_editor_toolbar`), reading the active editor's
        // live `toolbar_snapshot`. VerveApp only re-renders when it — or an
        // entity it observes — notifies. Observe the notes/markdown *panels*
        // (not the editors): the notes panel rebuilds its editor entity on
        // every note switch (see NotesPanel::reload_editor), so observing the
        // editor would go stale. The panels re-notify on editor changes, which
        // keeps the toolbar's active states (H1-3, B/I/S/U, dropdown marks) in
        // sync with the editor below.
        let obs_notes = cx.observe(&app.notes, |_, _, cx| cx.notify());
        let obs_markdown = cx.observe(&app.markdown, |_, _, cx| cx.notify());

        app._subs = vec![
            sub_edited,
            sub_env,
            sub_project,
            sub_git,
            sub_manage,
            sub_share,
            sub_console,
            sub_mock,
            sub_tree,
            obs_notes,
            obs_markdown,
        ];

        // Silent startup update check: query GitHub for the latest release
        // after a short delay. On success, store the result so the title-bar
        // button can show a red dot — no notification is shown (to avoid
        // bothering the user on every launch).
        app.startup_update_check(cx);

        // Consume any .md files pending open (from argv or macOS "open file"
        // events). Route them to the Markdown editor panel.
        app.open_pending_markdown_files(cx);

        // Keep draining the pending-files queue while the app is running. The
        // initial drain above only covers files present at launch; files opened
        // later (double-click a .md when Verve is already the default app, or
        // `verve note.md` from a terminal against a running instance) land in
        // the queue via main.rs's argv parser / `on_open_urls` and would never
        // be opened without this watcher. The poll is cheap — the drain is a
        // no-op when the queue is empty.
        app.start_pending_file_watcher(cx);

        app
    }

    /// Drains the `PendingMarkdownFiles` global (set in main.rs from argv and
    /// macOS open-file events) and opens each in the Markdown editor.
    pub(super) fn open_pending_markdown_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx
            .try_global::<crate::PendingMarkdownFiles>()
            .and_then(|g| g.0.lock().ok().map(|mut g| std::mem::take(&mut *g)))
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        self.active_view = SideView::MarkdownEditor;
        for path in paths {
            let _ = self.markdown.update(cx, |p, cx| p.open_path(&path, cx));
        }
        cx.notify();
    }
}

impl VerveApp {
    pub(super) fn start_pending_file_watcher(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let Ok(()) = this.update(cx, |app, cx| {
                    app.open_pending_markdown_files(cx);
                }) else {
                    return;
                };
            }
        })
        .detach();
    }

    pub(super) fn startup_update_check(&mut self, cx: &mut Context<Self>) {
        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;
            let result = crate::updater::run_check(client).await;
            let _ = this.update(cx, |this, cx| {
                if let crate::updater::UpdateCheckResult::UpdateAvailable(info) = &result {
                    this.update_info = Some(info.clone());
                    this.update_check_result = Some(result);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn check_updates_manual(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        cx.notify();

        let client = cx.http_client();
        cx.spawn(async move |this, cx| {
            let result = crate::updater::run_check(client).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.update_checking = false;
                this.update_check_result = Some(result.clone());
                match &result {
                    crate::updater::UpdateCheckResult::UpdateAvailable(info) => {
                        this.update_info = Some(info.clone());
                        let msg =
                            format!("发现新版本 v{}！点击标题栏下载按钮前往下载。", info.version);
                        window.push_notification(
                            gpui_component::notification::Notification::new()
                                .title("发现新版本")
                                .message(msg)
                                .autohide(true),
                            cx,
                        );
                    }
                    crate::updater::UpdateCheckResult::UpToDate => {
                        window.push_notification(
                            gpui_component::notification::Notification::new()
                                .title("检查更新")
                                .message("当前已是最新版本。")
                                .autohide(true),
                            cx,
                        );
                    }
                    crate::updater::UpdateCheckResult::Error(e) => {
                        window.push_notification(
                            gpui_component::notification::Notification::new()
                                .title("检查更新失败")
                                .message(e.clone())
                                .autohide(true),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_update_download(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(info) = &self.update_info {
            // Prefer the direct download URL; fall back to the release page.
            let url = info.download_url.as_deref().unwrap_or(&info.release_url);
            cx.open_url(url);
        }
    }
}

/// Reorder rail names so the fixed-shortcut views (`SideView::SHORTCUT_VIEWS`)
/// come first in key order, preserving the relative order of the rest.
fn shortcut_first_rail_order(order: &mut Vec<String>) {
    let shortcut_names: Vec<&str> = SideView::SHORTCUT_VIEWS.iter().map(|v| v.name()).collect();
    let mut front: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for name in order.drain(..) {
        if shortcut_names.contains(&name.as_str()) {
            front.push(name);
        } else {
            rest.push(name);
        }
    }
    front.sort_by_key(|n| {
        shortcut_names
            .iter()
            .position(|s| *s == n.as_str())
            .unwrap_or(usize::MAX)
    });
    order.extend(front);
    order.extend(rest);
}

#[cfg(test)]
mod tests {
    // No `use super::*` here: it would glob in gpui's `test` attribute macro
    // (the parent does `use gpui::*`) and shadow the builtin `#[test]`.
    use super::shortcut_first_rail_order;

    #[test]
    fn shortcut_views_move_to_front_in_key_order() {
        let mut order: Vec<String> = ["Share", "Ssh", "Mock", "Notes", "Api", "JsonFormat"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        shortcut_first_rail_order(&mut order);
        let expected: Vec<String> = ["Api", "Ssh", "JsonFormat", "Notes", "Share", "Mock"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(order, expected);
    }
}
