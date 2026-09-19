//! Disk-backed workspace access for the MCP server.
//!
//! The MCP server runs as a **separate subprocess** (the AI client spawns
//! `verve mcp`), so it never shares the GUI's in-memory [`AppState`]. Every
//! tool call therefore reads the current `workspace.json` from disk, and
//! writes go through the same backup → mutate → persist → round-trip validate
//! → rollback guard as the in-app AI agent (`ai::agent::apply_guarded`).
//!
//! [`AppState`]: crate::state::AppState

use crate::state::models::WorkspaceData;
use crate::state::persistence;

/// Load the active workspace straight from disk (fresh on every tool call).
pub fn load() -> WorkspaceData {
    persistence::load_or_default()
}

/// Index of the active project in `data.projects`, clamped to 0 / last.
pub fn active_project_index(data: &WorkspaceData) -> usize {
    let idx = data
        .active_project_id
        .as_deref()
        .and_then(|id| data.projects.iter().position(|p| p.id == id))
        .unwrap_or(0);
    idx.min(data.projects.len().saturating_sub(1))
}

/// Resolve a project selector (id or unique name) to an index.
///
/// `None` / empty / whitespace means the active project. Name matching is the
/// tolerant fallback the internal AI tools use (unique name accepted,
/// ambiguous names rejected and asked for an id).
pub fn resolve_project(data: &WorkspaceData, selector: Option<&str>) -> Result<usize, String> {
    let Some(sel) = selector.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(active_project_index(data));
    };
    if let Some(i) = data.projects.iter().position(|p| p.id == sel) {
        return Ok(i);
    }
    let matches: Vec<usize> = data
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| p.name == sel)
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!("项目不存在: {sel}（可传 project_id 或唯一项目名）")),
        _ => Err(format!("项目名「{sel}」存在多个匹配，请改用 project_id")),
    }
}

/// Whether `workspace.json` currently exists on disk.
pub fn workspace_exists() -> bool {
    persistence::workspace_file()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Run a mutation under the file-write safety net:
/// 1. refuse to touch a corrupt workspace;
/// 2. copy `workspace.json` → `.bak`;
/// 3. run the closure on freshly-loaded data and persist;
/// 4. parse-check the written file and restore the backup on failure.
pub fn mutate<T>(
    f: impl FnOnce(&mut WorkspaceData) -> Result<T, String>,
) -> Result<T, String> {
    if workspace_exists() {
        if let Err(e) = persistence::validate_workspace_file() {
            return Err(format!("workspace.json 已损坏，已拒绝写入: {e}"));
        }
    }
    let backup = persistence::backup_workspace_file();
    let mut data = persistence::load_or_default();
    let out = f(&mut data)?;
    if let Err(e) = persistence::save(&data) {
        return Err(format!("保存 workspace.json 失败: {e}"));
    }
    if let Err(e) = persistence::validate_workspace_file() {
        if backup.is_some() {
            match persistence::restore_workspace_backup() {
                Ok(()) => log::error!("MCP 写入后校验失败，已从 .bak 回滚: {e}"),
                Err(re) => log::error!("MCP 写入后校验失败且回滚失败: {e}; {re}"),
            }
        }
        return Err(format!("保存后校验失败，已回滚: {e}"));
    }
    Ok(out)
}
