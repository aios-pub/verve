//! Download buttons for the three platforms, linking to GitHub Releases.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

use crate::consts::{RELEASES_ALL, RELEASES_LATEST, VERVE_VERSION};

#[component]
pub fn DownloadButtons() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="text-center">
            <p class="text-sm text-ink-400">
                {move || t!(i18n, download.subtitle)}
                <span class="ml-1 inline-flex items-center gap-1 font-mono text-bolt-300">
                    "v" {VERVE_VERSION}
                </span>
            </p>

            <div class="mt-6 flex flex-wrap items-center justify-center gap-4">
                <PlatformButton icon="" hint="macOS 11+">{move || t!(i18n, hero.platform_macos)}</PlatformButton>
                <PlatformButton icon="" hint=".deb">{move || t!(i18n, hero.platform_linux)}</PlatformButton>
                <PlatformButton icon="" hint=".msi">{move || t!(i18n, hero.platform_windows)}</PlatformButton>
            </div>

            <a
                href=RELEASES_LATEST
                class="mt-6 inline-flex items-center gap-1.5 text-sm text-ink-300 hover:text-bolt-300 transition"
            >
                {move || t!(i18n, download.btn)}
                " →"
            </a>

            <div class="mt-2">
                <a
                    href=RELEASES_ALL
                    class="text-xs text-ink-500 hover:text-ink-300 transition"
                >
                    {move || t!(i18n, download.latest)}
                </a>
            </div>
        </div>
    }
}

#[component]
fn PlatformButton(
    icon: &'static str,
    children: Children,
    hint: &'static str,
) -> impl IntoView {
    view! {
        <a
            href=RELEASES_LATEST
            class="group flex min-w-[10rem] flex-col items-center rounded-xl border border-white/10 bg-ink-900 px-6 py-4 transition hover:border-bolt-500/40 hover:bg-ink-850"
        >
            <span class="text-2xl">{icon}</span>
            <span class="mt-2 font-semibold text-ink-100">{children()}</span>
            <span class="text-xs text-ink-500">{hint}</span>
        </a>
    }
}
