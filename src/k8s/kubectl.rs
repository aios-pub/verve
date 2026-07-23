//! Helpers that shell out to `kubectl`.
//!
//! We deliberately avoid pulling in the kube client crate (heavy); kubectl is
//! assumed installed and uses the user's existing kubeconfig. This module
//! provides:
//!   - `list_pods`        — parse `kubectl get pods -o json`
//!   - `stream_logs`      — spawn `kubectl logs -f` and stream lines to a channel
//!   - `port_forward`     — spawn `kubectl port-forward` in the background
//!
//! Interactive `kubectl exec -it` is reserved for a later iteration (it needs
//! a PTY — the SSH TerminalView can be adapted but is tightly coupled to russh).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

use super::api_tunnel::ApiTunnel;
use super::clusters::{load_manual_clusters, ManualCluster};

/// A pod summary returned by [`list_pods`].
#[derive(Debug, Clone, Deserialize)]
pub struct Pod {
    pub metadata: PodMeta,
    pub status: PodStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PodMeta {
    pub name: String,
    pub namespace: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PodStatus {
    pub phase: String,
    #[serde(rename = "containerStatuses", default)]
    pub container_statuses: Vec<ContainerStatus>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContainerStatus {
    pub name: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(rename = "restartCount", default)]
    pub restart_count: u32,
}

#[derive(Debug, Deserialize)]
struct PodList {
    items: Vec<Pod>,
}

#[derive(Debug, Clone)]
pub struct PodError(pub String);

impl std::fmt::Display for PodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// SSH tunnel management
// ---------------------------------------------------------------------------

/// Global cache of open SSH tunnels for Kubernetes clusters, keyed by
/// **cluster id**. A tunnel is opened on first use for a given cluster and
/// kept alive until the app exits (kubectl opens short-lived connections;
/// re-establishing the SSH transport per call would be too slow).
static TUNNELS: OnceLock<StdMutex<std::collections::HashMap<String, Arc<ApiTunnel>>>> =
    OnceLock::new();

fn tunnels() -> &'static StdMutex<std::collections::HashMap<String, Arc<ApiTunnel>>> {
    TUNNELS.get_or_init(|| StdMutex::new(std::collections::HashMap::new()))
}

/// Close (and drop) the SSH tunnel for a given cluster id, if open.
///
/// The actual drop is offloaded to a dedicated std thread because dropping the
/// `ApiTunnel` tears down its embedded tokio `Runtime`, which panics if invoked
/// from within an asynchronous context (the "Cannot drop a runtime in a context
/// where blocking is not allowed" panic).
pub fn close_ssh_tunnel(cluster_id: &str) {
    // Recover from a poisoned mutex (if a previous open/drop panicked, the data
    // is still structurally fine — we just want to remove the entry).
    let mut guard = tunnels()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(tunnel) = guard.remove(cluster_id) {
        log::info!("k8s tunnel: closed for cluster {cluster_id}");
        defer_tunnel_drop(tunnel);
    }
}

/// Drop an `Arc<ApiTunnel>` on a dedicated std thread so the contained tokio
/// `Runtime` is torn down in a blocking context. Must be used whenever the last
/// Arc reference for a tunnel is released from within an async context.
fn defer_tunnel_drop(tunnel: Arc<ApiTunnel>) {
    std::thread::spawn(move || {
        drop(tunnel);
    });
}

/// Look up the ManualCluster by matching context name. The context name in
/// kubeconfig equals the ManualCluster.name (see clusters.rs::write_verve_kubeconfig).
fn find_manual_cluster_by_context(context: &str) -> Option<ManualCluster> {
    load_manual_clusters()
        .into_iter()
        .find(|c| c.name == context)
}

/// Resolve (opening if necessary) the SSH tunnel for a cluster that uses
/// SshTunnel mode. Returns a reference to the cached `ApiTunnel`.
///
/// **Important:** This may be called from within an existing tokio runtime
/// (e.g. inside async kubectl operations). Since `ApiTunnel::open` creates its
/// own dedicated tokio runtime and calls `block_on`, we must run the open on a
/// dedicated std thread to avoid "Cannot start a runtime from within a runtime"
/// panic (this mirrors how docker tunnels are opened).
///
/// Concurrent callers racing to open the same cluster are serialized by holding
/// the global lock across the entire open — the first one opens, subsequent
/// callers wait and then reuse the cached tunnel.
fn resolve_ssh_tunnel(cluster: &ManualCluster) -> Result<Arc<ApiTunnel>, PodError> {
    let cluster_id = cluster.id.clone();
    let ssh_host_id = cluster
        .ssh_host_id
        .as_ref()
        .ok_or_else(|| PodError("集群配置缺少 ssh_host_id".into()))?;

    // Recover from a poisoned mutex (a prior open/drop may have panicked; the
    // map itself is fine). We still need the lock to serialize concurrent opens.
    let mut guard = tunnels()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Fast path: tunnel already open.
    if let Some(tun) = guard.get(&cluster_id) {
        return Ok(tun.clone());
    }

    // Slow path: open a new tunnel on a dedicated std thread while *holding*
    // the lock, so concurrent callers wait here instead of each opening their
    // own duplicate tunnel.
    let server_url = cluster.server.clone();
    let ssh_host_id_owned = ssh_host_id.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<ApiTunnel, String>>();
    std::thread::spawn(move || {
        let result = match super::api_tunnel::find_ssh_host(&ssh_host_id_owned) {
            Some(ssh_host) => ApiTunnel::open(&ssh_host, &server_url),
            None => Err(format!("找不到关联的 SSH 主机: {ssh_host_id_owned}")),
        };
        let _ = tx.send(result);
    });
    let tunnel = rx
        .recv()
        .map_err(|_| PodError("SSH 隧道创建线程异常退出".into()))?
        .map_err(|e| PodError(format!("建立 SSH 隧道失败: {e}")))?;

    let arc = Arc::new(tunnel);
    guard.insert(cluster_id, arc.clone());
    Ok(arc)
}

/// Generate a one-off kubeconfig YAML that overrides the cluster server URL
/// to the local tunnel endpoint (and optionally enforces insecure-skip-tls-verify).
/// This avoids modifying the persisted Verve kubeconfig — we just set
/// KUBECONFIG to this temp file for kubectl calls over the tunnel.
fn generate_tunnel_kubeconfig(tunnel: &ApiTunnel, cluster: &ManualCluster) -> Result<PathBuf, PodError> {
    use crate::k8s::config::{
        Cluster, Context as KubeContext, KubeConfig, NamedCluster, NamedContext, NamedUser, UserEntry,
    };

    // Load the base (merged) kubeconfig, then patch the target cluster entry.
    let mut cfg = crate::k8s::clusters::load_verve_kubeconfig();

    // The cluster entry is named "<name>-cluster" (see write_verve_kubeconfig).
    let cluster_entry_name = format!("{}-cluster", cluster.name);

    // Patch: override server URL to local tunnel address.
    if let Some(named) = cfg.clusters.iter_mut().find(|c| c.name == cluster_entry_name) {
        named.cluster.server = tunnel.local_server_url();
        // When tunneling to 127.0.0.1 the server cert almost never matches;
        // respect the cluster's insecure_skip_tls flag (default true for SSH
        // tunnel clusters created via via_ssh()).
        named.cluster.insecure_skip_tls_verify = cluster.insecure_skip_tls;
        // MUST clear CA data when insecure is true, otherwise kubectl errors
        if cluster.insecure_skip_tls {
            named.cluster.ca_data = None;
        }
    } else {
        // Fallback: synthesize a minimal cluster entry if not found.
        cfg.clusters.push(NamedCluster {
            name: cluster_entry_name.clone(),
            cluster: Cluster {
                server: tunnel.local_server_url(),
                insecure_skip_tls_verify: cluster.insecure_skip_tls,
                ca_data: None,
                ca_file: None,
            },
        });
        // Make sure a matching context + user exist too.
        let user_name = format!("{}-user", cluster.name);
        if !cfg.contexts.iter().any(|c| c.name == cluster.name) {
            cfg.contexts.push(NamedContext {
                name: cluster.name.clone(),
                context: KubeContext {
                    cluster: cluster_entry_name.clone(),
                    user: Some(user_name.clone()),
                    namespace: if cluster.namespace.is_empty() {
                        None
                    } else {
                        Some(cluster.namespace.clone())
                    },
                },
            });
        }
        if !cfg.users.iter().any(|u| u.name == user_name) {
            let (cert_data, key_data, token, username, password) = match &cluster.auth {
                super::clusters::ClusterAuth::Token { token } => {
                    (None, None, Some(token.clone()), None, None)
                }
                super::clusters::ClusterAuth::ClientCert { cert_data, key_data } => {
                    (Some(cert_data.clone()), Some(key_data.clone()), None, None, None)
                }
                super::clusters::ClusterAuth::BasicAuth { username, password } => {
                    (None, None, None, Some(username.clone()), Some(password.clone()))
                }
            };
            cfg.users.push(NamedUser {
                name: user_name,
                user: UserEntry {
                    cert_data, key_data, token, username, password,
                    cert_file: None, key_file: None,
                },
            });
        }
    }

    // Set current-context so kubectl picks the right cluster without --context.
    cfg.current_context = cluster.name.clone();

    // Write to a temp file. Use deterministic per-cluster filename so kubectl
    // can reuse it across calls (file is overwritten each time).
    let tmp_dir = std::env::temp_dir();
    let short_hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        cluster.id.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };
    let path = tmp_dir.join(format!("vk8s-{}.yaml", &short_hash[..8]));
    let yaml = serde_yaml::to_string(&cfg)
        .map_err(|e| PodError(format!("序列化临时 kubeconfig 失败: {e}")))?;
    std::fs::write(&path, yaml)
        .map_err(|e| PodError(format!("写入临时 kubeconfig 失败: {e}")))?;
    Ok(path)
}

