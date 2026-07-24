//! SSH local port forwarding.
//!
//! A [`LocalForward`] binds a local TCP listener and bridges every accepted
//! connection over SSH via `channel_open_direct_tcpip`. Data is copied
//! bidirectionally until either side closes. The forward runs on the caller's
//! Tokio runtime (the SSH panel already spawns a dedicated runtime per
//! connection), so no additional runtime setup is required.
//!
//! Remote (`-R`) forwarding is not implemented in v0.2 — it requires the
//! client handler to answer server-side `direct-tcpip` channel requests, which
//! is a larger refactor of `SshHandler`. Local forwarding covers the dominant
//! "expose an internal service to the API tester" use case.

use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use super::client::SshHandler;

/// Handle returned by [`start_local`]. Dropping the handle (or calling
/// [`LocalForward::stop`]) stops accepting and aborts in-flight bridges.
pub struct LocalForward {
    /// Bound local address (useful when `local_port` was 0 = OS-assigned).
    pub bound_addr: String,
    stop_tx: mpsc::UnboundedSender<()>,
    task: JoinHandle<()>,
}

impl LocalForward {
    /// Stop the forward and tear down all bridges.
    pub fn stop(self) {
        let _ = self.stop_tx.send(());
        self.task.abort();
    }
}

/// Start a local forward: accept on `local_host:local_port`, bridge over SSH to
/// `remote_host:remote_port`.
///
/// `handle` is the russh client handle (same `Arc<Mutex<Handle<…>>>` owned by
/// `SshConnection`). Forwarding shares the SSH transport with shell/SFTP
/// channels concurrently — russh multiplexes them transparently.
pub(crate) async fn start_local(
    handle: Arc<Mutex<russh::client::Handle<SshHandler>>>,
    local_host: &str,
    local_port: u16,
    remote_host: &str,
    remote_port: u16,
) -> Result<LocalForward, String> {
    let bind = format!("{local_host}:{local_port}");
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| format!("本地监听失败 {bind}: {e}"))?;
    let bound_addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| bind.clone());

    let (stop_tx, mut stop_rx) = mpsc::unbounded_channel::<()>();

    let remote_host = remote_host.to_string();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop_rx.recv() => break,
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, peer)) => {
                            log::info!("ssh forward: accepted {peer}");
                            let handle = handle.clone();
                            let remote_host = remote_host.clone();
                            tokio::spawn(async move {
                                if let Err(e) = bridge_one(handle, stream, &remote_host, remote_port).await {
                                    log::warn!("ssh forward bridge error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            log::warn!("ssh forward accept error: {e}");
                            break;
                        }
                    }
                }
            }
        }
    });

    Ok(LocalForward {
        bound_addr,
        stop_tx,
        task,
    })
}

/// Establish one direct-tcpip channel for an inbound local connection and
/// bidirectionally copy bytes until EOF.
async fn bridge_one(
    handle: Arc<Mutex<russh::client::Handle<SshHandler>>>,
    mut stream: TcpStream,
    remote_host: &str,
    remote_port: u16,
) -> Result<(), String> {
    // Open the direct-tcpip channel on the SSH transport.
    let channel = {
        let h = handle.lock().await;
        h.channel_open_direct_tcpip(remote_host, remote_port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| format!("open direct-tcpip: {e}"))?
    };

    let mut ssh_stream = channel.into_stream();
    tokio::io::copy_bidirectional(&mut ssh_stream, &mut stream)
        .await
        .map_err(|e| format!("copy: {e}"))?;
    Ok(())
}
