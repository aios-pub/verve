//! Docker container management via the native `bollard` client.
//!
//! Replaces the old subprocess path that shelled out to the `docker` CLI.
//! Every public entry point keeps the same signature the UI layer
//! (`crate::ui::docker_panel`) already consumes, so the panel does not change.
//!
//! All functions are synchronous at the API boundary and run their async
//! `bollard` work on the shared [`docker_runtime`] (mirrors `crate::k8s`),
//! except `prune_images` which was already `async` and stays that way.

use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerSummary, ContainerSummaryStateEnum, ImageSummary, PortSummaryTypeEnum,
};
use bollard::query_parameters::{
    ListContainersOptionsBuilder, ListImagesOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder,
};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use super::client::{build_client, docker_runtime, format_err_chain};
use super::models::DockerHost;
use super::socket_tunnel::SocketTunnel;

/// Global cache of open SSH tunnels keyed by Verve SSH host id. A tunnel is
/// opened on first use for a given host and kept alive until the app exits
/// (bollard opens short-lived connections per request, so re-establishing the
/// SSH transport per call would be too slow). Closing the tunnel on
/// host-switch is handled by the panel explicitly.
static TUNNELS: OnceLock<StdMutex<std::collections::HashMap<String, Arc<SocketTunnel>>>> =
    OnceLock::new();

fn tunnels() -> &'static StdMutex<std::collections::HashMap<String, Arc<SocketTunnel>>> {
    TUNNELS.get_or_init(|| StdMutex::new(std::collections::HashMap::new()))
}

/// Resolve (opening if necessary) the SSH tunnel for a Verve-SSH-backed
/// Docker host. Returns the `unix://<local_path>` URL pointing at the local
/// tunnel endpoint. Tunnels are cached globally for the app lifetime.
///
/// **Important:** This may be called from within an existing tokio runtime
/// (e.g. inside the async `build_client` / `list_*` operations that run on
/// [`docker_runtime`]). Since [`SocketTunnel::open`] creates its own dedicated
/// tokio runtime and calls `block_on`, we must run the open on a dedicated std
/// thread to avoid the "Cannot start a runtime from within a runtime" panic.
/// (Mirrors `crate::k8s::kubectl::resolve_ssh_tunnel`.)
///
/// Concurrent callers racing to open the same host are serialized by holding
/// the global lock across the entire open — the first one opens, subsequent
/// callers wait and then reuse the cached tunnel.
pub(crate) fn resolve_ssh_tunnel(h: &DockerHost) -> Result<String, DockerError> {
    let ssh_id = h
        .ssh_host_id
        .clone()
        .ok_or_else(|| DockerError("Docker host 缺少 ssh_host_id".into()))?;

    // Recover from a poisoned mutex (a prior open/drop may have panicked; the
    // map itself is fine). We still need the lock to serialize concurrent opens.
    let mut guard = tunnels()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Fast path: tunnel already open.
    if let Some(tun) = guard.get(&ssh_id) {
        return Ok(tun.docker_host_env());
    }

    // Slow path: open a new tunnel on a dedicated std thread while *holding*
    // the lock, so concurrent callers wait here instead of each opening their
    // own duplicate tunnel.
    let (tx, rx) = std::sync::mpsc::channel::<Result<SocketTunnel, String>>();
    let ssh_id_for_thread = ssh_id.clone();
    std::thread::spawn(move || {
        let result = match super::socket_tunnel::find_ssh_host(&ssh_id_for_thread) {
            Some(ssh_host) => SocketTunnel::open(&ssh_host),
            None => Err(format!("找不到关联的 SSH 主机: {ssh_id_for_thread}")),
        };
        let _ = tx.send(result);
    });
    let tunnel = rx
        .recv()
        .map_err(|_| DockerError("SSH 隧道创建线程异常退出".into()))?
        .map_err(|e| DockerError(format!("建立 SSH 隧道失败: {e}")))?;

    let url = tunnel.docker_host_env();
    let arc = Arc::new(tunnel);
    guard.insert(ssh_id, arc);
    Ok(url)
}

