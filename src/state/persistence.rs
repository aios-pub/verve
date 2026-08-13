//! Local JSON-file persistence for the workspace.
//!
//! The full workspace is stored as a single JSON document at
//! `<data_dir>/verve/workspace.json`, where `data_dir` is the platform
//! convention (`~/.verve` on all platforms for simplicity).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context as _, Result};

use super::models::WorkspaceData;

/// Process-wide data-directory override (used by `verve_server --data-dir`).
///
/// Set once at startup; takes precedence over the `$HOME/.verve` default. Tests
/// should prefer [`set_thread_data_dir`] (per-thread) so parallel tests stay
/// isolated without touching this or the `HOME` env var.
static DATA_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

thread_local! {
    /// Per-thread data-directory override, used by tests to point each test at
    /// a unique temp dir without mutating the process-global `HOME` env var.
    /// The rust test runner spawns one thread per test, so thread-local scoping
    /// keeps parallel tests fully isolated.
    static THREAD_DATA_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Install a process-wide data-directory override (startup only). Subsequent
/// calls are ignored — the first value wins. Used by `verve_server --data-dir`.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR_OVERRIDE.set(dir);
}

/// Install a per-thread data-directory override (tests). The returned guard
/// clears the override on drop, restoring the prior value so nested scopes work.
#[doc(hidden)]
pub fn set_thread_data_dir(dir: PathBuf) -> ThreadDataDirGuard {
    let prev = THREAD_DATA_DIR.with(|c| c.borrow_mut().replace(dir));
    ThreadDataDirGuard(prev)
}

/// RAII guard that restores the prior per-thread data dir on drop.
#[doc(hidden)]
pub struct ThreadDataDirGuard(Option<PathBuf>);

impl Drop for ThreadDataDirGuard {
    fn drop(&mut self) {
        THREAD_DATA_DIR.with(|c| {
            *c.borrow_mut() = self.0.take();
        });
    }
}

/// Return the workspace directory, creating it if needed.
///
/// Resolution order: per-thread override (tests) → process override
/// (`set_data_dir`, e.g. `--data-dir`) → `$HOME/.verve`.
pub fn data_dir() -> Result<PathBuf> {
    let dir = resolved_data_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("create data dir {:?}", dir))?;
    Ok(dir)
}

fn resolved_data_dir() -> Result<PathBuf> {
    if let Some(dir) = THREAD_DATA_DIR.with(|c| c.borrow().clone()) {
        return Ok(dir);
    }
    if let Some(dir) = DATA_DIR_OVERRIDE.get() {
        return Ok(dir.clone());
    }
    home_data_dir()
}

fn home_data_dir() -> Result<PathBuf> {
    // Use ~/.verve everywhere. It is simple, predictable, and matches the
    // PRD's "completely offline mode" goal.
    let home = dirs_home()?;
    Ok(home.join(".verve"))
}

fn dirs_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }
    // Fall back to a cwd-based path; persistence still works in-tree.
    Ok(PathBuf::from("."))
}

fn workspace_path(dir: &Path) -> PathBuf {
    dir.join("workspace.json")
}

/// Load the workspace from disk, or return the demo workspace on first run /
/// when the file is missing or unreadable.
pub fn load_or_default() -> WorkspaceData {
    let dir = match data_dir() {
        Ok(d) => d,
        Err(e) => {
            log::error!("persistence data_dir error: {e:?}");
            return super::sample_data::demo_workspace();
        }
    };
    let path = workspace_path(&dir);
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<WorkspaceData>(&contents) {
            Ok(data) => data,
            Err(e) => {
                log::warn!("workspace.json corrupt, starting fresh: {e:?}");
                super::sample_data::demo_workspace()
            }
        },
        Err(_) => {
            // First run: seed demo data and persist it.
            let demo = super::sample_data::demo_workspace();
            let _ = save(&demo);
            demo
        }
    }
}

