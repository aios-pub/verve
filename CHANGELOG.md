# Changelog

All notable changes to Verve will be documented in this file.

## [0.8.7] - 2026-10-27

### ⬆️ Dependency stack migration (synced from upstream a5d854f)

- **GPUI now ships from crates.io** — the four zed-source crates (`gpui`,
  `gpui_platform`, `reqwest_client`, `http_client`) switch from the
  `amosgeek/zed` fork to the `gpui-pre-*` snapshot crates (`=0.3.9`), and
  `gpui-component` moves from the `lijingrs/gpui-component` fork to the
  official **0.7.3** release. All three fork patches are covered upstream /
  app-side: ListItem drag-and-drop works via native gpui `on_drag/on_drop/
  on_drag_move`, `TreeState::ensure_visible` became `reveal_item`, and 0.7's
  `TitleBar` skips drawn window controls when the WM draws its own. The
  `[patch."…zed"]` section is gone.
- **Editor component split** — gpui-component 0.7 separates single-line
  `Input`, multi-line `Textarea`, and code `Editor` states; the request body /
  response body / mock body editors, KV value cells and the JSON formatter
  migrate accordingly (`code_editor()` → `EditorState::language()`,
  `multi_line(true)` → `TextareaState`).

### 🔧 Enhancements (synced from upstream)

- **Request timeout raised 30 s → 3000 s** — the shared `DEFAULT_TIMEOUT_SECS`
  applies to HTTP/GraphQL/SSE/gRPC-Web send paths (long LLM SSE streams no
  longer time out client-side).
- **Popup dismissal fixed** — the folder "+" new-request picker and the
  "move to" picker now close on any click outside their card (`on_mouse_down_out`),
  and picking an import card closes the picker itself (`close_dialog` was a
  no-op for these panel-popovers).
- **SSH dialogs no longer lose focus to the terminal** — the host dialog and
  MFA prompt suppress the session terminal's auto-focus while open and restore
  it on save / dismiss.
- **rust-i18n 4.0 → 4.2.4, uuid +v7, serde_json `preserve_order` pinned
  explicitly** — matching gpui-component 0.7.3's workspace.

### 🧹 Removed

- **PDF editor leftovers** — upstream removed the PDF editor entirely (the
  notes → PDF export via Typst stays a Pioneer feature). The unused
  `file-pdf.svg` icon, `pdf.png` screenshot, and the `pdf.*` locale keys are
  dropped; READMEs no longer advertise a standalone PDF viewer/editor.

## [0.8.6] - 2026-09-25

### 🐛 Fixes (synced from upstream)

- **SSE requests shown by protocol, sent with the right method** — SSE now
  keeps the HTTP method selector visible (it runs over HTTP), and new SSE
  requests default to **POST** (LLM chat/completions style). Tree, tab, and
  folder-list badges label SSE requests as "SSE" (cyan) instead of their
  stored method, so "GET" on an SSE row no longer reads as a bug; other
  protocols keep the method badge. Legacy body-less SSE requests stay GET.

## [0.8.5] - 2026-09-21

### 🐛 Fixes

- **macOS build restored** — the CI installs `cargo-bundle` unpinned, and the
  freshly-released 0.12.0 tightened the bundle-config schema (no more
  `exception_domain` / `style` keys, both present-but-decorative since the
  initial commit). The two keys are removed and the CI now pins
  `cargo-bundle@0.12.0` so tool releases can't break the macOS job silently
  again. No app-level changes since 0.8.2.

## [0.8.2] - 2026-09-21

### 🔧 Enhancements (synced from upstream)

- **Linux: system title bar removed, matching Windows** — a gpui patch
  (`amosgeek/zed` fork, `verve-force-csd`: pinned rev + one commit) makes the
  X11 client-side-decoration probe only require a compositor. deepin's kwin
  and friends never list `_GTK_FRAME_EXTENTS` in `_NET_SUPPORTED`, which used
  to downgrade `WindowDecorations::Client` to Server and stack the WM's own
  "Verve" title bar on top of the app-drawn one. CSD works fine there (Motif
  undecorated hint + app-drawn shadow/resize via `_NET_WM_MOVERESIZE`);
  environments without a compositor still fall back to server decorations.
  All four zed-source crates (gpui, gpui_platform, reqwest_client,
  http_client) point at the same fork rev.

