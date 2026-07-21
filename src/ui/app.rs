//! Workspace shell: title bar + three-pane body, wired together.

use gpui::prelude::FluentBuilder as _;
use gpui::{img, *};

// Actions for keybindings (PRD §6).
actions!(verve, [SaveWorkspace, NewRequest]);
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    v_flex, ActiveTheme, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    WindowExt as _,
};

use crate::assets::{BRACES, DOCS, DOCKER as DOCKER_ICON, EXPORT, HISTORY, K8S as K8S_ICON, KANBAN, IMPORT, REFRESH_CW, SHARE};

/// Build an icon from a Verve-custom Lucide SVG path.
fn vicon(path: &'static str) -> Icon {
    Icon::from(IconName::Redo).path(path)
}

use crate::git::GitState;
use crate::share::models::{ShareConfig, ShareScope};
use crate::share::server::{self, ShareServer};
use crate::state::{AppEvent, AppState};
use crate::ui::console_panel::{ConsoleEvent, ConsolePanel};
use crate::ui::project_manage_panel::ProjectManagePanel;
use crate::ui::project_tree_panel::ProjectTreePanel;
use crate::ui::request_panel::RequestPanel;
use crate::ui::response_panel::ResponsePanel;
use crate::ui::share_dialog;
use crate::ui::share_panel::{ShareEvent, SharePanel};
use crate::ui::autotest_panel::AutoTestPanel;
use crate::ui::docker_panel::DockerPanel;
use crate::ui::k8s_panel::K8sPanel;
use crate::ui::proxy_panel::ProxyPanel;
use crate::ui::ssh_panel::SshPanel;
use crate::ui::stress_panel::StressPanel;

pub struct VerveApp {
    pub state: Entity<AppState>,
    pub tree: Entity<ProjectTreePanel>,
    pub request: Entity<RequestPanel>,
    pub response: Entity<ResponsePanel>,
    pub console: Entity<ConsolePanel>,
    /// Git version-control state (gix/gitoxide-backed).
    pub git: Entity<GitState>,
    /// Project-management sidebar panel (Git status / history / branches).
    pub project_manage: Entity<ProjectManagePanel>,
    /// Document-sharing management panel (文档管理).
    pub share: Entity<SharePanel>,
    /// SSH management panel (主机卡片 + 终端).
    pub ssh: Entity<SshPanel>,
    /// Automated testing panel (套件/用例/步骤 + 运行结果).
    pub autotest: Entity<AutoTestPanel>,
    /// Stress testing panel (oha/runner + live results).
    pub stress: Entity<StressPanel>,
    /// HTTP proxy / traffic capture panel.
    pub proxy: Entity<ProxyPanel>,
    /// Kubernetes cluster panel.
    pub k8s: Entity<K8sPanel>,
    /// Docker container panel.
    pub docker: Entity<DockerPanel>,
    /// Live share HTTP server handle (started when any share config exists).
    pub share_server: Option<ShareServer>,
    /// Shared config store backing the share server (hot-swappable).
    pub share_configs: std::sync::Arc<std::sync::RwLock<Vec<ShareConfig>>>,
    /// Display host for share URLs (from RemoteConfig; falls back to 127.0.0.1).
    pub share_host: String,
    /// Port the share server is listening on.
    pub share_port: u16,
    /// Active workspace id (used as tenant namespace when multi-tenant is enabled).
    pub active_workspace_id_cache: Option<String>,
    pub env_select: Entity<SelectState<Vec<String>>>,
    /// Project switcher (one entry per project in the workspace).
    pub project_select: Entity<SelectState<Vec<String>>>,
    pub show_console: bool,
    /// Which sidebar view is active (left activity rail selection).
    pub active_view: SideView,
    /// The view the brand mark "V" (首页) points to. The corresponding rail
    /// button is hidden to avoid a duplicate entry. Defaults to Api.
    pub home_view: SideView,
    /// Set when the project/environment lists changed and the switchers need
    /// their items rebuilt; reconciled in render where a Window is available.
    pub pending_switcher_refresh: bool,
    /// Whether the left sidebar (tree) is collapsed.
    pub sidebar_collapsed: bool,
    /// Whether the far-left activity rail is collapsed.
    pub rail_collapsed: bool,
    /// Which activity-rail items are currently hidden (SideView names).
    pub hidden_rails: std::collections::HashSet<String>,
    /// Whether the theme picker popover is open.
    pub theme_popover_open: bool,
    /// Whether the language switcher popover is open.
    pub lang_popover_open: bool,
    /// Whether the export format picker popover is open.
    pub export_popover_open: bool,
    /// Whether the project (workspace) selector popover is open.
    pub project_popover_open: bool,
    /// Whether the workspace switcher popover is open.
    pub workspace_popover_open: bool,
    /// Whether the environment selector popover is open.
    pub env_popover_open: bool,
    /// Pending new-project name dialog (set when "+ 新建项目" is clicked).
    pub pending_new_project: bool,
    /// Pending new-workspace creation (set when the workspace dialog confirms).
    pub pending_new_workspace: bool,
    /// The input entity for the pending new-workspace dialog (name field).
    pub pending_workspace_name_input: Option<Entity<gpui_component::input::InputState>>,
    /// Pending new-environment name dialog (set when "+ 新建环境" is clicked).
    pub pending_new_env: bool,
    /// Pending management dialog to open (set from popover click handlers
    /// which lack a Window; reconciled in render).
    pub pending_dialog: Option<PendingDialog>,
    /// Pending share dialog to open (set from the share panel's NewShare
    /// event, which lacks a Window; reconciled in render). Carries the
    /// (scope, target_id, target_name) triple.
    pub pending_share: Option<(ShareScope, Option<String>, Option<String>)>,
    /// Currently-applied git auto-sync interval (minutes); compared against
    /// persisted config on each render so settings changes apply live.
    pub applied_sync_interval: u32,
    /// Resizable state for the main 3-pane horizontal group.
    pub main_resize: Entity<ResizableState>,
    /// Resizable state for the center (request/console) vertical group.
    pub center_resize: Entity<ResizableState>,
    /// Saved panel sizes loaded at startup; applied as initial `.size()`.
    pub saved_layout: Option<crate::state::persistence::PanelLayout>,
    /// Latest available update info (Some when a newer version exists).
    pub update_info: Option<crate::updater::UpdateInfo>,
    /// Whether an update check is in progress (drives the title-bar button spinner).
    pub update_checking: bool,
    /// The result of the last *manual* update check (shown in the settings page).
    pub update_check_result: Option<crate::updater::UpdateCheckResult>,
    _subs: Vec<gpui::Subscription>,
}

