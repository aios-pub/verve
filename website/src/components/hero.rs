//! Hero section: aurora background, mouse spotlight, shimmer title, CTAs,
//! performance pills.

use leptos::prelude::*;
use leptos::html::Div;
use leptos_use::use_mouse_in_element;
use crate::i18n::{t, use_i18n};

use crate::consts::RELEASES_LATEST;
use crate::utils::route;

#[component]
pub fn Hero() -> impl IntoView {
    let i18n = use_i18n();
    let el: NodeRef<Div> = NodeRef::new();

    // Track the pointer position relative to the hero element for the spotlight.
    let mouse = use_mouse_in_element(el);

    let spotlight_style = Signal::derive(move || {
        let x = mouse.element_x.get();
        let y = mouse.element_y.get();
        format!(
            "background: radial-gradient(420px circle at {:.0}px {:.0}px, rgba(212,227,23,0.10), transparent 60%);",
            x, y
        )
    });

    view! {
        <section class="relative overflow-hidden">
            // Layer 1: drifting grid
            <div class="absolute inset-0 bg-grid bg-grid-animated"></div>

            // Layer 2: aurora blobs
            <div class="pointer-events-none absolute inset-0 overflow-hidden">
                <div
                    class="aurora-blob left-[5%] top-[-6%] h-[360px] w-[360px] bg-bolt-500/25"
                    style="animation-delay: 0s;"
                ></div>
                <div
                    class="aurora-blob right-[8%] top-[2%] h-[320px] w-[320px] bg-pro-500/20"
                    style="animation-delay: -6s;"
                ></div>
                <div
                    class="aurora-blob left-[35%] top-[30%] h-[280px] w-[280px] bg-bolt-400/10"
                    style="animation-delay: -12s;"
                ></div>
            </div>

            // Layer 3: top radial wash
            <div class="pointer-events-none absolute inset-x-0 top-0 h-[520px] bg-[radial-gradient(ellipse_60%_50%_at_50%_0%,rgba(212,227,23,0.16),transparent_70%)]"></div>

            // Layer 4: mouse-follow spotlight (sits above bg, below content)
            <div
                class="pointer-events-none absolute inset-0 transition-opacity duration-500"
                style=move || spotlight_style.get()
            ></div>

            <div
                node_ref=el
                class="relative mx-auto max-w-5xl px-4 pt-12 pb-20 sm:px-6 lg:px-8 lg:pt-16 lg:pb-24"
            >
                <div class="mx-auto max-w-3xl text-center">
                    // Badge
                    <div class="flex justify-center">
                        <span class="inline-flex items-center gap-2 rounded-full border border-bolt-500/25 bg-bolt-500/[0.06] px-3.5 py-1.5 text-xs font-medium text-bolt-200 backdrop-blur">
                            <span class="relative flex h-1.5 w-1.5">
                                <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-bolt-400 opacity-75"></span>
                                <span class="relative inline-flex h-1.5 w-1.5 rounded-full bg-bolt-400"></span>
                            </span>
                            {move || t!(i18n, hero.badge)}
                        </span>
                    </div>

                    // Title — short, stays on a single line on desktop.
                    <h1 class="mt-8 text-4xl font-extrabold leading-tight tracking-tight sm:text-6xl lg:whitespace-nowrap lg:text-7xl">
                        {move || t!(i18n, hero.title)}
                    </h1>
                    <p class="mt-3 text-2xl font-semibold tracking-tight sm:text-3xl lg:text-4xl">
                        <span class="shimmer-text">{move || t!(i18n, hero.title_accent)}</span>
                    </p>

                    // Subtitle
                    <p class="mx-auto mt-7 max-w-2xl text-base leading-relaxed text-ink-400 sm:text-lg">
                        {move || t!(i18n, hero.subtitle)}
                    </p>

                    // CTAs
                    <div class="mt-10 flex flex-col items-center justify-center gap-3 sm:flex-row sm:gap-4">
                        <a
                            href=RELEASES_LATEST
                            class="glow-bolt btn-bolt inline-flex w-full items-center justify-center gap-2 rounded-xl px-7 py-3.5 text-base font-semibold hover:scale-[1.03] sm:w-auto"
                        >
                            {move || t!(i18n, hero.cta_download)}
                        </a>
                        <a
                            href=route("/features")
                            class="inline-flex w-full items-center justify-center gap-2 rounded-xl border border-white/10 bg-white/[0.03] px-7 py-3.5 text-base font-medium text-ink-100 transition hover:border-bolt-500/40 hover:text-bolt-200 hover:scale-[1.03] sm:w-auto"
                        >
                            {move || t!(i18n, hero.cta_features)}
                            <span aria-hidden="true">"→"</span>
                        </a>
                    </div>

                    // Performance pills
                    <div class="mt-16 flex flex-wrap items-center justify-center gap-x-6 gap-y-3 text-sm">
                        <PerfPill>{move || t!(i18n, perf.startup)}</PerfPill>
                        <Dot />
                        <PerfPill>{move || t!(i18n, perf.ram)}</PerfPill>
                        <Dot />
                        <PerfPill>{move || t!(i18n, perf.fps)}</PerfPill>
                        <Dot />
                        <PerfPill>{move || t!(i18n, perf.offline)}</PerfPill>
                        <Dot />
                        <PerfPill>{move || t!(i18n, perf.safe)}</PerfPill>
                    </div>
                </div>
            </div>
        </section>
    }
}

#[component]
fn PerfPill(children: Children) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-1.5 font-medium text-ink-300">
            <svg class="h-4 w-4 text-bolt-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M20 6 9 17l-5-5" />
            </svg>
            {children()}
        </span>
    }
}

#[component]
fn Dot() -> impl IntoView {
    view! { <span class="hidden h-1 w-1 rounded-full bg-ink-600 sm:inline-block"></span> }
}
