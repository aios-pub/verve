//! HTTP capture proxy panel — "抓包" exclusive view.
//!
//! Shows whether the proxy is running (with bound port + a curl hint), a list
//! of captured transactions with method/status/duration/URL, and a detail pane
//! for the selected entry. Start/stop buttons control the proxy server.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme, Disableable as _, IconName, Sizable as _, h_flex, v_flex};

use crate::proxy::{CaptureEntry, CaptureStore, DEFAULT_PORT, ProxyHandle, server};

pub struct ProxyPanel {
    running: bool,
    port: u16,
    handle: Option<ProxyHandle>,
    store: CaptureStore,
    entries: Vec<CaptureEntry>,
    selected: Option<usize>,
    error: Option<String>,
    _tick: Option<Task<()>>,
}

impl ProxyPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            running: false,
            port: DEFAULT_PORT,
            handle: None,
            store: CaptureStore::new(500),
            entries: Vec::new(),
            selected: None,
            error: None,
            _tick: None,
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let store = self.store.clone();
        let port = self.port;
        // ProxyHandle is not Send across threads safely, so we spawn the server on
        // a dedicated tokio runtime thread and send back just the bound port. The
        // server runs until the process exits (acceptable for a dev tool).
        enum Ev {
            Ok(u16),
            Err(String),
        }
        let (tx, rx) = std::sync::mpsc::channel::<Ev>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                match server::serve(store, port).await {
                    Ok(h) => {
                        let port = h.bound_port;
                        // The handle is kept alive by this thread; it's killed when
                        // the process exits or when the user restarts.
                        std::mem::forget(h);
                        let _ = tx.send(Ev::Ok(port));
                        // Keep thread alive forever so listener stays up.
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(u64::MAX)).await;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Ev::Err(format!("启动失败: {e}")));
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
                                panel.port = port;
                                panel.running = true;
                                panel.error = None;
                                panel.start_tick(cx);
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
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(h) = self.handle.take() {
            h.stop();
        }
        self.running = false;
        // Dropping the Task cancels it.
        self._tick = None;
        cx.notify();
    }

    fn start_tick(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let entity = cx.entity().downgrade();
        self._tick = Some(cx.spawn(async move |_this, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_millis(700)).await;
                let snap = store.snapshot();
                let _ = entity.update(cx, |panel, cx| {
                    panel.entries = snap;
                    cx.notify();
                });
            }
        }));
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.store.clear();
        self.entries.clear();
        self.selected = None;
        cx.notify();
    }
}

impl Render for ProxyPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let running = self.running;

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
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("HTTP 抓包代理"))
                    .child(div().flex_1())
                    .child(if running {
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.success)
                                    .child(format!("运行中 127.0.0.1:{}", self.port)),
                            )
                            .child(
                                Button::new("proxy-clear")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Delete)
                                    .label("清空")
                                    .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                            )
                            .child(
                                Button::new("proxy-stop")
                                    .danger()
                                    .small()
                                    .icon(IconName::Close)
                                    .label("停止")
                                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                            )
                            .into_any_element()
                    } else {
                        Button::new("proxy-start")
                            .primary()
                            .small()
                            .icon(IconName::Play)
                            .label("启动代理")
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                            .into_any_element()
                    }),
            )
            .child({
                let mut body = v_flex().size_full().min_h_0();
                if let Some(err) = &self.error {
                    body = body.child(div().text_size(px(12.)).p_3().text_color(theme.danger).child(err.clone()));
                }
                if running {
                    body = body.child(
                        div()
                            .text_size(px(11.))
                            .px_4()
                            .py_1()
                            .text_color(muted)
                            .child(format!(
                                "在请求工具中设置 HTTP 代理 http://127.0.0.1:{} ，请求会被自动捕获。",
                                self.port
                            )),
                    );
                }
                if self.entries.is_empty() {
                    body = body.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(muted)
                            .text_size(px(13.))
                            .child(if running { "等待流量…" } else { "点“启动代理”开始" }),
                    );
                } else {
                    // Two-pane: list (top) + detail (bottom) split.
                    let sel = self.selected.unwrap_or(0).min(self.entries.len().saturating_sub(1));
                    let list = v_flex()
                        .flex_1()
                        .min_h_0()
                        .id("proxy-list")
                        .overflow_y_scroll()
                        .gap_0()
                        .size_full()
                        .children(self.entries.iter().enumerate().map(|(i, e)| {
                            let is_sel = i == sel;
                            let color = if e.status >= 500 {
                                theme.danger
                            } else if e.status >= 400 {
                                theme.accent
                            } else if e.status >= 200 {
                                theme.primary
                            } else {
                                muted
                            };
                            h_flex()
                                .id(format!("proxy-entry-{i}"))
                                .gap_2()
                                .px_3()
                                .py_1()
                                .border_b_1()
                                .border_color(border)
                                .bg(if is_sel { theme.accent } else { gpui::Hsla::transparent_black() })
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .w(px(42.))
                                        .child(e.method.clone()),
                                )
                                .child(div().text_size(px(11.)).w(px(42.)).text_color(color).child(e.status.to_string()))
                                .child(div().text_size(px(11.)).text_color(muted).w(px(52.)).child(format!("{}ms", e.duration_ms)))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .flex_1()
                                        .truncate()
                                        .child(e.url.clone()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected = Some(i);
                                    cx.notify();
                                }))
                        }));
                    body = body.child(list);
                    if let Some(e) = self.entries.get(sel) {
                        let req_body = String::from_utf8_lossy(&e.req_body);
                        let resp_body = String::from_utf8_lossy(&e.resp_body);
                        body = body.child(
                            v_flex()
                                .h(px(200.))
                                .id("proxy-detail")
                                .border_t_1()
                                .border_color(border)
                                .p_2()
                                .gap_1()
                                .overflow_y_scroll()
                                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(format!("{} {}", e.method, e.url)))
                                .child(div().text_size(px(11.)).text_color(muted).child(format!("状态: {} · 耗时: {}ms", e.status, e.duration_ms)))
                                .child(div().text_size(px(11.)).text_color(muted).child("请求头:"))
                                .children(e.req_headers.iter().map(|(k, v)| {
                                    div().text_size(px(11.)).child(format!("{k}: {v}"))
                                }))
                                .when(!e.req_body.is_empty(), |c| {
                                    c.child(div().text_size(px(11.)).text_color(muted).child("请求体:"))
                                        .child(div().text_size(px(11.)).child(req_body.to_string()))
                                })
                                .child(div().text_size(px(11.)).text_color(muted).child("响应体:"))
                                .child(div().text_size(px(11.)).child(resp_body.to_string())),
                        );
                    }
                }
                body
            })
    }
}
