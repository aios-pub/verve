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
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};
use tokio::io::AsyncWriteExt;

use crate::k8s::clusters::{
    import_kubeconfig as import_kubeconfig_file, load_manual_clusters, ClusterAuth, ManualCluster,
};
use crate::k8s::kubectl::{exec_pod, list_pods, port_forward, stream_logs, LogStream, Pod};
use crate::k8s::{kubeconfig_path, load_merged, KubeConfig};
use crate::ssh::terminal::TerminalView;
use crate::ui::k8s_cluster_dialog;

/// Which sub-view is active.
#[derive(Clone, PartialEq, Eq)]
enum View {
    Contexts,
    Pods,
    Logs,
    Forwards,
    Exec,
}

pub struct K8sPanel {
    cfg: KubeConfig,
    error: Option<String>,
    active_view: View,
    // --- pods ---
    namespace_input: Option<Entity<InputState>>,
    pods: Vec<Pod>,
    pods_loading: bool,
    // --- logs ---
    log_target: Option<String>,
    log_lines: Vec<String>,
    log_stream: Option<LogStream>,
    // --- forwards ---
    forwards: Vec<(u16, String)>, // (local_port, target_description)
    // --- exec ---
    exec_target: Option<String>,
    exec_terminal: Option<Entity<TerminalView>>,
    exec_stdin: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
}

