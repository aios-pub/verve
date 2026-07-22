//! Docker host connection configuration.
//!
//! A `DockerHost` describes how to reach a Docker daemon (local or remote).
//! The fields map directly to Docker CLI environment variables:
//!   - `host`          → DOCKER_HOST        (e.g. "tcp://1.2.3.4:2376", "ssh://user@host")
//!   - `tls_verify`    → DOCKER_TLS_VERIFY
//!   - `cert_path`     → DOCKER_CERT_PATH
//!   - `ssh_host_id`   → when set, tunnel through a Verve-managed SSH host
//!                       (credentials come from Verve's keychain/vault)

use serde::{Deserialize, Serialize};

use crate::state::models::new_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerHost {
    /// Unique id (UUID v4 simple).
    pub id: String,
    /// Display name, e.g. "生产环境 Docker".
    pub name: String,
    /// DOCKER_HOST value, e.g. "tcp://1.2.3.4:2376", "ssh://user@host", "unix:///var/run/docker.sock".
    /// Empty when `ssh_host_id` is set (tunnel provides the unix:// endpoint dynamically).
    pub host: String,
    /// Whether to enable TLS verification (DOCKER_TLS_VERIFY=1).
    #[serde(default)]
    pub tls_verify: bool,
    /// Directory containing ca.pem/cert.pem/key.pem (DOCKER_CERT_PATH).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_path: Option<String>,
    /// If set, references a Verve SSH host id — docker calls will be tunneled
    /// through an SSH unix-socket forward to the remote `/var/run/docker.sock`,
    /// using Verve's stored credentials (no system `ssh` needed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host_id: Option<String>,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
}

impl DockerHost {
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            id: new_id(),
            name: name.into(),
            host: host.into(),
            tls_verify: false,
            cert_path: None,
            ssh_host_id: None,
            created_at: now,
        }
    }

    /// Create a Docker host that tunnels through an existing Verve SSH host.
    pub fn via_ssh(name: impl Into<String>, ssh_host_id: impl Into<String>) -> Self {
        let mut s = Self::new(name, "");
        s.ssh_host_id = Some(ssh_host_id.into());
        s
    }

    /// Whether this host connects via Verve's SSH tunnel.
    pub fn uses_ssh_tunnel(&self) -> bool {
        self.ssh_host_id.is_some()
    }

    /// Synthetic "local" entry — represents the default local Docker daemon
    /// (no env vars set). Never persisted to disk.
    pub fn local() -> Self {
        Self {
            id: "local".into(),
            name: "本地 Docker".into(),
            host: String::new(),
            tls_verify: false,
            cert_path: None,
            ssh_host_id: None,
            created_at: 0,
        }
    }

    /// Whether this is the synthetic local entry.
    pub fn is_local(&self) -> bool {
        self.id == "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_entry_has_empty_host() {
        let l = DockerHost::local();
        assert!(l.is_local());
        assert!(l.host.is_empty());
    }

    #[test]
    fn new_sets_defaults() {
        let h = DockerHost::new("test", "tcp://1.2.3.4:2375");
        assert!(!h.id.is_empty());
        assert!(!h.tls_verify);
        assert!(h.cert_path.is_none());
    }
}
