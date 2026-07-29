//! Site footer: brand tagline, link columns, disclaimer.

use leptos::prelude::*;

use crate::consts::{REPO, RELEASES_ALL};
use crate::i18n::{t, use_i18n};
use crate::utils::{asset, route};

#[component]
pub fn Footer() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <footer class="border-t border-white/5 bg-ink-900">
            <div class="mx-auto max-w-7xl px-4 py-12 sm:px-6 lg:px-8">
                <div class="grid grid-cols-2 gap-8 md:grid-cols-4">
                    // Brand
                    <div class="col-span-2">
                        <div class="flex items-center gap-2 font-bold">
                            <img src=asset("icons/verve.png") alt="Verve" class="h-6 w-6" />
                            <span>"Verve"</span>
                        </div>
                        <p class="mt-3 max-w-xs text-sm text-ink-400">
                            {move || t!(i18n, footer.tagline)}
                        </p>
                    </div>

                    // Product links
                    <div>
                        <h4 class="text-xs font-semibold uppercase tracking-wider text-ink-300">
                            {move || t!(i18n, footer.product)}
                        </h4>
                        <ul class="mt-3 space-y-2 text-sm text-ink-400">
                            <li>
                                <a class="hover:text-bolt-300 transition" href=route("/features")>
                                    {move || t!(i18n, nav.features)}
                                </a>
                            </li>
                            <li>
                                <a class="hover:text-bolt-300 transition" href=route("/download")>
                                    {move || t!(i18n, nav.download)}
                                </a>
                            </li>
                            <li>
                                <a class="hover:text-bolt-300 transition" href=route("/pricing")>
                                    {move || t!(i18n, nav.pricing)}
                                </a>
                            </li>
                        </ul>
                    </div>

                    // Community links
                    <div>
                        <h4 class="text-xs font-semibold uppercase tracking-wider text-ink-300">
                            {move || t!(i18n, footer.community)}
                        </h4>
                        <ul class="mt-3 space-y-2 text-sm text-ink-400">
                            <li>
                                <a class="hover:text-bolt-300 transition" href=REPO target="_blank" rel="noopener">
                                    {move || t!(i18n, footer.github)}
                                </a>
                            </li>
                            <li>
                                <a class="hover:text-bolt-300 transition" href=RELEASES_ALL target="_blank" rel="noopener">
                                    {move || t!(i18n, footer.releases)}
                                </a>
                            </li>
                        </ul>
                    </div>
                </div>

                // Disclaimer
                <div class="mt-10 rounded-lg border border-white/5 bg-ink-850 p-4">
                    <p class="text-xs font-semibold text-ink-300">
                        {move || t!(i18n, footer.disclaimer_title)}
                    </p>
                    <p class="mt-1 text-xs leading-relaxed text-ink-400">
                        {move || t!(i18n, footer.disclaimer)}
                    </p>
                </div>

                <p class="mt-6 text-center text-xs text-ink-500">
                    "© 2026 Verve. "
                    {move || t!(i18n, footer.rights)}
                </p>
            </div>
        </footer>
    }
}