impl K8sPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let namespace_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("default");
            s.set_value("default", window, cx);
            s
        });
        let mut me = Self {
            cfg: load_merged(),
            error: None,
            active_view: View::Contexts,
            namespace_input: Some(namespace_input),
            pods: Vec::new(),
            pods_loading: false,
            log_target: None,
            log_lines: Vec::new(),
            log_stream: None,
            forwards: Vec::new(),
            exec_target: None,
            exec_terminal: None,
            exec_stdin: None,
        };
        me.maybe_default_namespace();
        me
    }

    fn maybe_default_namespace(&mut self) {
        if let Some(ctx) = self.cfg.active_context() {
            let ns = ctx.context.namespace.clone().unwrap_or_else(|| "default".into());
            if let Some(input) = &self.namespace_input {
                // Value is already set; nothing to do unless it's still the placeholder.
                let _ = (input, ns);
            }
        }
    }

    fn refresh_config(&mut self, cx: &mut Context<Self>) {
        self.cfg = load_merged();
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
        match self.active_view {
            View::Pods => "pods",
            View::Contexts => "contexts",
            View::Forwards => "forwards",
            View::Logs | View::Exec => "pods",
        }
    }

    /// Switch to the pods view and reload if empty.
    pub fn show_pods(&mut self, cx: &mut Context<Self>) {
        self.set_view(View::Pods, cx);
        if self.pods.is_empty() && !self.pods_loading {
            self.reload_pods(cx);
        }
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

    /// Open the kubeconfig import dialog from the title bar.
    pub fn open_import_kubeconfig(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import_kubeconfig_dialog(window, cx);
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

    fn switch_context(&mut self, name: String, cx: &mut Context<Self>) {
        // Update in-memory state. kubectl commands use --context, so we don't
        // need to modify any kubeconfig file on disk.
        self.cfg.current_context = name.clone();
        // Persist in the verve kubeconfig so the selection survives restarts.
        let mut verve_cfg = crate::k8s::clusters::load_verve_kubeconfig();
        verve_cfg.current_context = name;
        let _ = crate::k8s::clusters::save_verve_kubeconfig(&verve_cfg);
        self.pods.clear();
        self.error = None;
        cx.notify();
    }

    fn set_view(&mut self, v: View, cx: &mut Context<Self>) {
        let leaving_logs = v != View::Logs;
        let leaving_exec = v != View::Exec;
        self.active_view = v;
        if leaving_logs {
            self.log_stream = None;
        }
        if leaving_exec {
            self.exec_stdin = None;
            self.exec_terminal = None;
            // Note: child process lives on the dedicated tokio thread until app exit
        }
        cx.notify();
    }

    fn start_exec(&mut self, pod: String, _window: &mut Window, cx: &mut Context<Self>) {
        enum Ev {
            Ready {
                stdin_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
                data_rx: std::sync::mpsc::Receiver<Vec<u8>>,
            },
            Err(String),
        }
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        let ctx = self.current_context_name().map(String::from);
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let pod_for_thread = pod.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match exec_pod(&pod_for_thread, ns.as_deref(), None, ctx.as_deref()) {
                    Ok(mut sess) => {
                        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
                        let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<u8>>();
                        if let Some(mut cs) = sess.stdin.take() {
                            tokio::spawn(async move {
                                while let Some(bytes) = stdin_rx.recv().await {
                                    if cs.write_all(&bytes).await.is_err() {
                                        break;
                                    }
                                }
                            });
                        }
                        if let Some(stream) = sess.stdout.take() {
                            let data_tx = data_tx.clone();
                            tokio::spawn(async move {
                                use tokio::io::AsyncReadExt;
                                let mut s = stream;
                                let mut buf = [0u8; 4096];
                                loop {
                                    match s.read(&mut buf).await {
                                        Ok(0) | Err(_) => break,
                                        Ok(n) => {
                                            if data_tx.send(buf[..n].to_vec()).is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            });
                        }
                        if let Some(stream) = sess.stderr.take() {
                            let data_tx = data_tx.clone();
                            tokio::spawn(async move {
                                use tokio::io::AsyncReadExt;
                                let mut s = stream;
                                let mut buf = [0u8; 4096];
                                loop {
                                    match s.read(&mut buf).await {
                                        Ok(0) | Err(_) => break,
                                        Ok(n) => {
                                            if data_tx.send(buf[..n].to_vec()).is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            });
                        }
                        // Keep child alive; drop happens when exec view is left.
                        std::mem::forget(sess.child);
                        let _ = tx.send(Ev::Ready { stdin_tx, data_rx });
                        // Keep the runtime alive until streams end.
                        std::future::pending::<()>().await;
                    }
                    Err(e) => {
                        let _ = tx.send(Ev::Err(e.to_string()));
                    }
                }
            });
        });

        // Poll for the Ready event, then wire up the terminal.
        let ent = cx.entity().downgrade();
        let term_pod = pod.clone();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Ev::Ready { stdin_tx, data_rx }) => {
                        let term_opt: Option<Entity<TerminalView>> = ent
                            .update(cx, |p, cx| {
                                let terminal = cx.new(|cx| TerminalView::new(120, 32, cx));
                                p.exec_stdin = Some(stdin_tx.clone());
                                p.exec_terminal = Some(terminal.clone());
                                p.exec_target = Some(term_pod.clone());
                                p.active_view = View::Exec;
                                p.error = None;
                                cx.notify();
                                let tx = stdin_tx.clone();
                                _ = terminal.update(cx, |tv, _cx| {
                                    if let Ok(mut slot) = tv.input_tx.lock() {
                                        *slot = Some(tx);
                                    }
                                });
                                Some(terminal)
                            })
                            .ok()
                            .flatten();
                        if let Some(term) = term_opt {
                            let term_weak = term.downgrade();
                            cx.spawn(async move |cx| {
                                loop {
                                    match data_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                                        Ok(chunk) => {
                                            let _ = term_weak.update(cx, |tv, cx| {
                                                tv.feed(&chunk);
                                                cx.notify();
                                            });
                                        }
                                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                            smol::Timer::after(std::time::Duration::from_millis(30)).await;
                                        }
                                        Err(_) => break,
                                    }
                                }
                            })
                            .detach();
                        }
                        break;
                    }
                    Ok(Ev::Err(e)) => {
                        let _ = ent.update(cx, |p, cx| {
                            p.error = Some(e);
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

    fn reload_pods(&mut self, cx: &mut Context<Self>) {
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        let ctx = self.current_context_name().map(String::from);
        self.pods_loading = true;
        self.error = None;
        cx.notify();
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<Pod>, String>>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let res = rt.block_on(list_pods(ns.as_deref(), ctx.as_deref())).map_err(|e| e.to_string());
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

    fn start_logs(&mut self, pod: String, cx: &mut Context<Self>) {
        enum Ev {
            Started,
            Line(String),
            Err(String),
        }
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        let ctx = self.current_context_name().map(String::from);
        let pod_for_ui = pod.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let pod_for_thread = pod;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match stream_logs(&pod_for_thread, ns.as_deref(), None, 50, ctx.as_deref()) {
                    Ok(mut s) => {
                        let Some(mut lines_rx) = s.take_lines() else { return };
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
        cx.spawn(async move |_this, cx| loop {
            match rx.try_recv() {
                Ok(ev) => {
                    let _ = ent.update(cx, |p, cx| match ev {
                        Ev::Started => {
                            p.log_target = Some(pod_for_ui.clone());
                            p.log_lines.clear();
                            p.active_view = View::Logs;
                            p.error = None;
                            cx.notify();
                        }
                        Ev::Line(line) => {
                            if p.log_lines.len() > 1000 {
                                p.log_lines.remove(0);
                            }
                            p.log_lines.push(line);
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
        })
        .detach();
    }

    fn start_forward(&mut self, pod: String, remote_port: u16, cx: &mut Context<Self>) {
        enum Ev {
            Ok(u16),
            Err(String),
        }
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        let ctx = self.current_context_name().map(String::from);
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let pod_for_thread = pod.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match port_forward(&pod_for_thread, ns.as_deref(), 0, remote_port, ctx.as_deref()).await {
                    Ok(h) => {
                        let port = h.local_port;
                        log::info!("k8s port-forward {}:{} → localhost:{}", pod_for_thread, remote_port, port);
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
                                panel.log_lines.push(format!(
                                    "✓ 转发 {target} → 127.0.0.1:{port}"
                                ));
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

    /// Open a native file picker to import an external kubeconfig file.
    fn import_kubeconfig_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter("kubeconfig", &["yaml", "yml", "conf", "*"])
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let result = import_kubeconfig_file(&path);
                        let _ = ent.update(cx, |panel, cx| {
                            match result {
                                Ok(n) => {
                                    panel.error = Some(format!("成功导入 {n} 个 context"));
                                    panel.refresh_config(cx);
                                }
                                Err(e) => {
                                    panel.error = Some(e);
                                    cx.notify();
                                }
                            }
                        });
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        smol::Timer::after(std::time::Duration::from_millis(50)).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();
    }
}

impl Render for K8sPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.namespace_input.is_none() {
            let ns = cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder("default");
                s.set_value("default", window, cx);
                s
            });
            self.namespace_input = Some(ns);
        }
        let theme = cx.theme().clone();

        let ns_input = self.namespace_input.as_ref().unwrap().clone();
        let pods_loading = self.pods_loading;

        v_flex()
            .size_full()
            .min_h_0()
            .child(match self.active_view {
                View::Contexts => self.render_contexts(&theme, cx).into_any_element(),
                View::Pods => self.render_pods(&theme, ns_input.clone(), pods_loading, cx).into_any_element(),
                View::Logs => self.render_logs(&theme, cx).into_any_element(),
                View::Forwards => self.render_forwards(&theme, cx).into_any_element(),
                View::Exec => self.render_exec(&theme, cx).into_any_element(),
            })
    }
}

impl K8sPanel {
    fn render_contexts(
        &self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
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
                    .child(div().text_size(px(12.)).text_color(muted).child("当前 Context"))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if self.cfg.current_context.is_empty() { "(无)".into() } else { self.cfg.current_context.clone() }),
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
                            .child(div().text_size(px(11.)).text_color(muted).child("Namespace"))
                            .child(div().text_size(px(12.)).child(active_ns)),
                    ),
            )
            .children(self.error.as_ref().map(|e| {
                div().text_size(px(12.)).text_color(theme.danger).child(e.clone())
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
                        Button::new("k8s-import-kubeconfig")
                            .xsmall()
                            .ghost()
                            .label("导入 kubeconfig")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_kubeconfig_dialog(window, cx);
                            })),
                    )
                    .child(
                        Button::new("k8s-add-cluster")
                            .xsmall()
                            .primary()
                            .icon(IconName::Plus)
                            .label("添加集群")
                            .on_click(cx.listener(|this, _, window, cx| {
                                k8s_cluster_dialog::open_dialog(window, cx);
                                this.refresh_config(cx);
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
                h_flex()
                    .gap_2()
                    .items_center()
                    .id(c.name.clone())
                    .p_2()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .bg(if is_active { theme.accent.opacity(0.15) } else { Hsla::transparent_black() })
                    .child(div().text_size(px(12.)).font_weight(if is_active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).flex_1().child(c.name.clone()))
                    .child(div().text_size(px(11.)).text_color(muted).child(server))
                    .when(!is_active, |d| {
                        d.child(
                            Button::new(format!("k8s-switch-{}", c.name))
                                .xsmall()
                                .ghost()
                                .label("切换")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.switch_context(name.clone(), cx);
                                })),
                        )
                    })
            }))
            .into_any_element()
    }

    fn render_pods(
        &self,
        theme: &gpui_component::Theme,
        ns_input: Entity<InputState>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_size(px(12.)).text_color(muted).child("Namespace"))
                    .child(Input::new(&ns_input).small().w(px(180.)))
                    .child(
                        Button::new("k8s-reload-pods")
                            .small()
                            .primary()
                            .icon(if loading { IconName::Loader } else { IconName::File })
                            .label(if loading { "加载中…" } else { "加载" })
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| this.reload_pods(cx))),
                    ),
            )
            .children(self.error.as_ref().map(|e| {
                div().text_size(px(12.)).text_color(theme.danger).child(e.clone())
            }))
            .child(div().h(px(1.)).w_full().bg(border))
            .child({
                let rows = v_flex()
                    .id("k8s-pods-scroll")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .gap_1();
                if self.pods.is_empty() && !loading {
                    rows.child(div().text_size(px(12.)).text_color(muted).child("点\"加载\"列出 Pods"))
                        .into_any_element()
                } else {
                    rows.children(self.pods.iter().map(|p| {
                        let name = p.metadata.name.clone();
                        let phase = p.status.phase.clone();
                        let restarts: u32 = p.status.container_statuses.iter().map(|c| c.restart_count).sum();
                        let ready = p.status.container_statuses.iter().filter(|c| c.ready).count();
                        let total = p.status.container_statuses.len();
                        let phase_color = match phase.as_str() {
                            "Running" => theme.primary,
                            "Pending" => theme.accent,
                            "Failed" => theme.danger,
                            _ => muted,
                        };
                        let logs_name = name.clone();
                        let fwd_name = name.clone();
                        let exec_name = name.clone();
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
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded_sm()
                                    .bg(phase_color.opacity(0.18))
                                    .text_color(phase_color)
                                    .child(phase),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(format!("{ready}/{total} ready · {restarts}↻")),
                            )
                            .child(
                                Button::new(format!("k8s-logs-{}", p.metadata.name))
                                    .xsmall()
                                    .ghost()
                                    .label("日志")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_logs(logs_name.clone(), cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("k8s-exec-{}", p.metadata.name))
                                    .xsmall()
                                    .ghost()
                                    .label("Shell")
                                    .tooltip("kubectl exec -it 进入 Pod")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_exec(exec_name.clone(), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(format!("k8s-fwd-{}", p.metadata.name))
                                    .xsmall()
                                    .ghost()
                                    .label("转发 80→8080")
                                    .tooltip("转发 pod/xxx:8080 到 127.0.0.1:<port>")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_forward(fwd_name.clone(), 8080, cx);
                                    })),
                            )
                    })).into_any_element()
                }
            })
            .into_any_element()
    }

    fn render_logs(&self, theme: &gpui_component::Theme, _cx: &mut Context<Self>) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let target = self
            .log_target
            .clone()
            .unwrap_or_else(|| "(未选择)".into());
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_4()
                    .pt_2()
                    .child(div().text_size(px(12.)).text_color(muted).child("Pod"))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(target)),
            )
            .child(div().h(px(1.)).w_full().bg(border))
            .child(
                v_flex()
                    .id("k8s-logs-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .gap_0()
                    .bg(theme.muted)
                    .children(self.log_lines.iter().map(|line| {
                        div()
                            .text_size(px(11.))
                            .font_family(theme.mono_font_family.clone())
                            .child(line.clone())
                    })),
            )
            .into_any_element()
    }

    fn render_exec(&mut self, theme: &gpui_component::Theme, cx: &mut Context<Self>) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let target = self.exec_target.clone().unwrap_or_else(|| "(未连接)".into());
        // Lazily create a terminal if we don't have one (defensive: start_exec creates it).
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_4()
                    .pt_2()
                    .child(div().text_size(px(12.)).text_color(muted).child("Pod"))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(target)),
            )
            .child(div().h(px(1.)).w_full().bg(border))
            .child(
                div()
                    .id("k8s-exec-term")
                    .flex_1()
                    .min_h_0()
                    .bg(theme.muted)
                    .children(self.exec_terminal.clone().map(|t| t.into_any_element())),
            )
            .into_any_element()
    }

    fn render_forwards(&self, theme: &gpui_component::Theme, _cx: &mut Context<Self>) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        v_flex()
            .flex_1()
            .min_h_0()
            .id("k8s-fwd-scroll")
            .overflow_y_scroll()
            .p_4()
            .gap_2()
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("活跃 Port-Forward"))
            .child(div().text_size(px(12.)).text_color(muted).child("从 Pods 视图点 \"转发\" 启动。关闭应用会终止所有转发。"))
            .child(div().h(px(1.)).w_full().bg(border))
            .children(if self.forwards.is_empty() {
                Some(div().text_size(px(12.)).text_color(muted).child("暂无活跃转发"))
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
                    .child(div().text_size(px(12.)).flex_1().child(format!("127.0.0.1:{} → {}", port, target)))
            }))
            .into_any_element()
    }
}
