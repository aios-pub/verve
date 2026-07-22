//! Helpers that shell out to the `docker` CLI.
//!
//! Mirrors `crate::k8s::kubectl`: parse JSON from `--format '{{json .}}'`, stream
//! logs via a spawned child, and spawn an interactive exec session.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::{models::DockerHost, socket_tunnel::SocketTunnel};

/// Global cache of open SSH tunnels keyed by Verve SSH host id. A tunnel is
/// opened on first use for a given host and kept alive until the app exits
/// (docker CLI opens short-lived connections; re-establishing the SSH
/// transport per call would be too slow). Closing the tunnel on host-switch
/// is handled by the panel explicitly.
static TUNNELS: OnceLock<StdMutex<std::collections::HashMap<String, Arc<SocketTunnel>>>> =
    OnceLock::new();

fn tunnels() -> &'static StdMutex<std::collections::HashMap<String, Arc<SocketTunnel>>> {
    TUNNELS.get_or_init(|| StdMutex::new(std::collections::HashMap::new()))
}

/// Resolve the docker CLI absolute path, falling back to PATH lookup. Checks
/// platform-specific well-known install locations that aren't always on PATH.
fn docker_binary() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/opt/homebrew/bin/docker"));
        candidates.push(PathBuf::from("/usr/local/bin/docker"));
        candidates.push(PathBuf::from(
            "/Applications/Docker.app/Contents/Resources/bin/docker",
        ));
        candidates.push(PathBuf::from("/opt/homebrew/bin/nerdctl"));
        candidates.push(PathBuf::from("/usr/local/bin/nerdctl"));
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/bin/docker"));
        candidates.push(PathBuf::from("/usr/local/bin/docker"));
        candidates.push(PathBuf::from("/snap/bin/docker"));
        candidates.push(PathBuf::from("/usr/bin/podman"));
    }
    #[cfg(target_os = "windows")]
    {
        let program_files = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        candidates.push(program_files.join("Docker").join("Docker").join("resources").join("bin").join("docker.exe"));
        candidates.push(program_files.join("Docker").join("Docker").join("resources").join("bin").join("docker"));
        candidates.push(PathBuf::from(r"C:\ProgramData\DockerDesktop\version-bin\docker.exe"));
    }

    // ~/.docker/bin (user-installed, all platforms)
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        candidates.push(PathBuf::from(home).join(".docker/bin/docker"));
    }
    for p in &candidates {
        if p.exists() {
            return p.clone();
        }
    }
    // Fall back to PATH resolution.
    PathBuf::from(if cfg!(windows) { "docker.exe" } else { "docker" })
}

/// Resolve (opening if necessary) the SSH tunnel for a Verve-SSH-backed
/// Docker host. Returns the `DOCKER_HOST=unix://...` URL pointing at the local
/// tunnel endpoint. Tunnels are cached globally for the app lifetime.
fn resolve_ssh_tunnel(h: &DockerHost) -> Result<String, DockerError> {
    let Some(ssh_id) = h.ssh_host_id.as_ref() else {
        return Err(DockerError("Docker host 缺少 ssh_host_id".into()));
    };

    // Fast path: tunnel already open.
    {
        let guard = tunnels().lock().unwrap();
        if let Some(tun) = guard.get(ssh_id) {
            return Ok(tun.docker_host_env());
        }
    }

    // Slow path: open a new tunnel. SocketTunnel::open is synchronous — it
    // builds its own dedicated tokio runtime that lives as long as the tunnel,
    // so the accept loop keeps running after this call returns.
    let tunnel = {
        let ssh_host = super::socket_tunnel::find_ssh_host(ssh_id)
            .ok_or_else(|| DockerError(format!("找不到关联的 SSH 主机: {ssh_id}")))?;
        SocketTunnel::open(&ssh_host)
            .map_err(|e| DockerError(format!("建立 SSH 隧道失败: {e}")))?
    };

    let url = tunnel.docker_host_env();
    let arc = Arc::new(tunnel);
    tunnels().lock().unwrap().insert(ssh_id.clone(), arc);
    Ok(url)
}

/// Close any open SSH tunnel for the given Verve SSH host id. Called by the
/// panel when the user switches away from an SSH-backed host so we don't
/// leave dangling connections.
pub fn close_ssh_tunnel(ssh_host_id: &str) {
    let mut guard = tunnels().lock().unwrap();
    if guard.remove(ssh_host_id).is_some() {
        log::info!("docker tunnel: closed for ssh host {ssh_host_id}");
    }
}

