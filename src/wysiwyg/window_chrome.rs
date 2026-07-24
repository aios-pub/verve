//! Window chrome (title bar) stubs.
//!
//! velotype renders its own custom title bar (`render_custom_titlebar`). verve
//! owns window decorations via gpui-component, so the embedded editor never
//! renders a title bar — these helpers return empty/zero so the editor's
//! layout math stays consistent.

use gpui::{AnyElement, Context, SharedString, Window};

use crate::wysiwyg::theme::{Theme, ThemeDimensions};

/// Always zero — verve provides its own title bar, so the editor reserves no
/// vertical space for one.
pub(crate) fn custom_titlebar_height(_window: &Window, _dimensions: &ThemeDimensions) -> f32 {
    0.0
}

/// Always `None` — the embedded editor does not paint a custom title bar.
pub(crate) fn render_custom_titlebar<T: 'static>(
    _id: &'static str,
    _title: SharedString,
    _theme: &Theme,
    _window: &Window,
    _cx: &mut Context<T>,
    _on_close: fn(&mut T, &gpui::ClickEvent, &mut Window, &mut Context<T>),
) -> Option<AnyElement> {
    None
}
