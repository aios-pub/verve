<div align="center">

# ⚡ Verve

**A native, GPU-accelerated developer workbench built with Rust + GPUI — unifying API debugging, traffic capture, server triage, load testing, mock, notes, hosts and JSON tools in a single window.**

### A native developer workbench

Verve is built on **Rust + [GPUI](https://github.com/zed-industries/zed)** (the same GPU-accelerated UI framework that powers the Zed editor) — not Electron. The whole app is rendered on the GPU, starts in under a second, and stays well under 100 MB of RAM, so it stays responsive even while streaming container logs or running a load test.

`API & Testing` · `Terminal & Triage` · `Mock & Capture` · `Docs & Git` · `Notes` · `Hosts` · `JSON`

<br/>

<a href="https://aios-rs.github.io/verve/">
  <img src="https://img.shields.io/badge/website-verve.app-bolt?style=flat-square&labelColor=0a0c10&color=d4e317" alt="Official website" />
</a>
<a href="https://github.com/aios-rs/verve/releases/latest">
  <img src="https://img.shields.io/github/v/release/aios-rs/verve?style=flat-square&label=latest%20release&labelColor=0a0c10&color=d4e317" alt="Latest release" />
</a>
<a href="https://github.com/aios-rs/verve">
  <img src="https://img.shields.io/badge/license-AGPL--3.0%20%2F%20Pro-d4e317?style=flat-square&labelColor=0a0c10" alt="License" />
</a>

<br/>

> 🚫 **Stop juggling Postman + an SSH client like Termius + Swagger + a notes app + a Markdown editor like MarkText + a PDF editor + a JSON formatter + a hosts editor…**
> Verve absorbs the high-frequency dev workflow into one native window — built in Rust + GPUI for developers who refuse to settle for sluggish Electron bloat.

<br/>

<table>
  <tr>
    <td align="center"><b>🦀 Rust + GPUI</b><br/><sub>GPU rendering, no Chromium</sub></td>
    <td align="center"><b>⚡ &lt;1s Startup</b><br/><sub>Instant launch</sub></td>
    <td align="center"><b>💾 &lt;100MB RAM</b><br/><sub>5× lighter</sub></td>
    <td align="center"><b>🔒 Offline-First</b><br/><sub>Your data stays local</sub></td>
    <td align="center"><b>🖥️ Cross-Platform</b><br/><sub>macOS · Linux · Windows</sub></td>
  </tr>
</table>

<br/>

[Features](#-features) · [Community vs Pro](#-community-vs-pro) · [Getting Started](#-getting-started) · [Pro Edition](#-pro-edition--sponsor-to-unlock)

</div>

---

## 🦀 Built with Rust + GPUI — Why It Matters

Verve is built in **Rust** and rendered with **[GPUI](https://github.com/zed-industries/zed)** — the same GPU-accelerated framework that powers the Zed editor. No Electron, no Chromium. Here's the real-world difference:

| | Verve (Rust + GPUI) | Electron-based tools |
|---|---|---|
| ⚡ **Startup** | < 1 second | 3–5 seconds |
| 💾 **Memory** | < 100 MB | 500 MB+ |
| 🎨 **Rendering** | Native GPU, ~60fps | Chromium software compositing |
| 🛡️ **Safety** | Memory-safe, zero-cost abstraction | GC pauses, V8 overhead |

What this means for you: instant responses when debugging APIs, fluid terminals under heavy load, and a battery-friendly footprint that stays light all day.

---

## 🆍 Two Editions

Verve comes in two editions, with the same native app and the same daily API workflow — the Pro Edition adds server triage, testing, knowledge tooling, and cloud sharing on top.

### 🆍 Community Edition — Free & Open-Source

The **Community Edition** is free and released under the **AGPL-3.0** license. It covers everything an individual developer needs for daily API work: full HTTP debugging (incl. multi-protocol clients), traffic capture, JSON formatting, hosts management, local mock server, local document sharing, **and Git-based cross-machine sync** — in a fast, lightweight native app. It is intentionally strong enough to stand on its own against any individual API client.

### 💎 Pro Edition — Sponsor to Unlock

The **Pro Edition** is obtained via **sponsorship** (early-bird **¥99**, regular **¥199**). It layers on the advanced, professional capabilities an individual API client doesn't cover: server triage (**SSH / Docker / Kubernetes pod inspection**), **stress testing** and **automated test suites**, a **Markdown notes & file editor** (replaces tools like MarkText), a **PDF viewer/editor**, and **cloud document sharing** (push to a self-hosted `verve-server` for a public URL).

→ Full feature comparison: [Community vs Pro](#-community-vs-pro)
→ How to get it: [Pro Edition](#-pro-edition--sponsor-to-unlock)

---

## 🆚 Community vs Pro

**Edition principle**: the Community Edition covers everything an individual developer needs for daily, standalone API work (strong enough to compete head-on with Postman). The Pro Edition layers on professional capabilities beyond individual API debugging — server triage, testing, knowledge tooling, and cloud deployment.

> ✅ Included in both · ❌ Not in this edition · 💎 Pro-only feature

### Overview

| | 🆍 Community Edition | 💎 Pro Edition |
|---|---|---|
| **Positioning** | Individual developer's daily API toolbox | Toolbox + server triage + testing + knowledge + cloud |
| **License** | AGPL-3.0 (open-source) | Proprietary (Verve Pro License) |
| **Source code** | Open & auditable | Closed |
| **How to get** | Free download / build from source | Sponsor ¥99 early-bird / ¥199 regular |
| **Audience** | Individual devs, students, OSS community | Pro devs, ops engineers, small teams |
| **Platforms** | macOS · Linux · Windows | macOS · Linux · Windows |

### 🧪 API & Testing

| Feature | Community | Pro |
|---|:---:|:---:|
| HTTP API debugging (GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS) | ✅ | ✅ |
| Request body (none / form-data / x-www-form-urlencoded / raw JSON·XML·Text·HTML·JS) | ✅ | ✅ |
| `{{variable}}` placeholder (request > folder > env > global priority) | ✅ | ✅ |
| Pre-request & Tests scripts (JavaScript, boa_engine) | ✅ | ✅ |
| Multi-protocol clients — **HTTP / gRPC(gRPC-Web) / TCP / SSE / WebSocket / Socket.IO** | ✅ | ✅ |
| Response panel (status / time / size / headers / body, JSON pretty) | ✅ | ✅ |
| Project tree (folders, multi-level nesting, drag-reorder) | ✅ | ✅ |
| Multi-environment variable management (4-scope priority) | ✅ | ✅ |
| Request/response history | ✅ | ✅ |
| Local Mock server (Exact → Prefix → Regex matching, priority, template vars) | ✅ | ✅ |
| **HTTP capture proxy** — plaintext HTTP only on `127.0.0.1:<port>` (no HTTPS MITM) | ✅ | ✅ |
| **Git cross-machine sync** — per-workspace branch, auto commit + sync, HTTPS + token auth | ✅ | ✅ |
| **Stress testing** — native engine, concurrency/duration/QPS, live latency chart (p50/p90/p95/p99) | ❌ | 💎 |
| Stress scenario mode — multi-step test cases looped across workers | ❌ | 💎 |
| **Automated test suites** — suite/case/step, Request/If/Loop/Wait/Script steps, `apt.assert` + JsonPath/Header/Status extractors | ❌ | 💎 |

### 🛠️ Dev Utilities

| Feature | Community | Pro |
|---|:---:|:---:|
| JSON formatter (collapsible tree) | ✅ | ✅ |
| Hosts manager (read `/etc/hosts`, profiles, env binding) | ✅ | ✅ |
| 22 built-in themes (Catppuccin / Gruvbox / Tokyo Night / Solarized …) | ✅ | ✅ |
| i18n (Simplified Chinese default / English) | ✅ | ✅ |
| Import (Postman v2.1 / OpenAPI 3 / Swagger 2.0 / Postman 7+) | ✅ | ✅ |
| Export (Markdown / JSON / Postman, round-trip) | ✅ | ✅ |

### 🖥️ Server Triage (Pro-only)

The server-triage features are scoped to the daily triage workflow — inspecting, logging, and exec-ing into running targets. They are **not** full cluster lifecycle managers (no resource creation, no deployment management).

| Feature | Community | Pro |
|---|:---:|:---:|
| **SSH terminal** — password / private-key auth, multi-tab sessions | ❌ | 💎 |
| Terminal emulation — full ANSI / xterm-256color (16-color + 256-color + true color) | ❌ | 💎 |
| Paste (Cmd/Ctrl+V) · terminal text copy · host card management | ❌ | 💎 |
| Jump host / bastion (ProxyJump, chained `direct-tcpip` tunnels) | ❌ | 💎 |
| **SFTP** — list / mkdir / rename / upload / download, recursive `rm -rf`, transfer progress | ❌ | 💎 |
| **Zmodem** — in-terminal `rz` / `sz` file transfer | ❌ | 💎 |
| **SSH local port forwarding** (`-L`, one-click expose an internal service) | ❌ | 💎 |
| TOFU `known_hosts` verification (refuses host-key mismatch) | ❌ | 💎 |
| **Secure credential storage** — OS keychain + AES-256-GCM / Argon2id encrypted vault | ❌ | 💎 |
| **Docker** — list / start / stop / restart / remove containers, list & prune images | ❌ | 💎 |
| Docker — `docker logs -f` log streaming, `docker exec -it` (real PTY, multi-tab) | ❌ | 💎 |
| Docker — remote daemon via `DOCKER_HOST` or **SSH tunnel** (`docker system dial-stdio`) | ❌ | 💎 |
| **Kubernetes** — parse `~/.kube/config`, switch context, list **pods & namespaces** | ❌ | 💎 |
| K8s — `kubectl logs -f`, `kubectl exec -it` (PTY), `kubectl port-forward` | ❌ | 💎 |
| K8s — Direct (API server) or **SSH-tunnel** connection mode | ❌ | 💎 |

> **Not implemented (by design)**: SSH Agent auth & remote `-R` forwarding; Docker image build/pull/push, network/volume/compose/swarm, container inspect & resource stats; Kubernetes resources other than pods/namespaces (no service/deployment/configmap/…), no apply/create/delete, no helm/kustomize. The Docker/K8s panels focus on **log inspection and shell exec** — they do not replace a full Docker Desktop or cluster manager.

### 📝 Docs & Knowledge (split)

| Feature | Community | Pro |
|---|:---:|:---:|
| **Document sharing — local generation** (self-contained HTML from project/folder/request) | ✅ | ✅ |
| Document sharing — QR code + link + HTML export | ✅ | ✅ |
| Document sharing — access control (expiration + password, enforced by local server) | ✅ | ✅ |
| Document sharing — field-level display toggles (9 switches) | ✅ | ✅ |
| **Markdown notes** — block editor, notes tree (folders/pin/tags), live preview canvas | ❌ | 💎 |
| **Notes → PDF export** (built-in fonts, headings/code/lists/links) | ❌ | 💎 |
| **PDF viewer / editor** (Pdfium native, text/image/erase/page ops) — replaces a standalone PDF editor | ❌ | 💎 |
| **Standalone Markdown file editor** (multi-tab, Finder double-click / `verve file.md`) — replaces tools like MarkText | ❌ | 💎 |

### 🌍 Cloud Sharing (Pro-only)

| Feature | Community | Pro |
|---|:---:|:---:|
| **Self-hosted `verve-server`** (standalone binary, binds `0.0.0.0`, file-backed store) | ❌ | 💎 |
| **Cloud document sharing** — push a project to remote `verve-server`, get a public `/s/<id>` URL | ❌ | 💎 |
| `verve-server` `/admin` Web UI — upload / create / browse / delete shares, multi-tenant | ❌ | 💎 |

> ℹ️ Document sharing produces a **read-only snapshot** of the project at upload time. There is no real-time co-editing or live sync of an already-shared document. Note: Git-based workspace sync (cross-machine) is available in **both** editions — see the API & Testing table above.

### 📄 Licensing & Usage Rights

| | Community | Pro |
|---|---|---|
| **How to get** | Free download / build from source | Sponsor ¥99 early-bird / ¥199 regular |
| **License type** | AGPL-3.0 | Proprietary (Verve Pro License) |
| **Source visible** | ✅ Open | ❌ Closed |
| **Personal use** | ✅ | ✅ |
| **Commercial use** | ✅ | ✅ (after sponsorship) |
| **Modify / fork** | ✅ (derivative works & network use must also be AGPL-3.0) | ❌ |
| **Redistribute** | ✅ (under AGPL-3.0, source required) | ❌ |
| **Hosted / SaaS use** | ✅ (must open-source under AGPL-3.0) | ❌ (contact author) |
| **Reverse engineering** | ✅ | ❌ |
| **Updates** | Community-maintained / self-build | Official pre-built binaries + continuous updates |
| **Support** | Community issues | Priority support + early access to new features |

### Which one should I pick?

- **Community Edition** — you're an individual developer or student whose daily work is debugging APIs (including gRPC/WebSocket/SSE), capturing traffic, formatting JSON, managing hosts, running local mocks, sharing docs locally, and keeping your workspace in sync across machines; you want a fast, open, free native toolbox that can fully replace an individual API client.
- **Pro Edition** — you also need to SSH into servers to triage, tail Docker/K8s logs, run stress and automated tests, take Markdown notes / edit Markdown & PDF files, or share documents at a public cloud URL.

---

## ✨ Features

### 🆍 Community Edition Features

### 🔌 API Debugging
<div align="center">
  <img src="./assets/verve_demo/project.png" width="850" alt="Project Tree" />
  <br/>
  <em>Project tree (multi-level nesting)</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/api.png" width="850" alt="API Debugging" />
  <br/>
  <em>Request editor & response panel</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/drag_order.png" width="850" alt="Drag to Reorder" />
  <br/>
  <em>Drag &amp; drop to reorder the tree</em>
</div>

- Projects → folders → requests tree with multi-level nesting
- Full HTTP methods: `GET POST PUT DELETE PATCH HEAD OPTIONS`
- Request body: none / form-data / x-www-form-urlencoded / raw (JSON / XML / Text / HTML / JS)
- `{{variable}}` placeholder substitution with multi-scope priority (request > folder > environment > global)
- Pre-request & Tests scripts (JavaScript)
- Multi-protocol clients: **HTTP / gRPC (gRPC-Web) / TCP / SSE / WebSocket / Socket.IO**
- Multi-environment variable management (4-scope priority: system < global < environment < folder < request)
- Response panel: status / time / size / headers / body with JSON pretty-print
- API clone, JSON format & validation, history

### 🌐 HTTP Capture Proxy
<div align="center">
  <img src="./assets/verve_demo/http-captrue.png" width="850" alt="HTTP Capture Proxy" />
</div>

- Local HTTP forward proxy on `127.0.0.1:<port>`
- Records request + response pairs to an in-memory ring buffer for in-app inspection
- **Plaintext HTTP only — HTTPS / MITM decryption is not supported**

### 🎭 Local Mock Server
<div align="center">
  <img src="./assets/verve_demo/mock.png" width="850" alt="Mock Server" />
</div>

- Rule-based mock responses served on the unified share server (port 3097)
- Matching by method + path (Exact → Prefix → Regex) + query + headers, priority-ordered; template-variable substitution; one-click default-mock generation

### 📄 Document Sharing (Local)
<div align="center">
  <img src="./assets/verve_demo/doc.png" width="850" alt="Document Sharing" />
</div>

- Generate self-contained HTML docs from any project / folder / single request
- Modular layout with field display controls, color-coded parameter tables with required markers
- QR code sharing + link sharing + HTML export
- Strict access control: expiration + password protection (enforced by the local server)
- Field-level display toggles (9 switches: description / params / headers / body / auth / cookies / path / examples / mock)
- **Pushing to a remote `verve-server` for a public URL is a Pro-only feature** — see [Cloud Sharing](#-cloud-sharing-pro-only)

### 🌍 Git Cross-Machine Sync
<div align="center">
  <img src="./assets/verve_demo/git_sync_time.png" width="850" alt="Git Version Sync" />
</div>

- Multi-workspace support — **each workspace = a git branch** (default workspace → `main`, others → `verve/<id>`)
- Auto-commit + auto-sync every 30 minutes (configurable); commit on workspace switch
- Per-workspace `workspace.json` isolation; machine-local config (SSH hosts, layout, etc.) is git-ignored
- HTTPS remote + username / token (PAT) auth — token never touches disk, argv, or `.git/config` (uses a `GIT_ASKPASS` helper)
- Fast-forward-only sync (`--ff-only`); conflict detection with ours/theirs resolution
- Designed for **single-user cross-machine sync** (e.g. office + home), not concurrent team collaboration
- Works fully offline as a local version history even without a remote configured

### 🛠️ More Tools
<div align="center">
  <img src="./assets/verve_demo/hosts.png" width="420" alt="Hosts Manager" /> &nbsp;
  <img src="./assets/verve_demo/json-format.png" width="420" alt="JSON Format" />
  <br/>
  <em>Hosts manager &nbsp;·&nbsp; JSON format</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/theme.png" width="850" alt="Themes" />
  <br/>
  <em>22 built-in themes</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/settings.png" width="850" alt="Settings" />
  <br/>
  <em>Settings (i18n, theme, home view, auto-update)</em>
</div>

- **Hosts file manager** — read `/etc/hosts`, manage profiles
- i18n: Simplified Chinese (default) / English
- 22 built-in themes (Catppuccin, Gruvbox, Tokyo Night, Solarized, Everforest, Flexoki…)
- Import: Postman v2.1 / OpenAPI 3 / Swagger 2.0 / Postman 7+ (full format compatibility)
- Export: Markdown / JSON / Postman format (round-trip compatible)
- Configurable home view · Auto-update check · Cross-platform packaging (macOS / Linux / Windows)

---

### 💎 Pro Edition Features

### 🔐 SSH Terminal
<div align="center">
  <img src="./assets/verve_demo/ssh.png" width="850" alt="SSH Host Cards" />
  <br/>
  <em>Host card management (secure credential storage)</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/ssh-terminal.png" width="850" alt="SSH Terminal" />
  <br/>
  <em>Multi-tab terminal sessions</em>
</div>

- Connect to Linux servers with **password / private-key** authentication (key passphrase supported)
- Multi-tab terminal sessions (switch / close / persist)
- Built-in terminal emulator — full ANSI / xterm-256color (16-color + 256-color + true color), IME input
- Paste (Cmd/Ctrl+V) · terminal text copy
- **Jump host / bastion** (ProxyJump, chained `direct-tcpip` tunnels)
- TOFU `known_hosts` — first-seen records, re-connect compares, mismatch refused
- Host card management with **secure credential storage** (OS keychain + AES-256-GCM / Argon2id encrypted vault)

> *SSH Agent authentication and remote (`-R`) port forwarding are not implemented yet. This is an SSH client for triage, not a full iTerm2 replacement.*

### 📁 SFTP & File Transfer
<div align="center">
  <img src="./assets/verve_demo/ssh-file.png" width="850" alt="SFTP File Browser" />
</div>

- SFTP over SSH — list / mkdir / rename / upload / download, recursive directory removal, 64 KiB chunked streaming with progress

### 📁 Zmodem Transfer
- **`rz` / `sz`** file transfer directly inside the terminal — detected via `**\x18B` handshake, no extra setup

### 🔀 SSH Port Forwarding
- **Local port forwarding (`-L`)** — expose an internal service to the API tester with one click

### 🐳 Docker
<div align="center">
  <img src="./assets/verve_demo/docker.png" width="850" alt="Docker Management" />
  <br/>
  <em>Containers</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/docker-images.png" width="420" alt="Docker Images" /> &nbsp;
  <img src="./assets/verve_demo/docker-log.png" width="420" alt="Docker Logs" />
  <br/>
  <em>Images &nbsp;·&nbsp; Logs</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/docker-shell.png" width="850" alt="Docker Exec Shell" />
  <br/>
  <em>Exec shell (multi-tab)</em>
</div>

- Connect to a **local or remote** Docker daemon (remote via `DOCKER_HOST`, or over an **SSH tunnel** using `docker system dial-stdio`)
- **Containers** — list / start / stop / restart / remove
- **Images** — list / prune unused
- **Logs** — `docker logs -f --tail=N` streaming
- **Exec** — `docker exec -it` with a real PTY (auto-detects bash/sh/dash/ash), multi-tab

> *Focused on log inspection and shell exec. Image build/pull/push, networks, volumes, compose, swarm, and container resource stats are not supported — this is a lightweight triage tool, not a full Docker Desktop replacement.*

### ☸️ Kubernetes
<div align="center">
  <img src="./assets/verve_demo/k8s.png" width="850" alt="Kubernetes Management" />
  <br/>
  <em>Pods</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/k8s-log.png" width="420" alt="K8s Logs" /> &nbsp;
  <img src="./assets/verve_demo/k8s-shell.png" width="420" alt="K8s Exec Shell" />
  <br/>
  <em>Logs &nbsp;·&nbsp; Exec shell</em>
</div>

- Parse `~/.kube/config` + Verve-managed kubeconfig, switch context (Token / ClientCert / BasicAuth)
- **Pods & namespaces** — list (with containerStatuses: ready / restartCount), `-A` or scoped to a namespace
- **Logs** — `kubectl logs -f --tail=N [-c container]` streaming
- **Exec** — `kubectl exec -it` with a real PTY
- **Port-forward** — `kubectl port-forward pod/<pod> <local>:<remote>` (auto-allocates a local port)
- Two connection modes: **Direct** (API server) or **SSH tunnel** (via a Verve SSH bastion)

> *Focused on pod-level observation & debugging. Only pods and namespaces are supported — no service/deployment/configmap/ingress/node, no apply/create/delete, no helm/kustomize.*

### ⚡ Stress Testing
<div align="center">
  <img src="./assets/verve_demo/stress.png" width="850" alt="Stress Testing" />
</div>

- **Native load engine** (built on `reqwest`, not an external binary) — concurrency / duration / QPS cap / timeout, keepalive & redirect controls
- **Live chart** — a `StressSnapshot` every 200 ms (window RPS, p50/p90/p95/p99, latency histogram, status-code distribution)
- **Scenario mode** — multiple workers each loop a multi-step TestCase (reuses the autotest runner), per-iteration latency & pass/fail counts

### 🧪 Automated Testing
<div align="center">
  <img src="./assets/verve_demo/auto_test.png" width="850" alt="Automated Testing" />
</div>

- **Suite → Case → Step** structure (Postman-Collection-Runner-style), persisted in `workspace.json`
- **Step types**: `Request` (HTTP) · `If` (JS condition → then/else) · `Loop` (Repeat / ForEach over a JSON array / While) · `Wait` · `Script`
- **Assertions** via `apt.assert(condition, message?)` in JavaScript (boa engine); per-step pass/fail counters
- **Variable extractors** — JsonPath (dot-path) · Header · StatusCode · ResponseTime · Body; extracted vars are visible to later steps and across iterations
- **Script API** — `apt.variables` / `apt.environment` / `apt.setVariable` / `apt.assert` / `apt.echo` / `console.log`, with a `response` binding

### 🗒️ Markdown Notes & PDF Editor
<div align="center">
  <img src="./assets/verve_demo/note.png" width="850" alt="Markdown Notes" />
  <br/>
  <em>Notes with live preview</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/pdf.png" width="850" alt="PDF Viewer / Export" />
  <br/>
  <em>PDF viewer & export</em>
</div>

- Note tree (folders, pin, tags) with **live preview canvas**
- Export notes to **PDF** (built-in fonts, headings, code blocks, lists, links)
- **PDF viewer / editor** — Pdfium native, text / image / erase / page operations (replaces a standalone PDF editor)
- **Standalone Markdown file editor** — multi-tab, Finder double-click / `verve file.md` (replaces tools like MarkText)

### 📦 Self-hosted `verve-server` (Cloud Sharing)

The Pro Edition ships a standalone server binary for hosting shared documents in the cloud, with an admin Web UI for uploading projects and managing share links.

- Binds `0.0.0.0`, file-backed store under `<data>/cloud/`
- **Cloud document sharing** — push a project JSON to the remote server and get a public `/s/<id>` URL (Bearer-token authenticated upload)
- `/admin` Web UI — upload / create / browse / delete shares, multi-tenant isolation
- Shared documents are **read-only snapshots** of the project at upload time

See [`docs/verve-server.md`](./docs/verve-server.md) for the full deployment guide.

---

## 🚀 Getting Started

### 🆍 Community Edition

- **Pre-built binaries**: download the latest Community build for your platform (macOS / Linux / Windows) from [Releases](../../releases).
- **Build from source**: `git clone` this repo, then `cargo build --release`.
  - **Linux prerequisite**: the GPUI text stack pulls in `freetype-sys` + `fontconfig-sys`, which link the system FreeType/fontconfig via pkg-config, so install their development headers first:
    ```bash
    sudo apt-get install -y \
      build-essential pkg-config gcc g++ clang \
      libssl-dev libfontconfig1-dev libfreetype6-dev \
      libgtk-3-dev libwebkit2gtk-4.1-dev \
      libxkbcommon-x11-dev libx11-xcb-dev libwayland-dev \
      libzstd-dev libvulkan1 vulkan-validationlayers
    ```
    (Or run `./scripts/install-deps.sh`, which picks the right package manager.)
- On first launch, Verve auto-creates its data directory with a demo project so you can explore right away.

### 💎 Pro Edition

The Pro Edition (pre-compiled binaries + continuous updates + priority support) is obtained via sponsorship. See the [Pro Edition](#-pro-edition--sponsor-to-unlock) section below.

---

## 📋 Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd/Ctrl+Enter` | Send request |
| `Cmd/Ctrl+S` | Save workspace |
| `Cmd/Ctrl+N` | New request |
| `Cmd/Ctrl+V` | Paste (in terminal) |
| `Tab` | Auto-complete (in terminal) |

---

## 💎 Pro Edition — Sponsor to Unlock

> ⚠️ **Limited-Time Launch Pricing — ends soon!**

The **Pro Edition** unlocks everything beyond the Community Edition — SSH / Docker / Kubernetes pod triage, stress & automated testing, Markdown notes & file editor, PDF editor, and cloud document sharing via a self-hosted `verve-server`. It is obtained through a **sponsorship** model, and the entry price below is a **time-limited early-bird rate**.

### 🔥 Early-Bird Special — sponsor **¥99** (CNY)
The current **¥99** early-bird price is a limited-time launch offer. **It will return to the regular ¥199 once the promotion ends.** Lock in the lowest price now:

> **Sponsor ¥99+ today** to unlock the Pro Edition, including:
> - ✅ All Pro capabilities (SSH / Docker / K8s pod triage / stress testing / automated testing / Markdown notes & editor / PDF editor / cloud document sharing)
> - ✅ All official version updates (every future release, free)
> - ✅ Priority technical support
> - ✅ Early access to new features
> - ✅ Pre-compiled binaries for macOS / Linux / Windows — zero build hassle

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat_official.jpg" width="200" alt="WeChat Official Account (Sponsor)" />
      <br />
      <b>① Sponsor — scan to open the official-account article</b>
    </td>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="Author's Personal WeChat" />
      <br />
      <b>② Add my personal WeChat</b>
    </td>
  </tr>
</table>

> 📩 **How to get the Pro Edition:**
> 1. **Sponsor** — scan the left QR code to open the WeChat official-account article and complete your sponsorship (¥99+).
> 2. **Add me** — scan the right QR code to add my **personal WeChat** as a contact.
> 3. Send a screenshot of your sponsorship, and I'll send you the latest Pro Edition download link and activation instructions.

⏰ **Don't miss out — the price goes up after the launch window.**

---

## 💬 Feedback & Issues

Found a bug or have a feature request? Please [open an issue](../../issues/new).

---

## 📄 License

Verve is available in two editions under different licenses:

- **Community Edition** — released under the **AGPL-3.0** open-source license. Source code is public; you may use, modify, and redistribute it under the terms of AGPL-3.0. Note that AGPL-3.0 is a strong copyleft license: derivative works must be released under AGPL-3.0, and **network use (offering the software as a service over a network) also triggers the source-disclosure obligation.**
- **Pro Edition** — proprietary software under the **Verve Pro License**. Source code is **not open**. Without the author's written permission, the following are **prohibited**: reverse engineering / decompiling / disassembling; copying / modifying / redistributing the software or derivatives; using it for commercial resale or hosted services. The Pro Edition is obtained via sponsorship. For commercial licensing or team plans, contact the author via WeChat above.

The full text of the AGPL-3.0 is in the [`LICENSE`](LICENSE) file at the repository root.

---

## ⚠️ Disclaimer

Verve is an ongoing project under active development. **There is no guarantee that every feature described in this document is available, complete, or stable.** Some features may be unimplemented, experimental, or subject to change without notice. **The actual capabilities of the trial build shall prevail.** Please refer to the latest preview/Pro build for the real state of the software.
