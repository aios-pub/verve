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

/// List pods in the given namespace (or "" for the context default).
pub async fn list_pods(namespace: Option<&str>) -> Result<Vec<Pod>, PodError> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("get").arg("pods").arg("-o").arg("json");
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
) -> Result<LogStream, PodError> {
    let mut cmd = Command::new("kubectl");
    cmd.arg("logs")
        .arg("-f")
        .arg("--tail")
        .arg(tail.to_string())
        .arg(pod);
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
) -> Result<PortForwardHandle, PodError> {
    use tokio::io::AsyncBufReadExt as _;
    use tokio::time::timeout;

    let mut cmd = Command::new("kubectl");
    let port_spec = format!("{local_port}:{remote_port}");
    cmd.arg("port-forward")
        .arg(format!("pod/{pod}"))
        .arg(&port_spec);
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

/// Handle for an interactive `kubectl exec` session. After creation the caller
/// takes stdin/stdout/stderr via the Option fields; the child is kept alive so
/// Drop kills kubectl.
pub struct ExecSession {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

/// Spawn `kubectl exec -it pod [-n ns] -- shell` with stdin/stdout piped so the
/// caller can drive an interactive terminal. The shell defaults to `/bin/sh`.
pub fn exec_pod(pod: &str, namespace: Option<&str>, shell: Option<&str>) -> Result<ExecSession, PodError> {
    let shell = shell.unwrap_or("/bin/sh");
    let mut cmd = Command::new("kubectl");
    cmd.arg("exec")
        .arg("-i")
        .arg("-t")
        .arg(pod)
        .arg("--")
        .arg(shell);
    if let Some(ns) = namespace {
        if !ns.is_empty() {
            cmd.args(["-n", ns]);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| PodError(format!("启动 kubectl exec 失败: {e}")))?;
    let stdin = child.stdin.take().ok_or_else(|| PodError("kubectl 未提供 stdin".into()))?;
    let stdout = child.stdout.take().ok_or_else(|| PodError("kubectl 未提供 stdout".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| PodError("kubectl 未提供 stderr".into()))?;
    Ok(ExecSession {
        child,
        stdin: Some(stdin),
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
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
