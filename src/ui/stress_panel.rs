//! Stress testing panel — the full-bleed "压测" exclusive view.
//!
//! Lets the user pick a target request from a built-in tree selector, configure
//! concurrency/duration/QPS, and run a load test. Single-URL targets use oha;
//! multi-step scenarios use the built-in scenario runner. Results and a live
//! snapshot (completed count, elapsed, RPS) are displayed once the run ends.

use std::collections::BTreeSet;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState};
use gpui_component::progress::Progress;
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _, WindowExt as _};
use gpui_component::chart::{BarChart, LineChart};
use gpui_component::scroll::ScrollableElement as _;

use crate::state::models::{Folder, Project};
use crate::state::{AppEvent, AppState};
use crate::stress::{
    find_or_download, run_oha, StressConfig, StressResult, StressSnapshot,
    StressStopHandle, StressTarget,
};

pub struct StressPanel {
    pub state: Entity<AppState>,
    // --- config inputs (created lazily since they need &Window) ---
    concurrency: Option<Entity<InputState>>,
    duration_secs: Option<Entity<InputState>>,
    qps: Option<Entity<InputState>>,
    timeout_secs: Option<Entity<InputState>>,
    // --- option toggles ---
    disable_keepalive: bool,
    follow_redirects: bool,
    http2: bool,
    // --- target selection tree state ---
    /// Selected target request id (used when StressTarget::Request).
    selected_target: Option<String>,
    /// Expanded folder ids in the tree.
    expanded_folders: BTreeSet<String>,
    /// Whether the request tree is shown.
    tree_visible: bool,
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
        let timeout_secs = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("30");
            s.set_value("30", window, cx);
            s
        });

        // Select currently selected request from the main workbench by default
        let selected_target = state.read(cx).selected_request.clone();

        Self {
            state,
            concurrency: Some(concurrency),
            duration_secs: Some(duration_secs),
            qps: Some(qps),
            timeout_secs: Some(timeout_secs),
            disable_keepalive: false,
            follow_redirects: false,
            http2: false,
            selected_target,
            expanded_folders: BTreeSet::new(),
            tree_visible: true,
            running: false,
            snapshot: None,
            result: None,
            error: None,
            handle: None,
            _subs: vec![sub],
        }
    }

    fn toggle_folder(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        if self.expanded_folders.contains(folder_id) {
            self.expanded_folders.remove(folder_id);
        } else {
            self.expanded_folders.insert(folder_id.to_string());
        }
        cx.notify();
    }

    fn select_target(&mut self, req_id: &str, cx: &mut Context<Self>) {
        self.selected_target = Some(req_id.to_string());
        cx.notify();
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
        let timeout_str = self.timeout_secs.as_ref()?.read(cx).value();
        let timeout_secs: u32 = timeout_str.trim().parse().unwrap_or(30).max(1);
        if concurrency == 0 || duration_secs == 0 {
            return None;
        }
        let target_id = self.selected_target.clone()?;
        Some((
            StressConfig {
                target: StressTarget::Request(target_id.clone()),
                concurrency,
                duration_secs,
                qps,
                think_ms: 0,
                timeout_secs,
                disable_keepalive: self.disable_keepalive,
                follow_redirects: self.follow_redirects,
                http2: self.http2,
            },
            project
                .find_request(&target_id)
                .map(|(_, r)| r.name.clone())?,
        ))
    }

    fn toggle_disable_keepalive(&mut self, val: bool, cx: &mut Context<Self>) {
        self.disable_keepalive = val;
        cx.notify();
    }
    fn toggle_follow_redirects(&mut self, val: bool, cx: &mut Context<Self>) {
        self.follow_redirects = val;
        cx.notify();
    }
    fn toggle_http2(&mut self, val: bool, cx: &mut Context<Self>) {
        self.http2 = val;
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.result = None;
        self.error = None;
        self.snapshot = None;

        let Some((cfg, _target_name)) = self.build_config(cx) else {
            self.error = Some(t("请先在下方接口树中选中一个请求，并填写并发数/持续秒数（正整数）"));
            cx.notify();
            return;
        };

        if let Err(e) = find_or_download() {
            self.error = Some(e);
            cx.notify();
            return;
        }

        let project = match self.state.read(cx).active_project().cloned() {
            Some(p) => p,
            None => {
                self.error = Some(t("未选中项目"));
                cx.notify();
                return;
            }
        };

        self.running = true;

        // Resolve the target request (only single-request stress in this iteration).
        let req_id = match &cfg.target {
            StressTarget::Request(id) => id.clone(),
            StressTarget::TestCase(_) => {
                self.error = Some(t("多步骤场景压测请直接在自动化测试面板运行"));
                self.running = false;
                cx.notify();
                return;
            }
        };
        let Some((_, req)) = project.find_request(&req_id) else {
            self.error = Some(t("请求不存在"));
            self.running = false;
            cx.notify();
            return;
        };
        let req = req.clone();
        let vars = crate::stress::oha::effective_vars(&project);
        let opts = crate::stress::oha::OhaOptions {
            concurrency: cfg.concurrency,
            duration_secs: cfg.duration_secs,
            qps: cfg.qps,
            timeout_secs: cfg.timeout_secs,
            disable_keepalive: cfg.disable_keepalive,
            follow_redirects: cfg.follow_redirects,
            http2: cfg.http2,
        };

        // Kick off the streaming runner (returns a handle with snapshot + result channels).
        let mut handle = match run_oha(&(), &req, &vars, opts) {
            Ok(h) => h,
            Err(e) => {
                self.error = Some(e);
                self.running = false;
                cx.notify();
                return;
            }
        };
        // Take the oneshot receiver out; the handle stays in the panel for stop/snapshots.
        let result_rx = handle.result_rx.take();
        self.handle = Some(handle);

        let entity = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let mut result_rx = match result_rx {
                Some(rx) => rx,
                None => return,
            };
            loop {
                // Drain snapshots by reaching into the panel's handle.
                let _ = entity.update(cx, |panel, cx| {
                    if let Some(h) = panel.handle.as_mut() {
                        while let Some(snap) = h.poll_snapshot() {
                            panel.snapshot = Some(snap);
                            cx.notify();
                        }
                    }
                });
                match result_rx.try_recv() {
                    Ok(result) => {
                        let _ = entity.update(cx, |panel, cx| {
                            panel.running = false;
                            panel.handle = None;
                            panel.result = Some(result);
                            cx.notify();
                        });
                        break;
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        let _ = entity.update(cx, |panel, cx| {
                            panel.running = false;
                            panel.handle = None;
                            panel.error = Some(t("压测任务异常退出"));
                            cx.notify();
                        });
                        break;
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                }
                smol::Timer::after(std::time::Duration::from_millis(150)).await;
            }
        })
        .detach();

        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(h) = self.handle.as_ref() {
            h.stop();
        }
        // Keep the handle in place so the polling task can still receive the
        // partial (or final) StressResult and render it. We optimistically
        // set running=false so the button flips back to "开始压测" immediately;
        // the final result will still arrive and be displayed.
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
        if self.timeout_secs.is_none() {
            let t = cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder("30");
                s.set_value("30", window, cx);
                s
            });
            self.timeout_secs = Some(t);
        }

        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;

        let concurrency = self.concurrency.as_ref().unwrap().clone();
        let duration_secs = self.duration_secs.as_ref().unwrap().clone();
        let qps = self.qps.as_ref().unwrap().clone();
        let timeout_secs = self.timeout_secs.as_ref().unwrap().clone();
        let disable_keepalive = self.disable_keepalive;
        let follow_redirects = self.follow_redirects;
        let http2 = self.http2;

        let running = self.running;
        let has_project = self.state.read(cx).active_project().is_some();

        // Resolve selected target name
        let selected_id = self.selected_target.clone();
        let (selected_name, selected_method) = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| selected_id.as_deref().and_then(|id| p.find_request(id)))
            .map(|(_, r)| (r.name.clone(), Some(r.method)))
            .unwrap_or_else(|| (t("(未选中请求)"), None));

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
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t("压测")))
                    .child(div().flex_1())
                    .child(if running {
                        Button::new("stress-stop")
                            .small()
                            .danger()
                            .icon(IconName::Close)
                            .label(t("停止"))
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                            .into_any_element()
                    } else {
                        Button::new("stress-start")
                            .small()
                            .primary()
                            .icon(IconName::Play)
                            .label(t("开始压测"))
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
                    // Target selection
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t("目标请求")))
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("stress-tree-toggle")
                                            .ghost()
                                            .xsmall()
                                            .icon(if self.tree_visible { IconName::ChevronDown } else { IconName::ChevronRight })
                                            .label(if self.tree_visible { t("收起树") } else { t("展开树") })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.tree_visible = !this.tree_visible;
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(selected_name),
                                    )
                                    .when_some(selected_method, |this, method| {
                                        this.child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(crate::ui::method_colors::badge_color(method, cx))
                                                .child(method.badge_label()),
                                        )
                                    }),
                            )
                            .when(self.tree_visible, |c| {
                                c.child(
                                    div()
                                        .id("stress-request-tree")
                                        .w_full()
                                        .max_h(px(280.))
                                        .border_1()
                                        .border_color(border)
                                        .rounded_md()
                                        .overflow_y_scroll()
                                        .p_1()
                                        .children(
                                            self.state
                                                .read(cx)
                                                .active_project()
                                                .map(|p| render_request_tree(p, &self.expanded_folders, selected_id.as_deref(), cx))
                                                .unwrap_or_else(|| {
                                                    vec![div()
                                                        .text_size(px(12.))
                                                        .text_color(muted)
                                                        .p_2()
                                                        .child(t("请先打开或创建一个项目"))
                                                        .into_any_element()]
                                                }),
                                        ),
                                )
                            }),
                    )
                    .child(div().h(px(1.)).w_full().bg(border))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_end()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child(t("并发数")))
                                    .child(Input::new(&concurrency).small().w(px(110.))),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child(t("持续 (秒)")))
                                    .child(Input::new(&duration_secs).small().w(px(110.))),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child(t("QPS 上限 (可选)")))
                                    .child(Input::new(&qps).small().w(px(130.))),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().text_xs().text_color(muted).child(t("超时 (秒)")))
                                    .child(Input::new(&timeout_secs).small().w(px(110.))),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Checkbox::new("stress-opt-keepalive")
                                            .checked(disable_keepalive)
                                            .on_click(cx.listener(|this, val: &bool, _, cx| {
                                                this.toggle_disable_keepalive(*val, cx)
                                            })),
                                    )
                                    .child(div().text_xs().text_color(muted).child(t("禁用 Keep-Alive"))),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Checkbox::new("stress-opt-redirect")
                                            .checked(follow_redirects)
                                            .on_click(cx.listener(|this, val: &bool, _, cx| {
                                                this.toggle_follow_redirects(*val, cx)
                                            })),
                                    )
                                    .child(div().text_xs().text_color(muted).child(t("跟随重定向"))),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Checkbox::new("stress-opt-http2")
                                            .checked(http2)
                                            .on_click(cx.listener(|this, val: &bool, _, cx| {
                                                this.toggle_http2(*val, cx)
                                            })),
                                    )
                                    .child(div().text_xs().text_color(muted).child(t("强制 HTTP/2"))),
                            ),
                    )
                    // Snapshot / result.
                    .child(div().h(px(1.)).w_full().bg(border))
                    .when(self.running, |c| {
                        c.child(div().text_sm().child(t("压测进行中…")))
                            .when_some(self.snapshot.as_ref(), |c, snap| {
                                let ratio = if snap.duration_secs > 0.0 {
                                    (snap.elapsed_secs / snap.duration_secs).clamp(0.0, 1.0) as f32
                                } else {
                                    0.0
                                };
                                c.child(Progress::new("stress-progress").value(ratio))
                                    .child(render_live_stats(snap, muted))
                                    .child(render_latency_chart(
                                        &snap.latency_bins,
                                        &t("响应时间分布（实时）"),
                                        &theme,
                                    ))
                                    .child(render_rps_bins_chart(
                                        &snap.rps_bins,
                                        snap.timescale_secs,
                                        &theme,
                                    ))
                            })
                            .when(self.snapshot.is_none(), |c| {
                                c.child(div().text_xs().text_color(muted).child(t("正在启动压测…")))
                            })
                    })
                    .when(!self.running, |c| {
                        c.when(self.result.is_none() && self.error.is_none(), |c| {
                            c.child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t("配置参数后点“开始压测”。结果显示于此。")),
                            )
                        })
                    })
                    .children(self.error.as_ref().map(|e| {
                        div().text_size(px(12.)).text_color(theme.danger).child(format!("{}：{}", t("错误"), e))
                    }))
                    .children(self.result.as_ref().map(|r| render_result(r, &theme))),
            )
    }
}

