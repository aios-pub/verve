//! Kubernetes operations over the native `kube` client (kube-rs).
//!
//! This module exposes the public K8s surface (`list_pods`, `stream_logs`,
//! `port_forward`, `exec_pod`) implemented as direct API-server calls via the
//! `kube` crate — no dependency on an external `kubectl` binary. Client
//! construction and the shared tokio runtime live in [`super::client`].
//!
//! SSH-tunnel clusters reuse the existing `ApiTunnel` cache to forward the API
//! server port; the client simply points at the local forwarded endpoint.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::mpsc;

use super::api_tunnel::ApiTunnel;
use super::clusters::{ManualCluster, load_manual_clusters};

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
pub(crate) fn find_manual_cluster_by_context(context: &str) -> Option<ManualCluster> {
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
pub(crate) fn resolve_ssh_tunnel(cluster: &ManualCluster) -> Result<Arc<ApiTunnel>, PodError> {
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

/// List pods across all namespaces or in a specific namespace.
/// - `namespace = None` or `Some("")` → list pods across ALL namespaces
/// - `namespace = Some("ns")` → list only in that namespace
pub async fn list_pods(
    namespace: Option<&str>,
    context: Option<&str>,
) -> Result<Vec<Pod>, PodError> {
    use kube::Api;
    use kube::api::ListParams;

    let client = super::client::build_client(context).await?;
    let pods: Api<k8s_openapi::api::core::v1::Pod> = match namespace {
        Some(ns) if !ns.is_empty() => Api::namespaced(client, ns),
        _ => Api::all(client),
    };
    let list = pods
        .list(&ListParams::default())
        .await
        .map_err(|e| PodError(format!("获取 Pod 列表失败: {}", super::client::format_err_chain(&e))))?;
    Ok(list.items.into_iter().map(super::client::map_pod).collect())
}

/// List all namespaces in the cluster.
pub async fn list_namespaces(context: Option<&str>) -> Result<Vec<String>, PodError> {
    use kube::Api;
    use kube::api::ListParams;

    let client = super::client::build_client(context).await?;
    let namespaces: Api<k8s_openapi::api::core::v1::Namespace> = Api::all(client);
    let list = namespaces
        .list(&ListParams::default())
        .await
        .map_err(|e| PodError(format!("获取 Namespace 列表失败: {}", super::client::format_err_chain(&e))))?;
    Ok(list
        .items
        .into_iter()
        .filter_map(|ns| ns.metadata.name)
        .collect())
}

/// Handle returned by [`stream_logs`]. Call `take_lines()` to get the channel
/// (the background log-reading task is kept alive by the `task` field); dropping
/// the handle aborts the task and stops the follow stream.
pub struct LogStream {
    pub lines: Option<mpsc::Receiver<String>>,
    /// Holds the spawned log-reading task so it lives as long as the handle.
    /// Dropping the handle aborts the task.
    pub task: Option<tokio::task::JoinHandle<()>>,
}

impl LogStream {
    /// Extract the line receiver (consumes it so it can be moved into a task).
    pub fn take_lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.lines.take()
    }
}

/// Stream pod logs (`follow` + `tail`) over the API server, forwarding each
/// line to the returned channel.
///
/// This is a **synchronous** entry point (the UI calls it from a std thread
/// running a fresh tokio runtime). The follow stream is opened on the shared
/// k8s runtime and read by a spawned task; the handle keeps that task alive.
pub fn stream_logs(
    pod: &str,
    namespace: Option<&str>,
    container: Option<&str>,
    tail: u32,
    context: Option<&str>,
) -> Result<LogStream, PodError> {
    use kube::Api;
    use kube::api::LogParams;

    // Resolve the client and open the follow stream synchronously by blocking
    // on the shared k8s runtime. The runtime is multi-threaded, so blocking a
    // std thread here does not stall other k8s operations.
    let pod_name = pod.to_string();
    let ns = namespace.unwrap_or("default").to_string();
    let container = container.map(|s| s.to_string());
    let tail = tail.max(1);

    let (lines_tx, lines_rx) = mpsc::channel::<String>(256);

    let rt = super::client::k8s_runtime();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<Result<(), PodError>>();

    let context_owned = context.map(|s| s.to_string());
    let pod_name_for_task = pod_name.clone();
    let ns_for_task = ns.clone();
    let container_for_task = container.clone();

    let handle = rt.spawn(async move {
        let client = match super::client::build_client(context_owned.as_deref()).await {
            Ok(c) => c,
            Err(e) => {
                let _ = started_tx.send(Err(e));
                return;
            }
        };
        let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client, &ns_for_task);

        let mut lp = LogParams {
            follow: true,
            tail_lines: Some(tail as i64),
            ..Default::default()
        };
        if let Some(c) = &container_for_task {
            lp.container = Some(c.clone());
        }

        match pods.log_stream(&pod_name_for_task, &lp).await {
            Ok(mut reader) => {
                // Signal success so the synchronous caller can return.
                let _ = started_tx.send(Ok(()));
                // log_stream returns a futures::AsyncBufRead; read it line by line.
                use futures::AsyncBufReadExt as _;
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match reader.read_line(&mut buf).await {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            // Strip the trailing newline to match the old per-line behavior.
                            let line = buf.strip_suffix('\n').unwrap_or(&buf).to_string();
                            if lines_tx.send(line).await.is_err() {
                                break; // receiver dropped → stop streaming
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(e) => {
                let _ = started_tx.send(Err(PodError(format!("获取日志失败: {}", super::client::format_err_chain(&e)))));
            }
        }
    });

    // Wait for the stream to be established (or fail) before returning.
    match started_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            handle.abort();
            return Err(e);
        }
        Err(_) => {
            handle.abort();
            return Err(PodError("日志流线程异常退出".into()));
        }
    }

    Ok(LogStream {
        lines: Some(lines_rx),
        task: Some(handle),
    })
}

/// A running port-forward to a pod. Drop the handle to stop forwarding.
pub struct PortForwardHandle {
    /// Abort guard for the accept loop task; dropping the handle tears it down.
    pub task: Option<tokio::task::JoinHandle<()>>,
    pub local_port: u16,
    pub target: String,
}

impl PortForwardHandle {
    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for PortForwardHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Forward a local TCP port to a pod's port over the API server (replaces
/// `kubectl port-forward`). If `local_port == 0`, the OS assigns a free port.
///
/// A local TCP listener is bound on the shared k8s runtime. For each accepted
/// connection a fresh port-forwarder is opened to the pod and the connection is
/// bridged to the forwarder's data stream (the kube port-forward model provides
/// one stream per forwarder, so we open one forwarder per connection). The
/// returned handle keeps the listener alive; dropping it stops forwarding.
pub async fn port_forward(
    pod: &str,
    namespace: Option<&str>,
    local_port: u16,
    remote_port: u16,
    context: Option<&str>,
) -> Result<PortForwardHandle, PodError> {
    use kube::Api;
    use tokio::io::AsyncReadExt as _;
    use tokio::io::AsyncWriteExt as _;

    let client = super::client::build_client(context).await?;
    let ns = namespace.unwrap_or("default").to_string();
    let pod = pod.to_string();
    let target = format!("pod/{pod}:{remote_port}");

    // Bind a local TCP listener (OS-assigned port when local_port == 0).
    let bind = format!("127.0.0.1:{local_port}");
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|e| PodError(format!("本地监听失败 {bind}: {e}")))?;
    let bound_port = listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| PodError(format!("获取本地端口失败: {e}")))?;

    let task = tokio::spawn(async move {
        loop {
            // Accept loop: each connection opens its own port-forwarder.
            let (local_stream, _peer) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("k8s port-forward accept error: {e}");
                    break;
                }
            };
            let client = client.clone();
            let ns = ns.clone();
            let pod_clone = pod.clone();
            tokio::spawn(async move {
                let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client, &ns);
                // Open a per-connection port-forwarder and take its single data stream.
                let mut pf = match pods.portforward(&pod_clone, &[remote_port]).await {
                    Ok(pf) => pf,
                    Err(e) => {
                        log::warn!("k8s port-forward 启动失败: {e}");
                        return;
                    }
                };
                let Some(data_stream) = pf.take_stream(remote_port) else {
                    log::warn!("k8s port-forward 无可用数据流");
                    return;
                };
                // Split both ends into owned read/write halves so the two copy
                // loops can run concurrently without borrowing the same value.
                let (mut local_rx, mut local_tx) = local_stream.into_split();
                let (mut pf_rx, mut pf_tx) = tokio::io::split(data_stream);
                let client_to_pf = async {
                    let mut buf = [0u8; 16384];
                    loop {
                        match local_rx.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if pf_tx.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                                let _ = pf_tx.flush().await;
                            }
                        }
                    }
                };
                let pf_to_client = async {
                    let mut buf = [0u8; 16384];
                    loop {
                        match pf_rx.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if local_tx.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                                let _ = local_tx.flush().await;
                            }
                        }
                    }
                };
                tokio::join!(client_to_pf, pf_to_client);
                pf.abort();
            });
        }
    });

    Ok(PortForwardHandle {
        task: Some(task),
        local_port: bound_port,
        target,
    })
}

