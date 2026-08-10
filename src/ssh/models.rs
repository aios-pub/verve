//! SSH host configuration and credential data models.
//!
//! A [`SshHost`] describes everything needed to establish an SSH connection
//! except the secret (password / key passphrase) — that lives in the
//! device-key-encrypted vault (see [`super::credentials`]). This keeps the
//! host list file (`ssh_hosts.json`) safe to share or version-control.

use serde::{Deserialize, Serialize};

use crate::state::models::new_id;

/// Authentication method for an SSH connection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SshAuthType {
    /// Password authentication.
    #[default]
    Password,
    /// Public-key authentication (file path + optional passphrase).
    PrivateKey,
    /// SSH agent forwarding.
    Agent,
    /// Keyboard-interactive authentication — the SSH mechanism used by
    /// MFA/2FA (TOTP / OTP / hardware key). The server sends one or more
    /// prompts (e.g. "Verification code:") and the client must respond with
    /// the user's input. Many servers pair this with a password
    /// ("password + OTP"), so the stored `"password"` secret is sent first.
    #[serde(rename = "keyboard-interactive")]
    KeyboardInteractive,
}

impl SshAuthType {
    pub fn label(&self) -> &'static str {
        match self {
            SshAuthType::Password => "密码",
            SshAuthType::PrivateKey => "密钥",
            SshAuthType::Agent => "Agent",
            SshAuthType::KeyboardInteractive => "键盘交互(MFA)",
        }
    }

    pub const ALL: &'static [SshAuthType] = &[
        SshAuthType::Password,
        SshAuthType::PrivateKey,
        SshAuthType::Agent,
        SshAuthType::KeyboardInteractive,
    ];
}

/// The role of an SSH host in the connection topology.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SshHostType {
    /// A directly reachable host (no bastion needed).
    #[default]
    Direct,
    /// A bastion / jump host that acts as a gateway for internal hosts.
    /// Bastions are connected to directly; their child hosts tunnel through them.
    Bastion,
}

impl SshHostType {
    pub fn label(&self) -> &'static str {
        match self {
            SshHostType::Direct => "直连",
            SshHostType::Bastion => "跳板机",
        }
    }
}

/// One SSH host configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshHost {
    /// Unique id (UUID v4 simple).
    pub id: String,
    /// Display name (e.g. "生产服务器").
    pub name: String,
    /// Hostname or IP address.
    pub host: String,
    /// SSH port (default 22).
    #[serde(default = "default_port")]
    pub port: u16,
    /// SSH username.
    pub username: String,
    /// Authentication method.
    #[serde(default)]
    pub auth_type: SshAuthType,
    /// Path to private key file (PrivateKey auth only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// The role of this host in the topology (direct vs bastion/gateway).
    #[serde(default)]
    pub host_type: SshHostType,
    /// For **child hosts** (host_type=Direct with a bastion): the id of the
    /// bastion host this host tunnels through. For **bastion hosts**: leave
    /// `None`. This creates a parent-child relationship where the bastion is
    /// the gateway and the child connects through it via `direct-tcpip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bastion_id: Option<String>,
    /// Optional override for the address the bastion dials (when the bastion
    /// resolves the target differently). Falls back to `host:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bastion_target_override: Option<String>,
    /// Color tag for visual grouping (hex or theme key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Configured port forwards (local and remote). Active runtime state is
    /// NOT persisted here — this is just the configuration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_forwards: Vec<PortForward>,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
}

/// Direction of an SSH port forward.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardDirection {
    /// Local forward: listen on `local_host:local_port`, forward connections
    /// over SSH to `remote_host:remote_port` (e.g. access a remote DB locally).
    Local,
    /// Remote forward: listen on the *server* at `remote_host:remote_port`,
    /// forward back over SSH to `local_host:local_port` (expose a local
    /// service to the SSH server's network).
    Remote,
}

impl ForwardDirection {
    pub fn label(&self) -> &'static str {
        match self {
            ForwardDirection::Local => "本地转发 (Local)",
            ForwardDirection::Remote => "远程转发 (Remote)",
        }
    }
}