/// Render the request tree (folders + top-level requests).
fn render_request_tree(
    project: &Project,
    expanded: &BTreeSet<String>,
    selected_id: Option<&str>,
    cx: &Context<StressPanel>,
) -> Vec<AnyElement> {
    let mut items = Vec::new();

    // Render folders (recursively)
    for folder in &project.folders {
        render_folder_tree(folder, 0, expanded, selected_id, cx, &mut items);
    }

    // Render top-level requests
    for req in &project.requests {
        let is_selected = selected_id == Some(&req.id);
        let method = req.method;
        let method_color = crate::ui::method_colors::badge_color(method, cx);
        let rid = req.id.clone();
        items.push(
            h_flex()
                .id(format!("stress-top-req-{}", req.id.clone()))
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(if is_selected { cx.theme().accent } else { Hsla::transparent_black() })
                .child(div().w(px(16.))) // spacer for alignment with folder chevron
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(method_color)
                        .w(px(40.))
                        .child(method.badge_label()),
                )
                .child(div().text_size(px(12.)).flex_1().child(req.name.clone()))
                .on_click(cx.listener(move |this, _, _, cx| this.select_target(&rid, cx)))
                .into_any_element(),
        );
    }

    if items.is_empty() {
        items.push(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .p_2()
                .child(t("当前项目暂无请求"))
                .into_any_element(),
        );
    }

    items
}