/// Handle for an interactive `kubectl exec` session running under a PTY.
pub struct ExecSession {
    pub stdin_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

/// Probe whether `shell` exists in the pod by running a non-interactive exec
/// (`<shell> -c exit`) and waiting for it to finish within ~5s.
///
/// Blocking; runs the exec on the shared k8s runtime via a dedicated std thread
/// (the UI calls `exec_pod`, which calls this, from a plain std thread with no
/// ambient tokio runtime).
fn shell_available(pod: &str, namespace: Option<&str>, context: Option<&str>, shell: &str) -> bool {
    use kube::Api;
    use kube::api::AttachParams;
    use tokio::io::AsyncReadExt as _;

    let pod = pod.to_string();
    let ns = namespace.unwrap_or("default").to_string();
    let shell = shell.to_string();
    let context = context.map(|s| s.to_string());

    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => {
                let _ = tx.send(false);
                return;
            }
        };
        let result = rt.block_on(async move {
            let client = match super::client::build_client(context.as_deref()).await {
                Ok(c) => c,
                Err(_) => return false,
            };
            let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client, &ns);
            // Run `<shell> -c exit`; success means the shell exists.
            let ap = AttachParams::default().stdin(false).stdout(true).stderr(false);
            match pods
                .exec(&pod, vec![&shell, "-c", "exit"], &ap)
                .await
            {
                Ok(mut attached) => {
                    // Drain stdout so the process can finish, with a 5s deadline.
                    let mut out = String::new();
                    if let Some(mut stdout) = attached.stdout() {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(5),
                            stdout.read_to_string(&mut out),
                        )
                        .await;
                    }
                    // Wait for the attached process to complete.
                    let _ = tokio::time::timeout(Duration::from_secs(5), attached.join()).await;
                    true
                }
                Err(_) => false,
            }
        });
        let _ = tx.send(result);
    });
    rx.recv().unwrap_or(false)
}

