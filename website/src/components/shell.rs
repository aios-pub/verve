//! Page shell: sticky top nav + content slot + footer + global fx (grain).

use leptos::prelude::*;

use super::{Footer, Nav};

/// Wrap page content with the site chrome (nav + footer).
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    view! {
        <div id="top" class="relative min-h-screen flex flex-col bg-ink-950">
            // Film grain overlay — sits above the background but below content.
            <div class="grain" aria-hidden="true"></div>

            <Nav />
            <main class="relative z-10 flex-1">{children()}</main>
            <Footer />
        </div>
    }
}