/// Recursively render a folder and its children.
fn render_folder_tree(
    folder: &Folder,
    depth: usize,
    expanded: &BTreeSet<String>,
    selected_id: Option<&str>,
    cx: &Context<StressPanel>,
    items: &mut Vec<AnyElement>,
) {
    let is_expanded = expanded.contains(&folder.id);
    let fid = folder.id.clone();
    let indent = depth as f32 * 14.;

    // Folder header
    items.push(
        h_flex()
            .id(format!("stress-folder-row-{}", folder.id.clone()))
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .pl(px(indent + 8.))
            .child(
                div()
                    .id(format!("stress-tree-folder-chev-{}", folder.id.clone()))
                    .child(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                    .text_size(px(10.))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(&fid, cx))),
            )
            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(folder.name.clone()))
            .into_any_element(),
    );

    if is_expanded {
        // Child folders
        for sub in &folder.folders {
            render_folder_tree(sub, depth + 1, expanded, selected_id, cx, items);
        }
        // Child requests
        for req in &folder.requests {
            let is_selected = selected_id == Some(&req.id);
            let method = req.method;
            let method_color = crate::ui::method_colors::badge_color(method, cx);
            let rid = req.id.clone();
            items.push(
                h_flex()
                    .id(format!("stress-nested-req-{}", req.id.clone()))
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(if is_selected { cx.theme().accent } else { Hsla::transparent_black() })
                    .pl(px(indent + 8. + 16.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(method_color)
                            .w(px(40.))
                            .child(method.badge_label()),
                    )
                    .child(div().text_size(px(12.)).flex_1().child(req.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_target(&rid, cx)))
                    .into_any_element(),
            );
        }
    }
}

