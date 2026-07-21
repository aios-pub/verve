//! Helpers that shell out to the `docker` CLI.
//!
//! Mirrors `crate::k8s::kubectl`: parse JSON from `--format '{{json .}}'`, stream
//! logs via a spawned child, and spawn an interactive exec session.

use std::path::PathBuf;
use std::process::Stdio;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::DockerHost;

/// Resolve the docker CLI absolute path, falling back to PATH lookup. On macOS
/// Docker Desktop installs docker to a handful of well-known locations that
/// aren't always in the PATH when Verve is launched from Finder / Dock.
fn docker_binary() -> PathBuf {
    let mut candidates = vec![
        // Apple Silicon Homebrew
        PathBuf::from("/opt/homebrew/bin/docker"),
        // Intel Homebrew
        PathBuf::from("/usr/local/bin/docker"),
        // Docker Desktop bundled CLI
        PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
        // Rancher Desktop
        PathBuf::from("/opt/homebrew/bin/nerdctl"),
        PathBuf::from("/usr/local/bin/nerdctl"),
    ];
    // ~/.docker/bin (user-installed, rare)
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".docker/bin/docker"));
    }
    for p in &candidates {
        if p.exists() {
            return p.clone();
        }
    }
    // Fall back to PATH resolution (returns "docker" if not found — let the
    // OS surface the "not found" error when spawn runs).
    PathBuf::from("docker")
}

/// Apply Docker host environment variables (DOCKER_HOST, DOCKER_TLS_VERIFY,
/// DOCKER_CERT_PATH) to a tokio Command when a non-local host is configured.
fn apply_host_env(cmd: &mut Command, host: Option<&DockerHost>) {
    if let Some(h) = host {
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
}

/// Apply Docker host environment variables to a std (sync) Command.
fn apply_host_env_sync(cmd: &mut std::process::Command, host: Option<&DockerHost>) {
    if let Some(h) = host {
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
}

/// Build a Command targeting the docker binary, extending PATH with common
/// macOS locations so subcommands can find sibling helpers like `docker-credential-osxkeychain`.
fn docker_command(host: Option<&DockerHost>) -> Command {
    let bin = docker_binary();
    let mut cmd = Command::new(&bin);
    // Ensure Docker Desktop's bin dir and Homebrew are on PATH even when launched
    // from a GUI context without a shell profile.
    cmd.env(
        "PATH",
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/Applications/Docker.app/Contents/Resources/bin",
            "/usr/bin",
            "/bin",
            std::env::var("PATH").unwrap_or_default().as_str(),
        ]
        .join(":"),
    );
    apply_host_env(&mut cmd, host);
    cmd
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

/// Handle for an interactive `docker exec -it` session.
pub struct ExecSession {
    pub child: Child,
    pub stdin: Option<tokio::process::ChildStdin>,
    pub stdout: Option<tokio::process::ChildStdout>,
    pub stderr: Option<tokio::process::ChildStderr>,
}

/// Build a std::process::Command for blocking docker calls with extended PATH.
fn docker_command_sync(host: Option<&DockerHost>) -> std::process::Command {
    let bin = docker_binary();
    let mut cmd = std::process::Command::new(&bin);
    cmd.env("PATH", extended_path());
    apply_host_env_sync(&mut cmd, host);
    cmd
}

fn run_docker(host: Option<&DockerHost>, args: &[&str]) -> Result<std::process::Output, DockerError> {
    docker_command_sync(host)
        .args(args)
        .output()
        .map_err(|e| {
            DockerError(format!(
                "docker 执行失败: {e}; 请确认 Docker daemon 正在运行且 docker CLI 已安装"
            ))
        })
}

pub async fn run_docker_async(host: Option<&DockerHost>, args: &[&str]) -> Result<std::process::Output, DockerError> {
    docker_command(host)
        .args(args)
        .output()
        .await
        .map_err(|e| DockerError(format!("docker 执行失败: {e}")))
}

fn extended_path() -> String {
    [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/Applications/Docker.app/Contents/Resources/bin",
        "/usr/bin",
        "/bin",
        std::env::var("PATH").unwrap_or_default().as_str(),
    ]
    .join(":")
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
    let mut cmd = docker_command(host);
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

/// Spawn `docker exec -it <id> sh` for an interactive session.
pub fn exec_container(host: Option<&DockerHost>, id: &str, shell: Option<&str>) -> Result<ExecSession, DockerError> {
    let shell = shell.unwrap_or("sh");
    let mut cmd = docker_command(host);
    cmd.arg("exec")
        .arg("-i")
        .arg("-t")
        .arg(id)
        .arg(shell);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| DockerError(format!("启动 docker exec 失败: {e}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| DockerError("docker 未提供 stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DockerError("docker 未提供 stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| DockerError("docker 未提供 stderr".into()))?;
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
