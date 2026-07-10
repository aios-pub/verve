//! Cloud-native storage for `verve-server`.
//!
//! The desktop app reads/writes projects through GPUI state and a single
//! `workspace.json`. That doesn't fit a headless server, which needs its own
//! multi-tenant project store plus the ability to create shares from the admin
//! UI. This module provides a file-backed store under `<data_dir>/cloud/`:
//!
//! ```text
//! <data_dir>/cloud/
//! ├── shares.json              # all share configs (mirror of desktop shares.json)
//! └── projects/
//!     ├── <project-id>.json    # one Project per file (Verve-native JSON)
//!     └── ...
//! ```
//!
//! The store is the single source of truth for the cloud binary; the desktop
//! app keeps using its in-memory provider. Both go through the same
//! [`ProjectProvider`] trait so [`super::server`] routes work identically.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::share::models::ShareConfig;
use crate::share::persist;
use crate::state::models::Project;

/// The sub-directory under the data dir holding the cloud store.
pub const CLOUD_DIR: &str = "cloud";
pub const PROJECTS_DIR: &str = "projects";

/// Resolve the cloud root: `<data_dir>/cloud/`.
pub fn cloud_dir() -> Option<PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join(CLOUD_DIR))
}

/// Resolve the projects directory: `<data_dir>/cloud/projects/`.
pub fn projects_dir() -> Option<PathBuf> {
    cloud_dir().map(|d| d.join(PROJECTS_DIR))
}

/// A file-backed store for the cloud server: shares + projects on disk, with
/// an in-memory cache for fast lookups. Thread-safe via `RwLock`.
#[derive(Clone)]
pub struct CloudStore {
    /// Cached projects keyed by id.
    projects: Arc<RwLock<HashMap<String, Project>>>,
    /// Cached shares.
    shares: Arc<RwLock<Vec<ShareConfig>>>,
}

impl CloudStore {
    /// Load the store from disk, creating the directories if missing. Returns
    /// an empty store (no projects, no shares) when the dirs don't exist yet —
    /// the admin UI seeds them via uploads.
    pub fn load() -> Self {
        let projects = load_all_projects();
        let shares = persist::load_shares();
        Self {
            projects: Arc::new(RwLock::new(projects)),
            shares: Arc::new(RwLock::new(shares)),
        }
    }

    /// Insert or replace a project (also writes it to disk).
    pub fn upsert_project(&self, project: Project) {
        if let Some(dir) = projects_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("{}.json", sanitize_id(&project.id)));
            if let Ok(json) = serde_json::to_string_pretty(&project) {
                if std::fs::write(&path, json).is_err() {
                    log::error!("cloud: failed to write project {}", path.display());
                }
            }
        }
        if let Ok(mut guard) = self.projects.write() {
            guard.insert(project.id.clone(), project);
        }
    }

    /// Look up a project by id (cache hit is the common path).
    pub fn get_project(&self, id: &str) -> Option<Project> {
        if let Ok(guard) = self.projects.read() {
            if let Some(p) = guard.get(id) {
                return Some(p.clone());
            }
        }
        // Cache miss: try disk (e.g. written by another process).
        if let Some(p) = load_project(id) {
            if let Ok(mut guard) = self.projects.write() {
                guard.insert(id.to_string(), p.clone());
            }
            return Some(p);
        }
        None
    }

    /// Delete a project by id (disk + cache).
    pub fn delete_project(&self, id: &str) -> bool {
        let removed = if let Ok(mut guard) = self.projects.write() {
            guard.remove(id).is_some()
        } else {
            false
        };
        if let Some(dir) = projects_dir() {
            let path = dir.join(format!("{}.json", sanitize_id(id)));
            let _ = std::fs::remove_file(&path);
        }
        removed
    }

    /// List all projects (id, name) for the admin UI.
    pub fn list_projects(&self) -> Vec<(String, String)> {
        if let Ok(guard) = self.projects.read() {
            let mut v: Vec<(String, String)> = guard
                .values()
                .map(|p| (p.id.clone(), p.name.clone()))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        } else {
            Vec::new()
        }
    }

    /// Insert or replace a share config (disk + cache).
    pub fn upsert_share(&self, share: ShareConfig) {
        let all = persist::upsert_share(share.clone());
        if let Ok(mut guard) = self.shares.write() {
            *guard = all;
        }
    }

    /// All shares (cached).
    pub fn shares(&self) -> Vec<ShareConfig> {
        self.shares.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Delete a share by id (disk + cache).
    pub fn delete_share(&self, id: &str) {
        let all = persist::remove_share(id);
        if let Ok(mut guard) = self.shares.write() {
            *guard = all;
        }
    }
}

/// Load a single project by id from `<cloud>/projects/<id>.json`.
pub fn load_project(id: &str) -> Option<Project> {
    let dir = projects_dir()?;
    let path = dir.join(format!("{}.json", sanitize_id(id)));
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<Project>(&bytes)
        .map_err(|e| log::warn!("cloud: parse {}: {e}", path.display()))
        .ok()
}

/// Load every project file under `<cloud>/projects/`.
pub fn load_all_projects() -> HashMap<String, Project> {
    let mut map = HashMap::new();
    let Some(dir) = projects_dir() else {
        return map;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(project) = serde_json::from_slice::<Project>(&bytes) {
                map.insert(project.id.clone(), project);
            }
        }
    }
    map
}

/// Sanitize an id for use as a filename (strip path separators, etc.).
fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::models::{ApiRequest, RequestMethod};

    #[test]
    fn cloud_store_round_trips_projects() {
        // Point HOME at a temp dir so data_dir() resolves there.
        let tmp = std::env::temp_dir().join(format!(
            "verve-cloud-test-{}",
            crate::state::models::new_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);

        let store = CloudStore::load();
        let mut project = Project::new("Cloud API");
        project
            .requests
            .push(ApiRequest::new("Health", RequestMethod::Get, "/health"));
        let pid = project.id.clone();
        store.upsert_project(project);

        // Fresh load sees the persisted project.
        let store2 = CloudStore::load();
        assert!(store2.get_project(&pid).is_some());
        let names = store2.list_projects();
        assert!(names.iter().any(|(_, n)| n == "Cloud API"));

        // Delete.
        assert!(store2.delete_project(&pid));
        let store3 = CloudStore::load();
        assert!(store3.get_project(&pid).is_none());

        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sanitize_strips_separators() {
        assert_eq!(sanitize_id("abc/def"), "abcdef");
        assert_eq!(sanitize_id("../etc"), "etc");
        assert_eq!(sanitize_id("ok-id_123"), "ok-id_123");
    }
}
