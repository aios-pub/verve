//! Stress testing panel — the full-bleed "压测" exclusive view.
//!
//! Lets the user pick a target request from the active project, configure
//! concurrency/duration/QPS, and run a load test. Single-URL targets use oha;
//! multi-step scenarios use the built-in scenario runner. Results and a live
//! snapshot (completed count, elapsed, RPS) are displayed once the run ends.
//!
//! This iteration keeps the UI minimal: a form with numeric inputs, a start/stop
//! button, and a result summary card.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};

use crate::state::{AppEvent, AppState};
use crate::stress::{
    find_or_download, run_oha, run_scenario, StressConfig, StressResult, StressSnapshot,
    StressStopHandle, StressTarget,
};

pub struct StressPanel {
    pub state: Entity<AppState>,
    // --- config inputs (created lazily since they need &Window) ---
    concurrency: Option<Entity<InputState>>,
    duration_secs: Option<Entity<InputState>>,
    qps: Option<Entity<InputState>>,
    // --- runtime state ---
    running: bool,
    snapshot: Option<StressSnapshot>,
    result: Option<StressResult>,
    error: Option<String>,
    handle: Option<StressStopHandle>,
    _subs: Vec<gpui::Subscription>,
}

impl StressPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&state, |_this, _src, _ev: &AppEvent, _cx| {
            // Project changes don't affect an in-flight run; we just re-render on next poll.
        });

        let concurrency = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("50");
            s.set_value("50", window, cx);
            s
        });
        let duration_secs = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("10");
            s.set_value("10", window, cx);
            s
        });
        let qps = cx.new(|cx| InputState::new(window, cx).placeholder("不限制"));

        Self {
            state,
            concurrency: Some(concurrency),
            duration_secs: Some(duration_secs),
            qps: Some(qps),
            running: false,
            snapshot: None,
            result: None,
            error: None,
            handle: None,
            _subs: vec![sub],
        }
    }

    fn build_config(&self, cx: &App) -> Option<(StressConfig, String)> {
        let project = self.state.read(cx).active_project()?.clone();
        let concurrency: u32 = self
            .concurrency
            .as_ref()?
            .read(cx)
            .value()
            .trim()
            .parse()
            .ok()?;
        let duration_secs: u32 = self
            .duration_secs
            .as_ref()?
            .read(cx)
            .value()
            .trim()
            .parse()
            .ok()?;
        let qps_str = self.qps.as_ref()?.read(cx).value();
        let qps = if qps_str.trim().is_empty() {
            None
        } else {
            Some(qps_str.trim().parse::<u32>().ok()?)
        };
        if concurrency == 0 || duration_secs == 0 {
            return None;
        }
        // For v0.3 we only target the currently-selected request in the main workbench.
        let selected = self.state.read(cx).selected_request.clone()?;
        Some((
            StressConfig {
                target: StressTarget::Request(selected),
                concurrency,
                duration_secs,
                qps,
                think_ms: 0,
            },
            project
                .find_request(&self.state.read(cx).selected_request.clone()?)
                .map(|(_, r)| r.name.clone())?,
        ))
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.result = None;
        self.error = None;
        self.snapshot = None;

        let Some((cfg, _target_name)) = self.build_config(cx) else {
            self.error = Some(
                "请先在左侧选中一个请求，并填写并发数/持续秒数（正整数）".into(),
            );
            cx.notify();
            return;
        };

        let oha_path = match find_or_download() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };

        let http_client = cx.http_client();

        let project = match self.state.read(cx).active_project().cloned() {
            Some(p) => p,
            None => {
                self.error = Some("未选中项目".into());
                cx.notify();
                return;
            }
        };

        self.running = true;

        // Resolve the target request (only single-request stress in this iteration).
        let req_id = match &cfg.target {
            StressTarget::Request(id) => id.clone(),
            StressTarget::TestCase(_) => {
                self.error = Some("多步骤场景压测请直接在自动化测试面板运行".into());
                self.running = false;
                cx.notify();
                return;
            }
        };
        let Some((_, req)) = project.find_request(&req_id) else {
            self.error = Some("请求不存在".into());
            self.running = false;
            cx.notify();
            return;
        };
        let req = req.clone();
        let vars = crate::stress::oha::effective_vars(&project);
        let c = cfg.concurrency;
        let d = cfg.duration_secs;
        let q = cfg.qps;

        let entity = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let result = run_oha(&oha_path, &req, &vars, c, d, q).await;
            let _ = entity.update(cx, move |panel, cx| {
                panel.running = false;
                panel.handle = None;
                match result {
                    Ok(report) => {
                        panel.result = Some(StressResult::from(report));
                    }
                    Err(e) => {
                        panel.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();

        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(h) = self.handle.take() {
            h.stop();
        }
        // The oha path uses kill_on_drop, but we ran it detached; we flag stop here.
        // (Full graceful stop for oha is via the scenario runner only in this iteration.)
        self.running = false;
        cx.notify();
    }
}

impl Render for StressPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Ensure inputs exist (defensive: they're created in new(), but be safe).
        if self.concurrency.is_none() {
            let c = cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder("50");
                s.set_value("50", window, cx);
                s
            });
            self.concurrency = Some(c);
        }
        if self.duration_secs.is_none() {
            let d = cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder("10");
                s.set_value("10", window, cx);
                s
            });
            self.duration_secs = Some(d);
        }
        if self.qps.is_none() {
            self.qps = Some(cx.new(|cx| InputState::new(window, cx).placeholder("不限制")));
        }

        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;

        let concurrency = self.concurrency.as_ref().unwrap().clone();
        let duration_secs = self.duration_secs.as_ref().unwrap().clone();
        let qps = self.qps.as_ref().unwrap().clone();

        let running = self.running;
        let has_project = self.state.read(cx).active_project().is_some();
        let selected_name = self
            .state
            .read(cx)
            .selected_request
            .as_ref()
            .and_then(|id| {
                self.state
                    .read(cx)
                    .active_project()
                    .and_then(|p| p.find_request(id))
                    .map(|(_, r)| r.name.clone())
            })
            .unwrap_or_else(|| "(未选中请求)".into());

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
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("压测"))
                    .child(div().flex_1())
                    .child(if running {
                        Button::new("stress-stop")
                            .small()
                            .danger()
                            .icon(IconName::Close)
                            .label("停止")
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                            .into_any_element()
                    } else {
                        Button::new("stress-start")
                            .small()
                            .primary()
                            .icon(IconName::Play)
                            .label("开始压测")
                            .disabled(!has_project || running)
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                            .into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .id("stress-scroll")
                    .p_4()
                    .gap_3()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child("目标请求：在左侧接口树中选中一个接口后再点开始。当前选中："),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(selected_name),
                    )
                    .child(div().h(px(1.)).w_full().bg(border))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_end()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("并发数"))
                                    .child(Input::new(&concurrency).small().w(px(110.))),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("持续 (秒)"))
                                    .child(Input::new(&duration_secs).small().w(px(110.))),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child("QPS 上限 (可选)"))
                                    .child(Input::new(&qps).small().w(px(130.))),
                            ),
                    )
                    // Snapshot / result.
                    .child(div().h(px(1.)).w_full().bg(border))
                    .when(self.running, |c| {
                        c.child(
                            v_flex()
                                .gap_1()
                                .child(div().text_sm().child("压测进行中…"))
                                .child(if let Some(snap) = &self.snapshot {
                                    div().text_xs().text_color(muted).child(format!(
                                        "已完成 {} 次, {:.1}s",
                                        snap.completed, snap.elapsed_secs
                                    ))
                                } else {
                                    div().text_xs().text_color(muted).child("正在启动 oha…")
                                }),
                        )
                    })
                    .when(!self.running, |c| {
                        c.when(self.result.is_none() && self.error.is_none(), |c| {
                            c.child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("配置参数后点“开始压测”。结果显示于此。"),
                            )
                        })
                    })
                    .children(self.error.as_ref().map(|e| {
                        div().text_size(px(12.)).text_color(theme.danger).child(format!("错误：{e}"))
                    }))
                    .children(self.result.as_ref().map(|r| render_result(r, &theme))),
            )
    }
}