fn render_result(r: &StressResult, theme: &gpui_component::Theme) -> AnyElement {
    let lat = &r.latency_ms;
    let muted = theme.muted_foreground;
    v_flex()
        .gap_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t("结果")))
        .child(
            h_flex()
                .gap_4()
                .flex_wrap()
                .child(stat(t("总请求数"), format!("{}", r.total), muted))
                .child(stat(t("成功"), format!("{}", r.success), muted))
                .child(stat(t("失败"), format!("{}", r.failed), muted))
                .child(stat(t("QPS（每秒请求）"), format!("{:.2}", r.rps), muted))
                .child(stat(t("数据量"), format_bytes(r.total_bytes), muted)),
        )
        .child(
            h_flex()
                .gap_4()
                .flex_wrap()
                .child(stat(t("最快 (ms)"), format!("{:.0}", lat.min), muted))
                .child(stat(t("平均 (ms)"), format!("{:.0}", lat.mean), muted))
                .child(stat(t("最慢 (ms)"), format!("{:.0}", lat.max), muted))
                .child(stat(t("p50 (ms)"), format!("{:.0}", lat.p50), muted))
                .child(stat(t("p95 (ms)"), format!("{:.0}", lat.p95), muted))
                .child(stat(t("p99 (ms)"), format!("{:.0}", lat.p99), muted)),
        )
        .when(!r.response_time_histogram.is_empty(), |c| {
            c.child(render_latency_chart(
                &r.response_time_histogram,
                &t("响应时间分布"),
                theme,
            ))
        })
        .when(!r.rps_bins.is_empty(), |c| {
            c.child(render_rps_bins_chart(&r.rps_bins, r.rps_bins_timescale, theme))
        })
        .when(!r.rps_timeline.is_empty(), |c| {
            c.child(render_rps_chart(&r.rps_timeline, theme))
        })
        .child(render_status_chart(r, theme))
        .when(!r.status_distribution.is_empty(), |c| {
            c.child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("状态码分布（明细）")))
                    .children(r.status_distribution.iter().map(|(code, n)| {
                        div().text_size(px(12.)).child(format!("{code}: {n}"))
                    })),
            )
        })
        .when(!r.errors.is_empty(), |c| {
            c.child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("错误分布")))
                    .children(r.errors.iter().map(|(msg, n)| {
                        div().text_size(px(12.)).child(format!("{msg}: {n}"))
                    })),
            )
        })
        .into_any_element()
}

