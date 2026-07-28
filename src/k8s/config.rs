//! Minimal kubeconfig reader.
//!
//! Reads `~/.kube/config` (or `$KUBECONFIG`) and extracts clusters, contexts,
//! users, and the current context. We do not depend on the full `kube` crate —
//! this is just enough to drive the UI panel; live cluster operations shell out
//! to `kubectl` (expected to be on PATH).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct KubeConfig {
    #[serde(default)]
    pub clusters: Vec<NamedCluster>,
    #[serde(default)]
    pub contexts: Vec<NamedContext>,
    #[serde(default)]
    pub users: Vec<NamedUser>,
    #[serde(rename = "current-context", default)]
    pub current_context: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NamedCluster {
    pub name: String,
    pub cluster: Cluster,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Cluster {
    pub server: String,
    #[serde(rename = "insecure-skip-tls-verify", default)]
    pub insecure_skip_tls_verify: bool,
    #[serde(
        rename = "certificate-authority-data",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ca_data: Option<String>,
    #[serde(
        rename = "certificate-authority",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ca_file: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NamedContext {
    pub name: String,
    pub context: Context,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Context {
    pub cluster: String,
    pub user: Option<String>,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NamedUser {
    pub name: String,
    #[serde(default)]
    pub user: UserEntry,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UserEntry {
    #[serde(
        rename = "client-certificate-data",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cert_data: Option<String>,
    #[serde(
        rename = "client-key-data",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub key_data: Option<String>,
    #[serde(
        rename = "client-certificate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cert_file: Option<String>,
    #[serde(
        rename = "client-key",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub key_file: Option<String>,
    #[serde(rename = "token", default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(rename = "username", default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(rename = "password", default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Locate the kubeconfig path: $KUBECONFIG if set, else ~/.kube/config.
pub fn kubeconfig_path() -> PathBuf {
    if let Ok(p) = std::env::var("KUBECONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".kube").join("config");
    }
    PathBuf::from(".kube/config")
}

/// Load and parse the default kubeconfig. Returns an empty (default) config on
/// IO / parse failure so the UI can show a friendly "kubectl not configured"
/// message.
pub fn load_default() -> KubeConfig {
    let path = kubeconfig_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_yaml::from_str(&raw).unwrap_or_default(),
        Err(_) => KubeConfig::default(),
    }
}

/// Load and merge kubeconfigs: the system default (~/.kube/config) and the
/// Verve-managed file (~/.verve/kube_config.yaml). The Verve entries take
/// priority on name collisions (clusters/contexts/users with the same name
/// from the Verve file override the system ones).
pub fn load_merged() -> KubeConfig {
    use crate::k8s::clusters::verve_kubeconfig_path;

    let mut merged = load_default();

    // Load the Verve-managed kubeconfig if present.
    let verve_path = verve_kubeconfig_path();
    if let Ok(raw) = std::fs::read_to_string(&verve_path) {
        if let Ok(verve_cfg) = serde_yaml::from_str::<KubeConfig>(&raw) {
            // Merge clusters: verve wins on name collision.
            for c in verve_cfg.clusters {
                if let Some(pos) = merged.clusters.iter().position(|x| x.name == c.name) {
                    merged.clusters[pos] = c;
                } else {
                    merged.clusters.push(c);
                }
            }
            // Merge users.
            for u in verve_cfg.users {
                if let Some(pos) = merged.users.iter().position(|x| x.name == u.name) {
                    merged.users[pos] = u;
                } else {
                    merged.users.push(u);
                }
            }
            // Merge contexts.
            for ctx in verve_cfg.contexts {
                if let Some(pos) = merged.contexts.iter().position(|x| x.name == ctx.name) {
                    merged.contexts[pos] = ctx;
                } else {
                    merged.contexts.push(ctx);
                }
            }
            // Keep the current-context from verve if set, otherwise the default one.
            if !verve_cfg.current_context.is_empty() {
                merged.current_context = verve_cfg.current_context;
            }
        }
    }
    merged
}

/// Regenerate the Verve-managed kubeconfig from the manual-cluster index, then
/// return the freshly merged kubeconfig.
///
/// This is the read path the UI should use when it wants to reflect the current
/// state of the world. The manual-cluster index (`kube_clusters.json`) is the
/// source of truth for Verve-added clusters; the YAML (`kube_config.yaml`) is a
/// derived artifact that can drift (e.g. after a rename/delete outside the
/// normal mutation path, or a hand-edit). Regenerating before reading keeps
/// `load_merged` from returning stale entries — which otherwise makes the
/// "刷新" button look like a no-op and hides edit/delete buttons (they match
/// contexts to clusters by name).
pub fn reload_merged_kubeconfig() -> KubeConfig {
    // Regenerate the YAML from the authoritative index first. Best-effort: if
    // the index is empty/missing we still fall through to load_merged() so the
    // system kubeconfig (~/.kube/config) is still shown.
    let clusters = crate::k8s::clusters::load_manual_clusters();
    crate::k8s::clusters::write_verve_kubeconfig(&clusters);
    load_merged()
}

impl KubeConfig {
    /// Look up the currently-active context (NamedContext), if any.
    pub fn active_context(&self) -> Option<&NamedContext> {
        self.contexts
            .iter()
            .find(|c| c.name == self.current_context)
    }

    /// Look up the cluster for a given context name.
    pub fn cluster_for(&self, ctx_name: &str) -> Option<&Cluster> {
        let ctx = self.contexts.iter().find(|c| c.name == ctx_name)?;
        self.clusters
            .iter()
            .find(|cl| cl.name == ctx.context.cluster)
            .map(|cl| &cl.cluster)
    }

    /// Look up the user for a given context name.
    pub fn user_for(&self, ctx_name: &str) -> Option<&NamedUser> {
        let ctx = self.contexts.iter().find(|c| c.name == ctx_name)?;
        let user_name = ctx.context.user.as_deref()?;
        self.users.iter().find(|u| u.name == user_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
apiVersion: v1
kind: Config
current-context: dev-cluster
clusters:
- cluster:
    server: https://dev.example.com:6443
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
    username: admin
"#;

    #[test]
    fn parses_clusters_contexts_users() {
        let cfg: KubeConfig = serde_yaml::from_str(SAMPLE).unwrap();
        assert_eq!(cfg.current_context, "dev-cluster");
        assert_eq!(cfg.clusters.len(), 2);
        assert_eq!(cfg.contexts.len(), 2);
        assert_eq!(cfg.users.len(), 2);
        assert_eq!(
            cfg.cluster_for("prod-cluster").unwrap().server,
            "https://prod.example.com:6443"
        );
        assert!(
            cfg.cluster_for("prod-cluster")
                .unwrap()
                .insecure_skip_tls_verify
        );
        assert_eq!(
            cfg.active_context().unwrap().context.namespace.as_deref(),
            Some("default")
        );
        assert_eq!(
            cfg.user_for("dev-cluster").unwrap().user.token.as_deref(),
            Some("abcdef")
        );
    }

    /// Regression test for the k8s "刷新无效 / context 无法编辑删除" bug.
    ///
    /// The Verve-managed kubeconfig (`kube_config.yaml`) is a derived artifact
    /// that can drift out of sync with the authoritative manual-cluster index
    /// (`kube_clusters.json`). When it does, a plain `load_merged()` keeps
    /// returning the stale entries, so the "刷新" button appears to do nothing
    /// and edit/delete buttons (which match contexts to clusters by name)
    /// disappear. `reload_merged_kubeconfig` must regenerate the YAML from the
    /// index before reading so the UI reflects the real state.
    #[test]
    fn reload_merged_kubeconfig_repairs_drifted_yaml() {
        use crate::state::persistence::set_thread_data_dir;

        // Point the data dir (verve kubeconfig + cluster index) at a throwaway
        // directory so nothing real is touched. We intentionally do NOT touch
        // HOME: the system kubeconfig (~/.kube/config) may or may not exist, but
        // the assertions below only concern Verve-managed (stale vs. real)
        // entries, which are independent of whatever the system file contains.
        let tmp = std::env::temp_dir().join(format!(
            "verve-k8s-merge-test-{}",
            crate::state::models::new_id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = set_thread_data_dir(tmp.clone());

        // Seed an authoritative manual cluster named "my-cluster".
        let cluster =
            crate::k8s::clusters::ManualCluster::new("my-cluster", "https://1.2.3.4:6443");
        crate::k8s::clusters::save_manual_clusters(&[cluster.clone()]);
        crate::k8s::clusters::write_verve_kubeconfig(&[cluster]);

        // Simulate drift: hand-write a STALE kube_config.yaml whose context
        // names no longer match any cluster in the index (e.g. left over from a
        // rename). This is exactly the on-disk state that caused the bug.
        let stale = r#"
apiVersion: v1
kind: Config
current-context: old-renamed-cluster
clusters:
- cluster:
    server: https://9.9.9.9:6443
  name: old-renamed-cluster-cluster
contexts:
- context:
    cluster: old-renamed-cluster-cluster
    user: old-renamed-cluster-user
  name: old-renamed-cluster
users:
- name: old-renamed-cluster-user
  user:
    token: stale
"#;
        let verve_path = crate::k8s::clusters::verve_kubeconfig_path();
        std::fs::write(&verve_path, stale).unwrap();

        // load_merged() alone returns the stale context (the bug): the stale
        // entry is present and the real cluster is absent.
        let buggy = load_merged();
        let buggy_names: Vec<&str> = buggy.contexts.iter().map(|c| c.name.as_str()).collect();
        assert!(
            buggy_names.contains(&"old-renamed-cluster"),
            "precondition: stale entry must be visible via load_merged before the fix"
        );
        assert!(
            !buggy_names.contains(&"my-cluster"),
            "precondition: real cluster missing from drifted YAML"
        );

        // reload_merged_kubeconfig() regenerates the YAML then reads: the stale
        // entry is gone and the real cluster is present.
        let fixed = reload_merged_kubeconfig();
        let fixed_names: Vec<&str> = fixed.contexts.iter().map(|c| c.name.as_str()).collect();
        assert!(
            fixed_names.contains(&"my-cluster"),
            "after reload the real cluster must be visible (so edit/delete buttons render)"
        );
        assert!(
            !fixed_names.contains(&"old-renamed-cluster"),
            "after reload stale entries must be gone"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