/// Open an interactive exec session into a pod over the API server (replaces
/// `kubectl exec -it` under a PTY). When no shell is requested, shells are
/// tried in order (bash → sh → dash → ash) to support distroless, Alpine (ash),
/// Debian/Ubuntu minimal (dash), and standard images.
///
/// This is a **synchronous** entry point (the UI calls it from a std thread).
/// The exec is opened on the shared k8s runtime; stdin/stdout are bridged to
/// the returned `ExecSession`'s tokio mpsc channels, which the TerminalView
/// reads from / writes to.
pub fn exec_pod(
    pod: &str,
    namespace: Option<&str>,
    shell: Option<&str>,
    context: Option<&str>,
) -> Result<ExecSession, PodError> {
    use kube::Api;
    use kube::api::AttachParams;
    use tokio::io::AsyncReadExt as _;
    use tokio::io::AsyncWriteExt as _;

    // Caller-requested shell wins; otherwise auto-detect from common shells.
    let resolved_shell = match shell {
        Some(s) => s.to_string(),
        None => {
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
                None => {
                    return Err(PodError(
                        "该容器中未找到可用的 shell (bash/sh/dash/ash)。可能是极简镜像 (distroless/scratch)。"
                            .to_string(),
                    ))
                }
            }
        }
    };

    let pod = pod.to_string();
    let ns = namespace.unwrap_or("default").to_string();
    let shell = resolved_shell.clone();
    let context = context.map(|s| s.to_string());

    // Open the exec session synchronously by blocking on the shared k8s runtime.
    // AttachParams: tty=true so the server allocates a PTY and merges stderr into
    // stdout (we read a single stream, matching the old PTY-based behavior).
    let rt = super::client::k8s_runtime();
    let (started_tx, started_rx) =
        std::sync::mpsc::channel::<Result<AttachedHandle, PodError>>();

    rt.spawn(async move {
        let client = match super::client::build_client(context.as_deref()).await {
            Ok(c) => c,
            Err(e) => {
                let _ = started_tx.send(Err(e));
                return;
            }
        };
        let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(client, &ns);
        let ap = AttachParams::default()
            .tty(true)
            .stdin(true)
            .stdout(true)
            .stderr(false);
        match pods.exec(&pod, vec![&resolved_shell], &ap).await {
            Ok(attached) => {
                let _ = started_tx.send(Ok(AttachedHandle::new(attached)));
            }
            Err(e) => {
                let _ = started_tx.send(Err(PodError(format!("启动 exec 失败: {}", super::client::format_err_chain(&e)))));
            }
        }
    });

    let attached = match started_rx.recv() {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(PodError("exec 启动线程异常退出".into())),
    };

    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Writer task: UI keystrokes → exec stdin.
    let mut writer = attached.stdin;
    rt.spawn(async move {
        while let Some(bytes) = stdin_rx.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
            let _ = writer.flush().await;
        }
    });

    // Reader task: exec stdout → UI TerminalView.
    let mut reader = attached.stdout;
    rt.spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
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

    Ok(ExecSession {
        stdin_tx,
        output_rx,
    })
}

/// Holds the stdin/stdout halves of a `kube` `AttachedProcess`, split out so
/// they can be moved into separate reader/writer tasks. The `AttachedProcess`
/// itself is *not* dropped — its `Drop` aborts the exec task, which would cut
/// the duplex streams. We keep it alive until the handle is dropped.
struct AttachedHandle {
    stdin: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    stdout: Box<dyn tokio::io::AsyncRead + Unpin + Send>,
    // Kept alive so the underlying exec background task is not aborted.
    _attached: Option<kube::api::AttachedProcess>,
}

impl AttachedHandle {
    fn new(mut attached: kube::api::AttachedProcess) -> Self {
        let stdin = attached
            .stdin()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>)
            .unwrap_or_else(|| Box::new(tokio::io::sink()));
        let stdout = attached
            .stdout()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)
            .unwrap_or_else(|| Box::new(tokio::io::empty()));
        Self {
            stdin,
            stdout,
            _attached: Some(attached),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