/// A management dialog queued for opening in render (where a Window exists).
#[derive(Clone, PartialEq, Eq)]
pub enum PendingDialog {
    Environments,
    /// The full settings window (通用设置 + 环境管理), opening on the General
    /// section so the user can configure the home view.
    Settings,
    GlobalCookies,
    GlobalParams,
    GlobalHeaders,
    GlobalVariables,
    /// Project settings (rename) — carries the project index.
    ProjectSettings(usize),
    /// Delete the project at the given index (after confirm).
    DeleteProject(usize),
    /// Delete the environment with the given id (after confirm).
    DeleteEnv(String),
}

/// The selectable views in the far-left activity rail.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SideView {
    /// API tree (the default workbench view).
    Api,
    /// Request/response history.
    History,
    /// Environments manager.
    Environments,
    /// Mock service status.
    Mock,
    /// Git-based project management.
    ProjectManage,
    /// Document-sharing management (文档管理).
    Share,
    /// Automated testing (自动化测试).
    AutoTest,
    /// Stress testing (压测).
    Stress,
    /// HTTP proxy / traffic capture.
    Proxy,
    /// Kubernetes cluster management (K8s).
    K8s,
    /// Docker container management.
    Docker,
    /// SSH connection management (SSH 管理).
    Ssh,
}

impl SideView {
    /// All rail-selectable views, in rail order.
    pub const ALL: &'static [SideView] = &[
        SideView::Api,
        SideView::AutoTest,
        SideView::Stress,
        SideView::Proxy,
        SideView::Share,
        SideView::ProjectManage,
        SideView::K8s,
        SideView::Docker,
        SideView::Environments,
        SideView::Mock,
        SideView::History,
        SideView::Ssh,
    ];

    /// A stable string key for persistence.
    pub fn name(self) -> &'static str {
        match self {
            SideView::Api => "Api",
            SideView::History => "History",
            SideView::Environments => "Environments",
            SideView::Mock => "Mock",
            SideView::ProjectManage => "ProjectManage",
            SideView::Share => "Share",
            SideView::AutoTest => "AutoTest",
            SideView::Stress => "Stress",
            SideView::Proxy => "Proxy",
            SideView::K8s => "K8s",
            SideView::Docker => "Docker",
            SideView::Ssh => "Ssh",
        }
    }

    /// Parse a string key back into a view (falls back to Api).
    pub fn parse(s: &str) -> SideView {
        match s {
            "History" => SideView::History,
            "Environments" => SideView::Environments,
            "Mock" => SideView::Mock,
            "ProjectManage" => SideView::ProjectManage,
            "Share" => SideView::Share,
            "AutoTest" => SideView::AutoTest,
            "Stress" => SideView::Stress,
            "Proxy" => SideView::Proxy,
            "K8s" => SideView::K8s,
            "Docker" => SideView::Docker,
            "Ssh" => SideView::Ssh,
            _ => SideView::Api,
        }
    }

    /// Human-readable label, i18n-aware (matches the rail tooltip).
    pub fn label(self) -> String {
        match self {
            SideView::Api => rust_i18n::t!("view.api").to_string(),
            SideView::History => rust_i18n::t!("view.history").to_string(),
            SideView::Environments => rust_i18n::t!("view.environments").to_string(),
            SideView::Mock => rust_i18n::t!("view.mock").to_string(),
            SideView::ProjectManage => rust_i18n::t!("view.project_manage").to_string(),
            SideView::Share => rust_i18n::t!("view.share").to_string(),
            SideView::AutoTest => rust_i18n::t!("view.autotest").to_string(),
            SideView::Stress => "压测".to_string(),
            SideView::Proxy => "抓包".to_string(),
            SideView::K8s => "K8s".to_string(),
            SideView::Docker => "Docker".to_string(),
            SideView::Ssh => rust_i18n::t!("view.ssh").to_string(),
        }
    }
}

/// Make a project name safe for use as a filename.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

impl VerveApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let tree = cx.new(|cx| ProjectTreePanel::new(state.clone(), window, cx));
        let request = cx.new(|cx| RequestPanel::new(state.clone(), window, cx));
        let response = cx.new(|cx| ResponsePanel::new(state.clone(), window, cx));
        let console = cx.new(|cx| ConsolePanel::new(state.clone(), cx));

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
            let sync_minutes = cfg.sync_interval_minutes.unwrap_or(crate::state::persistence::DEFAULT_SYNC_INTERVAL_MINUTES);
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
                // Auto-sync on the configured interval: commit dirty changes +
                // push/pull so work is versioned without manual intervention.
                g.start_auto_sync(cx, std::time::Duration::from_secs((sync_minutes as u64) * 60));
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
        let tenant_id = remote_cfg.multi_tenant.then(|| active_ws_id.clone().unwrap_or_default());
        let share_configs = server::config_store(crate::share::persist::load_shares());
        let share_server = if remote_cfg.enabled || share_configs.read().map(|c| !c.is_empty()).unwrap_or(false) {
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
            ))
        } else {
            None
        };

        let mut app = Self {
            state: state.clone(),
            tree,
            request,
            response,
            console,
            git,
            project_manage,
            share,
            ssh,
            autotest,
            stress,
            proxy,
            k8s,
            docker,
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
                                s.data.active_project_id = s.data.projects.get(idx).map(|p| p.id.clone());
                                s.selected_request = None;
                                s.selected_folder = None;
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
                    this.export_project(fmt.clone(), cx);
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
                        this.state.update(cx, |s, cx| {
                            s.selected_request = Some(req_id.clone());
                            s.selected_folder = None;
                            cx.emit(AppEvent::SelectionChanged);
                            cx.emit(AppEvent::LocateActive);
                        });
                        // Collapse the bottom dock so the request/response is visible.
                        this.show_console = false;
                    }
                    cx.notify();
                }
                ConsoleEvent::OpenRequest { request_id: None, .. } => {
                    // Legacy entries without a request id: nothing to navigate to.
                }
            },
        );

        app._subs = vec![
            sub_edited,
            sub_env,
            sub_project,
            sub_git,
            sub_manage,
            sub_share,
            sub_console,
        ];

        // Silent startup update check: query GitHub for the latest release
        // after a short delay. On success, store the result so the title-bar
        // button can show a red dot — no notification is shown (to avoid
        // bothering the user on every launch).
        app.startup_update_check(cx);

        app
    }
}

