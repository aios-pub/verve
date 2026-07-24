//! SSH connection management using `russh`.
//!
//! Handles: connect → authenticate → open PTY shell or SFTP subsystem.
//! Supports jump-host tunneling via `channel_open_direct_tcpip` + `connect_stream`.
//!
//! The connection is split into two layers:
//! - [`SshConnection`] — an authenticated handle that can open multiple channels
//!   (shell, SFTP) over the same SSH transport. Cloning is cheap (handle is
//!   wrapped in `Arc<Mutex<…>>`).
//! - [`SshSession`] — a live PTY shell session with input/resize/output channels,
//!   driven on a background Tokio task.
//!
//! The legacy top-level [`connect`] function is preserved as a convenience
//! (it does `SshConnection::connect` + `open_shell`).

use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use tokio::sync::{Mutex, mpsc};

use super::credentials;
use super::host_store;
use super::known_hosts;
use super::models::{SshAuthType, SshHost};
use super::port_forward::LocalForward;
use super::sftp::SftpSessionWrapper;

/// Messages from the SSH session to the terminal/UI.
#[derive(Debug)]
pub enum SshOutput {
    /// Raw data from the remote shell (stdout).
    Data(Vec<u8>),
    /// The session ended (with optional exit code).
    Closed(Option<u32>),
    /// Connection error.
    Error(String),
}

/// A resize command sent to the background task.
#[derive(Clone, Debug)]
pub struct ResizeCmd {
    pub cols: u32,
    pub rows: u32,
}

/// russh client handler with Trust-On-First-Use known_hosts verification.
#[derive(Clone)]
pub(crate) struct SshHandler {
    /// The target host (the inner-most host, after bastion hops). The host
    /// key is stored under `host:port` in the known_hosts file.
    host: String,
    port: u16,
}

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match known_hosts::verify(&self.host, self.port, server_public_key) {
            Ok(known_hosts::VerifyDecision::Trusted) => Ok(true),
            Ok(known_hosts::VerifyDecision::AcceptedNew) => {
                log::info!(
                    "ssh: new host key accepted for {}:{} (fingerprint {})",
                    self.host,
                    self.port,
                    known_hosts::fingerprint(server_public_key)
                );
                Ok(true)
            }
            Ok(known_hosts::VerifyDecision::Mismatch {
                stored_fingerprint: _,
                presented_fingerprint,
            }) => {
                log::error!(
                    "ssh: host key MISMATCH for {}:{} — rejecting connection (presented: {})",
                    self.host,
                    self.port,
                    presented_fingerprint
                );
                Ok(false)
            }
            Err(e) => {
                // If we can't read/write the store, fail open with a warning
                // (matches OpenSSH's behaviour when known_hosts is unwritable).
                log::warn!("ssh: known_hosts verify failed ({e}); accepting key");
                Ok(true)
            }
        }
    }
}

/// An authenticated SSH connection handle.
///
/// Use [`SshConnection::connect`] to establish a connection (handles bastion
/// chaining and authentication), then call [`SshConnection::open_shell`] or
/// [`SshConnection::open_sftp`] to open subsystems. Cloning is cheap — the
/// russh Handle is shared behind an `Arc<Mutex<…>>`, so multiple channels
/// (shell + SFTP) can be opened concurrently over one TCP connection.
#[derive(Clone)]
pub struct SshConnection {
    handle: Arc<Mutex<Handle<SshHandler>>>,
}