/// Close any open SSH tunnel for the given Verve SSH host id. Called by the
/// panel when the user switches away from an SSH-backed host so we don't
/// leave dangling connections. The tunnel (which owns a tokio runtime) is
/// dropped on a dedicated std thread to avoid the "Cannot drop a runtime in a
/// context where blocking is not allowed" panic when called from an async
/// context.
pub fn close_ssh_tunnel(ssh_host_id: &str) {
    let mut guard = tunnels()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(tunnel) = guard.remove(ssh_host_id) {
        log::info!("docker tunnel: closed for ssh host {ssh_host_id}");
        defer_tunnel_drop(tunnel);
    }
}

/// Drop an `Arc<SocketTunnel>` on a dedicated std thread so the contained
/// tokio `Runtime` is torn down in a blocking context. Must be used whenever
/// the last Arc reference for a tunnel is released from within an async
/// context.
fn defer_tunnel_drop(tunnel: Arc<SocketTunnel>) {
    std::thread::spawn(move || {
        drop(tunnel);
    });
}

// ---------------------------------------------------------------------------
// Public types (unchanged shapes — the UI depends on these)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DockerError(pub String);

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A container summary. Field set mirrors what the old `docker ps --format
/// '{{json .}}'` produced, so the UI rendering is unchanged.
#[derive(Debug, Clone)]
pub struct Container {
    pub id: String,
    pub names: String,
    pub image: String,
    pub status: String,
    pub state: String,
    pub ports: String,
    pub created: String,
}

/// Aggregated container state (one of "running" / "exited" / "paused" / "created").
pub type ContainerState = String;

/// An image summary. Mirrors the old `docker images --format '{{json .}}'` output.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub repository: String,
    pub tag: String,
    pub id: String,
    pub size: String,
    pub created: String,
}

/// Handle for streaming logs; drop to cancel the stream and abort the reader
/// task (replaces the old `kill_on_drop` child process).
pub struct LogStream {
    pub lines: Option<mpsc::Receiver<String>>,
    _abort: tokio::task::AbortHandle,
}

impl LogStream {
    pub fn take_lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.lines.take()
    }
}

