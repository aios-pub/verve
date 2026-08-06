//! Persistence for SSH host configurations (`~/.verve/ssh_hosts.json`).
//!
//! Only non-secret data is stored here. Passwords and key passphrases live in
//! the device-key-encrypted vault (see [`super::credentials`]).

use std::fs;

use super::models::SshHost;

/// `~/.verve/ssh_hosts.json`.
fn hosts_path() -> Option<std::path::PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("ssh_hosts.json"))
}

/// Load all SSH hosts. Returns an empty vec on any error.
pub fn load_hosts() -> Vec<SshHost> {
    let Some(path) = hosts_path() else {
        return Vec::new();
    };
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str::<Vec<SshHost>>(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Persist all SSH hosts atomically (write `.tmp` then rename).
pub fn save_hosts(hosts: &[SshHost]) {
    let Some(path) = hosts_path() else {
        log::error!("ssh_hosts.json path unavailable");
        return;
    };
    let Ok(json) = serde_json::to_string_pretty(hosts) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = fs::write(&tmp, json) {
        log::error!("write ssh_hosts.json: {e}");
        return;
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        log::error!("rename ssh_hosts.json: {e}");
    }
}

/// Insert or replace a host (matched by id).
pub fn upsert_host(host: SshHost) -> Vec<SshHost> {
    let mut hosts = load_hosts();
    if let Some(existing) = hosts.iter_mut().find(|h| h.id == host.id) {
        *existing = host;
    } else {
        hosts.push(host);
    }
    save_hosts(&hosts);
    hosts
}

/// Remove a host by id. Also deletes its stored credentials.
pub fn remove_host(id: &str) -> Vec<SshHost> {
    // Best-effort credential cleanup.
    let _ = super::credentials::delete_secret(id, "password");
    let _ = super::credentials::delete_secret(id, "key_passphrase");

    let mut hosts = load_hosts();
    hosts.retain(|h| h.id != id);
    save_hosts(&hosts);
    // Clear dangling bastion references on child hosts.
    let mut changed = false;
    for h in &mut hosts {
        if h.bastion_id.as_deref() == Some(id) {
            h.bastion_id = None;
            changed = true;
        }
    }
    if changed {
        save_hosts(&hosts);
    }
    hosts
}

/// Find a host by id.
pub fn find_host(id: &str) -> Option<SshHost> {
    load_hosts().into_iter().find(|h| h.id == id)
}

/// Get the full jump chain (from outermost bastion to the target host).
/// Returns an error if a cycle is detected or a referenced host is missing.
pub fn jump_chain(host: &SshHost) -> Result<Vec<SshHost>, String> {
    let mut chain = vec![host.clone()];
    let mut current = host.clone();
    let mut visited = std::collections::HashSet::new();
    visited.insert(host.id.clone());

    while let Some(bastion_id) = &current.bastion_id {
        if !visited.insert(bastion_id.clone()) {
            return Err(format!("跳板机循环检测：{}", bastion_id));
        }
        let bastion =
            find_host(bastion_id).ok_or_else(|| format!("找不到跳板机配置：{}", bastion_id))?;
        chain.insert(0, bastion.clone());
        current = bastion;
        if chain.len() > 8 {
            return Err("跳板机层级超过最大深度（8）".into());
        }
    }
    Ok(chain)
}

/// Get all child hosts that tunnel through a given bastion id.
pub fn children_of_bastion(bastion_id: &str) -> Vec<SshHost> {
    load_hosts()
        .into_iter()
        .filter(|h| h.bastion_id.as_deref() == Some(bastion_id))
        .collect()
}

/// Get all bastion hosts (host_type == Bastion).
pub fn bastions() -> Vec<SshHost> {
    load_hosts()
        .into_iter()
        .filter(|h| h.is_bastion())
        .collect()
}

/// Get all standalone direct hosts (not a bastion, no bastion parent).
pub fn standalone_hosts() -> Vec<SshHost> {
    load_hosts()
        .into_iter()
        .filter(|h| !h.is_bastion() && !h.has_bastion())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::models::new_id;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let tmp = std::env::temp_dir().join(format!("verve-ssh-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // Point the persistence layer at a unique per-thread dir instead of
        // mutating the process-global HOME env var. The thread-local override
        // keeps parallel tests isolated with no mutex and no cross-module race.
        let _guard = crate::state::persistence::set_thread_data_dir(tmp.clone());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn round_trip_via_temp_dir() {
        with_temp_home(|| {
            let mut h = SshHost::new("Test", "10.0.0.1");
            h.username = "root".into();
            let saved = upsert_host(h.clone());
            assert_eq!(saved.len(), 1);

            let loaded = load_hosts();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].name, "Test");
            assert_eq!(loaded[0].username, "root");

            let after = remove_host(&h.id);
            assert!(after.is_empty());
        });
    }

    #[test]
    fn jump_chain_direct() {
        with_temp_home(|| {
            let target = SshHost::new("Target", "10.0.0.2");
            let target_id = target.id.clone();
            upsert_host(target);
            let chain = jump_chain(&find_host(&target_id).unwrap()).unwrap();
            assert_eq!(chain.len(), 1);
        });
    }

    #[test]
    fn jump_chain_with_bastion() {
        with_temp_home(|| {
            let mut bastion = SshHost::new("Bastion", "10.0.0.1");
            bastion.host_type = crate::ssh::models::SshHostType::Bastion;
            let bastion_id = bastion.id.clone();
            upsert_host(bastion);

            let mut target = SshHost::new("Target", "10.0.0.2");
            target.bastion_id = Some(bastion_id.clone());
            let target_id = target.id.clone();
            upsert_host(target);

            let chain = jump_chain(&find_host(&target_id).unwrap()).unwrap();
            assert_eq!(chain.len(), 2);
            assert_eq!(chain[0].name, "Bastion");
            assert_eq!(chain[1].name, "Target");
        });
    }
}