### 🖥️ SSH improvements (synced from upstream)

- **Keepalive + fast dead-connection detection** — a global keepalive every
  15s with a 60s inactivity timeout surfaces network drops / host reboots /
  VPN restarts promptly instead of hanging on TCP's multi-hour timeout.
- **Auto-reconnect on abnormal disconnect** — a channel that drops without an
  exit status (network loss / keepalive timeout) now shows
  "✕ 连接已断开，正在尝试重连…" and reconnects, instead of leaving a dead
  prompt that silently swallows keystrokes. A deliberate `exit` still closes
  the tab as before.

## [0.8.1] - 2026-09-20

### 🐛 Fixes (synced from upstream)

- **Windows local terminal silent on current Win11 builds** — portable-pty
  0.9 spawns ConPTY clients with `STARTF_USESTDHANDLES` +
  `INVALID_HANDLE_VALUE`, which leaves cmd/PowerShell alive but with zero
  output on Windows 11 build 26200+. The local-terminal mode now uses a
  purpose-built ConPTY backend (`ssh/win_conpty.rs`, modeled on the official
  MS sample): `CreatePseudoConsole` with flags=0, no `STARTF_USESTDHANDLES`,
  parent std handles cleared before spawn, and an explicit
  `ClosePseudoConsole` when the child exits. Unix keeps portable-pty.
- **Stall watchdog** — a 5s no-output watchdog now reports a diagnostic hint
  in the SSH panel instead of an apparently-dead silent terminal.
- **Logs persisted to `~/.verve/logs/verve.log`** — a Windows GUI process has
  no console, so stderr diagnostics were simply lost; env_logger output is
  now mirrored to a rotating log file (5 MB → verve.log.old).

## [0.8.0] - 2026-09-19

> 🎉 **The SSH / SFTP terminal is now open-source** — the full SSH module
> joins the Community Edition, and the Pro Edition is renamed to the
> **Pioneer Edition**. Version follows upstream 0.8.0.

### ✨ Added (SSH open-source)

