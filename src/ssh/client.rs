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
    /// Connection error, with a flag telling the UI whether it is worth
    /// auto-retrying. Authentication / configuration errors set
    /// `retryable = false` so the UI can stop retrying and prompt the user to
    /// fix their settings instead of hammering the server 5 times.
    Error { message: String, retryable: bool },
}

/// A classified SSH error: a human-readable `message` plus a `retryable` hint.
///
/// `retryable == false` means the error is a configuration problem (bad
/// credentials, wrong port, host-key mismatch, …) that will not resolve by
/// retrying, so the caller should surface it to the user and stop.
#[derive(Clone, Debug)]
pub struct SshError {
    pub message: String,
    pub retryable: bool,
}

impl SshError {
    fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            message: message.into(),
            retryable,
        }
    }

    /// Build an [`SshError`] from a low-level [`russh::Error`], classifying it
    /// into a friendly Chinese message and a retryable hint. `where_` is a
    /// short description of where the error happened (e.g. `"连接"`, `"跳板机"`).
    fn from_russh(e: russh::Error, where_: &str, target_host: &str) -> Self {
        match e {
            // Host key mismatch (check_server_key returned false).
            russh::Error::UnknownKey => Self::new(
                "服务器主机密钥与本地已保存的不一致，可能存在中间人攻击，已拒绝连接",
                false,
            ),
            // TCP / IO level errors — classify by io::ErrorKind.
            russh::Error::IO(io) => Self::from_io(io, where_, target_host),
            russh::Error::ConnectionTimeout
            | russh::Error::KeepaliveTimeout
            | russh::Error::InactivityTimeout => {
                Self::new(format!("{where_}超时，请检查网络或主机地址是否可达"), true)
            }
            russh::Error::HUP | russh::Error::Disconnect => {
                Self::new(format!("{where_}被对端关闭"), true)
            }
            other => Self::new(format!("{where_}失败: {other}"), true),
        }
    }

    /// Classify a [`std::io::Error`] from the connection dial.
    fn from_io(io: std::io::Error, where_: &str, target_host: &str) -> Self {
        use std::io::ErrorKind;
        match io.kind() {
            // A refused connection almost always means "nothing is listening
            // on that port" — i.e. the port is wrong. Not retryable.
            ErrorKind::ConnectionRefused => Self::new(
                "连接被拒绝：目标端口上没有服务，请确认端口号是否正确、SSH 服务是否已启动",
                false,
            ),
            // Network unreachable / no route. If the target is a local-network
            // address (192.168/10./172.16-31/localhost), on macOS this is the
            // signature of the Local Network privacy permission being missing
            // or denied — the kernel surfaces the block as ENETUNREACH rather
            // than a permission error, even though `ssh` from the shell works.
            // Give the user the actionable hint instead of the generic message.
            ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable => {
                if is_local_network_host(target_host) && cfg!(target_os = "macos") {
                    Self::new(
                        "无法访问本地网络主机。这通常是 macOS「本地网络」隐私权限未授予：\n\
                         请打开 系统设置 → 隐私与安全性 → 本地网络，允许 Verve 访问，然后重试。\n\
                         （若列表中无 Verve，请使用打包后的 .app 重新启动以触发授权弹窗）",
                        true,
                    )
                } else {
                    Self::new("网络不可达，请检查主机地址或网络连接", true)
                }
            }
            ErrorKind::TimedOut => {
                Self::new("连接超时，请检查主机地址是否正确、网络是否可达", true)
            }
            ErrorKind::NotFound => Self::new("无法解析主机地址，请检查主机名是否正确", false),
            ErrorKind::PermissionDenied => Self::new(format!("{where_}被拒绝（权限不足）"), false),
            other => Self::new(format!("{where_}失败: {other}"), true),
        }
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SshError {}

/// Return true if `host` is a local-network address (RFC 1918 / loopback /
/// link-local). On macOS, connecting to these requires the Local Network
/// privacy permission; without it the kernel blocks the TCP connection and
/// reports it as `ENETUNREACH`/`EHOSTUNREACH`.
///
/// Only literal IP addresses are matched — hostnames can't be classified
/// cheaply, so they fall through to the generic message (and would normally
/// resolve to a public address anyway).
fn is_local_network_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('%');
    // Strip a trailing zone id / interface on IPv6 link-local (fe80::…%en0).
    let host = host.split('%').next().unwrap_or(host);
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return true;
    }
    // Try to parse as an IPv4/IPv6 literal and check for private ranges.
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_broadcast()
            }
            std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unicast_link_local(),
        };
    }
    // `.local` hostnames (mDNS / Bonjour) are also local-network.
    host.ends_with(".local") || host == "local"
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
    pub async fn connect(host: &SshHost) -> Result<Self, SshError> {
        let config = Arc::new(client::Config::default());

        // Resolve the jump chain (bastions from outermost to target).
        let chain = host_store::jump_chain(host)
            .map_err(|e| SshError::new(format!("跳板机配置错误: {e}"), false))?;

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
                .map_err(|e| SshError::from_russh(e, "连接", &host.host))?
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
                .map_err(|e| SshError::from_russh(e, "连接跳板机", &bastion.host))?;
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
                .map_err(|e| SshError::from_russh(e, "跳板机隧道", &target_host))?;
            let stream = tunnel.into_stream();
            let target_handler = SshHandler {
                host: target_host.clone(),
                port: target.port,
            };
            client::connect_stream(config.clone(), stream, target_handler)
                .await
                .map_err(|e| SshError::from_russh(e, "通过跳板机连接", &target_host))?
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
    pub async fn open_shell(&self, cols: u32, rows: u32) -> Result<SshSession, SshError> {
        let handle = self.handle.lock().await;
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| SshError::from_russh(e, "打开会话", ""))?;

        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
        channel
            .request_pty(false, &term, cols, rows, 0, 0, &[])
            .await
            .map_err(|e| SshError::from_russh(e, "请求 PTY", ""))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| SshError::from_russh(e, "请求 shell", ""))?;

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
    let conn = SshConnection::connect(host).await.map_err(|e| e.message)?;
    conn.open_shell(cols, rows).await.map_err(|e| e.message)
}

