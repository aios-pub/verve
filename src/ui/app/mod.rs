//! Workspace shell: title bar + three-pane body, wired together.

use gpui::prelude::FluentBuilder as _;
use gpui::{img, *};

// Actions for keybindings (PRD §6).
actions!(
    verve,
    [
        SaveWorkspace,
        NewRequest,
        CloseFile,
    ]
);

/// Switch to one of the fixed shortcut views (payload indexes
/// `SideView::SHORTCUT_VIEWS`). This rev of gpui's `actions!` macro only
/// defines unit actions, so data-carrying actions are declared via the
/// derive; `no_json` because it only originates from in-code keybindings,
/// never from JSON.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = verve, no_json)]
pub struct SelectRailSlot(pub usize);

/// Keybindings for the rail switcher: ⌘/Ctrl + 1..=N switch to the Nth of
/// `SideView::SHORTCUT_VIEWS` (the commonly-used views).
///
/// Context "!BlockEditor": a context-less binding ties with the deepest key
/// context and could shadow deeper-focused bindings; with the negated
/// context, more specific bindings win while a block editor is focused.
pub fn rail_slot_keybindings(modifier: &str) -> Vec<KeyBinding> {
    SideView::SHORTCUT_VIEWS
        .iter()
        .enumerate()
        .map(|(i, _)| {
            KeyBinding::new(
                &format!("{}-{}", modifier, i + 1),
                SelectRailSlot(i),
                Some("!BlockEditor"),
            )
        })
        .collect()
}

/// Center a window of the requested logical size on the primary display,
/// clamping the size to the display's visible (work-area) bounds first and
/// centering within the work area (not the full screen), so the window
/// never overlaps the Dock / taskbar on launch.
pub fn centered_window_bounds(width: Pixels, height: Pixels, cx: &App) -> Bounds<Pixels> {
    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, gpui::size(width, height), cx);
    };
    let visible = display.visible_bounds();
    // Small margin so the window never touches the screen edges;
    // never shrink below a usable floor for tiny screens.
    let margin = px(24.);
    let floor = gpui::size(px(320.), px(240.));
    let avail = gpui::size(
        (visible.size.width - margin).max(floor.width),
        (visible.size.height - margin).max(floor.height),
    );
    let size = gpui::size(width.min(avail.width), height.min(avail.height));
    // `Bounds::centered` centers on the full display bounds; when the Dock /
    // taskbar sits at the bottom, that center lies below the work-area
    // center and the window's bottom edge slightly overlaps it.
    Bounds::centered_at(visible.center(), size)
}

/// Apply Verve's windowing conventions to a self-chromed OS window (one that
/// renders the gpui-component `TitleBar` with drawn min/max/close controls).
///
/// * `window_decorations: Client` — the TitleBar draws its own controls on
///   every non-macOS platform unconditionally, while gpui otherwise asks
///   the Linux WM for server-side decorations by default; the WM's title
///   bar then stacks a *second* close button on top of ours. Client-side
///   decorations remove the WM chrome (X11: Motif undecorated hints;
///   Wayland: xdg-decoration client mode). Windows/macOS ignore this field,
///   so it is set unconditionally.
/// * `kind: Dialog` (Linux only, when `dialog`) — dialog-type toplevels are
///   centered over and kept above their transient parent (the focused main
///   window) by the WM/compositor, which programmatic bounds cannot
///   achieve: Wayland clients cannot position themselves at all, and X11
///   WMs re-place untyped windows. Only the settings-family windows pass
///   `dialog = true`.
pub fn apply_window_chrome_fixes(options: &mut gpui::WindowOptions, dialog: bool) {
    options.window_decorations = Some(gpui::WindowDecorations::Client);
    #[cfg(target_os = "linux")]
    if dialog {
        options.kind = gpui::WindowKind::Dialog;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dialog;
}
use crate::assets::{
    BRACES, BRACES_JSON, DOCS, EXPORT, HISTORY, IMPORT, REFRESH_CW, SAVE, SAVE_AS, SERVER, SHARE,
};
use gpui_component::Size::Medium;
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    resizable::{ResizableState, h_resizable, resizable_panel, v_resizable},
    v_flex,
};

/// Build an icon from a Verve-custom Lucide SVG path.
fn vicon(path: &'static str) -> Icon {
    Icon::from(IconName::Redo).path(path)
}

