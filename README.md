<div align="center">

# Verve

**Offline-First API Co-Development Platform**

HTTP Debugging · SSH Terminal · Document Sharing · Git Version Control

A native desktop app built with Rust + GPUI

</div>

---

## ✨ Features

### 🔌 API Management
- Projects → folders → requests tree with multi-level nesting
- Full HTTP debugging: `GET POST PUT DELETE PATCH HEAD OPTIONS`
- Request body: none / form-data / x-www-form-urlencoded / raw (JSON/XML/Text/HTML/JS)
- `{{variable}}` placeholder substitution with multi-scope priority (request > folder > environment > global)
- Pre-request & Tests scripts (JavaScript via `boa_engine`)
- Response panel: status / time / size / headers / body with JSON pretty-print

### 🔐 SSH Terminal
- Connect to Linux servers with password / private-key / agent authentication
- Multi-tab terminal sessions (switch / close / persist)
- Built-in terminal emulator (alacritty_terminal + GPUI rendering)
- ANSI color support (16 colors + 256-color + truecolor)
- Tab auto-completion · Paste support (Cmd/Ctrl+V)
- Jump host / bastion support (ProxyJump via `channel_open_direct_tcpip`)
- Auto-reconnect with exponential backoff
- Host card management with secure credential storage (OS keychain + encrypted vault)

### 📄 Document Sharing
- Generate self-contained HTML documentation from any project / folder / single request
- Modular layout with field display controls
- Color-coded parameter tables with required markers
- QR code sharing + link sharing + HTML export
- Strict access control: expiration + password protection (server-enforced)
- Local HTTP server (`verve-server`) with cloud deployment support + admin Web UI

### 🌍 Git Version Control
- Multi-workspace support (each workspace = a git branch)
- Auto-commit + auto-sync every 30 minutes
- Per-workspace `workspace.json` isolation
- Username / password / token authentication · Branch management UI

### 🎨 More
- i18n: Simplified Chinese (default) / English
- 22 built-in themes (Catppuccin, Gruvbox, Tokyo Night, etc.)
- Import: Postman v2.1 / OpenAPI 3 / Apipost 7+ (full format compatibility)
- Export: Markdown / JSON / Apipost format (round-trip compatible)
- Configurable home view · Cross-platform packaging (macOS / Linux / Windows)

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
./scripts/build.sh linux    # → .deb

# Windows
./scripts/build.sh windows  # → .msi
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
| Encryption | AES-256-GCM + Argon2 |

---

## 📦 verve-server (Cloud Deployment)

Verve includes a standalone server binary for cloud document hosting:

```bash
verve-server --port 8080
# Admin UI: http://your-server:8080/admin
```

Docker deployment:
```bash
docker build -t verve-server .
docker run -d -p 3097:3097 -v verve-data:/data verve-server
```

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

## 💰 Official Release

Verve is currently an open-source preview, actively under development.

The official release is offered through a **donation-based model**:

> **Donate ¥99+ (CNY)** to get the official release license, including:
> - All official version updates
> - Priority technical support
> - Early access to new features

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="WeChat Donate" />
      <br />
      <b>WeChat Donate</b>
    </td>
  </tr>
</table>

After donating, please note your GitHub username or email in WeChat, and I'll send you the official release download link.

---

## 📝 Changelog

### v0.1.0 (Current Preview)
- ✅ HTTP debugging (all methods / body types / variable substitution / scripts)
- ✅ SSH terminal (multi-tab / bastion / auto-reconnect / Tab completion)
- ✅ Document sharing (HTML generation / strict access control / verve-server cloud)
- ✅ Git version control (multi-workspace / auto-commit / branch management)
- ✅ i18n (Chinese / English)
- ✅ Import/Export (Postman / OpenAPI / Apipost round-trip compatible)
- ✅ Cross-platform packaging (macOS / Linux / Windows)

### Planned
- ⬜ SFTP file browser
- ⬜ Port forwarding management
- ⬜ Automated testing
- ⬜ Terminal color customization

---

## 💬 Feedback & Issues

Found a bug or have a feature request? Please [open an issue](../../issues/new).

---

## License

Source code is licensed under **MIT**, free to use, modify, and distribute.

The official release (pre-compiled binaries + continuous updates + technical support) is provided through the donation model.
