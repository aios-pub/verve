//! SSH connection management using `russh`.
//!
//! Handles: connect → authenticate → open PTY shell. Supports jump-host
//! tunneling via `channel_open_direct_tcpip` + `connect_stream`.
//!
//! The connection runs on a dedicated Tokio task. Data from the remote shell
//! is sent through a channel to the terminal model; keystrokes are written
//! back through the russh channel.

use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc;

use super::credentials;
use super::host_store;
use super::models::{SshAuthType, SshHost};

/// Messages from the SSH session to the terminal/UI.
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

/// A live SSH terminal session. Owns the russh handle + channel and drives the
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

/// russh client handler — accepts any server key (TOFU for now).
struct SshHandler;

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO: implement known_hosts verification (TOFU + mismatch warning).
        Ok(true)
    }
}

/// Connect to a host and return a live session with a PTY shell.
///
/// If the host has a `bastion_id`, the connection tunnels through the bastion
/// via `direct-tcpip` + `connect_stream`.
pub async fn connect(host: &SshHost, cols: u32, rows: u32) -> Result<SshSession, String> {
    let config = Arc::new(client::Config::default());

    // Resolve the jump chain (bastions from outermost to target).
    let chain = host_store::jump_chain(host).map_err(|e| format!("跳板机配置错误: {e}"))?;

    // Connect through the chain.
    let handle = if chain.len() == 1 {
        // Direct connection.
        let addr = format!("{}:{}", host.host, host.port);
        client::connect(config.clone(), addr, SshHandler)
            .await
            .map_err(|e| format!("连接失败: {e}"))?
    } else {
        // Multi-hop: connect to first bastion, then tunnel.
        let bastion = &chain[0];
        let bastion_addr = format!("{}:{}", bastion.host, bastion.port);
        let mut bastion_handle = client::connect(config.clone(), bastion_addr, SshHandler)
            .await
            .map_err(|e| format!("连接跳板机失败: {e}"))?;
        authenticate(&mut bastion_handle, bastion).await?;

        // Tunnel through to the target.
        let target = chain.last().unwrap();
        let target_addr = target
            .bastion_target_override
            .as_deref()
            .unwrap_or(&target.host);
        let tunnel = bastion_handle
            .channel_open_direct_tcpip(target_addr, target.port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| format!("跳板机隧道失败: {e}"))?;
        let stream = tunnel.into_stream();
        client::connect_stream(config.clone(), stream, SshHandler)
            .await
            .map_err(|e| format!("通过跳板机连接失败: {e}"))?
    };

    // Authenticate on the final target.
    let mut handle = handle;
    authenticate(&mut handle, host).await?;

    // Open a session channel with PTY.
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("打开会话失败: {e}"))?;

    let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
    let channel = channel;
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
        let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
    });

    Ok(SshSession {
        input_tx,
        resize_tx,
        output_rx,
        _task: task,
    })
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