/// Resolved kubectl environment: either standard merged kubeconfig, or a
/// tunnel-specific temp kubeconfig when using SSH tunnel mode.
enum KubeconfigSource {
    Standard,
    Tunnel(PathBuf),
}

/// Build a kubectl Command configured to talk to the given `context`. If the
/// context corresponds to a ManualCluster in SSH-tunnel mode, the tunnel is
/// established (or reused from cache) and KUBECONFIG is pointed at a temp file
/// that overrides the server URL to the local forwarded port.
fn kubectl_command_for_context(context: Option<&str>) -> Result<Command, PodError> {
    let source = match context {
        Some(ctx) if !ctx.is_empty() => {
            if let Some(cluster) = find_manual_cluster_by_context(ctx) {
                if cluster.uses_ssh_tunnel() {
                    let tunnel = resolve_ssh_tunnel(&cluster)?;
                    let kubeconfig_path = generate_tunnel_kubeconfig(&tunnel, &cluster)?;
                    KubeconfigSource::Tunnel(kubeconfig_path)
                } else {
                    KubeconfigSource::Standard
                }
            } else {
                KubeconfigSource::Standard
            }
        }
        _ => KubeconfigSource::Standard,
    };

    let mut cmd = Command::new("kubectl");
    match source {
        KubeconfigSource::Standard => {
            cmd.env("KUBECONFIG", crate::k8s::clusters::merged_kubeconfig_env());
        }
        KubeconfigSource::Tunnel(path) => {
            // When using the tunnel, point KUBECONFIG to our patched temp file.
            // Include the default kubeconfig in the merge so unrelated contexts
            // are still addressable (though the current-context is already set
            // to the tunnel cluster in the patched file).
            let default = crate::k8s::clusters::default_kubeconfig_path();
            let sep = if cfg!(windows) { ";" } else { ":" };
            let mut merged = String::new();
            if default.exists() {
                merged.push_str(&default.to_string_lossy());
                merged.push_str(sep);
            }
            merged.push_str(&path.to_string_lossy());
            cmd.env("KUBECONFIG", merged);
        }
    }
    Ok(cmd)
}

