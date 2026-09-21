//! SSH management panel — the full-bleed "SSH 管理" view.
//!
//! Two layouts:
//! - **HostList**: a responsive card grid of saved SSH hosts + "new host" button.
//! - **Terminal**: (planned) multi-tab terminal sessions.
//!
//! The panel is an exclusive view (`SideView::Ssh`), replacing the entire
//! workbench — just like `ProjectManage` and `Share`.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::FutureExt as _;

use crate::ssh::client::SshConnection;
use crate::ssh::credentials;
use crate::ssh::host_store;
use crate::ssh::local_pty::LocalPtySession;
use crate::ssh::models::SshHost;
use crate::ssh::port_forward::LocalForward;
use crate::ssh::terminal::TerminalView;
use crate::ui::sftp_browser::SftpBrowser;

/// Events the SSH panel emits upward.
#[derive(Clone, Debug)]
pub enum SshEvent {
    NewHost,
    EditHost(String),
    Connect(String),
}

/// Reply sent from the UI back to the pump thread after the user chooses a
/// file path for an rz/sz transfer.
enum ZmPathReply {
    Save(Option<PathBuf>),
    Send(Option<PathBuf>),
}

/// Monotonic counter so session uids are unique even when two sessions are
/// created within the same nanosecond.
static SESSION_UID_COUNTER: AtomicU64 = AtomicU64::new(1);
fn rand_session_seed() -> u64 {
    SESSION_UID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Force-switch to English input method when opening a terminal session, so
/// keystrokes reach the shell immediately instead of being swallowed by the
/// Chinese IME. Only switches at open — the user can freely toggle back to
/// Chinese afterwards. Best-effort per platform; failures are logged, never
/// block session creation.
fn switch_to_english_input() {
    #[cfg(target_os = "macos")]
    {
        // Preferred path: the Carbon TIS API. Synchronous, permission-free,
        // and deterministic (System Events key simulation needs Accessibility
        // access that unsigned builds don't have, and races with fast typists).
        match crate::ssh::macos_input::select_english_keyboard_layout() {
            Ok(source_id) => {
                log::info!("输入法已切换为英文键盘布局: {source_id}");
            }
            Err(err) => {
                log::warn!("TIS 切换输入法失败({err}),回退 osascript 按键模拟");
                std::thread::spawn(|| {
                    let _ = std::process::Command::new("osascript")
                        .arg("-e")
                        .arg(r#"tell application "System Events" to key code 102"#)
                        .output();
                });
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        // ActivateKeyboardLayout switches the calling (UI) thread's layout,
        // so only Verve's window is affected.
        match crate::ssh::windows_input::select_english_keyboard_layout() {
            Ok(id) => log::info!("输入法已切换为英文键盘布局: {id}"),
            Err(err) => log::warn!("切换英文键盘布局失败: {err}"),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match crate::ssh::linux_input::select_english_keyboard_layout() {
            Ok(via) => log::info!("输入法已切换为英文输入: {via}"),
            Err(err) => log::warn!("切换英文输入失败: {err}"),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {}
}

/// Elide a server-supplied prompt to `max` characters (by `char` count, not
/// bytes — safe for UTF-8). Used for MFA prompt display.
fn truncate_prompt(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let head: String = chars.iter().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Messages from the SSH pump thread back to GPUI's executor.
enum PumpMsg {
    Data(Vec<u8>),
    InputTx(tokio::sync::mpsc::UnboundedSender<Vec<u8>>),
    ResizeTx(tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>),
    /// A connection has been established (used to make Sftp available on demand).
    Connected(crate::ssh::client::SshConnection),
    Closed(Option<u32>),
    /// Connection error. `retryable == false` means it's a configuration
    /// problem (bad credentials / wrong port / host-key mismatch) that the UI
    /// should not retry.
    Error {
        message: String,
        retryable: bool,
    },
    Disconnected,
    /// ZMODEM UI event (started/progress/completed/error). Carries the
    /// originating session's uid so multi-tab events are routed correctly.
    ZmEvent {
        uid: u64,
        event: crate::ssh::zmodem::ZmEvent,
    },
    /// ZMODEM needs a save path for an incoming file. UI must reply via reply_tx.
    ZmNeedSavePath {
        uid: u64,
        suggested_name: String,
        reply_tx: tokio::sync::mpsc::UnboundedSender<ZmPathReply>,
    },
    /// ZMODEM needs a file to send. UI must reply via reply_tx.
    ZmNeedFileToSend {
        uid: u64,
        reply_tx: tokio::sync::mpsc::UnboundedSender<ZmPathReply>,
    },
    /// The SSH server issued a keyboard-interactive (MFA) challenge and needs
    /// the user's verification code. The UI must render an OTP input and reply
    /// via the oneshot sender; dropping it without sending = user cancelled.
    MfaPrompt {
        prompt_text: String,
        reply_tx: tokio::sync::oneshot::Sender<String>,
    },
}

/// Which view is active within a session tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionView {
    Terminal,
    Sftp,
    Forwards,
}

/// One active terminal session.
pub struct TerminalSession {
    pub host_name: String,
    pub host_id: String,
    /// Unique-per-session random id (u64) so concurrent tabs for the same
    /// host can route ZMODEM events independently (host_id is shared across
    /// all tabs for the same host, which would cause cross-tab bleed).
    pub session_uid: u64,
    pub terminal: Entity<TerminalView>,
    /// SFTP file browser, initialized lazily when the user first switches to
    /// the Files view.
    pub sftp_browser: Option<Entity<SftpBrowser>>,
    pub view: SessionView,
    /// Shared authenticated SSH connection (used to open SFTP on demand).
    pub connection: Option<SshConnection>,
    /// Input sender for writing keystrokes to SSH.
    pub input_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    /// Resize sender for terminal size changes.
    pub resize_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>>,
    /// Status of the connection.
    pub status: SessionStatus,
    /// Active ZMODEM transfer state (if any).
    pub(crate) zm_state: ZmUiState,
    /// Currently-active local port forwards. Dropping this vec (or the session)
    /// stops the forwards via LocalForward::stop's Drop impl on abort.
    pub active_forwards: Vec<LocalForward>,
    /// OTP/MFA input field, present only while a keyboard-interactive challenge
    /// is awaiting a response. `None` = no pending prompt. Created lazily in
    /// `render` (which has window access) from `pending_mfa_prompt`.
    pub(crate) mfa_input: Option<Entity<InputState>>,
    /// The prompt text (e.g. "Verification code:") for the current MFA challenge.
    pub(crate) mfa_prompt: String,
    /// Reply channel back to the SSH connect thread for the current MFA prompt.
    pub(crate) mfa_reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
    /// A freshly-arrived MFA challenge awaiting UI setup. Set from the spawn
    /// loop (no window access); consumed in `render` to build `mfa_input`.
    pub(crate) pending_mfa_prompt: Option<(String, tokio::sync::oneshot::Sender<String>)>,
    /// Local PTY handle when this tab is a "local terminal" (the user's own
    /// shell on this machine, no SSH involved). `None` = remote SSH session.
    pub local: Option<LocalPtySession>,
}

impl TerminalSession {
    /// True when this tab runs the local shell in a local PTY — SFTP/forwards
    /// and reconnect logic do not apply.
    pub fn is_local(&self) -> bool {
        self.local.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    Reconnecting(usize), // attempt number
    Error(String),
    Closed,
}

/// Derive a short status-bar label (shown as "✕ <label>") from a detailed
/// SSH error message. Keeps the status bar compact while the terminal shows
/// the full explanation.
fn ssh_error_short_label(message: &str) -> String {
    if message.contains("认证失败") {
        "认证失败".to_string()
    } else if message.contains("连接被拒绝") {
        "端口不通".to_string()
    } else if message.contains("超时") {
        "连接超时".to_string()
    } else if message.contains("主机密钥") {
        "主机密钥异常".to_string()
    } else if message.contains("无法解析主机") || message.contains("网络不可达") {
        "主机不可达".to_string()
    } else if message.contains("跳板机") {
        "跳板机错误".to_string()
    } else {
        "连接失败".to_string()
    }
}

/// Active ZMODEM transfer state (GPUI UI-side mirror of what the pump actor reports).
#[derive(Clone)]
pub(crate) struct ZmUiState {
    active: bool,
    filename: String,
    total: u64,
    transferred: u64,
    role: crate::ssh::zmodem::ZmRole,
    done: bool,
    error: Option<String>,
    cancelled: bool,
}

impl Default for ZmUiState {
    fn default() -> Self {
        Self {
            active: false,
            filename: String::new(),
            total: 0,
            transferred: 0,
            role: crate::ssh::zmodem::ZmRole::ReceiveFromRemote,
            done: false,
            error: None,
            cancelled: false,
        }
    }
}

pub struct SshPanel {
    /// Cached host list, reloaded on render when stale.
    pub hosts: Vec<SshHost>,
    /// Set true when the data may have changed; triggers a reload on render.
    pub stale: bool,
    /// Expanded bastion card ids.
    pub expanded_bastions: std::collections::HashSet<String>,
    /// Active terminal sessions (one per tab).
    pub sessions: Vec<TerminalSession>,
    /// Index of the active tab.
    pub active_tab: usize,
    /// Pending connection requests (processed in render where a Window exists).
    pub pending_connect: Option<SshHost>,
    /// Currently-open tab right-click context menu: the right-clicked tab
    /// index plus the mouse position (used to anchor the floating menu).
    pub tab_context_menu: Option<(usize, gpui::Point<gpui::Pixels>)>,
    /// The panel's window-absolute bounds, captured each render via a hidden
    /// canvas child. Used to convert window-absolute mouse coordinates (from
    /// MouseDownEvent) into panel-relative coordinates for positioning the
    /// context-menu overlay (GPUI's `absolute` is relative to the parent's
    /// content box, not the window viewport).
    pub panel_bounds: std::sync::Arc<std::sync::Mutex<gpui::Bounds<gpui::Pixels>>>,
    /// When true, show the host picker overlay (for starting new connections
    /// without closing existing sessions).
    pub show_host_picker: bool,
    /// When true (and sessions exist), the content area shows the full host
    /// card grid instead of the active session — the "host list" pseudo-tab.
    /// Sessions stay alive in the background; clicking any session tab flips
    /// this back to false.
    pub show_host_grid: bool,
    /// When Some(message), show a small status toast in the Forwards view
    /// (e.g. "已启动: 127.0.0.1:8080 → ..."). Cleared after a few seconds by
    /// the render loop when stale.
    pub fwd_status: Option<(std::time::Instant, String)>,
    /// Cached InputState entities for the "add forward" form. Created lazily
    /// on first render (they require a `&Window`).
    pub fwd_inputs: Option<FwdInputs>,
    /// Cached InputState for the host-picker search box (filters by name/host).
    /// Created lazily on first render; its value persists across opens.
    pub host_picker_search: Option<Entity<InputState>>,
    focus_handle: FocusHandle,
    _subs: Vec<gpui::Subscription>,
}

/// Cached InputState entities for the add-forward form.
pub struct FwdInputs {
    pub local_port: Entity<InputState>,
    pub remote_host: Entity<InputState>,
    pub remote_port: Entity<InputState>,
}

impl EventEmitter<SshEvent> for SshPanel {}

impl SshPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let hosts = host_store::load_hosts();
        Self {
            hosts,
            stale: false,
            expanded_bastions: std::collections::HashSet::new(),
            sessions: Vec::new(),
            active_tab: 0,
            pending_connect: None,
            tab_context_menu: None,
            panel_bounds: std::sync::Arc::new(std::sync::Mutex::new(gpui::Bounds::default())),
            show_host_picker: false,
            show_host_grid: false,
            fwd_status: None,
            fwd_inputs: None,
            host_picker_search: None,
            focus_handle,
            _subs: Vec::new(),
        }
    }

    /// Lazily build the add-forward InputState entities on first render.
    fn ensure_fwd_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fwd_inputs.is_some() {
            return;
        }
        let local_port = cx.new(|cx| InputState::new(window, cx).placeholder("本地端口 8080"));
        let remote_host =
            cx.new(|cx| InputState::new(window, cx).placeholder("远端 host 127.0.0.1"));
        let remote_port = cx.new(|cx| InputState::new(window, cx).placeholder("远端端口 80"));
        self.fwd_inputs = Some(FwdInputs {
            local_port,
            remote_host,
            remote_port,
        });
    }

    /// Reload the cached hosts from disk.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.hosts = host_store::load_hosts();
        self.stale = false;
        cx.notify();
    }
}

impl Render for SshPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.stale {
            self.reload(cx);
        }
        // Reload hosts if the picker is about to be shown (ensure fresh list).
        if self.show_host_picker && self.hosts.is_empty() {
            self.hosts = host_store::load_hosts();
        }
        // Process pending connection.
        if let Some(host) = self.pending_connect.take() {
            self.start_session(host, None, cx);
        }
        let theme = cx.theme().clone();
        let bg = theme.background;
        let fg = theme.foreground;
        let _accent = theme.accent;

        // Two modes: terminal sessions (if any) or host card grid.
        // When show_host_picker is true and we have sessions, show the host
        // picker as an overlay on top of the terminal view.
        let has_sessions = !self.sessions.is_empty();
        let show_picker = self.show_host_picker;

        if has_sessions {
            let terminal_view = self.render_terminal_view(window, cx, theme.clone());

            if show_picker {
                // Overlay: dimmed background + host card grid on top.
                return v_flex()
                    .size_full()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(bg)
                    .text_color(fg)
                    .child(terminal_view)
                    .child(self.render_host_picker_overlay(window, cx, theme))
                    .into_any_element();
            }
            return terminal_view.into_any_element();
        }

        // No panel-local header: the app title bar already shows the SSH
        // context (title, host count, "new host" action) — see
        // `VerveApp::render_ssh_title_bar`. A second bar here would stack two
        // headers on top of each other.
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(bg)
            .text_color(fg)
            // Body: host card grid.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("ssh-host-scroll")
                    .overflow_y_scroll()
                    .p_4()
                    .child(self.render_card_grid(cx, theme.clone())),
            )
            .into_any_element()
    }
}

impl SshPanel {
    fn render_card_grid(
        &self,
        cx: &mut Context<Self>,
        theme: gpui_component::Theme,
    ) -> impl IntoElement {
        let panel = cx.entity();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let _fg = theme.foreground;
        let accent = theme.accent;
        let danger = theme.danger;
        // Hosts that currently have at least one Connected session — used to
        // badge cards with a live-status dot while browsing the host grid.
        let connected_hosts: std::collections::HashSet<String> = self
            .sessions
            .iter()
            .filter(|s| matches!(s.status, SessionStatus::Connected))
            .map(|s| s.host_id.clone())
            .collect();

        if self.hosts.is_empty() {
            return v_flex()
                .items_center()
                .justify_center()
                .size_full()
                .gap_3()
                .child(div().text_2xl().child("🖥️"))
                .child(
                    div()
                        .text_color(muted)
                        .child("还没有 SSH 主机。点击右上角「新建主机」添加一台。"),
                )
                .child(
                    Button::new("ssh-local-term-empty")
                        .primary()
                        .small()
                        .label("打开本地终端")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_local_session(cx);
                        })),
                )
                .into_any_element();
        }

        // Build a lookup for jump host names.
        let host_name_of = |id: &str| -> String {
            self.hosts
                .iter()
                .find(|h| h.id == id)
                .map(|h| h.name.clone())
                .unwrap_or_else(|| "(已删除)".into())
        };

        let grid = h_flex()
            .flex_wrap()
            .gap_3()
            // Local terminal quick card — always first in the grid.
            .child(
                v_flex()
                    .id("ssh-local-term-card")
                    .w(px(220.))
                    .h(px(120.))
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(theme.background)
                    .hover(|d| d.border_color(accent))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_lg().child("⌨️"))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("本地终端"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("在本机伪终端中打开默认 Shell"),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .mt_1()
                            .child(
                                Button::new("ssh-local-term-open")
                                    .primary()
                                    .xsmall()
                                    .label("打开")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_local_session(cx);
                                    })),
                            ),
                    ),
            )
            .children(self.hosts.iter().enumerate().map(|(ix, host)| {
                let theme_c = theme.clone();
                let _theme_c2 = theme.clone();
                let _theme_c3 = theme.clone();
                let host_id = host.id.clone();
                let panel_c = panel.clone();
                let host_id_edit = host.id.clone();
                let host_id_del = host.id.clone();
                let host_id_dup = host.id.clone();
                let jump_name = host.bastion_id.as_ref().map(|jid| host_name_of(jid));
                let is_connected = connected_hosts.contains(&host.id);

                // Card: 200px wide, auto height.
                v_flex()
                    .id(("ssh-card", ix))
                    .w(px(220.))
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(theme_c.background)
                    .hover(|d| d.border_color(accent))
                    // Top row: icon + name (+ live dot when connected).
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_lg().child("🖥️"))
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(host.name.clone()),
                            )
                            .when(is_connected, |row| {
                                row.child(
                                    div()
                                        .w(px(8.))
                                        .h(px(8.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(gpui::hsla(0.33, 0.6, 0.4, 1.0)),
                                )
                            }),
                    )
                    // Address.
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(host.address_summary()),
                    )
                    // Auth type badge.
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py(px(1.))
                                    .rounded(px(3.))
                                    .bg(theme_c.muted)
                                    .child(host.auth_type.label().to_string()),
                            )
                            .when_some(jump_name.as_ref(), |row, name| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .px_2()
                                        .py(px(1.))
                                        .rounded(px(3.))
                                        .bg(accent.opacity(0.2))
                                        .text_color(accent)
                                        .child(format!("🔗 {}", name)),
                                )
                            }),
                    )
                    // Actions.
                    .child(
                        h_flex()
                            .gap_1()
                            .mt_1()
                            .child(
                                Button::new(("ssh-connect", ix))
                                    .primary()
                                    .xsmall()
                                    .label("连接")
                                    .on_click(move |_, _window, cx: &mut App| {
                                        let _ = panel_c.update(cx, |this, cx| {
                                            // Find the host and start a session.
                                            if let Some(host) =
                                                this.hosts.iter().find(|h| h.id == host_id).cloned()
                                            {
                                                this.pending_connect = Some(host);
                                                cx.notify();
                                            }
                                        });
                                    }),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new(("ssh-duplicate", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .tooltip("复制（连同密码）")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.duplicate_host(&host_id_dup, cx);
                                    })),
                            )
                            .child(
                                Button::new(("ssh-edit", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Settings)
                                    .tooltip("编辑")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_host_dialog(Some(&host_id_edit), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("ssh-delete", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .text_color(danger)
                                    .tooltip("删除")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_delete(&host_id_del, window, cx);
                                    })),
                            ),
                    )
            }))
            // "New host" card.
            .child(
                v_flex()
                    .id("ssh-new-card")
                    .w(px(220.))
                    .h(px(120.))
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_dashed()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|d| d.border_color(accent).bg(accent.opacity(0.05)))
                    .child(div().text_2xl().child("＋"))
                    .child(div().text_sm().text_color(muted).child("新建主机"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_host_dialog(None, window, cx);
                    })),
            );

        grid.into_any_element()
    }

    /// Number of saved hosts — shown as a badge in the app's SSH title bar.
    pub fn host_count(&self) -> usize {
        self.hosts.len()
    }

    /// Open the "new host" dialog from outside the panel (the app title bar's
    /// primary action).
    pub fn open_new_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_host_dialog(None, window, cx);
    }

    /// Open the host dialog for new or editing.
    fn open_host_dialog(
        &mut self,
        editing: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editing_host = editing.and_then(|id| host_store::find_host(id));
        let available = self.hosts.clone();
        let panel = cx.entity();
        crate::ui::ssh_host_dialog::open_dialog(
            editing_host.as_ref(),
            available,
            move |_host, _window, cx: &mut App| {
                let _ = panel.update(cx, |this, cx| {
                    this.reload(cx);
                });
            },
            window,
            cx,
        );
    }

    /// Confirm-then-delete a host.
    fn confirm_delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let host_name = host_store::find_host(id)
            .map(|h| h.name)
            .unwrap_or_else(|| id.to_string());
        let id_del = id.to_string();
        let panel = cx.entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let host_name = host_name.clone();
            dialog
                .title("确认删除")
                .content(move |content, _, _| {
                    content.child(
                        v_flex().p_4().w(px(360.)).gap_2().child(
                            div()
                                .text_sm()
                                .text_color(gpui::hsla(0.0, 0.0, 0.5, 1.0))
                                .child(format!(
                                    "确定要删除主机「{}」吗？保存的密码/密钥也会一并删除。",
                                    host_name
                                )),
                        ),
                    )
                })
                .footer({
                    let id = id_del.clone();
                    let panel = panel.clone();
                    Button::new("confirm-ssh-delete")
                        .primary()
                        .small()
                        .label("确认删除")
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            host_store::remove_host(&id);
                            let _ = panel.update(cx, |this, cx| {
                                this.reload(cx);
                            });
                        })
                })
        });
    }

    /// Duplicate a host (clone config + stored secret under a new id).
    ///
    /// Creates a new host with a fresh id and a "副本" name suffix, copies over
    /// any stored password / key passphrase from the encrypted vault, and saves
    /// it. The original host is left untouched.
    fn duplicate_host(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(src) = host_store::find_host(id) else {
            return;
        };
        let new_id = crate::state::models::new_id();
        // Rebuild as a new host record with a fresh id + creation time. We
        // construct via SshHost::new to get a new created_at, then overwrite
        // every field with the source's values except id/name.
        let mut dup = SshHost::new(format!("{} 副本", src.name), src.host.clone());
        dup.id = new_id.clone();
        dup.port = src.port;
        dup.username = src.username.clone();
        dup.auth_type = src.auth_type;
        dup.key_path = src.key_path.clone();
        dup.host_type = src.host_type;
        dup.bastion_id = src.bastion_id.clone();
        dup.bastion_target_override = src.bastion_target_override.clone();
        dup.color = src.color.clone();
        // Port forwards get fresh sub-ids so they don't collide with the source.
        dup.port_forwards = src
            .port_forwards
            .iter()
            .map(|pf| crate::ssh::models::PortForward {
                id: crate::state::models::new_id(),
                direction: pf.direction,
                local_host: pf.local_host.clone(),
                local_port: pf.local_port,
                remote_host: pf.remote_host.clone(),
                remote_port: pf.remote_port,
                auto_start: pf.auto_start,
            })
            .collect();

        // Copy the stored secret (if any) to the new id. The kind depends on
        // the auth type; copy both candidates best-effort so the duplicate is
        // ready regardless of which auth mode it ends up using.
        if let Some(secret) = credentials::load_secret(id, "password") {
            let _ = credentials::store_secret(&new_id, "password", &secret);
        }
        if let Some(secret) = credentials::load_secret(id, "key_passphrase") {
            let _ = credentials::store_secret(&new_id, "key_passphrase", &secret);
        }

        host_store::upsert_host(dup);
        self.reload(cx);
    }

    /// Start a new SSH session.
    ///
    /// `existing_conn`: when `Some`, reuse an already-authenticated connection
    /// (e.g. when duplicating a session) — skips connect + MFA and just opens
    /// a second shell on the shared transport. When `None`, do a full connect.
    fn start_session(
        &mut self,
        host: SshHost,
        existing_conn: Option<SshConnection>,
        cx: &mut Context<Self>,
    ) {
        let host_name = host.name.clone();
        let host_id = host.id.clone();
        let host_id_for_session = host_id.clone();
        // Generate a unique per-session id so concurrent tabs for the same
        // host don't bleed zmodem events into each other.
        use std::time::{SystemTime, UNIX_EPOCH};
        let session_uid = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ rand_session_seed();

        // Force-switch to English input method on macOS when opening a new
        // session; see `switch_to_english_input` for details.
        switch_to_english_input();

        // Create a terminal view with a wider initial size so ls uses multi-column.
        let terminal = cx.new(|cx| TerminalView::new(180, 50, cx));

        // Spawn the SSH connection on a background Tokio task.
        let terminal_for_task = terminal.clone();
        let host_for_task = host.clone();
        // Weak entity reference so we can stash the SshConnection on the session.
        let panel_for_task = cx.entity().downgrade();
        let session_uid_for_task = session_uid;
        // When set, skip connect+MFA and reuse this already-authenticated
        // connection (session duplication). Moved into the spawn closure.
        let reuse_conn_for_task = existing_conn;
        let mut reuse_conn_for_task = reuse_conn_for_task;

        cx.spawn(async move |_this, cx| {
            let mut attempt = 0usize;
            let max_attempts = 5;

            loop {
                attempt += 1;
                let (tx, mut rx) =
                    tokio::sync::mpsc::unbounded_channel::<crate::ssh::client::SshOutput>();

                // For each (re)connect, create a fresh input_tx + resize_tx channel.
                let (input_tx_tx, mut input_tx_rx) = tokio::sync::mpsc::unbounded_channel::<
                    tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
                >();
                let (resize_tx_tx, mut resize_tx_rx) = tokio::sync::mpsc::unbounded_channel::<
                    tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>,
                >();
                // The SshConnection is sent back to the UI once established so
                // the SFTP view can open_sftp() on demand.
                let (conn_tx, mut conn_rx) = tokio::sync::mpsc::unbounded_channel::<
                    crate::ssh::client::SshConnection,
                >();

                // Dedicated channel for MFA/OTP prompts: the SSH connect thread
                // (running on its own OS thread + runtime) sends each
                // keyboard-interactive challenge here and awaits the user's
                // code via a oneshot reply. Created before the connect thread
                // starts so the OtpPromptFn closure can capture the sender.
                let (mfa_prompt_tx, mfa_prompt_rx) =
                    tokio::sync::mpsc::unbounded_channel::<(
                        String,
                        tokio::sync::oneshot::Sender<String>,
                    )>();

                // Build the OtpPromptFn handed to the SSH layer. Each server
                // prompt → a PumpMsg::MfaPrompt on the main pump channel via
                // mfa_prompt_tx; the await on the oneshot blocks the connect
                // thread until the user submits (or cancels by dropping).
                let otp_prompt: crate::ssh::client::OtpPromptFn =
                    std::sync::Arc::new(move |_name: String, prompt: String, _echo: bool| {
                        let mfa_tx = mfa_prompt_tx.clone();
                        async move {
                            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<String>();
                            if mfa_tx.send((prompt, reply_tx)).is_err() {
                                // Pump loop gone (session torn down) → cancelled.
                                return Err(());
                            }
                            reply_rx.await.map_err(|_| ())
                        }
                        .boxed()
                    });

                // Show connecting / reconnecting status.
                let status_msg = if attempt == 1 {
                    None
                } else {
                    Some(format!("\r\n⟳ 正在重连（第 {} 次）…\r\n", attempt))
                };
                if let Some(msg) = status_msg {
                    let _ = terminal_for_task.update(cx, |tv, cx| {
                        tv.feed(msg.as_bytes());
                        cx.notify();
                    });
                }

                let host_connect = host_for_task.clone();
                // Reuse connection only on the first attempt (if provided);
                // retries fall back to a fresh connect (which may re-trigger MFA).
                let reuse_conn = if attempt == 1 { reuse_conn_for_task.take() } else { None };
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                    rt.block_on(async move {
                        // Establish the connection: either reuse an existing
                        // authenticated one (skip connect+MFA) or do a full connect.
                        let conn_result: Result<
                            crate::ssh::client::SshConnection,
                            crate::ssh::client::SshError,
                        > = if let Some(conn) = reuse_conn {
                            log::info!("SSH reuse: skipping connect+MFA (duplicating session)");
                            Ok(conn)
                        } else {
                            crate::ssh::client::SshConnection::connect_with_otp(
                                &host_connect,
                                Some(otp_prompt),
                            )
                            .await
                        };
                        match conn_result {
                            Ok(conn) => {
                                // Open a PTY shell on this connection.
                                match conn.open_shell(180, 50).await {
                                    Ok(mut session) => {
                                        let _ = conn_tx.send(conn);
                                        let _ = input_tx_tx.send(session.input_tx);
                                        let _ = resize_tx_tx.send(session.resize_tx);
                                        while let Some(msg) = session.output_rx.recv().await {
                                            if tx.send(msg).is_err() {
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("SSH open_shell attempt {}: {e}", attempt);
                                        let _ = tx.send(crate::ssh::client::SshOutput::Error {
                                            message: e.message,
                                            retryable: e.retryable,
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("SSH connect attempt {}: {e}", attempt);
                                let _ = tx.send(crate::ssh::client::SshOutput::Error {
                                    message: e.message,
                                    retryable: e.retryable,
                                });
                            }
                        }
                    });
                });

                // Pump output to the terminal — run the select loop on a
                // dedicated OS thread with its own Tokio runtime, since
                // tokio::select! and mpsc::recv() require a Tokio context.
                // Results are forwarded back to GPUI via channels.
                let (pump_tx, mut pump_rx) = tokio::sync::mpsc::unbounded_channel::<PumpMsg>();
                let pump_tx_clone = pump_tx.clone();
                let terminal_clone = terminal_for_task.clone();

                // Channel for UI -> pump thread replies (chosen file paths
                // for rz/sz dialogs). Clones of the sender are sent to the UI
                // inside PumpMsg::ZmPathReplyTx so it can reply back.
                let (path_reply_tx, mut path_reply_rx) =
                    tokio::sync::mpsc::unbounded_channel::<ZmPathReply>();

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                    rt.block_on(async move {
                        use crate::ssh::zmodem::{ZmodemDetector, ZmodemSession};
                        let mut disconnected = false;
                        let mut input_tx: Option<
                            tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
                        > = None;
                        let mut detector = ZmodemDetector::new();
                        // Active zmodem session, if any.
                        let mut zm: Option<ZmodemSession> = None;
                        // After an abort/error, briefly suspend trigger
                        // detection so lrzsz's in-flight retransmit frames
                        // aren't re-parsed as a new session (which would pop
                        // a phantom file dialog). TIME based, not byte based:
                        // a byte-count grace swallows a deliberately
                        // re-run `rz`/`sz`, because the terminal output
                        // between two transfers is far smaller than any sane
                        // byte budget. Clean completions get NO grace — the
                        // closing handshake already drained the wire, so a
                        // following transfer must be detected immediately.
                        let mut zm_grace_until: Option<std::time::Instant> = None;
                        let zm_grace = |cancelled: bool| -> Option<std::time::Instant> {
                            if cancelled {
                                Some(
                                    std::time::Instant::now()
                                        + std::time::Duration::from_millis(1500),
                                )
                            } else {
                                None
                            }
                        };

                        // Helper: drain outgoing bytes + events.
                        let zm_uid = session_uid;
                        let drain_zm = |zm: &mut ZmodemSession,
                                            input_tx: &Option<
                            tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
                        >,
                                            pump_tx: &tokio::sync::mpsc::UnboundedSender<
                            PumpMsg,
                        >,
                                            reply_tx: &tokio::sync::mpsc::UnboundedSender<ZmPathReply>| {
                            let outgoing = zm.drain_outgoing();
                            if !outgoing.is_empty() {
                                if let Some(tx) = input_tx {
                                    let _ = tx.send(outgoing);
                                }
                            }
                            for ev in zm.pop_events() {
                                // Attach a reply sender for prompt events so
                                // the UI can reply with the chosen path.
                                let wrapped = match ev {
                                    crate::ssh::zmodem::ZmEvent::NeedSavePath { suggested_name } => {
                                        PumpMsg::ZmNeedSavePath {
                                            uid: zm_uid,
                                            suggested_name,
                                            reply_tx: reply_tx.clone(),
                                        }
                                    }
                                    crate::ssh::zmodem::ZmEvent::NeedFileToSend => {
                                        PumpMsg::ZmNeedFileToSend {
                                            uid: zm_uid,
                                            reply_tx: reply_tx.clone(),
                                        }
                                    }
                                    other => PumpMsg::ZmEvent {
                                        uid: zm_uid,
                                        event: other,
                                    },
                                };
                                let _ = pump_tx.send(wrapped);
                            }
                        };

                        loop {
                            tokio::select! {
                                Some(conn) = conn_rx.recv() => {
                                    let _ = pump_tx_clone.send(PumpMsg::Connected(conn));
                                }
                                Some(tx) = input_tx_rx.recv() => {
                                    input_tx = Some(tx.clone());
                                    let _ = pump_tx_clone.send(PumpMsg::InputTx(tx));
                                }
                                Some(rtx) = resize_tx_rx.recv() => {
                                    let _ = pump_tx_clone.send(PumpMsg::ResizeTx(rtx));
                                }
                                Some(reply) = path_reply_rx.recv() => {
                                    // UI replied to a path prompt. Feed it
                                    // into the zmodem session.
                                    if let Some(session) = zm.as_mut() {
                                        match reply {
                                            ZmPathReply::Save(path) => session.provide_save_path(path),
                                            ZmPathReply::Send(path) => session.provide_file_to_send(path),
                                        }
                                        session.resume();
                                        drain_zm(session, &input_tx, &pump_tx_clone, &path_reply_tx);
                                        if session.is_done() {
                                            zm_grace_until = zm_grace(session.was_cancelled());
                                            zm = None;
                                            detector.reset();
                                        }
                                    }
                                }
                                msg = rx.recv() => {
                                    let Some(msg) = msg else {
                                        let _ = pump_tx_clone.send(PumpMsg::Disconnected);
                                        break;
                                    };
                                    match msg {
                                        crate::ssh::client::SshOutput::Data(data) => {
                                            if let Some(session) = zm.as_mut() {
                                                session.feed(&data);
                                                drain_zm(session, &input_tx, &pump_tx_clone, &path_reply_tx);
                                                if session.is_done() {
                                                    zm_grace_until = zm_grace(session.was_cancelled());
                                                    zm = None;
                                                    detector.reset();
                                                }
                                            } else {
                                                // Post-abort grace window: keep
                                                // forwarding bytes to the terminal
                                                // (abort messages stay visible) but
                                                // don't scan for a new trigger, so
                                                // in-flight lrzsz retransmits can't
                                                // spawn a phantom session.
                                                let in_grace = zm_grace_until
                                                    .map(|t| std::time::Instant::now() < t)
                                                    .unwrap_or(false);
                                                if in_grace {
                                                    let _ = pump_tx_clone.send(PumpMsg::Data(data));
                                                } else {
                                                    zm_grace_until = None;
                                                    if let Some(_trigger_idx) = detector.feed(&data) {
                                                        let mut session = ZmodemSession::new();
                                                        session.feed(&data);
                                                        drain_zm(&mut session, &input_tx, &pump_tx_clone, &path_reply_tx);
                                                        if !session.is_done() {
                                                            zm = Some(session);
                                                        } else {
                                                            zm_grace_until =
                                                                zm_grace(session.was_cancelled());
                                                            detector.reset();
                                                            // Forward any non-ZMODEM bytes.
                                                            let _ = pump_tx_clone.send(PumpMsg::Data(data));
                                                        }
                                                    } else {
                                                        let _ = pump_tx_clone.send(PumpMsg::Data(data));
                                                    }
                                                }
                                            }
                                        }
                                        crate::ssh::client::SshOutput::Closed(code) => {
                                            let _ = pump_tx_clone.send(PumpMsg::Closed(code));
                                            disconnected = false;
                                            break;
                                        }
                                        crate::ssh::client::SshOutput::Error {
                                            message,
                                            retryable,
                                        } => {
                                            let _ =
                                                pump_tx_clone.send(PumpMsg::Error {
                                                    message,
                                                    retryable,
                                                });
                                            disconnected = true;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        if disconnected {
                            let _ = pump_tx_clone.send(PumpMsg::Disconnected);
                        }
                    });
                });

                // Receive pump messages on GPUI's executor and update the terminal.
                let mut got_disconnected = false;
                // Whether the last error is worth retrying. Authentication /
                // config errors set this false so we stop after one attempt.
                // `Disconnected` (no explicit error) defaults to retryable.
                let mut last_error_retryable = true;
                let session_uid = session_uid_for_task;
                let mut mfa_prompt_rx = mfa_prompt_rx;
                let mfa_forward_tx = pump_tx.clone();
                // Once the OTP channel's senders are all gone (connect thread
                // finished auth), recv() returns None forever. If we kept polling
                // it we'd busy-loop on GPUI's foreground executor and freeze the
                // UI. So we set this flag to false on the first None and replace
                // the branch with a never-ready future thereafter.
                let mut mfa_open = true;
                loop {
                    // Merge MFA prompts (from the SSH connect thread) into the
                    // main pump stream so a single match handles everything.
                    let msg = tokio::select! {
                        m = pump_rx.recv() => match m {
                            Some(m) => m,
                            None => break,
                        },
                        // Only poll the OTP channel while it may still produce a
                        // challenge; once closed, pin a pending() future so this
                        // branch never wakes and we never spin.
                        m = async {
                            if !mfa_open {
                                std::future::pending::<
                                    Option<(String, tokio::sync::oneshot::Sender<String>)>,
                                >().await
                            } else {
                                mfa_prompt_rx.recv().await
                            }
                        } => match m {
                            Some((prompt_text, reply_tx)) => {
                                // Forward as a pump message; if the pump is
                                // closed the send fails and the reply_tx drops,
                                // which cancels the OTP on the connect thread.
                                let _ = mfa_forward_tx.send(PumpMsg::MfaPrompt {
                                    prompt_text,
                                    reply_tx,
                                });
                                continue;
                            }
                            None => {
                                // All OTP senders gone (auth finished). Disable
                                // this branch permanently so we don't spin.
                                mfa_open = false;
                                continue;
                            }
                        },
                    };
                    match msg {
                        PumpMsg::Connected(conn) => {
                            // Stash the SshConnection on the matching session
                            // immediately so the Files-view toggle becomes
                            // usable and can open_sftp() on demand.
                            if let Some(panel) = panel_for_task.upgrade() {
                                let uid = session_uid;
                                let _ = panel.update(cx, move |this, cx| {
                                    if let Some(s) = this
                                        .sessions
                                        .iter_mut()
                                        .find(|s| s.session_uid == uid)
                                    {
                                        s.connection = Some(conn);
                                        s.status = SessionStatus::Connected;
                                        cx.notify();
                                    }
                                });
                            }
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.status_connected();
                                cx.notify();
                            });
                        }
                        PumpMsg::Data(data) => {
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.feed(&data);
                                cx.notify();
                            });
                        }
                        PumpMsg::InputTx(tx) => {
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.set_input_tx(tx);
                                tv.status_connected();
                                // A live input channel means we are (re)connected;
                                // clear any stale "press any key to reconnect".
                                tv.disarm_reconnect_trigger();
                                cx.notify();
                            });
                        }
                        PumpMsg::ResizeTx(rtx) => {
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.set_resize_tx(rtx);
                                tv.send_window_change(tv.cols, tv.rows);
                                cx.notify();
                            });
                        }
                        PumpMsg::Closed(code) => {
                            // Some(exit_status) = the remote shell ran `exit` and
                            // reported its exit code: a deliberate logout, leave it
                            // closed. None = the channel dropped without an exit status
                            // (network loss / host reboot / keepalive timeout): treat
                            // as an abnormal disconnect and auto-reconnect, so the user
                            // sees it instead of a dead prompt swallowing keystrokes.
                            match code {
                                Some(exit_status) => {
                                    let m = format!(
                                        "\r\n[会话结束，退出码: {}]\r\n",
                                        exit_status
                                    );
                                    let _ = terminal_clone.update(cx, |tv, cx| {
                                        tv.feed(m.as_bytes());
                                        cx.notify();
                                    });
                                    if let Some(panel) = panel_for_task.upgrade() {
                                        let uid = session_uid;
                                        let _ = panel.update(cx, move |this, cx| {
                                            if let Some(s) = this.sessions.iter_mut().find(
                                                |s| s.session_uid == uid,
                                            ) {
                                                s.status = SessionStatus::Closed;
                                                cx.notify();
                                            }
                                        });
                                    }
                                    got_disconnected = false;
                                }
                                None => {
                                    let m = "\r\n✕ 连接已断开，正在尝试重连…\r\n";
                                    let _ = terminal_clone.update(cx, |tv, cx| {
                                        tv.feed(m.as_bytes());
                                        cx.notify();
                                    });
                                    if let Some(panel) = panel_for_task.upgrade() {
                                        let uid = session_uid;
                                        let _ = panel.update(cx, move |this, cx| {
                                            if let Some(s) = this.sessions.iter_mut().find(
                                                |s| s.session_uid == uid,
                                            ) {
                                                s.status = SessionStatus::Reconnecting(1);
                                                cx.notify();
                                            }
                                        });
                                    }
                                    last_error_retryable = true;
                                    got_disconnected = true;
                                }
                            }
                            break;
                        }
                        PumpMsg::Error { message, retryable } => {
                            log::warn!(
                                "SSH session error (attempt {}, retryable={}): {}",
                                attempt,
                                retryable,
                                message
                            );
                            // Surface the full, specific reason to the user.
                            let m = if retryable {
                                format!("\r\n✕ 连接错误：{message}\r\n")
                            } else {
                                // Non-retryable: make it crystal clear this is a
                                // configuration problem and we won't retry.
                                format!(
                                    "\r\n✕ 连接错误：{message}\r\n（已停止重试，请修改连接配置后重试）\r\n"
                                )
                            };
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.feed(m.as_bytes());
                                cx.notify();
                            });
                            // Reflect the failure in the status bar.
                            let short = ssh_error_short_label(&message);
                            if let Some(panel) = panel_for_task.upgrade() {
                                let uid = session_uid;
                                let _ = panel.update(cx, move |this, cx| {
                                    if let Some(s) = this
                                        .sessions
                                        .iter_mut()
                                        .find(|s| s.session_uid == uid)
                                    {
                                        s.status = SessionStatus::Error(short);
                                        cx.notify();
                                    }
                                });
                            }
                            last_error_retryable = retryable;
                            got_disconnected = true;
                            break;
                        }
                        PumpMsg::Disconnected => {
                            // Generic disconnect with no explicit error — treat
                            // as transient and retry.
                            last_error_retryable = true;
                            got_disconnected = true;
                            break;
                        }
                        PumpMsg::ZmEvent { uid, event } => {
                            if let Some(panel) = panel_for_task.upgrade() {
                                let _ = panel.update(cx, move |this, cx| {
                                    this.handle_zm_event(uid, event, cx);
                                });
                            }
                        }
                        PumpMsg::ZmNeedSavePath {
                            uid: _uid,
                            suggested_name,
                            reply_tx,
                        } => {
                            // Ask the user where to save. We run the dialog on
                            // a dedicated thread (rfd requires a platform event
                            // loop) and send the reply through the channel.
                            let tx = reply_tx.clone();
                            cx.background_executor().spawn(async move {
                                let (tx2, rx2) = std::sync::mpsc::channel::<Option<PathBuf>>();
                                std::thread::spawn(move || {
                                    let res = rfd::FileDialog::new()
                                        .set_file_name(&suggested_name)
                                        .save_file();
                                    let _ = tx2.send(res.map(|p| p.to_path_buf()));
                                });
                                let chosen = rx2.recv().ok().flatten();
                                let _ = tx.send(ZmPathReply::Save(chosen));
                            }).detach();
                        }
                        PumpMsg::ZmNeedFileToSend {
                            uid: _uid,
                            reply_tx,
                        } => {
                            // Native open-file dialog via rfd.
                            let tx = reply_tx.clone();
                            cx.background_executor().spawn(async move {
                                let (tx2, rx2) = std::sync::mpsc::channel::<Option<PathBuf>>();
                                std::thread::spawn(move || {
                                    let res = rfd::FileDialog::new().pick_file();
                                    let _ = tx2.send(res.map(|p| p.to_path_buf()));
                                });
                                let chosen = rx2.recv().ok().flatten();
                                let _ = tx.send(ZmPathReply::Send(chosen));
                            }).detach();
                        }
                        PumpMsg::MfaPrompt {
                            prompt_text,
                            reply_tx,
                        } => {
                            // The SSH server sent a keyboard-interactive
                            // challenge. Stash it on the session; `render`
                            // (which has window access) builds the InputState.
                            if let Some(panel) = panel_for_task.upgrade() {
                                let uid = session_uid;
                                let _ = panel.update(cx, move |this, cx| {
                                    if let Some(s) = this
                                        .sessions
                                        .iter_mut()
                                        .find(|s| s.session_uid == uid)
                                    {
                                        // Replace any prior pending prompt (its
                                        // reply_tx is dropped → connect thread
                                        // sees cancel and surfaces an error).
                                        s.pending_mfa_prompt =
                                            Some((prompt_text, reply_tx));
                                        cx.notify();
                                    }
                                });
                            } else {
                                // Panel dropped — cancel the prompt.
                                drop(reply_tx);
                            }
                        }
                    }
                }

                if !got_disconnected {
                    break;
                }

                // Non-retryable error (bad credentials / wrong port / host-key
                // mismatch): retrying won't help, so stop now. The error status
                // was already set on the session in the PumpMsg::Error branch.
                if !last_error_retryable {
                    break;
                }

                if attempt >= max_attempts {
                    // Auto-reconnect exhausted. Dead-ending here left every
                    // keystroke falling into the dead channel (no response).
                    // Instead, show a clear prompt and park until the user
                    // presses any key, then start a fresh round of attempts.
                    let msg = format!(
                        "\r\n✕ 重连失败（已尝试 {max_attempts} 次）。\r\n\r\n  已断开连接，按任意键重连…\r\n"
                    );
                    let _ = terminal_for_task.update(cx, |tv, cx| {
                        tv.feed(msg.as_bytes());
                        cx.notify();
                    });
                    // Mark the session as failed in the status bar.
                    if let Some(panel) = panel_for_task.upgrade() {
                        let uid = session_uid;
                        let _ = panel.update(cx, move |this, cx| {
                            if let Some(s) = this
                                .sessions
                                .iter_mut()
                                .find(|s| s.session_uid == uid)
                            {
                                s.status = SessionStatus::Error("连接失败".to_string());
                                cx.notify();
                            }
                        });
                    }

                    // Arm the "press any key to reconnect" handshake. The
                    // terminal fires this oneshot on the next regular keystroke.
                    let (reconnect_tx, reconnect_rx) =
                        tokio::sync::oneshot::channel::<()>();
                    let _ = terminal_for_task.update(cx, |tv, _cx| {
                        tv.arm_reconnect_trigger(reconnect_tx);
                    });
                    match reconnect_rx.await {
                        Ok(()) => {
                            // User pressed a key → reset the attempt counter so
                            // the loop runs a full fresh round of retries.
                            log::info!(
                                "SSH reconnect triggered by keypress for uid {}",
                                session_uid
                            );
                            attempt = 0;
                            continue;
                        }
                        Err(_) => {
                            // Sender dropped (session closed / disarmed).
                            break;
                        }
                    }
                }

                // Reflect the reconnecting attempt in the status bar.
                if let Some(panel) = panel_for_task.upgrade() {
                    let uid = session_uid;
                    let next_attempt = attempt + 1;
                    let _ = panel.update(cx, move |this, cx| {
                        if let Some(s) = this
                            .sessions
                            .iter_mut()
                            .find(|s| s.session_uid == uid)
                        {
                            s.status = SessionStatus::Reconnecting(next_attempt);
                            cx.notify();
                        }
                    });
                }

                // Exponential backoff: 1s, 2s, 4s, 8s, 16s.
                let delay = std::time::Duration::from_secs(1 << (attempt - 1).min(4));
                let _ = terminal_for_task.update(cx, |tv, cx| {
                    tv.feed(format!("\r\n⏳ {} 秒后重连…\r\n", delay.as_secs()).as_bytes());
                    cx.notify();
                });
                // Use smol::Timer (GPUI's executor) instead of tokio::time::sleep
                // — we're on GPUI's async executor, NOT a Tokio runtime.
                cx.background_executor().timer(delay).await;
            }
        })
        .detach();

        // Add the session tab.
        self.sessions.push(TerminalSession {
            host_name,
            host_id: host_id_for_session,
            session_uid,
            terminal,
            sftp_browser: None,
            view: SessionView::Terminal,
            connection: None,
            input_tx: None,
            resize_tx: None,
            status: SessionStatus::Connecting,
            zm_state: ZmUiState::default(),
            active_forwards: Vec::new(),
            mfa_input: None,
            mfa_prompt: String::new(),
            mfa_reply_tx: None,
            pending_mfa_prompt: None,
            local: None,
        });
        self.active_tab = self.sessions.len() - 1;
        // Connecting from the host-list pseudo-tab lands on the new session.
        self.show_host_grid = false;
        cx.notify();
    }

    /// Start a "local terminal" session: the user's default shell in a local
    /// pseudo-terminal, reusing the same TerminalView + channel wiring as the
    /// remote SSH sessions. This is the Terminal.app-replacement mode.
    pub fn start_local_session(&mut self, cx: &mut Context<Self>) {
        // Same as SSH sessions: open with English input so keystrokes go
        // straight to the shell (user can toggle back afterwards).
        switch_to_english_input();

        // Same uid scheme as start_session (nanos ^ monotonic counter).
        use std::time::{SystemTime, UNIX_EPOCH};
        let session_uid = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ rand_session_seed();

        let terminal = cx.new(|cx| TerminalView::new(180, 50, cx));

        match crate::ssh::local_pty::spawn_local_pty(180, 50) {
            Ok((pty, output_rx)) => {
                let shell_name = pty.shell_name();
                terminal.update(cx, |tv, _| {
                    tv.set_input_tx(pty.input_tx.clone());
                    tv.set_resize_tx(pty.resize_tx.clone());
                });

                // PTY output → terminal view. Same pattern as the Docker exec
                // sessions: the tokio channel is awaited on the GPUI executor.
                // Status updates locate the tab by session_uid so closing
                // neighboring tabs can't misroute them.
                let mut output_rx = output_rx;
                let term_weak = terminal.downgrade();
                let panel = cx.entity().downgrade();
                let uid_for_task = session_uid;
                let shell_for_task = shell_name.clone();
                cx.spawn(async move |_this, cx| {
                    use crate::ssh::local_pty::LocalPtyEvent;
                    while let Some(ev) = output_rx.recv().await {
                        match ev {
                            LocalPtyEvent::Data(data) => {
                                let _ = term_weak.update(cx, |tv, cx| {
                                    tv.feed(&data);
                                    cx.notify();
                                });
                            }
                            LocalPtyEvent::Stall => {
                                // Shell is alive but produced nothing at all
                                // (hung profile, broken std handles, …).
                                let _ = term_weak.update(cx, |tv, cx| {
                                    tv.feed(
                                        format!(
                                            "\r\n\x1b[33m⚠ 本地 shell({shell_for_task})启动后 \
                                             5 秒无输出，可关闭此标签页重试\x1b[0m\r\n"
                                        )
                                        .as_bytes(),
                                    );
                                    cx.notify();
                                });
                            }
                            LocalPtyEvent::Closed => {
                                let _ = term_weak.update(cx, |tv, cx| {
                                    tv.feed("\r\n\x1b[90m○ 本地 shell 已退出\x1b[0m\r\n".as_bytes());
                                    cx.notify();
                                });
                                let _ = panel.update(cx, move |this, cx| {
                                    if let Some(s) = this
                                        .sessions
                                        .iter_mut()
                                        .find(|s| s.session_uid == uid_for_task)
                                    {
                                        s.status = SessionStatus::Closed;
                                        cx.notify();
                                    }
                                });
                                break;
                            }
                        }
                    }
                })
                .detach();

                self.sessions.push(TerminalSession {
                    host_name: format!("本地 {shell_name}"),
                    host_id: "local".to_string(),
                    session_uid,
                    terminal,
                    sftp_browser: None,
                    view: SessionView::Terminal,
                    connection: None,
                    input_tx: Some(pty.input_tx.clone()),
                    resize_tx: Some(pty.resize_tx.clone()),
                    status: SessionStatus::Connected,
                    zm_state: ZmUiState::default(),
                    active_forwards: Vec::new(),
                    mfa_input: None,
                    mfa_prompt: String::new(),
                    mfa_reply_tx: None,
                    pending_mfa_prompt: None,
                    local: Some(pty),
                });
            }
            Err(err) => {
                // Spawn failure: still create the tab so the error is visible
                // in the terminal area + status bar (mirrors the SSH error UX).
                let msg = format!("\r\n\x1b[31m✕ {err:#}\x1b[0m\r\n");
                terminal.update(cx, |tv, cx| {
                    tv.feed(msg.as_bytes());
                    cx.notify();
                });
                self.sessions.push(TerminalSession {
                    host_name: "本地终端".to_string(),
                    host_id: "local".to_string(),
                    session_uid,
                    terminal,
                    sftp_browser: None,
                    view: SessionView::Terminal,
                    connection: None,
                    input_tx: None,
                    resize_tx: None,
                    status: SessionStatus::Error(format!("{err:#}")),
                    zm_state: ZmUiState::default(),
                    active_forwards: Vec::new(),
                    mfa_input: None,
                    mfa_prompt: String::new(),
                    mfa_reply_tx: None,
                    pending_mfa_prompt: None,
                    local: None,
                });
            }
        }
        self.active_tab = self.sessions.len() - 1;
        // Connecting from the host-list pseudo-tab lands on the new session.
        self.show_host_grid = false;
        cx.notify();
    }

    /// Render the multi-tab terminal/SFTP view.
    fn render_terminal_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        theme: gpui_component::Theme,
    ) -> impl IntoElement {
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let accent = theme.accent;
        let active = self.active_tab;

        let active_view = self
            .sessions
            .get(active)
            .map(|s| s.view)
            .unwrap_or(SessionView::Terminal);

        // Lazily initialize the SFTP browser if the active tab is in Sftp view
        // and the browser hasn't been created yet.
        self.ensure_active_sftp_browser(cx);

        // Materialize the OTP input for any pending MFA challenge on the active
        // session. InputState construction needs a Window, which render has —
        // the spawn loop can only stash the prompt here.
        self.ensure_mfa_input(window, cx);

        // Resolve the content element based on view mode. When the host-list
        // pseudo-tab is active, show the full host card grid instead of the
        // active session; sessions keep running in the background.
        let show_grid = self.show_host_grid;
        let content: AnyElement = if show_grid {
            div()
                .flex_1()
                .min_h_0()
                .id("ssh-host-grid-scroll")
                .overflow_y_scroll()
                .p_4()
                .child(self.render_card_grid(cx, theme.clone()))
                .into_any_element()
        } else if active_view == SessionView::Sftp {
            self.sessions
                .get(active)
                .and_then(|s| s.sftp_browser.clone())
                .map(|b| b.into_any_element())
                .unwrap_or_else(|| {
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_4()
                        .text_color(muted)
                        .text_sm()
                        .child("正在打开 SFTP…")
                        .into_any_element()
                })
        } else if active_view == SessionView::Forwards {
            self.render_forwards_view(window, &theme, active, cx)
                .into_any_element()
        } else {
            self.sessions
                .get(active)
                .map(|s| s.terminal.clone().into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        };

        let active_status = self
            .sessions
            .get(active)
            .map(|s| s.status.clone())
            .unwrap_or(SessionStatus::Closed);

        // Whether the active tab has a connection ready (for enabling Files button).
        let active_has_conn = self
            .sessions
            .get(active)
            .map(|s| s.connection.is_some())
            .unwrap_or(false);

        // Local-terminal tabs have no SSH transport: SFTP/forwards N/A.
        let active_is_local = self
            .sessions
            .get(active)
            .map(|s| s.is_local())
            .unwrap_or(false);

        let active_zm_state = self
            .sessions
            .get(active)
            .map(|s| s.zm_state.clone())
            .unwrap_or_default();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(bg)
            .text_color(fg)
            // Hidden 0-size canvas: captures this container's window-absolute
            // bounds each render so the context-menu overlay can convert
            // window-absolute mouse coords into panel-relative coords.
            .child({
                let bounds = self.panel_bounds.clone();
                gpui::canvas(
                    move |b, _window, _cx| b,
                    move |b, _prev, _window, _cx| {
                        if let Ok(mut g) = bounds.lock() {
                            *g = b;
                        }
                    },
                )
                .absolute()
                .size(px(0.))
            })
            // Tab bar — sits at the very top, SAME LINE as the macOS
            // traffic-light buttons. Left padding (78px) clears them.
            .child(
                h_flex()
                    .h(px(38.))
                    .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
                    .when(!cfg!(target_os = "macos"), |this| this.pl_2())
                    .pr_2()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(border)
                    .bg(theme.muted)
                    // Fixed "host list" pseudo-tab — switches the content area
                    // to the full host card grid without touching sessions.
                    .child(
                        h_flex()
                            .id("ssh-hosts-tab")
                            .px_3()
                            .h(px(28.))
                            .items_center()
                            .gap_1()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .when(show_grid, |d| d.bg(accent.opacity(0.5)).text_color(fg))
                            .when(!show_grid, |d| {
                                d.text_color(muted).hover(|s| s.bg(theme.muted))
                            })
                            .child(Icon::new(IconName::LayoutDashboard).size_3())
                            .child(div().text_sm().child("主机"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_host_grid = true;
                                cx.notify();
                            })),
                    )
                    .children(self.sessions.iter().enumerate().map(|(i, s)| {
                        let is_active = i == active;
                        let host_name = s.host_name.clone();
                        let tc = theme.clone();
                        h_flex()
                            .id(("ssh-tab", i))
                            .px_3()
                            .h(px(28.))
                            .items_center()
                            .gap_2()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .when(is_active, |d| {
                                d.bg(tc.accent.opacity(0.5)).text_color(tc.foreground)
                            })
                            .when(!is_active, |d| {
                                d.text_color(tc.muted_foreground).hover(|s| s.bg(tc.muted))
                            })
                            .child(div().text_sm().child(host_name.clone()))
                            .child(
                                gpui_component::button::Button::new(("ssh-tab-close", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.close_session(i, cx);
                                    })),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_tab = i;
                                // Leaving the host-list pseudo-tab: show this
                                // session's terminal again.
                                this.show_host_grid = false;
                                cx.notify();
                            }))
                            // Right-click → open the tab context menu (anchored
                            // at the cursor). Bounds-checked (AGENTS.md §五):
                            // the tab list may have changed between renders.
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                                    if i < this.sessions.len() {
                                        log::info!("tab right-click on idx {} at {:?}", i, ev.position);
                                        this.tab_context_menu = Some((i, ev.position));
                                        cx.notify();
                                    }
                                }),
                            )
                    }))
                    // "+" button to open host list and start a new connection.
                    .child(
                        gpui_component::button::Button::new("ssh-new-conn")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("新建连接")
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.show_host_picker = true;
                                // Clear any stale search so the full list shows.
                                if let Some(s) = this.host_picker_search.as_ref() {
                                    s.update(cx, |state, cx| state.set_value("", window, cx));
                                }
                                // Stop the terminal from stealing focus back from
                                // the search box on every render frame.
                                if let Some(s) = this.sessions.get(this.active_tab) {
                                    s.terminal.update(cx, |tv, cx| {
                                        tv.suppress_auto_focus = true;
                                        cx.notify();
                                    });
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    // View toggle: Terminal / Files — session-specific, so
                    // hidden while the host-list pseudo-tab is shown.
                    .when(!show_grid, |bar| {
                        bar.child(
                            h_flex()
                                .gap_1()
                                .pr_2()
                                .child(
                                    Button::new("ssh-view-terminal")
                                        .ghost()
                                        .xsmall()
                                        .label("终端")
                                        .when(active_view == SessionView::Terminal, |b| b.primary())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(s) = this.sessions.get_mut(this.active_tab)
                                            {
                                                s.view = SessionView::Terminal;
                                            }
                                            cx.notify();
                                        })),
                                )
                                // Files/Forwards need the SSH transport — hidden
                                // for local-terminal tabs.
                                .when(!active_is_local, |row| {
                                    row.child({
                                        let can_open = active_has_conn;
                                        Button::new("ssh-view-files")
                                            .ghost()
                                            .xsmall()
                                            .label("文件")
                                            .when(active_view == SessionView::Sftp, |b| b.primary())
                                            .tooltip(if can_open {
                                                "文件浏览器（SFTP）"
                                            } else {
                                                "等待连接建立…"
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if !can_open {
                                                    return;
                                                }
                                                if let Some(s) =
                                                    this.sessions.get_mut(this.active_tab)
                                                {
                                                    s.view = SessionView::Sftp;
                                                }
                                                cx.notify();
                                            }))
                                    })
                                    .child({
                                        let can_open = active_has_conn;
                                        let active_count = self
                                            .sessions
                                            .get(active)
                                            .map(|s| s.active_forwards.len())
                                            .unwrap_or(0);
                                        let label = if active_count > 0 {
                                            format!("转发 ({active_count})")
                                        } else {
                                            "端口转发".into()
                                        };
                                        Button::new("ssh-view-forwards")
                                            .ghost()
                                            .xsmall()
                                            .label(label)
                                            .when(active_view == SessionView::Forwards, |b| {
                                                b.primary()
                                            })
                                            .tooltip(if can_open {
                                                "本地端口转发：将远端服务映射到本地端口"
                                            } else {
                                                "等待连接建立…"
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if !can_open {
                                                    return;
                                                }
                                                if let Some(s) =
                                                    this.sessions.get_mut(this.active_tab)
                                                {
                                                    s.view = SessionView::Forwards;
                                                }
                                                cx.notify();
                                            }))
                                    })
                                }),
                        )
                    }),
            )
            // MFA / OTP prompt bar — shown when the active session is awaiting
            // a keyboard-interactive verification code. Hidden while the
            // host-list pseudo-tab is shown (it belongs to the session).
            .when(!show_grid, |col| {
                col.when_some(self.render_mfa_bar(active, &theme, cx), |col, bar| {
                    col.child(bar)
                })
            })
            // Content area.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("ssh-content-area")
                    .overflow_hidden()
                    .child(content),
            )
            // ZMODEM progress bar: visible during an active transfer, and
            // briefly after success/error. Cancelled transfers don't show.
            // Hidden while the host-list pseudo-tab is shown.
            .when(
                !show_grid
                    && (active_zm_state.active
                        || (active_zm_state.done && !active_zm_state.cancelled)
                        || active_zm_state.error.is_some()),
                |c| {
                    let zm = &active_zm_state;
                    let pct = if zm.total == 0 {
                        0.0
                    } else {
                        (zm.transferred as f64 / zm.total as f64).clamp(0.0, 1.0)
                    };
                    let bar_w = 220.0 * pct;
                    let arrow = match zm.role {
                        crate::ssh::zmodem::ZmRole::SendToRemote => "↑",
                        crate::ssh::zmodem::ZmRole::ReceiveFromRemote => "↓",
                    };
                    let label = if let Some(err) = &zm.error {
                        format!("{arrow} rz/sz 错误: {err}")
                    } else if zm.done {
                        format!("{arrow} {} 传输完成", zm.filename)
                    } else {
                        format!(
                            "{arrow} {}  {} / {}",
                            zm.filename,
                            crate::ssh::sftp::humanize_size(zm.transferred),
                            crate::ssh::sftp::humanize_size(zm.total),
                        )
                    };
                    c.child(
                        h_flex()
                            .h(px(22.))
                            .px_3()
                            .gap_2()
                            .items_center()
                            .border_t_1()
                            .border_color(border)
                            .bg(theme.muted)
                            .child(div().text_xs().child(label))
                            .when(!zm.done && zm.error.is_none(), |row| {
                                row.child(
                                    div()
                                        .w(px(220.))
                                        .h(px(6.))
                                        .rounded(px(3.))
                                        .bg(border)
                                        .child(
                                            div()
                                                .w(px(bar_w as f32))
                                                .h_full()
                                                .rounded(px(3.))
                                                .bg(accent),
                                        ),
                                )
                            }),
                    )
                },
            )
            // Status bar — session-specific, hidden while the host-list
            // pseudo-tab is shown.
            .when(!show_grid, |col| {
                col.child(
                    h_flex()
                        .h(px(24.))
                        .px_3()
                        .items_center()
                        .border_t_1()
                        .border_color(border)
                        .bg(theme.muted)
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .text_color(match &active_status {
                                    SessionStatus::Connected => gpui::hsla(0.33, 0.6, 0.4, 1.0),
                                    SessionStatus::Connecting => accent,
                                    SessionStatus::Reconnecting(_) => accent,
                                    SessionStatus::Error(_) => theme.danger,
                                    SessionStatus::Closed => muted,
                                })
                                .child(match &active_status {
                                    SessionStatus::Connected => "● 已连接".to_string(),
                                    SessionStatus::Connecting => "○ 连接中…".to_string(),
                                    SessionStatus::Reconnecting(n) => {
                                        format!("⟳ 重连中（第 {} 次）", n)
                                    }
                                    SessionStatus::Error(e) => format!("✕ {e}"),
                                    SessionStatus::Closed => "○ 已断开".to_string(),
                                }),
                        )
                        .child(div().flex_1()),
                )
            })
            // Tab right-click context menu — MUST be the last child so it paints
            // on top of the content/status bars (later siblings paint later in
            // GPUI's flex layout).
            .when_some(self.tab_context_menu, |col, (idx, pos)| {
                col.child(self.render_tab_context_menu(idx, pos, &theme, cx))
            })
    }

    /// Lazily create + bind the SFTP browser for the active session when the
    /// user switches to Files view.
    fn ensure_active_sftp_browser(&mut self, cx: &mut Context<Self>) {
        let idx = self.active_tab;
        let Some(session) = self.sessions.get_mut(idx) else {
            return;
        };
        if session.view != SessionView::Sftp {
            return;
        }
        if session.sftp_browser.is_some() {
            return;
        }
        // Need a live connection to open SFTP.
        let Some(conn) = session.connection.clone() else {
            return;
        };
        let host_name = session.host_name.clone();
        // Create the browser entity with a placeholder path. bind_sftp_browser
        // will open SFTP, canonicalize "~", spawn the actor, and kick off listing.
        let browser = cx.new(|cx| {
            let mut b = SftpBrowser::new(host_name, "~".to_string(), cx);
            crate::ui::sftp_browser::bind_sftp_browser(&mut b, conn, cx);
            b
        });
        session.sftp_browser = Some(browser);

        cx.notify();
    }

    /// Materialize the OTP input entity for the active session's pending MFA
    /// challenge, if any. Must run in `render` because `InputState::new`
    /// requires a `&mut Window` (the spawn loop has none).
    fn ensure_mfa_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.active_tab;
        let Some(session) = self.sessions.get_mut(idx) else {
            return;
        };
        // Take the pending prompt out first to avoid a double borrow below.
        let Some((prompt_text, reply_tx)) = session.pending_mfa_prompt.take() else {
            return;
        };
        // Replace any prior pending prompt — dropping its reply_tx cancels that
        // challenge on the connect thread (which surfaces an error).
        let mfa_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("验证码")
                .masked(true)
        });
        // Grab focus so the user can type the verification code immediately
        // without an extra click.
        mfa_input.update(cx, |state, cx| {
            state.focus(window, cx);
        });
        // Stop the terminal from stealing focus back on every render while the
        // OTP input is up.
        session.terminal.update(cx, |tv, cx| {
            tv.suppress_auto_focus = true;
            cx.notify();
        });
        // Submit the OTP when the user presses Enter.
        // NOTE: Context::subscribe's closure ALREADY receives &mut SshPanel
        // and &mut Context<SshPanel> directly (the framework wraps it in an
        // entity update internally). We must NOT call panel.update(cx, ..)
        // here — that would re-lease the entity mid-update and panic
        // ("cannot update SshPanel while it is already being updated").
        cx.subscribe(&mfa_input, move |this, _input, event, cx| {
            if let gpui_component::input::InputEvent::PressEnter { .. } = event {
                // idx is captured by value; the tab list may have shifted
                // between setting up the prompt and the user pressing Enter,
                // so re-validate and fall back to active_tab.
                let target = if this.sessions.get(idx).is_some() {
                    idx
                } else {
                    this.active_tab
                };
                this.submit_mfa(target, cx);
            }
        })
        .detach();
        session.mfa_input = Some(mfa_input);
        session.mfa_prompt = prompt_text;
        session.mfa_reply_tx = Some(reply_tx);
        cx.notify();
    }

    /// Build the OTP prompt bar element for the active session, or `None` if
    /// no challenge is pending.
    fn render_mfa_bar(
        &mut self,
        idx: usize,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let Some(session) = self.sessions.get(idx) else {
            return None;
        };
        let mfa_input = session.mfa_input.clone()?;
        let prompt_text = session.mfa_prompt.clone();
        let prompt_display: String = if prompt_text.trim().is_empty() {
            "请输入验证码 (MFA)".to_string()
        } else {
            prompt_text.trim().to_string()
        };

        let bg = theme.muted;
        let border = theme.border;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let panel = cx.entity().downgrade();

        Some(
            h_flex()
                .w_full()
                .h(px(44.))
                .px_4()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(border)
                .bg(bg)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        // Prompt text is server-supplied (untrusted); cap its
                        // length by char count and elide to avoid layout blowup.
                        .child(truncate_prompt(&prompt_display, 40)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&mfa_input).small().appearance(true)),
                )
                .child(
                    Button::new("mfa-submit")
                        .primary()
                        .small()
                        .label("提交")
                        .on_click({
                            let panel = panel.clone();
                            move |_, _window, cx: &mut App| {
                                let _ = panel.update(cx, |this, cx| {
                                    this.submit_mfa(idx, cx);
                                });
                            }
                        }),
                )
                .child(
                    Button::new("mfa-cancel")
                        .ghost()
                        .small()
                        .label("取消")
                        .text_color(danger.opacity(0.9))
                        .on_click(move |_, _window, cx: &mut App| {
                            let _ = panel.update(cx, |this, cx| {
                                this.cancel_mfa(idx, cx);
                            });
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("一次性验证码不保存"),
                ),
        )
    }

    /// Submit the user-entered OTP for the active session's pending challenge.
    fn submit_mfa(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get_mut(idx) else {
            return;
        };
        let mfa_input = match session.mfa_input.clone() {
            Some(e) => e,
            None => return,
        };
        let code = mfa_input.read(cx).value().trim().to_string();
        if code.is_empty() {
            return;
        }
        log::info!(
            "MFA: submitting OTP (len={}) for session idx {} (uid {})",
            code.len(),
            idx,
            session.session_uid
        );
        let reply_tx = session.mfa_reply_tx.take();
        session.mfa_input = None;
        session.mfa_prompt.clear();
        if let Some(tx) = reply_tx {
            let _ = tx.send(code);
        }
        // Re-render so the OTP bar disappears immediately.
        cx.notify();
        // Defer handing focus back to the terminal + resetting
        // suppress_auto_focus to the next run-loop tick. Doing it inline inside
        // the subscribe callback (which the framework runs within an SshPanel
        // update) re-entered the terminal entity's update during the same
        // dispatch and froze the UI on Enter-submit. cx.spawn runs on GPUI's
        // async executor, breaking the re-entry.
        let terminal = session.terminal.clone();
        cx.spawn(async move |_this, cx| {
            let _ = terminal.update(cx, |tv, cx| {
                tv.suppress_auto_focus = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Cancel the active session's MFA challenge (drops reply_tx → connect
    /// thread observes cancellation and surfaces an auth error).
    fn cancel_mfa(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get_mut(idx) else {
            return;
        };
        session.mfa_input = None;
        session.mfa_prompt.clear();
        session.mfa_reply_tx.take(); // dropped here → cancel
        cx.notify();
        // Defer the focus hand-back (same re-entry concern as submit_mfa).
        let terminal = session.terminal.clone();
        cx.spawn(async move |_this, cx| {
            let _ = terminal.update(cx, |tv, cx| {
                tv.suppress_auto_focus = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Render the floating right-click context menu for a session tab.
    ///
    /// A full-size transparent backdrop closes the menu on any left-click;
    /// the inner menu panel is anchored at the click position and stops
    /// propagation so clicks on its items don't dismiss it prematurely.
    fn render_tab_context_menu(
        &mut self,
        idx: usize,
        pos: gpui::Point<gpui::Pixels>,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        log::info!(
            "render_tab_context_menu: idx={} pos={:?} origin={:?}",
            idx, pos, self.panel_bounds.lock().map(|g| g.origin).unwrap_or_default()
        );
        let bg = theme.background;
        let border = theme.border;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let accent = theme.accent;

        // Build three menu rows. Each is a full-width button; the action runs
        // in a cx.listener (has &mut self + &mut Context<Self>).
        let menu_rows = v_flex()
            // 复制当前会话
            .child(
                h_flex()
                    .id("ctx-duplicate")
                    .w_full()
                    .h(px(32.))
                    .px_3()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(fg)
                    .child(Icon::new(IconName::Copy).size_3())
                    .child(div().text_sm().child("复制当前会话"))
                    .hover(|s| s.bg(accent.opacity(0.4)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.duplicate_session(idx, cx);
                    })),
            )
            // 关闭当前标签页
            .child(
                h_flex()
                    .id("ctx-close")
                    .w_full()
                    .h(px(32.))
                    .px_3()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(fg)
                    .child(Icon::new(IconName::Close).size_3())
                    .child(div().text_sm().child("关闭当前标签页"))
                    .hover(|s| s.bg(accent.opacity(0.4)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.tab_context_menu = None;
                        this.close_session(idx, cx);
                    })),
            )
            // 关闭其他标签页
            .child(
                h_flex()
                    .id("ctx-close-others")
                    .w_full()
                    .h(px(32.))
                    .px_3()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(muted)
                    .child(Icon::new(IconName::Close).size_3())
                    .child(div().text_sm().child("关闭其他标签页"))
                    .hover(|s| s.bg(danger.opacity(0.18)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.close_other_sessions(idx, cx);
                    })),
            );

        // Convert the window-absolute click position into panel-relative
        // coordinates: GPUI's `absolute` is positioned relative to this
        // container's content box, but `pos` (from MouseDownEvent) is
        // window-absolute. Subtract the panel's captured origin to align them.
        let origin = self
            .panel_bounds
            .lock()
            .map(|g| g.origin)
            .unwrap_or_default();
        let rel_x = pos.x - origin.x;
        let rel_y = pos.y - origin.y;
        // Anchor: offset slightly down-right from the cursor so the click
        // point sits at the menu's top-left corner.
        let left = rel_x + px(2.);
        let top = rel_y + px(2.);

        // Backdrop: closes the menu on left-click anywhere outside the panel.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _window, cx| {
                    this.tab_context_menu = None;
                    cx.notify();
                }),
            )
            // Also dismiss on right-click elsewhere (re-anchor or close).
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _ev, _window, cx| {
                    this.tab_context_menu = None;
                    cx.notify();
                }),
            )
            .child(
                v_flex()
                    .absolute()
                    .left(left)
                    .top(top)
                    .w(px(180.))
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .rounded(px(6.))
                    .shadow_md()
                    .overflow_hidden()
                    // Keep the menu open when clicking inside it.
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx: &mut App| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_ev, _window, cx: &mut App| {
                        cx.stop_propagation();
                    })
                    .child(menu_rows),
            )
    }

    /// Render the Port Forwards view for the active session: list of configured
    /// forwards on the host with start/stop toggles, plus an inline "add" form.
    /// Active forwards show their bound URL for copy into the API tester.
    fn render_forwards_view(
        &mut self,
        window: &mut Window,
        theme: &gpui_component::Theme,
        active: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_component::button::ButtonVariants as _;

        self.ensure_fwd_inputs(window, cx);

        // Expire the status toast after 4s.
        if let Some((t, _)) = self.fwd_status {
            if t.elapsed().as_secs() >= 4 {
                self.fwd_status = None;
            }
        }
        let status_text = self
            .fwd_status
            .as_ref()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();

        let host_id = self
            .sessions
            .get(active)
            .map(|s| s.host_id.clone())
            .unwrap_or_default();
        // Configured forwards from the stored host record.
        let forwards: Vec<crate::ssh::models::PortForward> = self
            .hosts
            .iter()
            .find(|h| h.id == host_id)
            .map(|h| h.port_forwards.clone())
            .unwrap_or_default();

        // Clone input entities for the add form (so closures can move them).
        let (lp_ent, rh_ent, rp_ent) = match &self.fwd_inputs {
            Some(f) => (
                f.local_port.clone(),
                f.remote_host.clone(),
                f.remote_port.clone(),
            ),
            None => return div().into_any_element(),
        };

        let muted = theme.muted_foreground;
        let border = theme.border;

        v_flex()
            .size_full()
            .id("ssh-forwards-scroll")
            .p_4()
            .gap_3()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("本地端口转发"),
            )
            .child(div().text_size(px(12.)).text_color(muted).child(
                "把远端服务映射到本地端口，转发后可在内置 API 测试中直接访问 http://127.0.0.1:端口",
            ))
            // Add-new form
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(110.))
                            .child(Input::new(&lp_ent).small().appearance(false)),
                    )
                    .child(div().text_color(muted).child("→"))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&rh_ent).small().appearance(false)),
                    )
                    .child(
                        div()
                            .w(px(110.))
                            .child(Input::new(&rp_ent).small().appearance(false)),
                    )
                    .child(
                        Button::new("fwd-add")
                            .small()
                            .primary()
                            .label("添加")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let lp_raw = lp_ent.read(cx).value().to_string();
                                let rh = rh_ent.read(cx).value().trim().to_string();
                                let rp_raw = rp_ent.read(cx).value().to_string();
                                let Ok(lp) = lp_raw.trim().parse::<u16>() else {
                                    this.fwd_status =
                                        Some((std::time::Instant::now(), "本地端口无效".into()));
                                    cx.notify();
                                    return;
                                };
                                let Ok(rp) = rp_raw.trim().parse::<u16>() else {
                                    this.fwd_status =
                                        Some((std::time::Instant::now(), "远端端口无效".into()));
                                    cx.notify();
                                    return;
                                };
                                if rh.is_empty() {
                                    this.fwd_status = Some((
                                        std::time::Instant::now(),
                                        "远端 host 不能为空".into(),
                                    ));
                                    cx.notify();
                                    return;
                                }
                                let host_id = this
                                    .sessions
                                    .get(active)
                                    .map(|s| s.host_id.clone())
                                    .unwrap_or_default();
                                if let Some(h) = this.hosts.iter_mut().find(|h| h.id == host_id) {
                                    let mut f = crate::ssh::models::PortForward::new();
                                    f.local_port = lp;
                                    f.remote_host = rh;
                                    f.remote_port = rp;
                                    h.port_forwards.push(f);
                                    crate::ssh::host_store::save_hosts(&this.hosts);
                                    // Reset form fields.
                                    lp_ent.update(cx, |s, cx| s.set_value("", window, cx));
                                    rh_ent.update(cx, |s, cx| s.set_value("", window, cx));
                                    rp_ent.update(cx, |s, cx| s.set_value("", window, cx));
                                    this.fwd_status = Some((
                                        std::time::Instant::now(),
                                        "已添加，点 ▶ 启动".into(),
                                    ));
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(!status_text.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.primary)
                        .child(status_text.clone()),
                )
            })
            .child(div().h(px(1.)).w_full().bg(border))
            .child(if forwards.is_empty() {
                div()
                    .text_size(px(13.))
                    .text_color(muted)
                    .child("暂无转发规则")
                    .into_any_element()
            } else {
                // Build an owned vec of (index, forward, bound_addr) so the map closure
                // doesn't borrow `forwards` across the `.children()` iterator.
                let rows: Vec<(usize, crate::ssh::models::PortForward, String)> = forwards
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let bound_addr = self
                            .sessions
                            .get(active)
                            .and_then(|s| {
                                s.active_forwards.iter().find_map(|lf| {
                                    if lf.bound_addr.ends_with(&format!(":{}", f.local_port)) {
                                        Some(lf.bound_addr.clone())
                                    } else {
                                        None
                                    }
                                })
                            })
                            .unwrap_or_default();
                        (i, f.clone(), bound_addr)
                    })
                    .collect();

                v_flex()
                    .gap_2()
                    .children(rows.into_iter().map(|(i, f, bound_addr)| {
                        let spec = format!(
                            "127.0.0.1:{} → {}:{}",
                            f.local_port, f.remote_host, f.remote_port
                        );
                        let is_running = !bound_addr.is_empty();
                        let bound_addr_c = bound_addr.clone();

                        h_flex()
                            .gap_2()
                            .items_center()
                            .p_2()
                            .border_1()
                            .border_color(border)
                            .rounded_md()
                            .child(
                                Button::new(("fwd-toggle", i))
                                    .xsmall()
                                    .icon(if is_running {
                                        IconName::Pause
                                    } else {
                                        IconName::Play
                                    })
                                    .when(is_running, |b| b.primary())
                                    .tooltip(if is_running {
                                        "停止转发"
                                    } else {
                                        "启动转发"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let Some(session) = this.sessions.get_mut(active) else {
                                            return;
                                        };
                                        if is_running {
                                            if let Some(pos) = session
                                                .active_forwards
                                                .iter()
                                                .position(|lf| lf.bound_addr == bound_addr)
                                            {
                                                let lf = session.active_forwards.remove(pos);
                                                lf.stop();
                                                this.fwd_status = Some((
                                                    std::time::Instant::now(),
                                                    "已停止转发".into(),
                                                ));
                                            }
                                        } else {
                                            let Some(conn) = session.connection.clone() else {
                                                this.fwd_status = Some((
                                                    std::time::Instant::now(),
                                                    "未连接".into(),
                                                ));
                                                cx.notify();
                                                return;
                                            };
                                            let local_host = "127.0.0.1".to_string();
                                            let lp = f.local_port;
                                            let rh = f.remote_host.clone();
                                            let rp = f.remote_port;
                                            let panel = cx.entity().downgrade();
                                            cx.spawn(async move |_this, cx| {
                                                let res = conn
                                                    .start_local_forward(&local_host, lp, &rh, rp)
                                                    .await;
                                                let _ = panel.update(cx, move |this, cx| {
                                                    match res {
                                                        Ok(lf) => {
                                                            let bound = lf.bound_addr.clone();
                                                            if let Some(s) =
                                                                this.sessions.get_mut(active)
                                                            {
                                                                s.active_forwards.push(lf);
                                                            }
                                                            this.fwd_status = Some((
                                                                std::time::Instant::now(),
                                                                format!("已启动: http://{bound}"),
                                                            ));
                                                        }
                                                        Err(e) => {
                                                            this.fwd_status = Some((
                                                                std::time::Instant::now(),
                                                                format!("启动失败: {e}"),
                                                            ));
                                                        }
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                            .detach();
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1().text_size(px(13.)).child(spec))
                            .when(is_running, |d| {
                                let copy_addr = bound_addr_c.clone();
                                d.child(
                                    Button::new(("fwd-copy", i))
                                        .ghost()
                                        .xsmall()
                                        .label("复制 URL")
                                        .tooltip(format!("http://{copy_addr}"))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                format!("http://{copy_addr}"),
                                            ));
                                        })),
                                )
                            })
                            .child(
                                Button::new(("fwd-del", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let host_id = this
                                            .sessions
                                            .get(active)
                                            .map(|s| s.host_id.clone())
                                            .unwrap_or_default();
                                        // Stop if running.
                                        if let Some(session) = this.sessions.get_mut(active) {
                                            if let Some(pos) = session
                                                .active_forwards
                                                .iter()
                                                .position(|lf| lf.bound_addr == bound_addr_c)
                                            {
                                                let lf = session.active_forwards.remove(pos);
                                                lf.stop();
                                            }
                                        }
                                        if let Some(h) =
                                            this.hosts.iter_mut().find(|h| h.id == host_id)
                                        {
                                            if i < h.port_forwards.len() {
                                                h.port_forwards.remove(i);
                                                crate::ssh::host_store::save_hosts(&this.hosts);
                                            }
                                        }
                                        cx.notify();
                                    })),
                            )
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }

    /// Close a terminal session by tab index.
    pub fn close_session(&mut self, idx: usize, cx: &mut Context<Self>) {
        // Drop the input sender to signal the background task to stop, and
        // terminate a local shell if this is a local-terminal tab.
        if let Some(session) = self.sessions.get_mut(idx) {
            if let Some(mut pty) = session.local.take() {
                pty.kill();
            }
            session.input_tx.take();
            self.sessions.remove(idx);
            if self.active_tab >= self.sessions.len() && !self.sessions.is_empty() {
                self.active_tab = self.sessions.len() - 1;
            }
            if self.sessions.is_empty() {
                self.active_tab = 0;
                // Back to the plain host-grid page (render falls through to
                // the empty-state grid once no tabs remain).
                self.show_host_grid = false;
            }
        }
        cx.notify();
    }

    /// Duplicate the session at `idx`: looks up the host config and starts a
    /// fresh, independent connection in a new tab (own socket + session_uid;
    /// the original is untouched). Calls start_session directly rather than
    /// going through pending_connect so it works even outside the render loop.
    fn duplicate_session(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.tab_context_menu = None;
        let Some(session) = self.sessions.get(idx) else {
            log::warn!("duplicate_session: idx {} out of range (len {})", idx, self.sessions.len());
            return;
        };
        let host_id = session.host_id.clone();
        let host_name = session.host_name.clone();
        let session_is_local = session.is_local();
        log::info!(
            "duplicate_session: idx={} host_id={} host_name={:?}",
            idx, host_id, host_name
        );

        // Local-terminal tabs duplicate into a fresh local shell.
        if session_is_local {
            self.start_local_session(cx);
            return;
        }

        // Priority 1: reuse an already-authenticated connection (skips
        // connect + MFA — the duplicated tab gets a fresh independent shell on
        // the same transport). This is the whole point of "duplicate" for
        // MFA-protected hosts: no second OTP prompt.
        if let Some(conn) = session.connection.clone() {
            log::info!(
                "duplicate_session: reusing authenticated connection for {} (skip MFA)",
                host_name
            );
            let host = host_store::find_host(&host_id).unwrap_or_else(|| SshHost {
                id: host_id.clone(),
                name: host_name.clone(),
                ..SshHost::new(&host_name, "")
            });
            self.start_session(host, Some(conn), cx);
            return;
        }

        // Priority 2: no live connection yet (still connecting, or the
        // connection dropped) → fall back to a full fresh connect, which may
        // re-trigger MFA.
        let Some(host) = host_store::find_host(&host_id) else {
            log::warn!(
                "duplicate_session: host_store::find_host returned None for id {} — \
                 host config may have been deleted; no new session will start",
                host_id
            );
            cx.notify();
            return;
        };
        log::info!(
            "duplicate_session: no live connection, starting fresh connect for {}",
            host.name
        );
        self.start_session(host, None, cx);
    }

    /// Close every session except the one at `keep`. The kept session moves to
    /// index 0 and becomes active. Each dropped session's input_tx is taken so
    /// its background SSH task observes the channel close and exits.
    fn close_other_sessions(&mut self, keep: usize, cx: &mut Context<Self>) {
        // Bounds check first (AGENTS.md §一): `keep` must reference a real tab.
        if keep >= self.sessions.len() {
            self.tab_context_menu = None;
            return;
        }
        // Move the kept session out (swap_remove avoids a shift), then drain the
        // rest — taking each one's input_tx so its background SSH task exits,
        // and killing any local shells.
        let kept = self.sessions.swap_remove(keep);
        for mut s in self.sessions.drain(..) {
            if let Some(mut pty) = s.local.take() {
                pty.kill();
            }
            s.input_tx.take();
        }
        self.sessions.push(kept);
        self.active_tab = 0;
        self.tab_context_menu = None;
        cx.notify();
    }

    /// Apply a ZMODEM event received from the pump thread to the session
    /// matching `uid` (unique per-session id, not host_id, so multiple tabs
    /// for the same host route events independently).
    fn handle_zm_event(
        &mut self,
        uid: u64,
        ev: crate::ssh::zmodem::ZmEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::ssh::zmodem::ZmEvent::*;
        let Some(s) = self.sessions.iter_mut().find(|s| s.session_uid == uid) else {
            return;
        };
        match ev {
            Started {
                filename,
                total,
                role,
            } => {
                s.zm_state = ZmUiState {
                    active: true,
                    filename,
                    total,
                    transferred: 0,
                    role,
                    done: false,
                    error: None,
                    cancelled: false,
                };
            }
            Progress {
                transferred, total, ..
            } => {
                s.zm_state.transferred = transferred;
                s.zm_state.total = total;
            }
            Completed { filename, .. } => {
                s.zm_state.active = false;
                s.zm_state.done = true;
                s.zm_state.filename = filename;
            }
            Cancelled => {
                // User (or remote) cancelled; cleanly return to terminal.
                s.zm_state.active = false;
                s.zm_state.done = true;
                s.zm_state.cancelled = true;
                s.zm_state.error = None;
            }
            Error { message } => {
                s.zm_state.active = false;
                s.zm_state.done = true;
                s.zm_state.error = Some(message);
            }
            NeedSavePath { .. } | NeedFileToSend => {
                // These are intercepted at the pump match arms (we convert
                // them to dedicated PumpMsg variants) and never reach here.
            }
        }
        cx.notify();
    }

    /// While the host-picker overlay is up, the terminal's per-render
    /// `window.focus(...)` would steal focus from the search box every frame,
    /// so we suppress it. Call `restore_terminal_focus` when the overlay closes.
    fn restore_terminal_focus(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.sessions.get(self.active_tab) {
            let terminal = s.terminal.clone();
            terminal.update(cx, |tv, cx| {
                tv.suppress_auto_focus = false;
                cx.notify();
            });
        }
    }

    /// Render a host-picker overlay for starting new connections.
    fn render_host_picker_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        theme: gpui_component::Theme,
    ) -> impl IntoElement {
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let panel = cx.entity();

        // Lazily create the search input on first render (needs &mut Window).
        if self.host_picker_search.is_none() {
            let search = cx.new(|cx| {
                InputState::new(window, cx).placeholder("搜索名称或主机地址…")
            });
            cx.subscribe(
                &search,
                |_this: &mut Self,
                 _src,
                 ev: &gpui_component::input::InputEvent,
                 cx: &mut Context<Self>| {
                    if matches!(ev, gpui_component::input::InputEvent::Change) {
                        cx.notify();
                    }
                },
            )
            .detach();
            self.host_picker_search = Some(search);
        }
        let search_input = self.host_picker_search.clone().unwrap();
        // Grab focus so the user can start typing immediately.
        search_input.update(cx, |state, cx| state.focus(window, cx));
        // Case-insensitive filter against name and host address (also port).
        let query = search_input.read(cx).value().to_string().to_lowercase();
        let filtered: Vec<&crate::ssh::models::SshHost> = self
            .hosts
            .iter()
            .filter(|h| {
                query.is_empty()
                    || h.name.to_lowercase().contains(&query)
                    || h.host.to_lowercase().contains(&query)
                    || h.port.to_string().contains(&query)
            })
            .collect();

        v_flex()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(gpui::black().opacity(0.4))
            .items_start()
            .pt(px(50.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.show_host_picker = false;
                    this.restore_terminal_focus(cx);
                    cx.notify();
                }),
            )
            .child(
                v_flex()
                    .id("host-picker-panel")
                    .mt(px(44.))
                    .ml(px(80.))
                    .w(px(360.))
                    .max_h(px(500.))
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .shadow_md()
                    .overflow_y_scroll()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx: &mut App| {
                        // Prevent the backdrop's on_mouse_down from closing the picker
                        // when the user clicks inside the panel.
                        cx.stop_propagation();
                    })
                    .child(
                        h_flex()
                            .h(px(36.))
                            .px_3()
                            .items_center()
                            .border_b_1()
                            .border_color(border)
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("选择主机连接"),
                            )
                            .child(div().flex_1())
                            .child(
                                gpui_component::button::Button::new("picker-close")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_host_picker = false;
                                        this.restore_terminal_focus(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    // Search box: filters the host list below by name or host.
                    .child(
                        div()
                            .px_2()
                            .py_1p5()
                            .border_b_1()
                            .border_color(border)
                            .child(Input::new(&search_input).w_full().small()),
                    )
                    // Local terminal entry — the user's shell on this machine
                    // (Terminal.app replacement), no SSH involved.
                    .child({
                        let tc = theme.clone();
                        let panel_c = panel.clone();
                        h_flex()
                            .id("picker-local")
                            .w_full()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .border_b_1()
                            .border_color(border)
                            .cursor_pointer()
                            .hover(|d| d.bg(tc.muted))
                            .child(div().text_lg().child("⌨️"))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("本地终端"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child("在本机打开默认 Shell"),
                                    ),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_event, _window, cx: &mut App| {
                                    cx.stop_propagation();
                                    let _ = panel_c.update(cx, |this, cx| {
                                        this.show_host_picker = false;
                                        this.restore_terminal_focus(cx);
                                        this.start_local_session(cx);
                                    });
                                },
                            )
                    })
                    .children({
                        let host_rows = filtered.iter().enumerate().map(|(i, host)| {
                        let host_id = host.id.clone();
                        let host_name = host.name.clone();
                        let host_addr = host.address_summary();
                        let tc = theme.clone();
                        let panel_c = panel.clone();

                        h_flex()
                            .id(("picker-host", i))
                            .w_full()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .cursor_pointer()
                            .hover(|d| d.bg(tc.muted))
                            .child(div().text_lg().child("🖥️"))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(host_name),
                                    )
                                    .child(div().text_xs().text_color(muted).child(host_addr)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_event, _window, cx: &mut App| {
                                    cx.stop_propagation();
                                    let _ = panel_c.update(cx, |this, cx| {
                                        if let Some(h) =
                                            this.hosts.iter().find(|h| h.id == host_id).cloned()
                                        {
                                            this.pending_connect = Some(h);
                                        }
                                        this.show_host_picker = false;
                                        this.restore_terminal_focus(cx);
                                        cx.notify();
                                    });
                                },
                            )
                    });
                        let empty = if filtered.is_empty() && !query.is_empty() {
                            Some(
                                div()
                                    .id("picker-empty")
                                    .px_3()
                                    .py_3()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("无匹配的主机"),
                            )
                        } else {
                            None
                        };
                        host_rows.chain(empty)
                    })
            )
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_prompt;

    #[test]
    fn truncate_prompt_short_unchanged() {
        assert_eq!(truncate_prompt("Verification code:", 40), "Verification code:");
    }

    #[test]
    fn truncate_prompt_long_elided() {
        let long = "请输入您的动态验证码以完成多因素认证并登录到远程服务器系统";
        let out = truncate_prompt(long, 10);
        assert!(out.chars().count() == 10, "elided length is exactly max");
        assert!(out.ends_with('…'));
        // The elided string must be a valid char boundary slice (no panic).
        assert_eq!(out.chars().count(), out.chars().count());
    }

    #[test]
    fn truncate_prompt_multibyte_safe() {
        // Pure multibyte input — must never panic on byte boundaries.
        let out = truncate_prompt("验证码验证码验证码", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_prompt_empty() {
        assert_eq!(truncate_prompt("", 40), "");
    }
}
