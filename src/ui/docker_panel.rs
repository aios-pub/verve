//! Docker management panel — the full-bleed "Docker" exclusive view.
//!
//! Lists containers and images, supports start/stop/restart/rm, streams logs,
//! opens interactive exec. All docker CLI calls are synchronous std::process
//! invocations; we run them on background threads so they never block the UI.
//!
//! Sessions (logs, shells) are modeled as independent tabs that persist across
//! Containers/Images/Hosts page switches — the user can open a shell in one
//! container, browse images, then come back to the same shell without losing
//! state.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme, Disableable as _, IconName, Sizable as _, h_flex, v_flex};

use crate::docker::{
    Container, DockerError, DockerHost, ExecSession, ImageInfo, LogStream, close_ssh_tunnel,
    exec_container, find_host, get_active_id, list_containers, list_images, load_hosts,
    prune_images, remove as docker_remove, remove_host, restart as docker_restart, set_active_id,
    start as docker_start, stop as docker_stop, stream_logs, upsert_host,
};
use crate::ssh::terminal::TerminalView;
use crate::ui::docker_host_dialog;
use gpui_component::text::TextView;

/// Top-level "page" the user is viewing. Sessions (logs/shell) are overlaid
/// on top of these via a tab bar; switching between Containers/Images/Hosts
/// does NOT tear down open sessions.
#[derive(Clone, PartialEq, Eq)]
enum Page {
    Containers,
    Images,
    Hosts,
}

/// What kind of session a tab represents.
#[derive(Clone, PartialEq, Eq)]
enum SessionKind {
    Logs,
    Shell,
}

/// An open docker session: streaming logs for a container, or an interactive
/// shell. Sessions persist across page switches and each owns its background
/// resources (log stream, PTY exec). Switching back to a session just re-shows
/// its UI without reconnecting.
struct Session {
    kind: SessionKind,
    container_id: String,
    container_name: String,
    // Logs state.
    log_lines: Vec<String>,
    log_stream: Option<LogStream>,
    // Shell state.
    terminal: Option<Entity<TerminalView>>,
    stdin_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    /// Owns the PTY/IO threads — kept here so they stay alive.
    _exec_session: Option<ExecSession>,
}

impl Session {
    fn new_logs(id: String, name: String) -> Self {
        Self {
            kind: SessionKind::Logs,
            container_id: id,
            container_name: name,
            log_lines: Vec::new(),
            log_stream: None,
            terminal: None,
            stdin_tx: None,
            _exec_session: None,
        }
    }
    fn new_shell(id: String, name: String) -> Self {
        Self {
            kind: SessionKind::Shell,
            container_id: id,
            container_name: name,
            log_lines: Vec::new(),
            log_stream: None,
            terminal: None,
            stdin_tx: None,
            _exec_session: None,
        }
    }
    fn title(&self) -> String {
        let short_id: String = self.container_id.chars().take(10).collect();
        let label = match self.kind {
            SessionKind::Logs => "日志",
            SessionKind::Shell => "Shell",
        };
        format!("{label} · {short_id}")
    }
}

pub struct DockerPanel {
    error: Option<String>,
    active_page: Page,
    /// Currently visible session id (container id with optional kind). We key
    /// sessions by (container_id, kind) stored as `"{id}|{kind}"`.
    active_session_key: Option<String>,
    sessions: Vec<Session>,
    hosts: Vec<DockerHost>,
    active_host_id: String,
    containers: Vec<Container>,
    containers_loading: bool,
    container_filter: String,
    containers_loaded: bool,
    images: Vec<ImageInfo>,
    images_loading: bool,
}

fn session_key(id: &str, kind: SessionKind) -> String {
    let k = match kind {
        SessionKind::Logs => "logs",
        SessionKind::Shell => "shell",
    };
    format!("{id}|{k}")
}

