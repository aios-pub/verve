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

/// Insert a share, replacing any existing share for the same document target
/// (same project_id + scope + target_id). Only the latest share for a given
/// document is kept; older shares become invalid.
pub fn upsert_share(share: ShareConfig) -> Vec<ShareConfig> {
    let mut shares = load_shares();
    // Remove any existing share for the same document target
    shares.retain(|s| {
        !(s.project_id == share.project_id
            && s.scope == share.scope
            && s.target_id == share.target_id)
    });
    // Add the new share
    shares.push(share);
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
        // Isolate data_dir() at a unique per-thread temp dir (no HOME mutation,
        // no mutex) so this test can't race with sibling data-dir tests.
        let tmp = std::env::temp_dir().join(format!("verve-share-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());
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

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn same_target_replaces_old_share() {
        // Isolate data_dir() at a unique per-thread temp dir (no HOME mutation,
        // no mutex) so this test can't race with sibling data-dir tests.
        let tmp = std::env::temp_dir().join(format!("verve-share-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());
        // Create first share for project proj-1, project scope
        let s1 = ShareConfig::new("proj-1", "My Project");
        let id1 = s1.id.clone();
        upsert_share(s1);
        assert_eq!(load_shares().len(), 1);

        // Create second share for the same target
        let mut s2 = ShareConfig::new("proj-1", "My Project");
        s2.title = "Updated Docs".into();
        let id2 = s2.id.clone();
        let saved = upsert_share(s2);

        // Should only have one share, the new one
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id2);
        assert_eq!(saved[0].title, "Updated Docs");

        let loaded = load_shares();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, id2);
        assert_ne!(loaded[0].id, id1);

        // Cleanup
        remove_share(&id2);
        assert!(load_shares().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn different_targets_coexist() {
        // Isolate data_dir() at a unique per-thread temp dir (no HOME mutation,
        // no mutex) so this test can't race with sibling data-dir tests.
        let tmp = std::env::temp_dir().join(format!("verve-share-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());
        // Create share for project proj-1
        let s1 = ShareConfig::new("proj-1", "Project 1");
        let id1 = s1.id.clone();
        upsert_share(s1);

        // Create share for different project proj-2
        let s2 = ShareConfig::new("proj-2", "Project 2");
        let id2 = s2.id.clone();
        upsert_share(s2);

        // Create share for same project but different target (specific request)
        let mut s3 = ShareConfig::new("proj-1", "Project 1");
        s3.scope = crate::share::ShareScope::Request;
        s3.target_id = Some("req-123".into());
        let id3 = s3.id.clone();
        upsert_share(s3);

        // All three should exist
        let loaded = load_shares();
        assert_eq!(loaded.len(), 3);
        assert!(loaded.iter().any(|s| s.id == id1));
        assert!(loaded.iter().any(|s| s.id == id2));
        assert!(loaded.iter().any(|s| s.id == id3));

        // Cleanup
        remove_share(&id1);
        remove_share(&id2);
        remove_share(&id3);
        assert!(load_shares().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
