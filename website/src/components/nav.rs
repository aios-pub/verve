//! Top navigation: brand, section links, language switcher, download CTA,
//! scroll-progress bar and an active-route indicator.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::components::fx::ScrollProgress;
use crate::consts::RELEASES_LATEST;
use crate::i18n::{t, use_i18n, Locale};
use crate::utils::{asset, route};

/// Sticky top navigation bar.
#[component]
pub fn Nav() -> impl IntoView {
    let i18n = use_i18n();
    let location = use_location();
    let pathname = move || location.pathname.get();

    let is_active = move |href: &str| {
        let p = pathname();
        if href == "/" {
            p == "/" || p.is_empty()
        } else {
            p == href || p.starts_with(&format!("{href}/"))
        }
    };

    // Toggle between zh-CN and en, persisting the choice.
    let toggle_lang = move |_| {
        let cur = i18n.get_locale();
        let next = if cur == Locale::zh_CN { Locale::en } else { Locale::zh_CN };
        i18n.set_locale(next);
    };

    let current_label = move || {
        if i18n.get_locale() == Locale::en { "中文" } else { "EN" }
    };

    let link_class = move |href: &'static str| {
        if is_active(href) {
            "rounded-lg px-3 py-2 transition nav-link-active"
        } else {
            "rounded-lg px-3 py-2 transition hover:bg-white/[0.04] hover:text-bolt-200"
        }
    };

    view! {
        <header class="sticky top-0 z-50 border-b border-ink-700 bg-ink-950/80 backdrop-blur-xl">
            <nav class="mx-auto flex h-16 max-w-6xl items-center justify-between px-4 sm:px-6 lg:px-8">
                // Brand
                <A href=route("/") attr:class="group flex items-center gap-2.5 font-bold tracking-tight transition hover:opacity-90">
                    <img src=asset("icons/verve.png") alt="Verve"
                        class="h-7 w-7 rounded-md transition group-hover:drop-shadow-[0_0_8px_rgba(232,249,63,0.5)]" />
                    <span class="text-[17px] text-ink-100">"Verve"</span>
                </A>

                // Section links
                <div class="hidden items-center gap-1 text-sm font-medium text-ink-300 md:flex">
                    <A href=route("/features") attr:class=move || link_class("/features")>
                        {move || t!(i18n, nav.features)}
                    </A>
                    <A href=route("/download") attr:class=move || link_class("/download")>
                        {move || t!(i18n, nav.download)}
                    </A>
                    <A href=route("/docs") attr:class=move || link_class("/docs")>
                        {move || t!(i18n, nav.docs)}
                    </A>
                    <A href=route("/blog") attr:class=move || link_class("/blog")>
                        {move || t!(i18n, nav.blog)}
                    </A>
                </div>

                // Actions
                <div class="flex items-center gap-2">
                    <super::ThemeToggle />
                    <button
                        on:click=toggle_lang
                        class="rounded-lg border border-white/10 px-2.5 py-1.5 text-xs font-semibold text-ink-200 transition hover:border-bolt-500/50 hover:text-bolt-200"
                        aria-label="Switch language"
                    >
                        {current_label}
                    </button>
                    <a
                        href=RELEASES_LATEST
                        class="btn-bolt rounded-lg px-3.5 py-1.5 text-sm font-semibold"
                    >
                        {move || t!(i18n, nav.get_app)}
                    </a>
                </div>
            </nav>
            <ScrollProgress />
        </header>
    }
}
