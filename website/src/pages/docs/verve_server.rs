//! verve-server deployment doc, embedded from /docs/verve-server.md at build
//! time. Rendered in a monospace block for now (no markdown dependency).

use leptos::prelude::*;

use crate::consts::REPO;

/// Raw markdown of the verve-server deployment guide.
const VERVE_SERVER_MD: &str = include_str!("../../../../docs/verve-server.md");

#[component]
pub fn VerveServer() -> impl IntoView {
    view! {
        <section class="border-b border-white/5 bg-grid">
            <div class="mx-auto max-w-7xl px-4 py-12 sm:px-6 lg:px-8">
                <nav class="text-sm text-ink-500">
                    <a href="/verve/docs" class="hover:text-bolt-300 transition">"Docs"</a>
                    " / "
                    <span class="text-ink-300">"verve-server"</span>
                </nav>
                <h1 class="mt-4 text-3xl font-extrabold tracking-tight sm:text-4xl">
                    "Self-host verve-server"
                </h1>
                <p class="mt-3 text-sm text-ink-400">
                    <a href=REPO target="_blank" rel="noopener" class="text-bolt-300 hover:underline">
                        "View source on GitHub"
                    </a>
                </p>
            </div>
        </section>

        <div class="mx-auto max-w-4xl px-4 py-12 sm:px-6 lg:px-8">
            <pre class="overflow-x-auto rounded-xl border border-white/5 bg-ink-950 p-6 text-sm leading-relaxed text-ink-300">
                <code>{VERVE_SERVER_MD}</code>
            </pre>
        </div>
    }
}
