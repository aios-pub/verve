//! Bottom pane: request/response console log (history).

use gpui::*;
use gpui_component::{ActiveTheme, h_flex, v_flex};

use crate::state::{AppEvent, AppState};

/// Events emitted by the console panel upward to VerveApp.
#[derive(Clone, Debug)]
pub enum ConsoleEvent {
    /// User clicked a history row; payload is (request_id, project_id).
    /// request_id may be None for legacy entries that predate the field.
    OpenRequest {
        request_id: Option<String>,
        project_id: String,
    },
}

pub struct ConsolePanel {
    pub state: Entity<AppState>,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

impl EventEmitter<ConsoleEvent> for ConsolePanel {}

impl ConsolePanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&state, |_this, _src, _ev: &AppEvent, _cx| {});
        Self {
            state,
            _subs: vec![sub],
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for ConsolePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let history = self.state.read(cx).data.history.clone();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(rust_i18n::t!("history.title").to_string())
                    .child(div().flex_1())
                    .child(rust_i18n::t!("history.entry_count", count = history.len()).to_string()),
            )
            .child(
                v_flex()
                    .id("console-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .gap(px(1.))
                    .children(history.iter().enumerate().map(|(i, h)| {
                        let color = if h.status >= 200 && h.status < 300 {
                            theme.foreground
                        } else if h.status >= 400 || h.error.is_some() {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        };
                        // Display: prefer the request name; fall back to URL for legacy/unnamed entries.
                        let display_name = if !h.name.is_empty() {
                            h.name.clone()
                        } else if !h.url.is_empty() {
                            shorten_url(&h.url)
                        } else {
                            String::new()
                        };
                        let req_id = h.request_id.clone();
                        let proj_id = h.project_id.clone();
                        h_flex()
                            .id(("console-row", i))
                            .gap_3()
                            .text_sm()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(theme.muted))
                            .child(
                                div()
                                    .w(px_unit(60.))
                                    .text_color(color)
                                    .child(format!("{}", h.status)),
                            )
                            .child(div().w(px_unit(60.)).child(h.method.to_string()))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_color(if !h.name.is_empty() || !h.url.is_empty() {
                                        theme.foreground
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .truncate()
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} ms", h.time_ms)),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(h.at.chars().take(19).collect::<String>()),
                            )
                            .on_click(cx.listener(move |_this, _, _, cx| {
                                cx.emit(ConsoleEvent::OpenRequest {
                                    request_id: req_id.clone(),
                                    project_id: proj_id.clone(),
                                });
                            }))
                    })),
            )
    }
}

/// Trim a URL to something displayable: strip query string and fragment.
fn shorten_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    let without_frag = without_query.split('#').next().unwrap_or(without_query);
    without_frag.to_string()
}

fn px_unit(n: f32) -> gpui::Pixels {
    gpui::px(n)
}

impl Focusable for ConsolePanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
