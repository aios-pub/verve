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
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

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

/// Build a `kubectl` Command with the merged KUBECONFIG env var set.
fn kubectl_command() -> Command {
    let mut cmd = Command::new("kubectl");
    cmd.env("KUBECONFIG", crate::k8s::clusters::merged_kubeconfig_env());
    cmd
}

/// List pods in the given namespace (or "" for the context default).
pub async fn list_pods(namespace: Option<&str>, context: Option<&str>) -> Result<Vec<Pod>, PodError> {
    let mut cmd = kubectl_command();
    cmd.arg("get").arg("pods").arg("-o").arg("json");
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
    let mut cmd = kubectl_command();
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

    let mut cmd = kubectl_command();
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

    let kubeconfig = crate::k8s::clusters::merged_kubeconfig_env();
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

/// Spawn `kubectl exec -it pod [-n ns] [--context ctx] -- shell` attached to a PTY
/// so that Tab completion, arrow keys, and other terminal features work. When no
/// shell is requested, bash is tried first and sh is used as a fallback for pods
/// that don't ship bash (Alpine, distroless, minimal images).
pub fn exec_pod(pod: &str, namespace: Option<&str>, shell: Option<&str>, context: Option<&str>) -> Result<ExecSession, PodError> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};

    // Caller-requested shell wins; otherwise auto-detect: prefer bash, fall back to sh.
    let resolved_shell = match shell {
        Some(s) => s.to_string(),
        None => {
            if shell_available(pod, namespace, context, "bash") {
                "bash".to_string()
            } else {
                "sh".to_string()
            }
        }
    };
    let shell = resolved_shell.as_str();

    let kubeconfig = crate::k8s::clusters::merged_kubeconfig_env();
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
    let mut pair = pty_system
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
