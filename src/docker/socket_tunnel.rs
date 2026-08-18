//! Docker daemon over SSH: bridge a local Unix socket to the remote
//! `/var/run/docker.sock` via an SSH remote command.
//!
//! Instead of relying on the SSH `direct-streamlocal` channel extension (which
//! some hardened sshd_configs disable via `AllowStreamLocalForwarding no`),
//! we run `docker system dial-stdio` on the remote host — a command built into
//! the Docker CLI itself (the same trick the official docker `ssh://` transport
//! uses) that proxies stdin/stdout to the local Docker daemon's unix socket.
//! The SSH exec channel carries the HTTP/1.1 traffic transparently, and every
//! accepted local connection gets its own exec session.
//!
//! [`SocketTunnel`] binds a temporary Unix socket on the local filesystem and
//! forwards every connection from the local socket through SSH to the remote
//! `docker system dial-stdio` helper. The docker CLI (or any other client) can
//! then talk to the remote daemon by setting `DOCKER_HOST=unix://<local_path>`
//! — no system `ssh`, no manual `ssh -L`, and credentials come straight from
//! Verve's encrypted vault.
//!
//! Lifecycle:
//! - One tunnel per active Docker+SSH host pair (managed by the caller).
//! - Drop the [`SocketTunnel`] (or call [`SocketTunnel::close`]) to unlink the
//!   local socket and disconnect the SSH transport.

use std::path::PathBuf;

#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::runtime::Runtime;
#[cfg(unix)]
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio::task::JoinHandle;

#[cfg(unix)]
use crate::ssh::client::SshConnection;
use crate::ssh::host_store as ssh_host_store;
use crate::ssh::models::SshHost;

/// Default remote Docker socket path (matches `dockerd` defaults on Linux).
/// We don't connect to it directly; we rely on `docker system dial-stdio`
/// which uses the local default socket on the remote side.
#[cfg(unix)]
const _DEFAULT_REMOTE_SOCK: &str = "/var/run/docker.sock";

/// Remote command that proxies stdio to the docker daemon. This is an official
/// Docker CLI subcommand designed for the ssh:// transport — it exists on every
/// host that has the docker CLI installed, and requires no extra packages.
///
/// We prepend common Docker install paths to PATH because non-interactive SSH
/// exec sessions often have a minimal PATH (e.g. just /usr/bin:/bin) that
/// excludes `/usr/local/bin/docker` or snap-installed docker.
#[cfg(unix)]
const REMOTE_DIAL_CMD: &str =
    "PATH=/usr/bin:/usr/local/bin:/bin:/snap/bin:$PATH docker system dial-stdio";

/// An active Docker-socket tunnel. Owns:
/// - the dedicated Tokio runtime that drives the accept loop and bridge tasks
///   (the runtime must outlive the tasks spawned on it, so we hold it here)
/// - the authenticated SSH connection (kept alive for the tunnel lifetime;
///   individual bridges open their own exec sessions)
/// - the local Unix listener bound in the OS temp dir
/// - the background accept loop task
///
/// Dropping the handle closes the listener, aborts the accept loop, and shuts
/// down the runtime.
#[cfg(unix)]
pub struct SocketTunnel {
    /// Absolute path of the local Unix socket docker CLI connects to.
    pub local_path: PathBuf,
    _rt: Runtime,
    _conn: SshConnection,
    _task: JoinHandle<()>,
}

/// Windows stub: bollard and tokio have no local Unix socket support here, so
/// the SSH-tunnel transport is Unix-only. The type exists so callers
/// (`cli.rs`, panels) compile unchanged; opening always fails.
#[cfg(windows)]
pub struct SocketTunnel {
    pub local_path: PathBuf,
}

#[cfg(unix)]
impl Drop for SocketTunnel {
    fn drop(&mut self) {
        self._task.abort();
        // Best-effort socket cleanup — ignore errors (file may already be gone).
        let _ = std::fs::remove_file(&self.local_path);
        log::info!("docker tunnel: closed {}", self.local_path.display());
    }
}

#[cfg(unix)]
impl SocketTunnel {
    /// Open an SSH connection to `ssh_host` and start forwarding a local
    /// temporary Unix socket to the remote docker daemon via
    /// `docker system dial-stdio`.
    pub fn open(ssh_host: &SshHost) -> Result<Self, String> {
        log::info!(
            "docker tunnel: connecting to {}@{}:{}",
            ssh_host.username,
            ssh_host.host,
            ssh_host.port
        );

        // Build a dedicated multi-threaded Tokio runtime for this tunnel. We
        // can't reuse the caller's runtime because `open()` is invoked from a
        // sync context (cli::run_docker runs on a std worker thread with its
        // own short-lived runtime), and the accept loop + bridge tasks must
        // keep running long after the opening call returns.
        let rt = Runtime::new().map_err(|e| format!("初始化 tokio runtime 失败: {e}"))?;

        // Establish the SSH connection and bind the listener *on* the new rt so
        // all IO resources (russh handle, UnixListener) are bound to it.
        let (conn, listener, local_path) = rt.block_on(async move {
            // Authenticate an SSH connection using Verve's credentials subsystem.
            let conn = SshConnection::connect(ssh_host)
                .await
                .map_err(|e| format!("SSH 连接失败: {e}"))?;

            // Bind a unique local socket in the OS temp directory. Path length
            // is critical: macOS SUN_LEN is 104 bytes, and TMPDIR on macOS is
            // already ~50 bytes; use an extremely short filename derived from
            // the ssh_host id hash. Deterministic name per host; unlink first
            // to clean stale sockets.
            let short_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                ssh_host.id.hash(&mut hasher);
                format!("{:x}", hasher.finish())
            };
            let local_path = std::env::temp_dir().join(format!("vd{}.sock", &short_hash[..8]));
            let _ = std::fs::remove_file(&local_path);

            let listener = UnixListener::bind(&local_path)
                .map_err(|e| format!("本地 socket 监听失败 {}: {e}", local_path.display()))?;

            Ok::<(SshConnection, UnixListener, PathBuf), String>((conn, listener, local_path))
        })?;

