//! Community vs Pro comparison table.

use leptos::prelude::*;
use crate::i18n::{t, t_string, use_i18n};

/// Which i18n key a comparison row uses for its label.
#[derive(Clone, Copy)]
enum RowKey {
    Api,
    Capture,
    Mock,
    Git,
    Json,
    Hosts,
    DocShare,
    Ssh,
    Zmodem,
    Docker,
    K8s,
    Stress,
    Autotest,
    Notes,
    MdEditor,
    Server,
    Relocate,
}

#[component]
pub fn ComparisonTable() -> impl IntoView {
    let i18n = use_i18n();

    // (row key, community, pro)
    let rows = vec![
        (RowKey::Api, true, true),
        (RowKey::Capture, true, true),
        (RowKey::Mock, true, true),
        (RowKey::Git, true, true),
        (RowKey::Json, true, true),
        (RowKey::Hosts, true, true),
        (RowKey::DocShare, true, true),
        (RowKey::Ssh, false, true),
        (RowKey::Zmodem, false, true),
        (RowKey::Docker, false, true),
        (RowKey::K8s, false, true),
        (RowKey::Stress, false, true),
        (RowKey::Autotest, false, true),
        (RowKey::Notes, false, true),
        (RowKey::MdEditor, false, true),
        (RowKey::Server, false, true),
        (RowKey::Relocate, false, true),
    ];

    // Render the label reactively (a String) so it follows the current locale.
    let label = move |key: RowKey| -> String {
        match key {
            RowKey::Api => t_string!(i18n, compare.r_api),
            RowKey::Capture => t_string!(i18n, compare.r_capture),
            RowKey::Mock => t_string!(i18n, compare.r_mock),
            RowKey::Git => t_string!(i18n, compare.r_git),
            RowKey::Json => t_string!(i18n, compare.r_json),
            RowKey::Hosts => t_string!(i18n, compare.r_hosts),
            RowKey::DocShare => t_string!(i18n, compare.r_doc_share),
            RowKey::Ssh => t_string!(i18n, compare.r_ssh),
            RowKey::Zmodem => t_string!(i18n, compare.r_zmodem),
            RowKey::Docker => t_string!(i18n, compare.r_docker),
            RowKey::K8s => t_string!(i18n, compare.r_k8s),
            RowKey::Stress => t_string!(i18n, compare.r_stress),
            RowKey::Autotest => t_string!(i18n, compare.r_autotest),
            RowKey::Notes => t_string!(i18n, compare.r_notes),
            RowKey::MdEditor => t_string!(i18n, compare.r_mdeditor),
            RowKey::Server => t_string!(i18n, compare.r_server),
            RowKey::Relocate => t_string!(i18n, compare.r_relocate),
        }
        .to_string()
    };

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
                    {rows.into_iter().map(|(key, community, pro)| {
                        view! {
                            <tr class="row-hover-bar border-t border-ink-700 transition hover:bg-ink-800/40">
                                <td class="px-5 py-3 text-ink-300">
                                    {move || label(key)}
                                </td>
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
        view! { <span class="check-glow text-lg leading-none">"✓"</span> }
    } else {
        view! { <span class="text-ink-600">"—"</span> }
    }
}

