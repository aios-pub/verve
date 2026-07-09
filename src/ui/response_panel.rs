//! Right pane: response viewer (status/time/size + Body + Headers).

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, tab::{Tab, TabBar}, v_flex,
};

use crate::state::{AppEvent, AppState};
use crate::state::models::RequestMethod;

pub struct ResponsePanel {
    pub state: Entity<AppState>,
    pub body_view: Entity<InputState>,
    /// Pretty or raw body rendering.
    pub pretty: bool,
    /// Active response sub-tab.
    pub active_tab: RespTab,
    /// Set when a response/selection changed and the body must be reloaded;
    /// reconciled at the top of render where a Window is available.
    pub pending_refresh: bool,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RespTab {
    /// 实时响应 — the live response body (also shows SSE/WS streams).
    Realtime,
    /// 响应头.
    Headers,
    /// Cookie — parsed from Set-Cookie response headers.
    Cookie,
    /// 响应示例 — example/mock response body.
    Example,
    /// 实际请求 — the actual request that was sent.
    ActualRequest,
    /// 控制台 — script/console output.
    Console,
}

impl RespTab {
    fn label(self) -> &'static str {
        match self {
            RespTab::Realtime => "实时响应",
            RespTab::Headers => "响应头",
            RespTab::Cookie => "Cookie",
            RespTab::Example => "响应示例",
            RespTab::ActualRequest => "实际请求",
            RespTab::Console => "控制台",
        }
    }
    fn all() -> [RespTab; 6] {
        [
            RespTab::Realtime,
            RespTab::Headers,
            RespTab::Cookie,
            RespTab::Example,
            RespTab::ActualRequest,
            RespTab::Console,
        ]
    }
    fn index(self) -> usize {
        match self {
            RespTab::Realtime => 0,
            RespTab::Headers => 1,
            RespTab::Cookie => 2,
            RespTab::Example => 3,
            RespTab::ActualRequest => 4,
            RespTab::Console => 5,
        }
    }
    fn from_index(i: usize) -> Self {
        match i {
            1 => RespTab::Headers,
            2 => RespTab::Cookie,
            3 => RespTab::Example,
            4 => RespTab::ActualRequest,
            5 => RespTab::Console,
            _ => RespTab::Realtime,
        }
    }
}

impl ResponsePanel {
    pub fn new(
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let body_view = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("json")
                .placeholder("响应体将在发送后显示。")
        });
        let sub = cx.subscribe(&state, Self::on_state_event);
        let focus_handle = cx.focus_handle();
        Self {
            state,
            body_view,
            pretty: true,
            active_tab: RespTab::Realtime,
            pending_refresh: false,
            _subs: vec![sub],
            focus_handle,
        }
    }

    fn on_state_event(
        &mut self,
        _src: Entity<AppState>,
        _ev: &AppEvent,
        cx: &mut Context<Self>,
    ) {
        // refresh_body needs a Window, which subscribe handlers lack; defer.
        self.pending_refresh = true;
        cx.notify();
    }

    fn refresh_body(&mut self, _id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let resp = self.state.read(cx).active_project().and_then(|p| {
            p.find_request(self.state.read(cx).selected_request.as_deref()?)
                .map(|(_, r)| r.last_response.clone())
        });
        match resp {
            Some(Some(r)) => {
                // If there's an error and no body, show the error in the body
                // area so the user sees WHY the request failed.
                let text = if r.body.is_empty() {
                    if let Some(err) = &r.error {
                        format!("❌ 请求失败\n\n{err}")
                    } else {
                        String::new()
                    }
                } else {
                    r.body.clone()
                };
                self.body_view.update(cx, |s, cx| {
                    s.set_value(&text, window, cx);
                });
            }
            _ => {
                self.body_view.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                });
            }
        }
        cx.notify();
    }
}