fn render_live_stats(snap: &StressSnapshot, muted: Hsla) -> AnyElement {
    v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_4()
                .child(stat(t("已完成"), format!("{}", snap.completed), muted))
                .child(stat(t("成功"), format!("{}", snap.success), muted))
                .child(stat(t("失败"), format!("{}", snap.failed), muted))
                .child(stat(t("平均 QPS"), format!("{:.2}", snap.rps), muted))
                .child(stat(t("实时 QPS"), format!("{:.2}", snap.window_rps), muted))
                .child(stat(t("耗时 (s)"), format!("{:.1}", snap.elapsed_secs), muted))
                .child(stat(t("数据量"), format_bytes(snap.total_bytes), muted)),
        )
        .child(
            h_flex()
                .gap_4()
                .child(stat(t("最快 (ms)"), format!("{:.0}", snap.window_min_ms), muted))
                .child(stat(t("平均 (ms)"), format!("{:.0}", snap.window_mean_ms), muted))
                .child(stat(t("最慢 (ms)"), format!("{:.0}", snap.window_max_ms), muted))
                .child(stat(t("p50 (ms)"), format!("{:.0}", snap.p50_ms), muted))
                .child(stat(t("p95 (ms)"), format!("{:.0}", snap.p95_ms), muted))
                .child(stat(t("p99 (ms)"), format!("{:.0}", snap.p99_ms), muted)),
        )
        .into_any_element()
}

fn format_bytes(b: u64) -> String {
    const KI: f64 = 1024.0;
    const MI: f64 = KI * 1024.0;
    const GI: f64 = MI * 1024.0;
    let b = b as f64;
    if b >= GI {
        format!("{:.2} GiB", b / GI)
    } else if b >= MI {
        format!("{:.2} MiB", b / MI)
    } else if b >= KI {
        format!("{:.1} KiB", b / KI)
    } else {
        format!("{} B", b as u64)
    }
}

