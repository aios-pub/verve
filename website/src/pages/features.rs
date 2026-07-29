//! Features page: detailed feature breakdown with screenshots.

use leptos::prelude::*;
use crate::i18n::{t, t_string, use_i18n};

use crate::components::{FeatureCard, FeatureGroup};
use crate::consts::RELEASES_LATEST;
use crate::utils::route;

#[component]
pub fn Features() -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <PageHeader eyebrow="Features">
            {move || t!(i18n, pillars.title)}
        </PageHeader>

        <div class="mx-auto max-w-6xl px-4 py-16 sm:px-6 lg:px-8">
            // ── Community features ────────────────────────────────────────
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

            // ── Pro: Server triage ───────────────────────────────────────
            <div class="mt-20">
                <h3 class="flex items-center gap-2 text-xl font-bold">
                    <span class="text-pro-400">{move || t!(i18n, editions.pro)}</span>
                    <span class="text-ink-300">"·"</span>
                    <span class="text-lg font-semibold text-ink-200">{move || t!(i18n, pillars.terminal)}</span>
                </h3>
                <p class="mt-2 max-w-2xl text-sm text-ink-400">
                    {move || t_string!(i18n, features.server_triage_note)}
                </p>
            </div>

            <div class="mt-6 grid gap-5 lg:grid-cols-2">
                <FeatureGroup pro=true
                    title="SSH Terminal"
                    description=t_string!(i18n, features.ssh_d)
                    screenshot=Some("ssh-terminal.png")
                    specs={vec![
                        "Multi-tab sessions",
                        "ANSI / xterm-256color (true color)",
                        "Password / private-key auth",
                        "Jump host (ProxyJump, chained tunnels)",
                        "SFTP — list / mkdir / upload / download",
                        "Zmodem in-terminal rz / sz",
                        "SSH local port forwarding (-L)",
                        "TOFU known_hosts verification",
                        "Secure vault — keychain + AES-256-GCM / Argon2id",
                    ]}
                />
                <FeatureGroup pro=true
                    title="Docker"
                    description=t_string!(i18n, features.docker_d)
                    screenshot=Some("docker.png")
                    specs={vec![
                        "List / start / stop / restart / remove containers",
                        "Image list & prune",
                        "docker logs -f log streaming",
                        "docker exec -it (real PTY, multi-tab)",
                        "Remote daemon via DOCKER_HOST",
                        "Remote daemon via SSH tunnel (dial-stdio)",
                    ]}
                />
                <FeatureGroup pro=true
                    title="Kubernetes Shell"
                    description=t_string!(i18n, features.k8s_d)
                    screenshot=Some("k8s.png")
                    specs={vec![
                        "Parse ~/.kube/config, switch context",
                        "List pods & namespaces",
                        "kubectl logs -f",
                        "kubectl exec -it (PTY)",
                        "kubectl port-forward",
                        "Direct (API server) or SSH-tunnel mode",
                    ]}
                />
                <FeatureGroup pro=true
                    title="SFTP File Browser"
                    description=t_string!(i18n, features.sftp_d)
                    screenshot=Some("ssh-file.png")
                    specs={vec![
                        "List / new folder / rename",
                        "Upload / download with progress",
                        "Recursive rm -rf",
                        "Drag-and-drop transfers",
                        "Reuses the authenticated SSH session",
                    ]}
                />
            </div>

            // ── Pro: Knowledge tools ─────────────────────────────────────
            <div class="mt-20">
                <h3 class="flex items-center gap-2 text-xl font-bold">
                    <span class="text-pro-400">{move || t!(i18n, editions.pro)}</span>
                    <span class="text-ink-300">"·"</span>
                    <span class="text-lg font-semibold text-ink-200">{move || t!(i18n, features.knowledge_title)}</span>
                </h3>
            </div>

            <div class="mt-6 grid gap-5 lg:grid-cols-2">
                <FeatureGroup pro=true
                    title="Markdown Editor"
                    description=t_string!(i18n, features.md_d)
                    screenshot=Some("note.png")
                    specs={vec![
                        "Block editor with live-preview canvas",
                        "Notes tree — folders / pin / tags",
                        "Export notes to PDF (built-in fonts)",
                        "Standalone .md file editor (multi-tab)",
                        "Open via Finder double-click or `verve file.md`",
                        "Replaces MarkText-style tools",
                    ]}
                />
                <FeatureGroup pro=true
                    title="PDF Viewer & Editor"
                    description=t_string!(i18n, features.pdf_d)
                    screenshot=Some("pdf.png")
                    specs={vec![
                        "Pdfium-powered native rendering",
                        "Text / image / erase annotations",
                        "Page insert / delete / reorder",
                        "Replaces a standalone PDF editor",
                    ]}
                />
            </div>

            // ── Pro: Testing & cloud ─────────────────────────────────────
            <div class="mt-20">
                <h3 class="flex items-center gap-2 text-xl font-bold">
                    <span class="text-pro-400">{move || t!(i18n, editions.pro)}</span>
                    <span class="text-ink-300">"·"</span>
                    <span class="text-lg font-semibold text-ink-200">{move || t!(i18n, features.testing_title)}</span>
                </h3>
            </div>

            <div class="mt-6 grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
                <FeatureCard pro=true icon="" title="Stress Testing"
                    description="Native engine, concurrency/duration/QPS, live p50/p90/p95/p99 chart, scenario mode."
                    screenshot=Some("stress.png") />
                <FeatureCard pro=true icon="" title="Automated Testing"
                    description="Suite→Case→Step; Request/If/Loop/Wait/Script; apt.assert; JsonPath/Header/Status extractors."
                    screenshot=Some("auto_test.png") />
                <FeatureCard pro=true icon="" title="verve-server"
                    description="Self-host cloud sharing: push a project → public /s/<id> URL, /admin Web UI."
                    screenshot=None />
            </div>

            // ── Disk relocation (system tool) ────────────────────────────
            <div class="mt-20">
                <h3 class="flex items-center gap-2 text-xl font-bold">
                    <span class="text-bolt-300">{move || t!(i18n, features.relocate_title)}</span>
                </h3>
            </div>

            <div class="mt-6">
                <FeatureGroup pro=true
                    title="Disk Relocation"
                    description=t_string!(i18n, features.relocate_d)
                    screenshot=None
                    specs={vec![
                        "macOS — internal SSD ⇋ Thunderbolt/USB4 external NVMe",
                        "Auto-scan & smart-cluster large directories",
                        "Absolute-path symlinks (apps stay unaware)",
                        "Custom directories + auto-grouped tasks",
                        "Pause / resume, live speed & progress",
                        "Full transactional rollback (write-back + cleanup)",
                        "External-disk detection & space pre-check",
                    ]}
                />
            </div>

            // ── CTA ──────────────────────────────────────────────────────
            <div class="mt-16 flex flex-col items-center justify-center gap-4 rounded-2xl border border-ink-700 bg-ink-900 p-10 text-center sm:flex-row">
                <a href=RELEASES_LATEST class="btn-bolt rounded-lg px-6 py-3 font-semibold">
                    {move || t!(i18n, nav.get_app)}
                </a>
                <a href=route("/pricing") class="rounded-lg border border-ink-600 px-6 py-3 font-semibold text-ink-100 transition hover:border-bolt-500/50 hover:text-bolt-300">
                    {move || t!(i18n, editions.pro)}
                </a>
            </div>
        </div>
    }
}

#[component]
fn PageHeader(eyebrow: &'static str, children: Children) -> impl IntoView {
    view! {
        <section class="relative border-b border-ink-700 bg-grid">
            <div class="mx-auto max-w-6xl px-4 pb-14 pt-10 text-center sm:px-6 lg:px-8">
                <span class="text-xs font-semibold uppercase tracking-[0.2em] text-bolt-400">{eyebrow}</span>
                <h1 class="mt-2 text-4xl font-extrabold leading-tight tracking-tight sm:text-5xl">{children()}</h1>
            </div>
        </section>
    }
}
