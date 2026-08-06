//! 1px gradient scroll-progress bar, pinned to the bottom of the sticky nav.

use leptos::prelude::*;
use leptos_use::use_window_scroll;

#[component]
pub fn ScrollProgress() -> impl IntoView {
    let (x, _y) = use_window_scroll();

    let pct = Signal::derive(move || {
        let win = web_sys::window();
        let doc = win.as_ref().and_then(|w| w.document());
        let scroll_y = x.get();
        let height = doc
            .as_ref()
            .and_then(|d| d.document_element())
            .map(|el| el.scroll_height() as f64)
            .unwrap_or(0.0);
        let inner = win
            .as_ref()
            .and_then(|w| w.inner_height().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let max = (height - inner).max(1.0);
        (scroll_y / max * 100.0).clamp(0.0, 100.0)
    });

    view! {
        <div
            class="scroll-progress z-[60]"
            style=move || format!("width:{:.2}%;", pct.get())
            aria-hidden="true"
        ></div>
    }
}