/// Handle for an interactive exec session. The daemon allocates a PTY (Tty=true)
/// so stdin is a TTY and stdout carries ANSI-escaped bytes — same shape the old
/// `docker exec -it` over a local PTY produced. Keystrokes flow in via
/// `stdin_tx`, container output comes back over `output_rx`.
pub struct ExecSession {
    /// Send bytes (keystrokes) to write to the exec stdin.
    pub stdin_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Raw output bytes (ANSI-escaped container output) to feed into TerminalView.
    pub output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Field mapping: bollard Engine API types → Verve display structs
// ---------------------------------------------------------------------------

/// Map a `ContainerSummary` (Engine API) into the flat [`Container`] the UI
/// expects. The formatting matches the old CLI `--format '{{json .}}'` output
/// closely enough that the panel's rendering is unchanged.
fn map_container(c: ContainerSummary) -> Container {
    let id = c.id.unwrap_or_default();
    // Engine API returns names with a leading "/" (e.g. "/web"); trim it and
    // join multiples with commas, mirroring `{{json .Names}}`.
    let names = c
        .names
        .unwrap_or_default()
        .iter()
        .map(|n| n.trim_start_matches('/').to_string())
        .collect::<Vec<_>>()
        .join(",");

    let image = c.image.unwrap_or_default();

    let state = c.state.as_ref().map(|s| s.to_string()).unwrap_or_default();

    let status = c.status.unwrap_or_default();

    let ports = c
        .ports
        .unwrap_or_default()
        .iter()
        .map(format_port)
        .collect::<Vec<_>>()
        .join(", ");

    let created = c.created.map(format_created_time).unwrap_or_default();

    Container {
        id,
        names,
        image,
        status,
        state,
        ports,
        created,
    }
}

/// Format a [`bollard::models::PortSummary`] like the CLI does, e.g.
/// `0.0.0.0:8080->80/tcp`, `443/tcp`, or `80/tcp` (no host binding).
fn format_port(p: &bollard::models::PortSummary) -> String {
    let proto = match p.typ {
        Some(PortSummaryTypeEnum::TCP) => "tcp",
        Some(PortSummaryTypeEnum::UDP) => "udp",
        Some(PortSummaryTypeEnum::SCTP) => "sctp",
        _ => "tcp",
    };
    match (&p.ip, &p.public_port) {
        (Some(ip), Some(pub_port)) => {
            // Canonical "host:pub->priv/proto" form (e.g. "0.0.0.0:8080->80/tcp").
            format!("{}:{}->{}/{}", ip, pub_port, p.private_port, proto)
        }
        _ => format!("{}/{}", p.private_port, proto),
    }
}

/// Format a Unix timestamp (seconds) the way the CLI's `{{json .CreatedAt}}`
/// roughly does: `2024-01-01 00:00:00 +0000 UTC`. Falls back to the raw number
/// if the timestamp is invalid.
fn format_created_time(secs: i64) -> String {
    use chrono::{FixedOffset, TimeZone};
    if secs <= 0 {
        return String::new();
    }
    // Match the CLI's human-readable form: "2024-01-01 00:00:00 +0000 UTC".
    let utc = FixedOffset::east_opt(0).unwrap(); // +00:00
    match utc.timestamp_opt(secs, 0) {
        chrono::LocalResult::Single(dt) => {
            format!("{} UTC", dt.format("%Y-%m-%d %H:%M:%S %z"))
        }
        _ => secs.to_string(),
    }
}

/// Map an `ImageSummary` (Engine API) into the flat [`ImageInfo`] the UI expects.
fn map_image(img: ImageSummary) -> ImageInfo {
    // repo_tags looks like ["nginx:latest", ...]; split the first one into
    // repository:tag. `<none>:<none>` (untagged) becomes ("", "").
    let (repository, tag) = img
        .repo_tags
        .first()
        .and_then(|t| {
            // Split on the last ':' so repositories containing ':' (rare, but
            // possible with ports in registry refs) still work.
            t.rsplit_once(':')
                .map(|(r, tg)| (r.to_string(), tg.to_string()))
        })
        .unwrap_or_default();

    // Shorten "sha256:<hex>" to the first 19 chars (matches `docker images` ID
    // column width). Falls back to the full id if it doesn't look like a digest.
    let id = if img.id.starts_with("sha256:") && img.id.len() > 19 {
        img.id[..19].to_string()
    } else {
        img.id
    };

    let size = format_size(img.size);
    let created = format_created_relative(img.created);

    ImageInfo {
        repository,
        tag,
        id,
        size,
        created,
    }
}

/// Format a byte count into a human-readable string (e.g. "123.4MB"), matching
/// the CLI's approximate units.
fn format_size(bytes: i64) -> String {
    let bytes = bytes as f64;
    const UNITS: &[&str] = &["B", "kB", "MB", "GB", "TB"];
    if bytes < 1.0 {
        return "0B".to_string();
    }
    let mut size = bytes;
    let mut unit_idx = 0;
    while size >= 1000.0 && unit_idx < UNITS.len() - 1 {
        size /= 1000.0;
        unit_idx += 1;
    }
    // docker prints with one decimal for MB+, integer for B.
    if unit_idx == 0 {
        format!("{}{}", size as i64, UNITS[unit_idx])
    } else {
        format!("{:.1}{}", size, UNITS[unit_idx])
    }
}

/// Format a Unix timestamp (seconds) as a relative time like "2 weeks ago",
/// approximating the CLI's `{{json .CreatedSince}}`.
fn format_created_relative(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let now = chrono::Utc::now().timestamp();
    let delta = now.saturating_sub(secs);
    let (amount, unit) = if delta < 60 {
        (delta, "second")
    } else if delta < 3600 {
        (delta / 60, "minute")
    } else if delta < 86400 {
        (delta / 3600, "hour")
    } else if delta < 604800 {
        (delta / 86400, "day")
    } else if delta < 2592000 {
        (delta / 604800, "week")
    } else if delta < 31536000 {
        (delta / 2592000, "month")
    } else {
        (delta / 31536000, "year")
    };
    let plural = if amount == 1 { "" } else { "s" };
    format!("{} {}{} ago", amount, unit, plural)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// List all containers (running + stopped).
pub fn list_containers(host: Option<&DockerHost>) -> Result<Vec<Container>, DockerError> {
    let host = host.cloned();
    docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        let options = ListContainersOptionsBuilder::new().all(true).build();
        let summaries: Vec<ContainerSummary> = client
            .list_containers(Some(options))
            .await
            .map_err(|e| DockerError(format!("获取容器列表失败: {}", format_err_chain(&e))))?;
        Ok(summaries.into_iter().map(map_container).collect())
    })
}

