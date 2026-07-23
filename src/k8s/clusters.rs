//! Manual Kubernetes cluster management.
//!
//! Verve maintains its own kubeconfig file at `~/.verve/kube_config.yaml` and
//! a JSON index of manually-added clusters at `~/.verve/kube_clusters.json`.
//! kubectl is invoked with `KUBECONFIG` set to the merged path
//! (`$HOME/.kube/config:~/.verve/kube_config.yaml`) so both the user's existing
//! configuration and Verve-added clusters are visible without modifying the
//! system kubeconfig.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::models::new_id;

/// Connection mode for a Kubernetes cluster.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClusterConnectionType {
    /// Direct connection: use kubeconfig server URL directly (default).
    #[default]
    Direct,
    /// SSH tunnel: forward Kubernetes API server port through an SSH jump host.
    /// The local kubeconfig server is rewritten to 127.0.0.1:auto-port.
    SshTunnel,
}

impl ClusterConnectionType {
    pub fn label(&self) -> &'static str {
        match self {
            ClusterConnectionType::Direct => "直连",
            ClusterConnectionType::SshTunnel => "SSH 隧道",
        }
    }

    pub const ALL: &'static [ClusterConnectionType] = &[
        ClusterConnectionType::Direct,
        ClusterConnectionType::SshTunnel,
    ];
}

/// Authentication method for a manually-added cluster entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClusterAuth {
    /// Bearer token authentication.
    Token { token: String },
    /// Client certificate + key (base64-encoded PEM data).
    ClientCert {
        cert_data: String,
        key_data: String,
    },
    /// Username + password (HTTP basic auth).
    BasicAuth { username: String, password: String },
}

/// A manually-added Kubernetes cluster (stored in kube_clusters.json).
///
/// Can be created two ways:
/// 1. Import from kubeconfig file - raw config is preserved, connection mode can be switched freely
/// 2. Manual entry - server URL + auth credentials are stored directly
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualCluster {
    pub id: String,
    /// Display name (user-editable).
    pub name: String,
    /// API server URL (from kubeconfig or manual entry).
    /// - In Direct mode: the actual server URL
    /// - In SshTunnel mode: the remote API server address reachable from the SSH host
    pub server: String,
    /// Default namespace.
    #[serde(default = "default_namespace")]
    pub namespace: String,
    pub auth: ClusterAuth,
    /// Skip ALL TLS verification: CA validation, hostname matching.
    /// Equivalent to kubectl --insecure-skip-tls-verify=true
    #[serde(default)]
    pub insecure_skip_tls: bool,
    /// Connection mode: direct or via SSH tunnel.
    #[serde(default)]
    pub connection_type: ClusterConnectionType,
    /// When connection_type = SshTunnel: the Verve SSH host id to tunnel through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host_id: Option<String>,
    /// Optional: raw kubeconfig YAML this cluster was imported from.
    /// When present, preserves CA data and other config; connection mode is
    /// applied at runtime without modifying the original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_kubeconfig: Option<String>,
    /// Original context name from imported kubeconfig (for reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_context: Option<String>,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
}

fn default_namespace() -> String {
    "default".into()
}

