//! Docs index: navigation to doc pages.

use leptos::prelude::*;
use leptos_router::components::A;
use crate::i18n::{t, use_i18n};

use crate::consts::REPO;
use crate::utils::route;

#[component]
pub fn DocsIndex() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <section class="relative isolate overflow-hidden border-b border-ink-700">
            <div class="bg-grid pointer-events-none absolute inset-0" aria-hidden="true"></div>
            <div class="relative z-10 mx-auto max-w-7xl px-4 pb-14 pt-10 text-center sm:px-6 lg:px-8">
                <h1 class="text-4xl font-extrabold leading-tight tracking-tight sm:text-5xl">
                    {move || t!(i18n, docs.title)}
                </h1>
            </div>
        </section>

        <div class="mx-auto max-w-3xl px-4 py-16 sm:px-6 lg:px-8">
            <div class="grid gap-4 sm:grid-cols-2">
                // External: getting-started → repo
                <DocCardExt href=REPO.to_string()>
                    {move || t!(i18n, docs.getting_started)}
                </DocCardExt>
                // Internal SPA routes
                <DocCardInt href=route("/docs/verve-server")>
                    {move || t!(i18n, docs.verve_server)}
                </DocCardInt>
                <DocCardInt href=route("/docs/cloud")>
                    {move || t!(i18n, docs.cloud)}
                </DocCardInt>
            </div>
        </div>
    }
}

/// Internal doc card — uses the router `<A>` for SPA navigation (no reload).
#[component]
fn DocCardInt(href: String, children: Children) -> impl IntoView {
    view! {
        <A href=href attr:class="group flex items-center justify-between rounded-xl border border-ink-700 bg-ink-900 p-5 transition hover:border-bolt-500/30 hover:bg-ink-800">
            <div class="flex items-center gap-3">
                <span class="font-semibold text-ink-100">{children()}</span>
            </div>
            <span class="text-ink-500 transition group-hover:text-bolt-300">"→"</span>
        </A>
    }
}

/// External doc card — plain anchor opening in a new tab.
#[component]
fn DocCardExt(href: String, children: Children) -> impl IntoView {
    view! {
        <a href=href target="_blank" rel="noopener"
            class="group flex items-center justify-between rounded-xl border border-ink-700 bg-ink-900 p-5 transition hover:border-bolt-500/30 hover:bg-ink-800">
            <div class="flex items-center gap-3">
                <span class="font-semibold text-ink-100">{children()}</span>
            </div>
            <span class="text-ink-500 transition group-hover:text-bolt-300">"→"</span>
        </a>
    }
}