impl VerveApp {
    /// Silent startup update check: fire after a 3s delay so the UI has settled.
    /// Only stores the result for the title-bar indicator; does not notify.
    fn startup_update_check(&mut self, cx: &mut Context<Self>) {
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

    /// Manual update check (triggered by the title-bar button or settings page).
    /// Shows a notification with the result.
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

    /// Open the latest release page (or direct download URL) in the browser.
    pub fn open_update_download(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(info) = &self.update_info {
            // Prefer the direct download URL; fall back to the release page.
            let url = info.download_url.as_deref().unwrap_or(&info.release_url);
            cx.open_url(url);
        }
    }

    /// Open a generic KV-manager dialog for the given scope.
    fn open_kv_manager(
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

    /// Open a small "new project" prompt dialog.
    fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                                    s.notify_workspace(cx);
                                });
                            }
                            window.close_dialog(cx);
                        })
                })
        });
    }

    // -----------------------------------------------------------------
    // Workspace management: switch / create / delete.
    // Each workspace maps to a git branch (verve/<id>); switching commits the
    // current workspace, checks out the target branch (rewriting workspace.json),
    // then reloads. Git is optional — when not initialized, switching just
    // swaps the active marker.
    // -----------------------------------------------------------------

    /// Switch to a different workspace by id.
    fn switch_workspace(&mut self, id: String, cx: &mut Context<Self>) {
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

    /// Create a new workspace with the given name.
    fn create_workspace(&mut self, name: String, cx: &mut Context<Self>) {
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

    /// Delete a workspace by id. The default workspace cannot be deleted.
    fn delete_workspace(&mut self, id: String, cx: &mut Context<Self>) {
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

    /// Open the "new workspace" name dialog.
    fn open_new_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Open a small "new environment" prompt dialog.
    fn open_new_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Open a project-settings (rename) dialog for the project at `idx`.
    fn open_project_settings(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Open a confirmation dialog to delete the project at `idx`.
    fn confirm_delete_project(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
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
                                    s.notify_workspace(cx);
                                }
                            });
                            window.close_dialog(cx);
                        })
                })
        });
    }

    /// Open a confirmation dialog to delete the environment `id`.
    fn confirm_delete_env(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Generate default mock rules for every request that doesn't have one yet,
    /// persist the change, and hot-swap the running mock server.
    fn generate_mocks(&mut self, cx: &mut Context<Self>) {
        let app = self.state.read(cx);
        let idx = app.active_project;
        let mut s = app.data.clone();
        drop(app);
        let Some(p) = s.projects.get_mut(idx) else { return };
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
        fn walk(folders: &mut [crate::state::models::Folder], map: &mut std::collections::HashMap<String, crate::state::models::MockRule>) {
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
    }

    /// Export the active project to a chosen file (Markdown or JSON).
    fn export_project(&mut self, format: crate::export::Format, cx: &mut Context<Self>) {
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
            crate::export::Format::PostmanV2_1 => match crate::export::project_to_postman_v2_1(&project) {
                Ok(s) => s,
                Err(_) => return,
            },
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

    /// Open the share-configuration dialog for the given scope/target. The
    /// `on_confirm` handler persists the config, (re)starts the share server,
    /// refreshes the panel, and opens the link in the browser.
    fn open_share_dialog(
        &mut self,
        scope: ShareScope,
        target_id: Option<String>,
        target_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let configs_store = self.share_configs.clone();
        let share_host = self.share_host.clone();
        let share_port = self.share_port;
        let active_ws = self.active_workspace_id_cache.clone();
        let remote_cfg = crate::state::persistence::load_remote_config();
        let is_cloud = crate::share::remote::is_cloud_mode(&remote_cfg);
        // The confirm handler needs `this` to start/refresh the server + panel,
        // but `this` isn't movable into `open_dialog`. We close over the pieces
        // we need (the config store) and do the panel refresh via cx.notify.
        share_dialog::open_dialog(
            state.clone(),
            scope,
            target_id,
            target_name,
            move |cfg, window, cx: &mut App| {
                // Apply tenant namespace when multi-tenant mode is enabled.
                let cfg = if remote_cfg.multi_tenant {
                    if let Some(ws) = active_ws.as_deref() {
                        cfg.with_tenant(ws)
                    } else {
                        cfg
                    }
                } else {
                    cfg
                };
                log::info!("创建分享: id={} title={}", cfg.id, cfg.display_title());

                if is_cloud {
                    // ---- Cloud mode: push to remote server over HTTP ----
                    // Resolve the project to upload.
                    let project_id_str = cfg.project_id.clone();
                    let project = state.read(cx)
                        .data
                        .projects
                        .iter()
                        .find(|p| p.id == project_id_str)
                        .cloned();
                    let Some(project) = project else {
                        let not_found_id = project_id_str.clone();
                        window.open_dialog(cx, move |dialog, _window, _cx| {
                            let msg = format!("找不到项目：{}", not_found_id);
                            dialog.title("分享失败").content(move |c, _, _| {
                                c.child(div().p_4().text_sm().child(msg.clone()))
                            })
                        });
                        return;
                    };
                    let cfg_for_push = cfg.clone();
                    let cfg_for_err = cfg.clone();
                    let client = cx.http_client();
                    let remote_cfg_clone = remote_cfg.clone();
                    let window_handle = window.window_handle();
                    cx.spawn(async move |cx| {
                        let cfg_for_result = cfg_for_push.clone();
                        let result = crate::share::remote::push_share(
                            client,
                            &remote_cfg_clone,
                            &project,
                            &cfg_for_push,
                        ).await;
                        let _ = window_handle.update(cx, |_this, window, cx| {
                            match result {
                                Ok(remote_id) => {
                                    let url = crate::share::remote::share_url(&remote_cfg_clone, &remote_id)
                                        .unwrap_or_default();
                                    show_share_result_dialog(window, cx, cfg_for_result, url);
                                }
                                Err(e) => {
                                    let err_msg = format!("推送到远程服务失败：{e}");
                                    log::error!("{}", err_msg);
                                    window.open_dialog(cx, move |dialog, _window, cx| {
                                        let msg = err_msg.clone();
                                        dialog.title("分享失败").content(move |c, _, _| {
                                            c.child(div().p_4().w(px(400.)).text_sm().child(msg.clone()))
                                        })
                                        .footer(
                                            Button::new("share-error-close")
                                                .primary().small().label("关闭")
                                                .on_click(|_, w, cx| w.close_dialog(cx)),
                                        )
                                    });
                                }
                            }
                        });
                    }).detach();
                    return;
                }

                // ---- Local mode: persist + serve from embedded server ----
                let all = crate::share::persist::upsert_share(cfg.clone());
                if let Ok(mut guard) = configs_store.write() {
                    *guard = all;
                }

                let url = format!("http://{share_host}:{share_port}/s/{}", cfg.id);
                show_share_result_dialog(window, cx, cfg, url);
            },
            window,
            cx,
        );
    }

    /// Build the public URL for a share, using the remote server address when
    /// cloud mode is active, or the local bind host/port otherwise.
    fn build_share_url(&self, id: &str) -> String {
        let remote_cfg = crate::state::persistence::load_remote_config();
        if let Some(cloud_url) = crate::share::remote::share_url(&remote_cfg, id) {
            return cloud_url;
        }
        format!("http://{}:{}/s/{}", self.share_host, self.share_port, id)
    }

    /// Delete a share config (after the panel's confirm) and refresh the panel.
    /// The server keeps running — the shared config store is updated so the
    /// deleted share immediately 404s, and restarting the TCP listener
    /// mid-session risks dropping in-flight requests.
    fn delete_share(&mut self, id: String, cx: &mut Context<Self>) {
        let all = crate::share::persist::remove_share(&id);
        log::info!("删除分享 {id}，剩余 {} 个", all.len());
        if let Ok(mut guard) = self.share_configs.write() {
            *guard = all;
        }
        // Refresh the share panel so the row disappears.
        self.share.update(cx, |s, cx| s.reload(cx));
        cx.notify();
    }

    /// Prompt for a collection JSON file and import it as a new project.
    fn import_collection(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import collection (Apipost / Postman v2.1 / Swagger / OpenAPI 3 JSON)".into()),
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

impl Render for VerveApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pick up home-view changes made in the settings window (which writes
        // layout.json directly). Re-reading on every render is cheap (one small
        // JSON file) and ensures the rail reflects the latest choice instantly.
        if let Some(layout) = crate::state::persistence::load_layout() {
            if let Some(name) = layout.home_view.as_deref() {
                let parsed = SideView::parse(name);
                if parsed != self.home_view {
                    self.home_view = parsed;
                }
            }
            // Also pick up hidden-rail changes from settings.
            let new_hidden = crate::state::persistence::load_hidden_rails();
            if new_hidden != self.hidden_rails {
                self.hidden_rails = new_hidden;
            }
            // Pick up sync-interval changes from settings and restart the timer
            // live (no app restart needed).
            let new_interval = crate::state::persistence::load_sync_interval_minutes();
            if new_interval != self.applied_sync_interval {
                self.applied_sync_interval = new_interval;
                let dur = std::time::Duration::from_secs((new_interval as u64) * 60);
                let git = self.git.clone();
                cx.spawn(async move |_, cx| {
                    let _ = git.update(cx, |g, cx| g.start_auto_sync(cx, dur));
                })
                .detach();
            }
        }
        // Safety: if the active view was just hidden, fall back to home.
        if self.active_view != self.home_view && self.hidden_rails.contains(self.active_view.name()) {
            self.active_view = self.home_view;
        }
        // Lazily start the share server when the first share config appears
        // (created via the share dialog). The configs store is shared mutably,
        // so a running server picks up new/updated configs automatically — this
        // only starts the TCP listener once.
        if self.share_server.is_none() {
            let has_shares = self
                .share_configs
                .read()
                .map(|g| !g.is_empty())
                .unwrap_or(false);
            if has_shares {
                let configs = self.share_configs.clone();
                let remote_cfg = crate::state::persistence::load_remote_config();
                let tenant_id = remote_cfg.multi_tenant.then(|| {
                    self.active_workspace_id_cache.clone().unwrap_or_default()
                });
                self.share_server = Some(server::start_desktop_with_config(
                    remote_cfg.host.clone(),
                    remote_cfg.port,
                    remote_cfg.auth_token.clone(),
                    tenant_id,
                    configs,
                    |project_id| {
                        let data = crate::state::persistence::load_or_default();
                        data.projects.iter().find(|p| p.id == project_id).cloned()
                    },
                ));
                log::info!("share server started lazily on {}:{}", remote_cfg.host, remote_cfg.port);
            }
        }
        if self.pending_switcher_refresh {
            self.pending_switcher_refresh = false;
            self.refresh_switchers(_window, cx);
        }
        // Reconcile pending dialog opens (set from popover click handlers
        // which may lack a Window at click time).
        if self.pending_new_project {
            self.pending_new_project = false;
            self.open_new_project(_window, cx);
        }
        // Reconcile pending workspace creation: read the name from the stored
        // input entity and call create_workspace.
        if self.pending_new_workspace {
            self.pending_new_workspace = false;
            let name = self
                .pending_workspace_name_input
                .as_ref()
                .and_then(|input| input.read(cx).value().to_string().into())
                .filter(|s: &String| !s.trim().is_empty());
            if let Some(name) = name {
                self.create_workspace(name, cx);
            }
            self.pending_workspace_name_input = None;
        }
        if self.pending_new_env {
            self.pending_new_env = false;
            self.open_new_env(_window, cx);
        }
        if let Some(dialog) = self.pending_dialog.take() {
            match dialog {
                // The environments manager opens its own OS window; defer it
                // out of render (open_window mutates app state, which must not
                // happen during the read-only render phase).
                PendingDialog::Environments => {
                    let state = self.state.clone();
                    cx.defer(move |cx| {
                        let bounds =
                            gpui::Bounds::centered(None, gpui::size(px(960.), px(620.)), cx);
                        let _ = cx.open_window(
                            gpui::WindowOptions {
                                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                                window_min_size: Some(gpui::size(px(820.), px(520.))),
                                titlebar: None,
                                is_minimizable: false,
                                ..Default::default()
                            },
                            |window, cx| {
                                let view = cx.new(|cx| {
                                    crate::ui::settings_window::SettingsWindow::new(
                                        state.clone(),
                                        crate::ui::settings_window::SettingsKind::Environments,
                                        window,
                                        cx,
                                    )
                                });
                                cx.new(|cx| {
                                    gpui_component::Root::new(view, window, cx)
                                        .bg(cx.theme().background)
                                })
                            },
                        );
                    });
                }
                // The full settings window opening on the General section
                // (home-view selector + other app preferences).
                PendingDialog::Settings => {
                    let state = self.state.clone();
                    cx.defer(move |cx| {
                        let bounds =
                            gpui::Bounds::centered(None, gpui::size(px(720.), px(560.)), cx);
                        let _ = cx.open_window(
                            gpui::WindowOptions {
                                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                                window_min_size: Some(gpui::size(px(600.), px(440.))),
                                titlebar: None,
                                is_minimizable: false,
                                ..Default::default()
                            },
                            |window, cx| {
                                let view = cx.new(|cx| {
                                    crate::ui::settings_window::SettingsWindow::new(
                                        state.clone(),
                                        crate::ui::settings_window::SettingsKind::General,
                                        window,
                                        cx,
                                    )
                                });
                                cx.new(|cx| {
                                    gpui_component::Root::new(view, window, cx)
                                        .bg(cx.theme().background)
                                })
                            },
                        );
                    });
                }
                PendingDialog::GlobalCookies => self.open_kv_manager(
                    crate::ui::kv_manager_view::KvScope::GlobalCookies,
                    _window,
                    cx,
                ),
                PendingDialog::GlobalParams => self.open_kv_manager(
                    crate::ui::kv_manager_view::KvScope::GlobalParams,
                    _window,
                    cx,
                ),
                PendingDialog::GlobalHeaders => self.open_kv_manager(
                    crate::ui::kv_manager_view::KvScope::GlobalHeaders,
                    _window,
                    cx,
                ),
                PendingDialog::GlobalVariables => self.open_kv_manager(
                    crate::ui::kv_manager_view::KvScope::GlobalVariables,
                    _window,
                    cx,
                ),
                PendingDialog::ProjectSettings(idx) => self.open_project_settings(idx, _window, cx),
                PendingDialog::DeleteProject(idx) => self.confirm_delete_project(idx, _window, cx),
                PendingDialog::DeleteEnv(id) => self.confirm_delete_env(id, _window, cx),
            }
        }
        // Reconcile pending share-dialog opens (set from the share panel's
        // NewShare event, which lacks a Window).
        if let Some((scope, target_id, target_name)) = self.pending_share.take() {
            self.open_share_dialog(scope, target_id, target_name, _window, cx);
        }
        // Fetch the overlay layers BEFORE building the rest of the tree. The
        // gpui-component Root does not auto-render these in its own render();
        // the consumer must attach them so dialogs/sheets/notifications paint.
        let sheet_layer = gpui_component::Root::render_sheet_layer(_window, cx);
        let dialog_layer = gpui_component::Root::render_dialog_layer(_window, cx);
        let notification_layer = gpui_component::Root::render_notification_layer(_window, cx);

        let theme = cx.theme().clone();
        let show_console = self.show_console;
        let tree = self.tree.clone();
        let request = self.request.clone();
        let response = self.response.clone();
        let console = self.console.clone();
        let project_manage = self.project_manage.clone();
        let share = self.share.clone();
        let ssh = self.ssh.clone();

        // Restore saved sizes (if any) for the initial render. The center column
        // sizes are clamped to a sane band so a stale persisted layout (from a
        // differently-sized window) can't squash the request/response panels.
        let saved_tree = self
            .saved_layout
            .as_ref()
            .and_then(|l| l.main)
            .map(|m| m[0].clamp(180., 440.))
            .map(px)
            .unwrap_or(px(260.));
        let (saved_resp, saved_console) = self
            .saved_layout
            .as_ref()
            .and_then(|l| l.center)
            .map(|c| {
                let resp = c.get(1).copied().unwrap_or(360.).clamp(240., 760.);
                let con = c.get(2).copied().unwrap_or(200.).clamp(100., 400.);
                (px(resp), px(con))
            })
            .unwrap_or((px(420.), px(200.)));

        let main_state = self.main_resize.clone();
        let center_state = self.center_resize.clone();

        // The left panel switches with the active side-view: the API tree by
        // default, or the history list for the History view. ProjectManage is
        // NOT handled here — it takes over the whole body (see below).
        let side_left: gpui::AnyElement = match self.active_view {
            SideView::Api | SideView::Environments | SideView::Mock => tree.into_any_element(),
            SideView::History => console.clone().into_any_element(),
            SideView::ProjectManage | SideView::Share | SideView::AutoTest | SideView::Stress | SideView::Proxy | SideView::K8s | SideView::Docker | SideView::Ssh => {
                tree.into_any_element() // unused (exclusive branch)
            }
        };

        // When a folder is selected, the center column is taken over by the
        // folder detail view (rendered inside RequestPanel) — no response or
        // console panes are shown, matching postman's exclusive folder view.
        let folder_exclusive = self.state.read(cx).selected_folder.is_some()
            && self.state.read(cx).selected_request.is_none();

        // Center column: request (top) and response (bottom) in a vertical
        // resizable split — the postman-style request/response stack. The
        // console docks below when toggled on. A selected folder collapses
        // this to the request panel alone.
        let center = v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .when(folder_exclusive, |col| {
                // Folder view: request panel fills the whole center column.
                col.child(
                    div()
                        .size_full()
                        .min_h_0()
                        .overflow_hidden()
                        .child(request.clone()),
                )
            })
            .when(!folder_exclusive, |col| {
                col.child(
                    v_resizable("verve-center")
                        .with_state(&center_state)
                        .on_resize(|state, _, cx| persist_center(state, cx))
                        .child(resizable_panel().child(request.clone()).overflow_hidden())
                        .child(
                            resizable_panel()
                                .size(saved_resp)
                                .size_range(px(120.)..px(1200.))
                                .child(response.clone())
                                .overflow_hidden(),
                        )
                        .when(show_console, |group| {
                            group.child(
                                resizable_panel()
                                    .size(saved_console)
                                    .size_range(px(80.)..px(500.))
                                    .child(console.clone())
                                    .overflow_hidden(),
                            )
                        }),
                )
            })
            .into_any_element();

        let sidebar_collapsed = self.sidebar_collapsed;
        let rail_collapsed = self.rail_collapsed;

        // ProjectManage is a full-bleed settings view: it replaces the entire
        // workbench (tree + request/response) with its own two-column layout.
        // This matches postman, where the project-management activity rail item
        // opens an exclusive management surface, NOT a panel beside the API tree.
        // ProjectManage and Share are both full-body exclusive views: they
        // replace the entire workbench (rail + tree + request/response).
        let _exclusive_view = matches!(
            self.active_view,
            SideView::ProjectManage | SideView::Share | SideView::AutoTest | SideView::Stress | SideView::Proxy | SideView::K8s | SideView::Docker | SideView::Ssh
        );

        // Exclusive views: rail + title bar visible, body taken over by the panel.
        let exclusive_view = matches!(
            self.active_view,
            SideView::ProjectManage | SideView::Share | SideView::AutoTest | SideView::Stress | SideView::Proxy | SideView::K8s | SideView::Docker | SideView::Ssh
        );

        v_flex()
            .size_full()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .when(!rail_collapsed, |body| {
                        body.child(self.render_activity_rail(cx))
                    })
                    // Exclusive management view takes over the whole body.
                    .when(exclusive_view, |body| {
                        // Pick which exclusive panel to render.
                        let theme_clone = theme.clone();
                        let panel: gpui::AnyElement = match self.active_view {
                            SideView::ProjectManage => project_manage.clone().into_any_element(),
                            SideView::Share => share.clone().into_any_element(),
                            SideView::Ssh => ssh.clone().into_any_element(),
                            SideView::AutoTest => self.autotest.clone().into_any_element(),
                            SideView::Stress => self.stress.clone().into_any_element(),
                            SideView::Proxy => self.proxy.clone().into_any_element(),
                            SideView::K8s => self.k8s.clone().into_any_element(),
                            SideView::Docker => self.docker.clone().into_any_element(),
                            _ => div().into_any_element(),
                        };
                        body.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .min_h_0()
                                .overflow_hidden()
                                .child(panel),
                        )
                    })
                    .when(!exclusive_view, |body| {
                        body.child(
                            // The resizable group fills the remaining width (after
                            // the rail) and the full body height. Wrapped so it can
                            // shrink horizontally and not overflow into the rail.
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .min_h_0()
                                .overflow_hidden()
                                .child(
                                    h_resizable("verve-main")
                                        .with_state(&main_state)
                                        .on_resize(|state, _, cx| persist_main(state, cx))
                                        .when(!sidebar_collapsed, |group| {
                                            group.child(
                                                resizable_panel()
                                                    .size(saved_tree)
                                                    .size_range(px(180.)..px(440.))
                                                    .child(side_left),
                                            )
                                        })
                                        .child(resizable_panel().child(center).overflow_hidden()),
                                ),
                        )
                    }),
            )
            // Overlay layers: dialogs, sheets, notifications. MUST be attached
            // (the gpui-component Root doesn't auto-render them). Wrapped in a
            // relative container so the deferred/anchored overlays position
            // correctly over the whole window.
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

/// Persist the main horizontal group sizes ([tree]).
fn persist_main(state: &Entity<ResizableState>, cx: &mut gpui::App) {
    let sizes = state.read(cx).sizes().clone();
    if !sizes.is_empty() {
        let mut layout = crate::state::persistence::load_layout().unwrap_or_default();
        // Keep the array shape ([f32; 2]) for backward compat; only [0] (tree) is used.
        layout.main = Some([sizes[0].as_f32(), 0.0]);
        let _ = crate::state::persistence::save_layout(&layout);
    }
}

/// Persist the center vertical group sizes ([request, response, console?]).
fn persist_center(state: &Entity<ResizableState>, cx: &mut gpui::App) {
    let sizes = state.read(cx).sizes().clone();
    if sizes.len() >= 2 {
        let mut layout = crate::state::persistence::load_layout().unwrap_or_default();
        let req = sizes[0].as_f32();
        let resp = sizes[1].as_f32();
        let con = sizes.get(2).map(|p| p.as_f32()).unwrap_or(200.0);
        layout.center = Some([req, resp, con]);
        let _ = crate::state::persistence::save_layout(&layout);
    }
}

impl VerveApp {
    /// Rebuild the project/environment switcher option lists from state.
    fn refresh_switchers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let st = self.state.read(cx);
        let project_names: Vec<String> = st.data.projects.iter().map(|p| p.name.clone()).collect();
        let active_idx = st.active_project;
        let env_names: Vec<String> = match st.active_project() {
            Some(p) => {
                let mut v = vec!["No environment".to_string()];
                v.extend(p.environments.iter().map(|e| e.name.clone()));
                v
            }
            None => vec!["No environment".to_string()],
        };
        self.project_select.update(cx, |s, cx| {
            s.set_items(project_names, window, cx);
            s.set_selected_index(Some(gpui_component::IndexPath::new(active_idx)), window, cx);
        });
        self.env_select.update(cx, |s, cx| {
            s.set_items(env_names, window, cx);
        });
    }

    /// The far-left vertical activity rail (postman-style icon sidebar).
    fn render_activity_rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active_view;
        let hidden = &self.hidden_rails;

        // Helper: is a given SideView visible (not the home view, not hidden)?
        let is_visible = |v: SideView| v != self.home_view && !hidden.contains(v.name());

        // A rail button: icon that highlights when its view is active.
        // The tooltip is derived from the view's i18n label.
        let rail_btn = |id: &'static str,
                        icon: Icon,
                        view: SideView,
                        active_view: SideView,
                        theme: &gpui_component::Theme,
                        cx: &mut Context<Self>| {
            let is_active = active_view == view;
            let tooltip = view.label();
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .selected(is_active)
                .tooltip(tooltip)
                .when(is_active, |btn| {
                    btn.bg(theme.accent.opacity(0.5))
                        .text_color(theme.foreground)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_view = view;
                    cx.notify();
                }))
        };

        v_flex()
            .w(px(44.))
            .flex_shrink_0()
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
            // No right border — the adjacent tree panel has its own border, and
            // a border here would create a visible seam/gap between the rail
            // and the resizable group.
            .bg(theme.muted)
            // Brand mark "V" — click to return to the home/API workbench.
            // Hover lightens the background so the highlight is always visible.
            .child({
                let p = theme.primary;
                let hover_bg = gpui::hsla(p.h, p.s, (p.l + 0.12).min(1.0), p.a);
                div()
                    .id("brand-mark")
                    .size_7()
                    .mb_2()
                    .rounded(theme.radius)
                    .bg(theme.primary)
                    .text_color(theme.primary_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover_bg))
                    .child(div().font_weight(FontWeight::BOLD).child("V"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_view = this.home_view;
                        cx.notify();
                    }))
            })
            .when(is_visible(SideView::Api), |rail| {
                rail.child(rail_btn(
                    "rail-api",
                    IconName::Network.into(),
                    SideView::Api,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::AutoTest), |rail| {
                rail.child(rail_btn(
                    "rail-test",
                    IconName::Play.into(),
                    SideView::AutoTest,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::Stress), |rail| {
                rail.child(rail_btn(
                    "rail-stress",
                    IconName::ChartPie.into(),
                    SideView::Stress,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::Proxy), |rail| {
                rail.child(rail_btn(
                    "rail-proxy",
                    IconName::Globe.into(),
                    SideView::Proxy,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::Share), |rail| {
                rail.child(rail_btn(
                    "rail-share",
                    vicon(DOCS),
                    SideView::Share,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::K8s), |rail| {
                rail.child(rail_btn(
                    "rail-k8s",
                    vicon(K8S_ICON),
                    SideView::K8s,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::Docker), |rail| {
                rail.child(rail_btn(
                    "rail-docker",
                    vicon(DOCKER_ICON),
                    SideView::Docker,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::ProjectManage), |rail| {
                rail.child(rail_btn(
                    "rail-project",
                    vicon(KANBAN),
                    SideView::ProjectManage,
                    active,
                    &theme,
                    cx,
                ))
            })
            // Environment variables button → opens the Settings window directly
            // (no dedicated sidebar panel; managed via the environments dialog).
            .when(!hidden.contains(SideView::Environments.name()), |rail| {
                rail.child(
                    Button::new("rail-env")
                        .ghost()
                        .small()
                        .icon(vicon(BRACES))
                        .selected(false)
                        .tooltip(rust_i18n::t!("view.environments").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_dialog = Some(PendingDialog::Environments);
                            cx.notify();
                        }))
                )
            })
            .when(is_visible(SideView::Mock), |rail| {
                rail.child(rail_btn(
                    "rail-mock",
                    IconName::Cpu.into(),
                    SideView::Mock,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::History), |rail| {
                rail.child(rail_btn(
                    "rail-history",
                    vicon(HISTORY),
                    SideView::History,
                    active,
                    &theme,
                    cx,
                ))
            })
            .when(is_visible(SideView::Ssh), |rail| {
                rail.child(rail_btn(
                    "rail-ssh",
                    IconName::SquareTerminal.into(),
                    SideView::Ssh,
                    active,
                    &theme,
                    cx,
                ))
            })
            .child(div().flex_1())
            // ---- Settings + theme picker (isolated from the feature buttons) ----
            // A thin separator + the Settings entry, then the theme picker below.
            .child(
                div()
                    .w(px(24.))
                    .h(px(1.))
                    .my_1()
                    .bg(theme.border.opacity(0.5)),
            )
            // Settings button — opens the settings window (通用设置 + 环境管理).
            .child(
                Button::new("rail-settings")
                    .ghost()
                    .small()
                    .icon(IconName::Settings)
                    .tooltip(rust_i18n::t!("settings.title").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending_dialog = Some(PendingDialog::Settings);
                        cx.notify();
                    })),
            )
            // Theme picker at the bottom.
            .child({
                let open = self.theme_popover_open;
                let theme_name = crate::ui::themes::current_theme_name(cx);
                Popover::new("theme-picker")
                    .anchor(gpui::Anchor::TopCenter)
                    .open(open)
                    .on_open_change(cx.listener(|this, open, _, cx| {
                        this.theme_popover_open = *open;
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("rail-theme")
                            .ghost()
                            .small()
                            .icon(IconName::Palette)
                            .tooltip(format!("主题：{}", theme_name)),
                    )
                    .p(px(4.))
                    .child({
                        let names = crate::ui::themes::theme_names(cx);
                        let active = theme_name.clone();
                        let entity = cx.entity();
                        let theme_fg = cx.theme().muted_foreground;
                        let theme_accent = cx.theme().accent;
                        let cols = 3usize;
                        let per = names.len().div_ceil(cols);
                        // Build a single column of theme items.
                        let build_items =
                            |slice: &[String],
                             offset: usize,
                             active: &str,
                             ent: &gpui::Entity<VerveApp>| {
                                v_flex().w(px(140.)).gap(px(1.)).children(
                                    slice.iter().enumerate().map(move |(i, name)| {
                                        let n = name.clone();
                                        let is_active = n == active;
                                        let ent = ent.clone();
                                        div()
                                            .id(format!("theme-opt-{}", offset + i))
                                            .w_full()
                                            .px(px(8.))
                                            .py(px(4.))
                                            .rounded(px(4.))
                                            .text_size(px(12.))
                                            .when(is_active, |d| {
                                                d.bg(gpui::black().opacity(0.3))
                                                    .text_color(gpui::white())
                                            })
                                            .when(!is_active, |d| d.text_color(theme_fg))
                                            .hover(|d| d.bg(theme_accent.opacity(0.3)))
                                            .child(n.clone())
                                            .on_click(move |_, _window, cx: &mut App| {
                                                crate::ui::themes::apply_theme(&n, cx);
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.theme_popover_open = false;
                                                    cx.notify();
                                                });
                                            })
                                    }),
                                )
                            };
                        h_flex()
                            .gap(px(4.))
                            .max_h(px(420.))
                            .child(build_items(&names[..per], 0, &active, &entity))
                            .child(build_items(&names[per..per * 2], per, &active, &entity))
                            .child(build_items(&names[per * 2..], per * 2, &active, &entity))
                    })
            })
    }

    /// Pick the title bar for the active view. The API workbench gets the full
    /// workspace/project/environment toolbar; the SSH manager gets a tailored
    /// bar (its hosts are global, so the workspace/project/env switchers don't
    /// apply). Both variants keep the same height and the same leading rail
    /// toggle, so the window chrome never shifts when the activity rail
    /// switches views.
    fn render_title_bar(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.active_view {
            SideView::Ssh => self.render_ssh_title_bar(cx).into_any_element(),
            SideView::Docker => self.render_docker_title_bar(cx).into_any_element(),
            SideView::K8s => self.render_k8s_title_bar(cx).into_any_element(),
            _ => self.render_api_title_bar(cx).into_any_element(),
        }
    }

    fn render_api_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
            // Active nav: the current project name as a breadcrumb.
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.muted_foreground)
                    .child(IconName::ChevronRight)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(project_name),
                    ),
            )
            // Share the whole project's docs (scope = Project).
            .child(
                Button::new("share-project")
                    .ghost()
                    .small()
                    .icon(vicon(SHARE))
                    .tooltip("分享当前项目文档")
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

    /// The far-left activity rail collapse/expand toggle — shared by every
    /// title-bar variant so the control sits at exactly the same position in
    /// all views.
    fn render_rail_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
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

    /// Update check button — shared by every title-bar variant. Shows a
    /// spinner while checking, a download icon when an update is available,
    /// or a refresh icon otherwise.
    fn render_update_button(&self, cx: &Context<Self>) -> impl IntoElement {
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

    /// Export format picker — a popover dropdown listing all supported export
    /// formats. Trigger uses the custom export icon; each row exports the
    /// active project in the chosen format on click.
    fn render_export_picker(&self, cx: &Context<Self>) -> impl IntoElement {
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
            .child(
                v_flex()
                    .w(px(200.))
                    .gap(px(2.))
                    .children(crate::export::Format::ALL.iter().map(|&f| {
                        let ent = cx.entity();
                        div()
                            .id(format!("export-fmt-{}", f.label()))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.muted))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child(format!("{} (.{})", f.label(), f.extension())),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.export_popover_open = false;
                                this.export_project(f, cx);
                            }))
                    })),
            )
    }

    /// Language switcher — shared by every title-bar variant.
    fn render_lang_picker(&self, cx: &Context<Self>) -> impl IntoElement {
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
                                let mut l = crate::state::persistence::load_layout().unwrap_or_default();
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
                                let mut l = crate::state::persistence::load_layout().unwrap_or_default();
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
    fn render_ssh_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
    fn render_docker_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let docker = self.docker.clone();
        let (host_count, active_host, cur_view) = {
            let r = self.docker.read(cx);
            (r.host_count(), r.active_host_name(), r.current_view().to_string())
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
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(IconName::LayoutDashboard),
                    )
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
    fn render_k8s_title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let k8s_import = k8s.clone();
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
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(IconName::Globe),
                    )
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
                                k8s_pods.update(cx, |p, cx| p.show_pods(cx));
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
                Button::new("k8s-title-import")
                    .ghost()
                    .small()
                    .label("导入 kubeconfig")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        k8s_import.update(cx, |p, cx| p.open_import_kubeconfig(window, cx));
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
                            crate::ui::k8s_cluster_dialog::open_dialog(window, cx);
                            let _ = panel.update(cx, |p, cx| p.refresh_current(cx));
                        });
                    })),
            )
            .child(self.render_update_button(cx))
            .child(self.render_lang_picker(cx))
    }
}

