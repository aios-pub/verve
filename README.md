[English](./README.md) | [简体中文](./README.zh-CN.md)

<div align="center">

# ⚡ Verve

**A native, GPU-accelerated developer workbench built with Rust + GPUI — unifying API debugging, traffic capture, an SSH/SFTP terminal (jump host / port forwarding / Zmodem), mock, hosts, and JSON tools in a single window.**

### A native developer workbench

Verve is built on **Rust + [GPUI](https://github.com/zed-industries/zed)** (the same GPU-accelerated UI framework that powers the Zed editor) — not Electron. The whole app is rendered on the GPU, starts in under a second, and stays well under 100 MB of RAM, so it stays responsive even while streaming container logs or running a load test.

`API & Testing` · `SSH Terminal` · `Mock & Capture` · `Docs & Git` · `Hosts` · `JSON`

<br/>

<a href="https://aios.pub/verve/">
  <img src="https://img.shields.io/badge/website-verve.app-bolt?style=flat-square&labelColor=0a0c10&color=d4e317" alt="Official website" />
</a>
<a href="https://github.com/aios-pub/verve/releases/latest">
  <img src="https://img.shields.io/github/v/release/aios-pub/verve?style=flat-square&label=latest%20release&labelColor=0a0c10&color=d4e317" alt="Latest release" />
</a>
<a href="https://github.com/aios-pub/verve">
  <img src="https://img.shields.io/badge/license-AGPL--3.0%20%2F%20Pioneer-d4e317?style=flat-square&labelColor=0a0c10" alt="License" />
</a>

<br/>

> 🚫 **Stop juggling Postman + an SSH client like Termius + Swagger + a notes app + a Markdown editor like MarkText + a JSON formatter + a hosts editor…**
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

[Features](#-features) · [Community vs Pioneer](#-community-vs-pioneer) · [Community Updates](#-community-edition--recent-iterations) · [Pioneer Updates](#-pioneer-edition--recent-iterations) · [Getting Started](#-getting-started) · [Pioneer Edition](#-pioneer-edition--sponsor-to-unlock)

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

Verve comes in two editions, with the same native app and the same daily API workflow — the Pioneer Edition adds container & cluster inspection (Docker / Kubernetes), testing, knowledge tooling, and cloud sharing on top.

### 🆍 Community Edition — Free & Open-Source

The **Community Edition** is free and released under the **AGPL-3.0** license. It covers everything an individual developer needs for daily work: full HTTP debugging (incl. multi-protocol clients), traffic capture, JSON formatting, hosts management, local mock server, local document sharing, **an SSH/SFTP terminal (jump host / port forwarding / Zmodem)**, **and Git-based cross-machine sync** — in a fast, lightweight native app. It is intentionally strong enough to stand on its own against any individual API client or SSH terminal tool.

### 💎 Pioneer Edition — Sponsor to Unlock

The **Pioneer Edition** is obtained via **sponsorship** (early-bird **¥99**, regular **¥199**). It layers on the advanced capabilities a single-purpose tool doesn't cover: container & cluster inspection (**Docker / Kubernetes pods**), **stress testing** and **automated test suites**, a **Markdown notes & file editor** (replaces tools like MarkText), and **cloud document sharing** (push to a self-hosted `verve-server` for a public URL).

→ Full feature comparison: [Community vs Pioneer](#-community-vs-pioneer)
→ How to get it: [Pioneer Edition](#-pioneer-edition--sponsor-to-unlock)

---

## 🆚 Community vs Pioneer

**Edition principle**: the Community Edition covers everything an individual developer needs for daily, standalone API and server-terminal work (strong enough to compete head-on with Postman or Termius). The Pioneer Edition layers on professional capabilities beyond individual API debugging — container & cluster inspection, testing, knowledge tooling, and cloud deployment.

> ✅ Included in both · ❌ Not in this edition · 💎 Pioneer-only feature

### Overview

| | 🆍 Community Edition | 💎 Pioneer Edition |
|---|---|---|
| **Positioning** | Individual developer's daily API + SSH terminal toolbox | Toolbox + containers/clusters + testing + knowledge + cloud |
| **License** | AGPL-3.0 (open-source) | Proprietary (Verve Pioneer License) |
| **Source code** | Open & auditable | Closed |
| **How to get** | Free download / build from source | Sponsor ¥99 early-bird / ¥199 regular |
| **Audience** | Individual devs, students, OSS community | Pro devs, ops engineers, small teams |
| **Platforms** | macOS · Linux · Windows | macOS · Linux · Windows |

### 🧪 API & Testing

| Feature | Community | Pioneer |
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

| Feature | Community | Pioneer |
|---|:---:|:---:|
| JSON formatter (collapsible tree) | ✅ | ✅ |
| Hosts manager (read `/etc/hosts`, profiles, env binding) | ✅ | ✅ |
| 22 built-in themes (Catppuccin / Gruvbox / Tokyo Night / Solarized …) | ✅ | ✅ |
| i18n (Simplified Chinese default / English) | ✅ | ✅ |
| Import (Postman v2.1 / OpenAPI 3 / Swagger 2.0 / Postman 7+) | ✅ | ✅ |
| Export (Markdown / JSON / Postman, round-trip) | ✅ | ✅ |

### 🖥️ Server Triage (SSH open-source · Docker/K8s Pioneer-only)

The server-triage features are scoped to the daily triage workflow — inspecting, logging, and exec-ing into running targets. They are **not** full cluster lifecycle managers (no resource creation, no deployment management). The **SSH / SFTP terminal capabilities are fully open-source** and ship free with the Community Edition.

| Feature | Community | Pioneer |
|---|:---:|:---:|
| **SSH terminal** — password / private-key auth, multi-tab sessions | ✅ | ✅ |
| Terminal emulation — full ANSI / xterm-256color (16-color + 256-color + true color) | ✅ | ✅ |
| Paste (Cmd/Ctrl+V) · terminal text copy · host card management | ✅ | ✅ |
| Jump host / bastion (ProxyJump, chained `direct-tcpip` tunnels) | ✅ | ✅ |
| **SFTP** — list / mkdir / rename / upload / download, recursive `rm -rf`, transfer progress | ✅ | ✅ |
| **Zmodem** — in-terminal `rz` / `sz` file transfer | ✅ | ✅ |
| **SSH local port forwarding** (`-L`, one-click expose an internal service) | ✅ | ✅ |
| TOFU `known_hosts` verification (refuses host-key mismatch) | ✅ | ✅ |
| **Secure credential storage** — OS keychain + AES-256-GCM / Argon2id encrypted vault | ✅ | ✅ |
| **Docker** — list / start / stop / restart / remove containers, list & prune images | ❌ | 💎 |
| Docker — `docker logs -f` log streaming, `docker exec -it` (real PTY, multi-tab) | ❌ | 💎 |
| Docker — remote daemon via `DOCKER_HOST` or **SSH tunnel** (`docker system dial-stdio`) | ❌ | 💎 |
| **Kubernetes** — parse `~/.kube/config`, switch context, list **pods & namespaces** | ❌ | 💎 |
| K8s — `kubectl logs -f`, `kubectl exec -it` (PTY), `kubectl port-forward` | ❌ | 💎 |
| K8s — Direct (API server) or **SSH-tunnel** connection mode | ❌ | 💎 |

> **Not implemented (by design)**: SSH Agent auth & remote `-R` forwarding; Docker image build/pull/push, network/volume/compose/swarm, container inspect & resource stats; Kubernetes resources other than pods/namespaces (no service/deployment/configmap/…), no apply/create/delete, no helm/kustomize. The Docker/K8s panels focus on **log inspection and shell exec** — they do not replace a full Docker Desktop or cluster manager.

### 📝 Docs & Knowledge (split)

| Feature | Community | Pioneer |
|---|:---:|:---:|
| **Document sharing — local generation** (self-contained HTML from project/folder/request) | ✅ | ✅ |
| Document sharing — QR code + link + HTML export | ✅ | ✅ |
| Document sharing — access control (expiration + password, enforced by local server) | ✅ | ✅ |
| Document sharing — field-level display toggles (9 switches) | ✅ | ✅ |
| **Markdown notes** — block editor, notes tree (folders/pin/tags), live preview canvas | ❌ | 💎 |
| **Notes → PDF export** (built-in fonts, headings/code/lists/links) | ❌ | 💎 |
| **Standalone Markdown file editor** (multi-tab, Finder double-click / `verve file.md`) — replaces tools like MarkText | ❌ | 💎 |

### 🌍 Cloud Sharing (Pioneer-only)

| Feature | Community | Pioneer |
|---|:---:|:---:|
| **Self-hosted `verve-server`** (standalone binary, binds `0.0.0.0`, file-backed store) | ❌ | 💎 |
| **Cloud document sharing** — push a project to remote `verve-server`, get a public `/s/<id>` URL | ❌ | 💎 |
| `verve-server` `/admin` Web UI — upload / create / browse / delete shares, multi-tenant | ❌ | 💎 |

> ℹ️ Document sharing produces a **read-only snapshot** of the project at upload time. There is no real-time co-editing or live sync of an already-shared document. Note: Git-based workspace sync (cross-machine) is available in **both** editions — see the API & Testing table above.

### 📄 Licensing & Usage Rights

| | Community | Pioneer |
|---|---|---|
| **How to get** | Free download / build from source | Sponsor ¥99 early-bird / ¥199 regular |
| **License type** | AGPL-3.0 | Proprietary (Verve Pioneer License) |
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

- **Community Edition** — you're an individual developer or student whose daily work is debugging APIs (including gRPC/WebSocket/SSE), working over SSH/SFTP (jump host, port forwarding, Zmodem), capturing traffic, formatting JSON, managing hosts, running local mocks, sharing docs locally, and keeping your workspace in sync across machines; you want a fast, open, free native toolbox that can fully replace an individual API client and an SSH terminal.
- **Pioneer Edition** — you also need to tail Docker/K8s logs, run stress and automated tests, take Markdown notes / edit Markdown files, or share documents at a public cloud URL.

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
- **Pushing to a remote `verve-server` for a public URL is a Pro-only feature** — see [Cloud Sharing](#-cloud-sharing-pioneer-only)

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

### 💎 Pioneer Edition Features

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

### 🗒️ Markdown Notes Editor
<div align="center">
  <img src="./assets/verve_demo/note.png" width="850" alt="Markdown Notes" />
  <br/>
  <em>Notes with live preview</em>
</div>

- Note tree (folders, pin, tags) with **live preview canvas**
- Export notes to **PDF** (built-in fonts, headings, code blocks, lists, links)
- **Standalone Markdown file editor** — multi-tab, Finder double-click / `verve file.md` (replaces tools like MarkText)

### 📦 Self-hosted `verve-server` (Cloud Sharing)

The Pioneer Edition ships a standalone server binary for hosting shared documents in the cloud, with an admin Web UI for uploading projects and managing share links.

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

### 💎 Pioneer Edition

The Pioneer Edition (pre-compiled binaries + continuous updates + priority support) is obtained via sponsorship. See the [Pioneer Edition](#-pioneer-edition--sponsor-to-unlock) section below.

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

## 🆕 Community Edition — Recent Iterations

The Community Edition keeps improving too. This is a transparent log of what has landed in the free, open-source build (the same code in this repo). Pioneer-only features (Docker / K8s / notes / PDF / testing) are listed separately below.

> Last updated: 2026-09

- **SSE protocol polish** — SSE requests default to POST with an editable method (LLM chat/completions style) and badge as "SSE" in the tree/tabs/folder list, so a GET-labeled SSE row no longer looks like a bug.
- **Linux title-bar parity** — a gpui CSD patch removes the WM's own title bar on deepin/kwin (client-side decorations now only require a compositor), matching Windows; plus SSH keepalive (15s/60s) with auto-reconnect on abnormal disconnects.
- **Windows local-terminal fix** — cmd/PowerShell went silent (zero output) in the local terminal on current Windows 11 builds; Verve now ships its own ConPTY backend and a 5s stall watchdog, and app logs persist to `~/.verve/logs/verve.log` (a Windows GUI process has no console).
- **MCP server (`verve mcp`)** 🤖 — expose the workspace to external AI clients (Claude Desktop, Cursor, Claude Code, …) over the Model Context Protocol: search/list/open requests and apply structured create/update/move/delete operations, with a backup-validate-rollback guard on `workspace.json` and live hot-reload (with notification) in the running GUI. See [docs/mcp.md](docs/mcp.md).
- **SSH / SFTP terminal is now open-source** 🎉 — the full SSH module joins the Community Edition: host cards, multi-tab terminals (full ANSI / xterm-256color, double-click word selection, scrollback), jump-host/bastion tunnels, SFTP file browsing (incl. recursive directory upload), in-terminal Zmodem `rz`/`sz`, one-click local port forwarding, local terminal mode, TOFU `known_hosts` verification, MFA / 2FA (`keyboard-interactive`) auth, and OS-keychain + AES-256-GCM / Argon2id encrypted credential storage — free for everyone, and now fixed for SSH disconnect/reconnect. SSH gets the ⌘2 rail slot.
- **JSON search** — ⌘/Ctrl+F in the JSON formatter opens a search bar that filters/matches JSON paths instantly (scoped key context so it never collides with the Markdown editor's find).
- **Save file-stream responses** — attachment / octet-stream responses keep their raw bytes (no UTF-8 corruption) and a「保存文件」button in the status bar opens a native save dialog; server-sent filenames are decoded (RFC 5987) and sanitized.
- **Multipart upload fixes** — your own `Content-Type` on a form-data part is honored (no duplicate header), names/filenames are escaped against header injection, and copy-as-curl emits proper `-F` parts; the actual-request panel shows parts structurally instead of binary mojibake.
- **Calmer file picking** — form-data file rows pick a file by clicking the value cell; switching the row type to File no longer auto-opens a dialog.
- **Query tab ⇄ URL bar live sync** — while the Query tab is open the URL bar previews `path?a=1&b=2` composed live from the params table (percent-encoded exactly like the send path); typing in either side updates the other, and a query string already present in the URL is folded into the table when the tab opens — nothing is dropped, and the stored URL stays query-free.
- **Traffic lights centered (macOS)** — the window traffic lights now sit exactly mid-height of the app's 40px self-drawn title bar.
- **Hardened SSE streaming** — the SSE response parser was rebuilt as an incremental chunk-safe parser (partial lines and `\r\n` splits across network chunks are handled correctly; comments and multi-line `data:` fields per spec).
- **Tab-close crash fix** — closing a request tab from the tab-overflow dropdown no longer risks a double-lease panic.

- **JSON editing upgrades** — delete any property (object key / array element / whole subtree) directly from the formatted view with order preserved; Format & Simplify actions built into the Raw-JSON request-body editor.
- **Smarter copy-as-curl** — bodies use the portable `-d` flag (works on old curl), and globals/dynamic variables are merged exactly like the real send path.
- **Response receive time** in the status bar, and the actual-request tab is now a copyable code editor.
- **Linux window fixes** — no more double close buttons (client-side decorations), settings windows centered as dialogs, and the first-run wizard no longer opens behind the main window.
- **Rail-switch shortcuts** — ⌘/Ctrl + 1..=5 jump to the daily-workflow views (API / Mock / JSON / Proxy / Hosts); rail tooltips show the shortcut.
- **Windows fit-and-finish** — windows are clamped to the display work area and centered within it (no more off-screen windows on high-DPI, no Dock/taskbar overlap), title-bar buttons are clickable (the drag region no longer swallows their clicks), and the main-thread stack reserve is raised to 16MB (fixes render-frame stack overflows).
- **Git sync fixes** — tree paths use `/` separators (Windows syncs used to fail validation), and auto-sync only runs when git auto-commit is explicitly enabled.
- **Accurate curl commands** — "Copy as cURL" (project-tree context menu) now reflects the request that was actually sent: variables substituted, query params appended, base URL joined, headers/body/auth included.
- **"实际请求" (actual request) tab** — the response panel shows exactly what went on the wire, as a readable text block plus an executable curl command.
- **Windows titlebar fixed (issue #2)** — the window chrome is now drawn by gpui-component's `TitleBar`: the bar is a drag region and Windows/Linux get min/max/close buttons. Previously the Windows build opened with no titlebar at all.
- **`{{variable}}` auto-completion** — typing `{{` in the URL, KV value, or base-URL inputs opens an LSP-style completion popover listing global + environment variables (plus dynamic ones like `$random`) with scope badges, filtering as you type.
- **Variable rename sync** — renaming an environment/global variable key rewrites every `{{oldKey}}` reference across requests, folders, params/headers/cookies, and base-URL overrides automatically.
- **Base-URL plain display** — the folder base-URL input shows the resolved plain URL (no `{{var}}` braces) while keeping the placeholder in storage, so variable linkage stays intact.
- **Global parameters / headers / cookies** — project-level globals are auto-applied to every request; a same-named per-request entry overrides the global one (case-insensitive, correct for HTTP headers).
- **Per-request base URL override (tri-state)** — each request can inherit the folder's base URL, explicitly disable any prefix, or set a custom one (supports `{{var}}` placeholders).
- **Postman-style dynamic variables** — `{{$random}}`, `{{$uuid}}`, `{{$timestamp}}`, and `{{$sparkid}}` (21-char time-sortable id) are expanded to a fresh value on every send; a user-defined variable of the same name still wins.
- **Clear response on send** — clicking Send immediately clears the previous response and shows a "请求中…" (requesting) state until the reply arrives, so stale content is never mistaken for the live result.
- **Smarter request search** — the project tree search now matches request **URLs/paths**, not just names.
- **"Move to" picker** — move a request or folder into another folder via a searchable destination picker (in addition to drag-and-drop).
- **Tab overflow dropdown** — when too many request tabs are open, a `»` dropdown lists the rest, and `cmd-w` / `ctrl-w` closes the active tab.
- **Environment manager** — the environments window now fills its container, has an overflow-scroll, and asks for confirmation before deleting a non-empty row.
- **KV table** — value cells expand on focus when content overflows, with configurable width / description flex / enabled-toggle.
- **sparkid entity IDs** — new records now use collision-resistant, time-sortable sparkids instead of UUID v4 (shorter codes, no near-simultaneous collisions).

---

## 🆕 Pioneer Edition — Recent Iterations

The Pioneer Edition ships new work almost every day. This section is a transparent log of recent Pro-side iterations, so the community can see exactly what's being built on top of the free Community Edition. (Community Edition users get the full daily API workflow for free; these land in the Pro build.)

> Last updated: 2026-09

### 🆘 App-wide

- **Built-in AI assistant** — a floating assistant panel (⌘J, drag / resize / minimize-to-edge) with multi-provider LLM support (OpenAI-compatible; streaming, tool calls, usage & price display, 429 auto-retry). Plan mode (writes need approval) vs full access, agent loop with progressive context loading and auto-compaction, Codex-style working plan that survives compaction, progressively loaded skills with autonomous switching, backup-validate-rollback on AI writes, and file paste support (images to vision; Word/PDF/Office converted via `anydoc`; online doc URLs fetched). API keys live in the encrypted local vault.
- **Markdown requirement docs** — Markdown nodes in the project tree become first-class requirement documents with doc ⇄ API link chips (both directions), AI-assisted extraction ("AI 智能提取 API 文档" import cards), and an embedded wysiwyg editor.
- **Searchable help center** — a built-in help center (⌘/ / F1) covering every view and keyboard shortcut, including the user's custom shortcut overrides (the same table that registers the real bindings); searchable, with Esc-aware view switching.

### 🗒️ Markdown Notes & PDF

- **Find matches inside table cells** — markdown search now matches content inside table cells, not just outside tables.
- **New-tab save fix** — opening a markdown file in a new editor tab no longer pops a "Save as" dialog on save; it writes back to the original file.
- **Full-text search** — notes are indexed with a tantivy inverted index for instant keyword search across the whole notebook.
- **Media storage (local + S3)** — images and videos embedded in notes are stored locally or pushed to S3, with a cleanup module for orphaned media. The media root folder can now be picked from a native directory browser.
- **Notes scale & loading** — optimized load algorithm, an original-file management mechanism, node move, tab collapse, live word count (with a fixed word-count edge case), and auto-expand of long text on focus.
- **Editor performance** — reduced per-frame CPU via word-count throttling (now cached per content version), render-cache reuse, block rows windowed once measured instead of full re-mounting, and row strides re-measured after viewport width changes; markdown/notes scrolling is now smoother.
- **Markdown paste fix** — fixed a bug where pasting formatted Markdown rendered ordered-list numbering incorrectly.
- **PDF export overhaul** — switched to a new PDF export engine.
- **Markdown rendering** — multiple rounds of rendering refinements.

### 🖥️ Server Triage

- **Docker panel** optimization.
- **Kubernetes panel** optimization.

### 🧪 API & Testing (shared foundation)

- **Global parameters** — project-level params / headers / cookies auto-applied to every request; a same-named per-request entry overrides the global one (case-insensitive). Shared by HTTP send, stress testing, and the automated-test runner.

---

## 💎 Pioneer Edition — Sponsor to Unlock

> ⚠️ **Limited-Time Launch Pricing — ends soon!**

The **Pioneer Edition** unlocks everything beyond the Community Edition — Docker / Kubernetes pod triage, stress & automated testing, Markdown notes & file editor, and cloud document sharing via a self-hosted `verve-server`. It is obtained through a **sponsorship** model, and the entry price below is a **time-limited early-bird rate**.

### 🔥 Early-Bird Special — sponsor **¥99** (CNY)
The current **¥99** early-bird price is a limited-time launch offer. **It will return to the regular ¥199 once the promotion ends.** Lock in the lowest price now:

> **Sponsor ¥99+ today** to unlock the Pioneer Edition, including:
> - ✅ All Pioneer capabilities (Docker / K8s pod triage / stress testing / automated testing / Markdown notes & editor / cloud document sharing)
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

> 📩 **How to get the Pioneer Edition:**
> 1. **Sponsor** — scan the left QR code to open the WeChat official-account article and complete your sponsorship (¥99+).
> 2. **Add me** — scan the right QR code to add my **personal WeChat** as a contact.
> 3. Send a screenshot of your sponsorship, and I'll send you the latest Pioneer Edition download link and activation instructions.

⏰ **Don't miss out — the price goes up after the launch window.**

---

## 💬 Feedback & Issues

Found a bug or have a feature request? Please [open an issue](../../issues/new).

---

## 📄 License

Verve is available in two editions under different licenses:

- **Community Edition** — released under the **AGPL-3.0** open-source license. Source code is public; you may use, modify, and redistribute it under the terms of AGPL-3.0. Note that AGPL-3.0 is a strong copyleft license: derivative works must be released under AGPL-3.0, and **network use (offering the software as a service over a network) also triggers the source-disclosure obligation.**
- **Pioneer Edition** — proprietary software under the **Verve Pioneer License**. Source code is **not open**. Without the author's written permission, the following are **prohibited**: reverse engineering / decompiling / disassembling; copying / modifying / redistributing the software or derivatives; using it for commercial resale or hosted services. The Pioneer Edition is obtained via sponsorship. For commercial licensing or team plans, contact the author via WeChat above.

The full text of the AGPL-3.0 is in the [`LICENSE`](LICENSE) file at the repository root.

---

## ⚠️ Disclaimer

Verve is an ongoing project under active development. **There is no guarantee that every feature described in this document is available, complete, or stable.** Some features may be unimplemented, experimental, or subject to change without notice. **The actual capabilities of the trial build shall prevail.** Please refer to the latest preview/Pro build for the real state of the software.
