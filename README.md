<div align="center">

# Verve

**The Offline-First API Co-Development Platform**

HTTP Debugging · SSH Terminal · Docker & Kubernetes · Stress Testing · Mock & Proxy · Document Sharing · Git Sync

A native desktop workbench built with Rust + GPUI

[Features](#-features) · [What's New](#-whats-new) · [Getting Started](#-getting-started) · [Official Release](#-official-release--limited-time-offer)

</div>

---

## ✨ Features

### 🔌 API Debugging
- Projects → folders → requests tree with multi-level nesting
- Full HTTP methods: `GET POST PUT DELETE PATCH HEAD OPTIONS`
- Request body: none / form-data / x-www-form-urlencoded / raw (JSON / XML / Text / HTML / JS)
- `{{variable}}` placeholder substitution with multi-scope priority (request > folder > environment > global)
- Pre-request & Tests scripts (JavaScript via `boa_engine`)
- Response panel: status / time / size / headers / body with JSON pretty-print
- API clone, JSON format & validation, history hidden

### 🔐 SSH Terminal
- Connect to Linux servers with password / private-key / agent authentication
- Multi-tab terminal sessions (switch / close / persist), multi-terminal support
- Built-in terminal emulator (alacritty_terminal + GPUI rendering)
- ANSI color support (16 colors + 256-color + truecolor)
- Tab auto-completion · Paste support (Cmd/Ctrl+V) · Terminal text copy
- Jump host / bastion support (ProxyJump via `channel_open_direct_tcpip`)
- Auto-reconnect with exponential backoff
- Host card management with secure credential storage (OS keychain + encrypted vault)

### 📁 SFTP & File Transfer
- SFTP file browser over SSH (upload / download with native file dialogs)
- **Zmodem `rz` / `sz`** transfer directly inside the terminal — no extra setup

### 🔀 SSH Port Forwarding
- Local port forwarding (`-L`) via `channel_open_direct_tcpip`
- Expose an internal service to the API tester with one click

### 🐳 Docker Management
- Connect to local or remote Docker daemons (Docker host cards)
- List / start / stop / restart / remove containers, list & prune images
- Stream container logs, exec into containers (real PTY, multi-tab shells)
- Shell auto-completion · Container filtering · Docker SSH tunnel for remote daemons

### ☸️ Kubernetes Management
- Parse `~/.kube/config` + Verve-managed kubeconfig, switch context
- List pods, stream logs, exec into pods/containers, port-forward
- Two connection modes: **Direct** (API server) or **SSH tunnel** (via bastion)
- kubectl shell auto-completion · Multi-tab · Shell backup scripts

### ⚡ Stress Testing
- Built-in load generator (powered by `oha`) — concurrency + duration, **live chart** streaming
- Scenario stress: multi-step test cases run in a loop, latency percentile tracking
- Labeled result metrics · i18n

### 🧪 Automated Testing
- Test suites / cases / steps with sequential runner
- Reuses the scripting & HTTP infrastructure for assertions

### 🎭 Mock Server
- Rule-based mock responses served on the unified share server (port 3097)
- Matching by method + path (Exact → Prefix → Regex) + query + headers, priority-ordered

### 🌐 HTTP Capture Proxy
- Local HTTP forward proxy on `127.0.0.1:<port>`
- Records request + response pairs to a ring buffer for in-app inspection

### 📄 Document Sharing
- Generate self-contained HTML docs from any project / folder / single request
- Modular layout with field display controls, color-coded parameter tables with required markers
- QR code sharing + link sharing + HTML export
- Strict access control: expiration + password protection (server-enforced)
- Local HTTP server (`verve-server`) + **push to a remote `verve-server`** for a public URL
- Admin Web UI for upload / create / browse / delete shares

### 🗒️ Markdown Notes
- Note tree (folders, pin, tags) with **live preview canvas**
- Export notes to **PDF** (built-in fonts, headings, code blocks, lists, links)
- PDF viewer with zoom · PDF editor

### 🌍 Git Version Control
- Multi-workspace support (each workspace = a git branch)
- Auto-commit + auto-sync every 30 minutes
- Per-workspace `workspace.json` isolation
- Username / password / token authentication · Branch management UI

### 🛠️ More Tools
- **Hosts file manager** — read `/etc/hosts`, manage profiles
- **Database client launcher** — auto-detect & launch TablePlus, DBeaver, DataGrip, pgAdmin, RedisInsight, Sequel Ace… (macOS / Linux / Windows)
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
- ⚡ **Stress testing** — `oha`-powered load generator with real-time charts + scenario mode
- 🧪 **Automated testing** — suites / cases / steps runner with assertions
- 🎭 **Mock server** — priority-ordered rule matching (Exact / Prefix / Regex)
- 🌐 **HTTP capture proxy** — inspect local HTTP traffic
- 📁 **SFTP browser** + 🔀 **SSH port forwarding** + **Zmodem `rz`/`sz`** file transfer
- 🗒️ **Markdown notes** with live preview + **PDF export / viewer / editor**
- 🛠️ **Hosts file manager** + **Database client launcher**
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

### Build & Run
```bash
# Requires Rust 1.95.0+
cargo run
```

First launch auto-creates `~/.verve/` data directory with a demo project.

### Package for Distribution
```bash
# macOS
./scripts/build.sh macos    # → .app + .dmg

# Linux
./scripts/build.sh linux    # → .deb + .AppImage

# Windows
./scripts/build.sh windows  # → .msi

# Current platform
./scripts/build.sh all
```

---

## 🏗️ Tech Stack

| Component | Technology |
|-----------|------------|
| UI Framework | [GPUI](https://github.com/zed-industries/zed) (same as Zed editor) |
| SSH Client | [russh](https://github.com/Eugeny/russh) 0.60 (pure Rust) |
| Terminal Emulator | [alacritty_terminal](https://crates.io/crates/alacritty_terminal) 0.24 |
| Git Engine | [gix](https://crates.io/crates/gix) 0.69 (gitoxide) |
| Script Engine | [boa_engine](https://crates.io/crates/boa_engine) (JavaScript) |
| Load Testing | [oha](https://crates.io/crates/oha) 1.15 |
| PDF Render | [pdfium-render](https://crates.io/crates/pdfium-render) + [printpdf](https://crates.io/crates/printpdf) |
| Encryption | AES-256-GCM + Argon2 |

---

## 📦 verve-server (Cloud Deployment)

Verve ships a standalone server binary for hosting documents & mock APIs in the cloud:

```bash
verve-server --port 8080
# Admin UI: http://your-server:8080/admin
```

Docker deployment:
```bash
docker build -t verve-server .
docker run -d -p 3097:3097 -v verve-data:/data verve-server
```

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

Verve is open source and fully usable. The **official release** (pre-compiled binaries + continuous updates + priority support) is offered through a **donation-based model**, and the entry price below is a **time-limited early-bird rate**.

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
      <img src="./assets/wechat.jpg" width="200" alt="WeChat Donate" />
      <br />
      <b>WeChat Donate</b>
    </td>
  </tr>
</table>

⏰ **Don't miss out — the price goes up after the launch window.** After donating, please note your GitHub username or email in WeChat, and I'll send you the official release download link.

---

## 💬 Feedback & Issues

Found a bug or have a feature request? Please [open an issue](../../issues/new).

---

## License

Source code is licensed under **MIT**, free to use, modify, and distribute.

The official release (pre-compiled binaries + continuous updates + technical support) is provided through the donation model.
