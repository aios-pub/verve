//! Theme helpers: toggle light/dark and read the current mode.

use gpui::App;
use gpui_component::{ActiveTheme, Theme, ThemeMode};

/// Read the current theme mode by inspecting the global theme.
pub fn current_mode(cx: &App) -> ThemeMode {
    if cx.theme().mode.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

/// Toggle between light and dark.
pub fn toggle(cx: &mut App) {
    let next = if current_mode(cx).is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(next, None, cx);
}
