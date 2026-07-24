//! Hosts profile data model + persistence.
//!
//! A "hosts profile" is a named set of /etc/hosts-style entries that can be
//! toggled on/off, bound to specific environments, and applied either as an
//! in-app DNS override (virtual) or written to the system hosts file.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::state::persistence::data_dir;

fn default_true() -> bool {
    true
}

/// A single hosts entry in a profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostEntryEdit {
    pub id: String,
    pub ip: String,
    pub host: String,
    pub comment: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A named group of hosts entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostsProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub entries: Vec<HostEntryEdit>,
    /// When non-empty, entries are only applied when the active environment id
    /// is in this list.
    #[serde(default)]
    pub bound_envs: Vec<String>,
    /// Write to the system /etc/hosts file (requires privilege escalation).
    #[serde(default)]
    pub apply_to_system: bool,
    /// Override DNS inside the app's HTTP requests (no privileges needed).
    #[serde(default = "default_true")]
    pub apply_virtual: bool,
}

/// Complete hosts configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostsProfileStore {
    #[serde(default)]
    pub profiles: Vec<HostsProfile>,
    /// Currently selected profile id in the editor UI.
    #[serde(default)]
    pub active_profile: Option<String>,
}

fn profiles_path(dir: &Path) -> std::path::PathBuf {
    dir.join("hosts_profiles.json")
}

/// Load hosts profiles from disk, or return defaults.
pub fn load() -> HostsProfileStore {
    let Ok(dir) = data_dir() else {
        return default_store();
    };
    let path = profiles_path(&dir);
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<HostsProfileStore>(&contents) {
            Ok(mut store) => {
                if store.profiles.is_empty() {
                    store = default_store();
                }
                store
            }
            Err(e) => {
                log::warn!("hosts_profiles.json corrupt, starting fresh: {e:?}");
                default_store()
            }
        },
        Err(_) => default_store(),
    }
}

/// Persist hosts profiles to disk.
pub fn save(store: &HostsProfileStore) -> Result<()> {
    let dir = data_dir()?;
    let path = profiles_path(&dir);
    let json = serde_json::to_string_pretty(store)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).with_context(|| format!("write hosts profiles {:?}", tmp))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename hosts profiles {:?}", path))?;
    Ok(())
}

fn default_store() -> HostsProfileStore {
    let default_id = uuid::Uuid::new_v4().to_string();
    HostsProfileStore {
        profiles: vec![HostsProfile {
            id: default_id.clone(),
            name: "default".to_string(),
            enabled: true,
            entries: Vec::new(),
            bound_envs: Vec::new(),
            apply_to_system: false,
            apply_virtual: true,
        }],
        active_profile: Some(default_id),
    }
}

/// Create a new empty profile.
pub fn create_profile(store: &mut HostsProfileStore, name: String) -> &HostsProfile {
    let id = uuid::Uuid::new_v4().to_string();
    let profile = HostsProfile {
        id: id.clone(),
        name: if name.is_empty() {
            rust_i18n::t!("hosts.new_profile").to_string()
        } else {
            name
        },
        enabled: false,
        entries: Vec::new(),
        bound_envs: Vec::new(),
        apply_to_system: false,
        apply_virtual: true,
    };
    store.profiles.push(profile);
    store.active_profile = Some(id);
    store.profiles.last().unwrap()
}

/// Delete a profile by id.
pub fn delete_profile(store: &mut HostsProfileStore, id: &str) {
    store.profiles.retain(|p| p.id != id);
    if store.active_profile.as_deref() == Some(id) {
        store.active_profile = store.profiles.first().map(|p| p.id.clone());
    }
}

/// Add a new entry to the active (or specified) profile.
pub fn add_entry(store: &mut HostsProfileStore, profile_id: &str) {
    let entry = HostEntryEdit {
        id: uuid::Uuid::new_v4().to_string(),
        ip: "127.0.0.1".to_string(),
        host: String::new(),
        comment: None,
        enabled: true,
    };
    if let Some(p) = store.profiles.iter_mut().find(|p| p.id == profile_id) {
        p.entries.push(entry);
    } else if !store.profiles.is_empty() {
        store.profiles[0].entries.push(entry);
    }
}

/// Remove an entry.
pub fn remove_entry(store: &mut HostsProfileStore, profile_id: &str, entry_id: &str) {
    if let Some(p) = store.profiles.iter_mut().find(|p| p.id == profile_id) {
        p.entries.retain(|e| e.id != entry_id);
    }
}

/// Compute effective hostname→ip overrides from enabled profiles, filtered by
/// environment binding. Returns `(hostname, ip)` pairs.
pub fn effective_virtual_overrides(
    store: &HostsProfileStore,
    active_env_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for p in &store.profiles {
        if !p.enabled || !p.apply_virtual {
            continue;
        }
        if !p.bound_envs.is_empty() {
            if let Some(env_id) = active_env_id {
                if !p.bound_envs.iter().any(|e| e == env_id) {
                    continue;
                }
            } else {
                continue;
            }
        }
        for e in &p.entries {
            if e.enabled && !e.host.is_empty() && !e.ip.is_empty() {
                out.push((e.host.clone(), e.ip.clone()));
            }
        }
    }
    out
}

