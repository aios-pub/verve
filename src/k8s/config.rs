//! Minimal kubeconfig reader.
//!
//! Reads `~/.kube/config` (or `$KUBECONFIG`) and extracts clusters, contexts,
//! users, and the current context. We do not depend on the full `kube` crate —
//! this is just enough to drive the UI panel; live cluster operations shell out
//! to `kubectl` (expected to be on PATH).

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
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

#[derive(Debug, Clone, Deserialize)]
pub struct NamedCluster {
    pub name: String,
    pub cluster: Cluster,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cluster {
    pub server: String,
    #[serde(rename = "insecure-skip-tls-verify", default)]
    pub insecure_skip_tls_verify: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NamedContext {
    pub name: String,
    pub context: Context,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Context {
    pub cluster: String,
    pub user: Option<String>,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NamedUser {
    pub name: String,
    #[serde(default)]
    pub user: UserEntry,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserEntry {
    #[serde(rename = "client-certificate-data", default)]
    pub cert_data: Option<String>,
    #[serde(rename = "client-key-data", default)]
    pub key_data: Option<String>,
    #[serde(rename = "token", default)]
    pub token: Option<String>,
    #[serde(rename = "username", default)]
    pub username: Option<String>,
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

impl KubeConfig {
    /// Look up the currently-active context (NamedContext), if any.
    pub fn active_context(&self) -> Option<&NamedContext> {
        self.contexts.iter().find(|c| c.name == self.current_context)
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
        assert!(cfg.cluster_for("prod-cluster").unwrap().insecure_skip_tls_verify);
        assert_eq!(
            cfg.active_context().unwrap().context.namespace.as_deref(),
            Some("default")
        );
        assert_eq!(cfg.user_for("dev-cluster").unwrap().user.token.as_deref(), Some("abcdef"));
    }
}
