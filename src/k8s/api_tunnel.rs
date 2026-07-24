//! Kubernetes API server over SSH tunnel.
//!
//! [`ApiTunnel`] establishes a local TCP port forward over SSH to a remote
//! Kubernetes API server (typically port 6443). It mirrors the architecture of
//! [`crate::docker::socket_tunnel`]: a dedicated Tokio runtime drives the accept
//! loop, an authenticated [`SshConnection`] is kept alive for the tunnel lifetime,
//! and the local port is automatically selected (OS-assigned free port).
//!
//! Unlike the Docker tunnel (which bridges to a Unix socket via `docker system
//! dial-stdio`), the K8s tunnel uses standard SSH local port forwarding
//! (`direct-tcpip` channel) — the SSH server connects directly to the remote
//! API server address, no remote helper command required.
//!
//! Lifecycle:
//! - One tunnel per active K8s-cluster+SSH-host pair (managed by the caller via
//!   a global cache, see [`crate::k8s::kubectl`]).
//! - Drop the [`ApiTunnel`] (or call [`ApiTunnel::close`]) to stop listening and
//!   disconnect the SSH transport.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::ssh::client::SshConnection;
use crate::ssh::host_store as ssh_host_store;
use crate::ssh::models::SshHost;
use crate::ssh::port_forward::LocalForward;

/// An active Kubernetes API server SSH tunnel. Owns:
/// - the dedicated Tokio runtime that drives the port forward
/// - the authenticated SSH connection (kept alive for the tunnel lifetime)
/// - the local forward handle (bound local address + background task)
///
/// Dropping the handle stops the forward and shuts down the runtime.
pub struct ApiTunnel {
    /// Local address the forward is listening on (e.g. "127.0.0.1:54321").
    pub bound_addr: String,
    /// Local port number.
    pub local_port: u16,
    _rt: Runtime,
    _conn: SshConnection,
    _forward: LocalForward,
}

impl Drop for ApiTunnel {
    fn drop(&mut self) {
        log::info!("k8s tunnel: closed on {}", self.bound_addr);
    }
}

impl ApiTunnel {
    /// Parse the remote host and port from a kubeconfig server URL.
    ///
    /// Extracts host:port from URLs like:
    /// - `https://127.0.0.1:6443` → ("127.0.0.1", 6443)
    /// - `https://kubernetes.default.svc:443` → ("kubernetes.default.svc", 443)
    /// - `https://10.0.0.1` (no port) → ("10.0.0.1", 6443, default K8s port)
    pub fn parse_server_url(server: &str) -> Result<(String, u16), String> {
        let url = url::Url::parse(server).map_err(|e| format!("解析 API server URL 失败: {e}"))?;
        let host = url.host_str().ok_or("URL 缺少 host")?.to_string();
        let port = url.port().unwrap_or(6443);
        Ok((host, port))
    }

    /// Open an SSH connection to `ssh_host` and start forwarding a local
    /// TCP port to the remote Kubernetes API server specified by `server_url`.
    ///
    /// The local port is automatically assigned by the OS (pass 0).
    pub fn open(ssh_host: &SshHost, server_url: &str) -> Result<Self, String> {
        let (remote_host, remote_port) = Self::parse_server_url(server_url)?;
        log::info!(
            "k8s tunnel: connecting to {}@{}:{}, forwarding to {}:{}",
            ssh_host.username,
            ssh_host.host,
            ssh_host.port,
            remote_host,
            remote_port
        );

        // Build a dedicated multi-threaded Tokio runtime for this tunnel.
        let rt = Runtime::new().map_err(|e| format!("初始化 tokio runtime 失败: {e}"))?;

        let remote_host_clone = remote_host.clone();
        let (conn, forward, local_port) = rt.block_on(async move {
            // Establish SSH connection.
            let conn = SshConnection::connect(ssh_host)
                .await
                .map_err(|e| format!("SSH 连接失败: {e}"))?;

            // Start local port forward: 127.0.0.1:0 → remote_host:remote_port.
            // Port 0 lets the OS pick a free port.
            let forward = conn
                .start_local_forward("127.0.0.1", 0, &remote_host_clone, remote_port)
                .await
                .map_err(|e| format!("启动端口转发失败: {e}"))?;

            // Extract the assigned local port from the bound address.
            let bound_addr: SocketAddr = forward
                .bound_addr
                .parse()
                .map_err(|e| format!("解析绑定地址失败: {e}"))?;
            let local_port = bound_addr.port();

            Ok::<(SshConnection, LocalForward, u16), String>((conn, forward, local_port))
        })?;

        let bound_addr = format!("127.0.0.1:{local_port}");
        log::info!(
            "k8s tunnel: listening on {} → {}:{}",
            bound_addr,
            remote_host,
            remote_port
        );

        Ok(ApiTunnel {
            bound_addr,
            local_port,
            _rt: rt,
            _conn: conn,
            _forward: forward,
        })
    }

    /// Return the local HTTPS URL to use as kubeconfig server override.
    pub fn local_server_url(&self) -> String {
        format!("https://{}", self.bound_addr)
    }
}

/// Look up an SSH host by id from Verve's persisted store.
pub fn find_ssh_host(id: &str) -> Option<SshHost> {
    ssh_host_store::find_host(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_url_with_port() {
        let (host, port) = ApiTunnel::parse_server_url("https://127.0.0.1:6443").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 6443);
    }

    #[test]
    fn parses_server_url_without_port_defaults_to_6443() {
        let (host, port) = ApiTunnel::parse_server_url("https://kubernetes.default.svc").unwrap();
        assert_eq!(host, "kubernetes.default.svc");
        assert_eq!(port, 6443);
    }

    #[test]
    fn parses_server_url_with_custom_port() {
        let (host, port) = ApiTunnel::parse_server_url("https://k8s.example.com:8443").unwrap();
        assert_eq!(host, "k8s.example.com");
        assert_eq!(port, 8443);
    }

    #[test]
    fn rejects_invalid_url() {
        assert!(ApiTunnel::parse_server_url("not-a-url").is_err());
    }
}