        log::info!("docker tunnel: listening on {}", local_path.display());

        let handle = conn.handle();
        let task = rt.spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let handle = handle.clone();
                        tokio::spawn(async move {
                            if let Err(e) = bridge_one(handle, stream).await {
                                log::warn!("docker tunnel bridge error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        log::warn!("docker tunnel accept error: {e}");
                        break;
                    }
                }
            }
        });

        Ok(SocketTunnel {
            local_path,
            _rt: rt,
            _conn: conn,
            _task: task,
        })
    }

    /// The `DOCKER_HOST` value to set for docker CLI calls ("unix://...").
    pub fn docker_host_env(&self) -> String {
        format!("unix://{}", self.local_path.display())
    }
}

#[cfg(windows)]
impl SocketTunnel {
    pub fn open(_ssh_host: &crate::ssh::models::SshHost) -> Result<Self, String> {
        Err("SSH 隧道依赖本地 Unix socket，暂不支持 Windows。请在主机上使用 tcp:// 地址连接远程 Docker。".to_string())
    }

    pub fn docker_host_env(&self) -> String {
        format!("unix://{}", self.local_path.display())
    }
}

/// Bridge one accepted local Unix stream to the remote docker daemon by
/// opening an SSH exec channel running `docker system dial-stdio` and copying
/// bytes bidirectionally until EOF.
///
/// We drive the channel manually (instead of using `into_stream` +
/// `copy_bidirectional`) so we can drain stderr and capture the exit status
/// — `docker system dial-stdio` may fail (docker CLI not installed, PATH issue
/// in non-interactive shell, permission denied on docker.sock) and we want
/// that error reported in the UI, not a silent hang.
#[cfg(unix)]
async fn bridge_one(
    handle: Arc<Mutex<russh::client::Handle<crate::ssh::client::SshHandler>>>,
    mut local: UnixStream,
) -> Result<(), String> {
    use russh::ChannelMsg;
    log::debug!("docker tunnel: opening exec channel for `{REMOTE_DIAL_CMD}`");
    let mut channel = {
        let h = handle.lock().await;
        let ch = h
            .channel_open_session()
            .await
            .map_err(|e| format!("打开 SSH session 通道失败: {e}"))?;
        ch.exec(true, REMOTE_DIAL_CMD)
            .await
            .map_err(|e| format!("远端执行 `{REMOTE_DIAL_CMD}` 失败: {e}"))?;
        ch
    };

    // We keep the channel itself (for wait()/msg processing) but write to it
    // via a writer half — channel.data() sends Data messages. Reads come via
    // channel.wait().
    let mut stderr_buf: Vec<u8> = Vec::new();
    let mut exit_status: Option<u32> = None;
    let mut local_buf: Vec<u8> = vec![0u8; 64 * 1024];

    loop {
        tokio::select! {
            // docker CLI → SSH channel (stdin of dial-stdio)
            n = local.read(&mut local_buf) => {
                match n {
                    Ok(0) => {
                        // Local closed (docker CLI disconnected). Send EOF to remote.
                        let _ = channel.eof().await;
                        // Drain remaining msgs until exit.
                        break;
                    }
                    Ok(n) => {
                        channel.data(&local_buf[..n]).await
                            .map_err(|e| format!("写入 SSH stdin 失败: {e}"))?;
                    }
                    Err(e) => return Err(format!("读取本地 socket 失败: {e}")),
                }
            }
            // SSH channel → local docker CLI
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        local.write_all(&data).await
                            .map_err(|e| format!("写入本地 socket 失败: {e}"))?;
                    }
                    Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                        // stderr — accumulate for error message if the cmd fails
                        stderr_buf.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                        exit_status = Some(code);
                    }
                    Some(ChannelMsg::Eof) | None => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = local.shutdown().await;

    if let Some(code) = exit_status {
        if code != 0 {
            let err = String::from_utf8_lossy(&stderr_buf);
            let trimmed = err.trim();
            let detail = if trimmed.is_empty() {
                "(远端命令无 stderr 输出，请确认远端安装了 docker CLI 且在 PATH 中)".to_string()
            } else {
                trimmed.to_string()
            };
            return Err(format!(
                "远端 `{REMOTE_DIAL_CMD}` 失败 (退出码 {code}):\n{detail}"
            ));
        }
    }
    Ok(())
}

/// Look up an SSH host by id from Verve's persisted store.
pub fn find_ssh_host(id: &str) -> Option<SshHost> {
    ssh_host_store::find_host(id)
}
