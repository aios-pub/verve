//! Hero section: particle constellation, aurora, mouse spotlight, shimmer
//! title, animated stat readouts, a cycling "boot line", and CTAs.

use leptos::html::Div;
use leptos::prelude::*;
use leptos_use::use_mouse_in_element;

use crate::components::fx::{BootLine, CountUp, ParticleField, TechBadge};
use crate::consts::RELEASES_LATEST;
use crate::i18n::{t, use_i18n};
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
        <section class="relative isolate overflow-hidden">
            // Layer 1: drifting grid (its own absolute layer — never put bg-grid
            // on a content-bearing element, its mask would fade the content out).
            <div class="absolute inset-0 z-0 bg-grid bg-grid-animated"></div>

            // Layer 2: WASM particle constellation
            <ParticleField />

            // Layer 3: aurora blobs
            <div class="pointer-events-none absolute inset-0 z-[2] overflow-hidden">
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

            // Layer 4: top radial wash + slow scanline
            <div class="pointer-events-none absolute inset-x-0 top-0 z-[2] h-[520px] bg-[radial-gradient(ellipse_60%_50%_at_50%_0%,rgba(212,227,23,0.16),transparent_70%)]"></div>
            <div class="scanline z-[3]" style="top:0;"></div>

            // Layer 5: mouse-follow spotlight
            <div
                class="pointer-events-none absolute inset-0 z-[3] transition-opacity duration-500"
                style=move || spotlight_style.get()
            ></div>

            <div
                node_ref=el
                class="relative z-10 mx-auto max-w-5xl px-4 pt-12 pb-20 sm:px-6 lg:px-8 lg:pt-16 lg:pb-24"
            >
                <div class="mx-auto max-w-3xl text-center">
                    // Badge + tech tags
                    <div class="flex flex-col items-center gap-3">
                        <span class="inline-flex items-center gap-2 rounded-full border border-bolt-500/25 bg-bolt-500/[0.06] px-3.5 py-1.5 text-xs font-medium text-bolt-200 backdrop-blur">
                            <span class="relative flex h-1.5 w-1.5">
                                <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-bolt-400 opacity-75"></span>
                                <span class="relative inline-flex h-1.5 w-1.5 rounded-full bg-bolt-400"></span>
                            </span>
                            {move || t!(i18n, hero.badge)}
                        </span>
                        <div class="flex items-center gap-3">
                            <TechBadge label="rust" />
                            <TechBadge label="wasm" />
                            <TechBadge label="gpu" live=true />
                        </div>
                    </div>

                    // Title
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

                    // Cycling boot status line
                    <BootLine />

                    // HUD stat readouts (count up on scroll into view)
                    <div class="mt-10 grid grid-cols-2 gap-px overflow-hidden rounded-xl border border-white/[0.07] bg-white/[0.04] text-center sm:grid-cols-4">
                        <Stat label=t!(i18n, hud.cold_boot)>
                            <span class="text-bolt-300">"<" <CountUp value=1.0 decimals=1 suffix="s" /></span>
                        </Stat>
                        <Stat label=t!(i18n, hud.mem_baseline)>
                            <span class="text-bolt-300">"<" <CountUp value=100.0 suffix="MB" /></span>
                        </Stat>
                        <Stat label=t!(i18n, hud.frame_rate)>
                            <span class="text-bolt-300">"~" <CountUp value=60.0 suffix="fps" /></span>
                        </Stat>
                        <Stat label=t!(i18n, hud.mem_safe)>
                            <span class="text-bolt-300"><CountUp value=100.0 suffix="%" /></span>
                        </Stat>
                    </div>
                </div>
            </div>
        </section>
    }
}

#[component]
fn Stat(label: impl IntoView + 'static, children: Children) -> impl IntoView {
    view! {
        <div class="bg-ink-950/40 px-4 py-4 backdrop-blur-sm">
            <div class="stat-value text-2xl sm:text-3xl">{children()}</div>
            <div class="mt-1 text-[11px] font-medium uppercase tracking-wider text-ink-500">
                {label}
            </div>
        </div>
    }
}
