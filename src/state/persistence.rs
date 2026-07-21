//! Local JSON-file persistence for the workspace.
//!
//! The full workspace is stored as a single JSON document at
//! `<data_dir>/verve/workspace.json`, where `data_dir` is the platform
//! convention (`~/.verve` on all platforms for simplicity).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use super::models::WorkspaceData;

/// Return the workspace directory, creating it if needed.
pub fn data_dir() -> Result<PathBuf> {
    let dir = home_data_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("create data dir {:?}", dir))?;
    Ok(dir)
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

/// The `.gitignore` content excluding cross-branch / non-data files from git.
const GITIGNORE: &str =
    "workspaces.json\nlayout.json\nshares.json\nssh_hosts.json\ndocker_hosts.json\ndocker_active.json\nkube_config.yaml\nkube_clusters.json\n.verve-askpass.sh\nexports/\n";

/// Ensure `~/.verve/.gitignore` exists with the right exclusions. Called once
/// when the git repo is initialised so cross-branch files don't leak into
/// per-workspace commits.
pub fn ensure_gitignore() {
    let Ok(dir) = data_dir() else { return };
    let path = dir.join(".gitignore");
    // Re-write if missing or if it's missing any of the known exclusions so
    // newly added entries (e.g. shares.json) are picked up on existing installs.
    let needs_write = match fs::read_to_string(&path) {
        Ok(existing) => {
            !existing.contains("workspaces.json")
                || !existing.contains("shares.json")
                || !existing.contains("ssh_hosts.json")
                || !existing.contains("docker_hosts.json")
                || !existing.contains("kube_config.yaml")
        }
        Err(_) => true,
    };
    if needs_write {
        let _ = fs::write(&path, GITIGNORE);
    }
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
        })
    }
}

fn layout_path(dir: &Path) -> PathBuf {
    dir.join("layout.json")
}

/// Load the saved panel layout, or `None` on first run / corruption.
pub fn load_layout() -> Option<PanelLayout> {
    let dir = data_dir().ok()?;
    let path = layout_path(&dir);
    let contents = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<PanelLayout>(&contents).ok()
}

/// Persist the panel layout.
pub fn save_layout(layout: &PanelLayout) -> Result<()> {
    let dir = data_dir()?;
    let path = layout_path(&dir);
    let json = serde_json::to_string_pretty(layout)?;
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