/// Apply Docker host environment variables (DOCKER_HOST, DOCKER_TLS_VERIFY,
/// DOCKER_CERT_PATH) to a tokio Command when a non-local host is configured.
/// For Verve SSH-tunnel hosts, the local unix socket URL is resolved first.
fn apply_host_env(cmd: &mut Command, host: Option<&DockerHost>) -> Result<(), DockerError> {
    if let Some(h) = host {
        if let Some(ref ssh_id) = h.ssh_host_id {
            let url = resolve_ssh_tunnel(h)?;
            cmd.env("DOCKER_HOST", url);
            return Ok(());
        }
        if !h.is_local() && !h.host.is_empty() {
            cmd.env("DOCKER_HOST", &h.host);
            if h.tls_verify {
                cmd.env("DOCKER_TLS_VERIFY", "1");
            }
            if let Some(ref cert) = h.cert_path {
                cmd.env("DOCKER_CERT_PATH", cert);
            }
        }
    }
    Ok(())
}

/// Apply Docker host environment variables to a std (sync) Command.
fn apply_host_env_sync(cmd: &mut std::process::Command, host: Option<&DockerHost>) -> Result<(), DockerError> {
    if let Some(h) = host {
        if let Some(ref ssh_id) = h.ssh_host_id {
            let url = resolve_ssh_tunnel(h)?;
            cmd.env("DOCKER_HOST", url);
            return Ok(());
        }
        if !h.is_local() && !h.host.is_empty() {
            cmd.env("DOCKER_HOST", &h.host);
            if h.tls_verify {
                cmd.env("DOCKER_TLS_VERIFY", "1");
            }
            if let Some(ref cert) = h.cert_path {
                cmd.env("DOCKER_CERT_PATH", cert);
            }
        }
    }
    Ok(())
}

/// Build a Command targeting the docker binary, extending PATH with common
/// macOS locations so subcommands can find sibling helpers like `docker-credential-osxkeychain`.
fn docker_command(host: Option<&DockerHost>) -> Result<Command, DockerError> {
    let bin = docker_binary();
    let mut cmd = Command::new(&bin);
    cmd.env("PATH", extended_path());
    apply_host_env(&mut cmd, host)?;
    Ok(cmd)
}

/// Build a std::process::Command for blocking docker calls with extended PATH.
fn docker_command_sync(host: Option<&DockerHost>) -> Result<std::process::Command, DockerError> {
    let bin = docker_binary();
    let mut cmd = std::process::Command::new(&bin);
    cmd.env("PATH", extended_path());
    apply_host_env_sync(&mut cmd, host)?;
    Ok(cmd)
}

#[derive(Debug, Clone)]
pub struct DockerError(pub String);

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A container summary (from `docker ps -a --format '{{json .}}'`).
#[derive(Debug, Clone, Deserialize)]
pub struct Container {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Names")]
    pub names: String,
    #[serde(rename = "Image")]
    pub image: String,
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Ports")]
    pub ports: String,
    #[serde(rename = "CreatedAt")]
    pub created: String,
}

/// Aggregated container state (one of "running" / "exited" / "paused" / "created").
pub type ContainerState = String;

/// An image summary (from `docker images --format '{{json .}}'`).
#[derive(Debug, Clone, Deserialize)]
pub struct ImageInfo {
    #[serde(rename = "Repository")]
    pub repository: String,
    #[serde(rename = "Tag")]
    pub tag: String,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Size")]
    pub size: String,
    #[serde(rename = "CreatedSince")]
    pub created: String,
}

/// Handle for streaming logs; drop to kill `docker logs -f`.
pub struct LogStream {
    pub lines: Option<mpsc::Receiver<String>>,
    pub _child: Child,
}

impl LogStream {
    pub fn take_lines(&mut self) -> Option<mpsc::Receiver<String>> {
        self.lines.take()
    }
}