/// Authenticate on a russh handle using the host's configured method.
async fn authenticate(handle: &mut Handle<SshHandler>, host: &SshHost) -> Result<(), SshError> {
    let ok = match host.auth_type {
        SshAuthType::Password => {
            let password = credentials::load_secret(&host.id, "password").unwrap_or_default();
            let res = handle
                .authenticate_password(&host.username, password)
                .await
                .map_err(|e| SshError::from_russh(e, "认证", &host.host))?;
            if !res.success() {
                // Server reached, but credentials rejected — not retryable.
                return Err(SshError::new(
                    "认证失败：用户名或密码错误，请检查账号配置",
                    false,
                ));
            }
            true
        }
        SshAuthType::PrivateKey => {
            let key_path = host
                .key_path
                .as_deref()
                .ok_or_else(|| SshError::new("未配置密钥路径", false))?;
            let passphrase = credentials::load_secret(&host.id, "key_passphrase");
            let key = load_secret_key(key_path, passphrase.as_deref()).map_err(|e| {
                SshError::new(format!("加载私钥失败（路径 {key_path}）: {e}"), false)
            })?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| SshError::from_russh(e, "RSA 哈希协商", &host.host))?
                .flatten();
            let res = handle
                .authenticate_publickey(
                    &host.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(|e| SshError::from_russh(e, "密钥认证", &host.host))?;
            if !res.success() {
                return Err(SshError::new(
                    "认证失败：密钥被服务器拒绝，请检查用户名或私钥是否正确",
                    false,
                ));
            }
            true
        }
        SshAuthType::Agent => {
            // Agent auth not yet implemented.
            return Err(SshError::new("Agent 认证暂不支持", false));
        }
    };

    if !ok {
        return Err(SshError::new("认证失败：用户名/密码/密钥错误", false));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_local_network_host;

    #[test]
    fn detects_rfc1918_local_addresses() {
        // Private ranges — these require the macOS Local Network permission.
        assert!(is_local_network_host("192.168.1.17"));
        assert!(is_local_network_host("192.168.0.1"));
        assert!(is_local_network_host("10.0.0.5"));
        assert!(is_local_network_host("10.255.255.255"));
        assert!(is_local_network_host("172.16.0.1"));
        assert!(is_local_network_host("172.31.255.255"));
    }

    #[test]
    fn detects_loopback_and_link_local() {
        assert!(is_local_network_host("127.0.0.1"));
        assert!(is_local_network_host("localhost"));
        assert!(is_local_network_host("::1"));
        // IPv4 link-local (APIPA).
        assert!(is_local_network_host("169.254.1.1"));
    }

    #[test]
    fn does_not_flag_public_addresses() {
        assert!(!is_local_network_host("8.8.8.8"));
        assert!(!is_local_network_host("1.1.1.1"));
        assert!(!is_local_network_host("203.0.113.5"));
        // 172.32.x.x is OUTSIDE the private range.
        assert!(!is_local_network_host("172.32.0.1"));
    }

    #[test]
    fn handles_mdns_local_suffix() {
        assert!(is_local_network_host("myhost.local"));
        assert!(!is_local_network_host("example.com"));
    }

    #[test]
    fn strips_ipv6_zone_id() {
        // fe80::… link-local with a zone id still recognised.
        assert!(is_local_network_host("fe80::1%en0"));
    }
}
