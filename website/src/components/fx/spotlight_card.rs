//! Card wrapper that tracks the cursor and drives a CSS radial spotlight plus a
//! subtle 3D tilt. The Rust side only writes two CSS custom properties
//! (`--mx`, `--my`, and on tilt `--rx`, `--ry`); all visuals live in CSS, which
//! means there's nothing to clean up and no per-frame work.
//!
//! On touch / reduced-motion devices the `.tilt` transform is a no-op (CSS
//! media query) and the spotlight simply doesn't appear without a hover.

use leptos::html::Div;
use leptos::prelude::*;
use leptos_use::use_mouse_in_element;

#[component]
pub fn SpotlightCard(
    /// Extra classes for the outer element (e.g. `"card p-5"`).
    #[prop(optional)]
    class: &'static str,
    /// Corner bracket accent: `"pro"` for purple, anything else (incl. `""`)
    /// for bolt-yellow brackets.
    #[prop(default = "")]
    corners: &'static str,
    /// Apply a hover tilt. Defaults to true; ignored on touch via CSS.
    #[prop(default = true)]
    tilt: bool,
    children: Children,
) -> impl IntoView {
    let el: NodeRef<Div> = NodeRef::new();
    let mouse = use_mouse_in_element(el);

    let style = Signal::derive(move || {
        let x = mouse.element_x.get();
        let y = mouse.element_y.get();
        if !tilt {
            return format!("--mx:{x:.1}px;--my:{y:.1}px;");
        }
        let w = mouse.element_width.get().max(1.0);
        let h = mouse.element_height.get().max(1.0);
        // Max tilt ~3.5deg, centered on the card.
        let rx = (0.5 - y / h) * 3.5;
        let ry = (x / w - 0.5) * 3.5;
        format!("--mx:{x:.1}px;--my:{y:.1}px;--rx:{rx:.2}deg;--ry:{ry:.2}deg;")
    });

    let tilt_class = if tilt { "tilt" } else { "" };
    let show_corners = !corners.is_empty();

    view! {
        <div
            node_ref=el
            class=format!("spotlight {class} {tilt_class}")
            style=move || style.get()
        >
            // Corner brackets live on their own overlay so they don't fight the
            // spotlight glow for the ::before pseudo-element.
            {show_corners.then(|| {
                let cls = if corners == "pro" { "card-corners is-pro" } else { "card-corners" };
                view! { <span class=cls aria-hidden="true"></span> }
            })}
            {children()}
        </div>
    }
}
