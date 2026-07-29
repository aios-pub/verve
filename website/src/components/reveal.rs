//! Scroll-reveal: toggles a signal when the element scrolls into view (via
//! `leptos_use::use_intersection_observer`), and the class is applied
//! reactively through the view rather than direct DOM mutation. This avoids
//! relying on JS interop inside the observer callback.
//!
//! Usage:
//!   <Reveal>"content"</Reveal>                  // single block
//!   <Reveal stagger=true> {...cards} </Reveal>  // staggered children

use leptos::prelude::*;
use leptos::html::Div;
use leptos_use::use_intersection_observer;

/// Wraps content that fades/slides up when scrolled into view.
#[component]
pub fn Reveal(
    /// If true, children are revealed one after another (staggered).
    #[prop(optional)]
    stagger: bool,
    children: Children,
) -> impl IntoView {
    let el: NodeRef<Div> = NodeRef::new();
    let (visible, set_visible) = signal(false);

    use_intersection_observer(el, move |entries, _| {
        for entry in entries {
            if entry.is_intersecting() {
                set_visible.set(true);
            }
        }
    });

    let base = if stagger { "reveal-stagger" } else { "reveal" };
    let class = Signal::derive(move || {
        if visible.get() {
            format!("{base} is-visible")
        } else {
            base.to_string()
        }
    });

    view! {
        <div node_ref=el class=class>
            {children()}
        </div>
    }
}