/// A configured port forward rule (persisted on the host record).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortForward {
    /// Unique id (UUID v4 simple).
    pub id: String,
    /// Local vs Remote direction.
    pub direction: ForwardDirection,
    /// Local bind address (usually "127.0.0.1").
    pub local_host: String,
    /// Local port.
    pub local_port: u16,
    /// Remote/target host (reachable from the SSH server).
    pub remote_host: String,
    /// Remote/target port.
    pub remote_port: u16,
    /// Auto-start the forward when the SSH connection opens.
    #[serde(default)]
    pub auto_start: bool,
}

impl PortForward {
    pub fn new() -> Self {
        Self {
            id: new_id(),
            direction: ForwardDirection::Local,
            local_host: "127.0.0.1".into(),
            local_port: 8080,
            remote_host: "127.0.0.1".into(),
            remote_port: 80,
            auto_start: false,
        }
    }
}

impl Default for PortForward {
    fn default() -> Self {
        Self::new()
    }
}

fn default_port() -> u16 {
    22
}

impl SshHost {
    /// Create a new host with sensible defaults.
    pub fn new(name: impl Into<String>, host: impl Into<String>) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            id: new_id(),
            name: name.into(),
            host: host.into(),
            port: 22,
            username: String::new(),
            auth_type: SshAuthType::Password,
            key_path: None,
            host_type: SshHostType::Direct,
            bastion_id: None,
            bastion_target_override: None,
            color: None,
            port_forwards: Vec::new(),
            created_at: now,
        }
    }

    /// Human-readable summary for list display: `username ssh`.
    pub fn address_summary(&self) -> String {
        let user = if self.username.is_empty() {
            "user".to_string()
        } else {
            self.username.clone()
        };
        format!("{} ssh", user)
    }

    /// Whether this host is a bastion/gateway.
    pub fn is_bastion(&self) -> bool {
        self.host_type == SshHostType::Bastion
    }

    /// Whether this host tunnels through a bastion.
    pub fn has_bastion(&self) -> bool {
        self.bastion_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_defaults() {
        let h = SshHost::new("Test", "10.0.0.1");
        assert_eq!(h.port, 22);
        assert_eq!(h.auth_type, SshAuthType::Password);
        assert_eq!(h.host_type, SshHostType::Direct);
        assert!(!h.is_bastion());
        assert!(!h.has_bastion());
    }

    #[test]
    fn address_summary() {
        let h = SshHost::new("Test", "example.com");
        assert_eq!(h.address_summary(), "user ssh");
        let mut h2 = h.clone();
        h2.username = "root".into();
        h2.port = 2222;
        assert_eq!(h2.address_summary(), "root ssh");
    }

    #[test]
    fn bastion_detection() {
        let mut h = SshHost::new("跳板机", "10.0.0.1");
        assert!(!h.is_bastion());
        h.host_type = SshHostType::Bastion;
        assert!(h.is_bastion());
    }

    #[test]
    fn keyboard_interactive_variant() {
        // The MFA auth type must be in ALL (so the UI selector shows it) and
        // have a human-readable label.
        assert!(SshAuthType::ALL.contains(&SshAuthType::KeyboardInteractive));
        assert_eq!(
            SshAuthType::KeyboardInteractive.label(),
            "键盘交互(MFA)"
        );
    }

    #[test]
    fn keyboard_interactive_serialization() {
        // serde tag must round-trip and stay lowercase-kebab so existing host
        // config files don't break and future reads are stable.
        let json = serde_json::to_string(&SshAuthType::KeyboardInteractive).unwrap();
        assert_eq!(json, "\"keyboard-interactive\"");
        let parsed: SshAuthType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SshAuthType::KeyboardInteractive);
    }

    #[test]
    fn child_host_has_bastion() {
        let mut h = SshHost::new("内网主机", "192.168.1.10");
        assert!(!h.has_bastion());
        h.bastion_id = Some("bastion-id".into());
        assert!(h.has_bastion());
    }
}
