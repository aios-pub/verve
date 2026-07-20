//! Docker management panel — the full-bleed "Docker" exclusive view.
//!
//! Lists containers and images, supports start/stop/restart/rm, streams logs,
//! opens interactive exec. All docker CLI calls are synchronous std::process
//! invocations; we run them on background threads via smol::unblock so they
//! never block the UI.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};
use tokio::io::AsyncWriteExt;

use crate::docker::{
    exec_container, list_containers, list_images, prune_images, remove as docker_remove,
    restart as docker_restart, start as docker_start, stop as docker_stop, stream_logs, Container,
    DockerError, ExecSession, ImageInfo, LogStream,
};
use crate::ssh::terminal::TerminalView;

#[derive(Clone, PartialEq, Eq)]
enum View {
    Containers,
    Images,
    Logs,
    Exec,
}

pub struct DockerPanel {
    error: Option<String>,
    active_view: View,
    containers: Vec<Container>,
    containers_loading: bool,
    images: Vec<ImageInfo>,
    images_loading: bool,
    log_target: Option<String>,
    log_lines: Vec<String>,
    log_stream: Option<LogStream>,
    exec_target: Option<String>,
    exec_terminal: Option<Entity<TerminalView>>,
    exec_stdin: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    exec_child: Option<tokio::process::Child>,
}

