//! Native Docker client built on the `bollard` crate.
//!
//! Replaces subprocess calls to the external `docker` binary. The client is
//! configured directly from a Verve `DockerHost` (local socket, `tcp://` URL,
//! TLS certs, or SSH tunnel) — no `docker` CLI on PATH, no env-var shuffling.
//!
//! Mirrors `crate::k8s::client`: this module owns the shared tokio runtime that
//! hosts all Docker operations. The SSH-tunnel runtime (inside `SocketTunnel`)
//! stays separate on purpose — opening/dropping it from within another runtime
//! panics, so it is isolated on its own dedicated runtime.

use std::sync::OnceLock;
use std::time::Duration;

use bollard::{API_DEFAULT_VERSION, Docker};

use super::cli::{DockerError, resolve_ssh_tunnel};
use super::models::DockerHost;

/// Walk an error's full `.source()` chain and join each layer's `Display`.
///
/// bollard/hyper errors nest the real cause (e.g. an `io::Error` like
/// "Connection refused" or a TLS error) several layers deep beneath a generic
/// "ServiceServiceError" surface message. Printing only the top-level Display
/// hides the actionable detail, so callers use this to expose the full chain.
/// (Copied from `crate::k8s::client::format_err_chain` to keep the docker
/// module self-contained.)
pub(crate) fn format_err_chain<E: std::error::Error + ?Sized>(err: &E) -> String {
    let mut parts: Vec<String> = vec![err.to_string()];
    let mut source: Option<&(dyn std::error::Error + 'static)> = err.source();
    while let Some(cause) = source {
        let msg = cause.to_string();
        // Avoid duplicating identical adjacent layers (some wrappers re-print
        // the inner message verbatim).
        if parts
            .last()
            .is_some_and(|last| !msg.is_empty() && last != &msg)
        {
            parts.push(msg);
        }
        source = cause.source();
    }
    parts.join(": ")
}

// ---------------------------------------------------------------------------
// Shared tokio runtime
// ---------------------------------------------------------------------------

/// A single shared multi-threaded tokio runtime for all Docker operations.
///
/// The `bollard::Docker` client (and its streaming operations: logs/exec)
/// must run on a tokio runtime. Sharing one avoids the per-call
/// `Runtime::new()` cost the old subprocess path incurred. The SSH-tunnel
/// runtime (inside `SocketTunnel`) stays separate on purpose — opening/dropping
/// it from within another runtime panics, so it is isolated on dedicated std
/// threads (see `socket_tunnel.rs::SocketTunnel::open`).
pub(crate) fn docker_runtime() -> &'static tokio::runtime::Runtime {
    static DOCKER_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    DOCKER_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build shared docker tokio runtime")
    })
}

// ---------------------------------------------------------------------------
// Client construction
// ---------------------------------------------------------------------------

/// Timeout (seconds) applied to every Docker Engine API request. Matches the
/// old CLI path's `DOCKER_TIMEOUT` (30s) so a hung/unreachable daemon cannot
/// freeze the UI indefinitely — bollard surfaces a timeout error instead.
const DOCKER_TIMEOUT_SECS: u64 = 30;

/// Build a `bollard::Docker` client for the given Verve [`DockerHost`].
///
/// Resolution order:
/// - `None` / local host → auto-detected local socket (`/var/run/docker.sock`
///   on Unix, named pipe on Windows).
/// - SSH-tunnel host → the local Unix socket the [`SocketTunnel`] bound, set up
///   via `connect_with_unix`. The remote daemon is reached by russh + the
///   remote `docker system dial-stdio` helper (see `socket_tunnel.rs`).
/// - `unix://...` → the explicit socket path.
/// - `tcp://` / `http://` → plain HTTP (no TLS).
/// - `https://` → HTTPS via bollard's rustls transport (uses the system root
///   store; custom client certs are not yet wired up — a `tcp://` host with
///   `tls_verify=true` falls back to SSL defaults from `DOCKER_*` env).
///
/// All clients get a [`DOCKER_TIMEOUT_SECS`] request timeout.
///
/// [`SocketTunnel`]: super::socket_tunnel::SocketTunnel
pub(crate) async fn build_client(host: Option<&DockerHost>) -> Result<Docker, DockerError> {
    let docker = match host {
        None => Docker::connect_with_local_defaults(),
        Some(h) if h.is_local() || (h.host.is_empty() && h.ssh_host_id.is_none()) => {
            Docker::connect_with_local_defaults()
        }
        Some(h) if h.uses_ssh_tunnel() => {
            let url = resolve_ssh_tunnel(h)?;
            // url is "unix://<path>"; bollard wants the bare socket path.
            let path = url
                .strip_prefix("unix://")
                .unwrap_or(&url);
            Docker::connect_with_unix(path, DOCKER_TIMEOUT_SECS, API_DEFAULT_VERSION)
        }
        Some(h) => {
            let scheme = h.host.split("://").next().unwrap_or("");
            match scheme {
                "unix" => {
                    let path = h
                        .host
                        .strip_prefix("unix://")
                        .unwrap_or(&h.host);
                    Docker::connect_with_unix(path, DOCKER_TIMEOUT_SECS, API_DEFAULT_VERSION)
                }
                // Plain TCP / HTTP — no TLS.
                "tcp" | "http" => {
                    Docker::connect_with_http(&h.host, DOCKER_TIMEOUT_SECS, API_DEFAULT_VERSION)
                }
                // HTTPS (rustls) — uses the system root store. For a `tcp://`
                // host with tls_verify=true the user typically also sets
                // DOCKER_CERT_PATH; we honor that via the SSL defaults path,
                // which reads DOCKER_HOST/DOCKER_CERT_PATH/DOCKER_TLS_VERIFY.
                "https" => {
                    let _ = &h.cert_path; // custom cert loading is a future improvement
                    Docker::connect_with_ssl_defaults()
                }
                // bollard has no native `ssh://` transport. Users should use the
                // Verve SSH-tunnel host type instead (which uses russh).
                "ssh" => {
                    return Err(DockerError(format!(
                        "bollard 不支持 `ssh://` 传输。请在 Verve 中新建「SSH 隧道」类型的 Docker 主机来连接 {host}",
                        host = h.host
                    )));
                }
                _ => {
                    // Unknown scheme: try treating it as a local socket address.
                    Docker::connect_with_unix(&h.host, DOCKER_TIMEOUT_SECS, API_DEFAULT_VERSION)
                }
            }
        }
    }
    .map_err(|e| DockerError(format!("连接 Docker daemon 失败: {}", format_err_chain(&e))))?;

    // The local/SSL default constructors apply their own default timeout (2min);
    // normalize every path to the shorter UI-friendly timeout.
    Ok(docker.with_timeout(Duration::from_secs(DOCKER_TIMEOUT_SECS)))
}