/// Lightweight i18n helper: identity pass-through for now. If a translation
/// layer is added later, this is the single place to wire it up.
#[inline]
fn t(s: &str) -> String {
    s.to_string()
}

/// A single (bucket_ms, count) datum for the bar chart.
#[derive(Clone)]
struct HistoBar {
    label: gpui::SharedString,
    count: u64,
    bucket_ms: f64,
}

fn render_latency_chart(
    bins: &[(f64, u64)],
    title: &str,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let title: gpui::SharedString = title.to_string().into();
    // Take up to ~30 most-relevant non-empty bins to keep the chart readable.
    let non_empty: Vec<(f64, u64)> = bins.iter().copied().filter(|(_, c)| *c > 0).collect();
    let data: Vec<HistoBar> = non_empty
        .iter()
        .map(|(upper_ms, count)| HistoBar {
            label: format_bucket_label(*upper_ms).into(),
            count: *count,
            bucket_ms: *upper_ms,
        })
        .collect();

    if data.is_empty() {
        let empty_msg: gpui::SharedString = format!("{}（暂无数据）", title).into();
        return div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(empty_msg)
            .into_any_element();
    }

    // Color thresholds: green ≤ 100ms, yellow ≤ 500ms, red otherwise.
    let green = theme.success;
    let yellow = theme.warning;
    let red = theme.danger;
    let chart_id: gpui::SharedString = format!("latency-histo-{}", title).into();
    v_flex()
        .w_full()
        .gap_1()
        .child(div().text_xs().text_color(theme.muted_foreground).child(title.clone()))
        .child(
            div()
                .w_full()
                .h(px(200.))
                .child(
                    BarChart::new(data)
                        .id(chart_id)
                        .band(|d| d.label.clone())
                        .value(|d| d.count as f64)
                        .fill(move |d, _, _, _| {
                            if d.bucket_ms <= 100.0 {
                                green
                            } else if d.bucket_ms <= 500.0 {
                                yellow
                            } else {
                                red
                            }
                        }),
                ),
        )
        .into_any_element()
}

fn format_bucket_label(upper_ms: f64) -> String {
    if upper_ms > 60_000.0 {
        t(">60s")
    } else if upper_ms >= 1000.0 {
        format!("{:.0}s", upper_ms / 1000.0)
    } else {
        format!("{:.0}ms", upper_ms)
    }
}

#[derive(Clone)]
struct RpsPoint {
    t_sec: gpui::SharedString,
    rps: f64,
}

fn render_rps_chart(timeline: &[(f64, f64)], theme: &gpui_component::Theme) -> AnyElement {
    if timeline.len() < 2 {
        return div().into_any_element();
    }
    // Timeline is already sampled at ~1s intervals by the aggregator (oha-style
    // sampling), so each point represents one second of the run. Show integer
    // second labels and keep width modest (60px per second, min 100% width).
    let point_count = timeline.len();
    let chart_width_px = (point_count as f32 * 60.0).max(600.0);
    let data: Vec<RpsPoint> = timeline
        .iter()
        .map(|(t, rps)| RpsPoint {
            t_sec: format!("{}s", t.round() as u64).into(),
            // round to 2 decimals so tooltip doesn't show raw float noise
            rps: (*rps * 100.0).round() / 100.0,
        })
        .collect();

    v_flex()
        .w_full()
        .gap_1()
        .child(div().text_xs().text_color(theme.muted_foreground).child(t("QPS 随时间变化")))
        .child(
            div()
                .w_full()
                .h(px(180.))
                .overflow_x_scrollbar()
                .child(
                    div()
                        .w(px(chart_width_px))
                        .h(px(160.))
                        .child(
                            LineChart::new(data)
                                .id("rps-timeline")
                                .x(|d| d.t_sec.clone())
                                .y(|d| d.rps)
                                .name(t("QPS"))
                                .linear(),
                        ),
                ),
        )
        .into_any_element()
}