/// Handle for an interactive `docker exec -it` session. The docker CLI runs
/// attached to a real local PTY (via `portable-pty`), so stdin is a TTY —
/// this lets us pass `-t` and get proper shell prompts/readline/color output
/// inside the container.
pub struct ExecSession {
    /// Send bytes (keystrokes) to write to the PTY master → into docker exec.
    pub stdin_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Raw output bytes from the PTY master (ANSI-escaped container output)
    /// to feed into TerminalView.
    pub output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

/// Timeout for docker CLI calls. If the docker CLI doesn't respond within this
/// duration (e.g. daemon is hung/unreachable), we kill the child and return an
/// error so the UI never gets stuck on "loading…".
const DOCKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn run_docker(host: Option<&DockerHost>, args: &[&str]) -> Result<std::process::Output, DockerError> {
    // std's `.output()` internally spawns reader threads for both stdout and
    // stderr before waiting on the child — it correctly handles large output
    // and can never deadlock on pipe buffers. We run it on a helper thread and
    // apply a timeout via recv_timeout so a hung Docker daemon can't freeze
    // the UI forever. The helper thread may be leaked on timeout (the docker
    // CLI process will still exit on its own or be reaped by init), which is
    // acceptable for a desktop app.
    let mut cmd = docker_command_sync(host)?;
    cmd.args(args);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(cmd.output());
    });
    match rx.recv_timeout(DOCKER_TIMEOUT) {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(DockerError(format!(
            "docker 执行失败: {e}; 请确认 Docker daemon 正在运行且 docker CLI 已安装"
        ))),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(DockerError(
            "docker 命令超时（30秒），请确认 Docker daemon 正在运行".into(),
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(DockerError(
            "docker 命令执行异常终止，请重试".into(),
        )),
    }
}

pub async fn run_docker_async(host: Option<&DockerHost>, args: &[&str]) -> Result<std::process::Output, DockerError> {
    use tokio::time::timeout;
    let mut cmd = docker_command(host)?;
    cmd.args(args);
    match timeout(DOCKER_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(DockerError(format!("docker 执行失败: {e}"))),
        Err(_) => Err(DockerError(
            "docker 命令超时（30秒），请确认 Docker daemon 正在运行".into(),
        )),
    }
}

fn extended_path() -> String {
    let mut dirs: Vec<String> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
        dirs.push("/Applications/Docker.app/Contents/Resources/bin".into());
        dirs.push("/usr/bin".into());
        dirs.push("/bin".into());
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push("/usr/bin".into());
        dirs.push("/usr/local/bin".into());
        dirs.push("/snap/bin".into());
    }
    #[cfg(target_os = "windows")]
    {
        // Docker Desktop's bin dir on Windows; let PATH handle the rest.
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            let docker_bin = PathBuf::from(&pf).join("Docker").join("Docker").join("resources").join("bin");
            if docker_bin.exists() {
                if let Some(s) = docker_bin.to_str() {
                    dirs.push(s.to_string());
                }
            }
        }
    }

    if let Ok(existing) = std::env::var("PATH") {
        dirs.push(existing);
    }
    // Use the platform-native PATH separator.
    let sep = if cfg!(windows) { ";" } else { ":" };
    dirs.join(sep)
}

