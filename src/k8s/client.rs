//! Native Kubernetes client built on the `kube` crate (kube-rs).
//!
//! Replaces subprocess calls to the external `kubectl` binary. The client is
//! configured directly from a Verve `ManualCluster` (server URL, auth, TLS) —
//! no KUBECONFIG env var or temp YAML files. SSH-tunnel clusters resolve their
//! server URL to the local forwarded port via the existing `ApiTunnel` cache.
//!
//! This module owns the shared tokio runtime that hosts all k8s operations and
//! the mapping from `k8s_openapi` types to the lightweight Verve structs that
//! the UI layer consumes.

use std::sync::OnceLock;

use base64::Engine as _;
use http::Uri;
use kube::{Client, Config};

/// Walk an error's full `.source()` chain and join each layer's `Display`.
///
/// kube/hyper errors nest the real cause (e.g. an `io::Error` like
/// "Connection refused" or a TLS error) several layers deep beneath a generic
/// "ServiceError: client error (Connect)" surface message. Printing only the
/// top-level Display hides the actionable detail, so callers use this to expose
/// the full chain.
pub(crate) fn format_err_chain<E: std::error::Error + ?Sized>(err: &E) -> String {
    let mut parts: Vec<String> = vec![err.to_string()];
    let mut source: Option<&(dyn std::error::Error + 'static)> = err.source();
    while let Some(cause) = source {
        let msg = cause.to_string();
        // Avoid duplicating identical adjacent layers (some wrappers re-print
        // the inner message verbatim).
        if parts.last().is_none_or(|last| !msg.is_empty() && last != &msg) {
            parts.push(msg);
        }
        source = cause.source();
    }
    parts.join(": ")
}

use super::clusters::{ClusterAuth, ManualCluster};
use super::kubectl::{Pod, PodError, find_manual_cluster_by_context, resolve_ssh_tunnel};

// ---------------------------------------------------------------------------
// Shared tokio runtime
// ---------------------------------------------------------------------------

/// A single shared multi-threaded tokio runtime for all Kubernetes operations.
///
/// The `kube::Client` (and its streaming operations: logs/exec/port-forward)
/// must run on a tokio runtime. Sharing one avoids the per-call
/// `Runtime::new().unwrap()` cost the old subprocess path incurred, and keeps
/// connection pooling effective. The SSH-tunnel runtime (inside `ApiTunnel`)
/// stays separate on purpose — opening/dropping it from within another runtime
/// panics, so it is isolated on dedicated std threads.
pub(crate) fn k8s_runtime() -> &'static tokio::runtime::Runtime {
    static K8S_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    K8S_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build shared k8s tokio runtime")
    })
}

// ---------------------------------------------------------------------------
// Client construction
// ---------------------------------------------------------------------------

/// Build a `kube::Client` for the given context.
///
/// - If `context` matches a Verve `ManualCluster`, the client is configured
///   directly from that cluster's server/auth/TLS settings (preserving Verve's
///   auth model rather than relying on `Config::infer()`). SSH-tunnel clusters
///   resolve their server URL to the local forwarded port.
/// - If no Verve cluster matches, fall back to `Config::infer()` so clusters
///   managed purely via `~/.kube/config` still work.
pub(crate) async fn build_client(context: Option<&str>) -> Result<Client, PodError> {
    match context {
        Some(ctx) if !ctx.is_empty() => {
            match find_manual_cluster_by_context(ctx) {
                Some(cluster) => build_client_for_cluster(&cluster).await,
                // Not a Verve-managed cluster (e.g. a context from ~/.kube/config).
                None => build_inferred_client().await,
            }
        }
        _ => build_inferred_client().await,
    }
}

/// Build a client for a system-managed context by inferring from the
/// environment (`~/.kube/config` / `KUBECONFIG` / in-cluster).
async fn build_inferred_client() -> Result<Client, PodError> {
    let config = Config::infer()
        .await
        .map_err(|e| PodError(format!("读取 kubeconfig 失败: {}", format_err_chain(&e))))?;
    Client::try_from(config).map_err(|e| PodError(format!("创建 K8s 客户端失败: {}", format_err_chain(&e))))
}

