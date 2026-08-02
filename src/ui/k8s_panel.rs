//! Kubernetes panel — the full-bleed "K8s" exclusive view.
//!
//! Shows the current kubeconfig context/cluster, lists pods in the active
//! namespace (via `kubectl get pods`), and supports two actions per pod:
//!   - 日志: stream `kubectl logs -f` into a scrollable view
//!   - 转发: start `kubectl port-forward` in the background (like SSH local forwards)
//!
//! Pod exec (interactive shell) is reserved for a future iteration (it needs a
//! PTY, analogous to the SSH TerminalView).

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectState};
use gpui_component::{ActiveTheme, Disableable as _, IconName, Sizable as _, h_flex, v_flex};
use tokio::io::AsyncWriteExt;

use crate::k8s::clusters::{
    ClusterAuth, ManualCluster, load_manual_clusters, remove_manual_cluster,
};
use crate::k8s::kubectl::{
    ExecSession, LogStream, Pod, exec_pod, list_namespaces, list_pods, port_forward, stream_logs,
};
use crate::k8s::{KubeConfig, kubeconfig_path, reload_merged_kubeconfig};
use crate::ssh::terminal::TerminalView;
use crate::ui::k8s_cluster_dialog;
use gpui_component::text::TextView;

/// Which top-level page is active (session tabs overlay these).
#[derive(Clone, PartialEq, Eq)]
pub enum View {
    Contexts,
    Pods,
    Forwards,
}

/// What kind of session a tab represents.
#[derive(Clone, PartialEq, Eq)]
enum SessionKind {
    Logs,
    Shell,
}

/// An open k8s session tab: streaming logs for a pod, or an interactive shell.
/// Sessions persist across Contexts/Pods/Forwards page switches — switching back
/// to a tab just re-shows its UI without reconnecting.
struct Session {
    kind: SessionKind,
    pod_name: String,
    namespace: String,
    // Logs state.
    log_lines: Vec<String>,
    log_stream: Option<LogStream>,
    // Shell state.
    terminal: Option<Entity<TerminalView>>,
    stdin_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
}

impl Session {
    fn new_logs(pod: String, ns: String) -> Self {
        Self {
            kind: SessionKind::Logs,
            pod_name: pod,
            namespace: ns,
            log_lines: Vec::new(),
            log_stream: None,
            terminal: None,
            stdin_tx: None,
        }
    }
    fn new_shell(pod: String, ns: String) -> Self {
        Self {
            kind: SessionKind::Shell,
            pod_name: pod,
            namespace: ns,
            log_lines: Vec::new(),
            log_stream: None,
            terminal: None,
            stdin_tx: None,
        }
    }
    fn title(&self) -> String {
        let label = match self.kind {
            SessionKind::Logs => "日志",
            SessionKind::Shell => "Shell",
        };
        format!("{label} · {}", self.pod_name)
    }
}

fn session_key(pod: &str, ns: &str, kind: &SessionKind) -> String {
    let k = match kind {
        SessionKind::Logs => "logs",
        SessionKind::Shell => "shell",
    };
    format!("{pod}|{ns}|{k}")
}

pub struct K8sPanel {
    cfg: KubeConfig,
    pub error: Option<String>,
    active_view: View,
    // --- sessions (tabs) ---
    sessions: Vec<Session>,
    active_session_key: Option<String>,
    // --- pods ---
    namespace_select: Option<Entity<SelectState<Vec<String>>>>,
    pub namespaces: Vec<String>,
    namespaces_loaded: bool,
    /// Currently selected namespace filter:
    /// - None → all namespaces (`kubectl get pods -A`)
    /// - Some("") → all namespaces (placeholder)
    /// - Some("name") → specific namespace
    pub selected_namespace: Option<String>,
    search_input: Option<Entity<InputState>>,
    pub pods: Vec<Pod>,
    pub pods_loading: bool,
    // --- forwards ---
    forwards: Vec<(u16, String)>, // (local_port, target_description)
}