impl EventEmitter<()> for VerveApp {}

/// Build a popover menu row: optional icon + label, highlighted when active,
/// and an optional hover-revealed settings (gear) button on the right. The
/// settings button stops propagation so clicking it doesn't trigger the row's
/// main click. Uses a per-row hover group to reveal the gear.
fn menu_item(
    id: String,
    label: String,
    is_active: bool,
    icon: Option<IconName>,
    on_delete: Option<Box<dyn Fn(&mut gpui::App) + 'static>>,
    on_settings: Option<Box<dyn Fn(&mut gpui::App) + 'static>>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::App) + 'static,
    theme: &gpui_component::Theme,
) -> impl IntoElement {
    let theme = theme.clone();
    let has_action = on_settings.is_some() || on_delete.is_some();
    let group = format!("menu-row-{}", id);
    div()
        .id(id.clone())
        .w_full()
        .px(px(8.))
        .py(px(5.))
        .gap(px(6.))
        .flex()
        .items_center()
        .group(group.clone())
        .rounded(px(4.))
        .text_size(px(12.))
        .text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .when(is_active, |d| {
            d.bg(theme.primary.opacity(0.2))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .hover(|d| d.bg(theme.accent.opacity(0.4)))
        .when_some(icon, |d, ic| {
            d.child(
                div()
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child(ic),
            )
        })
        // The label: nowrap + ellipsis so icon and text stay on one line.
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(label),
        )
        .when(is_active, |d| {
            // When active AND no action buttons, show the check mark.
            d.when(!has_action, |d| {
                d.child(div().text_color(theme.primary).child(IconName::Check))
            })
        })
        // The hover-revealed delete button (left of settings).
        .when_some(on_delete, |d, on_del| {
            let theme_g = theme.clone();
            let theme_hover = theme.clone();
            d.child(
                div()
                    .id(format!("{}-del", id))
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_color(theme_g.muted_foreground)
                    .opacity(0.0)
                    .group_hover(group.clone(), move |b| {
                        b.opacity(1.0).text_color(theme_g.danger)
                    })
                    .hover(move |b| b.bg(theme_hover.danger.opacity(0.2)))
                    .child(IconName::Delete)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        move |_, window, cx: &mut gpui::App| {
                            cx.stop_propagation();
                            let _ = window;
                            (on_del)(cx);
                        },
                    ),
            )
        })
        // The hover-revealed settings button (rightmost).
        .when_some(on_settings, |d, on_set| {
            let theme_g = theme.clone();
            let theme_hover = theme.clone();
            d.child(
                div()
                    .id(format!("{}-set", id))
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_color(theme_g.muted_foreground)
                    // Hidden by default; revealed on row hover.
                    .opacity(0.0)
                    .group_hover(group.clone(), move |b| {
                        b.opacity(1.0).text_color(theme_g.foreground)
                    })
                    .hover(move |b| b.bg(theme_hover.border))
                    .child(IconName::Settings)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        move |_, window, cx: &mut gpui::App| {
                            cx.stop_propagation();
                            let _ = window;
                            (on_set)(cx);
                        },
                    ),
            )
        })
        .on_click(move |ev, _window, cx: &mut gpui::App| {
            (on_click)(ev, cx);
        })
}