impl Render for ResponsePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_refresh {
            self.pending_refresh = false;
            self.refresh_body(None, window, cx);
        }
        let theme = cx.theme().clone();

        let active_request = self.state.read(cx).active_project().and_then(|p| {
            let sel = self.state.read(cx).selected_request.clone();
            sel.and_then(|id| p.find_request(&id).map(|(_, r)| r.clone()))
        });
        let response = active_request
            .as_ref()
            .and_then(|r| r.last_response.clone());

        let status_line = match &response {
            Some(r) if r.status > 0 => format!("{} {}", r.status, r.status_text),
            Some(r) if !r.status_text.is_empty() => r.status_text.clone(),
            Some(r) => r.error.clone().unwrap_or_else(|| "暂无响应".into()),
            None => "暂无响应".to_string(),
        };
        let is_streaming = response.as_ref().map(|r| r.streaming).unwrap_or(false);
        let time = response.as_ref().map(|r| format!("{} ms", r.time_ms)).unwrap_or_default();
        let size = response
            .as_ref()
            .map(|r| format_bytes(r.size))
            .unwrap_or_default();

        let status_color = status_color(response.as_ref().map(|r| r.status).unwrap_or(0));

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(is_streaming, |bar| {
                        bar.child(
                            div()
                                .size_2()
                                .rounded_full()
                                .bg(theme.primary)
                                .text_xs(),
                        )
                    })
                    .child(
                        div()
                            .text_color(status_color)
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(status_line)),
                    )
                    .child(div().text_color(theme.muted_foreground).text_sm().child(time))
                    .child(div().text_color(theme.muted_foreground).text_sm().child(size))
                    .child(div().flex_1())
                    .child(
                        Button::new("pretty-toggle")
                            .ghost()
                            .small()
                            .icon(IconName::Menu)
                            .selected(self.pretty)
                            .tooltip("Pretty-print")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pretty = !this.pretty;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .child(
                        TabBar::new("response-tabs")
                            .selected_index(self.active_tab.index())
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.active_tab = RespTab::from_index(*ix);
                                cx.notify();
                            }))
                            .children(
                                RespTab::all()
                                    .iter()
                                    .map(|t| Tab::new().label(t.label())),
                            ),
                    ),
            )
            .child({
                let active_tab = self.active_tab;
                let headers = response
                    .as_ref()
                    .map(|r| r.headers.clone())
                    .unwrap_or_default();
                let error = response.as_ref().and_then(|r| r.error.clone());
                // Cookies parsed from Set-Cookie response headers.
                let cookies: Vec<crate::state::models::KeyValue> = response
                    .as_ref()
                    .map(|r| parse_response_cookies(&r.headers))
                    .unwrap_or_default();
                div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .child(match active_tab {
                        RespTab::Realtime => Input::new(&self.body_view)
                            .h_full()
                            .bordered(false)
                            .appearance(false)
                            .font_family(theme.mono_font_family.clone())
                            .text_size(theme.mono_font_size)
                            .into_any_element(),
                        RespTab::Headers => render_headers(&headers, &theme).into_any_element(),
                        RespTab::Cookie => render_cookies(&cookies, &theme).into_any_element(),
                        RespTab::Example => render_example(&active_request, &theme)
                            .into_any_element(),
                        RespTab::ActualRequest => {
                            render_actual_request(&active_request, &theme).into_any_element()
                        }
                        RespTab::Console => render_console(&response, &theme).into_any_element(),
                    })
                    .when_some(error, |this, err| {
                        this.border_t_1().border_color(theme.danger).child(
                            div()
                                .p_3()
                                .text_color(theme.danger)
                                .child(format!("Error: {err}")),
                        )
                    })
            })
            .when(active_request.is_none(), |this| {
                this.border_color(theme.border)
            })
    }
}

/// Render the response headers as a read-only key/value list.
fn render_headers(headers: &[crate::state::models::KeyValue], theme: &gpui_component::Theme) -> AnyElement {
    let rows: Vec<AnyElement> = headers
        .iter()
        .enumerate()
        .map(|(i, kv)| {
            h_flex()
                .w_full()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .w(px(220.))
                        .text_color(theme.muted_foreground)
                        .child(kv.key.clone()),
                )
                .child(div().flex_1().child(kv.value.clone()))
                .id(("resp-header", i))
                .into_any_element()
        })
        .collect();
    // Wrap in a scrollable container when there are many headers.
    if headers.len() > 16 {
        v_flex()
            .w_full()
            .id("resp-headers-scroll")
            .overflow_y_scroll()
            .gap_1()
            .children(rows)
            .into_any_element()
    } else {
        v_flex()
            .w_full()
            .gap_1()
            .children(rows)
            .into_any_element()
    }
}