/// Persist the workspace to disk, creating the directory if needed.
pub fn save(data: &WorkspaceData) -> Result<()> {
    let dir = data_dir()?;
    let path = workspace_path(&dir);
    let json = serde_json::to_string_pretty(data)?;
    // Write to a temp file then rename, for atomicity.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).with_context(|| format!("write workspace {:?}", tmp))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename workspace {:?}", path))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Workspaces index (workspaces.json) — the cross-branch registry of all
// workspaces. This file is NOT tracked by git (it's shared across branches).
// ---------------------------------------------------------------------------

fn workspaces_index_path(dir: &Path) -> PathBuf {
    dir.join("workspaces.json")
}

/// Load the workspaces index. On first run (file missing) or corruption,
/// create the built-in `default` workspace, persist it, and return it.
pub fn load_workspaces_index() -> super::models::WorkspacesIndex {
    let dir = match data_dir() {
        Ok(d) => d,
        Err(e) => {
            log::error!("workspaces index data_dir error: {e:?}");
            return super::models::WorkspacesIndex::with_default();
        }
    };
    let path = workspaces_index_path(&dir);
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<super::models::WorkspacesIndex>(&contents) {
            Ok(mut idx) => {
                // Guarantee the default workspace always exists (migration safety).
                if !idx.workspaces.iter().any(|w| w.is_default) {
                    idx.workspaces
                        .insert(0, super::models::WorkspaceMeta::default_workspace());
                    let _ = save_workspaces_index(&idx);
                }
                idx
            }
            Err(e) => {
                log::warn!("workspaces.json corrupt, recreating with default: {e:?}");
                let idx = super::models::WorkspacesIndex::with_default();
                let _ = save_workspaces_index(&idx);
                idx
            }
        },
        Err(_) => {
            // First run: create the default workspace index.
            log::info!("首次运行：创建默认 default workspace 索引");
            let idx = super::models::WorkspacesIndex::with_default();
            let _ = save_workspaces_index(&idx);
            idx
        }
    }
}

/// Persist the workspaces index to `workspaces.json`.
pub fn save_workspaces_index(idx: &super::models::WorkspacesIndex) -> Result<()> {
    let dir = data_dir()?;
    let path = workspaces_index_path(&dir);
    let json = serde_json::to_string_pretty(idx)?;
    fs::write(&path, json).with_context(|| format!("write workspaces index {:?}", path))?;
    Ok(())
}

/// Set the active workspace id and persist the index.
pub fn set_active_workspace(id: &str) {
    let mut idx = load_workspaces_index();
    idx.active = Some(id.to_string());
    let _ = save_workspaces_index(&idx);
}

/// The `.gitignore` content excluding cross-branch / non-data / machine-local files from git.
/// layout.json is now tracked (shared settings); layout.local.json contains machine-specific panel sizes.
const GITIGNORE: &str = "workspaces.json\nlayout.local.json\nshares.json\nssh_hosts.json\ndocker_hosts.json\ndocker_active.json\nkube_config.yaml\nkube_clusters.json\nssh_vault.bin\ndevice.key\n.verve-askpass.sh\nexports/\nhosts.staging\nnotes.index/\nmedia/\n.bootstrap_done\n";

/// Ensure `~/.verve/.gitignore` exists with the right exclusions. Called once
/// when the git repo is initialised so cross-branch files don't leak into
/// per-workspace commits.
pub fn ensure_gitignore() {
    let Ok(dir) = data_dir() else { return };
    let path = dir.join(".gitignore");
    // When media `git_sync` is off, `media_storage.toml` (which holds S3
    // credentials) must stay machine-local → add it to .gitignore.
    let media_config_local_only =
        !crate::media::config::load_media_config().global.git_sync;
    // Re-write if missing or if it's missing any of the known exclusions so
    // newly added entries (e.g. layout.local.json) are picked up on existing installs.
    let needs_write = match fs::read_to_string(&path) {
        Ok(existing) => {
            let missing_base = !existing.contains("workspaces.json")
                || !existing.contains("layout.local.json")
                || !existing.contains("shares.json")
                || !existing.contains("ssh_hosts.json")
                || !existing.contains("docker_hosts.json")
                || !existing.contains("kube_config.yaml")
                || !existing.contains("notes.index/");
            // The media-config ignore line must match the current git_sync state.
            let media_mismatch =
                media_config_local_only != existing.contains("media_storage.toml");
            missing_base || media_mismatch || existing.contains("layout.json\n")
        }
        Err(_) => true,
    };
    if needs_write {
        let mut content = GITIGNORE.to_string();
        if media_config_local_only {
            content.push_str("media_storage.toml\n");
        }
        let _ = fs::write(&path, content);
    }
}