/// List all images.
pub fn list_images(host: Option<&DockerHost>) -> Result<Vec<ImageInfo>, DockerError> {
    let host = host.cloned();
    docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        // digests=true so repo_digests is populated; matches old `docker images` behavior.
        let options = ListImagesOptionsBuilder::new()
            .all(true)
            .digests(true)
            .build();
        let summaries: Vec<ImageSummary> = client
            .list_images(Some(options))
            .await
            .map_err(|e| DockerError(format!("获取镜像列表失败: {}", format_err_chain(&e))))?;
        Ok(summaries.into_iter().map(map_image).collect())
    })
}

pub fn start(host: Option<&DockerHost>, id: &str) -> Result<(), DockerError> {
    let host = host.cloned();
    let id = id.to_string();
    docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        client
            .start_container(&id, None)
            .await
            .map_err(|e| DockerError(format!("启动容器失败: {}", format_err_chain(&e))))?;
        Ok(())
    })
}

pub fn stop(host: Option<&DockerHost>, id: &str) -> Result<(), DockerError> {
    let host = host.cloned();
    let id = id.to_string();
    docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        client
            .stop_container(&id, None)
            .await
            .map_err(|e| DockerError(format!("停止容器失败: {}", format_err_chain(&e))))?;
        Ok(())
    })
}

pub fn restart(host: Option<&DockerHost>, id: &str) -> Result<(), DockerError> {
    let host = host.cloned();
    let id = id.to_string();
    docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        client
            .restart_container(&id, None)
            .await
            .map_err(|e| DockerError(format!("重启容器失败: {}", format_err_chain(&e))))?;
        Ok(())
    })
}

pub fn remove(host: Option<&DockerHost>, id: &str, force: bool) -> Result<(), DockerError> {
    let host = host.cloned();
    let id = id.to_string();
    docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        let options = RemoveContainerOptionsBuilder::new().force(force).build();
        client
            .remove_container(&id, Some(options))
            .await
            .map_err(|e| DockerError(format!("删除容器失败: {}", format_err_chain(&e))))?;
        Ok(())
    })
}

/// Prune dangling + unused images. Returns a human-readable summary string
/// (e.g. "Total reclaimed space: 1.23GB").
pub async fn prune_images(host: Option<&DockerHost>) -> Result<String, DockerError> {
    let host = host.cloned();
    let client = build_client(host.as_ref()).await?;
    let resp = client
        .prune_images(None::<bollard::query_parameters::PruneImagesOptions>)
        .await
        .map_err(|e| DockerError(format!("清理镜像失败: {}", format_err_chain(&e))))?;
    let reclaimed = resp.space_reclaimed.unwrap_or(0);
    Ok(format!("Total reclaimed space: {}", format_size(reclaimed)))
}