fn render_result(r: &StressResult, theme: &gpui_component::Theme) -> AnyElement {
    let lat = &r.latency_ms;
    v_flex()
        .gap_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("结果"))
        .child(
            h_flex()
                .gap_4()
                .child(stat("总请求".into(), format!("{}", r.total)))
                .child(stat("成功".into(), format!("{}", r.success)))
                .child(stat("失败".into(), format!("{}", r.failed)))
                .child(stat("QPS".into(), format!("{:.1}", r.rps))),
        )
        .child(
            h_flex()
                .gap_4()
                .child(stat("p50 (ms)".into(), format!("{:.0}", lat.p50)))
                .child(stat("p95 (ms)".into(), format!("{:.0}", lat.p95)))
                .child(stat("p99 (ms)".into(), format!("{:.0}", lat.p99)))
                .child(stat("max (ms)".into(), format!("{:.0}", lat.max))),
        )
        .when(!r.status_distribution.is_empty(), |c| {
            c.child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child("状态码分布"))
                    .children(r.status_distribution.iter().map(|(code, n)| {
                        div().text_size(px(12.)).child(format!("{code}: {n}"))
                    })),
            )
        })
        .into_any_element()
}

fn stat(label: String, value: String) -> AnyElement {
    v_flex()
        .gap_0()
        .child(div().text_xs().text_color(gpui::Hsla::default()).child(label))
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(value))
        .into_any_element()
}