fn check_ok(out: std::process::Output) -> Result<(), DockerError> {
    if out.status.success() {
        Ok(())
    } else {
        Err(DockerError(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

pub fn list_containers(host: Option<&DockerHost>) -> Result<Vec<Container>, DockerError> {
    let out = run_docker(host, &["ps", "-a", "--format", "{{json .}}"])?;
    check_ok(out.clone())?;
    parse_json_lines(&String::from_utf8_lossy(&out.stdout))
}

pub fn list_images(host: Option<&DockerHost>) -> Result<Vec<ImageInfo>, DockerError> {
    let out = run_docker(host, &["images", "--format", "{{json .}}"])?;
    check_ok(out.clone())?;
    parse_json_lines(&String::from_utf8_lossy(&out.stdout))
}

fn parse_json_lines<'a, T: Deserialize<'a>>(text: &'a str) -> Result<Vec<T>, DockerError> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(line) {
            Ok(v) => out.push(v),
            Err(e) => log::warn!("docker: skipping malformed line: {e}: {line}"),
        }
    }
    Ok(out)
}

pub fn start(host: Option<&DockerHost>, id: &str) -> Result<(), DockerError> {
    check_ok(run_docker(host, &["start", id])?)
}
pub fn stop(host: Option<&DockerHost>, id: &str) -> Result<(), DockerError> {
    check_ok(run_docker(host, &["stop", id])?)
}
pub fn restart(host: Option<&DockerHost>, id: &str) -> Result<(), DockerError> {
    check_ok(run_docker(host, &["restart", id])?)
}
pub fn remove(host: Option<&DockerHost>, id: &str, force: bool) -> Result<(), DockerError> {
    let mut args = vec!["rm"];
    if force {
        args.push("-f");
    }
    args.push(id);
    check_ok(run_docker(host, &args)?)
}

/// Prune dangling + unused images. Returns the number of bytes reclaimed.
pub async fn prune_images(host: Option<&DockerHost>) -> Result<String, DockerError> {
    let out = run_docker_async(host, &["image", "prune", "-f"]).await?;
    if !out.status.success() {
        return Err(DockerError(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    // Last line typically reads "Total reclaimed space: 1.23GB".
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Spawn `docker logs -f --tail=N <id>` and stream lines back to a channel.
pub fn stream_logs(host: Option<&DockerHost>, id: &str, tail: u32) -> Result<LogStream, DockerError> {
    let mut cmd = docker_command(host)?;
    cmd.arg("logs")
        .arg("-f")
        .arg("--tail")
        .arg(tail.to_string())
        .arg(id);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| DockerError(format!("启动 docker logs 失败: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DockerError("docker 未输出 stdout".into()))?;
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
        _child: child,
    })
}

/// Spawn `docker exec -it <id> <shell>` for an interactive session.
///
/// The docker CLI runs with its stdio attached to a real local pseudo-terminal
/// (PTY) allocated via `portable-pty`, so we can safely pass `-t` — the CLI
/// sees a genuine TTY, allocates one inside the container, and we get proper
/// shell prompts, line editing, color output, and signals. Output bytes are
/// forwarded over a tokio channel to TerminalView for ANSI rendering; input
/// bytes come back via stdin_tx and are written to the PTY master.
///
/// PTY IO runs on a dedicated std thread (not tokio) because `portable-pty`'s
/// MasterPty is a trait object without async support; we do blocking reads/
/// writes and hand bytes across a channel boundary.
pub fn exec_container(host: Option<&DockerHost>, id: &str, shell: Option<&str>) -> Result<ExecSession, DockerError> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};

    let shell = shell.unwrap_or("sh");
    let bin = docker_binary();

    // Build a CommandBuilder pointing at the docker binary, forwarding host env.
    let mut cmd = CommandBuilder::new(&bin);
    cmd.env("PATH", extended_path());
    if let Some(h) = host {
        if let Some(ref ssh_id) = h.ssh_host_id {
            let url = resolve_ssh_tunnel(h)?;
            cmd.env("DOCKER_HOST", url);
        } else if !h.is_local() && !h.host.is_empty() {
            cmd.env("DOCKER_HOST", &h.host);
            if h.tls_verify {
                cmd.env("DOCKER_TLS_VERIFY", "1");
            }
            if let Some(ref cert) = h.cert_path {
                cmd.env("DOCKER_CERT_PATH", cert);
            }
        }
    }
    cmd.arg("exec");
    cmd.arg("-it");
    cmd.arg(id);
    cmd.arg(shell);

    // Allocate a PTY at a default size; terminal resizes happen via PTY resize API
    // but wiring that up is a later improvement — 120x32 is a sane default.
    let pty_system = native_pty_system();
    let mut pair = pty_system
        .openpty(PtySize { rows: 32, cols: 120, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| DockerError(format!("分配 PTY 失败: {e}")))?;

    // Spawn docker exec as a child of the PTY slave.
    let mut _child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| DockerError(format!("启动 docker exec 失败: {e}")))?;
    drop(pair.slave);

    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    // portable-pty exposes separate reader/writer handles via dedicated methods:
    // `try_clone_reader()` → independent boxed Read impl (own fd)
    // `take_writer()`      → boxed Write impl that consumes write-side ownership
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| DockerError(format!("PTY reader 获取失败: {e}")))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| DockerError(format!("PTY writer 获取失败: {e}")))?;
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
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(_) => return,
        };
        // Wrap reader in an Arc<Mutex> so it can be safely passed to spawn_blocking
        // without borrowing across an await point.
        let reader = std::sync::Arc::new(std::sync::Mutex::new(reader));
        rt.block_on(async move {
            loop {
                let reader = reader.clone();
                // Blocking read on the PTY reader, off the async runtime.
                let read_res = tokio::task::spawn_blocking(move || {
                    let mut buf = [0u8; 8192];
                    let mut guard = reader.lock().unwrap();
                    let n = (*guard).read(&mut buf)?;
                    Ok::<(usize, Vec<u8>), std::io::Error>((n, buf[..n].to_vec()))
                })
                .await;
                match read_res {
                    Ok(Ok((0, _))) | Err(_) => break,
                    Ok(Ok((_n, data))) => {
                        if output_tx.send(data).is_err() {
                            break;
                        }
                    }
                    Ok(Err(_)) => break,
                }
            }
            let _ = _child.wait();
        });
    });

    Ok(ExecSession { stdin_tx, output_rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_container_json_lines() {
        let text = r#"
{"ID":"abc123","Names":"web","Image":"nginx","Status":"Up 2 hours","State":"running","Ports":"0.0.0.0:80->80/tcp","CreatedAt":"2024-01-01"}
{"ID":"def456","Names":"db","Image":"postgres","Status":"Exited (0) 3 days ago","State":"exited","Ports":"5432/tcp","CreatedAt":"2024-01-02"}
"#;
        let v: Vec<Container> = parse_json_lines(text).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].names, "web");
        assert_eq!(v[0].state, "running");
        assert_eq!(v[1].state, "exited");
        assert_eq!(v[1].ports, "5432/tcp");
    }

    #[test]
    fn parses_image_json_lines() {
        let text = r#"
{"Repository":"nginx","Tag":"latest","ID":"sha256:abc","Size":"123MB","CreatedSince":"2 weeks ago"}
"#;
        let v: Vec<ImageInfo> = parse_json_lines(text).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].repository, "nginx");
        assert_eq!(v[0].size, "123MB");
    }
}
