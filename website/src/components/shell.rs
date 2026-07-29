//! Page shell: sticky top nav + content slot + footer.

use leptos::prelude::*;

use super::{Footer, Nav};

/// Wrap page content with the site chrome (nav + footer).
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    view! {
        <div class="min-h-screen flex flex-col bg-ink-950">
            <Nav />
            <main class="flex-1">{children()}</main>
            <Footer />
        </div>
    }
}