// ---------------------------------------------------------------------------
// First-run bootstrap marker.
// ---------------------------------------------------------------------------

/// Check if this is the first run (bootstrap not yet completed).
pub fn is_first_run() -> bool {
    let Ok(dir) = data_dir() else { return false };
    !dir.join(".bootstrap_done").exists()
}

/// Mark first-run bootstrap as completed.
pub fn mark_bootstrap_done() {
    let Ok(dir) = data_dir() else { return };
    let _ = fs::write(dir.join(".bootstrap_done"), "");
}

// ---------------------------------------------------------------------------
// Hosts profiles persistence.
// ---------------------------------------------------------------------------

/// Load hosts profiles from disk.
pub fn load_hosts_profiles() -> crate::hosts_profiles::HostsProfileStore {
    crate::hosts_profiles::load()
}

/// Save hosts profiles to disk.
pub fn save_hosts_profiles(store: &crate::hosts_profiles::HostsProfileStore) -> Result<()> {
    crate::hosts_profiles::save(store)
}

// ---------------------------------------------------------------------------
// Notes persistence.
// ---------------------------------------------------------------------------

/// Load notes from disk.
pub fn load_notes() -> crate::notes::NotesStore {
    crate::notes::load()
}

/// Save notes to disk.
pub fn save_notes(store: &crate::notes::NotesStore) -> Result<()> {
    crate::notes::save(store)
}

/// Persist a single note to its own `.md` file (atomic write; title changes
/// rename the file automatically via the `paths` cache in O(1)).
pub fn save_note_one(
    note: &crate::notes::Note,
    paths: &mut crate::notes::NoteFilePaths,
) -> Result<()> {
    crate::notes::save_note(note, paths)
}

/// Persist notes UI state + sidebar ordering to `notes.meta.json`.
pub fn save_notes_meta(store: &crate::notes::NotesStore) -> Result<()> {
    crate::notes::save_meta(store)
}

/// Delete the on-disk `.md` files for each note id (used by cascade deletes),
/// in O(ids) using the path cache.
pub fn delete_notes_files(ids: &[String], paths: &mut crate::notes::NoteFilePaths) {
    crate::notes::delete_notes_files(ids, paths)
}

// ---------------------------------------------------------------------------
// Panel layout persistence (resizable panel sizes).
// ---------------------------------------------------------------------------

/// Saved panel sizes, in pixels, for the main horizontal and center vertical
/// resizable groups. `None` entries mean "use the default".
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PanelLayout {
    /// [tree_width] for the main horizontal group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<[f32; 2]>,
    /// [request_height, response_height, console_height] for the center vertical group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f32; 3]>,
    /// User-customizable columns shown in the folder interface list, by serde
    /// lowercase name. Empty/absent means "use the defaults".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iface_columns: Option<Vec<String>>,
    /// Git integration config (auth token, remote url, auto-push toggle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitConfig>,
    /// The activity-rail view the brand mark "V" (首页) points to, stored as
    /// the SideView variant's serde name (e.g. "Api", "AutoTest"). The pointed-
    /// to module is hidden from the rail to avoid a duplicate entry. Defaults
    /// to "Api" when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_view: Option<String>,
    /// The UI locale code (e.g. "zh-CN", "en"). Applied via
    /// `rust_i18n::set_locale` at startup. Defaults to "zh-CN".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Which activity-rail items are hidden. Each entry is a SideView name
    /// (e.g. "History", "Ssh"). Absent/empty means all items are shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_rails: Option<Vec<String>>,
    /// Custom order of activity-rail items (SideView names). When present, rail
    /// buttons render in this order. Items not in the list appear after in default order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rail_order: Option<Vec<String>>,
    /// Remote document-sharing server configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteConfig>,
    /// Whether the notes panel's left-side note tree is collapsed.
    #[serde(default)]
    pub notes_collapsed: bool,
    /// Left-sidebar widths (px) for panels with a fixed left tree/list:
    /// [notes, autotest, stress, hosts]. Each is optional so absent = panel default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_widths: Option<[f32; 4]>,
    /// Workspace-drawer (outline) width (px) for the markdown editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown_drawer_width: Option<f32>,
    /// Custom WYSIWYG editor keyboard shortcuts, keyed by shortcut id (e.g.
    /// "bold_selection") → list of key strings (e.g. ["cmd-b", "ctrl-b"]).
    /// Applied at startup via `init_with_keybindings`. Absent = use defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// After-send autosave behavior for response examples.
    /// 0 = off (不保存), 1 = save success example (自动保存到成功示例),
    /// 2 = save failure examples (自动保存到失败示例).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autosave_examples: Option<u8>,
}