/// One bin of the oha-style rolling "Requests / past Xs" bar chart.
#[derive(Clone)]
struct RpsBin {
    /// X-axis label: seconds-ago offset (rounded).
    label: gpui::SharedString,
    /// Number of requests in this bin.
    count: u64,
}

/// Render a 32-bin rolling bar chart matching oha's "Requests / past X" view.
/// `bins` comes from the aggregator in chronological order (oldest → newest).
/// Labels are shown sparsely (every ~8th bin) to avoid crowding.
fn render_rps_bins_chart(
    bins: &[(f64, u64)],
    timescale_secs: f64,
    theme: &gpui_component::Theme,
) -> AnyElement {
    if bins.is_empty() {
        return div().into_any_element();
    }
    let n = bins.len();
    let step = (n / 8).max(1);
    let color = theme.primary;
    let data: Vec<RpsBin> = bins
        .iter()
        .enumerate()
        .map(|(i, (secs_ago, count))| {
            // The aggregator stores bin_center in seconds from "now" going backward.
            // Oldest bin was N ticks ago, newest was ~0 ticks ago. Label by "Xs ago".
            let label = if i % step == 0 || i == n - 1 {
                format!("{:.1}s", secs_ago)
            } else {
                String::new()
            };
            RpsBin {
                label: label.into(),
                count: *count,
            }
        })
        .collect();

    let title: gpui::SharedString = format!("{}（近 {}s）", t("请求量 / 时间窗口"), format_ts(timescale_secs)).into();
    v_flex()
        .w_full()
        .gap_1()
        .child(div().text_xs().text_color(theme.muted_foreground).child(title))
        .child(
            div()
                .w_full()
                .h(px(160.))
                .child(
                    BarChart::new(data)
                        .id("rps-bins")
                        .band(|d| d.label.clone())
                        .value(|d| d.count as f64)
                        .fill(move |_, _, _, _| color),
                ),
        )
        .into_any_element()
}

fn format_ts(s: f64) -> String {
    if s >= 60.0 {
        format!("{:.0}", s)
    } else if s >= 1.0 {
        format!("{:.1}", s)
    } else {
        format!("{}ms", (s * 1000.0) as u64)
    }
}

#[derive(Clone)]
struct StatusBar {
    code: gpui::SharedString,
    count: u64,
    is_success: bool,
}

/// Render the status code distribution as a small bar chart.
fn render_status_chart(r: &StressResult, theme: &gpui_component::Theme) -> AnyElement {
    if r.status_distribution.is_empty() {
        return div().into_any_element();
    }
    let green = theme.success;
    let yellow = theme.warning;
    let red = theme.danger;
    // Top 8 status codes by count to keep it readable.
    let mut pairs: Vec<(u16, u64)> = r.status_distribution.iter().map(|(k, v)| (*k, *v)).collect();
    pairs.sort_by_key(|b| std::cmp::Reverse(b.1));
    pairs.truncate(8);
    let data: Vec<StatusBar> = pairs
        .into_iter()
        .map(|(code, count)| StatusBar {
            code: code.to_string().into(),
            count,
            is_success: (200..400).contains(&code),
        })
        .collect();

    v_flex()
        .w_full()
        .gap_1()
        .child(div().text_xs().text_color(theme.muted_foreground).child(t("状态码分布")))
        .child(
            div()
                .w_full()
                .h(px(140.))
                .child(
                    BarChart::new(data)
                        .id("status-dist-chart")
                        .band(|d| d.code.clone())
                        .value(|d| d.count as f64)
                        .fill(move |d, _, _, _| {
                            let code: u16 = d.code.to_string().parse().unwrap_or(0);
                            if (200..300).contains(&code) {
                                green
                            } else if (300..400).contains(&code) || (400..500).contains(&code) {
                                yellow
                            } else {
                                red
                            }
                        }),
                ),
        )
        .into_any_element()
}

fn stat(label: String, value: String, muted: Hsla) -> AnyElement {
    v_flex()
        .gap_0()
        .child(div().text_xs().text_color(muted).child(label))
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(value))
        .into_any_element()
}
