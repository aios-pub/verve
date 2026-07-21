//! Persistence for Docker host configurations (`~/.verve/docker_hosts.json`).
//!
//! Mirrors `crate::ssh::host_store`. Only connection metadata is stored;
//! there are no secrets for Docker (env vars carry all the info needed).

use std::fs;

use super::models::DockerHost;

/// `~/.verve/docker_hosts.json`.
fn hosts_path() -> Option<std::path::PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("docker_hosts.json"))
}

/// `~/.verve/docker_active.json` — stores the id of the active host.
fn active_path() -> Option<std::path::PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("docker_active.json"))
}

/// Load all Docker hosts. The synthetic "local" entry is always prepended.
pub fn load_hosts() -> Vec<DockerHost> {
    let mut out = vec![DockerHost::local()];
    let Some(path) = hosts_path() else {
        return out;
    };
    if let Ok(s) = fs::read_to_string(&path) {
        if let Ok(stored) = serde_json::from_str::<Vec<DockerHost>>(&s) {
            out.extend(stored);
        }
    }
    out
}

/// Persist hosts atomically (write `.tmp` then rename). The "local" sentinel
/// is filtered out before saving.
pub fn save_hosts(hosts: &[DockerHost]) {
    let Some(path) = hosts_path() else {
        log::error!("docker_hosts.json path unavailable");
        return;
    };
    let to_save: Vec<&DockerHost> = hosts.iter().filter(|h| !h.is_local()).collect();
    let Ok(json) = serde_json::to_string_pretty(&to_save) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = fs::write(&tmp, json) {
        log::error!("write docker_hosts.json: {e}");
        return;
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        log::error!("rename docker_hosts.json: {e}");
    }
}

/// Return the id of the currently active host (defaults to "local").
pub fn get_active_id() -> String {
    let Some(path) = active_path() else {
        return "local".into();
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("active").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| "local".into())
}

/// Persist the active host id.
pub fn set_active_id(id: &str) {
    let Some(path) = active_path() else {
        return;
    };
    let json = serde_json::json!({ "active": id }).to_string();
    let tmp = path.with_extension("json.tmp");
    let _ = fs::write(&tmp, &json);
    let _ = fs::rename(&tmp, &path);
}

/// Insert or replace a host (matched by id).
pub fn upsert_host(host: DockerHost) -> Vec<DockerHost> {
    let mut all = load_hosts();
    // load_hosts prepends local — replace the matching entry (skip local).
    if let Some(existing) = all.iter_mut().find(|h| h.id == host.id && !h.is_local()) {
        *existing = host;
    } else if !host.is_local() {
        all.push(host);
    }
    save_hosts(&all);
    all
}

/// Remove a host by id. Returns the updated list.
pub fn remove_host(id: &str) -> Vec<DockerHost> {
    if id == "local" {
        return load_hosts();
    }
    let mut all = load_hosts();
    all.retain(|h| h.id != id);
    save_hosts(&all);
    // If we removed the active host, fall back to local.
    if get_active_id() == id {
        set_active_id("local");
    }
    all
}

/// Find a host by id.
pub fn find_host(id: &str) -> Option<DockerHost> {
    load_hosts().into_iter().find(|h| h.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::models::new_id;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let tmp = std::env::temp_dir().join(format!("verve-docker-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &tmp);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn round_trip() {
        with_temp_home(|| {
            let h = DockerHost::new("Test Docker", "tcp://10.0.0.1:2375");
            upsert_host(h.clone());

            let loaded = load_hosts();
            assert_eq!(loaded.len(), 2); // local + the new one
            assert!(loaded[0].is_local());
            assert_eq!(loaded[1].name, "Test Docker");

            // Default active is local.
            assert_eq!(get_active_id(), "local");

            set_active_id(&h.id);
            assert_eq!(get_active_id(), h.id);

            remove_host(&h.id);
            let after = load_hosts();
            assert_eq!(after.len(), 1);
            assert!(after[0].is_local());
            assert_eq!(get_active_id(), "local"); // fell back
        });
    }
}
