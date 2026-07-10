//! Bottom pane: request/response console log (history).

use gpui::*;
use gpui_component::{h_flex, v_flex, ActiveTheme};

use crate::state::{AppEvent, AppState};

pub struct ConsolePanel {
    pub state: Entity<AppState>,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

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
        let history = &self.state.read(cx).data.history;
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
                    .child("Console")
                    .child(div().flex_1())
                    .child(format!("{} entries", history.len())),
            )
            .child(
                v_flex()
                    .id("console-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_1()
                    .children(history.iter().enumerate().map(|(i, h)| {
                        let color = if h.status >= 200 && h.status < 300 {
                            theme.foreground
                        } else if h.status >= 400 || h.error.is_some() {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        };
                        h_flex()
                            .gap_3()
                            .text_sm()
                            .child(
                                div()
                                    .w(px_unit(60.))
                                    .text_color(color)
                                    .child(format!("{}", h.status)),
                            )
                            .child(div().w(px_unit(60.)).child(h.method.to_string()))
                            .child(div().flex_1().child(h.name.clone()))
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
                            .id(("console-row", i))
                    })),
            )
    }
}

fn px_unit(n: f32) -> gpui::Pixels {
    gpui::px(n)
}

impl Focusable for ConsolePanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for ConsolePanel {}
