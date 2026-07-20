//! Kubernetes cluster management.
//!
//! Mirrors the SSH panel's architecture but for K8s clusters: parses
//! `~/.kube/config`, lets the user switch context, and exposes the active
//! cluster's context for downstream operations (kubectl logs / exec /
//! port-forward added in subsequent iterations).
//!
//! This iteration deliberately does NOT pull in the heavy `k8s-openapi` /
//! `kube` crates — we read the kubeconfig YAML manually with a tiny parser to
//! keep the dependency surface small. kubectl is assumed installed for live
//! operations, matching how the SSH shell delegates to the system's SSH client.

pub mod config;
pub mod kubectl;

pub use config::{kubeconfig_path, Cluster, Context as KubeContext, KubeConfig, UserEntry, load_default};
pub use kubectl::{
    exec_pod, list_pods, port_forward, stream_logs, ContainerStatus, ExecSession, LogStream, Pod,
    PodError, PodMeta, PodStatus, PortForwardHandle,
};
