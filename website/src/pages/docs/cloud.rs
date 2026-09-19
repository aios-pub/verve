//! Cloud docs & collaboration (Pioneer): document the self-hosted `verve-server`
//! standalone mode, cloud document sharing, and the /admin Web UI.

use leptos::prelude::*;
use crate::i18n::{t, t_string, use_i18n};

use crate::consts::REPO;

#[component]
pub fn CloudDocs() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <section class="relative isolate overflow-hidden border-b border-ink-700">
            <div class="bg-grid pointer-events-none absolute inset-0" aria-hidden="true"></div>
            <div class="relative z-10 mx-auto max-w-4xl px-4 pb-12 pt-10 sm:px-6 lg:px-8">
                <nav class="text-sm text-ink-500">
                    <a href="/verve/docs" class="hover:text-bolt-300 transition">{move || t!(i18n, docs.title)}</a>
                    " / "
                    <span class="text-ink-300">{move || t!(i18n, clouddocs.crumb)}</span>
                </nav>
                <div class="mt-4 flex items-center gap-3">
                    <span class="rounded-full bg-pro-500/15 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-pro-400 ring-1 ring-inset ring-pro-500/20">"Pioneer"</span>
                    <h1 class="text-3xl font-extrabold tracking-tight sm:text-4xl">
                        {move || t!(i18n, clouddocs.title)}
                    </h1>
                </div>
                <p class="mt-4 max-w-2xl text-base leading-relaxed text-ink-300">
                    {move || t!(i18n, clouddocs.intro)}
                </p>
            </div>
        </section>

        <div class="mx-auto max-w-4xl px-4 py-12 sm:px-6 lg:px-8">
            <div class="space-y-12">

                // ── Standalone server mode ───────────────────────────────
                <Block
                    n="01"
                    title={move || t!(i18n, clouddocs.s1_title)}
                    desc={move || t!(i18n, clouddocs.s1_desc)}
                >
                    <ul class="mt-4 space-y-2 text-sm text-ink-300">
                        {bullets(vec![
                            t_string!(i18n, clouddocs.s1_b1).to_string(),
                            t_string!(i18n, clouddocs.s1_b2).to_string(),
                            t_string!(i18n, clouddocs.s1_b3).to_string(),
                            t_string!(i18n, clouddocs.s1_b4).to_string(),
                        ])}
                    </ul>
                </Block>

                // ── Cloud document sharing ──────────────────────────────
                <Block
                    n="02"
                    title={move || t!(i18n, clouddocs.s2_title)}
                    desc={move || t!(i18n, clouddocs.s2_desc)}
                >
                    <ul class="mt-4 space-y-2 text-sm text-ink-300">
                        {bullets(vec![
                            t_string!(i18n, clouddocs.s2_b1).to_string(),
                            t_string!(i18n, clouddocs.s2_b2).to_string(),
                            t_string!(i18n, clouddocs.s2_b3).to_string(),
                        ])}
                    </ul>
                </Block>

                // ── Collaboration ───────────────────────────────────────
                <Block
                    n="03"
                    title={move || t!(i18n, clouddocs.s3_title)}
                    desc={move || t!(i18n, clouddocs.s3_desc)}
                >
                    <ul class="mt-4 space-y-2 text-sm text-ink-300">
                        {bullets(vec![
                            t_string!(i18n, clouddocs.s3_b1).to_string(),
                            t_string!(i18n, clouddocs.s3_b2).to_string(),
                            t_string!(i18n, clouddocs.s3_b3).to_string(),
                        ])}
                    </ul>
                </Block>

                // ── Admin Web UI ────────────────────────────────────────
                <Block
                    n="04"
                    title={move || t!(i18n, clouddocs.s4_title)}
                    desc={move || t!(i18n, clouddocs.s4_desc)}
                >
                    <ul class="mt-4 space-y-2 text-sm text-ink-300">
                        {bullets(vec![
                            t_string!(i18n, clouddocs.s4_b1).to_string(),
                            t_string!(i18n, clouddocs.s4_b2).to_string(),
                            t_string!(i18n, clouddocs.s4_b3).to_string(),
                        ])}
                    </ul>
                </Block>

                // ── Note: limitations ───────────────────────────────────
                <div class="rounded-xl border border-ink-700 bg-ink-900 p-5">
                    <p class="text-sm text-ink-300">
                        <span class="font-semibold text-ink-100">{move || t!(i18n, clouddocs.note_label)}" "</span>
                        {move || t!(i18n, clouddocs.note)}
                    </p>
                </div>

                // ── CTA: full deployment guide ──────────────────────────
                <div class="flex flex-col items-center gap-3 rounded-xl border border-ink-700 bg-ink-900 p-6 text-center sm:flex-row sm:justify-between sm:text-left">
                    <p class="text-sm text-ink-300">{move || t!(i18n, clouddocs.deploy_cta)}</p>
                    <a href="/verve/docs/verve-server" class="btn-bolt shrink-0 rounded-lg px-4 py-2 text-sm font-semibold">
                        {move || t!(i18n, clouddocs.deploy_link)}
                    </a>
                </div>

                <p class="text-center text-xs text-ink-500">
                    <a href=REPO target="_blank" rel="noopener" class="hover:text-bolt-300">{move || t!(i18n, clouddocs.source)}</a>
                </p>
            </div>
        </div>
    }
}

/// A numbered documentation block.
#[component]
fn Block(
    n: &'static str,
    title: impl IntoView + 'static,
    desc: impl IntoView + 'static,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="relative pl-12">
            <span class="absolute left-0 top-0 font-mono text-sm font-bold text-bolt-400">{n}</span>
            <h2 class="text-xl font-bold text-ink-100">{title}</h2>
            <p class="mt-2 text-sm leading-relaxed text-ink-400">{desc}</p>
            {children()}
        </div>
    }
}

/// Helper: render a Vec of bullets.
fn bullets(items: Vec<String>) -> impl IntoView {
    items
        .into_iter()
        .map(|item| {
            view! {
                <li class="flex items-start gap-2">
                    <svg class="mt-0.5 h-4 w-4 shrink-0 text-bolt-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M20 6 9 17l-5-5" />
                    </svg>
                    <span>{item}</span>
                </li>
            }
        })
        .collect::<Vec<_>>()
}
