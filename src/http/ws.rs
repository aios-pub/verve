//! WebSocket client.
//!
//! Runs the connection on a dedicated tokio runtime (GPUI's executor is
//! smol-based, so we keep tokio isolated here). Messages flow back through an
//! `smol::channel` that the GPUI side drains in a `cx.spawn` loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// (no extra anyhow import needed: connect_async errors are stringified)
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

use crate::state::models::Response;

/// One item the background WS task emits to the UI.
pub enum WsFrame {
    /// A received text/binary message.
    Message(String),
    /// The connection state changed (e.g. "已连接").
    Status(String),
    /// The stream ended (with an optional error message).
    Done(Option<String>),
}

/// A live WebSocket connection handle. Dropping it aborts the runtime task.
pub struct WsConnection {
    /// Sender for outgoing messages.
    pub tx: smol::channel::Sender<String>,
    /// Receiver for incoming frames (messages/status/done).
    pub frames: smol::channel::Receiver<WsFrame>,
    /// Stop flag shared with the background task.
    stop: Arc<AtomicBool>,
    _join: Option<std::thread::JoinHandle<()>>,
}

impl WsConnection {
    /// Request the background task to disconnect.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for WsConnection {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Lazily-initialized dedicated tokio runtime for WebSocket work.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .thread_name("verve-ws")
            .build()
            .expect("build ws runtime")
    })
}

/// Connect to a WebSocket URL and return a live connection handle. Messages
/// arrive on `frames`; send outgoing text via `tx`.
pub fn connect(url: &str) -> WsConnection {
    let stop = Arc::new(AtomicBool::new(false));
    let (frame_tx, frame_rx) = smol::channel::bounded::<WsFrame>(256);
    let (msg_tx, msg_rx) = smol::channel::bounded::<String>(64);
    let url = url.to_string();
    let stop_for_task = stop.clone();

    let join = runtime().spawn(async move {
        // Announce connecting.
        let _ = frame_tx.send(WsFrame::Status("正在连接…".into())).await;
        // connect_async runs on this tokio runtime directly.
        let ws_stream = match tokio_tungstenite::connect_async(url.clone()).await {
            Ok((stream, _resp)) => {
                let _ = frame_tx.send(WsFrame::Status("已连接".into())).await;
                stream
            }
            Err(e) => {
                let _ = frame_tx
                    .send(WsFrame::Done(Some(format!("连接失败：{e}"))))
                    .await;
                return;
            }
        };
        let (mut write, mut read) = ws_stream.split();

        // Outgoing pump: read from msg channel, write to socket.
        let out_pump = async {
            while let Ok(text) = msg_rx.recv().await {
                if stop_for_task.load(Ordering::SeqCst) {
                    break;
                }
                if write.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            let _ = write.close().await;
        };

        // Incoming pump: read from socket, forward frames.
        let in_pump = async {
            while let Some(msg) = read.next().await {
                if stop_for_task.load(Ordering::SeqCst) {
                    break;
                }
                match msg {
                    Ok(Message::Text(t)) => {
                        if frame_tx.send(WsFrame::Message(t)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Binary(b)) => {
                        let text = String::from_utf8_lossy(&b).to_string();
                        if frame_tx.send(WsFrame::Message(text)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) => {
                        let _ = frame_tx.send(WsFrame::Done(None)).await;
                        break;
                    }
                    Ok(_) => {} // ping/pong
                    Err(e) => {
                        let _ = frame_tx
                            .send(WsFrame::Done(Some(format!("读取错误：{e}"))))
                            .await;
                        break;
                    }
                }
            }
        };

        futures::future::select(Box::pin(out_pump), Box::pin(in_pump)).await;
        let _ = frame_tx.send(WsFrame::Done(None)).await;
    });

    // The tokio task runs on the dedicated runtime; dropping WsConnection sets
    // the stop flag and closes the channels, which ends the pumps. We let the
    // join handle detach.
    drop(join);
    WsConnection {
        tx: msg_tx,
        frames: frame_rx,
        stop,
        _join: None,
    }
}

/// Build a "message log" response body line for the UI.
pub fn format_message(direction: &str, text: &str) -> String {
    format!("[{direction}] {text}")
}

/// A terminal response shell when WS fails to even start.
pub fn error_response(msg: impl Into<String>) -> Response {
    Response {
        status: 0,
        status_text: "WebSocket".into(),
        error: Some(msg.into()),
        streaming: false,
        ..Default::default()
    }
}