/// Extract cookie name/value pairs from Set-Cookie response headers.
fn parse_response_cookies(
    headers: &[crate::state::models::KeyValue],
) -> Vec<crate::state::models::KeyValue> {
    headers
        .iter()
        .filter(|h| h.key.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|h| {
            // A Set-Cookie value looks like: `name=value; Path=/; HttpOnly`.
            let first = h.value.split(';').next()?.trim();
            let (k, v) = first.split_once('=')?;
            Some(crate::state::models::KeyValue::new(k.trim(), v.trim()))
        })
        .collect()
}

/// Render response cookies as a key/value list.
fn render_cookies(
    cookies: &[crate::state::models::KeyValue],
    theme: &gpui_component::Theme,
) -> AnyElement {
    if cookies.is_empty() {
        return empty_state("该响应没有 Cookie。", theme);
    }
    let rows: Vec<AnyElement> = cookies
        .iter()
        .enumerate()
        .map(|(i, kv)| {
            h_flex()
                .w_full()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .w(px(220.))
                        .text_color(theme.muted_foreground)
                        .child(kv.key.clone()),
                )
                .child(div().flex_1().child(kv.value.clone()))
                .id(("resp-cookie", i))
                .into_any_element()
        })
        .collect();
    v_flex().w_full().gap_1().children(rows).into_any_element()
}

/// Render the response-example tab: shows the request body as a stand-in
/// example (a dedicated example field is a future enhancement).
fn render_example(
    active_request: &Option<crate::state::models::ApiRequest>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let body = active_request
        .as_ref()
        .map(|r| match r.body.body_type {
            crate::state::models::BodyType::Raw => r.body.raw.clone(),
            _ => String::new(),
        })
        .filter(|b| !b.trim().is_empty());
    match body {
        Some(b) => code_block(&b, theme),
        None => empty_state("暂无响应示例。", theme),
    }
}

/// Render the actual-request tab: the method/URL/headers/body that was sent.
fn render_actual_request(
    active_request: &Option<crate::state::models::ApiRequest>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let req = match active_request {
        Some(r) => r,
        None => return empty_state("暂无请求信息。", theme),
    };
    let mut text = format!("{} {} {}\n", req.protocol, req.method, req.url);
    if !req.headers.is_empty() {
        text.push_str("\n[Headers]\n");
        for h in req.headers.iter().filter(|h| h.enabled && !h.is_empty()) {
            text.push_str(&format!("{}: {}\n", h.key, h.value));
        }
    }
    if !req.cookies.is_empty() {
        text.push_str("\n[Cookies]\n");
        for c in req.cookies.iter().filter(|c| c.enabled && !c.is_empty()) {
            text.push_str(&format!("{}={}\n", c.key, c.value));
        }
    }
    if !req.body.raw.is_empty() {
        text.push_str("\n[Body]\n");
        text.push_str(&req.body.raw);
    }
    code_block(&text, theme)
}

/// Render the console tab: script logs embedded in the response body footer
/// (under the `// ── Script Output ──` marker).
fn render_console(
    response: &Option<crate::state::models::Response>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let body = response.as_ref().map(|r| r.body.as_str()).unwrap_or("");
    if let Some(idx) = body.find("── Script Output ──") {
        let logs = &body[idx..];
        code_block(logs, theme)
    } else {
        empty_state("暂无控制台输出。运行预执行/后执行脚本后会在此显示。", theme)
    }
}

/// A monospace code block in a scrollable container.
fn code_block(text: &str, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .id("resp-code-block")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .text_sm()
        .font_family(theme.mono_font_family.clone())
        .text_size(theme.mono_font_size)
        .text_color(theme.foreground)
        .child(text.to_string())
        .into_any_element()
}

/// A centered muted placeholder.
fn empty_state(msg: &str, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.muted_foreground)
        .child(msg.to_string())
        .into_any_element()
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn status_color(status: u16) -> gpui::Hsla {
    // Approximation using theme-agnostic hues.
    use gpui::hsla;
    if status == 0 {
        return hsla(0.0, 0.0, 0.5, 1.0);
    }
    if status >= 200 && status < 300 {
        hsla(0.33, 0.8, 0.4, 1.0) // green
    } else if status >= 300 && status < 400 {
        hsla(0.11, 0.8, 0.45, 1.0) // amber
    } else if status >= 400 {
        hsla(0.0, 0.75, 0.5, 1.0) // red
    } else {
        hsla(0.6, 0.0, 0.5, 1.0)
    }
}

impl Focusable for ResponsePanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for ResponsePanel {}