impl DockerPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            error: None,
            active_view: View::Containers,
            containers: Vec::new(),
            containers_loading: false,
            images: Vec::new(),
            images_loading: false,
            log_target: None,
            log_lines: Vec::new(),
            log_stream: None,
            exec_target: None,
            exec_terminal: None,
            exec_stdin: None,
            exec_child: None,
        }
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
            if let Some(mut c) = self.exec_child.take() {
                let _ = c.start_kill();
            }
        }
        cx.notify();
    }

    /// Run blocking work on a std::thread and return results via a std channel,
    /// then apply them on the UI thread inside cx.spawn. Uses std::sync::mpsc
    /// (not tokio) so no runtime is required.
    fn run_blocking<F, T>(&mut self, cx: &mut Context<Self>, f: F, on_done: impl 'static + Send + FnOnce(&mut Self, &mut Context<Self>, T))
    where
        F: 'static + Send + FnOnce() -> T,
        T: 'static + Send,
    {
        let (tx, rx) = std::sync::mpsc::channel::<T>();
        std::thread::spawn(move || {
            let result = f();
            let _ = tx.send(result);
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            // Poll the std receiver on the smol executor without blocking.
            loop {
                match rx.try_recv() {
                    Ok(result) => {
                        let _ = ent.update(cx, move |panel, cx| on_done(panel, cx, result));
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

    fn reload_containers(&mut self, cx: &mut Context<Self>) {
        if self.containers_loading {
            return;
        }
        self.containers_loading = true;
        self.error = None;
        cx.notify();
        self.run_blocking(cx, list_containers, |panel, cx, res| {
            panel.containers_loading = false;
            match res {
                Ok(cs) => panel.containers = cs,
                Err(e) => panel.error = Some(e.to_string()),
            }
            cx.notify();
        });
    }

    fn reload_images(&mut self, cx: &mut Context<Self>) {
        if self.images_loading {
            return;
        }
        self.images_loading = true;
        self.error = None;
        cx.notify();
        self.run_blocking(cx, list_images, |panel, cx, res| {
            panel.images_loading = false;
            match res {
                Ok(imgs) => panel.images = imgs,
                Err(e) => panel.error = Some(e.to_string()),
            }
            cx.notify();
        });
    }

    fn do_prune(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, DockerError>>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(prune_images());
            let _ = tx.send(result);
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(result) => {
                        let _ = ent.update(cx, |panel, cx| {
                            panel.error = Some(result.unwrap_or_else(|e| e.to_string()));
                            panel.reload_images(cx);
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

    fn container_action<F>(&mut self, id: String, f: F, cx: &mut Context<Self>)
    where
        F: 'static + Send + Fn(&str) -> Result<(), DockerError>,
    {
        self.run_blocking(cx, move || f(&id), |panel, cx, res| {
            if let Err(e) = res {
                panel.error = Some(e.to_string());
            }
            panel.reload_containers(cx);
        });
    }

    fn start_logs(&mut self, id: String, cx: &mut Context<Self>) {
        enum Ev {
            Started(String),
            Line(String),
            Err(String),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let id_for_thread = id.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match stream_logs(&id_for_thread, 50) {
                    Ok(mut stream) => {
                        let Some(mut lines_rx) = stream.take_lines() else { return };
                        let _ = tx.send(Ev::Started(id_for_thread));
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
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(ev) => {
                        let _ = ent.update(cx, |p, cx| match ev {
                            Ev::Started(id) => {
                                p.log_target = Some(id);
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
            }
        })
        .detach();
    }

    fn start_exec(&mut self, id: String, _window: &mut Window, cx: &mut Context<Self>) {
        enum Ev {
            Ready {
                stdin_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
                data_rx: std::sync::mpsc::Receiver<Vec<u8>>,
            },
            Err(String),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        let id_for_thread = id.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match exec_container(&id_for_thread, None) {
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
                        // Keep sess.child alive by leaking it into this runtime;
                        // drop happens when exec is closed via set_view.
                        std::mem::forget(sess.child);
                        let _ = tx.send(Ev::Ready { stdin_tx, data_rx });
                        // Keep runtime alive until streams end (spawned tasks above exit).
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
        let term_id = id.clone();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Ev::Ready { stdin_tx, data_rx }) => {
                        // Create the terminal and wire stdin.
                        let term_opt: Option<Entity<TerminalView>> = ent
                            .update(cx, |p, cx| {
                                let terminal = cx.new(|cx| TerminalView::new(120, 32, cx));
                                p.exec_stdin = Some(stdin_tx.clone());
                                p.exec_terminal = Some(terminal.clone());
                                p.exec_target = Some(term_id.clone());
                                p.active_view = View::Exec;
                                p.error = None;
                                cx.notify();
                                // Wire the terminal's input_tx to our stdin sender.
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
                            // Pump data to the terminal via an inner spawn on the term entity.
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
}

impl Render for DockerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let active_view = self.active_view.clone();

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
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Docker"))
                    .child(div().flex_1())
                    .child(
                        Button::new("docker-containers")
                            .ghost()
                            .small()
                            .label("Containers")
                            .when(active_view == View::Containers, |b| b.disabled(true))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(View::Containers, cx);
                                if this.containers.is_empty() && !this.containers_loading {
                                    this.reload_containers(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("docker-images")
                            .ghost()
                            .small()
                            .label("Images")
                            .when(active_view == View::Images, |b| b.disabled(true))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view(View::Images, cx);
                                if this.images.is_empty() && !this.images_loading {
                                    this.reload_images(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("docker-reload")
                            .ghost()
                            .small()
                            .icon(IconName::Loader)
                            .label("刷新")
                            .on_click(cx.listener(|this, _, _, cx| match this.active_view {
                                View::Containers => this.reload_containers(cx),
                                View::Images => this.reload_images(cx),
                                _ => cx.notify(),
                            })),
                    ),
            )
            .child(match self.active_view {
                View::Containers => self.render_containers(&theme, cx).into_any_element(),
                View::Images => self.render_images(&theme, cx).into_any_element(),
                View::Logs => self.render_logs(&theme).into_any_element(),
                View::Exec => self.render_exec(&theme).into_any_element(),
            })
    }
}

impl DockerPanel {
    fn render_containers(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let loading = self.containers_loading;

        let rows = v_flex().id("docker-ct-scroll").overflow_y_scroll().flex_1().min_h_0().gap_1();

        let body = if self.containers.is_empty() && !loading {
            rows.child(div().text_size(px(12.)).text_color(muted).child(
                "点“刷新”列出容器。需要 Docker daemon 正在运行,docker CLI 在 PATH 中。",
            ))
        } else {
            rows.children(self.containers.iter().map(|c| {
                let state_color = match c.state.as_str() {
                    "running" => theme.primary,
                    "exited" => muted,
                    "paused" => theme.accent,
                    _ => muted,
                };
                let id_short: String = c.id.chars().take(12).collect();
                let is_running = c.state == "running";
                let cid_log = c.id.clone();
                let cid_exec = c.id.clone();
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
                    .child(div().text_size(px(11.)).w(px(90.)).text_color(muted).child(id_short))
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
                                this.start_logs(cid_log.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new(format!("docker-exec-{}", c.id))
                            .xsmall()
                            .ghost()
                            .label("Shell")
                            .disabled(!is_running)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_exec(cid_exec.clone(), window, cx);
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
                                this.container_action(cid_rm.clone(), |id| docker_remove(id, true), cx);
                            })),
                    )
            }))
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_2()
            .children(self.error.as_ref().map(|e| {
                div().text_size(px(12.)).text_color(theme.danger).child(e.clone())
            }))
            .child(if loading {
                div().text_size(px(12.)).text_color(muted).child("加载中…").into_any_element()
            } else {
                div().into_any_element()
            })
            .child(body)
            .into_any_element()
    }

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
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("docker-prune")
                            .small()
                            .ghost()
                            .icon(IconName::Delete)
                            .label("清理未用镜像")
                            .on_click(cx.listener(|this, _, _, cx| this.do_prune(cx))),
                    ),
            )
            .children(self.error.as_ref().map(|e| {
                div().text_size(px(12.)).text_color(theme.danger).child(e.clone())
            }))
            .child(if loading {
                div().text_size(px(12.)).text_color(muted).child("加载中…").into_any_element()
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
                            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).flex_1().child(format!("{}:{}", i.repository, i.tag)))
                            .child(div().text_size(px(11.)).text_color(muted).child(i.size.clone()))
                            .child(div().text_size(px(10.)).text_color(muted).child(i.created.clone()))
                    })),
            )
            .into_any_element()
    }

    fn render_logs(&self, theme: &gpui_component::Theme) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let target = self.log_target.clone().unwrap_or_else(|| "(未选中)".into());
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
                    .child(div().text_size(px(12.)).text_color(muted).child("Container"))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(target)),
            )
            .child(div().h(px(1.)).w_full().bg(border))
            .child(
                v_flex()
                    .id("docker-logs-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .bg(theme.muted)
                    .children(self.log_lines.iter().map(|l| {
                        div()
                            .text_size(px(11.))
                            .font_family(theme.mono_font_family.clone())
                            .child(l.clone())
                    })),
            )
            .into_any_element()
    }

    fn render_exec(&self, theme: &gpui_component::Theme) -> AnyElement {
        let muted = theme.muted_foreground;
        let border = theme.border;
        let target = self.exec_target.clone().unwrap_or_else(|| "(未连接)".into());
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
                    .child(div().text_size(px(12.)).text_color(muted).child("Container"))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(target)),
            )
            .child(div().h(px(1.)).w_full().bg(border))
            .child(
                div()
                    .id("docker-exec-term")
                    .flex_1()
                    .min_h_0()
                    .bg(theme.muted)
                    .children(self.exec_terminal.clone().map(|t| t.into_any_element())),
            )
            .into_any_element()
    }
}