impl DockerPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            error: None,
            active_page: Page::Containers,
            active_session_key: None,
            sessions: Vec::new(),
            hosts: load_hosts(),
            active_host_id: get_active_id(),
            containers: Vec::new(),
            containers_loading: false,
            container_filter: "running".into(),
            containers_loaded: false,
            images: Vec::new(),
            images_loading: false,
        }
    }

    /// Called by app.rs when the user clicks the Docker sidebar item. Lazily
    /// loads containers on first activation so app startup isn't delayed.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if !self.containers_loaded && !self.containers_loading {
            self.containers_loaded = true;
            self.reload_containers(cx);
        }
    }

    fn active_host(&self) -> Option<DockerHost> {
        find_host(&self.active_host_id)
    }

    pub fn host_count(&self) -> usize {
        self.hosts.len()
    }

    pub fn active_host_name(&self) -> String {
        self.hosts
            .iter()
            .find(|h| h.id == self.active_host_id)
            .map(|h| h.name.clone())
            .unwrap_or_else(|| "本地 Docker".into())
    }

    /// Which page is active for title-bar tab highlighting.
    pub fn current_view(&self) -> &'static str {
        if self.active_session_key.is_some() {
            return "containers";
        }
        match self.active_page {
            Page::Containers => "containers",
            Page::Images => "images",
            Page::Hosts => "hosts",
        }
    }

    fn in_session(&self) -> bool {
        self.active_session_key.is_some()
    }

    /// Count open sessions (shown as a badge in the tab bar).
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn show_containers(&mut self, cx: &mut Context<Self>) {
        self.active_page = Page::Containers;
        self.active_session_key = None;
        if self.containers.is_empty() || self.containers_loading {
            self.containers_loading = false;
            self.reload_containers(cx);
        }
        cx.notify();
    }

    pub fn show_images(&mut self, cx: &mut Context<Self>) {
        self.active_page = Page::Images;
        self.active_session_key = None;
        if self.images.is_empty() || self.images_loading {
            self.images_loading = false;
            self.reload_images(cx);
        }
        cx.notify();
    }

    pub fn show_hosts(&mut self, cx: &mut Context<Self>) {
        self.active_page = Page::Hosts;
        self.active_session_key = None;
        cx.notify();
    }

    pub fn refresh_current(&mut self, cx: &mut Context<Self>) {
        if self.in_session() {
            self.reload_containers(cx);
            return;
        }
        match self.active_page {
            Page::Containers => self.reload_containers(cx),
            Page::Images => self.reload_images(cx),
            Page::Hosts => cx.notify(),
        }
    }

    pub fn open_add_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_host_dialog(None, window, cx);
    }

    /// Open the Docker host dialog (add when `host_id` is None, edit
    /// otherwise). While the modal is up over a shell session, the terminal's
    /// per-render `window.focus(...)` would steal focus from the dialog's
    /// input fields every frame (same failure as the SSH panel's overlays —
    /// see `TerminalView::suppress_auto_focus`), so suppress it and restore
    /// via `restore_terminal_focus` on save / dismiss.
    fn open_host_dialog(
        &mut self,
        host_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editing = host_id.and_then(find_host);
        let panel = cx.entity();
        self.suppress_terminal_auto_focus(cx);
        let panel_for_dismiss = panel.clone();
        docker_host_dialog::open_dialog(
            editing.as_ref(),
            move |_host: DockerHost, _window, cx: &mut App| {
                let _ = panel.update(cx, |this, cx| {
                    this.reload_hosts();
                    cx.notify();
                    this.restore_terminal_focus(cx);
                });
            },
            move |_window, cx: &mut App| {
                let _ = panel_for_dismiss.update(cx, |this, cx| {
                    this.restore_terminal_focus(cx);
                });
            },
            window,
            cx,
        );
    }

    fn reload_hosts(&mut self) {
        self.hosts = load_hosts();
        if self
            .hosts
            .iter()
            .find(|h| h.id == self.active_host_id)
            .is_none()
        {
            self.active_host_id = "local".into();
            set_active_id("local");
        }
    }

    fn switch_host(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(prev) = find_host(&self.active_host_id) {
            if let Some(ssh_id) = &prev.ssh_host_id {
                close_ssh_tunnel(ssh_id);
            }
        }
        self.active_host_id = id.clone();
        set_active_id(&id);
        // Close all sessions (they belong to the old host).
        self.sessions.clear();
        self.active_session_key = None;
        self.containers.clear();
        self.images.clear();
        self.container_filter = "running".into();
        self.containers_loaded = false;
        self.error = None;
        cx.notify();
    }

    /// Open an existing session or create a new one for the given container/kind.
    fn open_or_focus_session(
        &mut self,
        container_id: String,
        container_name: String,
        kind: SessionKind,
        cx: &mut Context<Self>,
    ) {
        let key = session_key(&container_id, kind.clone());
        if let Some(pos) = self
            .sessions
            .iter()
            .position(|s| session_key(&s.container_id, s.kind.clone()) == key)
        {
            self.active_session_key = Some(key);
            cx.notify();
            return;
        }
        let session = match kind {
            SessionKind::Logs => Session::new_logs(container_id, container_name),
            SessionKind::Shell => Session::new_shell(container_id, container_name),
        };
        self.sessions.push(session);
        self.active_session_key = Some(key);
        cx.notify();
    }

    fn close_session_by_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .sessions
            .iter()
            .position(|s| session_key(&s.container_id, s.kind.clone()) == key)
        {
            self.sessions.remove(pos);
            if self.active_session_key.as_deref() == Some(key) {
                self.active_session_key = None;
            }
            cx.notify();
        }
    }

    fn active_session_idx(&self) -> Option<usize> {
        let key = self.active_session_key.as_ref()?;
        self.sessions
            .iter()
            .position(|s| session_key(&s.container_id, s.kind.clone()) == *key)
    }

    /// While the host dialog modal is up, the visible session's terminal must
    /// not re-focus itself every render (see `open_host_dialog`). Pair with
    /// `restore_terminal_focus` when the dialog closes. No-op when no shell
    /// session is visible (e.g. on the Hosts page).
    fn suppress_terminal_auto_focus(&mut self, cx: &mut Context<Self>) {
        let Some(idx) = self.active_session_idx() else {
            return;
        };
        // Bounds-checked per AGENTS.md; a connecting session may have no
        // terminal yet.
        if let Some(s) = self.sessions.get(idx) {
            if let Some(terminal) = &s.terminal {
                terminal.update(cx, |tv, cx| {
                    tv.suppress_auto_focus = true;
                    cx.notify();
                });
            }
        }
    }

    /// Re-enable the terminal's auto-focus after the host dialog closes.
    fn restore_terminal_focus(&mut self, cx: &mut Context<Self>) {
        let Some(idx) = self.active_session_idx() else {
            return;
        };
        if let Some(s) = self.sessions.get(idx) {
            if let Some(terminal) = &s.terminal {
                terminal.update(cx, |tv, cx| {
                    tv.suppress_auto_focus = false;
                    cx.notify();
                });
            }
        }
    }

    // ---- blocking work runner ------------------------------------------------

    fn run_blocking<F, T>(
        &mut self,
        cx: &mut Context<Self>,
        f: F,
        on_done: impl 'static + Send + FnOnce(&mut Self, &mut Context<Self>, T),
        on_err: impl 'static + Send + FnOnce(&mut Self, &mut Context<Self>, String),
    ) where
        F: 'static + Send + FnOnce() -> T,
        T: 'static + Send,
    {
        let (tx, rx) = tokio::sync::oneshot::channel::<T>();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            match result {
                Ok(v) => {
                    let _ = tx.send(v);
                }
                Err(panic) => {
                    let msg = panic
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "后台线程异常终止".into());
                    log::error!("docker worker thread panicked: {msg}");
                    drop(tx);
                }
            }
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| match rx.await {
            Ok(result) => {
                let _ = ent.update(cx, move |panel, cx| on_done(panel, cx, result));
            }
            Err(_) => {
                let _ = ent.update(cx, move |panel, cx| {
                    on_err(panel, cx, "docker 操作异常终止，请重试".into())
                });
            }
        })
        .detach();
    }

    // ---- data reloaders ------------------------------------------------------

    fn reload_containers(&mut self, cx: &mut Context<Self>) {
        if self.containers_loading {
            return;
        }
        self.containers_loading = true;
        self.error = None;
        cx.notify();
        let host = self.active_host();
        self.run_blocking(
            cx,
            move || list_containers(host.as_ref()),
            |panel, cx, res| {
                panel.containers_loading = false;
                match res {
                    Ok(cs) => panel.containers = cs,
                    Err(e) => panel.error = Some(e.to_string()),
                }
                cx.notify();
            },
            |panel, cx, msg| {
                panel.containers_loading = false;
                panel.error = Some(msg);
                cx.notify();
            },
        );
    }

    fn reload_images(&mut self, cx: &mut Context<Self>) {
        if self.images_loading {
            return;
        }
        self.images_loading = true;
        self.error = None;
        cx.notify();
        let host = self.active_host();
        self.run_blocking(
            cx,
            move || list_images(host.as_ref()),
            |panel, cx, res| {
                panel.images_loading = false;
                match res {
                    Ok(imgs) => panel.images = imgs,
                    Err(e) => panel.error = Some(e.to_string()),
                }
                cx.notify();
            },
            |panel, cx, msg| {
                panel.images_loading = false;
                panel.error = Some(msg);
                cx.notify();
            },
        );
    }

    fn do_prune(&mut self, cx: &mut Context<Self>) {
        let host = self.active_host();
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, DockerError>>();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(_) => return Err(DockerError("无法创建 tokio runtime".into())),
                };
                rt.block_on(prune_images(host.as_ref()))
            }));
            let _ = tx.send(result.unwrap_or_else(|_| Err(DockerError("清理操作异常终止".into()))));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| match rx.await {
            Ok(result) => {
                let _ = ent.update(cx, |panel, cx| {
                    panel.error = Some(result.unwrap_or_else(|e| e.to_string()));
                    panel.reload_images(cx);
                });
            }
            Err(_) => {
                let _ = ent.update(cx, |panel, cx| {
                    panel.error = Some("清理操作异常终止，请重试".into());
                    panel.images_loading = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn container_action<F>(&mut self, id: String, f: F, cx: &mut Context<Self>)
    where
        F: 'static + Send + Fn(Option<&DockerHost>, &str) -> Result<(), DockerError>,
    {
        let host = self.active_host();
        self.run_blocking(
            cx,
            move || f(host.as_ref(), &id),
            |panel, cx, res| {
                if let Err(e) = res {
                    panel.error = Some(e.to_string());
                }
                panel.reload_containers(cx);
            },
            |panel, cx, msg| {
                panel.error = Some(msg);
                cx.notify();
            },
        );
    }

    // ---- logs / exec --------------------------------------------------------

    fn start_logs_for(&mut self, container_id: &str, cx: &mut Context<Self>) {
        enum Ev {
            Line(String),
            Err(String),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let id_for_thread = container_id.to_string();
        let host = self.active_host();
        let key = session_key(container_id, SessionKind::Logs);

        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                match stream_logs(host.as_ref(), &id_for_thread, 200) {
                    Ok(mut stream) => {
                        let Some(mut lines_rx) = stream.take_lines() else {
                            return;
                        };
                        while let Some(line) = lines_rx.recv().await {
                            if tx.send(Ev::Line(line)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Ev::Err(e.to_string()));
                    }
                }
            });
        });
        let ent = cx.entity().downgrade();
        let key_for_updates = key.clone();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Ev::Line(line)) => {
                        let _ = ent.update(cx, |p, cx| {
                            if let Some(s) = p.sessions.iter_mut().find(|s| {
                                session_key(&s.container_id, s.kind.clone()) == key_for_updates
                            }) {
                                if s.log_lines.len() > 2000 {
                                    s.log_lines.remove(0);
                                }
                                s.log_lines.push(line);
                                cx.notify();
                            }
                        });
                    }
                    Ok(Ev::Err(e)) => {
                        let _ = ent.update(cx, |p, cx| {
                            p.error = Some(e);
                            cx.notify();
                        });
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        smol::Timer::after(std::time::Duration::from_millis(80)).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    fn start_shell_for(&mut self, container_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.active_host();
        let cid = container_id.to_string();
        let key = session_key(container_id, SessionKind::Shell);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<ExecSession, String>>();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                exec_container(host.as_ref(), &cid, None)
            }));
            let msg = match result {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("docker exec 启动异常".into()),
            };
            let _ = ready_tx.send(msg);
        });

        let ent = cx.entity().downgrade();
        let key_for_setup = key.clone();
        let term_id = container_id.to_string();
        cx.spawn(async move |_this, cx| {
            let sess = match ready_rx.await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    let _ = ent.update(cx, |p, cx| {
                        p.error = Some(e);
                        cx.notify();
                    });
                    return;
                }
                Err(_) => {
                    let _ = ent.update(cx, |p, cx| {
                        p.error = Some("启动 shell 失败".into());
                        cx.notify();
                    });
                    return;
                }
            };
            let ExecSession {
                stdin_tx,
                output_rx,
            } = sess;
            // Can't destructure a struct with a field we need to keep without
            // moving; rebuild an ExecSession with only the channels by wrapping
            // in a helper (the IO threads stay alive as long as channels exist).
            let stdin_tx_term = stdin_tx.clone();
            let stdin_tx_panel = stdin_tx;
            let mut output_rx = output_rx;
            let (term_tx, term_rx) = tokio::sync::oneshot::channel::<Entity<TerminalView>>();

            let _ = ent.update(cx, |p, cx| {
                if let Some(s) = p
                    .sessions
                    .iter_mut()
                    .find(|s| session_key(&s.container_id, s.kind.clone()) == key_for_setup)
                {
                    let terminal = cx.new(|cx| TerminalView::new(120, 32, cx));
                    s.terminal = Some(terminal.clone());
                    s.stdin_tx = Some(stdin_tx_panel);
                    // Wire terminal keystrokes → PTY.
                    _ = terminal.update(cx, |tv, _cx| {
                        if let Ok(mut slot) = tv.input_tx.lock() {
                            *slot = Some(stdin_tx_term);
                        }
                    });
                    p.error = None;
                    let _ = term_tx.send(terminal);
                }
                cx.notify();
            });

            let terminal = match term_rx.await {
                Ok(t) => t,
                Err(_) => return,
            };
            let term_weak = terminal.downgrade();
            while let Some(chunk) = output_rx.recv().await {
                let _ = term_weak.update(cx, |tv, cx| {
                    tv.feed(&chunk);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

// ---- rendering ---------------------------------------------------------------

impl Render for DockerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        self.reload_hosts();

        // Tab bar shown when one or more sessions are open.
        let tab_bar = if self.sessions.is_empty() {
            div().into_any_element()
        } else {
            let active_key = self.active_session_key.clone();
            h_flex()
                .gap_1()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .children(self.sessions.iter().enumerate().map(|(i, s)| {
                    let key = session_key(&s.container_id, s.kind.clone());
                    let is_active = active_key.as_deref() == Some(&key);
                    let key_close = key.clone();
                    let key_focus = key.clone();
                    h_flex()
                        .gap_1()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .when(is_active, |d| {
                            d.bg(theme.accent.opacity(0.25)).text_color(theme.accent)
                        })
                        .when(!is_active, |d| d.text_color(theme.muted_foreground))
                        .child(
                            Button::new(format!("docker-session-focus-{}", i))
                                .ghost()
                                .xsmall()
                                .label(s.title())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.active_session_key = Some(key_focus.clone());
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(format!("docker-session-close-{}", i))
                                .ghost()
                                .xsmall()
                                .label("×")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_session_by_key(&key_close, cx);
                                })),
                        )
                }))
                .into_any_element()
        };

        let body = if let Some(idx) = self.active_session_idx() {
            let s = &self.sessions[idx];
            match s.kind {
                SessionKind::Logs => self.render_logs(&theme, idx, cx),
                SessionKind::Shell => self.render_shell(&theme, idx),
            }
        } else {
            match self.active_page {
                Page::Containers => self.render_containers(&theme, cx).into_any_element(),
                Page::Images => self.render_images(&theme, cx).into_any_element(),
                Page::Hosts => self.render_hosts(&theme, window, cx).into_any_element(),
            }
        };

        v_flex().size_full().min_h_0().child(tab_bar).child(body)
    }
}