use crate::git::GitState;
use crate::share::models::{ShareConfig, ShareScope};
use crate::share::server::{self, ShareServer};
use crate::state::{AppEvent, AppState};
use crate::ui::console_panel::{ConsoleEvent, ConsolePanel};
use crate::ui::hosts_panel::HostsPanel;
use crate::ui::json_panel::JsonPanel;
use crate::ui::mock_console_panel::{MockConsoleEvent, MockConsolePanel};
use crate::ui::project_manage_panel::ProjectManagePanel;
use crate::ui::project_tree_panel::ProjectTreePanel;
use crate::ui::proxy_panel::ProxyPanel;
use crate::ui::request_panel::RequestPanel;
use crate::ui::response_panel::ResponsePanel;
use crate::ui::share_dialog;
use crate::ui::share_panel::{ShareEvent, SharePanel};
use crate::ui::ssh_panel::SshPanel;

// ---- submodule declarations (impl VerveApp blocks live in siblings) ----
mod actions;
mod construction;
mod rail;
mod share;
mod titlebar;
mod widgets;
mod workspaces;

// Make sibling-module helpers visible to mod.rs's Render impl.
use widgets::*;

pub struct VerveApp {
    pub state: Entity<AppState>,
    pub tree: Entity<ProjectTreePanel>,
    pub request: Entity<RequestPanel>,
    pub response: Entity<ResponsePanel>,
    pub console: Entity<ConsolePanel>,
    /// Mock service console panel.
    pub mock_console: Entity<MockConsolePanel>,
    /// Git version-control state (gix/gitoxide-backed).
    pub git: Entity<GitState>,
    /// Project-management sidebar panel (Git status / history / branches).
    pub project_manage: Entity<ProjectManagePanel>,
    /// Document-sharing management panel (文档管理).
    pub share: Entity<SharePanel>,
    /// SSH management panel (主机卡片 + 终端).
    pub ssh: Entity<SshPanel>,
    /// HTTP proxy / traffic capture panel.
    pub proxy: Entity<ProxyPanel>,
    /// Hosts quick editor panel.
    pub hosts: Entity<HostsPanel>,
    /// JSON formatter / pretty printer panel.
    pub json: Entity<JsonPanel>,
    /// Live share HTTP server handle (started when any share config exists).
    pub share_server: Option<ShareServer>,
    /// Shared config store backing the share server (hot-swappable).
    pub share_configs: std::sync::Arc<std::sync::RwLock<Vec<ShareConfig>>>,
    /// Display host for share URLs (defaults to 127.0.0.1).
    pub share_host: String,
    /// Port the share server is listening on.
    pub share_port: u16,
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
    /// Custom order of activity rail items (SideView names).
    pub rail_order: Vec<String>,
    /// The SideView name currently being dragged (if any).
    pub dragging_rail: Option<String>,
    /// The SideView name that is the current drop target (if any).
    pub rail_drop_target: Option<String>,
    _subs: Vec<gpui::Subscription>,
}

/// Drag payload for activity rail reordering — carries the SideView name.
#[derive(Debug, Clone)]
struct RailDrag(pub String);

impl Render for RailDrag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = SideView::parse(&self.0);
        let label = view.label();
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .rounded(theme.radius)
            .bg(theme.primary.opacity(0.9))
            .text_color(gpui::white())
            .text_xs()
            .shadow_md()
            .child(VerveApp::rail_icon_for(view))
            .child(label)
    }
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
    /// Mock service status.
    Mock,
    /// Git-based project management.
    ProjectManage,
    /// Document-sharing management (文档管理).
    Share,
    /// SSH connection management (SSH 管理).
    Ssh,
    /// HTTP proxy / traffic capture.
    Proxy,
    /// Hosts quick editor.
    Hosts,
    /// JSON formatter / pretty printer with collapsible tree.
    JsonFormat,
}

