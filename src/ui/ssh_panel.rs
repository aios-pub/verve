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
    ActiveTheme, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ssh::client::SshConnection;
use crate::ssh::host_store;
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

/// Messages from the SSH pump thread back to GPUI's executor.
enum PumpMsg {
    Data(Vec<u8>),
    InputTx(tokio::sync::mpsc::UnboundedSender<Vec<u8>>),
    ResizeTx(tokio::sync::mpsc::UnboundedSender<crate::ssh::client::ResizeCmd>),
    /// A connection has been established (used to make Sftp available on demand).
    Connected(crate::ssh::client::SshConnection),
    Closed(Option<u32>),
    Error(String),
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    Reconnecting(usize), // attempt number
    Error(String),
    Closed,
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
    /// When true, show the host picker overlay (for starting new connections
    /// without closing existing sessions).
    pub show_host_picker: bool,
    /// When Some(message), show a small status toast in the Forwards view
    /// (e.g. "已启动: 127.0.0.1:8080 → ..."). Cleared after a few seconds by
    /// the render loop when stale.
    pub fwd_status: Option<(std::time::Instant, String)>,
    /// Cached InputState entities for the "add forward" form. Created lazily
    /// on first render (they require a `&Window`).
    pub fwd_inputs: Option<FwdInputs>,
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
            show_host_picker: false,
            fwd_status: None,
            fwd_inputs: None,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.stale {
            self.reload(cx);
        }
        // Reload hosts if the picker is about to be shown (ensure fresh list).
        if self.show_host_picker && self.hosts.is_empty() {
            self.hosts = host_store::load_hosts();
        }
        // Process pending connection.
        if let Some(host) = self.pending_connect.take() {
            self.start_session(host, cx);
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
            let terminal_view = self.render_terminal_view(_window, cx, theme.clone());

            if show_picker {
                // Overlay: dimmed background + host card grid on top.
                return v_flex()
                    .size_full()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(bg)
                    .text_color(fg)
                    .child(terminal_view)
                    .child(self.render_host_picker_overlay(cx, theme))
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
            .children(self.hosts.iter().enumerate().map(|(ix, host)| {
                let theme_c = theme.clone();
                let _theme_c2 = theme.clone();
                let _theme_c3 = theme.clone();
                let host_id = host.id.clone();
                let panel_c = panel.clone();
                let host_id_edit = host.id.clone();
                let host_id_del = host.id.clone();
                let jump_name = host.bastion_id.as_ref().map(|jid| host_name_of(jid));

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
                    // Top row: icon + name.
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
                            ),
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

    /// Start a new SSH session.
    fn start_session(&mut self, host: SshHost, cx: &mut Context<Self>) {
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
        // SSH session. Uses AppleScript to select the ABC/US keyboard layout.
        // After this initial switch, the user can manually toggle to Chinese
        // — we only force English at init, not block later IME changes.
        #[cfg(target_os = "macos")]
        {
            std::thread::spawn(|| {
                let _ = std::process::Command::new("osascript")
                    .arg("-e")
                    .arg(r#"tell application "System Events" to key code 102"#)
                    .output();
            });
        }

        // Create a terminal view with a wider initial size so ls uses multi-column.
        let terminal = cx.new(|cx| TerminalView::new(180, 50, cx));

        // Spawn the SSH connection on a background Tokio task.
        let terminal_for_task = terminal.clone();
        let host_for_task = host.clone();
        // Weak entity reference so we can stash the SshConnection on the session.
        let panel_for_task = cx.entity().downgrade();
        let session_uid_for_task = session_uid;

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
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
                    rt.block_on(async move {
                        match crate::ssh::client::SshConnection::connect(&host_connect).await {
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
                                        let _ = tx.send(crate::ssh::client::SshOutput::Error(e));
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("SSH connect attempt {}: {e}", attempt);
                                let _ = tx.send(crate::ssh::client::SshOutput::Error(e));
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
                                        crate::ssh::client::SshOutput::Error(e) => {
                                            let _ = pump_tx_clone.send(PumpMsg::Error(e));
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
                let session_uid = session_uid_for_task;
                while let Some(msg) = pump_rx.recv().await {
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
                            let m = format!("\r\n[会话结束，退出码: {:?}]\r\n", code);
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.feed(m.as_bytes());
                                cx.notify();
                            });
                            if let Some(panel) = panel_for_task.upgrade() {
                                let uid = session_uid;
                                let _ = panel.update(cx, move |this, cx| {
                                    if let Some(s) = this
                                        .sessions
                                        .iter_mut()
                                        .find(|s| s.session_uid == uid)
                                    {
                                        s.status = SessionStatus::Closed;
                                        cx.notify();
                                    }
                                });
                            }
                            got_disconnected = false;
                            break;
                        }
                        PumpMsg::Error(e) => {
                            log::warn!("SSH session error (attempt {}): {}", attempt, e);
                            let m = format!("\r\n连接错误: {}\r\n", e);
                            let _ = terminal_clone.update(cx, |tv, cx| {
                                tv.feed(m.as_bytes());
                                cx.notify();
                            });
                            got_disconnected = true;
                            break;
                        }
                        PumpMsg::Disconnected => {
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
                    }
                }

                if !got_disconnected {
                    break;
                }

                if attempt >= max_attempts {
                    let msg = format!(
                        "\r\n✕ 重连失败（已尝试 {} 次）。请检查网络后重新连接。\r\n",
                        max_attempts
                    );
                    let _ = terminal_for_task.update(cx, |tv, cx| {
                        tv.feed(msg.as_bytes());
                        cx.notify();
                    });
                    break;
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
        });
        self.active_tab = self.sessions.len() - 1;
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

        // Resolve the content element based on view mode.
        let content: AnyElement = if active_view == SessionView::Sftp {
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
                                cx.notify();
                            }))
                    }))
                    // "+" button to open host list and start a new connection.
                    .child(
                        gpui_component::button::Button::new("ssh-new-conn")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip("新建连接")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_host_picker = true;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    // View toggle: Terminal / Files
                    .child(
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
                                        if let Some(s) = this.sessions.get_mut(this.active_tab) {
                                            s.view = SessionView::Terminal;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child({
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
                                        if let Some(s) = this.sessions.get_mut(this.active_tab) {
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
                                    .when(active_view == SessionView::Forwards, |b| b.primary())
                                    .tooltip(if can_open {
                                        "本地端口转发：将远端服务映射到本地端口"
                                    } else {
                                        "等待连接建立…"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !can_open {
                                            return;
                                        }
                                        if let Some(s) = this.sessions.get_mut(this.active_tab) {
                                            s.view = SessionView::Forwards;
                                        }
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
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
            .when(
                active_zm_state.active
                    || (active_zm_state.done && !active_zm_state.cancelled)
                    || active_zm_state.error.is_some(),
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
            // Status bar.
            .child(
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
    fn close_session(&mut self, idx: usize, cx: &mut Context<Self>) {
        // Drop the input sender to signal the background task to stop.
        if idx < self.sessions.len() {
            self.sessions[idx].input_tx.take();
            self.sessions.remove(idx);
            if self.active_tab >= self.sessions.len() && !self.sessions.is_empty() {
                self.active_tab = self.sessions.len() - 1;
            }
            if self.sessions.is_empty() {
                self.active_tab = 0;
            }
        }
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

    /// Render a host-picker overlay for starting new connections.
    fn render_host_picker_overlay(
        &mut self,
        cx: &mut Context<Self>,
        theme: gpui_component::Theme,
    ) -> impl IntoElement {
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let panel = cx.entity();

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
                                        cx.notify();
                                    })),
                            ),
                    )
                    .children(self.hosts.iter().enumerate().map(|(i, host)| {
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
                                        cx.notify();
                                    });
                                },
                            )
                    })),
            )
    }
}