/// After-send autosave mode for response examples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoSaveMode {
    /// Do not autosave (default).
    #[default]
    Off,
    /// Autosave successful responses (2xx/3xx) as the success example.
    SaveSuccess,
    /// Autosave failed responses (4xx/5xx/errors) as failure examples.
    SaveFailure,
    /// Autosave both: success responses to the success example, failures to
    /// the failure examples list.
    SaveBoth,
}

impl AutoSaveMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => AutoSaveMode::SaveSuccess,
            2 => AutoSaveMode::SaveFailure,
            3 => AutoSaveMode::SaveBoth,
            _ => AutoSaveMode::Off,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            AutoSaveMode::Off => 0,
            AutoSaveMode::SaveSuccess => 1,
            AutoSaveMode::SaveFailure => 2,
            AutoSaveMode::SaveBoth => 3,
        }
    }

    /// Whether this mode saves successful responses as the success example.
    pub fn saves_success(self) -> bool {
        matches!(self, AutoSaveMode::SaveSuccess | AutoSaveMode::SaveBoth)
    }

    /// Whether this mode saves failed responses as failure examples.
    pub fn saves_failure(self) -> bool {
        matches!(self, AutoSaveMode::SaveFailure | AutoSaveMode::SaveBoth)
    }

    pub fn label(self) -> &'static str {
        match self {
            AutoSaveMode::Off => "不保存（默认）",
            AutoSaveMode::SaveSuccess => "自动保存到成功示例",
            AutoSaveMode::SaveFailure => "自动保存到失败示例",
            AutoSaveMode::SaveBoth => "同时自动保存到成功和失败示例",
        }
    }
}

/// Persisted Git integration settings.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GitConfig {
    #[serde(default)]
    pub auto_commit: bool,
    #[serde(default)]
    pub auto_push: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// Auto-sync interval in minutes. `None` means "use the default of 30".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_interval_minutes: Option<u32>,
}

/// Remote document-sharing server configuration.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemoteConfig {
    /// Whether the share server accepts connections beyond loopback.
    #[serde(default)]
    pub enabled: bool,
    /// Bind address: "127.0.0.1" for local-only, "0.0.0.0" for LAN/public.
    #[serde(default = "default_bind_host")]
    pub host: String,
    /// TCP port (default 3097).
    #[serde(default = "default_remote_port")]
    pub port: u16,
    /// Optional bearer token protecting /admin/api/* routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// When true, shares are namespaced by workspace id (multi-tenant).
    #[serde(default)]
    pub multi_tenant: bool,
    /// Optional public hostname displayed in share URLs (vs the bind host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_host: Option<String>,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_bind_host(),
            port: default_remote_port(),
            auth_token: None,
            multi_tenant: false,
            public_host: None,
        }
    }
}

fn default_bind_host() -> String {
    "127.0.0.1".to_string()
}

fn default_remote_port() -> u16 {
    crate::share::server::DEFAULT_PORT
}

