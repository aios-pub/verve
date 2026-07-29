//! Community vs Pro comparison table.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

#[component]
pub fn ComparisonTable() -> impl IntoView {
    let i18n = use_i18n();

    // (label key, community, pro)
    let rows = vec![
        ("API & Testing", true, true),
        ("HTTP Capture Proxy", true, true),
        ("Local Mock Server", true, true),
        ("Git Cross-Machine Sync", true, true),
        ("JSON Formatter", true, true),
        ("Hosts Manager", true, true),
        ("Document Sharing (local)", true, true),
        ("SSH Terminal + SFTP", false, true),
        ("Zmodem / Port Forwarding", false, true),
        ("Docker triage", false, true),
        ("Kubernetes triage", false, true),
        ("Stress Testing", false, true),
        ("Automated Testing", false, true),
        ("Markdown Notes + PDF", false, true),
        ("Self-hosted verve-server", false, true),
    ];

    view! {
        <div class="card overflow-hidden p-0">
            <table class="w-full border-collapse text-sm">
                <thead>
                    <tr class="bg-white/[0.02] text-left">
                        <th class="px-5 py-4 font-semibold text-ink-200">
                            {move || t!(i18n, compare.col_feature)}
                        </th>
                        <th class="px-5 py-4 text-center font-semibold text-ink-100">
                            {move || t!(i18n, editions.community)}
                            <span class="ml-2 rounded-full bg-bolt-500/10 px-2 py-0.5 text-[10px] font-medium text-bolt-300 ring-1 ring-inset ring-bolt-500/15">
                                {move || t!(i18n, editions.community_tag)}
                            </span>
                        </th>
                        <th class="px-5 py-4 text-center font-semibold text-ink-100">
                            {move || t!(i18n, editions.pro)}
                            <span class="ml-2 rounded-full bg-pro-500/15 px-2 py-0.5 text-[10px] font-medium text-pro-400 ring-1 ring-inset ring-pro-500/20">
                                {move || t!(i18n, editions.pro_tag)}
                            </span>
                        </th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|(label, community, pro)| {
                        view! {
                            <tr class="border-t border-white/[0.06] transition hover:bg-white/[0.02]">
                                <td class="px-5 py-3 text-ink-300">{label}</td>
                                <td class="px-5 py-3 text-center">{check(community)}</td>
                                <td class="px-5 py-3 text-center">{check(pro)}</td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

fn check(present: bool) -> impl IntoView {
    if present {
        view! { <span class="text-bolt-400">"✓"</span> }
    } else {
        view! { <span class="text-ink-600">"—"</span> }
    }
}