/// Start/end markers for the Verve-managed block in the system hosts file.
pub const VERVE_BLOCK_START: &str = "# >>> verve hosts start >>>";
pub const VERVE_BLOCK_END: &str = "# <<< verve hosts end <<<";

/// Render enabled profiles' entries as /etc/hosts format lines.
pub fn render_enabled_entries(store: &HostsProfileStore, active_env_id: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(VERVE_BLOCK_START);
    s.push('\n');
    s.push_str("# Managed by Verve — do not edit between markers.\n");
    for p in &store.profiles {
        if !p.enabled || !p.apply_to_system {
            continue;
        }
        if !p.bound_envs.is_empty() {
            if let Some(env_id) = active_env_id {
                if !p.bound_envs.iter().any(|e| e == env_id) {
                    continue;
                }
            } else {
                continue;
            }
        }
        s.push_str(&format!("# Profile: {}\n", p.name));
        for e in &p.entries {
            if !e.enabled || e.host.is_empty() || e.ip.is_empty() {
                continue;
            }
            s.push_str(&e.ip);
            s.push('\t');
            s.push_str(&e.host);
            if let Some(c) = &e.comment {
                s.push_str("  # ");
                s.push_str(c);
            }
            s.push('\n');
        }
    }
    s.push_str(VERVE_BLOCK_END);
    s.push('\n');
    s
}

/// Merge Verve-managed content into an existing hosts file, replacing any
/// previous Verve block. Returns the new file content.
pub fn merge_into_existing(existing: &str, verve_block: &str) -> String {
    let start_idx = existing.find(VERVE_BLOCK_START);
    let end_idx = existing.find(VERVE_BLOCK_END);

    match (start_idx, end_idx) {
        (Some(s), Some(e)) => {
            let end_pos = e + VERVE_BLOCK_END.len();
            // Find end of line after end marker.
            let end_pos = if existing[end_pos..].starts_with('\n') {
                end_pos + 1
            } else if existing[end_pos..].starts_with("\r\n") {
                end_pos + 2
            } else {
                end_pos
            };
            let before = existing[..s].to_string();
            let after = &existing[end_pos..];
            format!("{}{}{}", before, verve_block, after)
        }
        _ => {
            // No existing block — append.
            let mut out = existing.to_string();
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(verve_block);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_overrides_basic() {
        let mut store = default_store();
        store.profiles[0].enabled = true;
        store.profiles[0].entries.push(HostEntryEdit {
            id: "1".into(),
            ip: "127.0.0.1".into(),
            host: "api.local".into(),
            comment: None,
            enabled: true,
        });
        let overrides = effective_virtual_overrides(&store, None);
        assert_eq!(overrides.len(), 1);
        assert_eq!(
            overrides[0],
            ("api.local".to_string(), "127.0.0.1".to_string())
        );
    }

    #[test]
    fn virtual_overrides_env_binding() {
        let mut store = default_store();
        store.profiles[0].enabled = true;
        store.profiles[0].bound_envs = vec!["env-dev".into()];
        store.profiles[0].entries.push(HostEntryEdit {
            id: "1".into(),
            ip: "10.0.0.1".into(),
            host: "db.local".into(),
            comment: None,
            enabled: true,
        });
        // No env active → no overrides.
        assert!(effective_virtual_overrides(&store, None).is_empty());
        // Matching env → overrides.
        assert_eq!(
            effective_virtual_overrides(&store, Some("env-dev")).len(),
            1
        );
        // Wrong env → no overrides.
        assert!(effective_virtual_overrides(&store, Some("env-prod")).is_empty());
    }

    #[test]
    fn merge_into_existing_appends_when_no_block() {
        let existing = "127.0.0.1 localhost\n";
        let block = format!(
            "{}\n127.0.0.1 test\n{}\n",
            VERVE_BLOCK_START, VERVE_BLOCK_END
        );
        let merged = merge_into_existing(existing, &block);
        assert!(merged.contains("127.0.0.1 localhost"));
        assert!(merged.contains(VERVE_BLOCK_START));
        assert!(merged.contains("127.0.0.1 test"));
    }

    #[test]
    fn merge_into_existing_replaces_old_block() {
        let existing = format!(
            "127.0.0.1 localhost\n{}\n10.0.0.1 old\n{}\n::1 localhost\n",
            VERVE_BLOCK_START, VERVE_BLOCK_END
        );
        let block = format!("{}\n10.0.0.2 new\n{}\n", VERVE_BLOCK_START, VERVE_BLOCK_END);
        let merged = merge_into_existing(&existing, &block);
        assert!(merged.contains("127.0.0.1 localhost"));
        assert!(merged.contains("::1 localhost"));
        assert!(merged.contains("10.0.0.2 new"));
        assert!(!merged.contains("10.0.0.1 old"));
    }
}