- **SSH terminal** — host cards with password / private-key auth, multi-tab
  sessions, full ANSI / xterm-256color emulation (true color, IME), double-click
  word selection, scrollback buffer with clear-history, and a local terminal
  mode (spawn the user's shell in a PTY, no host needed).
- **Jump host / bastion** — ProxyJump with chained `direct-tcpip` tunnels.
- **SFTP file browser** — list / mkdir / rename / upload / download, recursive
  directory removal **and recursive directory upload**, 64 KiB chunked streaming
  with progress.
- **Zmodem** — in-terminal `rz` / `sz` transfer via `\x18B` handshake framing.
- **SSH local port forwarding** (`-L`) — one-click expose an internal service.
- **TOFU `known_hosts`** verification — host-key mismatch refuses to connect.
- **MFA / 2FA** — `keyboard-interactive` auth for TOTP / OTP / hardware keys,
  including "password + OTP" pairing and skip-MFA on duplicate session.
- **Secure credential storage** — OS keychain + AES-256-GCM / Argon2id
  encrypted vault; `ssh_hosts.json`, `ssh_vault.bin`, and `device.key` are
  excluded from git sync.
- **Disconnect/reconnect fix** — terminals recover cleanly after the connection
  drops.
- SSH joins the rail at ⌘/Ctrl+**2** (one-time rail reorder; Hosts remains
  reachable from the rail).

### 🔧 Enhancements (synced from upstream)

- **MCP server (`verve mcp`)** 🤖 — expose the Verve workspace to external AI
  clients (Claude Desktop, Cursor, Claude Code, VS Code Copilot, …) over the
  Model Context Protocol: the client launches `verve mcp` as a stdio
  subprocess and can search/list/open requests, inspect the project tree, and
  apply structured change operations (create/update/move/delete requests &
  folders, with the same id/variable semantics as the GUI). Every write goes
  through a backup → validate → rollback guard on `workspace.json`, and the
  running GUI hot-reloads external changes (with a notification) via a file
  watcher. Docs: [docs/mcp.md](docs/mcp.md). Ships with the shared change-plan
  executor (`ai::ops`); the LLM client and agent loop remain upstream-only.
- **JSON search** — ⌘/Ctrl+F in the JSON formatter opens a scoped search bar
  (key context `"JsonPanel"`, so it never overrides the Markdown editor's find);
  the panel root is focused on view switch so the shortcut works immediately.

## [0.7.2] - 2026-09-05

### 🔧 Enhancements (synced from upstream)

- **Save file-stream responses to disk** — responses with
  `Content-Disposition: attachment` (incl. RFC 5987 `filename*`, percent-decoded)
  or `application/octet-stream` are detected as file streams: the raw bytes are
  kept in a session temp file (immune to UTF-8 lossy corruption) and the
  response status bar gains a「保存文件」button that opens a native save dialog
  and copies asynchronously. Server-sent names are sanitized against path
  traversal.
- **File-picker interaction** — for form-data file rows the picker opens by
  clicking the value cell (placeholder shown until a file is chosen); switching
  the type to File no longer pops a dialog automatically.

### 🐛 Fixes (synced from upstream)

- **Multipart uploads** — a user-set `Content-Type` header on a form-data part
  is now honored (its boundary reused, or replaced in place) instead of
  appending a second one that broke server-side parsing; `Content-Disposition`
  name/filename values are quoted/newline-escaped against header injection.
- **Copy-as-curl for form-data** — the actual-request curl emits `-F` parts
  instead of dumping binary bodies as `-d`, and the actual-request panel shows
  parts structurally rather than mojibake.
- **Wire-level form-data tests** — end-to-end tests verify the exact bytes on
  the wire for multipart bodies.

## [0.7.1] - 2026-08-29

### 🧹 Maintenance

- Repository moved to `aios-pub/verve`: updated the Cargo homepage, README
  links/badges, docs-site links (now `https://aios.pub/verve/` — the old
  Pages URL is dead), and the updater's release-check repo slug.

## [0.7.0] - 2026-08-29

> Version jumps 0.5.11 → 0.7.0 to stay in lockstep with the upstream project.

### 🔧 Enhancements (synced from upstream)

- **Query tab ⇄ URL bar live sync** — while the Query tab is active the URL
  input previews `path?a=1&b=2` composed live from the params table,
  percent-encoded exactly like the send path (`url::form_urlencoded`). Edits in
  either side propagate to the other (idempotent compose keeps the URL-bar
  cursor stable), and a query string already present in the stored URL is
  folded into the params table on tab activation, so nothing is dropped. The
  persisted URL, curl generation, and the send path all use the query-free
  base value — the preview is display-only.
- **macOS traffic lights centered** — main-window traffic lights move from
  y=16 to y=14, the exact vertical center of the 40px self-drawn title bar.
- **Hardened SSE streaming** — `http::sse` gains an incremental `SseParser`
  (chunk-split-safe partial lines, `\r\n` handling, comment lines, multi-line
  `data:` joining per the SSE spec) with 4 new round-trip tests; the response
  panel's live stream now routes through it.
- **Structured app logs** — env_logger now prints `timestamp [LEVEL] [target]
  message` with local time.

### 🐛 Fixes (synced from upstream)

- **Tab-close double-lease panic** — closing a request tab from the
  tab-overflow dropdown leased the panel entity twice (inside `cx.listener`);
  it now uses the already-leased `this` directly.

## [0.5.11] - 2026-08-20

### 🔧 Enhancements (synced from upstream)

- **JSON format panel: delete any property** — a per-row delete button removes
  an object key, array element, or whole subtree from the formatted view; the
  trimmed document syncs back to the input with key order preserved
  (`serde_json` preserve_order) and expansion state repaired.
- **Request body editor: Format / Simplify** — built-in toolbar actions in the
  Raw-JSON code editor: pretty-print in place, or recursively keep only the
  first item of every array (same rule as the JSON panel's compact mode).
- **Portable curl `-d`** — copy-as-curl emits request bodies with the
  long-supported `-d` flag instead of `--data-raw` (curl 7.43+), so old curl
  versions work.
- **Accurate curl globals** — curl generation (preview tab + project-tree
  action) merges project-global params/headers/cookies before variable
  substitution, mirroring the real send path.
- **Actual-request tab** — renders as a selectable/copyable code editor with a
  one-click copy button.
- **Response receive time** — the status bar shows when each response arrived
  (`YYYY-MM-DD HH:mm`), stamped per protocol.

### 🐛 Fixes (synced from upstream)

- **Linux double close buttons** — client-side decorations remove the WM title
  bar that stacked a second close button over the drawn controls; settings
  windows open as dialogs so the WM centers them over the main window. Also
  bumps gpui-component to the `verve-fork-v2` rev carrying the TitleBar
  server-side-decoration fix.
- **First-run welcome z-order** — the main window now opens before the welcome
  dialog, which is explicitly activated afterwards, so the wizard is no longer
  buried behind the main window on Windows/Linux.

## [0.5.10] - 2026-08-19

### 🐛 Fixes (synced from upstream)

- **Title-bar buttons clickable on Windows** — the `TitleBar` drag region made
  plain buttons (rail/sidebar toggle, share, sync, import, update check, and
  the JSON toolbar) unclickable on Windows: the OS window-drag loop consumed
  their mouse-up. Each control is now wrapped in an occluding hitbox so the
  click completes; dragging empty parts of the bar still works.
- **Center within the work area** — windows now center on the visible work
  area (`centered_at`), no longer slightly overlapping the Dock / taskbar.

## [0.5.9] - 2026-08-19

### 🔧 Enhancements & Fixes (synced from upstream)

- **Rail-switch shortcuts** — ⌘/Ctrl + 1..=5 now jump to the fixed daily-workflow
  views (API / Mock / JSON / Proxy / Hosts). A one-time rail reorder puts the
  shortcut views at the front of the saved order (runs once; drag-to-reorder
  afterwards is preserved), and rail tooltips show each view's shortcut.
- **Window clamped to work area** — windows are centered with their logical
  size clamped to the display's visible bounds first. Fixes the Windows
  high-DPI case (e.g. 150% scale) where the fixed 1400×900 main window opened
  partly off-screen.
- **Windows stack reserve raised to 16MB** — the MSVC default 1MB main-thread
  stack overflows (0xc000041d) on deep GPUI render frames; the linker flag now
  matches macOS/Linux headroom (`.cargo/config.toml`).
- **Git sync fixes** — tree paths use `/` separators (Windows `\` made every
  sync commit fail gix validation), and auto-sync now requires the user to
  have actually enabled git auto-commit in settings instead of syncing the
  auto-created repo on first run.

## [0.5.8] - 2026-08-17

### 🔧 Enhancements (synced from upstream)

- **Accurate curl commands** — "Copy as cURL" in the project-tree context menu
  now generates the command from the request that was actually sent (variables
  substituted, query params appended, base URL joined, headers/body/auth
  included), via a dedicated `http::curl` module. Previously it approximated
  from the stored definition, producing wrong URLs when placeholders or the
  folder base URL were in play.
- **"实际请求" (actual request) tab** — the response panel now shows exactly
  what went on the wire after `prepare()`: the resolved request as a text
  block plus its executable curl form, both captured on the `Response`.

## [0.5.7] - 2026-08-16

### 🐛 Fixes (synced from upstream)

- **Windows titlebar missing (issue #2)** — the main window is created with
  `appears_transparent`, which hides the native titlebar; nothing redrew the
  window chrome, so the Windows build had no min/max/close buttons and the bar
  couldn't drag the window. The title bar is now wrapped in gpui-component's
  `TitleBar`, which owns the drag region and draws min/max/close controls on
  Windows/Linux (macOS keeps native traffic lights, appearance unchanged).
- The settings window got the same treatment (drag region + drawn controls;
  macOS keeps the self-drawn close button) and both auxiliary windows are now
  minimizable.

## [0.5.6] - 2026-08-15

### 🔧 Enhancements (synced from upstream)

- **`{{variable}}` auto-completion** — typing `{{` in URL / KV value / base-URL
  inputs now opens an LSP-style completion popover listing global + environment
  variables (and dynamic variables like `$random`) with scope badges; filters as
  you type.
- **Variable rename sync** — renaming an environment/global variable key now
  rewrites every `{{oldKey}}` placeholder reference across requests, folders,
  params/headers/cookies, and base-URL overrides, so existing references stay
  valid.
- **Base-URL plain display** — the folder base-URL input shows the resolved
  plain URL (no `{{var}}` braces) while persisting the original placeholder, so
  variable linkage stays intact.

### 📝 Docs

- READMEs: refreshed both iteration logs — Community (variable completion /
  rename sync) and Pro (SSH local terminal mode, notes/markdown per-frame perf,
  word-count fix).

## [0.5.5] - 2026-08-14

### 🔧 Enhancements (synced from upstream)

- **Folder base-URL dropdown scroll** — the "select from environment variables"
  popover for a folder's base URL now scrolls when the list is long, and shows
  the bare env-var name instead of the `{{name}}` placeholder.

### 📝 Docs

- READMEs: refreshed the Pro Edition iterations log — editor per-frame CPU
  optimization (word-count throttle, render-cache reuse, window bookkeeping),
  smoother markdown/notes scrolling, a Markdown paste ordered-list fix, and a
  native directory picker for the media root.

## [0.5.3] - 2026-08-13

### 🔧 Enhancements (synced from upstream)

- **Postman-style dynamic variables** — `{{$random}}`, `{{$uuid}}`,
  `{{$timestamp}}`, and the new `{{$sparkid}}` (21-char Base58, time-sortable)
  are expanded to a fresh value on every send; a user-defined variable of the
  same name still wins.
- **Clear response on send** — clicking Send immediately clears the previous
  response and shows a "请求中…" (requesting) state in the realtime panel until
  the reply arrives, so stale content is never mistaken for the live result.
- **Smarter request search** — the project tree search now matches request
  URLs/paths, not just names.
- **"Move to" picker** — move a request or folder into another folder via a
  searchable destination picker (in addition to drag-and-drop).
- **Tab overflow dropdown** — when too many request tabs are open, a `»`
  dropdown lists the overflow; `cmd-w` / `ctrl-w` closes the active tab.
- **Environment manager** — the window now fills its container, has an
  overflow-scroll, and confirms before deleting a non-empty row.
- **KV table** — value cells expand on focus when content overflows, with
  configurable value width / description flex / enabled-toggle.
- **Per-request base URL precedence fix** — an explicit override
  (disable/custom) now always takes precedence over host-based URL splitting on
  reload.
- **sparkid entity IDs** — new records use collision-resistant, time-sortable
  sparkids instead of UUID v4 (shorter short-codes, no near-simultaneous
  collisions). Affects project/folder/request/share/hosts-profile ids and the
  Postman export ids.

### 📦 Other

- Added `sparkid = "2.2.1"` dependency.
- READMEs: added a "Community Edition — Recent Iterations" section and refreshed
  the Pro Edition iterations log (notes full-text search, media storage,
  SSH duplicate-session skip-MFA, etc.).
- Pro-only modules (SSH / Docker / Kubernetes / stress testing / WYSIWYG notes
  / PDF / relocate / autotest / cloud sharing / verve-server / notes-index /
  media) remain exclusive to the Pro Edition.

## [0.5.0] - 2026-08-08

### 🔧 Enhancements (synced from upstream)

- **Per-request base URL override (tri-state)** — each request can now choose to
  inherit the folder's base URL, explicitly disable any prefix, or set a custom
  one (supports `{{var}}` placeholders). The dropdown shows the effective URL and
  the "不使用前置 URL" state is persisted per request.
- **Global parameters / headers / cookies** — project-level global
  params/headers/cookies are now auto-applied to every request, with a same-named
  per-request entry overriding the global one (matching the global-manager UI
  copy "接口级同名头覆盖"). Matching is case-insensitive for HTTP headers.
- **Method-colored Send button & refined tab bar** — the Send button and method
  chip now use a custom variant tinted by the HTTP method color, and the request
  tabs use an Apifox/Postman-style active underline.
- i18n: added the `export_failed` message (en + zh-CN).

### 📦 Other

- Non-code resources (locales) synced from upstream.
- Pro-only modules (SSH / Docker / Kubernetes / stress testing / WYSIWYG notes /
  PDF / relocate / autotest / cloud sharing / `verve-server`) remain exclusive to
  the Pro Edition and are not part of the Community Edition.

## [0.3.0] - 2026-08-02

### 🎉 Major Release — Rust + GPUI Rewrite

This is a complete rewrite of Verve in Rust using the GPUI framework (the same GPU-accelerated UI framework that powers the Zed editor). This release brings dramatic performance improvements and a native, lightweight experience.

### ⚡ Performance & Architecture

- **Rust + GPUI architecture** — Complete rewrite from the ground up in Rust
- **GPU-accelerated rendering** — Native GPU rendering via GPUI, no Electron/Chromium
- **< 1 second startup** — Instant launch compared to 3-5 seconds for Electron apps
- **< 100 MB RAM** — 5× lighter than Electron-based tools (typically 500 MB+)
- **~60 FPS rendering** — Fluid, responsive UI even under heavy load

### 🔧 Core Infrastructure

#### New Module Structure
- **Git operations** (`src/git/`) — Complete git integration for cross-machine sync
  - `ops.rs` — Git operations wrapper around gitoxide
  - `state.rs` — Git state management for workspaces
- **HTTP clients** (`src/http/`) — Multi-protocol client implementations
  - `client.rs` — HTTP client with reqwest
  - `grpc.rs` — gRPC-Web client support
  - `sse.rs` — Server-Sent Events client
  - `tcp.rs` — TCP client support
  - `ws.rs` — WebSocket client
  - `variable.rs` — Variable substitution system
- **Share system** (`src/share/`) — Document sharing infrastructure
  - `html.rs` — HTML documentation generator
  - `server.rs` — Local HTTP share server
  - `models.rs` — Share data models
  - `persist.rs` — Share persistence layer
  - `qrcode.rs` — QR code generation

#### UI Architecture Refactor
- **New app structure** (`src/ui/app/`)
  - `mod.rs` — Main app module
  - `construction.rs` — UI construction helpers
  - `rail.rs` — Navigation rail component
  - `titlebar.rs` — Native titlebar implementation
  - `workspaces.rs` — Workspace management UI
  - `actions.rs` — UI action handlers
  - `widgets.rs` — Reusable UI widgets
  - `share.rs` — Share dialog components

- **New specialized panels**
  - `bootstrap_dialog.rs` — First-run setup dialog
  - `console_panel.rs` — Console output panel
  - `env_panel.rs` — Environment variable editor
  - `environments_view.rs` — Multi-environment management UI
  - `hosts_panel.rs` — Hosts file management UI
  - `json_panel.rs` — JSON formatter panel
  - `kv_manager_view.rs` — Key-value pair manager
  - `kv_table.rs` — Key-value table component
  - `method_colors.rs` — HTTP method color coding
  - `mock_console_panel.rs` — Mock server console
  - `project_manage_panel.rs` — Project management UI
  - `project_tree_panel.rs` — Project tree with drag-reorder
  - `proxy_panel.rs` — HTTP capture proxy UI
  - `request_panel/` — Request editor (split into multiple files)
  - `response_panel.rs` — Response viewer
  - `settings_window.rs` — Settings dialog
  - `share_dialog.rs` — Share generation dialog
  - `share_panel.rs` — Share history panel
  - `theme.rs` — Theme system implementation

#### State Management
- **New state architecture** (`src/state/`)
  - `app_state.rs` — Global application state
  - `models.rs` — Data models for projects, requests, environments
  - `sample_data.rs` — Demo project data
  - `persistence.rs` — Enhanced workspace persistence with Git sync

### 🌍 Internationalization

- **Locale system** (`locales/`)
  - `en.yml` — English translations (500+ keys)
  - `zh-CN.yml` — Simplified Chinese translations (500+ keys)
- **rust-i18n integration** — Compile-time i18n with type-safe translation API

### 🎨 Themes

- **22 built-in themes** (`themes/`)
  - Catppuccin (Mocha/Latte/Frappe/Macchiato)
  - Gruvbox (Dark/Light)
  - Tokyo Night
  - Solarized (Dark/Light)
  - Everforest
  - Flexoki
  - Adventure
  - Alduin
  - Asciiinema
  - Aurora
  - Ayu
  - Fahrenheit
  - Harper
  - Hybrid
  - Jellybeans
  - Kibble
  - macOS Classic
  - Matrix
  - Mellifluous
  - Molokai
  - Spaceduck
  - Twilight

### 🛠️ Build & Tooling

- **Enhanced build script** (`scripts/build.sh`)
  - Cross-platform build automation (macOS/Linux/Windows)
  - Native dependency detection and auto-install for Linux
  - Support for `.app`, `.deb`, `.AppImage`, and `.msi` packages
  - CI-friendly `--no-auto-install` flag
- **Dependency install script** (`scripts/install-deps.sh`)
  - Automatic detection of package manager (apt/yum/dnf/pacman)
  - Installs FreeType, fontconfig, and other native dependencies
- **Icon generation script** (`scripts/gen_icon.py`)
  - Automated icon asset generation from SVG sources

### 📦 New Dependencies

Major dependency updates and additions:
- `gpui` + `gpui_platform` — GPU-accelerated UI framework
- `gpui-component` — UI component library (custom fork with drag-and-drop)
- `gix` (gitoxide) — Pure-Rust git operations
- `tokio-tungstenite` — WebSocket client
- `boa_engine` — JavaScript engine for scripts
- `rust-embed` — Asset embedding
- `rust-i18n` — Internationalization
- `qrcode` — QR code generation
- `semver` — Semantic versioning

### 🔧 Configuration Updates

- **Cargo.toml** — Updated to v0.3.0 with new dependencies
- **Cargo.lock** — Regenerated for Rust rewrite
- **Desktop entries** — Updated `.desktop` file for Linux
- **macOS bundle metadata** — Updated Info.plist values

### 📚 Documentation

- **Updated README.md** — Comprehensive feature documentation
  - Rust + GPUI architecture explanation
  - Community vs Pro edition comparison
  - Detailed feature lists for both editions
  - Performance benchmarks
  - Installation instructions
- **Updated README.zh-CN.md** — Chinese version of documentation
- **New demo assets** — Screenshots and demo videos
- **Removed old assets** — Cleaned up legacy video files

### 🗑️ Removed Files

Legacy files from pre-Rust version:
- `src/bin/verve_server.rs` — Old server binary (relocated)
- `src/git.rs`, `src/http.rs`, `src/share.rs`, `src/ssh.rs` — Consolidated into modules
- `src/ui/app.rs`, `src/ui/request_panel.rs` — Restructured UI architecture
- Old demo videos — Replaced with new thumbnail-based links

### ✅ Features Retained

All major features from v0.2.0 have been reimplemented in the new architecture:
- ✅ HTTP API debugging (all methods)
- ✅ Multi-protocol clients (HTTP/gRPC/TCP/SSE/WebSocket)
- ✅ Project tree with multi-level nesting
- ✅ Environment variable management
- ✅ Request/response history
- ✅ Mock server (Exact/Prefix/Regex matching)
- ✅ HTTP capture proxy
- ✅ Document sharing (local HTML generation)
- ✅ Hosts file manager
- ✅ JSON formatter
- ✅ Import/Export (Postman/OpenAPI/Swagger)
- ✅ Git-based cross-machine sync

### 💎 Pro Edition Features

Pro-only features (not in Community Edition):
- ✅ SSH terminal with SFTP
- ✅ Docker container management
- ✅ Kubernetes pod inspection
- ✅ Stress testing
- ✅ Automated test suites
- ✅ Markdown notes editor
- ✅ PDF viewer/editor
- ✅ Cloud document sharing (verve-server)

### 🔒 Security

- **Secure credential storage** — OS keychain integration
- **AES-256-GCM encryption** — For SSH credentials
- **Argon2id key derivation** — Password hashing
- **TOFU known_hosts verification** — SSH host key validation
- **Token-based Git auth** — PAT never touches disk

### 🐛 Bug Fixes

- Fixed drag-and-drop reordering in project tree
- Fixed theme switching consistency
- Fixed environment variable priority resolution
- Fixed JSON pretty-printing edge cases
- Fixed share server port binding conflicts

### ⚠️ Breaking Changes

- **Minimum OS requirements updated**
  - macOS: 11.0+
  - Linux: glibc 2.31+
- **Configuration format** — Workspace JSON format updated (migration handled automatically)
- **GPUI version pinning** — Locked to specific Zed commit for stability

### 📝 Migration Notes

Users upgrading from v0.2.x:
1. Workspaces will be automatically migrated to the new format
2. Theme selections will be reset (22 new themes available)
3. Re-authenticate Git remotes if using cross-machine sync
4. Some keyboard shortcuts have changed — see Settings > Shortcuts

---

## [0.2.0] - Previous Release

(Release notes archived in git history)

---

**For older releases, please refer to the git commit history.**
