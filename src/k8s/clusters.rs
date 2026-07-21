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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualCluster {
    pub id: String,
    /// Context / display name.
    pub name: String,
    /// API server URL.
    pub server: String,
    /// Default namespace.
    #[serde(default = "default_namespace")]
    pub namespace: String,
    pub auth: ClusterAuth,
    /// Skip TLS verification.
    #[serde(default)]
    pub insecure_skip_tls: bool,
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
            created_at: now,
        }
    }
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
        paths.join(":")
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
    if let Some(existing) = clusters.iter_mut().find(|c| c.id == cluster.id) {
        *existing = cluster;
    } else {
        clusters.push(cluster);
    }
    save_manual_clusters(&clusters);
    // Rewrite the kubeconfig YAML to include all manual clusters.
    write_verve_kubeconfig(&clusters);
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

/// Import an external kubeconfig file by merging its clusters/contexts/users
/// into the Verve-managed kubeconfig. Returns the number of contexts imported.
pub fn import_kubeconfig(path: &std::path::Path) -> Result<usize, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取 kubeconfig 失败: {e}"))?;
    let external: crate::k8s::config::KubeConfig =
        serde_yaml::from_str(&raw).map_err(|e| format!("解析 kubeconfig 失败: {e}"))?;
    let count = external.contexts.len();

    // Load existing verve kubeconfig and merge.
    let mut existing = load_verve_kubeconfig();
    for cluster in external.clusters {
        if !existing.clusters.iter().any(|c| c.name == cluster.name) {
            existing.clusters.push(cluster);
        }
    }
    for user in external.users {
        if !existing.users.iter().any(|u| u.name == user.name) {
            existing.users.push(user);
        }
    }
    for ctx in external.contexts {
        if !existing.contexts.iter().any(|c| c.name == ctx.name) {
            existing.contexts.push(ctx);
        }
    }
    // If the external config has a current-context and ours is empty, adopt it.
    if existing.current_context.is_empty() && !external.current_context.is_empty() {
        existing.current_context = external.current_context;
    }
    save_verve_kubeconfig(&existing).map_err(|e| format!("写入 kubeconfig 失败: {e}"))?;
    Ok(count)
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

/// Generate a merged KubeConfig from the manual cluster list and write it.
pub fn write_verve_kubeconfig(clusters: &[ManualCluster]) {
    use crate::k8s::config::{
        Cluster, Context as KubeContext, KubeConfig, NamedCluster, NamedContext, NamedUser, UserEntry,
    };

    let mut cfg = load_verve_kubeconfig();
    // Remove all entries that originated from manual clusters (we'll re-add them).
    // We identify them by name being in the manual list.
    let manual_names: std::collections::HashSet<&str> =
        clusters.iter().map(|c| c.name.as_str()).collect();
    cfg.clusters.retain(|c| !manual_names.contains(c.name.as_str()));
    cfg.contexts.retain(|c| !manual_names.contains(c.name.as_str()));
    cfg.users.retain(|u| !manual_names.contains(u.name.as_str()));

    for mc in clusters {
        let cluster_name = format!("{}-cluster", mc.name);
        let user_name = format!("{}-user", mc.name);

        // Build the Cluster entry.
        let cluster = Cluster {
            server: mc.server.clone(),
            insecure_skip_tls_verify: mc.insecure_skip_tls,
            ca_data: None,
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
}