impl PanelLayout {
    /// Load the saved git config or a default (auto_commit on, auto_push off).
    pub fn git_config(&self) -> GitConfig {
        self.git.clone().unwrap_or_else(|| GitConfig {
            auto_commit: true,
            auto_push: false,
            remote: None,
            username: String::new(),
            token: String::new(),
            sync_interval_minutes: None,
        })
    }
}

/// Default auto-sync interval: 30 minutes.
pub const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 30;

/// Load the configured auto-sync interval in minutes (default 30).
pub fn load_sync_interval_minutes() -> u32 {
    load_git_config()
        .sync_interval_minutes
        .unwrap_or(DEFAULT_SYNC_INTERVAL_MINUTES)
}

/// Persist the auto-sync interval (in minutes) into the git config.
pub fn save_sync_interval_minutes(minutes: u32) {
    let mut cfg = load_git_config();
    cfg.sync_interval_minutes = Some(minutes);
    save_git_config(&cfg);
}

fn layout_path(dir: &Path) -> PathBuf {
    dir.join("layout.json")
}

fn local_layout_path(dir: &Path) -> PathBuf {
    dir.join("layout.local.json")
}

/// Machine-specific panel sizes (kept out of git so different machines can have different layouts).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct LocalLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f32; 3]>,
}

/// Load the saved panel layout, or `None` on first run / corruption.
/// Merges shared settings from layout.json with machine-specific sizes from layout.local.json.
pub fn load_layout() -> Option<PanelLayout> {
    let dir = data_dir().ok()?;

    // Load shared layout.
    let mut layout: PanelLayout = match fs::read_to_string(layout_path(&dir)) {
        Ok(contents) => serde_json::from_str::<PanelLayout>(&contents)
            .map(|mut l| {
                // Migration: if old layout.json contains main/center, extract to local and save back.
                if l.main.is_some() || l.center.is_some() {
                    let local = LocalLayout {
                        main: l.main.take(),
                        center: l.center.take(),
                    };
                    let _ = save_local_layout(&local);
                    let _ = save_layout(&l);
                    log::info!("Migrated layout: panel sizes moved to layout.local.json");
                }
                l
            })
            .ok()?,
        Err(_) => return None,
    };

    // Overlay machine-specific sizes.
    if let Ok(contents) = fs::read_to_string(local_layout_path(&dir)) {
        if let Ok(local) = serde_json::from_str::<LocalLayout>(&contents) {
            layout.main = local.main;
            layout.center = local.center;
        }
    }

    Some(layout)
}

/// Save machine-specific panel sizes to layout.local.json (git-ignored).
fn save_local_layout(local: &LocalLayout) -> Result<()> {
    let dir = data_dir()?;
    let path = local_layout_path(&dir);
    let json = serde_json::to_string_pretty(local)?;
    fs::write(&path, json).with_context(|| format!("write local layout {:?}", path))?;
    Ok(())
}

/// Persist the panel layout. Shared settings go to layout.json (tracked by git);
/// panel sizes go to layout.local.json (ignored by git).
pub fn save_layout(layout: &PanelLayout) -> Result<()> {
    let dir = data_dir()?;

    // Save machine-specific sizes separately.
    let local = LocalLayout {
        main: layout.main,
        center: layout.center,
    };
    save_local_layout(&local)?;

    // Save shared settings to layout.json (without main/center).
    let mut shared = layout.clone();
    shared.main = None;
    shared.center = None;
    let path = layout_path(&dir);
    let json = serde_json::to_string_pretty(&shared)?;
    fs::write(&path, json).with_context(|| format!("write layout {:?}", path))?;
    Ok(())
}

/// Load the user's customized interface-list columns, falling back to the
/// default set when unset/invalid.
pub fn load_iface_columns() -> Vec<crate::state::models::IfaceColumn> {
    let raw = load_layout().and_then(|l| l.iface_columns);
    match raw {
        Some(names) => {
            let parsed: Vec<_> = names
                .iter()
                .filter_map(|n| crate::state::models::IfaceColumn::parse(n))
                .collect();
            if parsed.is_empty() {
                crate::state::models::IfaceColumn::defaults()
            } else {
                parsed
            }
        }
        None => crate::state::models::IfaceColumn::defaults(),
    }
}

