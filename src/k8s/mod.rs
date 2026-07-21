//! Kubernetes cluster management.
//!
//! Mirrors the SSH panel's architecture but for K8s clusters: parses
//! `~/.kube/config` and a Verve-managed kubeconfig, lets the user switch
//! context, and exposes the active cluster's context for downstream
//! operations (kubectl logs / exec / port-forward).

pub mod clusters;
pub mod config;
pub mod kubectl;

pub use clusters::{
    import_kubeconfig, load_manual_clusters, load_verve_kubeconfig, merged_kubeconfig_env,
    remove_manual_cluster, save_verve_kubeconfig, upsert_manual_cluster, ClusterAuth,
    ManualCluster,
};
pub use config::{
    kubeconfig_path, load_default, load_merged, Cluster, Context as KubeContext, KubeConfig,
    NamedCluster, NamedContext, NamedUser, UserEntry,
};
pub use kubectl::{
    exec_pod, list_pods, port_forward, stream_logs, ContainerStatus, ExecSession, LogStream, Pod,
    PodError, PodMeta, PodStatus, PortForwardHandle,
};
