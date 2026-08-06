//! Small `// LABEL` mono tag used across the site to reinforce the engineering
//! aesthetic. Pure presentational.

use leptos::prelude::*;

#[component]
pub fn TechBadge(
    /// Label rendered after the `//` glyph (uppercased via CSS).
    label: &'static str,
    /// Show a pulsing live-status dot before the label.
    #[prop(optional)]
    live: bool,
    /// Additional classes.
    #[prop(optional)]
    class: &'static str,
) -> impl IntoView {
    view! {
        <span class=format!("tech-label {class}")>
            {live.then(|| view! { <span class="status-dot"></span> })}
            {label}
        </span>
    }
}