impl SshConnection {
    /// Connect to a host and authenticate.
    ///
    /// If the host has a `bastion_id`, the connection tunnels through the
    /// bastion via `direct-tcpip` + `connect_stream`.
    pub async fn connect(host: &SshHost) -> Result<Self, String> {
        let config = Arc::new(client::Config::default());

        // Resolve the jump chain (bastions from outermost to target).
        let chain = host_store::jump_chain(host).map_err(|e| format!("跳板机配置错误: {e}"))?;

        // Connect through the chain.
        let handle = if chain.len() == 1 {
            // Direct connection.
            let addr = format!("{}:{}", host.host, host.port);
            let handler = SshHandler {
                host: host.host.clone(),
                port: host.port,
            };
            client::connect(config.clone(), addr, handler)
                .await
                .map_err(|e| format!("连接失败: {e}"))?
        } else {
            // Multi-hop: connect to first bastion, then tunnel.
            //
            // Note: for the bastion itself we still perform TOFU against the
            // bastion's host/port; the inner (final) target's key is verified
            // by the connect_stream call below against target.host/target.port.
            let bastion = &chain[0];
            let bastion_addr = format!("{}:{}", bastion.host, bastion.port);
            let bastion_handler = SshHandler {
                host: bastion.host.clone(),
                port: bastion.port,
            };
            let mut bastion_handle = client::connect(config.clone(), bastion_addr, bastion_handler)
                .await
                .map_err(|e| format!("连接跳板机失败: {e}"))?;
            authenticate(&mut bastion_handle, bastion).await?;

            // Tunnel through to the target.
            let target = chain.last().unwrap();
            let target_host = target
                .bastion_target_override
                .clone()
                .unwrap_or_else(|| target.host.clone());
            let tunnel = bastion_handle
                .channel_open_direct_tcpip(&target_host, target.port as u32, "127.0.0.1", 0)
                .await
                .map_err(|e| format!("跳板机隧道失败: {e}"))?;
            let stream = tunnel.into_stream();
            let target_handler = SshHandler {
                host: target_host.clone(),
                port: target.port,
            };
            client::connect_stream(config.clone(), stream, target_handler)
                .await
                .map_err(|e| format!("通过跳板机连接失败: {e}"))?
        };

        let mut handle = handle;
        authenticate(&mut handle, host).await?;

        Ok(SshConnection {
            handle: Arc::new(Mutex::new(handle)),
        })
    }

