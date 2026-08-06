//! Download page: platform cards, install steps, build-from-source.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

use crate::components::DownloadButtons;
use crate::components::fx::SpotlightCard;
use crate::consts::{REPO, VERVE_VERSION};

#[component]
pub fn Download() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <section class="relative isolate overflow-hidden border-b border-ink-700">
            <div class="bg-grid pointer-events-none absolute inset-0" aria-hidden="true"></div>
            <div class="relative z-10 mx-auto max-w-7xl px-4 pb-14 pt-10 text-center sm:px-6 lg:px-8">
                <span class="tech-label">"download"</span>
                <h1 class="mt-3 text-4xl font-extrabold leading-tight tracking-tight sm:text-5xl">
                    {move || t!(i18n, download.title)}
                </h1>
                <div class="rule-dashed mx-auto mt-5 w-40"></div>
            </div>
        </section>

        <div class="mx-auto max-w-4xl px-4 py-16 sm:px-6 lg:px-8">
            <DownloadButtons />

            // Install steps
            <div class="mt-16">
                <h2 class="text-2xl font-bold">{move || t!(i18n, download.install_title)}</h2>
                <div class="mt-6 grid gap-4 sm:grid-cols-3">
                    <InstallCard os="macOS" steps={vec!["下载 .dmg", "双击挂载", "拖入 Applications"]} />
                    <InstallCard os="Linux" steps={vec!["下载 .deb", "sudo dpkg -i", "从应用菜单启动"]} />
                    <InstallCard os="Windows" steps={vec!["下载 .msi", "双击安装", "从开始菜单启动"]} />
                </div>
            </div>

            // Build from source
            <SpotlightCard class="mt-16 rounded-2xl border border-white/5 bg-ink-900 p-8" corners="bolt">
                <span class="tech-label">"build from source"</span>
                <h2 class="mt-3 text-2xl font-bold">{move || t!(i18n, download.build_title)}</h2>
                <p class="mt-3 text-sm text-ink-400">{move || t!(i18n, download.build_d)}</p>
                <pre class="mt-4 overflow-x-auto rounded-lg bg-ink-950 p-4 text-sm text-ink-200">
                    <code>"git clone " {REPO} "\ncd verve\ncargo build --release"</code>
                </pre>
                <p class="mt-3 font-mono text-xs text-ink-500">
                    "ver. " {VERVE_VERSION}
                </p>
            </SpotlightCard>
        </div>
    }
}

#[component]
fn InstallCard(os: &'static str, steps: Vec<&'static str>) -> impl IntoView {
    view! {
        <SpotlightCard class="rounded-xl border border-white/5 bg-ink-900 p-5" corners="bolt">
            <h3 class="font-semibold text-ink-100">{os}</h3>
            <ol class="mt-3 space-y-2 text-sm text-ink-400">
                {steps.into_iter().enumerate().map(|(i, step)| {
                    view! {
                        <li class="flex gap-2">
                            <span class="font-mono text-bolt-400">{i + 1}</span>
                            <span>{step}</span>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ol>
        </SpotlightCard>
    }
}
