//! Features page: detailed feature breakdown with screenshots.

use leptos::prelude::*;
use crate::i18n::{t, use_i18n};

use crate::components::FeatureCard;
use crate::consts::RELEASES_LATEST;
use crate::utils::route;

#[component]
pub fn Features() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <PageHeader eyebrow="Features">
            {move || t!(i18n, pillars.title)}
        </PageHeader>

        <div class="mx-auto max-w-7xl px-4 py-16 sm:px-6 lg:px-8">
            // Community features
            <h3 class="mb-6 flex items-center gap-2 text-xl font-bold">
                <span class="text-bolt-300">{move || t!(i18n, editions.community)}</span>
            </h3>
            <div class="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
                <FeatureCard icon="" title="API Debugging"       description="HTTP / gRPC-Web / TCP / SSE / WebSocket / Socket.IO, {{variable}} substitution across 4 scopes, pre-request & test scripts in JS." screenshot=Some("api.png") />
                <FeatureCard icon="" title="Project Tree"         description="Multi-level nesting with drag-and-drop reordering. Multi-environment variable management." screenshot=Some("project.png") />
                <FeatureCard icon="" title="HTTP Capture Proxy"   description="Capture plaintext HTTP traffic on 127.0.0.1:<port> and inspect requests in place." screenshot=Some("http-captrue.png") />
                <FeatureCard icon="" title="Local Mock Server"    description="Exact→Prefix→Regex matching with priority and template variables, on port 3097." screenshot=Some("mock.png") />
                <FeatureCard icon="" title="Document Sharing"     description="Self-contained HTML generation, QR/link export, expiration + password access control." screenshot=Some("doc.png") />
                <FeatureCard icon="" title="Git Cross-Machine Sync" description="Each workspace is a git branch; auto commit + sync every 30 min. Token never touches disk." screenshot=Some("git_sync_time.png") />
                <FeatureCard icon="" title="JSON Formatter"       description="Collapsible tree, instant validation, copy path & value." screenshot=Some("json-format.png") />
                <FeatureCard icon="" title="Hosts Manager"        description="Manage hosts profiles without editing system files." screenshot=Some("hosts.png") />
                <FeatureCard icon="" title="Themes & i18n"        description="22 built-in themes, zh-CN / English UI." screenshot=Some("theme.png") />
            </div>

            // Pro features
            <h3 class="mb-6 mt-16 flex items-center gap-2 text-xl font-bold">
                <span class="text-pro-400">{move || t!(i18n, editions.pro)}</span>
            </h3>
            <div class="grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
                <FeatureCard pro=true icon="" title="SSH Terminal"     description="Password / key auth, multi-tab, ANSI & xterm-256color, jump host, TOFU known_hosts, secure credential vault." screenshot=Some("ssh-terminal.png") />
                <FeatureCard pro=true icon="" title="SFTP + Zmodem"    description="Browse & transfer files, rz/sz over the terminal, SSH local port forwarding (-L)." screenshot=Some("ssh-file.png") />
                <FeatureCard pro=true icon="" title="Docker triage"    description="Start/stop/restart/remove containers, image list & prune, logs -f, exec -it with a real PTY." screenshot=Some("docker.png") />
                <FeatureCard pro=true icon="" title="Kubernetes triage" description="Parse kubeconfig, pods & namespaces, logs -f, exec -it, port-forward; Direct or SSH-tunnel mode." screenshot=Some("k8s.png") />
                <FeatureCard pro=true icon="" title="Stress Testing"   description="Native oha-based engine, live p50/p90/p95/p99 chart, scenario mode." screenshot=Some("stress.png") />
                <FeatureCard pro=true icon="" title="Automated Testing" description="Suite→Case→Step; Request/If/Loop/Wait/Script steps; apt.assert; JsonPath/Header/Status extractors." screenshot=Some("auto_test.png") />
                <FeatureCard pro=true icon="" title="Markdown Notes"   description="Block editor with live preview and PDF export." screenshot=Some("note.png") />
                <FeatureCard pro=true icon="" title="PDF Viewer"       description="Pdfium-powered viewer/editor; standalone .md file editor." screenshot=Some("pdf.png") />
                <FeatureCard pro=true icon="" title="verve-server"     description="Self-host cloud sharing: push a project → public /s/<id> URL, /admin Web UI." screenshot=None />
            </div>

            <div class="mt-16 flex flex-col items-center justify-center gap-4 rounded-2xl border border-white/5 bg-ink-900 p-10 text-center sm:flex-row">
                <a href=RELEASES_LATEST class="rounded-lg bg-bolt-400 px-6 py-3 font-semibold text-ink-950 hover:bg-bolt-300 transition">
                    {move || t!(i18n, nav.get_app)}
                </a>
                <a href=route("/pricing") class="rounded-lg border border-white/15 px-6 py-3 font-semibold text-ink-100 hover:border-bolt-500/50 hover:text-bolt-300 transition">
                    {move || t!(i18n, editions.pro)}
                </a>
            </div>
        </div>
    }
}

#[component]
fn PageHeader(eyebrow: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="border-b border-white/5 bg-grid">
            <div class="mx-auto max-w-7xl px-4 py-16 text-center sm:px-6 lg:px-8">
                <span class="text-xs font-semibold uppercase tracking-[0.2em] text-bolt-400">{eyebrow}</span>
                <h1 class="mt-2 text-4xl font-extrabold tracking-tight sm:text-5xl">{children()}</h1>
            </div>
        </section>
    }
}