    /// Open a PTY shell session on this connection.
    ///
    /// Returns an [`SshSession`] whose I/O loop runs on a background Tokio
    /// task. Keystrokes are sent via `input_tx`, resize events via `resize_tx`,
    /// and shell output arrives on `output_rx`.
    pub async fn open_shell(&self, cols: u32, rows: u32) -> Result<SshSession, String> {
        let handle = self.handle.lock().await;
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| format!("打开会话失败: {e}"))?;

        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
        channel
            .request_pty(false, &term, cols, rows, 0, 0, &[])
            .await
            .map_err(|e| format!("请求 PTY 失败: {e}"))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| format!("请求 shell 失败: {e}"))?;

        // Set up I/O channels.
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (resize_tx, mut resize_rx) = mpsc::unbounded_channel::<ResizeCmd>();
        let (output_tx, output_rx) = mpsc::unbounded_channel::<SshOutput>();

        let handle_weak = Arc::downgrade(&self.handle);
        let task = tokio::spawn(async move {
            let mut channel = channel;
            loop {
                tokio::select! {
                    // Keystrokes from the user → send to remote.
                    Some(data) = input_rx.recv() => {
                        if !data.is_empty() {
                            if channel.data(&data[..]).await.is_err() {
                                break;
                            }
                        }
                    }
                    // Terminal resize → send window-change to remote.
                    Some(cmd) = resize_rx.recv() => {
                        if let Err(e) = channel.window_change(cmd.cols, cmd.rows, 0, 0).await {
                            log::warn!("window_change failed: {e}");
                        } else {
                            log::info!("window_change sent: {}x{}", cmd.cols, cmd.rows);
                        }
                    }
                    // Events from the remote shell.
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { data }) => {
                                let _ = output_tx.send(SshOutput::Data(data.to_vec()));
                            }
                            Some(ChannelMsg::ExtendedData { data, .. }) => {
                                let _ = output_tx.send(SshOutput::Data(data.to_vec()));
                            }
                            Some(ChannelMsg::ExitStatus { exit_status }) => {
                                let _ = output_tx.send(SshOutput::Closed(Some(exit_status)));
                                break;
                            }
                            Some(ChannelMsg::Eof) | None => {
                                let _ = output_tx.send(SshOutput::Closed(None));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Best-effort disconnect: if other channels (e.g. SFTP) are still
            // open on the same handle, the handle's own Drop will finalize.
            if let Some(handle_arc) = handle_weak.upgrade() {
                if let Ok(h) = handle_arc.try_lock() {
                    let _ = h.disconnect(Disconnect::ByApplication, "", "en").await;
                }
            }
        });

        Ok(SshSession {
            input_tx,
            resize_tx,
            output_rx,
            _task: task,
        })
    }

    /// Open an SFTP subsystem session on this connection.
    ///
    /// Returns a wrapper around `russh_sftp::client::SftpSession` that exposes
    /// the file operations used by the UI (list dir, upload, download, etc.).
    pub async fn open_sftp(&self) -> Result<SftpSessionWrapper, String> {
        let mut handle = self.handle.lock().await;
        SftpSessionWrapper::open(&mut *handle).await
    }

    /// Start a local port forward: bind `local_host:local_port`, bridge each
    /// connection over SSH to `remote_host:remote_port`. The returned handle
    /// owns the listener task; drop it or call [`LocalForward::stop`] to tear
    /// the forward down.
    pub async fn start_local_forward(
        &self,
        local_host: &str,
        local_port: u16,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<LocalForward, String> {
        super::port_forward::start_local(
            self.handle.clone(),
            local_host,
            local_port,
            remote_host,
            remote_port,
        )
        .await
    }

    /// Expose the underlying handle for advanced use (callers must not
    /// disconnect while other channels are active).
    pub(crate) fn handle(&self) -> Arc<Mutex<Handle<SshHandler>>> {
        self.handle.clone()
    }
}

/// A live SSH terminal session. Owns the russh channel and drives the
/// I/O loop on a background Tokio task.
pub struct SshSession {
    /// Sender for writing keystrokes to the remote shell.
    pub input_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Sender for terminal resize notifications.
    pub resize_tx: mpsc::UnboundedSender<ResizeCmd>,
    /// Receiver for output from the remote shell.
    pub output_rx: mpsc::UnboundedReceiver<SshOutput>,
    /// The join handle for the background task (drop to cancel).
    _task: tokio::task::JoinHandle<()>,
}

/// Connect to a host and return a live session with a PTY shell.
///
/// Convenience wrapper: equivalent to `SshConnection::connect(host).await?.open_shell(cols, rows)`.
/// Preserved for backward compatibility with existing callers.
pub async fn connect(host: &SshHost, cols: u32, rows: u32) -> Result<SshSession, String> {
    let conn = SshConnection::connect(host).await?;
    conn.open_shell(cols, rows).await
}

/// Authenticate on a russh handle using the host's configured method.
async fn authenticate(handle: &mut Handle<SshHandler>, host: &SshHost) -> Result<(), String> {
    let ok = match host.auth_type {
        SshAuthType::Password => {
            let password = credentials::load_keychain(&host.id, "password").unwrap_or_default();
            handle
                .authenticate_password(&host.username, password)
                .await
                .map_err(|e| format!("认证失败: {e}"))?
                .success()
        }
        SshAuthType::PrivateKey => {
            let key_path = host.key_path.as_deref().ok_or("未配置密钥路径")?;
            let passphrase = credentials::load_keychain(&host.id, "key_passphrase").ok();
            let key = load_secret_key(key_path, passphrase.as_deref())
                .map_err(|e| format!("加载密钥失败: {e}"))?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| format!("RSA hash: {e}"))?
                .flatten();
            handle
                .authenticate_publickey(
                    &host.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(|e| format!("密钥认证失败: {e}"))?
                .success()
        }
        SshAuthType::Agent => {
            // Agent auth not yet implemented.
            return Err("Agent 认证暂不支持".into());
        }
    };

    if !ok {
        return Err("认证失败：用户名/密码/密钥错误".into());
    }
    Ok(())
}