impl ManualCluster {
    pub fn new(name: impl Into<String>, server: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            id: new_id(),
            name: name.into(),
            server: server.into(),
            namespace: "default".into(),
            auth: ClusterAuth::Token {
                token: String::new(),
            },
            insecure_skip_tls: false,
            connection_type: ClusterConnectionType::Direct,
            ssh_host_id: None,
            raw_kubeconfig: None,
            original_context: None,
            created_at: now,
        }
    }

    /// Create a cluster imported from kubeconfig.
    pub fn from_kubeconfig(
        name: impl Into<String>,
        server: impl Into<String>,
        namespace: impl Into<String>,
        auth: ClusterAuth,
        raw_kubeconfig: impl Into<String>,
        original_context: impl Into<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            id: new_id(),
            name: name.into(),
            server: server.into(),
            namespace: namespace.into(),
            auth,
            insecure_skip_tls: false,
            connection_type: ClusterConnectionType::Direct,
            ssh_host_id: None,
            raw_kubeconfig: Some(raw_kubeconfig.into()),
            original_context: Some(original_context.into()),
            created_at: now,
        }
    }

    /// Create a cluster that connects through an SSH tunnel.
    pub fn via_ssh(
        name: impl Into<String>,
        remote_server: impl Into<String>,
        ssh_host_id: impl Into<String>,
    ) -> Self {
        let mut s = Self::new(name, remote_server);
        s.connection_type = ClusterConnectionType::SshTunnel;
        s.ssh_host_id = Some(ssh_host_id.into());
        // Tunneling to 127.0.0.1 commonly causes TLS hostname mismatch;
        // default insecure_skip_tls to true for tunnel mode (can be toggled off).
        s.insecure_skip_tls = true;
        s
    }

    /// Whether this cluster connects via an SSH tunnel.
    pub fn uses_ssh_tunnel(&self) -> bool {
        self.connection_type == ClusterConnectionType::SshTunnel
    }

    /// Switch connection mode between Direct and SshTunnel.
    /// When switching to tunnel, insecure_skip_tls defaults to true.
    pub fn set_connection_type(&mut self, conn_type: ClusterConnectionType) {
        self.connection_type = conn_type;
        if conn_type == ClusterConnectionType::SshTunnel {
            self.insecure_skip_tls = true;
        }
    }
}

/// A parsed context from a kubeconfig file, for import selection.
#[derive(Debug, Clone)]
pub struct KubeconfigContextInfo {
    pub name: String,
    pub cluster: String,
    pub server: String,
    pub user: String,
    pub namespace: Option<String>,
    pub has_ca: bool,
    pub has_token: bool,
    pub has_client_cert: bool,
}

/// Preprocess kubeconfig YAML to repair common copy-paste issues:
/// Long base64 data lines that were hard-wrapped without proper indentation
/// (common when copying from terminals/web pages that auto-wrap long lines).
///
/// Lines that start at column 0 (no leading whitespace), don't contain a colon,
/// don't start with '-', and are long base64 strings are considered continuation
/// lines and appended directly to the previous line to rejoin wrapped data.
fn repair_wrapped_kubeconfig(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut result: Vec<String> = Vec::new();

    for line in lines {
        // Detect continuation lines:
        // - No leading whitespace (properly indented YAML lines are not continuations)
        // - Does not start with '-' (YAML list item)
        // - Does not contain ':' (not a YAML key/value pair)
        // - Length >= 30 characters (avoids false positives)
        let looks_like_continuation = !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.starts_with('#')
            && !line.starts_with('-')
            && !line.contains(':')
            && line.len() >= 30;

        if !result.is_empty() && looks_like_continuation {
            // Append directly to previous line (rejoin the wrapped base64 string)
            if let Some(last) = result.last_mut() {
                last.push_str(line);
                continue;
            }
        }
        result.push(line.to_string());
    }

    result.join("\n")
}

/// Parse a kubeconfig YAML string and return a list of available contexts
/// with summary info, for the import dialog to display.
pub fn parse_kubeconfig_contexts(raw_yaml: &str) -> Result<Vec<KubeconfigContextInfo>, String> {
    let repaired = repair_wrapped_kubeconfig(raw_yaml);
    let kcfg: crate::k8s::config::KubeConfig =
        serde_yaml::from_str(&repaired).map_err(|e| format!("解析 kubeconfig 失败: {e}"))?;

    let mut results = Vec::new();
    for ctx in &kcfg.contexts {
        let cluster = match kcfg.clusters.iter().find(|c| c.name == ctx.context.cluster) {
            Some(c) => c,
            None => continue,
        };
        let user = match ctx.context.user.as_deref().and_then(|uname| {
            kcfg.users.iter().find(|u| u.name == uname)
        }) {
            Some(u) => u,
            None => continue,
        };

        results.push(KubeconfigContextInfo {
            name: ctx.name.clone(),
            cluster: cluster.name.clone(),
            server: cluster.cluster.server.clone(),
            user: user.name.clone(),
            namespace: ctx.context.namespace.clone(),
            has_ca: cluster.cluster.ca_data.is_some() || cluster.cluster.ca_file.is_some(),
            has_token: user.user.token.is_some(),
            has_client_cert: user.user.cert_data.is_some() || user.user.cert_file.is_some(),
        });
    }

    Ok(results)
}

