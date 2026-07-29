//! Pricing cards: Community (free) vs Pro (sponsored).

use leptos::prelude::*;
use crate::i18n::{t, t_string, use_i18n};

use crate::consts::RELEASES_LATEST;
use crate::utils::{asset, route};

#[component]
pub fn PricingCards() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="grid gap-6 md:grid-cols-2">
            // Community
            <div class="card relative flex flex-col p-8">
                <h3 class="text-xl font-bold">{move || t!(i18n, editions.community)}</h3>
                <span class="mt-1.5 inline-flex w-fit items-center rounded-full bg-bolt-500/10 px-2.5 py-0.5 text-xs font-medium text-bolt-300 ring-1 ring-inset ring-bolt-500/15">
                    {move || t!(i18n, editions.community_tag)}
                </span>
                <p class="mt-4 flex-1 text-sm leading-relaxed text-ink-400">
                    {move || t!(i18n, editions.community_d)}
                </p>
                <div class="mt-6">
                    <span class="text-4xl font-extrabold text-ink-100">"¥0"</span>
                </div>
                <a
                    href=RELEASES_LATEST
                    class="mt-6 block rounded-xl border border-white/10 bg-white/[0.03] py-3 text-center text-sm font-semibold text-ink-100 transition hover:border-bolt-500/40 hover:text-bolt-200"
                >
                    {move || t!(i18n, pricing.cta_community)}
                </a>
            </div>

            // Pro
            <div class="card relative flex flex-col overflow-hidden border-pro-500/30 p-8 before:absolute before:inset-0 before:bg-gradient-to-b before:from-pro-500/[0.08] before:to-transparent before:pointer-events-none">
                <div class="relative">
                    <span class="absolute -top-1 right-0 rounded-full bg-pro-500/20 px-2.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-pro-400 ring-1 ring-inset ring-pro-500/25">
                        {move || t!(i18n, pricing.most_popular)}
                    </span>
                    <h3 class="text-xl font-bold">{move || t!(i18n, editions.pro)}</h3>
                    <span class="mt-1.5 inline-flex w-fit items-center rounded-full bg-pro-500/15 px-2.5 py-0.5 text-xs font-medium text-pro-400 ring-1 ring-inset ring-pro-500/20">
                        {move || t!(i18n, editions.pro_tag)}
                    </span>
                    <p class="mt-4 flex-1 text-sm leading-relaxed text-ink-400">
                        {move || t!(i18n, editions.pro_d)}
                    </p>
                    <div class="mt-6 flex items-baseline gap-3">
                        <div>
                            <span class="text-xs text-ink-400">{move || t!(i18n, pricing.early_bird)}</span>
                            <span class="ml-1 text-4xl font-extrabold text-ink-100">"¥99"</span>
                        </div>
                        <div class="text-ink-500">
                            <span class="text-xs">{move || t!(i18n, pricing.regular)}</span>
                            <span class="ml-1 text-2xl font-bold line-through">"¥199"</span>
                        </div>
                    </div>
                    <a
                        href=route("/pricing")
                        class="mt-6 block rounded-xl bg-pro-500 py-3 text-center text-sm font-semibold text-white transition hover:bg-pro-600"
                    >
                        {move || t!(i18n, pricing.cta_pro)}
                    </a>
                    <p class="mt-3 text-center text-xs text-ink-500">
                        {move || t!(i18n, pricing.per_once)}
                    </p>
                </div>
            </div>
        </div>
    }
}

/// WeChat sponsorship QR block used on the pricing page.
#[component]
pub fn SponsorQR() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="mt-12 flex flex-col items-center rounded-2xl border border-white/5 bg-ink-900 p-8 text-center">
            <img
                src=asset("wechat_official.jpg")
                alt=t_string!(i18n, pricing.sponsor_qr_alt)
                class="h-40 w-40 rounded-lg"
            />
            <p class="mt-4 max-w-sm text-sm text-ink-400">
                {move || t!(i18n, pricing.sponsor_note)}
            </p>
        </div>
    }
}