/// Persist the user's customized interface-list columns.
pub fn save_iface_columns(cols: &[crate::state::models::IfaceColumn]) {
    let mut layout = load_layout().unwrap_or_default();
    layout.iface_columns = Some(cols.iter().map(|c| c.to_string()).collect());
    let _ = save_layout(&layout);
}

/// Load the saved git config (or defaults).
pub fn load_git_config() -> GitConfig {
    load_layout().unwrap_or_default().git_config()
}

/// Persist the git config.
pub fn save_git_config(cfg: &GitConfig) {
    let mut layout = load_layout().unwrap_or_default();
    layout.git = Some(cfg.clone());
    let _ = save_layout(&layout);
}

/// Load the set of hidden activity-rail views (by SideView name).
///
/// Factory default: History is hidden from the rail (users can enable it in
/// Settings > Activity Bar Items). If the user has explicitly customized the
/// hidden-rails list (stored as `Some(v)`), that choice is respected verbatim.
/// Only when the field is `None` (never customized) do we apply the default.
pub fn load_hidden_rails() -> std::collections::HashSet<String> {
    match load_layout().and_then(|l| l.hidden_rails) {
        Some(v) => v.into_iter().collect(),
        None => {
            let mut set = std::collections::HashSet::new();
            set.insert("History".to_string());
            set
        }
    }
}

/// Persist the set of hidden activity-rail views. Always writes an explicit
/// array (even when empty) so load_hidden_rails can distinguish "user has
/// customized" from "never customized" (None), where the latter applies the
/// factory default of hiding History.
pub fn save_hidden_rails(hidden: &std::collections::HashSet<String>) {
    let mut layout = load_layout().unwrap_or_default();
    layout.hidden_rails = Some(hidden.iter().cloned().collect());
    let _ = save_layout(&layout);
}

/// Load custom WYSIWYG shortcut overrides (id → keys). Empty when none set.
pub fn load_shortcuts() -> std::collections::BTreeMap<String, Vec<String>> {
    load_layout().and_then(|l| l.shortcuts).unwrap_or_default()
}

/// Persist custom WYSIWYG shortcut overrides. Applied on next app startup.
pub fn save_shortcuts(map: &std::collections::BTreeMap<String, Vec<String>>) {
    let mut layout = load_layout().unwrap_or_default();
    layout.shortcuts = if map.is_empty() {
        None
    } else {
        Some(map.clone())
    };
    let _ = save_layout(&layout);
}

/// Load the custom order of activity-rail views (SideView names).
/// Returns None when no custom order is set (caller should use default order).
pub fn load_rail_order() -> Option<Vec<String>> {
    load_layout().and_then(|l| l.rail_order)
}

/// Persist the custom order of activity-rail views.
pub fn save_rail_order(order: &[String]) {
    let mut layout = load_layout().unwrap_or_default();
    layout.rail_order = Some(order.to_vec());
    let _ = save_layout(&layout);
}

/// Load the remote sharing-server configuration (defaults to local-only).
pub fn load_remote_config() -> RemoteConfig {
    load_layout().unwrap_or_default().remote.unwrap_or_default()
}

/// Persist the remote sharing-server configuration.
pub fn save_remote_config(cfg: &RemoteConfig) {
    let mut layout = load_layout().unwrap_or_default();
    layout.remote = Some(cfg.clone());
    let _ = save_layout(&layout);
}

/// Load the configured after-send autosave mode for response examples.
pub fn load_autosave_mode() -> AutoSaveMode {
    load_layout()
        .and_then(|l| l.autosave_examples)
        .map(AutoSaveMode::from_u8)
        .unwrap_or_default()
}

/// Persist the after-send autosave mode for response examples.
pub fn save_autosave_mode(mode: AutoSaveMode) {
    let mut layout = load_layout().unwrap_or_default();
    layout.autosave_examples = Some(mode.to_u8());
    let _ = save_layout(&layout);
}
