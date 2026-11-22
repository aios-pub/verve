//! Home page: hero, why-Rust table, capability pillars, download.

use leptos::prelude::*;
use crate::i18n::{t, t_string, use_i18n};

use crate::components::fx::SpotlightCard;
use crate::components::{DownloadButtons, FeatureCard, Hero, Reveal};

#[component]
pub fn Home() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <Hero />

        // Why Rust + GPUI
        <section class="mx-auto max-w-5xl px-4 py-20 sm:px-6 lg:py-28">
            <Reveal>
                <SectionTitle eyebrow="Rust + GPUI">
                    {move || t!(i18n, why.title)}
                </SectionTitle>
                <p class="mx-auto mt-4 mb-12 max-w-xl text-center text-sm text-ink-400">
                    {move || t!(i18n, why.subtitle)}
                </p>
            </Reveal>

            <Reveal stagger=true>
                <div class="grid gap-4 sm:grid-cols-2">
                    <WhyCard
                        index="01"
                        label=t_string!(i18n, why.row_startup)
                        v=t_string!(i18n, why.row_startup_v)
                        e=t_string!(i18n, why.row_startup_e)
                    />
                    <WhyCard
                        index="02"
                        label=t_string!(i18n, why.row_ram)
                        v=t_string!(i18n, why.row_ram_v)
                        e=t_string!(i18n, why.row_ram_e)
                    />
                    <WhyCard
                        index="03"
                        label=t_string!(i18n, why.row_render)
                        v=t_string!(i18n, why.row_render_v)
                        e=t_string!(i18n, why.row_render_e)
                    />
                    <WhyCard
                        index="04"
                        label=t_string!(i18n, why.row_safety)
                        v=t_string!(i18n, why.row_safety_v)
                        e=t_string!(i18n, why.row_safety_e)
                    />
                </div>
            </Reveal>
        </section>

        // Capability pillars
        <section class="border-y border-white/[0.06] bg-ink-900/40">
            <div class="mx-auto max-w-6xl px-4 py-20 sm:px-6 lg:py-28">
                <Reveal>
                    <SectionTitle eyebrow="Features">
                        {move || t!(i18n, pillars.title)}
                    </SectionTitle>
                </Reveal>
                <Reveal stagger=true>
                    <div class="mt-12 grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
                        <FeatureCard icon="" title=t_string!(i18n, pillars.api)       description=t_string!(i18n, pillars.api_d)       screenshot=Some("api.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.terminal)   description=t_string!(i18n, pillars.terminal_d)   screenshot=Some("ssh-terminal.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.testing)    description=t_string!(i18n, pillars.testing_d)    screenshot=Some("stress.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.mock)       description=t_string!(i18n, pillars.mock_d)       screenshot=Some("mock.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.docs)       description=t_string!(i18n, pillars.docs_d)       screenshot=Some("doc.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.notes)      description=t_string!(i18n, pillars.notes_d)      screenshot=Some("note.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.mdeditor)   description=t_string!(i18n, pillars.mdeditor_d)   screenshot=None />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.hosts)      description=t_string!(i18n, pillars.hosts_d)      screenshot=Some("hosts.png") />
                        <FeatureCard icon="" title=t_string!(i18n, pillars.json)       description=t_string!(i18n, pillars.json_d)       screenshot=Some("json-format.png") />
                    </div>
                </Reveal>
            </div>
        </section>

        // Download CTA
        <section class="border-t border-white/[0.06]">
            <div class="mx-auto max-w-4xl px-4 py-20 sm:px-6 lg:py-28">
                <Reveal>
                    <DownloadButtons />
                </Reveal>
            </div>
        </section>
    }
}

/// Shared section title with a mono eyebrow label and a hairline rule.
#[component]
fn SectionTitle(eyebrow: &'static str, children: Children) -> impl IntoView {
    view! {
        <div class="text-center">
            <span class="tech-label">{eyebrow}</span>
            <h2 class="mt-3 text-3xl font-bold leading-snug tracking-tight sm:text-4xl">{children()}</h2>
            <div class="rule-dashed mx-auto mt-5 w-40"></div>
        </div>
    }
}

/// A single Verve-vs-Electron comparison card. Strings are pre-evaluated.
#[component]
fn WhyCard(index: &'static str, label: &'static str, v: &'static str, e: &'static str) -> impl IntoView {
    view! {
        <SpotlightCard class="card p-5" corners="bolt">
            <div class="flex items-start justify-between gap-3">
                <p class="text-sm font-medium text-ink-400">{label}</p>
                <span class="font-mono text-[11px] tracking-wider text-ink-600">{index}</span>
            </div>
            <p class="mt-2 text-xl font-bold text-bolt-300">{v}</p>
            <div class="mt-3 flex items-center gap-2 border-t border-white/[0.06] pt-3 text-sm text-ink-500">
                <span class="rounded bg-white/[0.05] px-1.5 py-0.5 font-mono text-[10px] font-medium uppercase tracking-wider text-ink-400">"Electron"</span>
                <span>{e}</span>
            </div>
        </SpotlightCard>
    }
}