impl SideView {
    /// All rail-selectable views, in rail order.
    pub const ALL: &'static [SideView] = &[
        SideView::Api,
        SideView::Ssh,
        SideView::Share,
        SideView::Mock,
        SideView::ProjectManage,
        SideView::JsonFormat,
        SideView::Hosts,
        SideView::Proxy,
        SideView::History,
    ];

    /// The daily-workflow views with fixed ⌘/Ctrl+1..=5 rail-switch shortcuts,
    /// in key order. Mirrors the Community Edition's positioning (HTTP
    /// debugging, SSH, mock, JSON, capture).
    pub const SHORTCUT_VIEWS: &'static [SideView] = &[
        SideView::Api,
        SideView::Ssh,
        SideView::JsonFormat,
        SideView::Mock,
        SideView::Proxy,
    ];

    /// A stable string key for persistence.
    pub fn name(self) -> &'static str {
        match self {
            SideView::Api => "Api",
            SideView::History => "History",
            SideView::Mock => "Mock",
            SideView::ProjectManage => "ProjectManage",
            SideView::Share => "Share",
            SideView::Ssh => "Ssh",
            SideView::Proxy => "Proxy",
            SideView::Hosts => "Hosts",
            SideView::JsonFormat => "JsonFormat",
        }
    }

    /// Parse a string key back into a view (falls back to Api).
    pub fn parse(s: &str) -> SideView {
        match s {
            "History" => SideView::History,
            "Mock" => SideView::Mock,
            "ProjectManage" => SideView::ProjectManage,
            "Share" => SideView::Share,
            "Ssh" => SideView::Ssh,
            "Proxy" => SideView::Proxy,
            "Hosts" => SideView::Hosts,
            "JsonFormat" => SideView::JsonFormat,
            _ => SideView::Api,
        }
    }

    /// Human-readable label, i18n-aware (matches the rail tooltip).
    pub fn label(self) -> String {
        match self {
            SideView::Api => rust_i18n::t!("view.api").to_string(),
            SideView::History => rust_i18n::t!("view.history").to_string(),
            SideView::Mock => rust_i18n::t!("view.mock").to_string(),
            SideView::ProjectManage => rust_i18n::t!("view.project_manage").to_string(),
            SideView::Share => rust_i18n::t!("view.share").to_string(),
            SideView::Ssh => rust_i18n::t!("view.ssh").to_string(),
            SideView::Proxy => "抓包".to_string(),
            SideView::Hosts => rust_i18n::t!("view.hosts").to_string(),
            SideView::JsonFormat => rust_i18n::t!("view.json_format").to_string(),
        }
    }
}

/// Make a project name safe for use as a filename.
pub(super) fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
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
        if self.active_view != self.home_view && self.hidden_rails.contains(self.active_view.name())
        {
            self.active_view = self.home_view;
        }
        // Note: unified share+mock server is started eagerly in constructor for local mode,
        // no lazy start needed anymore.
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
                        let bounds = centered_window_bounds(px(960.), px(620.), cx);
                        let mut options = gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                            window_min_size: Some(gpui::size(px(820.), px(520.))),
                            titlebar: None,
                            is_minimizable: true,
                            ..Default::default()
                        };
                        apply_window_chrome_fixes(&mut options, true);
                        let _ = cx.open_window(
                            options,
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
                        let bounds = centered_window_bounds(px(720.), px(560.), cx);
                        let mut options = gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                            window_min_size: Some(gpui::size(px(600.), px(440.))),
                            titlebar: None,
                            is_minimizable: true,
                            ..Default::default()
                        };
                        apply_window_chrome_fixes(&mut options, true);
                        let _ = cx.open_window(
                            options,
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
        // Overlay layers (dialogs/sheets/notifications) are mounted by the
        // gpui-component Root itself since 0.7.0 — no consumer wiring needed.

        let theme = cx.theme().clone();
        let show_console = self.show_console;
        let tree = self.tree.clone();
        let request = self.request.clone();
        let response = self.response.clone();
        let console = self.console.clone();
        let project_manage = self.project_manage.clone();
        let ssh = self.ssh.clone();
        let share = self.share.clone();

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
            SideView::Api => tree.into_any_element(),
            SideView::History => console.clone().into_any_element(),
            SideView::ProjectManage
            | SideView::Share
            | SideView::Ssh
            | SideView::Proxy
            | SideView::Hosts
            | SideView::JsonFormat
            | SideView::Mock => {
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

        // ProjectManage and Share are full-body exclusive views: they replace
        // the entire workbench (rail + tree + request/response).
        // Exclusive views: rail + title bar visible, body taken over by the panel.
        let exclusive_view = matches!(
            self.active_view,
            SideView::ProjectManage
                | SideView::Share
                | SideView::Ssh
                | SideView::Proxy
                | SideView::Hosts
                | SideView::JsonFormat
                | SideView::Mock
        );

        v_flex()
            .id("verve-app-root")
            .size_full()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::on_save_workspace))
            .on_action(cx.listener(Self::on_close_file))
            .on_action(cx.listener(Self::on_select_rail_slot))
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
                        let panel: gpui::AnyElement = match self.active_view {
                            SideView::ProjectManage => project_manage.clone().into_any_element(),
                            SideView::Share => share.clone().into_any_element(),
                            SideView::Ssh => ssh.clone().into_any_element(),
                            SideView::Proxy => self.proxy.clone().into_any_element(),
                            SideView::Hosts => self.hosts.clone().into_any_element(),
                            SideView::JsonFormat => self.json.clone().into_any_element(),
                            SideView::Mock => self.mock_console.clone().into_any_element(),
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
    }
}

impl EventEmitter<()> for VerveApp {}