/// Stream container logs (`docker logs -f --tail=N <id>` equivalent) into a
/// channel of lines. Drop the returned [`LogStream`] to cancel the stream.
pub fn stream_logs(
    host: Option<&DockerHost>,
    id: &str,
    tail: u32,
) -> Result<LogStream, DockerError> {
    let host = host.cloned();
    let id = id.to_string();

    // Build the client synchronously so connection errors surface from this
    // call (matching the old behavior where `docker logs` spawn failures were
    // reported here, not silently lost).
    let client = docker_runtime().block_on(async move { build_client(host.as_ref()).await })?;

    let (tx, rx) = mpsc::channel::<String>(256);

    let tail_str = tail.to_string();
    let mut stream = client.logs(
        &id,
        Some(
            LogsOptionsBuilder::new()
                .follow(true)
                .stdout(true)
                .stderr(true)
                .tail(&tail_str)
                .build(),
        ),
    );

    // Pump log messages into the channel on the shared runtime. Aborting this
    // task drops the stream, which closes the server-side follow.
    let handle = docker_runtime().spawn(async move {
        while let Some(item) = stream.next().await {
            match item {
                Ok(LogOutput::StdOut { message })
                | Ok(LogOutput::StdErr { message })
                | Ok(LogOutput::Console { message }) => {
                    // bollard yields a `Bytes` per log frame; the old CLI path
                    // split on newlines. Convert to lossy UTF-8 and forward.
                    let line = String::from_utf8_lossy(&message).to_string();
                    if tx.send(line).await.is_err() {
                        break; // receiver dropped → stop streaming
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("docker logs stream error: {}", format_err_chain(&e));
                    break;
                }
            }
        }
    });

    Ok(LogStream {
        lines: Some(rx),
        _abort: handle.abort_handle(),
    })
}

/// Probe whether `shell` exists inside the container by running a non-interactive
/// exec (`<shell> -c exit`) and waiting for it to finish within ~5s.
///
/// Runs the exec on the shared docker runtime.
fn shell_available(host: Option<&DockerHost>, id: &str, shell: &str) -> bool {
    let host = host.cloned();
    let id = id.to_string();
    let shell = shell.to_string();

    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    docker_runtime().spawn(async move {
        let result = async {
            let client = match build_client(host.as_ref()).await {
                Ok(c) => c,
                Err(_) => return false,
            };
            // Non-interactive exec: no TTY, attach stdout only, run `<shell> -c exit`.
            let config = CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(vec![shell.clone(), "-c".to_string(), "exit".to_string()]),
                ..Default::default()
            };
            let exec = match client.create_exec(&id, config).await {
                Ok(e) => e,
                Err(_) => return false,
            };
            match client.start_exec(&exec.id, None).await {
                Ok(StartExecResults::Attached { mut output, .. }) => {
                    // Drain stdout/stderr so the process can finish, with a deadline.
                    let _ = tokio::time::timeout(Duration::from_secs(5), async {
                        while let Some(_item) = output.next().await {}
                    })
                    .await;
                    true
                }
                Ok(StartExecResults::Detached) => true,
                Err(_) => false,
            }
        };
        let _ = tx.send(result.await);
    });
    rx.recv().unwrap_or(false)
}

