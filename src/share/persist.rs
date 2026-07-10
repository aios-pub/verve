//! Persistence for share configs (`~/.verve/shares.json`).
//!
//! Mirrors the workspace-index pattern in `state::persistence`: a single JSON
//! file, cross-workspace, git-ignored, written atomically. Reads return an
//! empty list when the file is missing or corrupt (never panic the UI).

use std::fs;
use std::path::PathBuf;

use super::models::ShareConfig;

/// `~/.verve/shares.json`.
pub fn shares_path() -> Option<PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("shares.json"))
}

/// Load all share configs. Returns an empty vec on any error.
pub fn load_shares() -> Vec<ShareConfig> {
    let Some(path) = shares_path() else {
        return Vec::new();
    };
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str::<Vec<ShareConfig>>(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Persist all share configs atomically (write `.tmp` then rename), matching
/// the workspace save pattern. Logs on failure; never panics.
pub fn save_shares(shares: &[ShareConfig]) {
    let Some(path) = shares_path() else {
        log::error!("shares.json path unavailable; cannot save shares");
        return;
    };
    let Ok(json) = serde_json::to_string_pretty(shares) else {
        log::error!("failed to serialize shares.json");
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = fs::write(&tmp, json) {
        log::error!("failed to write {}: {e}", tmp.display());
        return;
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        log::error!(
            "failed to rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        );
    } else {
        log::debug!("saved {} share(s) to {}", shares.len(), path.display());
    }
}

/// Insert or replace a share (matched by id).
pub fn upsert_share(share: ShareConfig) -> Vec<ShareConfig> {
    let mut shares = load_shares();
    if let Some(existing) = shares.iter_mut().find(|s| s.id == share.id) {
        *existing = share;
    } else {
        shares.push(share);
    }
    save_shares(&shares);
    shares
}

/// Remove a share by id. Returns the resulting list.
pub fn remove_share(id: &str) -> Vec<ShareConfig> {
    let mut shares = load_shares();
    shares.retain(|s| s.id != id);
    save_shares(&shares);
    shares
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::models::new_id;

    #[test]
    fn round_trip_via_temp_dir() {
        // Use the shared ssh mutex to serialize all HOME-dependent tests.
        let _guard = crate::ssh::TEST_HOME_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let tmp = std::env::temp_dir().join(format!("verve-share-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);

        let mut s = ShareConfig::new("proj-1", "My Project");
        s.title = "Docs".into();
        let saved = upsert_share(s.clone());
        assert_eq!(saved.len(), 1);

        let loaded = load_shares();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, s.id);
        assert_eq!(loaded[0].title, "Docs");

        let after_remove = remove_share(&s.id);
        assert!(after_remove.is_empty());
        assert!(load_shares().is_empty());

        // Restore HOME.
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