impl K8sPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let namespace_select = cx.new(|cx| {
            SelectState::new(
                vec!["所有 Namespaces".to_string()],
                Some(gpui_component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("按名称搜索 Pod..."));
        Self {
            cfg: reload_merged_kubeconfig(),
            error: None,
            active_view: View::Contexts,
            sessions: Vec::new(),
            active_session_key: None,
            namespace_select: Some(namespace_select),
            namespaces: vec!["所有 Namespaces".to_string()],
            namespaces_loaded: false,
            selected_namespace: None, // default: all namespaces
            search_input: Some(search_input),
            pods: Vec::new(),
            pods_loading: false,
            forwards: Vec::new(),
        }
    }

    /// Fetch namespaces from the cluster and populate the dropdown.
    fn load_namespaces(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let ctx = self.current_context_name().map(String::from);
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<String>, String>>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt
                .block_on(list_namespaces(ctx.as_deref()))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Ok(mut ns_list)) => {
                        let _ = ent.update(cx, |panel, cx| {
                            ns_list.sort();
                            let mut options = vec!["所有 Namespaces".to_string()];
                            options.extend(ns_list.iter().cloned());
                            panel.namespaces = options.clone();
                            // Clear select so it gets recreated on next render with new options
                            panel.namespace_select = None;
                            panel.selected_namespace = None;
                            panel.namespaces_loaded = true;
                            cx.notify();
                        });
                        break;
                    }
                    Ok(Err(e)) => {
                        log::warn!("Failed to list namespaces: {e}");
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        smol::Timer::after(std::time::Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    /// Reload the merged kubeconfig into `self.cfg`, clearing any error.
    ///
    /// IMPORTANT: we regenerate the Verve-managed kubeconfig YAML from the
    /// manual-cluster index (`kube_clusters.json`) before reading. The index is
    /// the source of truth for Verve-added clusters, but the YAML
    /// (`kube_config.yaml`) can drift out of sync if a cluster was renamed or
    /// removed outside of the normal mutation path, or if the YAML was edited by
    /// hand. Without regenerating, stale entries linger in `kube_config.yaml`
    /// and `load_merged()` keeps returning them — which makes the "刷新" button
    /// look like a no-op and hides edit/delete buttons (they match on name).
    fn refresh_config(&mut self, cx: &mut Context<Self>) {
        self.cfg = reload_merged_kubeconfig();
        self.error = None;
        cx.notify();
    }

    /// Display name of the active kubectl context, for the title-bar badge.
    pub fn active_context_display(&self) -> String {
        self.current_context_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(未配置)".into())
    }

    /// Number of configured contexts (clusters), for the title-bar badge.
    pub fn context_count(&self) -> usize {
        self.cfg.contexts.len()
    }

    /// Number of active port-forwards, shown in the Forwards tab label.
    pub fn forward_count(&self) -> usize {
        self.forwards.len()
    }

    /// Which sub-view is currently active (for title-bar tab highlighting).
    pub fn current_view(&self) -> &'static str {
        if self.active_session_key.is_some() {
            return "pods";
        }
        match self.active_view {
            View::Pods => "pods",
            View::Contexts => "contexts",
            View::Forwards => "forwards",
        }
    }

    /// Called by app.rs when the user clicks the K8s sidebar item.
    /// If a cluster/context is already configured, jump straight to the Pods view
    /// and auto-trigger a pod reload so the user sees workloads immediately.
    /// Otherwise leave the panel on the Contexts view so they can add one.
    pub fn activate(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Always refresh config in case clusters were added externally.
        self.cfg = reload_merged_kubeconfig();
        if self.current_context_name().is_some() && !self.cfg.contexts.is_empty() {
            self.set_view(View::Pods, cx);
            // Only trigger a single load to avoid racing two concurrent SSH
            // tunnel opens (which caused duplicate tunnels and a panic when the
            // Runtime inside the losing tunnel was dropped from an async context).
            // Namespaces are loaded lazily when the user clicks the "加载" button.
            if self.pods.is_empty() && !self.pods_loading {
                self.reload_pods(cx);
            }
        } else {
            self.set_view(View::Contexts, cx);
        }
        cx.notify();
    }

    /// Switch to the pods view. Does NOT auto-load data — user must click "Load".
    pub fn show_pods(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_view(View::Pods, cx);
        // Do not auto-load namespaces or pods; lazy load when user clicks "Load" button
    }

    /// Switch to the contexts view.
    pub fn show_contexts(&mut self, cx: &mut Context<Self>) {
        self.set_view(View::Contexts, cx);
    }

    /// Switch to the forwards view.
    pub fn show_forwards(&mut self, cx: &mut Context<Self>) {
        self.set_view(View::Forwards, cx);
    }

    /// Refresh the current view (contexts/pods).
    pub fn refresh_current(&mut self, cx: &mut Context<Self>) {
        match self.active_view {
            View::Contexts => self.refresh_config(cx),
            View::Pods => self.reload_pods(cx),
            _ => cx.notify(),
        }
    }

    /// Return the current context name (passed as --context to kubectl).
    fn current_context_name(&self) -> Option<&str> {
        let name = &self.cfg.current_context;
        if name.is_empty() {
            None
        } else {
            Some(name.as_str())
        }
    }

    fn switch_context(&mut self, name: String, _window: &mut Window, cx: &mut Context<Self>) {
        // Update in-memory state. kubectl commands use --context, so we don't
        // need to modify any kubeconfig file on disk.
        self.cfg.current_context = name.clone();
        // Persist in the verve kubeconfig so the selection survives restarts.
        let mut verve_cfg = crate::k8s::clusters::load_verve_kubeconfig();
        verve_cfg.current_context = name;
        let _ = crate::k8s::clusters::save_verve_kubeconfig(&verve_cfg);
        // Close all sessions (they belong to the old context).
        self.sessions.clear();
        self.active_session_key = None;
        self.pods.clear();
        self.error = None;
        // Reset namespace list - will reload when user clicks Load
        self.namespaces = vec!["所有 Namespaces".to_string()];
        self.namespaces_loaded = false;
        self.namespace_select = None;
        self.selected_namespace = None;
        cx.notify();
    }

    pub fn set_view(&mut self, v: View, cx: &mut Context<Self>) {
        self.active_view = v;
        self.active_session_key = None;
        cx.notify();
    }

    /// Open an existing session tab or create a new one for the given pod/kind.
    fn open_or_focus_session(
        &mut self,
        pod: String,
        namespace: String,
        kind: SessionKind,
        cx: &mut Context<Self>,
    ) -> String {
        let key = session_key(&pod, &namespace, &kind);
        if let Some(pos) = self
            .sessions
            .iter()
            .position(|s| session_key(&s.pod_name, &s.namespace, &s.kind) == key)
        {
            self.active_session_key = Some(key.clone());
            cx.notify();
            return key;
        }
        let session = match kind {
            SessionKind::Logs => Session::new_logs(pod, namespace),
            SessionKind::Shell => Session::new_shell(pod, namespace),
        };
        self.sessions.push(session);
        self.active_session_key = Some(key.clone());
        cx.notify();
        key
    }

    pub fn close_session_by_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .sessions
            .iter()
            .position(|s| session_key(&s.pod_name, &s.namespace, &s.kind) == key)
        {
            // Drop stdin sender to signal the pump thread to exit.
            self.sessions[pos].stdin_tx = None;
            self.sessions.remove(pos);
            if self.active_session_key.as_deref() == Some(key) {
                self.active_session_key = None;
            }
            cx.notify();
        }
    }

    /// Close the currently-active pod session. Used by the global cmd-w
    /// (CloseFile) action. No-op if no session is active.
    pub fn close_active_session(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.active_session_key.clone() {
            self.close_session_by_key(&key, cx);
        }
    }

    fn active_session_idx(&self) -> Option<usize> {
        let key = self.active_session_key.as_ref()?;
        self.sessions
            .iter()
            .position(|s| session_key(&s.pod_name, &s.namespace, &s.kind) == *key)
    }

    fn start_exec_for(
        &mut self,
        pod: String,
        pod_namespace: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key =
            self.open_or_focus_session(pod.clone(), pod_namespace.clone(), SessionKind::Shell, cx);
        // If session already has a terminal, it's already connected — just focus it.
        if let Some(s) = self
            .sessions
            .iter()
            .find(|s| session_key(&s.pod_name, &s.namespace, &s.kind) == key)
        {
            if s.terminal.is_some() {
                return;
            }
        }
        // When viewing all namespaces (-A), use pod's own namespace
        let ns = Some(pod_namespace);
        let ctx = self.current_context_name().map(String::from);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<ExecSession, String>>();
        let pod_for_thread = pod.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                exec_pod(&pod_for_thread, ns.as_deref(), None, ctx.as_deref())
            }));
            let msg = match result {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("kubectl exec 启动异常".into()),
            };
            let _ = ready_tx.send(msg);
        });

        let ent = cx.entity().downgrade();
        let key_for_setup = key.clone();
        cx.spawn(async move |_this, cx| {
            let sess = match ready_rx.await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    let _ = ent.update(cx, |p, cx| {
                        p.close_session_by_key(&key_for_setup, cx);
                        p.error = Some(e);
                        cx.notify();
                    });
                    return;
                }
                Err(_) => {
                    let _ = ent.update(cx, |p, cx| {
                        p.close_session_by_key(&key_for_setup, cx);
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
            let stdin_tx_term = stdin_tx.clone();
            let stdin_tx_panel = stdin_tx;
            let mut output_rx = output_rx;
            let (term_tx, term_rx) = tokio::sync::oneshot::channel::<Entity<TerminalView>>();

            let _ = ent.update(cx, |p, cx| {
                if let Some(s) = p
                    .sessions
                    .iter_mut()
                    .find(|s| session_key(&s.pod_name, &s.namespace, &s.kind) == key_for_setup)
                {
                    let terminal = cx.new(|cx| TerminalView::new(120, 32, cx));
                    s.terminal = Some(terminal.clone());
                    s.stdin_tx = Some(stdin_tx_panel);
                    _ = terminal.update(cx, |tv, _cx| {
                        if let Ok(mut slot) = tv.input_tx.lock() {
                            *slot = Some(stdin_tx_term);
                        }
                    });
                    p.error = None;
                    cx.notify();
                    let _ = term_tx.send(terminal);
                }
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

    pub fn reload_pods(&mut self, cx: &mut Context<Self>) {
        // Guard against double-invocation (rapid clicks / activate() + button click)
        if self.pods_loading {
            return;
        }
        // Namespace filter: None → all namespaces (-A), Some(name) → specific namespace
        let ns = self.selected_namespace.clone();
        let ctx = self.current_context_name().map(String::from);
        self.pods_loading = true;
        self.error = None;
        cx.notify();
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<Pod>, String>>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            // Pass None or empty string for all namespaces
            let ns_arg = match &ns {
                Some(s) if s == "所有 Namespaces" => None,
                Some(s) => Some(s.as_str()),
                None => None,
            };
            let res = rt
                .block_on(list_pods(ns_arg, ctx.as_deref()))
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(res) => {
                        let _ = ent.update(cx, |panel, cx| {
                            panel.pods_loading = false;
                            match res {
                                Ok(pods) => panel.pods = pods,
                                Err(e) => panel.error = Some(e),
                            }
                            cx.notify();
                        });
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        smol::Timer::after(std::time::Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    fn start_logs_for(
        &mut self,
        pod: String,
        pod_namespace: Option<String>,
        cx: &mut Context<Self>,
    ) {
        enum Ev {
            Started,
            Line(String),
            Err(String),
        }
        // Use pod's own namespace when provided; fall back to filter namespace
        let ns = pod_namespace.or(self.selected_namespace.clone());
        let ns_for_session = ns.clone().unwrap_or_default();
        let ctx = self.current_context_name().map(String::from);
        let key = self.open_or_focus_session(pod.clone(), ns_for_session, SessionKind::Logs, cx);
        // If this session already has log lines, already streaming — just focus.
        if let Some(s) = self
            .sessions
            .iter()
            .find(|s| session_key(&s.pod_name, &s.namespace, &s.kind) == key)
        {
            if !s.log_lines.is_empty() || s.log_stream.is_some() {
                return;
            }
        }
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let pod_for_thread = pod;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match stream_logs(&pod_for_thread, ns.as_deref(), None, 50, ctx.as_deref()) {
                    Ok(mut s) => {
                        let Some(mut lines_rx) = s.take_lines() else {
                            return;
                        };
                        let _ = tx.send(Ev::Started);
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
                    Ok(ev) => {
                        let _ = ent.update(cx, |p, cx| match ev {
                            Ev::Started => {
                                if let Some(s) = p.sessions.iter_mut().find(|s| {
                                    session_key(&s.pod_name, &s.namespace, &s.kind)
                                        == key_for_updates
                                }) {
                                    s.log_lines.clear();
                                }
                                p.error = None;
                                cx.notify();
                            }
                            Ev::Line(line) => {
                                if let Some(s) = p.sessions.iter_mut().find(|s| {
                                    session_key(&s.pod_name, &s.namespace, &s.kind)
                                        == key_for_updates
                                }) {
                                    if s.log_lines.len() > 2000 {
                                        s.log_lines.remove(0);
                                    }
                                    s.log_lines.push(line);
                                }
                                cx.notify();
                            }
                            Ev::Err(e) => {
                                p.error = Some(e);
                                cx.notify();
                            }
                        });
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        smol::Timer::after(std::time::Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    fn start_forward(
        &mut self,
        pod: String,
        pod_namespace: Option<String>,
        remote_port: u16,
        cx: &mut Context<Self>,
    ) {
        enum Ev {
            Ok(u16),
            Err(String),
        }
        let ns = pod_namespace.or(self.selected_namespace.clone());
        let ctx = self.current_context_name().map(String::from);
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let pod_for_thread = pod.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match port_forward(
                    &pod_for_thread,
                    ns.as_deref(),
                    0,
                    remote_port,
                    ctx.as_deref(),
                )
                .await
                {
                    Ok(h) => {
                        let port = h.local_port;
                        log::info!(
                            "k8s port-forward {}:{} → localhost:{}",
                            pod_for_thread,
                            remote_port,
                            port
                        );
                        // Leak the handle so the child process lives until app exit;
                        // kill_on_drop(true) would fire when this future exits otherwise.
                        std::mem::forget(h);
                        let _ = tx.send(Ev::Ok(port));
                    }
                    Err(e) => {
                        let _ = tx.send(Ev::Err(e.to_string()));
                    }
                }
            });
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(ev) => {
                        let _ = ent.update(cx, |panel, cx| match ev {
                            Ev::Ok(port) => {
                                let target = format!("pod/{pod}:{remote_port}");
                                panel.error = None;
                                panel.forwards.push((port, target.clone()));
                                log::info!("✓ k8s port-forward {target} → 127.0.0.1:{port}");
                                panel.active_view = View::Forwards;
                                cx.notify();
                            }
                            Ev::Err(e) => {
                                panel.error = Some(e);
                                cx.notify();
                            }
                        });
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        smol::Timer::after(std::time::Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
        cx.notify();
    }
}

impl Render for K8sPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let pods_loading = self.pods_loading;

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
                    let key = session_key(&s.pod_name, &s.namespace, &s.kind);
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
                            Button::new(format!("k8s-session-focus-{}", i))
                                .ghost()
                                .xsmall()
                                .label(s.title())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.active_session_key = Some(key_focus.clone());
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(format!("k8s-session-close-{}", i))
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
                SessionKind::Logs => self.render_logs_session(&theme, idx, cx),
                SessionKind::Shell => self.render_shell_session(&theme, idx),
            }
        } else {
            match self.active_view {
                View::Contexts => self.render_contexts(&theme, cx).into_any_element(),
                View::Pods => self
                    .render_pods(&theme, pods_loading, window, cx)
                    .into_any_element(),
                View::Forwards => self.render_forwards(&theme, cx).into_any_element(),
            }
        };

        v_flex().size_full().min_h_0().child(tab_bar).child(body)
    }
}

impl K8sPanel {
    fn render_contexts(&self, theme: &gpui_component::Theme, cx: &mut Context<Self>) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        // Load manual clusters to detect SSH tunnel mode
        let manual_clusters = load_manual_clusters();
        let is_ssh_tunnel: std::collections::HashMap<&str, bool> = manual_clusters
            .iter()
            .map(|c| (c.name.as_str(), c.uses_ssh_tunnel()))
            .collect();

        let active_server = self
            .cfg
            .active_context()
            .and_then(|c| self.cfg.cluster_for(&c.name))
            .map(|cl| cl.server.clone())
            .unwrap_or_else(|| "(未配置)".into());
        let active_ns = self
            .cfg
            .active_context()
            .and_then(|c| c.context.namespace.clone())
            .unwrap_or_else(|| "default".into());
        v_flex()
            .flex_1()
            .min_h_0()
            .id("k8s-ctx-scroll")
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child("当前 Context"),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if self.cfg.current_context.is_empty() {
                                "(无)".into()
                            } else {
                                self.cfg.current_context.clone()
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_6()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_size(px(11.)).text_color(muted).child("Server"))
                            .child(div().text_size(px(12.)).child(active_server.clone())),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child("Namespace"),
                            )
                            .child(div().text_size(px(12.)).child(active_ns)),
                    ),
            )
            .children(self.error.as_ref().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(e.clone())
            }))
            .child(div().h(px(1.)).w_full().bg(border))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_1()
                            .child("可用 Context (点击切换)"),
                    )
                    .child(
                        Button::new("k8s-add-cluster")
                            .xsmall()
                            .primary()
                            .icon(IconName::Plus)
                            .label("添加集群")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let ent = cx.entity().downgrade();
                                k8s_cluster_dialog::open_dialog(
                                    None,
                                    move |_window, cx| {
                                        let _ = ent.update(cx, |panel, cx| {
                                            panel.refresh_config(cx);
                                        });
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .children(if self.cfg.contexts.is_empty() {
                Some(div().text_size(px(12.)).text_color(muted).child(format!(
                    "未找到 kubeconfig ({}).",
                    kubeconfig_path().display()
                )))
            } else {
                None
            })
            .children(self.cfg.contexts.iter().map(|c| {
                let is_active = c.name == self.cfg.current_context;
                let server = self
                    .cfg
                    .cluster_for(&c.name)
                    .map(|cl| cl.server.clone())
                    .unwrap_or_default();
                let name = c.name.clone();
                let is_tunnel = is_ssh_tunnel.get(c.name.as_str()).copied().unwrap_or(false);
                // Find the manual cluster (if it's a Verve-added cluster, not from ~/.kube/config)
                let manual_cluster = manual_clusters.iter().find(|mc| mc.name == c.name).cloned();
                let is_manual = manual_cluster.is_some();

                h_flex()
                    .gap_2()
                    .items_center()
                    .id(c.name.clone())
                    .p_2()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .bg(if is_active {
                        theme.accent.opacity(0.15)
                    } else {
                        Hsla::transparent_black()
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(if is_active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(c.name.clone()),
                            )
                            .when(is_tunnel, |row| {
                                row.child(
                                    div()
                                        .text_size(px(9.))
                                        .px(px(4.))
                                        .py(px(1.))
                                        .rounded_sm()
                                        .bg(theme.primary.opacity(0.2))
                                        .text_color(theme.primary)
                                        .child("SSH 隧道"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(server.clone()),
                    )
                    .when(!is_active, |d| {
                        d.child(
                            Button::new(format!("k8s-switch-{}", c.name))
                                .xsmall()
                                .ghost()
                                .label("切换")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.switch_context(name.clone(), window, cx);
                                })),
                        )
                    })
                    // Edit/Delete buttons only for manual (Verve-added) clusters
                    .when_some(manual_cluster, |d, mc| {
                        let edit_cluster = mc.clone();
                        let delete_id = mc.id.clone();
                        let delete_id_for_del = delete_id.clone();
                        let deleted_ctx_name = c.name.clone();
                        d.child(
                            Button::new(format!("k8s-edit-{}", c.name))
                                .xsmall()
                                .ghost()
                                .label("编辑")
                                .tooltip("编辑集群")
                                .on_click(cx.listener(move |_this, _, window, cx| {
                                    let ent = cx.entity().downgrade();
                                    let edit_c = edit_cluster.clone();
                                    let close_id = delete_id.clone();
                                    k8s_cluster_dialog::open_dialog(
                                        Some(&edit_c),
                                        move |_window, cx| {
                                            let _ = ent.update(cx, |panel, cx| {
                                                // Close any open tunnel for this cluster before refreshing
                                                crate::k8s::kubectl::close_ssh_tunnel(&close_id);
                                                panel.refresh_config(cx);
                                            });
                                        },
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new(format!("k8s-delete-{}", c.name))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Delete)
                                .tooltip("删除集群")
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    remove_manual_cluster(&delete_id_for_del);
                                    crate::k8s::kubectl::close_ssh_tunnel(&delete_id_for_del);
                                    // If we just deleted the active context, clear it
                                    if this.cfg.current_context == deleted_ctx_name {
                                        this.cfg.current_context = String::new();
                                    }
                                    this.refresh_config(cx);
                                })),
                        )
                    })
            }))
            .into_any_element()
    }

    fn render_pods(
        &mut self,
        theme: &gpui_component::Theme,
        loading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;

        // Lazily create namespace select and search input
        if self.namespace_select.is_none() {
            let sel = cx.new(|cx| {
                SelectState::new(
                    self.namespaces.clone(),
                    Some(gpui_component::IndexPath::new(0)),
                    window,
                    cx,
                )
            });
            self.namespace_select = Some(sel);
        }
        if self.search_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("按名称搜索..."));
            self.search_input = Some(input);
        }

        let ns_select = self.namespace_select.as_ref().unwrap().clone();
        let search_input = self.search_input.as_ref().unwrap().clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Select::new(&ns_select).small().w(px(200.)))
                    .child(Input::new(&search_input).small().w(px(200.)))
                    .child(
                        Button::new("k8s-reload-pods")
                            .small()
                            .primary()
                            .icon(if loading {
                                IconName::Loader
                            } else {
                                IconName::File
                            })
                            .label(if loading { "加载中…" } else { "加载" })
                            .disabled(loading)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // Lazy load namespaces on first load click
                                if !this.namespaces_loaded {
                                    this.load_namespaces(window, cx);
                                }
                                // Read selected namespace from dropdown on click
                                if let Some(sel) = &this.namespace_select {
                                    let val = sel.read(cx).selected_value();
                                    this.selected_namespace = match val {
                                        Some(s) if s == "所有 Namespaces" => None,
                                        Some(s) => Some(s.clone()),
                                        None => None,
                                    };
                                }
                                this.reload_pods(cx);
                            })),
                    ),
            )
            .children(self.error.as_ref().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(e.clone())
            }))
            .child(div().h(px(1.)).w_full().bg(border))
            .child({
                let mut rows = v_flex()
                    .id("k8s-pods-scroll")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .gap_1();
                let search_query = search_input.read(cx).value().to_string().to_lowercase();
                // Collect and filter pods by search query
                let filtered_pods: Vec<&Pod> = self
                    .pods
                    .iter()
                    .filter(|p| p.metadata.name.to_lowercase().contains(&search_query))
                    .collect();

                if self.pods.is_empty() && !loading {
                    rows = rows.child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child("点\"加载\"列出所有命名空间的 Pods"),
                    );
                } else if filtered_pods.is_empty() && !search_query.is_empty() {
                    rows = rows.child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child(format!("未找到匹配 \"{}\" 的 Pod", search_query)),
                    );
                } else {
                    let show_ns_column = self.selected_namespace.is_none(); // show NS column when viewing all
                    for p in filtered_pods {
                        let name = p.metadata.name.clone();
                        let pod_ns = p.metadata.namespace.clone();
                        let phase = p.status.phase.clone();
                        let restarts: u32 = p
                            .status
                            .container_statuses
                            .iter()
                            .map(|c| c.restart_count)
                            .sum();
                        let ready = p
                            .status
                            .container_statuses
                            .iter()
                            .filter(|c| c.ready)
                            .count();
                        let total = p.status.container_statuses.len();
                        let phase_color = match phase.as_str() {
                            "Running" => theme.primary,
                            "Pending" => theme.accent,
                            "Failed" => theme.danger,
                            _ => muted,
                        };
                        let logs_name = name.clone();
                        let logs_ns = pod_ns.clone();
                        let fwd_name = name.clone();
                        let fwd_ns = pod_ns.clone();
                        let exec_name = name.clone();
                        let exec_ns = pod_ns.clone();
                        let mut row = h_flex()
                            .gap_2()
                            .items_center()
                            .p_2()
                            .border_1()
                            .border_color(border)
                            .rounded_md();

                        // Show namespace when viewing all namespaces
                        if show_ns_column {
                            row = row.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .w(px(100.))
                                    .child(pod_ns.clone()),
                            );
                        }

                        row = row
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .flex_1()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded_sm()
                                    .bg(phase_color.opacity(0.18))
                                    .text_color(phase_color)
                                    .child(phase.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(format!("{ready}/{total} ready · {restarts}↻")),
                            )
                            .child(
                                Button::new(format!("k8s-logs-{}", name))
                                    .xsmall()
                                    .ghost()
                                    .label("日志")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_logs_for(
                                            logs_name.clone(),
                                            Some(logs_ns.clone()),
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(format!("k8s-exec-{}", name))
                                    .xsmall()
                                    .ghost()
                                    .label("Shell")
                                    .tooltip("kubectl exec -it 进入 Pod")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_exec_for(
                                            exec_name.clone(),
                                            exec_ns.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new(format!("k8s-fwd-{}", name))
                                    .xsmall()
                                    .ghost()
                                    .label("转发 80→8080")
                                    .tooltip("转发 pod/xxx:8080 到 127.0.0.1:<port>")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_forward(
                                            fwd_name.clone(),
                                            Some(fwd_ns.clone()),
                                            8080,
                                            cx,
                                        );
                                    })),
                            );
                        rows = rows.child(row);
                    }
                }
                rows.into_any_element()
            })
            .into_any_element()
    }

    fn render_logs_session(
        &self,
        theme: &gpui_component::Theme,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = &self.sessions[idx];
        v_flex()
            .flex_1()
            .min_h_0()
            .child({
                let lines = s.log_lines.clone();
                let session_idx = idx;
                h_flex().justify_end().px_2().py_1().child(
                    Button::new(format!("k8s-logs-copy-{}", session_idx))
                        .xsmall()
                        .ghost()
                        .label("复制全部")
                        .on_click(move |_, _window, cx| {
                            let text = lines.join("\n");
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        }),
                )
            })
            .child({
                let content = format!("```\n{}\n```", s.log_lines.join("\n"));
                div()
                    .id(("k8s-logs-text", idx))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .bg(theme.muted)
                    .text_size(px(11.))
                    .child(
                        TextView::markdown(format!("k8s-logs-md-{}", idx), content)
                            .selectable(true),
                    )
            })
            .into_any_element()
    }

    fn render_shell_session(&self, theme: &gpui_component::Theme, idx: usize) -> AnyElement {
        let s = &self.sessions[idx];
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id(("k8s-exec-term", idx))
                    .flex_1()
                    .min_h_0()
                    .bg(theme.muted)
                    .children(s.terminal.clone().map(|t| t.into_any_element())),
            )
            .into_any_element()
    }

    fn render_forwards(
        &self,
        theme: &gpui_component::Theme,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        v_flex()
            .flex_1()
            .min_h_0()
            .id("k8s-fwd-scroll")
            .overflow_y_scroll()
            .p_4()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("活跃 Port-Forward"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("从 Pods 视图点 \"转发\" 启动。关闭应用会终止所有转发。"),
            )
            .child(div().h(px(1.)).w_full().bg(border))
            .children(if self.forwards.is_empty() {
                Some(
                    div()
                        .text_size(px(12.))
                        .text_color(muted)
                        .child("暂无活跃转发"),
                )
            } else {
                None
            })
            .children(self.forwards.iter().map(|(port, target)| {
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
                            .flex_1()
                            .child(format!("127.0.0.1:{} → {}", port, target)),
                    )
            }))
            .into_any_element()
    }
}