/// Open an interactive exec session into a container (replaces `docker exec
/// -it` under a local PTY). When no shell is requested, shells are tried in
/// order (bash → sh → dash → ash) to support distroless, Alpine (ash),
/// Debian/Ubuntu minimal (dash), and standard images.
///
/// This is a **synchronous** entry point (the UI calls it from a std thread).
/// The exec is opened on the shared docker runtime; stdin/stdout are bridged
/// to the returned [`ExecSession`]'s tokio mpsc channels, which the
/// TerminalView reads from / writes to.
pub fn exec_container(
    host: Option<&DockerHost>,
    id: &str,
    shell: Option<&str>,
) -> Result<ExecSession, DockerError> {
    // Caller-requested shell wins; otherwise auto-detect from common shells.
    let resolved_shell = match shell {
        Some(s) => s.to_string(),
        None => {
            let candidates = ["bash", "sh", "dash", "ash"];
            let mut found = None;
            for candidate in candidates {
                if shell_available(host, id, candidate) {
                    found = Some(candidate.to_string());
                    break;
                }
            }
            match found {
                Some(s) => s,
                None => {
                    return Err(DockerError(
                        "该容器中未找到可用的 shell (bash/sh/dash/ash)。可能是极简镜像 (distroless/scratch)。"
                            .to_string(),
                    ))
                }
            }
        }
    };

    let host = host.cloned();
    let id = id.to_string();
    let resolved_shell_for_probe = resolved_shell.clone();

    // Open the exec session synchronously by blocking on the shared docker
    // runtime. Tty=true so the daemon allocates a PTY and (per the Engine API)
    // does NOT multiplex stdout/stderr — the upgraded socket is a raw byte
    // stream carrying ANSI escapes, exactly what TerminalView expects.
    let (input, output) = docker_runtime().block_on(async move {
        let client = build_client(host.as_ref()).await?;
        let config = CreateExecOptions {
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(false),
            tty: Some(true),
            cmd: Some(vec![resolved_shell_for_probe]),
            ..Default::default()
        };
        let exec = client
            .create_exec(&id, config)
            .await
            .map_err(|e| DockerError(format!("创建 exec 失败: {}", format_err_chain(&e))))?;
        match client.start_exec(&exec.id, None).await {
            Ok(StartExecResults::Attached { input, output }) => Ok((input, output)),
            Ok(StartExecResults::Detached) => Err(DockerError(
                "exec 以 detached 模式启动，无法附加交互会话".into(),
            )),
            Err(e) => Err(DockerError(format!(
                "启动 exec 失败: {}",
                format_err_chain(&e)
            ))),
        }
    })?;

    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Writer task: UI keystrokes → exec stdin. `input` is a Pin<Box<dyn
    // AsyncWrite + Send>>; pin-project it via Pin<&mut _> for write_all.
    let mut input = input;
    docker_runtime().spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Some(bytes) = stdin_rx.recv().await {
            if input.as_mut().write_all(&bytes).await.is_err() {
                break;
            }
            let _ = input.as_mut().flush().await;
        }
    });

    // Reader task: exec stdout → UI TerminalView. With Tty=true the output
    // stream is raw (no 8-byte multiplex header), so each LogOutput message is
    // a chunk of ANSI-escaped bytes.
    docker_runtime().spawn(async move {
        let mut output = output;
        while let Some(item) = output.next().await {
            match item {
                Ok(LogOutput::StdOut { message })
                | Ok(LogOutput::StdErr { message })
                | Ok(LogOutput::Console { message }) => {
                    if output_tx.send(message.to_vec()).is_err() {
                        break; // receiver dropped → stop
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    log::debug!("docker exec stream ended: {}", format_err_chain(&e));
                    break;
                }
            }
        }
    });

    Ok(ExecSession {
        stdin_tx,
        output_rx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_container_summary_to_display_struct() {
        let summary = ContainerSummary {
            id: Some("abc123".into()),
            names: Some(vec!["/web".into(), "/web2".into()]),
            image: Some("nginx:latest".into()),
            image_id: None,
            image_manifest_descriptor: None,
            command: None,
            created: Some(1704067200), // 2024-01-01 UTC
            ports: Some(vec![bollard::models::PortSummary {
                ip: Some("0.0.0.0".into()),
                private_port: 80,
                public_port: Some(8080),
                typ: Some(PortSummaryTypeEnum::TCP),
            }]),
            size_rw: None,
            size_root_fs: None,
            labels: None,
            state: Some(ContainerSummaryStateEnum::RUNNING),
            status: Some("Up 2 hours".into()),
            host_config: None,
            network_settings: None,
            mounts: None,
            health: None,
        };
        let c = map_container(summary);
        assert_eq!(c.id, "abc123");
        assert_eq!(c.names, "web,web2");
        assert_eq!(c.image, "nginx:latest");
        assert_eq!(c.state, "running");
        assert_eq!(c.status, "Up 2 hours");
        assert!(c.ports.contains("8080"), "ports = {}", c.ports);
        assert!(c.created.contains("2024"), "created = {}", c.created);
    }

    #[test]
    fn maps_image_summary_to_display_struct() {
        let img = ImageSummary {
            id: "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".into(),
            parent_id: String::new(),
            repo_tags: vec!["nginx:latest".into()],
            repo_digests: vec![],
            created: 1704067200,
            size: 142_600_000,
            shared_size: 0,
            labels: Default::default(),
            containers: -1,
            manifests: None,
            descriptor: None,
        };
        let info = map_image(img);
        assert_eq!(info.repository, "nginx");
        assert_eq!(info.tag, "latest");
        assert_eq!(info.id.len(), 19); // sha256: + 12 hex chars
        assert!(info.size.contains("MB"), "size = {}", info.size);
        assert!(info.created.ends_with("ago"));
    }

    #[test]
    fn formats_bytes_in_human_units() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1500), "1.5kB");
        assert!(format_size(142_600_000).contains("MB"));
    }

    #[test]
    fn formats_relative_time() {
        let now = chrono::Utc::now().timestamp();
        // 5 minutes ago
        let s = format_created_relative(now - 300);
        assert!(s.contains("minute"), "got {s}");
        // 3 days ago (under 1 week → "days")
        let s = format_created_relative(now - 86400 * 3);
        assert!(s.contains("day"), "got {s}");
        // 30 days ago (over 1 week → "weeks")
        let s = format_created_relative(now - 86400 * 30);
        assert!(s.contains("week") || s.contains("month"), "got {s}");
    }
}
