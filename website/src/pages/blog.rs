//! Blog / changelog page: static list of releases linking to GitHub.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

use crate::consts::{release_tag, RELEASES_ALL};

/// Known release tags. Kept static for the static build; the GitHub Releases
/// page is the source of truth.
const RELEASES: &[(&str, &str)] = &[
    ("v0.3.0", "2026-07"),
    ("v0.1.0", "2026-05"),
];

#[component]
pub fn Blog() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <section class="border-b border-ink-700 bg-grid">
            <div class="mx-auto max-w-7xl px-4 pb-14 pt-10 text-center sm:px-6 lg:px-8">
                <h1 class="text-4xl font-extrabold leading-tight tracking-tight sm:text-5xl">
                    {move || t!(i18n, blog.title)}
                </h1>
                <p class="mt-4 text-ink-400">{move || t!(i18n, blog.subtitle)}</p>
            </div>
        </section>

        <div class="mx-auto max-w-3xl px-4 py-16 sm:px-6 lg:px-8">
            <ul class="space-y-4">
                {RELEASES.iter().map(|(tag, date)| {
                    view! {
                        <li>
                            <a
                                href=release_tag(tag)
                                target="_blank"
                                rel="noopener"
                                class="flex items-center justify-between rounded-xl border border-white/5 bg-ink-900 p-5 transition hover:border-bolt-500/30 hover:bg-ink-850"
                            >
                                <div>
                                    <span class="font-mono text-lg font-semibold text-bolt-300">{*tag}</span>
                                </div>
                                <span class="text-sm text-ink-500">{*date}</span>
                            </a>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ul>

            <div class="mt-10 text-center">
                <a
                    href=RELEASES_ALL
                    target="_blank"
                    rel="noopener"
                    class="text-sm text-ink-300 hover:text-bolt-300 transition"
                >
                    {move || t!(i18n, download.latest)}
                </a>
            </div>
        </div>
    }
}
