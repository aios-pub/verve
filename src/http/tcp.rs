//! TCP client — raw socket connect/send/recv using a dedicated tokio runtime.
//!
//! The connection runs on a tokio runtime (shared with WebSocket). Messages
//! flow back via an smol channel that the GPUI side drains in a `cx.spawn` loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::state::models::Response;

/// One item the background TCP task emits to the UI.
pub enum TcpFrame {
    /// Received data (as lossy UTF-8 string).
    Data(String),
    /// Connection state changed.
    Status(String),
    /// Stream ended (optional error).
    Done(Option<String>),
}

/// A live TCP connection handle.
pub struct TcpConnection {
    /// Sender for outgoing data.
    pub tx: smol::channel::Sender<String>,
    /// Receiver for incoming frames.
    pub frames: smol::channel::Receiver<TcpFrame>,
    stop: Arc<AtomicBool>,
}

impl TcpConnection {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for TcpConnection {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Connect to a TCP endpoint (host:port parsed from `addr`). Spawns a task on
/// the dedicated tokio runtime that reads/writes until stopped or disconnected.
pub fn connect(addr: &str) -> TcpConnection {
    let stop = Arc::new(AtomicBool::new(false));
    let (frame_tx, frame_rx) = smol::channel::bounded::<TcpFrame>(256);
    let (msg_tx, msg_rx) = smol::channel::bounded::<String>(64);
    let addr = addr.to_string();
    let stop_for_task = stop.clone();

    let handle = crate::http::ws::runtime().spawn(async move {
        let _ = frame_tx.send(TcpFrame::Status("正在连接…".into())).await;

        // Parse host:port from the URL-like addr.
        let target = addr
            .strip_prefix("tcp://")
            .or_else(|| addr.strip_prefix("://"))
            .unwrap_or(&addr);
        let target = target.split('?').next().unwrap_or(target);

        let stream = match TcpStream::connect(target).await {
            Ok(s) => {
                let _ = frame_tx
                    .send(TcpFrame::Status(format!("已连接 → {target}")))
                    .await;
                s
            }
            Err(e) => {
                let _ = frame_tx
                    .send(TcpFrame::Done(Some(format!("连接失败：{e}"))))
                    .await;
                return;
            }
        };
        // Split into read/write halves so both pumps can run concurrently.
        let (mut read_half, mut write_half) = stream.into_split();

        // Incoming pump.
        let in_pump = async {
            let mut buf = [0u8; 4096];
            loop {
                if stop_for_task.load(Ordering::SeqCst) {
                    break;
                }
                match read_half.read(&mut buf).await {
                    Ok(0) => {
                        let _ = frame_tx.send(TcpFrame::Done(None)).await;
                        break;
                    }
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        if frame_tx.send(TcpFrame::Data(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = frame_tx
                            .send(TcpFrame::Done(Some(format!("读取错误：{e}"))))
                            .await;
                        break;
                    }
                }
            }
        };

        // Outgoing pump.
        let out_pump = async {
            while let Ok(data) = msg_rx.recv().await {
                if stop_for_task.load(Ordering::SeqCst) {
                    break;
                }
                let bytes = if let Some(stripped) = data.strip_prefix("0x") {
                    match hex_decode(stripped) {
                        Ok(b) => b,
                        Err(_) => data.into_bytes(),
                    }
                } else {
                    data.into_bytes()
                };
                if write_half.write_all(&bytes).await.is_err() {
                    break;
                }
                let _ = write_half.flush().await;
            }
            let _ = write_half.shutdown().await;
        };

        futures::future::select(Box::pin(in_pump), Box::pin(out_pump)).await;
        let _ = frame_tx.send(TcpFrame::Done(None)).await;
    });

    drop(handle);

    TcpConnection {
        tx: msg_tx,
        frames: frame_rx,
        stop,
    }
}

/// Decode a hex string into bytes.
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// A terminal response shell when TCP fails immediately.
pub fn error_response(msg: impl Into<String>) -> Response {
    Response {
        status: 0,
        status_text: "TCP".into(),
        error: Some(msg.into()),
        ..Default::default()
    }
}