/// Build a client configured directly from a Verve `ManualCluster`.
async fn build_client_for_cluster(cluster: &ManualCluster) -> Result<Client, PodError> {
    // Resolve the effective server URL. SSH-tunnel clusters point at the local
    // forwarded port (https://127.0.0.1:<port>); direct clusters use the URL as-is.
    let server_url = if cluster.uses_ssh_tunnel() {
        let tunnel = resolve_ssh_tunnel(cluster)?;
        log::info!(
            "k8s client: cluster {:?} via SSH tunnel → {}",
            cluster.name,
            tunnel.local_server_url()
        );
        tunnel.local_server_url()
    } else {
        log::info!(
            "k8s client: cluster {:?} direct → {}",
            cluster.name,
            cluster.server
        );
        cluster.server.clone()
    };

    let uri: Uri = server_url
        .parse()
        .map_err(|e| PodError(format!("无效的 API Server 地址 {server_url}: {e}")))?;

    let mut config = Config::new(uri);
    config.accept_invalid_certs = cluster.insecure_skip_tls;

    // CA certificate (only for imported clusters — extracted from the preserved
    // raw kubeconfig). Cleared when insecure, mirroring the old kubectl path
    // (rustls rejects insecure + custom root combos, same as kubectl does).
    if !cluster.insecure_skip_tls {
        if let Some(ca) = extract_ca(cluster) {
            config.root_cert = Some(ca);
        }
    }

    apply_auth(&mut config, &cluster.auth);

    Client::try_from(config).map_err(|e| PodError(format!("创建 K8s 客户端失败: {}", format_err_chain(&e))))
}

/// Extract the cluster's CA certificate bundle from the preserved raw
/// kubeconfig and parse it into the DER bytes that `Config::root_cert` expects.
///
/// kubeconfig stores `certificate-authority-data` as base64-encoded PEM. We
/// base64-decode it to PEM text, then parse each PEM block into DER. The kube
/// client feeds DER bytes to rustls as trust anchors (`CertificateDer`).
fn extract_ca(cluster: &ManualCluster) -> Option<Vec<Vec<u8>>> {
    let (raw_yaml, ctx) = match (&cluster.raw_kubeconfig, &cluster.original_context) {
        (Some(raw), Some(ctx)) => (raw.as_str(), ctx.as_str()),
        _ => return None,
    };
    // extract_from_raw_kubeconfig returns (server, ns, auth, ca_data, ctx).
    let (_, _, _, ca_data, _) = super::clusters::extract_from_raw_kubeconfig(raw_yaml, ctx)?;
    let ca_b64 = ca_data?;
    // kubeconfig stores certificate-authority-data base64-encoded; decode to PEM.
    let pem_bytes = base64::engine::general_purpose::STANDARD
        .decode(ca_b64.as_bytes())
        .ok()?;
    // Parse each PEM block into DER, keeping only CERTIFICATE entries.
    Some(
        pem::parse_many(pem_bytes)
            .ok()?
            .into_iter()
            .filter(|p| p.tag() == "CERTIFICATE")
            .map(|p| p.into_contents())
            .collect(),
    )
}

/// Apply a Verve `ClusterAuth` to a kube `Config`.
fn apply_auth(config: &mut Config, auth: &ClusterAuth) {
    match auth {
        ClusterAuth::Token { token } => {
            config.auth_info.token = Some(secrecy_010::SecretString::from(token.clone()));
        }
        ClusterAuth::ClientCert { cert_data, key_data } => {
            // kube's AuthInfo expects the SAME base64-encoded PEM strings that a
            // kubeconfig stores — kube does the base64 decode internally
            // (file_config::load_from_base64). Passing decoded PEM here would
            // make kube's decode fail ("invalid symbol 10" = the newline in PEM).
            config.auth_info.client_certificate_data = Some(cert_data.clone());
            config.auth_info.client_key_data = Some(secrecy_010::SecretString::from(key_data.clone()));
        }
        ClusterAuth::BasicAuth { username, password } => {
            config.auth_info.username = Some(username.clone());
            config.auth_info.password = Some(secrecy_010::SecretString::from(password.clone()));
        }
    }
}

// ---------------------------------------------------------------------------
// k8s_openapi → Verve struct mapping
// ---------------------------------------------------------------------------

/// Convert a `k8s_openapi` Pod into Verve's lightweight `Pod` summary.
pub(crate) fn map_pod(p: k8s_openapi::api::core::v1::Pod) -> Pod {
    use super::kubectl::{ContainerStatus, PodMeta, PodStatus};

    let metadata = p.metadata;
    let name = metadata.name.unwrap_or_default();
    let namespace = metadata.namespace.unwrap_or_else(|| "default".to_string());
    let labels = metadata.labels.unwrap_or_default();

    let (phase, container_statuses) = match p.status {
        Some(s) => (
            s.phase.unwrap_or_else(|| "Unknown".to_string()),
            s.container_statuses
                .unwrap_or_default()
                .into_iter()
                .map(|cs| ContainerStatus {
                    name: cs.name,
                    ready: cs.ready,
                    restart_count: cs.restart_count as u32,
                })
                .collect::<Vec<_>>(),
        ),
        None => ("Unknown".to_string(), Vec::new()),
    };

    Pod {
        metadata: PodMeta {
            name,
            namespace,
            labels,
        },
        status: PodStatus {
            phase,
            container_statuses,
        },
    }
}
