//! Shared motion-capability helpers.
//!
//! The heavy WASM/Canvas effects are only enabled when the user hasn't asked
//! for reduced motion AND the viewport is wide enough to be a desktop-class
//! browser. Narrow / touch devices get the lightweight static treatment.

use leptos::prelude::*;
use leptos_use::{use_prefers_reduced_motion, use_window_size};

/// Whether pointer-fine, motion-OK effects may run.
/// Reactive: flips off if the OS "reduce motion" toggle is on or the screen
/// is narrower than a tablet.
pub fn effects_enabled() -> Signal<bool> {
    let reduced = use_prefers_reduced_motion();
    let size = use_window_size();
    Signal::derive(move || !reduced.get() && size.width.get() >= 768.0)
}
