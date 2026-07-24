//! Environments panel — a placeholder for managing environments & variables.
//! Full editing UI is a roadmap item; for now it renders a read-only summary
//! and the title-bar env switcher drives selection.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{ActiveTheme, v_flex};

use crate::state::{AppEvent, AppState};

pub struct EnvPanel {
    pub state: Entity<AppState>,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

impl EnvPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&state, |_this, _src, _ev: &AppEvent, _cx| {});
        Self {
            state,
            _subs: vec![sub],
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for EnvPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let project = self.state.read(cx).active_project();
        v_flex()
            .size_full()
            .bg(theme.background)
            .p_4()
            .gap_2()
            .child("Environments")
            .when_some(project, |this, p| {
                this.children(p.environments.iter().map(|e| {
                    let active = p.active_environment.as_deref() == Some(e.id.as_str());
                    v_flex()
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius)
                        .p_3()
                        .gap_1()
                        .child(format!(
                            "{} {}",
                            e.name,
                            if active { "(active)" } else { "" }
                        ))
                        .children(
                            e.variables
                                .iter()
                                .map(|kv| v_flex().child(format!("{} = {}", kv.key, kv.value))),
                        )
                }))
            })
    }
}

impl Focusable for EnvPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for EnvPanel {}
