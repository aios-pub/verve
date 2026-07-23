<div align="center">

# ⚡ Verve

### The One-Stop Developer Workbench

**Debug APIs, investigate issues, tail logs, exec into containers — one window for the high-frequency tasks you repeat every day.** Built-in HTTP debugging, SSH terminal, Docker / K8s read-only inspection & troubleshooting, stress testing, mock, and traffic capture — plus notes, hosts, JSON formatting, and other dev utilities, with document sharing and Git sync. Everything a developer reaches for daily, finally under one roof.

`API & Testing` · `Terminal & Triage` · `Mock & Capture` · `Docs & Git` · `Notes` · `Hosts` · `JSON` · `PDF` · `DB Tools`

<br/>

> 🚫 **Stop juggling Postman + iTerm2 + Docker Desktop + Swagger + a notes app + a JSON formatter + a hosts editor…**
> Verve is the single native desktop app that absorbs your daily high-frequency dev workflow — built in Rust for developers who refuse to settle for sluggish Electron bloat.

<br/>

<table>
  <tr>
    <td align="center"><b>🦀 Rust-Native</b><br/><sub>No Electron, no Chromium</sub></td>
    <td align="center"><b>⚡ &lt;1s Startup</b><br/><sub>Instant launch</sub></td>
    <td align="center"><b>💾 &lt;100MB RAM</b><br/><sub>5× lighter</sub></td>
    <td align="center"><b>🔒 Offline-First</b><br/><sub>Your data stays local</sub></td>
    <td align="center"><b>🖥️ Cross-Platform</b><br/><sub>macOS · Linux · Windows</sub></td>
  </tr>
</table>

<br/>

