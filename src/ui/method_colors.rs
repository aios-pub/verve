//! HTTP method colors, matching the postman palette.
//!
//! Each method gets a consistent hue used for:
//! - the colored badge before a request name in the tree,
//! - the method chip merged into the URL bar,
//! - the Send button fill.
//!
//! The colors are tuned for both light and dark themes: `badge` returns a
//! solid-ish color for small text/badges; `accent` is the same hue slightly
//! desaturated for large fills (buttons) so it doesn't overwhelm.

use gpui::Hsla;
use gpui_component::ActiveTheme;

use crate::state::models::RequestMethod;

/// A pair of colors for a method: a vivid badge color and a softer fill.
pub struct MethodColor {
    pub badge: Hsla,
    pub fill: Hsla,
}

impl RequestMethod {
    /// The canonical color for this method.
    pub fn color(&self) -> MethodColor {
        use gpui::hsla;
        // Hue, saturation, lightness chosen to read well at small sizes on
        // both light and dark backgrounds.
        let (h, s) = match self {
            RequestMethod::Get => (0.33, 0.62),     // green
            RequestMethod::Post => (0.10, 0.78),    // amber/orange
            RequestMethod::Put => (0.58, 0.62),     // blue
            RequestMethod::Patch => (0.13, 0.65),   // yellow-amber
            RequestMethod::Delete => (0.00, 0.72),  // red
            RequestMethod::Head => (0.72, 0.18),    // gray-purple
            RequestMethod::Options => (0.78, 0.45), // purple
        };
        MethodColor {
            // Badge text: deeper/lighter depending on theme (caller adjusts).
            badge: hsla(h, s, 0.55, 1.0),
            fill: hsla(h, s * 0.85, 0.50, 1.0),
        }
    }

    /// A short label suitable for the tree badge (e.g. "GET", "POST").
    pub fn badge_label(&self) -> &'static str {
        match self {
            RequestMethod::Get => "GET",
            RequestMethod::Post => "POST",
            RequestMethod::Put => "PUT",
            RequestMethod::Delete => "DEL",
            RequestMethod::Patch => "PATCH",
            RequestMethod::Head => "HEAD",
            RequestMethod::Options => "OPT",
        }
    }
}

/// Resolve a method's badge color, adjusted for the active theme so it has
/// enough contrast on both light and dark backgrounds.
pub fn badge_color(method: RequestMethod, cx: &gpui::App) -> Hsla {
    let c = method.color().badge;
    if cx.theme().mode.is_dark() {
        // On dark backgrounds, lighten the badge a touch.
        gpui::hsla(c.h, c.s, 0.70, 1.0)
    } else {
        // On light backgrounds, darken for contrast.
        gpui::hsla(c.h, c.s, 0.38, 1.0)
    }
}

/// Resolve a method's fill color (for the Send button), theme-adjusted.
pub fn fill_color(method: RequestMethod, cx: &gpui::App) -> Hsla {
    let c = method.color().fill;
    if cx.theme().mode.is_dark() {
        gpui::hsla(c.h, c.s, 0.55, 1.0)
    } else {
        gpui::hsla(c.h, c.s * 0.9, 0.45, 1.0)
    }
}
