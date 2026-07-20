//! Kubernetes panel — the full-bleed "K8s" exclusive view.
//!
//! Shows the current kubeconfig context/cluster, lists pods in the active
//! namespace (via `kubectl get pods`), and supports two actions per pod:
//!   - 日志: stream `kubectl logs -f` into a scrollable view
//!   - 转发: start `kubectl port-forward` in the background (like SSH local forwards)
//!
//! Pod exec (interactive shell) is reserved for a future iteration (it needs a
//! PTY, analogous to the SSH TerminalView).

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};

use crate::k8s::kubectl::{list_pods, port_forward, stream_logs, LogStream, Pod, PortForwardHandle};
use crate::k8s::{kubeconfig_path, load_default, KubeConfig};

/// Which sub-view is active.
#[derive(Clone, PartialEq, Eq)]
enum View {
    Contexts,
    Pods,
    Logs,
    Forwards,
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
    forwards: Vec<PortForwardHandle>,
}

impl K8sPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let namespace_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("default");
            s.set_value("default", window, cx);
            s
        });
        let mut me = Self {
            cfg: load_default(),
            error: None,
            active_view: View::Contexts,
            namespace_input: Some(namespace_input),
            pods: Vec::new(),
            pods_loading: false,
            log_target: None,
            log_lines: Vec::new(),
            log_stream: None,
            forwards: Vec::new(),
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
        self.cfg = load_default();
        self.error = None;
        cx.notify();
    }

    fn switch_context(&mut self, name: String, cx: &mut Context<Self>) {
        let out = std::process::Command::new("kubectl")
            .args(["config", "use-context", &name])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                self.refresh_config(cx);
                self.pods.clear();
            }
            Ok(o) => {
                self.error = Some(
                    String::from_utf8_lossy(&o.stderr).trim().to_string(),
                );
                cx.notify();
            }
            Err(e) => {
                self.error = Some(format!("执行 kubectl 失败: {e}"));
                cx.notify();
            }
        }
    }

    fn set_view(&mut self, v: View, cx: &mut Context<Self>) {
        let leaving_logs = v != View::Logs;
        self.active_view = v;
        // If leaving logs, drop the stream.
        if leaving_logs {
            self.log_stream = None;
        }
        cx.notify();
    }

    fn reload_pods(&mut self, cx: &mut Context<Self>) {
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        self.pods_loading = true;
        self.error = None;
        cx.notify();
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let res = list_pods(ns.as_deref()).await;
            let _ = ent.update(cx, move |panel, cx| {
                panel.pods_loading = false;
                match res {
                    Ok(pods) => panel.pods = pods,
                    Err(e) => panel.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_logs(&mut self, pod: String, cx: &mut Context<Self>) {
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        match stream_logs(&pod, ns.as_deref(), None, 50) {
            Ok(mut s) => {
                let rx = s.take_lines();
                self.log_stream = Some(s);
                self.log_target = Some(pod);
                self.log_lines.clear();
                self.active_view = View::Logs;
                self.error = None;
                if let Some(mut rx) = rx {
                    let ent = cx.entity().downgrade();
                    cx.spawn(async move |_this, cx| loop {
                        match rx.recv().await {
                            Some(line) => {
                                let _ = ent.update(cx, |p, cx| {
                                    if p.log_lines.len() > 1000 {
                                        p.log_lines.remove(0);
                                    }
                                    p.log_lines.push(line);
                                    cx.notify();
                                });
                            }
                            None => break,
                        }
                    })
                    .detach();
                }
                cx.notify();
            }
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
            }
        }
    }

    fn start_forward(&mut self, pod: String, remote_port: u16, cx: &mut Context<Self>) {
        let ns = self
            .namespace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string());
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let res = port_forward(&pod, ns.as_deref(), 0, remote_port).await;
            let _ = ent.update(cx, move |panel, cx| {
                match res {
                    Ok(h) => {
                        let port = h.local_port;
                        panel.forwards.push(h);
                        panel.error = None;
                        log::info!("k8s port-forward {}:{} → localhost:{}", pod, remote_port, port);
                        panel
                            .log_lines
                            .push(format!("✓ 转发 pod/{pod}:{remote_port} → 127.0.0.1:{port}"));
                        panel.active_view = View::Forwards;
                    }
                    Err(e) => {
                        panel.error = Some(e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
        let muted = theme.muted_foreground;
        let border = theme.border;

        let active_ctx = self.cfg.active_context().cloned();
        let active_server = active_ctx
            .as_ref()
            .and_then(|c| self.cfg.cluster_for(&c.name))
            .map(|cl| cl.server.clone())
            .unwrap_or_else(|| "(未配置)".into());
        let active_name = self.cfg.current_context.clone();

        let ns_input = self.namespace_input.as_ref().unwrap().clone();
        let pods_loading = self.pods_loading;
        let active_view = self.active_view.clone();
        let _active_server = active_server.clone();

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Kubernetes"))
                    .child(div().flex_1())
                    .child(
                        Button::new("k8s-pods")
                            .ghost()
                            .small()
                            .label("Pods")
                            .disabled(active_view == View::Pods)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(View::Pods, cx);
                                if this.pods.is_empty() && !this.pods_loading {
                                    this.reload_pods(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("k8s-ctx")
                            .ghost()
                            .small()
                            .label("Contexts")
                            .disabled(active_view == View::Contexts)
                            .on_click(cx.listener(|this, _, _, cx| this.set_view(View::Contexts, cx))),
                    )
                    .child(
                        Button::new("k8s-fwd")
                            .ghost()
                            .small()
                            .label(&format!("转发 ({})", self.forwards.len()))
                            .disabled(active_view == View::Forwards)
                            .on_click(cx.listener(|this, _, _, cx| this.set_view(View::Forwards, cx))),
                    )
                    .child(
                        Button::new("k8s-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::Loader)
                            .label("刷新")
                            .on_click(cx.listener(|this, _, _, cx| match this.active_view {
                                View::Contexts => this.refresh_config(cx),
                                View::Pods => this.reload_pods(cx),
                                _ => cx.notify(),
                            })),
                    ),
            )
            .child(match self.active_view {
                View::Contexts => self.render_contexts(&theme, cx).into_any_element(),
                View::Pods => self.render_pods(&theme, ns_input.clone(), pods_loading, cx).into_any_element(),
                View::Logs => self.render_logs(&theme, cx).into_any_element(),
                View::Forwards => self.render_forwards(&theme, cx).into_any_element(),
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
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("可用 Context (点击切换)"),
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
            .children(self.forwards.iter().enumerate().map(|(i, f)| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .p_2()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .child(div().text_size(px(12.)).flex_1().child(format!("127.0.0.1:{} → {}", f.local_port, f.target)))
            }))
            .into_any_element()
    }
}