/// Build a `kubectl` Command with the merged KUBECONFIG env var set.
/// (Kept for calls that don't specify a context — uses standard kubeconfig.)
fn kubectl_command() -> Command {
    let mut cmd = Command::new("kubectl");
    cmd.env("KUBECONFIG", crate::k8s::clusters::merged_kubeconfig_env());
    cmd
}

/// List pods across all namespaces (kubectl get pods -A) or in a specific namespace.
/// - `namespace = None` or `Some("")` → list pods across ALL namespaces (-A)
/// - `namespace = Some("ns")` → list only in that namespace
pub async fn list_pods(namespace: Option<&str>, context: Option<&str>) -> Result<Vec<Pod>, PodError> {
    let mut cmd = kubectl_command_for_context(context)?;
    cmd.arg("get").arg("pods").arg("-o").arg("json");
    if let Some(ctx) = context {
        if !ctx.is_empty() {
            cmd.args(["--context", ctx]);
        }
    }
    match namespace {
        Some(ns) if !ns.is_empty() => {
            cmd.args(["-n", ns]);
        }
        _ => {
            // All namespaces
            cmd.arg("-A");
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let out = cmd
        .output()
        .await
        .map_err(|e| PodError(format!("kubectl 执行失败: {e}")))?;
    if !out.status.success() {
        return Err(PodError(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let list: PodList = serde_json::from_slice(&out.stdout)
        .map_err(|e| PodError(format!("解析 pod JSON 失败: {e}")))?;
    Ok(list.items)
}

/// A namespace summary returned by [`list_namespaces`].
#[derive(Debug, Clone, Deserialize)]
pub struct Namespace {
    #[serde(rename = "metadata")]
    pub metadata: NamespaceMeta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NamespaceMeta {
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct NamespaceList {
    items: Vec<Namespace>,
}

/// List all namespaces in the cluster.
pub async fn list_namespaces(context: Option<&str>) -> Result<Vec<String>, PodError> {
    let mut cmd = kubectl_command_for_context(context)?;
    cmd.arg("get").arg("namespaces").arg("-o").arg("json");
    if let Some(ctx) = context {
        if !ctx.is_empty() {
            cmd.args(["--context", ctx]);
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let out = cmd
        .output()
        .await
        .map_err(|e| PodError(format!("kubectl get namespaces 执行失败: {e}")))?;
    if !out.status.success() {
        return Err(PodError(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let list: NamespaceList = serde_json::from_slice(&out.stdout)
        .map_err(|e| PodError(format!("解析 namespace JSON 失败: {e}")))?;
    Ok(list.items.into_iter().map(|ns| ns.metadata.name).collect())
}

/// Handle returned by [`stream_logs`]. Call `take_lines()` to get the channel
/// (the process is kept alive by the `_child` field); drop the handle to kill kubectl.
pub struct LogStream {
    pub lines: Option<mpsc::Receiver<String>>,
    pub child: Child,
}

impl LogStream {
    /// Extract the line receiver (consumes it so it can be moved into a task).
    pub fn take_lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.lines.take()
    }
}

/// Spawn `kubectl logs -f <pod> [-n ns] [-c container] --tail=N` and stream lines.
/// The returned receiver gets one message per log line.
pub fn stream_logs(
    pod: &str,
    namespace: Option<&str>,
    container: Option<&str>,
    tail: u32,
    context: Option<&str>,
) -> Result<LogStream, PodError> {
    let mut cmd = kubectl_command_for_context(context)?;
    cmd.arg("logs")
        .arg("-f")
        .arg("--tail")
        .arg(tail.to_string())
        .arg(pod);
    if let Some(ctx) = context {
        if !ctx.is_empty() {
            cmd.args(["--context", ctx]);
        }
    }
    if let Some(ns) = namespace {
        if !ns.is_empty() {
            cmd.args(["-n", ns]);
        }
    }
    if let Some(c) = container {
        cmd.args(["-c", c]);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| PodError(format!("启动 kubectl logs 失败: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PodError("kubectl 未输出 stdout".into()))?;
    let (tx, rx) = mpsc::channel::<String>(256);

    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });

    Ok(LogStream {
        lines: Some(rx),
        child,
    })
}

/// A running `kubectl port-forward`. Drop the handle to kill the process.
pub struct PortForwardHandle {
    child: Child,
    pub local_port: u16,
    pub target: String,
}

impl PortForwardHandle {
    pub fn stop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for PortForwardHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawn `kubectl port-forward pod/<pod> <local>:<remote>` in the background.
/// If `local_port == 0`, kubectl picks a free port (we parse its stderr to find it).
pub async fn port_forward(
    pod: &str,
    namespace: Option<&str>,
    local_port: u16,
    remote_port: u16,
    context: Option<&str>,
) -> Result<PortForwardHandle, PodError> {
    use tokio::io::AsyncBufReadExt as _;
    use tokio::time::timeout;

    let mut cmd = kubectl_command_for_context(context)?;
    let port_spec = format!("{local_port}:{remote_port}");
    cmd.arg("port-forward")
        .arg(format!("pod/{pod}"))
        .arg(&port_spec);
    if let Some(ctx) = context {
        if !ctx.is_empty() {
            cmd.args(["--context", ctx]);
        }
    }
    if let Some(ns) = namespace {
        if !ns.is_empty() {
            cmd.args(["-n", ns]);
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| PodError(format!("启动 kubectl port-forward 失败: {e}")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PodError("kubectl 未输出 stderr".into()))?;

    // Wait briefly for kubectl to print "Forwarding from 127.0.0.1:<port>".
    let bound_port = if local_port != 0 {
        local_port
    } else {
        let mut reader = BufReader::new(stderr).lines();
        let line = timeout(Duration::from_secs(5), reader.next_line())
            .await
            .map_err(|_| PodError("等待 port-forward 超时".into()))?
            .map_err(|e| PodError(format!("读取 kubectl 输出失败: {e}")))?
            .unwrap_or_default();
        parse_forwarding_port(&line).unwrap_or(local_port)
    };

    Ok(PortForwardHandle {
        child,
        local_port: bound_port,
        target: format!("pod/{pod}:{remote_port}"),
    })
}

fn parse_forwarding_port(line: &str) -> Option<u16> {
    // "Forwarding from 127.0.0.1:12345 -> 5432"
    let colon = line.rfind(':')?;
    let space = line[colon..].find(' ').map(|p| colon + p).unwrap_or(line.len());
    line[colon + 1..space].parse().ok()
}

/// Handle for an interactive `kubectl exec` session running under a PTY.
pub struct ExecSession {
    pub stdin_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

/// Probe whether `shell` exists in the pod by running a non-interactive
/// `<shell> -c exit` and checking for a successful exit. Flags (`-n`, `--context`)
/// are placed BEFORE `--` so kubectl interprets them; the shell command comes after.
/// Returns false on any spawn/wait error or if the probe doesn't finish within 5s.
fn shell_available(pod: &str, namespace: Option<&str>, context: Option<&str>, shell: &str) -> bool {
    use std::process::{Command as StdCommand, Stdio as StdStdio};

    // Resolve kubeconfig (may set up SSH tunnel if needed).
    let kubeconfig = match resolve_kubeconfig_env_for_context_sync(context) {
        Ok(k) => k,
        Err(_) => return false,
    };

    let mut cmd = StdCommand::new("kubectl");
    cmd.env("KUBECONFIG", &kubeconfig);
    cmd.stdout(StdStdio::null());
    cmd.stderr(StdStdio::null());
    cmd.stdin(StdStdio::null());
    cmd.arg("exec");
    if let Some(ctx) = context {
        if !ctx.is_empty() {
            cmd.args(["--context", ctx]);
        }
    }
    if let Some(ns) = namespace {
        if !ns.is_empty() {
            cmd.args(["-n", ns]);
        }
    }
    cmd.arg(pod);
    cmd.arg("--");
    cmd.arg(shell);
    cmd.arg("-c");
    cmd.arg("exit");

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

/// Synchronous (blocking) version of kubeconfig resolution for std Command usage.
/// Returns the KUBECONFIG env var value for the given context, establishing an
/// SSH tunnel and writing the patched kubeconfig if needed.
fn resolve_kubeconfig_env_for_context_sync(context: Option<&str>) -> Result<String, PodError> {
    match context {
        Some(ctx) if !ctx.is_empty() => {
            if let Some(cluster) = find_manual_cluster_by_context(ctx) {
                if cluster.uses_ssh_tunnel() {
                    let tunnel = resolve_ssh_tunnel(&cluster)?;
                    let kubeconfig_path = generate_tunnel_kubeconfig(&tunnel, &cluster)?;
                    let default = crate::k8s::clusters::default_kubeconfig_path();
                    let sep = if cfg!(windows) { ";" } else { ":" };
                    let mut merged = String::new();
                    if default.exists() {
                        merged.push_str(&default.to_string_lossy());
                        merged.push_str(sep);
                    }
                    merged.push_str(&kubeconfig_path.to_string_lossy());
                    return Ok(merged);
                }
            }
            Ok(crate::k8s::clusters::merged_kubeconfig_env())
        }
        _ => Ok(crate::k8s::clusters::merged_kubeconfig_env()),
    }
}

/// Spawn `kubectl exec -it pod [-n ns] [--context ctx] -- shell` attached to a PTY
/// so that Tab completion, arrow keys, and other terminal features work. When no
/// shell is requested, multiple shells are tried in order (bash → sh → dash → ash)
/// to support distroless, Alpine (ash), Debian/Ubuntu minimal (dash), and standard images.
pub fn exec_pod(pod: &str, namespace: Option<&str>, shell: Option<&str>, context: Option<&str>) -> Result<ExecSession, PodError> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};

    // Caller-requested shell wins; otherwise auto-detect from common shells.
    let resolved_shell = match shell {
        Some(s) => s.to_string(),
        None => {
            // Try shells in order of preference.
            let candidates = ["bash", "sh", "dash", "ash"];
            let mut found = None;
            for candidate in candidates {
                if shell_available(pod, namespace, context, candidate) {
                    found = Some(candidate.to_string());
                    break;
                }
            }
            match found {
                Some(s) => s,
                None => return Err(PodError(
                    "该容器中未找到可用的 shell (bash/sh/dash/ash)。可能是极简镜像 (distroless/scratch)。".to_string()
                )),
            }
        }
    };
    let shell = resolved_shell.as_str();

    let kubeconfig = resolve_kubeconfig_env_for_context_sync(context)?;
    let mut cmd = CommandBuilder::new("kubectl");
    cmd.env("KUBECONFIG", &kubeconfig);
    cmd.arg("exec");
    cmd.arg("-it");
    if let Some(ctx) = context {
        if !ctx.is_empty() {
            cmd.arg("--context");
            cmd.arg(ctx);
        }
    }
    if let Some(ns) = namespace {
        if !ns.is_empty() {
            cmd.arg("-n");
            cmd.arg(ns);
        }
    }
    cmd.arg(pod);
    cmd.arg("--");
    cmd.arg(shell);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 32, cols: 120, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| PodError(format!("分配 PTY 失败: {e}")))?;
    let mut _child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| PodError(format!("启动 kubectl exec 失败: {e}")))?;
    drop(pair.slave);

    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| PodError(format!("PTY reader 获取失败: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| PodError(format!("PTY writer 获取失败: {e}")))?;
    drop(pair.master);

    // Writer thread: UI keystrokes → PTY master.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_) => return,
        };
        rt.block_on(async move {
            while let Some(bytes) = stdin_rx.recv().await {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });
    });
    // Reader thread: PTY master → UI TerminalView.
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if output_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(ExecSession { stdin_tx, output_rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_from_kubectl_line() {
        assert_eq!(
            parse_forwarding_port("Forwarding from 127.0.0.1:54321 -> 5432"),
            Some(54321)
        );
        assert_eq!(parse_forwarding_port("listening on 127.0.0.1:3306"), Some(3306));
        assert_eq!(parse_forwarding_port("no port here"), None);
    }

    #[test]
    fn parses_pod_list_json() {
        let json = r#"{
            "items": [
                {
                    "metadata": { "name": "web-abc", "namespace": "default" },
                    "status": { "phase": "Running", "containerStatuses": [{"name":"web","ready":true,"restartCount":2}] }
                },
                {
                    "metadata": { "name": "db-0", "namespace": "default" },
                    "status": { "phase": "Pending" }
                }
            ]
        }"#;
        let list: PodList = serde_json::from_str(json).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.items[0].metadata.name, "web-abc");
        assert_eq!(list.items[0].status.container_statuses[0].restart_count, 2);
        assert_eq!(list.items[1].status.phase, "Pending");
    }
}
