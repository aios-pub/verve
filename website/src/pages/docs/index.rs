//! Docs index: navigation to doc pages.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

use crate::consts::REPO;
use crate::utils::route;

#[component]
pub fn DocsIndex() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <section class="border-b border-white/5 bg-grid">
            <div class="mx-auto max-w-7xl px-4 py-16 text-center sm:px-6 lg:px-8">
                <h1 class="text-4xl font-extrabold tracking-tight sm:text-5xl">
                    {move || t!(i18n, docs.title)}
                </h1>
            </div>
        </section>

        <div class="mx-auto max-w-3xl px-4 py-16 sm:px-6 lg:px-8">
            <div class="grid gap-4 sm:grid-cols-2">
                <DocCard href=REPO.to_string() icon="">
                    {move || t!(i18n, docs.getting_started)}
                </DocCard>
                <DocCard href=route("/docs/verve-server") icon="">
                    {move || t!(i18n, docs.verve_server)}
                </DocCard>
            </div>
        </div>
    }
}

#[component]
fn DocCard(href: String, icon: &'static str, children: Children) -> impl IntoView {
    view! {
        <a
            href=href
            target="_blank"
            rel="noopener"
            class="group flex items-center justify-between rounded-xl border border-white/5 bg-ink-900 p-5 transition hover:border-bolt-500/30 hover:bg-ink-850"
        >
            <div class="flex items-center gap-3">
                <span class="text-xl">{icon}</span>
                <span class="font-semibold text-ink-100">{children()}</span>
            </div>
            <span class="text-ink-500 transition group-hover:text-bolt-300">"→"</span>
        </a>
    }
}
