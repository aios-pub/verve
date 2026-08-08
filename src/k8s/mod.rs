//! Kubernetes cluster management.
//!
//! Mirrors the SSH panel's architecture but for K8s clusters: parses
//! `~/.kube/config` and a Verve-managed kubeconfig, lets the user switch
//! context, and exposes the active cluster's context for downstream
//! operations (kubectl logs / exec / port-forward).
//!
//! Supports two connection modes per manual cluster:
//! - **Direct** — use the server URL from kubeconfig directly (default, for
//!   clusters with a reachable API server).
//! - **SSH tunnel** — forward the API server port through an SSH jump host
//!   (for clusters that are only reachable from inside a private network via
//!   a bastion). Mirrors the Docker SSH tunnel architecture.

pub mod api_tunnel;
pub mod client;
pub mod clusters;
pub mod config;
pub mod kubectl;

pub use api_tunnel::ApiTunnel;
pub use clusters::{
    ClusterAuth, ClusterConnectionType, KubeconfigContextInfo, ManualCluster,
    cluster_from_kubeconfig, import_kubeconfig, import_kubeconfig_from_string,
    load_manual_clusters, load_verve_kubeconfig, merged_kubeconfig_env, parse_kubeconfig_contexts,
    remove_manual_cluster, save_verve_kubeconfig, upsert_manual_cluster,
};
pub use config::{
    Cluster, Context as KubeContext, KubeConfig, NamedCluster, NamedContext, NamedUser, UserEntry,
    kubeconfig_path, load_default, load_merged, reload_merged_kubeconfig,
};
pub use kubectl::{
    ContainerStatus, ExecSession, LogStream, Pod, PodError, PodMeta, PodStatus, PortForwardHandle,
    close_ssh_tunnel, exec_pod, list_pods, port_forward, stream_logs,
};