[Features](#-features) · [What's New](#-whats-new) · [Getting Started](#-getting-started) · [Official Release](#-official-release--limited-time-offer)

</div>

---

## 🦀 Built with Rust — Why It Matters

Verve is built in **Rust** from the ground up. Here's the real-world difference against Electron-based tools:

| | Verve (Rust) | Electron-based tools |
|---|---|---|
| ⚡ **Startup** | < 1 second | 3–5 seconds |
| 💾 **Memory** | < 100 MB | 500 MB+ |
| 🎨 **Rendering** | Native GPU, ~60fps | Chromium software compositing |
| 🛡️ **Safety** | Memory-safe, zero-cost abstraction | GC pauses, V8 overhead |

What this means for you: instant responses when debugging APIs, fluid terminals under heavy load, and a battery-friendly footprint that stays light all day.

---

## ✨ Features

Verve covers the full daily workflow of a developer — not just API debugging. Four pillars, one app:

| Pillar | What's inside |
|--------|---------------|
| 🧪 **API & Testing** | HTTP debugging · stress testing · automated test suites · mock server · HTTP capture proxy |
| 🖥️ **Terminal & Triage** | SSH terminal · SFTP file transfer · port forwarding · Docker container inspection & log triage · Kubernetes pod inspection & log triage |
| 🤝 **Collaboration & Docs** | Document sharing (HTML/QR/link) · Git version sync · cloud `verve-server` |
| 🛠️ **Dev Utilities** | Markdown notes (+ PDF export) · hosts manager · JSON formatter · PDF viewer/editor · database client launcher · 22 themes · i18n |

Dive into each below 👇

---

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
- Response panel: status / time / size / headers / body with JSON pretty-print
- API clone, JSON format & validation, history hidden

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

- Connect to Linux servers with password / private-key / agent authentication
- Multi-tab terminal sessions (switch / close / persist), multi-terminal support
- Built-in terminal emulator with full ANSI color support
- Tab auto-completion · Paste support (Cmd/Ctrl+V) · Terminal text copy
- Jump host / bastion support (ProxyJump)
- Auto-reconnect with exponential backoff
- Host card management with secure credential storage (OS keychain + encrypted vault)

### 📁 SFTP & File Transfer
<div align="center">
  <img src="./assets/verve_demo/ssh-file.png" width="850" alt="SFTP File Browser" />
</div>

- SFTP file browser over SSH (upload / download with native file dialogs)
- **Zmodem `rz` / `sz`** transfer directly inside the terminal — no extra setup

### 🔀 SSH Port Forwarding
- Local port forwarding (`-L`)
- Expose an internal service to the API tester with one click

### 🐳 Docker Management
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

- Connect to local or remote Docker daemons (Docker host cards)
- List / start / stop / restart / remove containers, list & prune images
- Stream container logs, exec into containers (real PTY, multi-tab shells)
- Shell auto-completion · Container filtering · Docker SSH tunnel for remote daemons

### ☸️ Kubernetes Management
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

- Parse `~/.kube/config` + Verve-managed kubeconfig, switch context
- List pods, stream logs, exec into pods/containers, port-forward
- Two connection modes: **Direct** (API server) or **SSH tunnel** (via bastion)
- kubectl shell auto-completion · Multi-tab · Shell backup scripts

### ⚡ Stress Testing
<div align="center">
  <img src="./assets/verve_demo/stress.png" width="850" alt="Stress Testing" />
</div>

- Built-in load generator — concurrency + duration, **live chart** streaming
- Scenario stress: multi-step test cases run in a loop, latency percentile tracking
- Labeled result metrics · i18n

### 🧪 Automated Testing
<div align="center">
  <img src="./assets/verve_demo/auto_test.png" width="850" alt="Automated Testing" />
</div>

- Test suites / cases / steps with sequential runner
- Reuses the scripting & HTTP infrastructure for assertions

### 🎭 Mock Server
<div align="center">
  <img src="./assets/verve_demo/mock.png" width="850" alt="Mock Server" />
</div>

- Rule-based mock responses served on the unified share server (port 3097)
- Matching by method + path (Exact → Prefix → Regex) + query + headers, priority-ordered

### 🌐 HTTP Capture Proxy
<div align="center">
  <img src="./assets/verve_demo/http-captrue.png" width="850" alt="HTTP Capture Proxy" />
</div>

- Local HTTP forward proxy on `127.0.0.1:<port>`
- Records request + response pairs to a ring buffer for in-app inspection

### 📄 Document Sharing
<div align="center">
  <img src="./assets/verve_demo/doc.png" width="850" alt="Document Sharing" />
</div>

- Generate self-contained HTML docs from any project / folder / single request
- Modular layout with field display controls, color-coded parameter tables with required markers
- QR code sharing + link sharing + HTML export
- Strict access control: expiration + password protection (server-enforced)
- Local HTTP server (`verve-server`) + **push to a remote `verve-server`** for a public URL
- Admin Web UI for upload / create / browse / delete shares

### 🗒️ Markdown Notes
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
- PDF viewer with zoom · PDF editor

### 🌍 Git Version Control
<div align="center">
  <img src="./assets/verve_demo/git_sync_time.png" width="850" alt="Git Version Sync" />
</div>

- Multi-workspace support (each workspace = a git branch)
- Auto-commit + auto-sync every 30 minutes
- Per-workspace `workspace.json` isolation
- Username / password / token authentication · Branch management UI

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
- Import: Postman v2.1 / OpenAPI 3 / Postman 7+ (full format compatibility)
- Export: Markdown / JSON / Postman format (round-trip compatible)
- Configurable home view · Auto-update check · Cross-platform packaging (macOS / Linux / Windows)

---

## 🆕 What's New

### v0.2.0 (Current)
- 🐳 **Docker management** — containers, images, logs, exec, multi-tab shells, remote daemon tunneling
- ☸️ **Kubernetes management** — kubeconfig contexts, pods, logs, exec, port-forward, SSH-tunneled API server
- ⚡ **Stress testing** — load generator with real-time charts + scenario mode
- 🧪 **Automated testing** — suites / cases / steps runner with assertions
- 🎭 **Mock server** — priority-ordered rule matching (Exact / Prefix / Regex)
- 🌐 **HTTP capture proxy** — inspect local HTTP traffic
- 📁 **SFTP browser** + 🔀 **SSH port forwarding** + **Zmodem `rz`/`sz`** file transfer
- 🗒️ **Markdown notes** with live preview + **PDF export / viewer / editor**
- 🛠️ **Hosts file manager**
- 🔗 **Remote `verve-server` push** for public document URLs
- 🎨 Docker / K8s panel polish, drag-resizable dock, custom menu, new share icon

### v0.1.0
- ✅ HTTP debugging (all methods / body types / variable substitution / scripts)
- ✅ SSH terminal (multi-tab / bastion / auto-reconnect / Tab completion)
- ✅ Document sharing (HTML generation / strict access control / `verve-server` cloud)
- ✅ Git version sync (multi-workspace / auto-commit / branch management)
- ✅ i18n (Chinese / English) · 22 themes · Import/Export · Cross-platform packaging

### Planned
- ⬜ Remote (`-R`) port forwarding
- ⬜ HTTPS MITM capture (trusted CA)
- ⬜ Terminal color customization

---

## 🚀 Getting Started

Verve is closed-source proprietary software distributed as ready-to-use builds — no build toolchain needed.

1. **Download** the latest preview build for your platform (macOS / Linux / Windows).
2. On first launch, Verve auto-creates its data directory with a demo project so you can explore right away.
3. Want the full **official release** with continuous updates and priority support? See the [Official Release](#-official-release--limited-time-offer) section below.

---

## 📦 verve-server (Cloud Deployment)

Verve ships a standalone server for hosting shared documents & mock APIs in the cloud, with an admin Web UI for uploading projects and managing share links.

See [`docs/verve-server.md`](./docs/verve-server.md) for the full deployment guide.

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

## 💰 Official Release — Limited-Time Offer

> ⚠️ **Limited-Time Launch Pricing — ends soon!**

Verve is **closed-source proprietary software** (not open source); an official preview build is available to try. The **official release** (pre-compiled binaries + continuous updates + priority support) is offered through a **donation-based model**, and the entry price below is a **time-limited early-bird rate**.

### 🔥 Early-Bird Special — only **¥99** (CNY)
The current **¥99** early-bird price is a limited-time launch offer. **It will return to the regular ¥199 once the promotion ends.** Lock in the lowest price now:

> **Donate ¥99+ today** to get the official release license, including:
> - ✅ All official version updates (every future release, free)
> - ✅ Priority technical support
> - ✅ Early access to new features
> - ✅ Pre-compiled binaries for macOS / Linux / Windows — zero build hassle

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat_official.jpg" width="200" alt="WeChat Official Account (Donate)" />
      <br />
      <b>① Donate — scan to open the official-account article</b>
    </td>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="Author's Personal WeChat" />
      <br />
      <b>② Add my personal WeChat</b>
    </td>
  </tr>
</table>

> 📩 **How to get the official release:**
> 1. **Donate** — scan the left QR code to open the WeChat official-account article and complete your donation (¥99+).
> 2. **Add me** — scan the right QR code to add my **personal WeChat** as a contact.
> 3. Send a screenshot of your donation, and I'll send you the latest official-release download link and activation instructions.

⏰ **Don't miss out — the price goes up after the launch window.**

---

## 💬 Feedback & Issues

Found a bug or have a feature request? Please [open an issue](../../issues/new).

---

## 📄 License

Verve is **closed-source proprietary software**. The source code is **not open** and is **not distributed**. All Rights Reserved.

Without the author's written permission, the following are **prohibited**:
- ❌ Reverse engineering, decompiling, or disassembling the software
- ❌ Copying, modifying, or redistributing the software or derivatives
- ❌ Using the software for commercial resale or hosted services

**Permitted**: downloading the officially released preview / official build for personal and team day-to-day development use.

The official release (pre-compiled binaries + continuous updates + technical support) is licensed through the donation model. For commercial licensing or team collaboration plans, contact the author via WeChat above.
