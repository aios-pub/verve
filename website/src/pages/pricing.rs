//! Pricing page: comparison table, pricing cards, sponsor QR.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

use crate::components::pricing::SponsorQR;
use crate::components::{ComparisonTable, PricingCards};

#[component]
pub fn Pricing() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <section class="border-b border-white/5 bg-grid">
            <div class="mx-auto max-w-7xl px-4 py-16 text-center sm:px-6 lg:px-8">
                <h1 class="text-4xl font-extrabold tracking-tight sm:text-5xl">
                    {move || t!(i18n, pricing.title)}
                </h1>
                <p class="mx-auto mt-4 max-w-2xl text-ink-400">
                    {move || t!(i18n, pricing.subtitle)}
                </p>
            </div>
        </section>

        <div class="mx-auto max-w-7xl px-4 py-16 sm:px-6 lg:px-8">
            <PricingCards />

            <div class="mt-20">
                <h2 class="mb-8 text-center text-2xl font-bold">{move || t!(i18n, compare.title)}</h2>
                <div class="mx-auto max-w-4xl">
                    <ComparisonTable />
                </div>
            </div>

            <div class="mx-auto mt-16 max-w-4xl">
                <SponsorQR />
            </div>
        </div>
    }
}
