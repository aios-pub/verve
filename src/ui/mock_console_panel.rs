//! Mock service console panel — exclusive full-area view when the Mock activity
//! rail icon is clicked. Provides service status, quick start guide, scenario
//! presets, rule list, and (eventually) live request logs.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{h_flex, v_flex, ActiveTheme, IconName, Sizable as _, WindowExt as _};

use crate::state::{AppEvent, AppState};

/// Events emitted by the mock console upward to VerveApp.
#[derive(Clone, Debug)]
pub enum MockConsoleEvent {
    /// User clicked "一键生成 Mock" for all requests.
    GenerateAll,
}

pub struct MockConsolePanel {
    pub state: Entity<AppState>,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

impl EventEmitter<MockConsoleEvent> for MockConsolePanel {}

impl MockConsolePanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&state, |_this, _src, _ev: &AppEvent, _cx| {});
        Self {
            state,
            _subs: vec![sub],
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for MockConsolePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let remote_cfg = crate::state::persistence::load_remote_config();
        let service_url = crate::share::remote::mock_server_url(&remote_cfg);
        let is_remote = crate::share::remote::is_cloud_mode(&remote_cfg);
        let state = self.state.clone();
        let state_copy = self.state.clone();
        let state_copy2 = self.state.clone();
        let state_copy3 = self.state.clone();
        let state_copy4 = self.state.clone();
        let state_copy5 = self.state.clone();

        // Collect all enabled mock rules.
        let mocked = self.state.read(cx).active_project().map(|p| {
            p.iter_all_requests()
                .into_iter()
                .filter_map(|(path, req)| {
                    req.mock.as_ref().filter(|m| m.enabled).map(|m| {
                        (
                            path,
                            req.name.clone(),
                            req.url.clone(),
                            m.status,
                            m.delay_ms,
                        )
                    })
                })
                .collect::<Vec<_>>()
        }).unwrap_or_default();

        v_flex()
            .size_full()
            .bg(theme.background)
            .id("mock-console-scroll")
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p_6()
                    .gap_6()
                    .max_w(px(900.))
                    .mx_auto()
                    // ---- Header ----
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(24.))
                                            .font_weight(FontWeight::BOLD)
                                            .child("🧪 Mock 服务")
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .text_color(theme.muted_foreground)
                                            .child("本地模拟接口响应，前后端并行开发、异常场景测试神器")
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .bg(if is_remote { theme.primary.opacity(0.2) } else { theme.accent.opacity(0.2) })
                                    .child(
                                        div()
                                            .w(px(8.))
                                            .h(px(8.))
                                            .rounded_full()
                                            .bg(if is_remote { theme.primary } else { theme.accent }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(if is_remote { "远程模式" } else { "运行中" })
                                    )
                            ),
                    )
                    // ---- Service URL card ----
                    .child(
                        v_flex()
                            .w_full()
                            .p_4()
                            .gap_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.muted)
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .w(px(80.))
                                            .text_color(theme.muted_foreground)
                                            .child("服务地址：")
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(16.))
                                            .font_family(theme.mono_font_family.clone())
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(service_url.clone())
                                    )
                                    .child(
                                        Button::new("copy-mock-service-url")
                                            .small()
                                            .ghost()
                                            .icon(IconName::Copy)
                                            .label("复制地址")
                                            .on_click(move |_, window, cx: &mut App| {
                                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(service_url.clone()));
                                                window.push_notification(
                                                    gpui_component::notification::Notification::new()
                                                        .title("已复制")
                                                        .message("Mock服务地址已复制，你可以直接在URL中使用 {{mock_server}} 变量")
                                                        .autohide(true),
                                                    cx,
                                                );
                                            }),
                                    )
                            )
                            .when(is_remote, |c| {
                                c.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme.primary)
                                        .child("💡 当前使用远程Mock服务，本地不启动Mock服务器")
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child("💡 提示：你可以直接在请求URL中使用 {{mock_server}}/api/path，系统会自动替换为上面的地址")
                            )
                    )
                    // ---- Quick start guide ----
                    .child(
                        v_flex()
                            .gap_4()
                            .p_5()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.primary.opacity(0.3))
                            .bg(theme.primary.opacity(0.05))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("🚀 3步快速上手")
                            )
                            .child(
                                h_flex()
                                    .gap_6()
                                    .flex_wrap()
                                    .child(step_item(1, "一键生成Mock规则".to_string(), "点击下方按钮，自动为项目中所有接口创建默认200成功响应".to_string(), &theme))
                                    .child(step_item(2, "替换API基础地址".to_string(), "将你前端应用/测试用例的API基础地址替换为上方的Mock服务地址".to_string(), &theme))
                                    .child(step_item(3, "自定义响应（可选）".to_string(), "在接口编辑器的Mock标签页调整状态码、延迟、响应体，模拟各种异常场景".to_string(), &theme))
                            )
                            .child(
                                Button::new("mock-generate-all")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label("一键为所有接口生成Mock规则")
                                    .tooltip("为所有还没有Mock规则的接口生成默认200响应")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.emit(MockConsoleEvent::GenerateAll);
                                        let _ = this;
                                    })),
                            )
                    )
                    .child(div().h(px(1.)).w_full().bg(theme.border))
                    // ---- Quick scenario presets ----
                    .child(
                        v_flex()
                            .gap_4()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("⚡ 异常场景快捷模拟")
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme.muted_foreground)
                                    .child("点击下方按钮，一键将所有Mock接口切换到对应异常场景，方便测试前端错误处理逻辑")
                            )
                            .child(
                                h_flex()
                                    .gap_3()
                                    .flex_wrap()
                                    .child(
                                        Button::new("preset-500")
                                            .ghost()
                                            .label("🔴 500 服务器错误")
                                            .tooltip("所有接口返回500错误，模拟服务器异常")
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let state = state.clone();
                                                state.update(cx, |s, cx| {
                                                    if let Some(p) = s.active_project_mut() {
                                                        for (_path, req) in p.iter_all_requests_mut() {
                                                            if let Some(mock) = req.mock.as_mut() {
                                                                mock.enabled = true;
                                                                mock.status = 500;
                                                                mock.delay_ms = 0;
                                                                mock.body = r#"{"code":500,"message":"Internal Server Error"}"#.to_string();
                                                            }
                                                        }
                                                    }
                                                    s.notify_workspace(cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("preset-timeout")
                                            .ghost()
                                            .label("⏱️ 3秒超时延迟")
                                            .tooltip("所有接口延迟3秒返回，模拟慢接口/网络超时")
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let state = state_copy.clone();
                                                state.update(cx, |s, cx| {
                                                    if let Some(p) = s.active_project_mut() {
                                                        for (_path, req) in p.iter_all_requests_mut() {
                                                            if let Some(mock) = req.mock.as_mut() {
                                                                mock.enabled = true;
                                                                mock.status = 200;
                                                                mock.delay_ms = 3000;
                                                                mock.body = r#"{"code":0,"message":"ok"}"#.to_string();
                                                            }
                                                        }
                                                    }
                                                    s.notify_workspace(cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("preset-404")
                                            .ghost()
                                            .label("🚫 404 接口不存在")
                                            .tooltip("所有接口返回404错误，模拟接口不存在")
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let state = state_copy2.clone();
                                                state.update(cx, |s, cx| {
                                                    if let Some(p) = s.active_project_mut() {
                                                        for (_path, req) in p.iter_all_requests_mut() {
                                                            if let Some(mock) = req.mock.as_mut() {
                                                                mock.enabled = true;
                                                                mock.status = 404;
                                                                mock.delay_ms = 0;
                                                                mock.body = r#"{"code":404,"message":"Not Found"}"#.to_string();
                                                            }
                                                        }
                                                    }
                                                    s.notify_workspace(cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("preset-401")
                                            .ghost()
                                            .label("🚪 401 未授权")
                                            .tooltip("所有接口返回401错误，模拟登录失效/权限不足")
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let state = state_copy3.clone();
                                                state.update(cx, |s, cx| {
                                                    if let Some(p) = s.active_project_mut() {
                                                        for (_path, req) in p.iter_all_requests_mut() {
                                                            if let Some(mock) = req.mock.as_mut() {
                                                                mock.enabled = true;
                                                                mock.status = 401;
                                                                mock.delay_ms = 0;
                                                                mock.body = r#"{"code":401,"message":"Unauthorized"}"#.to_string();
                                                            }
                                                        }
                                                    }
                                                    s.notify_workspace(cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("preset-429")
                                            .ghost()
                                            .label("🚦 429 请求限流")
                                            .tooltip("所有接口返回429错误，模拟接口被限流")
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let state = state_copy4.clone();
                                                state.update(cx, |s, cx| {
                                                    if let Some(p) = s.active_project_mut() {
                                                        for (_path, req) in p.iter_all_requests_mut() {
                                                            if let Some(mock) = req.mock.as_mut() {
                                                                mock.enabled = true;
                                                                mock.status = 429;
                                                                mock.delay_ms = 0;
                                                                mock.body = r#"{"code":429,"message":"Too Many Requests"}"#.to_string();
                                                            }
                                                        }
                                                    }
                                                    s.notify_workspace(cx);
                                                });
                                            }),
                                    )
                                    .child(
                                        Button::new("preset-200")
                                            .ghost()
                                            .label("✅ 恢复正常200")
                                            .tooltip("所有接口恢复返回200成功响应")
                                            .on_click(move |_, _w, cx: &mut App| {
                                                let state = state_copy5.clone();
                                                state.update(cx, |s, cx| {
                                                    if let Some(p) = s.active_project_mut() {
                                                        for (_path, req) in p.iter_all_requests_mut() {
                                                            if let Some(mock) = req.mock.as_mut() {
                                                                mock.enabled = true;
                                                                mock.status = 200;
                                                                mock.delay_ms = 0;
                                                                mock.body = r#"{"code":0,"message":"ok"}"#.to_string();
                                                            }
                                                        }
                                                    }
                                                    s.notify_workspace(cx);
                                                });
                                            }),
                                    )
                            )
                    )
                    .child(div().h(px(1.)).w_full().bg(theme.border))
                    // ---- Enabled rules list ----
                    .child(
                        v_flex()
                            .gap_4()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(18.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("📋 已启用Mock规则")
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .px(px(10.))
                                            .py(px(3.))
                                            .rounded_full()
                                            .bg(theme.primary.opacity(0.2))
                                            .text_color(theme.primary)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(format!("{} 条", mocked.len()))
                                    )
                            )
                            .when(mocked.is_empty(), |c| {
                                c.child(
                                    div()
                                        .p_8()
                                        .text_center()
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(theme.border)
                                        .bg(theme.muted)
                                        .text_size(px(14.))
                                        .text_color(theme.muted_foreground)
                                        .child("暂无启用的Mock规则，点击上方「一键生成」按钮快速创建")
                                )
                            })
                            .when(!mocked.is_empty(), |c| {
                                c.child(
                                    v_flex()
                                        .gap_2()
                                        .children(mocked.iter().enumerate().map(|(i, (loc, name, url, status, delay))| {
                                            let color = if *status >= 200 && *status < 300 {
                                                theme.accent
                                            } else if *status >= 400 {
                                                theme.danger
                                            } else {
                                                theme.muted_foreground
                                            };
                                            h_flex()
                                                .id(("mock-rule-row", i))
                                                .w_full()
                                                .p_4()
                                                .gap_4()
                                                .items_center()
                                                .rounded_lg()
                                                .border_1()
                                                .border_color(theme.border)
                                                .hover(|d| d.bg(theme.muted))
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .px(px(10.))
                                                        .py(px(4.))
                                                        .rounded(px(6.))
                                                        .bg(color.opacity(0.2))
                                                        .text_color(color)
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .w(px(50.))
                                                        .text_center()
                                                        .child(format!("{}", status))
                                                )
                                                .child(
                                                    v_flex()
                                                        .gap_1()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .child(
                                                            div()
                                                                .text_size(px(14.))
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .child(name.clone())
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.))
                                                                .text_color(theme.muted_foreground)
                                                                .font_family(theme.mono_font_family.clone())
                                                                .text_ellipsis()
                                                                .whitespace_nowrap()
                                                                .overflow_hidden()
                                                                .child(url.clone())
                                                        )
                                                )
                                                .when(*delay > 0, |d| {
                                                    d.child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .px(px(8.))
                                                            .py(px(3.))
                                                            .rounded_full()
                                                            .bg(theme.warning.opacity(0.2))
                                                            .text_color(theme.warning)
                                                            .child(format!("⏱️ {}ms延迟", delay))
                                                    )
                                                })
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .text_color(theme.muted_foreground)
                                                        .w(px(150.))
                                                        .text_right()
                                                        .text_ellipsis()
                                                        .whitespace_nowrap()
                                                        .overflow_hidden()
                                                        .child(loc.clone())
                                                )
                                        }))
                                )
                            })
                    )
            )
    }
}

fn step_item(step: u8, title: String, desc: String, theme: &gpui_component::Theme) -> impl IntoElement {
    v_flex()
        .gap_2()
        .w(px(220.))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w(px(28.))
                        .h(px(28.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme.primary.opacity(0.2))
                        .text_color(theme.primary)
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{}", step))
                )
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title)
                )
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(desc)
        )
}