impl DockerPanel {
    // ---- containers ---------------------------------------------------------
    fn render_containers(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let loading = self.containers_loading;
        let filter = self.container_filter.clone();

        let running_count = self
            .containers
            .iter()
            .filter(|c| c.state == "running")
            .count();
        let total_count = self.containers.len();

        let rows = v_flex()
            .id("docker-ct-scroll")
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .gap_1();

        let filtered: Vec<&Container> = match filter.as_str() {
            "running" => self
                .containers
                .iter()
                .filter(|c| c.state == "running")
                .collect(),
            "all" => self.containers.iter().collect(),
            _ => self
                .containers
                .iter()
                .filter(|c| c.state == "running")
                .collect(),
        };

        let body =
            if self.containers.is_empty() && !loading {
                rows.child(
                    div().text_size(px(12.)).text_color(muted).child(
                        "点“刷新”列出容器。需要 Docker daemon 正在运行,docker CLI 在 PATH 中。",
                    ),
                )
            } else if filtered.is_empty() && !loading {
                rows.child(div().text_size(px(12.)).text_color(muted).child(
                    if filter == "running" {
                        format!("没有正在运行的容器（共 {total_count} 个已停止）")
                    } else {
                        "没有容器".to_string()
                    },
                ))
            } else {
                rows.children(filtered.into_iter().map(|c| {
                    let state_color = match c.state.as_str() {
                        "running" => theme.primary,
                        "exited" => muted,
                        "paused" => theme.accent,
                        _ => muted,
                    };
                    let id_short: String = c.id.chars().take(12).collect();
                    let is_running = c.state == "running";
                    let cid_log = c.id.clone();
                    let cname_log = c.names.clone();
                    let cid_exec = c.id.clone();
                    let cname_exec = c.names.clone();
                    let cid_stop = c.id.clone();
                    let cid_start = c.id.clone();
                    let cid_restart = c.id.clone();
                    let cid_rm = c.id.clone();
                    h_flex()
                        .gap_2()
                        .items_center()
                        .p_2()
                        .border_1()
                        .border_color(border)
                        .rounded_md()
                        .child(
                            div()
                                .text_size(px(11.))
                                .w(px(90.))
                                .text_color(muted)
                                .child(id_short),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .flex_1()
                                .child(c.names.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(muted)
                                .w(px(150.))
                                .truncate()
                                .child(c.image.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .px(px(6.))
                                .py(px(1.))
                                .rounded_sm()
                                .bg(state_color.opacity(0.18))
                                .text_color(state_color)
                                .child(c.state.clone()),
                        )
                        .child(
                            Button::new(format!("docker-logs-{}", c.id))
                                .xsmall()
                                .ghost()
                                .label("日志")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_or_focus_session(
                                        cid_log.clone(),
                                        cname_log.clone(),
                                        SessionKind::Logs,
                                        cx,
                                    );
                                    this.start_logs_for(&cid_log, cx);
                                })),
                        )
                        .child(
                            Button::new(format!("docker-exec-{}", c.id))
                                .xsmall()
                                .ghost()
                                .label("Shell")
                                .disabled(!is_running)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_or_focus_session(
                                        cid_exec.clone(),
                                        cname_exec.clone(),
                                        SessionKind::Shell,
                                        cx,
                                    );
                                    this.start_shell_for(&cid_exec, window, cx);
                                })),
                        )
                        .child(if is_running {
                            Button::new(format!("docker-stop-{}", c.id))
                                .xsmall()
                                .ghost()
                                .label("停止")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.container_action(cid_stop.clone(), docker_stop, cx);
                                }))
                                .into_any_element()
                        } else {
                            Button::new(format!("docker-start-{}", c.id))
                                .xsmall()
                                .ghost()
                                .label("启动")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.container_action(cid_start.clone(), docker_start, cx);
                                }))
                                .into_any_element()
                        })
                        .child(
                            Button::new(format!("docker-restart-{}", c.id))
                                .xsmall()
                                .ghost()
                                .label("重启")
                                .disabled(!is_running)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.container_action(cid_restart.clone(), docker_restart, cx);
                                })),
                        )
                        .child(
                            Button::new(format!("docker-rm-{}", c.id))
                                .xsmall()
                                .ghost()
                                .label("删除")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.container_action(
                                        cid_rm.clone(),
                                        |h, id| docker_remove(h, id, true),
                                        cx,
                                    );
                                })),
                        )
                }))
            };

        let is_running = filter == "running";
        let is_all = filter == "all";
        let filter_bar = h_flex()
            .gap_1()
            .child(
                Button::new("docker-filter-running")
                    .xsmall()
                    .ghost()
                    .label(format!("运行中 ({running_count})"))
                    .when(is_running, |b| {
                        b.bg(theme.accent.opacity(0.25)).text_color(theme.accent)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.container_filter = "running".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("docker-filter-all")
                    .xsmall()
                    .ghost()
                    .label(format!("全部 ({total_count})"))
                    .when(is_all, |b| {
                        b.bg(theme.accent.opacity(0.25)).text_color(theme.accent)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.container_filter = "all".into();
                        cx.notify();
                    })),
            );

        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_2()
            .children(self.error.as_ref().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(e.clone())
            }))
            .child(filter_bar)
            .child(if loading {
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("加载中…")
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(body)
            .into_any_element()
    }

    // ---- images -------------------------------------------------------------
    fn render_images(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let loading = self.images_loading;
        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_2()
            .child(
                h_flex().gap_2().child(
                    Button::new("docker-prune")
                        .small()
                        .ghost()
                        .icon(IconName::Delete)
                        .label("清理未用镜像")
                        .on_click(cx.listener(|this, _, _, cx| this.do_prune(cx))),
                ),
            )
            .children(self.error.as_ref().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(e.clone())
            }))
            .child(if loading {
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("加载中…")
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(
                v_flex()
                    .id("docker-img-scroll")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .children(self.images.iter().map(|i| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .p_2()
                            .border_1()
                            .border_color(border)
                            .rounded_md()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .flex_1()
                                    .child(format!("{}:{}", i.repository, i.tag)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(i.size.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(i.created.clone()),
                            )
                    })),
            )
            .into_any_element()
    }

    // ---- logs ---------------------------------------------------------------
    fn render_logs(
        &self,
        theme: &gpui_component::Theme,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &self.sessions[idx];
        let this_ent = cx.entity();
        // Render logs via GPUI's native selectable TextView (markdown code
        // block) for proper text selection without terminal wrap artifacts.
        let content = format!("```\n{}\n```", s.log_lines.join("\n"));
        v_flex()
            .id(format!("docker-logs-scroll-{}", idx))
            .flex_1()
            .min_h_0()
            .size_full()
            .child(
                h_flex().justify_end().px_2().py_1().child(
                    Button::new(format!("docker-logs-copy-{}", idx))
                        .xsmall()
                        .ghost()
                        .label("复制全部")
                        .on_click(move |_, _window, cx| {
                            let _ = this_ent.update(cx, |this, cx| {
                                if let Some(s) = this.sessions.get(idx) {
                                    let text = s.log_lines.join("\n");
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                                }
                            });
                        }),
                ),
            )
            .child(
                div()
                    .id(format!("docker-logs-text-{}", idx))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .bg(theme.muted)
                    .text_size(px(11.))
                    .child(
                        TextView::markdown(format!("docker-logs-md-{}", idx), content)
                            .selectable(true),
                    ),
            )
            .into_any_element()
    }

    // ---- shell --------------------------------------------------------------
    fn render_shell(&self, theme: &gpui_component::Theme, idx: usize) -> AnyElement {
        let s = &self.sessions[idx];
        match &s.terminal {
            Some(term) => div()
                .id(format!("docker-exec-term-{}", idx))
                .flex_1()
                .min_h_0()
                .size_full()
                .child(term.clone())
                .into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .bg(theme.background)
                .child("正在启动 Shell…")
                .into_any_element(),
        }
    }

    // ---- hosts --------------------------------------------------------------
    fn render_hosts(
        &mut self,
        theme: &gpui_component::Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let active_id = self.active_host_id.clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Docker 主机管理"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("docker-add-host")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label("添加主机")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_host_dialog(None, window, cx);
                                this.reload_hosts();
                                cx.notify();
                            })),
                    ),
            )
            .children(self.error.as_ref().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(e.clone())
            }))
            .child(
                v_flex()
                    .id("docker-hosts-scroll")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .children(self.hosts.iter().map(|h| {
                        let is_active = h.id == active_id;
                        let id = h.id.clone();
                        let id_connect = h.id.clone();
                        let id_edit = h.id.clone();
                        let id_delete = h.id.clone();
                        let is_local = h.is_local();
                        let is_ssh = h.ssh_host_id.is_some();
                        h_flex()
                            .gap_2()
                            .items_center()
                            .p_3()
                            .border_1()
                            .border_color(border)
                            .rounded_md()
                            .bg(if is_active {
                                theme.accent.opacity(0.15)
                            } else {
                                Hsla::transparent_black()
                            })
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(h.name.clone()),
                                    )
                                    .child(div().text_size(px(11.)).text_color(muted).child(
                                        if let Some(ssh_id) = &h.ssh_host_id {
                                            crate::ssh::host_store::find_host(ssh_id)
                                                .map(|s| {
                                                    format!(
                                                        "SSH 隧道 — {}@{}:{}",
                                                        s.username, s.host, s.port
                                                    )
                                                })
                                                .unwrap_or_else(|| {
                                                    "SSH 隧道 (关联主机已删除)".into()
                                                })
                                        } else if h.host.is_empty() {
                                            "(本地 daemon)".to_string()
                                        } else {
                                            h.host.clone()
                                        },
                                    )),
                            )
                            .when(is_ssh, |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.))
                                        .px(px(6.))
                                        .py(px(2.))
                                        .rounded_sm()
                                        .bg(theme.accent.opacity(0.2))
                                        .text_color(theme.accent)
                                        .child("SSH"),
                                )
                            })
                            .when(is_active, |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.))
                                        .px(px(6.))
                                        .py(px(2.))
                                        .rounded_sm()
                                        .bg(theme.primary.opacity(0.2))
                                        .text_color(theme.primary)
                                        .child("已连接"),
                                )
                            })
                            .when(!is_active, |d| {
                                d.child(
                                    Button::new(format!("docker-host-connect-{}", id_connect))
                                        .xsmall()
                                        .primary()
                                        .ghost()
                                        .label("连接")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.switch_host(id_connect.clone(), cx);
                                            this.active_page = Page::Containers;
                                            this.active_session_key = None;
                                            this.containers_loaded = false;
                                            this.reload_containers(cx);
                                        })),
                                )
                            })
                            .when(!is_local, |d| {
                                d.child(
                                    Button::new(format!("docker-host-edit-{}", id_edit))
                                        .xsmall()
                                        .ghost()
                                        .label("编辑")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if find_host(&id_edit).is_some() {
                                                this.open_host_dialog(
                                                    Some(&id_edit),
                                                    window,
                                                    cx,
                                                );
                                                this.reload_hosts();
                                                cx.notify();
                                            }
                                        })),
                                )
                                .child(
                                    Button::new(format!("docker-host-delete-{}", id_delete))
                                        .xsmall()
                                        .ghost()
                                        .label("删除")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            remove_host(&id_delete);
                                            this.reload_hosts();
                                            if this.active_host_id == id_delete {
                                                this.switch_host("local".into(), cx);
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                    })),
            )
            .into_any_element()
    }
}
