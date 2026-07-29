//! Reusable feature card: icon, title, description, optional screenshot.

use leptos::prelude::*;

use crate::utils::asset;

#[component]
pub fn FeatureCard(
    icon: &'static str,
    title: &'static str,
    description: &'static str,
    screenshot: Option<&'static str>,
    /// Whether this is a Pro-only feature (adds a Pro badge).
    #[prop(optional)]
    pro: bool,
) -> impl IntoView {
    let alt_text = title;
    view! {
        <div class="card group relative flex flex-col overflow-hidden p-5">
            // Pro badge
            {pro.then(|| view! {
                <span class="absolute right-4 top-4 rounded-full bg-pro-500/15 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-pro-400 ring-1 ring-inset ring-pro-500/20">
                    "Pro"
                </span>
            })}

            <h3 class="text-base font-semibold text-ink-100">{title}</h3>

            <p class="mt-2 flex-1 text-sm leading-relaxed text-ink-400">{description}</p>

            // Optional screenshot
            {screenshot.map(|s| {
                let src = asset(&format!("screenshots/{s}"));
                view! {
                    <div class="mt-4 overflow-hidden rounded-lg border border-white/[0.06] bg-ink-950">
                        <img
                            src=src
                            alt=alt_text
                            loading="lazy"
                            class="w-full object-top transition duration-500 group-hover:scale-[1.03]"
                        />
                    </div>
                }
            })}
        </div>
    }
}