/// A thin separator between menu groups.
fn menu_separator(theme: &gpui_component::Theme) -> impl IntoElement {
    div().w_full().h(px(1.)).my(px(2.)).bg(theme.border)
}

/// Show the "share created" result dialog with the given URL. Used by both
/// local-mode and cloud-mode share flows.
fn show_share_result_dialog(
    window: &mut Window,
    cx: &mut App,
    cfg: crate::share::ShareConfig,
    url: String,
) {
    // Build a QR code if the user selected that method.
    let wants_qr = cfg
        .share_methods
        .iter()
        .any(|m| matches!(m, crate::share::models::ShareMethod::QrCode));
    let qr_data_url = if wants_qr {
        crate::share::qrcode::to_svg_data_url(&url)
    } else {
        None
    };

    // If the user chose "导出 HTML", write a copy to the exports folder.
    // (In cloud mode this still happens locally — it's an offline artifact.)
    let cfg_id_for_html = cfg.id.clone();
    let cfg_project_id = cfg.project_id.clone();
    if cfg
        .share_methods
        .iter()
        .any(|m| matches!(m, crate::share::models::ShareMethod::ExportHtml))
    {
        if let Ok(export_dir) =
            crate::state::persistence::data_dir().map(|d| d.join("exports"))
        {
            let _ = std::fs::create_dir_all(&export_dir);
            let project = crate::state::persistence::load_or_default()
                .projects
                .iter()
                .find(|p| p.id == cfg_project_id)
                .cloned();
            if let Some(project) = project {
                let html = crate::share::html::render_doc_html(&cfg, &project);
                let fname = format!("{}.html", cfg_id_for_html);
                let _ = std::fs::write(export_dir.join(&fname), html);
            }
        }
    }

    cx.open_url(&url);

    let title = cfg.display_title();
    let expire_label = cfg.expire.label().to_string();
    let access_label = cfg.access_label().to_string();
    let url_arc = std::sync::Arc::new(url);
    let qr_arc = std::sync::Arc::new(qr_data_url);

    window.open_dialog(cx, move |dialog, _window, cx| {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let theme_muted = theme.muted;
        let qr = (*qr_arc).clone();
        let title_text = title.clone();
        let expire_label = expire_label.clone();
        let access_label = access_label.clone();
        let url_for_content = url_arc.clone();
        dialog
            .title("分享已创建")
            .content(move |content, _window, _cx| {
                let url_content = (*url_for_content).clone();
                let url_copy = (*url_for_content).clone();
                content.child(
                    v_flex()
                        .p_4()
                        .w(px(440.))
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("文档「{}」分享成功", title_text)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("访问以下链接查看文档："),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme_muted)
                                        .text_xs()
                                        .child(url_content),
                                )
                                .child(
                                    Button::new("share-copy-link")
                                        .ghost()
                                        .small()
                                        .label("复制")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(
                                                gpui::ClipboardItem::new_string(url_copy.clone()),
                                            );
                                        }),
                                ),
                        )
                        .when_some(qr.as_ref(), |col, qr_url| {
                            col.child(
                                div()
                                    .mt_2()
                                    .items_center()
                                    .justify_center()
                                    .flex()
                                    .child(img(qr_url.clone()).w(px(200.)).h(px(200.))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .text_center()
                                    .child("扫描二维码访问"),
                            )
                        })
                        .child(div().h(px(1.)).w_full().bg(border).text_color(muted))
                        .child(div().text_xs().text_color(muted).child(format!(
                            "有效期：{}  ·  访问限制：{}",
                            expire_label, access_label
                        ))),
                )
            })
            .footer(
                Button::new("share-result-close")
                    .primary()
                    .small()
                    .label("完成")
                    .on_click(|_, window, cx| {
                        window.close_dialog(cx);
                    }),
            )
    });
}
