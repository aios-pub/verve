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

/// A larger, detailed feature group: title, one-line description, a bullet
/// list of specs, and an optional screenshot. Used for the Pro server-triage
/// capabilities (SSH / Docker / Kubernetes) and other headline Pro features.
#[component]
pub fn FeatureGroup(
    title: &'static str,
    description: &'static str,
    /// Bullet-point specs (e.g. "Multi-tab sessions", "ANSI / xterm-256color").
    specs: Vec<&'static str>,
    screenshot: Option<&'static str>,
    /// Whether this is a Pro-only feature (adds a Pro badge).
    #[prop(optional)]
    pro: bool,
) -> impl IntoView {
    let alt_text = title;
    view! {
        <div class="card group relative flex flex-col overflow-hidden p-6">
            {pro.then(|| view! {
                <span class="absolute right-5 top-5 rounded-full bg-pro-500/15 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-pro-400 ring-1 ring-inset ring-pro-500/20">
                    "Pro"
                </span>
            })}

            <h3 class="text-lg font-semibold text-ink-100">{title}</h3>
            <p class="mt-1.5 text-sm leading-relaxed text-ink-400">{description}</p>

            <ul class="mt-4 grid gap-2 sm:grid-cols-2">
                {specs.into_iter().map(|spec| {
                    view! {
                        <li class="flex items-start gap-2 text-sm text-ink-300">
                            <svg class="mt-0.5 h-4 w-4 shrink-0 text-bolt-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M20 6 9 17l-5-5" />
                            </svg>
                            <span>{spec}</span>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ul>

            {screenshot.map(|s| {
                let src = asset(&format!("screenshots/{s}"));
                view! {
                    <div class="mt-5 overflow-hidden rounded-lg border border-white/[0.06] bg-ink-950">
                        <img
                            src=src
                            alt=alt_text
                            loading="lazy"
                            class="w-full object-top transition duration-500 group-hover:scale-[1.02]"
                        />
                    </div>
                }
            })}
        </div>
    }
}