/// Create a ManualCluster from imported kubeconfig YAML for a specific context.
pub fn cluster_from_kubeconfig(
    display_name: impl Into<String>,
    raw_yaml: impl Into<String>,
    context_name: &str,
) -> Result<ManualCluster, String> {
    let raw = repair_wrapped_kubeconfig(&raw_yaml.into());
    let (server, namespace, auth, _ca, _orig_ctx) = extract_from_raw_kubeconfig(&raw, context_name)
        .ok_or_else(|| format!("在 kubeconfig 中找不到 context: {context_name}"))?;

    let mc = ManualCluster::from_kubeconfig(
        display_name,
        &server,
        &namespace,
        auth,
        raw,
        context_name,
    );
    Ok(mc)
}

/// Path to the Verve-managed kubeconfig YAML.
pub fn verve_kubeconfig_path() -> PathBuf {
    crate::state::persistence::data_dir()
        .map(|d| d.join("kube_config.yaml"))
        .unwrap_or_else(|_| PathBuf::from("kube_config.yaml"))
}

/// Path to the manual cluster index JSON.
fn clusters_path() -> Option<PathBuf> {
    crate::state::persistence::data_dir()
        .ok()
        .map(|d| d.join("kube_clusters.json"))
}

/// Path to the default kubeconfig (~/.kube/config or $KUBECONFIG).
pub fn default_kubeconfig_path() -> PathBuf {
    crate::k8s::config::kubeconfig_path()
}

/// Return the merged KUBECONFIG value (colon-separated paths) passed to kubectl.
/// kubectl automatically merges multiple kubeconfigs listed in KUBECONFIG.
pub fn merged_kubeconfig_env() -> String {
    let mut paths = Vec::new();
    let default = default_kubeconfig_path();
    if default.exists() {
        paths.push(default.to_string_lossy().to_string());
    }
    let verve = verve_kubeconfig_path();
    if verve.exists() {
        paths.push(verve.to_string_lossy().to_string());
    }
    if paths.is_empty() {
        // Fall back to the default path so kubectl shows a clear error.
        default.to_string_lossy().to_string()
    } else {
        // KUBECONFIG uses the platform's PATH separator: ":" on Unix, ";" on Windows.
        let sep = if cfg!(windows) { ";" } else { ":" };
        paths.join(sep)
    }
}

/// Load the list of manually-added clusters.
pub fn load_manual_clusters() -> Vec<ManualCluster> {
    let Some(path) = clusters_path() else {
        return Vec::new();
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the manual cluster list atomically.
pub fn save_manual_clusters(clusters: &[ManualCluster]) {
    let Some(path) = clusters_path() else {
        return;
    };
    let Ok(json) = serde_json::to_string_pretty(clusters) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    let _ = fs::write(&tmp, json);
    let _ = fs::rename(&tmp, &path);
}

/// Insert or update a manual cluster (by id).
pub fn upsert_manual_cluster(cluster: ManualCluster) -> Vec<ManualCluster> {
    let mut clusters = load_manual_clusters();
    let is_new = clusters.iter().find(|c| c.id == cluster.id).is_none();
    if let Some(existing) = clusters.iter_mut().find(|c| c.id == cluster.id) {
        *existing = cluster.clone();
    } else {
        clusters.push(cluster.clone());
    }
    save_manual_clusters(&clusters);
    // Rewrite the kubeconfig YAML to include all manual clusters.
    write_verve_kubeconfig(&clusters);

    // If this is a new cluster and there's no current-context set (empty),
    // automatically switch to the newly added cluster for a better first-run UX.
    if is_new {
        let mut cfg = load_verve_kubeconfig();
        if cfg.current_context.is_empty() {
            cfg.current_context = cluster.name.clone();
            let _ = save_verve_kubeconfig(&cfg);
        }
    }

    clusters
}

/// Remove a manual cluster by id.
pub fn remove_manual_cluster(id: &str) -> Vec<ManualCluster> {
    let mut clusters = load_manual_clusters();
    clusters.retain(|c| c.id != id);
    save_manual_clusters(&clusters);
    write_verve_kubeconfig(&clusters);
    clusters
}

/// Import an external kubeconfig file.
///
/// All contexts found in the file are converted into ManualCluster entities
/// (so they can be edited/deleted individually like manually-added clusters).
/// Existing clusters with the same name are updated (overwritten), not duplicated.
/// Returns the number of contexts imported.
pub fn import_kubeconfig(path: &std::path::Path) -> Result<usize, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取 kubeconfig 失败: {e}"))?;
    import_kubeconfig_from_string(&raw)
}

/// Import kubeconfig from a YAML string (shared between file import and paste).
pub fn import_kubeconfig_from_string(raw: &str) -> Result<usize, String> {
    let repaired = repair_wrapped_kubeconfig(raw);
    let external: crate::k8s::config::KubeConfig =
        serde_yaml::from_str(&repaired).map_err(|e| format!("解析 kubeconfig 失败: {e}"))?;
    let count = external.contexts.len();
    let mut existing_clusters = load_manual_clusters();
    let mut imported = 0;

    // For each context in the external kubeconfig, create/update a ManualCluster.
    // We store the full raw kubeconfig in each entity so switching connection mode works later.
    for ctx in &external.contexts {
        match cluster_from_kubeconfig(&ctx.name, &repaired, &ctx.name) {
            Ok(mut new_cluster) => {
                // Check if we already have a cluster with this name; if so update it (overwrite),
                // preserving connection_type/ssh_host_id if the user previously configured them.
                if let Some(existing) = existing_clusters.iter_mut().find(|c| c.name == new_cluster.name) {
                    // Preserve user-configured connection settings if they were set
                    if existing.connection_type != ClusterConnectionType::Direct {
                        new_cluster.connection_type = existing.connection_type;
                        new_cluster.ssh_host_id = existing.ssh_host_id.clone();
                    }
                    // Close any open SSH tunnel for this cluster
                    crate::k8s::kubectl::close_ssh_tunnel(&existing.id);
                    new_cluster.id = existing.id.clone();
                    new_cluster.created_at = existing.created_at;
                    *existing = new_cluster;
                } else {
                    existing_clusters.push(new_cluster);
                }
                imported += 1;
            }
            Err(e) => {
                log::warn!("Skipping context {} during import: {e}", ctx.name);
            }
        }
    }

    if imported == 0 {
        return Err("未找到可导入的有效 context".into());
    }

    save_manual_clusters(&existing_clusters);
    write_verve_kubeconfig(&existing_clusters);

    // Set current context to the external current-context if we don't have one
    if !external.current_context.is_empty() {
        let mut cfg = load_verve_kubeconfig();
        if cfg.current_context.is_empty() {
            cfg.current_context = external.current_context;
            let _ = save_verve_kubeconfig(&cfg);
        }
    }

    Ok(imported)
}

/// Load the Verve-managed kubeconfig (empty if file missing).
pub fn load_verve_kubeconfig() -> crate::k8s::config::KubeConfig {
    let path = verve_kubeconfig_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_yaml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Atomically save a KubeConfig to the Verve-managed path.
pub fn save_verve_kubeconfig(cfg: &crate::k8s::config::KubeConfig) -> Result<(), String> {
    let path = verve_kubeconfig_path();
    let yaml = serde_yaml::to_string(cfg).map_err(|e| format!("序列化 kubeconfig 失败: {e}"))?;
    let tmp = path.with_extension("yaml.tmp");
    fs::write(&tmp, &yaml).map_err(|e| format!("写入 kubeconfig 失败: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("写入 kubeconfig 失败: {e}"))?;
    Ok(())
}

/// Parse an imported raw kubeconfig YAML and extract the cluster/context/user
/// for a specific context name, returning the extracted auth and cluster info.
fn extract_from_raw_kubeconfig(
    raw_yaml: &str,
    context_name: &str,
) -> Option<(String, String, ClusterAuth, Option<String>, String)> {
    let repaired = repair_wrapped_kubeconfig(raw_yaml);
    let kcfg: crate::k8s::config::KubeConfig = serde_yaml::from_str(&repaired).ok()?;

    // Find the named context
    let ctx = kcfg.contexts.iter().find(|c| c.name == context_name)?;
    let cluster_name = &ctx.context.cluster;
    let user_name = ctx.context.user.as_deref()?;

    // Find the cluster
    let cluster = kcfg.clusters.iter().find(|c| &c.name == cluster_name)?;

    // Find the user
    let named_user = kcfg.users.iter().find(|u| &u.name == user_name)?;

    // Extract auth from user entry
    let auth = if let Some(token) = &named_user.user.token {
        ClusterAuth::Token { token: token.clone() }
    } else if named_user.user.cert_data.is_some() || named_user.user.key_data.is_some() {
        ClusterAuth::ClientCert {
            cert_data: named_user.user.cert_data.clone().unwrap_or_default(),
            key_data: named_user.user.key_data.clone().unwrap_or_default(),
        }
    } else if named_user.user.username.is_some() || named_user.user.password.is_some() {
        ClusterAuth::BasicAuth {
            username: named_user.user.username.clone().unwrap_or_default(),
            password: named_user.user.password.clone().unwrap_or_default(),
        }
    } else {
        // Default to empty token (user can fill later)
        ClusterAuth::Token { token: String::new() }
    };

    let namespace = ctx.context.namespace.clone().unwrap_or_else(|| "default".into());

    Some((
        cluster.cluster.server.clone(),
        namespace,
        auth,
        cluster.cluster.ca_data.clone(),
        context_name.into(),
    ))
}

/// Generate a merged KubeConfig from the manual cluster list and write it.
pub fn write_verve_kubeconfig(clusters: &[ManualCluster]) {
    use crate::k8s::config::{
        Cluster, Context as KubeContext, KubeConfig, NamedCluster, NamedContext, NamedUser, UserEntry,
    };

    let mut cfg = load_verve_kubeconfig();
    // Remove all entries that originated from manual clusters (we'll re-add them).
    // Context names match cluster display names; clusters are "{name}-cluster", users "{name}-user".
    let manual_ctx_names: std::collections::HashSet<&str> =
        clusters.iter().map(|c| c.name.as_str()).collect();
    let manual_cluster_names: std::collections::HashSet<String> =
        clusters.iter().map(|c| format!("{}-cluster", c.name)).collect();
    let manual_user_names: std::collections::HashSet<String> =
        clusters.iter().map(|c| format!("{}-user", c.name)).collect();
    cfg.clusters.retain(|c| !manual_cluster_names.contains(&c.name));
    cfg.contexts.retain(|c| !manual_ctx_names.contains(c.name.as_str()));
    cfg.users.retain(|u| !manual_user_names.contains(&u.name));

    for mc in clusters {
        let cluster_name = format!("{}-cluster", mc.name);
        let user_name = format!("{}-user", mc.name);

        // Extract CA data from raw kubeconfig if available
        let ca_data = if let Some(raw) = &mc.raw_kubeconfig {
            if let Some(orig_ctx) = &mc.original_context {
                extract_from_raw_kubeconfig(raw, orig_ctx)
                    .and_then(|(_, _, _, ca, _)| ca)
            } else {
                None
            }
        } else {
            None
        };

        // Build the Cluster entry.
        // IMPORTANT: when insecure_skip_tls_verify is true, CA data MUST be cleared
        // (kubectl errors with "specifying a root certificates file with the insecure flag is not allowed")
        let cluster = Cluster {
            server: mc.server.clone(),
            insecure_skip_tls_verify: mc.insecure_skip_tls,
            ca_data: if mc.insecure_skip_tls { None } else { ca_data },
            ca_file: None,
        };
        cfg.clusters.push(NamedCluster {
            name: cluster_name.clone(),
            cluster,
        });

        // Build the User entry based on auth type.
        let (cert_data, key_data, token, username, password) = match &mc.auth {
            ClusterAuth::Token { token } => {
                (None, None, Some(token.clone()), None, None)
            }
            ClusterAuth::ClientCert {
                cert_data,
                key_data,
            } => (
                Some(cert_data.clone()),
                Some(key_data.clone()),
                None,
                None,
                None,
            ),
            ClusterAuth::BasicAuth { username, password } => {
                (None, None, None, Some(username.clone()), Some(password.clone()))
            }
        };
        let user = UserEntry {
            cert_data,
            key_data,
            token,
            username,
            password,
            cert_file: None,
            key_file: None,
        };
        cfg.users.push(NamedUser {
            name: user_name.clone(),
            user,
        });

        // Build the Context entry.
        cfg.contexts.push(NamedContext {
            name: mc.name.clone(),
            context: KubeContext {
                cluster: cluster_name,
                user: Some(user_name),
                namespace: if mc.namespace.is_empty() {
                    None
                } else {
                    Some(mc.namespace.clone())
                },
            },
        });
    }

    let _ = save_verve_kubeconfig(&cfg);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home<F: FnOnce()>(f: F) {
        use crate::state::models::new_id;
        let tmp = std::env::temp_dir().join(format!("verve-k8s-test-{}", new_id()));
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
    fn manual_cluster_round_trip() {
        with_temp_home(|| {
            let mut c = ManualCluster::new("test-cluster", "https://1.2.3.4:6443");
            c.namespace = "kube-system".into();
            c.auth = ClusterAuth::Token {
                token: "secret-token".into(),
            };
            upsert_manual_cluster(c.clone());

            let loaded = load_manual_clusters();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].name, "test-cluster");
            assert_eq!(loaded[0].namespace, "kube-system");

            // Check kubeconfig was written
            let kcfg = load_verve_kubeconfig();
            assert!(kcfg.clusters.iter().any(|cl| cl.name == "test-cluster-cluster"));
            assert!(kcfg.contexts.iter().any(|ctx| ctx.name == "test-cluster"));

            remove_manual_cluster(&c.id);
            assert!(load_manual_clusters().is_empty());
        });
    }

    #[test]
    fn ssh_tunnel_cluster_defaults() {
        // via_ssh() should set SshTunnel connection type, ssh_host_id, and
        // default insecure_skip_tls to true (common case for 127.0.0.1 cert mismatch).
        let c = ManualCluster::via_ssh(
            "private-cluster",
            "https://127.0.0.1:6443",
            "ssh-host-123",
        );
        assert_eq!(c.name, "private-cluster");
        assert_eq!(c.server, "https://127.0.0.1:6443");
        assert!(c.uses_ssh_tunnel());
        assert_eq!(c.connection_type, ClusterConnectionType::SshTunnel);
        assert_eq!(c.ssh_host_id.as_deref(), Some("ssh-host-123"));
        assert!(c.insecure_skip_tls);
    }

    #[test]
    fn direct_cluster_does_not_use_tunnel() {
        let c = ManualCluster::new("direct", "https://k8s.example.com:6443");
        assert!(!c.uses_ssh_tunnel());
        assert_eq!(c.connection_type, ClusterConnectionType::Direct);
        assert!(c.ssh_host_id.is_none());
        assert!(!c.insecure_skip_tls);
    }

    #[test]
    fn cluster_serialization_preserves_tunnel_fields() {
        with_temp_home(|| {
            let mut c = ManualCluster::via_ssh(
                "tunnel-cluster",
                "https://10.0.0.1:6443",
                "bastion-ssh-id",
            );
            c.insecure_skip_tls = false; // user explicitly disabled
            upsert_manual_cluster(c.clone());

            let loaded = load_manual_clusters();
            assert_eq!(loaded.len(), 1);
            assert!(loaded[0].uses_ssh_tunnel());
            assert_eq!(loaded[0].ssh_host_id.as_deref(), Some("bastion-ssh-id"));
            assert!(!loaded[0].insecure_skip_tls);
        });
    }

    #[test]
    fn parses_kubeconfig_contexts() {
        let sample = r#"
apiVersion: v1
kind: Config
current-context: dev-cluster
clusters:
- cluster:
    server: https://dev.example.com:6443
    certificate-authority-data: LS0tLS1CRUdJTi...
  name: dev-cluster
- cluster:
    server: https://prod.example.com:6443
    insecure-skip-tls-verify: true
  name: prod-cluster
contexts:
- context:
    cluster: dev-cluster
    user: dev-user
    namespace: default
  name: dev-cluster
- context:
    cluster: prod-cluster
    user: prod-user
    namespace: kube-system
  name: prod-cluster
users:
- name: dev-user
  user:
    token: abcdef
- name: prod-user
  user:
    client-certificate-data: Y2VydC1kYXRh
    client-key-data: a2V5LWRhdGE=
"#;
        let contexts = parse_kubeconfig_contexts(sample).unwrap();
        assert_eq!(contexts.len(), 2);

        assert_eq!(contexts[0].name, "dev-cluster");
        assert_eq!(contexts[0].server, "https://dev.example.com:6443");
        assert!(contexts[0].has_ca);
        assert!(contexts[0].has_token);
        assert!(!contexts[0].has_client_cert);

        assert_eq!(contexts[1].name, "prod-cluster");
        assert_eq!(contexts[1].server, "https://prod.example.com:6443");
        assert!(!contexts[1].has_ca);
        assert!(!contexts[1].has_token);
        assert!(contexts[1].has_client_cert);
    }

    #[test]
    fn creates_cluster_from_kubeconfig() {
        let sample = r#"
apiVersion: v1
kind: Config
clusters:
- cluster:
    server: https://test.example.com:6443
    certificate-authority-data: Q0EtREFUQQ==
  name: test-cluster
contexts:
- context:
    cluster: test-cluster
    user: test-user
    namespace: kube-system
  name: test-context
users:
- name: test-user
  user:
    token: my-secret-token
"#;
        let mc = cluster_from_kubeconfig("我的测试集群", sample, "test-context").unwrap();
        assert_eq!(mc.name, "我的测试集群");
        assert_eq!(mc.server, "https://test.example.com:6443");
        assert_eq!(mc.namespace, "kube-system");
        assert!(matches!(mc.auth, ClusterAuth::Token { ref token } if token == "my-secret-token"));
        assert!(mc.raw_kubeconfig.is_some());
        assert_eq!(mc.original_context.as_deref(), Some("test-context"));
        assert!(!mc.uses_ssh_tunnel());
    }

    #[test]
    fn write_kubeconfig_no_duplicates_on_multiple_saves() {
        with_temp_home(|| {
            // Create and upsert a cluster
            let mut c1 = ManualCluster::new("my-cluster", "https://example.com:6443");
            c1.auth = ClusterAuth::Token { token: "token1".into() };
            upsert_manual_cluster(c1.clone());

            // Modify token and upsert again
            let mut c2 = c1.clone();
            c2.auth = ClusterAuth::Token { token: "updated-token".into() };
            upsert_manual_cluster(c2);

            // Load resulting kubeconfig and verify no duplicates
            let kcfg = load_verve_kubeconfig();

            // Count cluster entries
            let cluster_count = kcfg.clusters.iter()
                .filter(|c| c.name == "my-cluster-cluster")
                .count();
            assert_eq!(cluster_count, 1, "should have exactly one cluster entry, got {cluster_count}");

            // Count user entries
            let user_count = kcfg.users.iter()
                .filter(|u| u.name == "my-cluster-user")
                .count();
            assert_eq!(user_count, 1, "should have exactly one user entry, got {user_count}");

            // Count context entries
            let ctx_count = kcfg.contexts.iter()
                .filter(|c| c.name == "my-cluster")
                .count();
            assert_eq!(ctx_count, 1, "should have exactly one context entry, got {ctx_count}");
        });
    }

    #[test]
    fn repairs_hard_wrapped_kubeconfig() {
        // Simulate a kubeconfig with long base64 lines hard-wrapped without proper indentation
        // (typical when copying from terminal output that auto-wraps lines)
        let broken = r#"apiVersion: v1
clusters:
- cluster:
    certificate-authority-data: Q0EtREFUQV9QUk9GSUNJRU5UX0RhdGFfVEhBVF9JU19WRVJZX0xPTkdfQU5EX1dBU19IYXJk
d3JhcHBlZF9ieV90aGVfb3V0cHV0X2J1ZmZlcl93aGVuX2NvcHlpbmdfZnJvbV9hX3Rlcm1pbmFs
    server: https://127.0.0.1:6443
  name: default
contexts:
- context:
    cluster: default
    user: default
  name: default
current-context: default
kind: Config
users:
- name: default
  user:
    client-certificate-data: Q0VSVElGSUNBVEVfREFUQV9W RVJZX0xPTkdfQU5EX1dBU19IYXJk d3JhcHBlZF9ieV90aGVfb3V0cHV0
    client-key-data: Q0xJRU5UX0tFWV9EQVRBX1ZFUllfTE9OR19BTkRfV0FTX0hhcmR3cmFwcGVk
"#;

        let contexts = parse_kubeconfig_contexts(broken).unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].name, "default");
        assert_eq!(contexts[0].server, "https://127.0.0.1:6443");
        assert!(contexts[0].has_ca, "CA data should be parsed after repair");
        assert!(contexts[0].has_client_cert, "client cert should be parsed after repair");
    }

    #[test]
    fn parses_user_provided_kubeconfig_file() {
        // Test against the user's actual file if it exists (development only)
        let path = "/Users/lijing/Downloads/kubeconfig-98.yaml";
        if let Ok(content) = std::fs::read_to_string(path) {
            let contexts = parse_kubeconfig_contexts(&content).expect("user kubeconfig should parse");
            assert!(!contexts.is_empty());
        }
    }

    #[test]
    fn import_kubeconfig_creates_manual_clusters() {
        with_temp_home(|| {
            let kubeconfig = r#"
apiVersion: v1
kind: Config
clusters:
- cluster:
    server: https://prod.example.com:6443
    certificate-authority-data: Q0EtREFUQQ==
  name: prod-cluster
- cluster:
    server: https://staging.example.com:6443
  name: staging-cluster
contexts:
- context:
    cluster: prod-cluster
    user: prod-user
    namespace: production
  name: prod
- context:
    cluster: staging-cluster
    user: staging-user
    namespace: staging
  name: staging
current-context: prod
users:
- name: prod-user
  user:
    token: prod-token
- name: staging-user
  user:
    token: staging-token
"#;
            // Import creates manual clusters
            let count = import_kubeconfig_from_string(kubeconfig).unwrap();
            assert_eq!(count, 2);

            // Should appear as manual clusters (editable/deletable)
            let clusters = load_manual_clusters();
            assert_eq!(clusters.len(), 2);
            assert!(clusters.iter().any(|c| c.name == "prod"));
            assert!(clusters.iter().any(|c| c.name == "staging"));
            assert!(clusters.iter().find(|c| c.name == "prod").unwrap().raw_kubeconfig.is_some());

            // Re-importing should update (not duplicate)
            let count2 = import_kubeconfig_from_string(kubeconfig).unwrap();
            assert_eq!(count2, 2);
            let clusters2 = load_manual_clusters();
            assert_eq!(clusters2.len(), 2, "re-import should not create duplicates");

            // Current context should be set
            let kcfg = load_verve_kubeconfig();
            assert_eq!(kcfg.current_context, "prod");
        });
    }
}
