//! HTTP forward proxy server.
//!
//! Listens on 127.0.0.1, accepts absolute-form HTTP requests, forwards them using
//! a shared reqwest client, and records each exchange in the [`CaptureStore`].
//! CONNECT / HTTPS is rejected with 501 (MITM support deferred).

use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use reqwest::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use super::capture::{CaptureEntry, CaptureStore};

pub const DEFAULT_PORT: u16 = 3060;

/// Handle returned by [`serve`]; dropping/stopping it aborts the listener.
pub struct ProxyHandle {
    pub bound_port: u16,
    stop_tx: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyHandle {
    pub fn stop(self) {
        let _ = self.stop_tx.send(());
        self.task.abort();
    }
}

pub async fn serve(store: CaptureStore, preferred_port: u16) -> std::io::Result<ProxyHandle> {
    let bind = format!("127.0.0.1:{preferred_port}");
    let listener = TcpListener::bind(bind).await?;
    let bound_port = listener.local_addr()?.port();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let client = Arc::new(
        Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?,
    );

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                accept = listener.accept() => {
                    let Ok((stream, _peer)) = accept else { continue };
                    let client = client.clone();
                    let store = store.clone();
                    tokio::spawn(async move {
                        let _ = handle_conn(stream, client, store).await;
                    });
                }
            }
        }
    });

    Ok(ProxyHandle {
        bound_port,
        stop_tx,
        task,
    })
}

async fn handle_conn(
    mut stream: TcpStream,
    client: Arc<Client>,
    store: CaptureStore,
) -> std::io::Result<()> {
    // Read the request head (and body up to 1MB).
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let mut body_to_read: Option<usize> = None;
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if body_to_read.is_none() {
                if let Some(cl) = content_length(&buf) {
                    body_to_read = Some(cl as usize);
                } else {
                    break;
                }
            }
            let body_start = sep + 4;
            let body_read = buf.len().saturating_sub(body_start);
            if let Some(want) = body_to_read {
                if body_read >= want || buf.len() > 1024 * 1024 {
                    break;
                }
            } else {
                break;
            }
        }
        if buf.len() > 65536 && body_to_read.is_none() {
            break;
        }
    }

    let text = match std::str::from_utf8(&buf) {
        Ok(t) => t,
        Err(_) => {
            let _ = write_response(&mut stream, 400, b"bad request").await;
            return Ok(());
        }
    };
    let (method, target, mut headers, body_start) = match parse_head(text) {
        Some(v) => v,
        None => {
            let _ = write_response(&mut stream, 400, b"bad request").await;
            return Ok(());
        }
    };
    if method.eq_ignore_ascii_case("CONNECT") {
        let _ = write_response(&mut stream, 501, b"CONNECT not supported").await;
        return Ok(());
    }
    let req_body = buf[body_start..].to_vec();

    // Build reqwest request.
    let mut rb = match method.to_ascii_uppercase().as_str() {
        "GET" => client.get(&target),
        "POST" => client.post(&target),
        "PUT" => client.put(&target),
        "DELETE" => client.delete(&target),
        "PATCH" => client.patch(&target),
        "HEAD" => client.head(&target),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, &target),
        other => {
            let _ = write_response(
                &mut stream,
                405,
                format!("method {other} not supported").as_bytes(),
            )
            .await;
            return Ok(());
        }
    };
    // Filter hop-by-hop.
    headers.retain(|(k, _)| {
        !matches!(
            k.to_ascii_lowercase().as_str(),
            "proxy-connection" | "connection" | "proxy-authorization" | "host"
        )
    });
    for (k, v) in &headers {
        rb = rb.header(k, v);
    }
    rb = rb.body(req_body.clone());

    let started = Instant::now();
    let (status, resp_headers, resp_body) = match rb.send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let mut rh = Vec::new();
            for (k, v) in r.headers() {
                rh.push((k.to_string(), v.to_str().unwrap_or("").to_string()));
            }
            let bytes = r.bytes().await.unwrap_or_default();
            (status, rh, bytes.to_vec())
        }
        Err(e) => {
            let msg = format!("upstream error: {e}");
            let _ = write_response(&mut stream, 502, msg.as_bytes()).await;
            store.push(CaptureEntry {
                id: 0,
                ts_ms: 0,
                method,
                url: target,
                status: 502,
                duration_ms: started.elapsed().as_millis() as u64,
                req_headers: headers,
                req_body,
                resp_headers: Vec::new(),
                resp_body: msg.into_bytes(),
            });
            return Ok(());
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;

    // Write back.
    let _ = write_response(&mut stream, status, &resp_body).await;

    store.push(CaptureEntry {
        id: 0,
        ts_ms: 0,
        method,
        url: target,
        status,
        duration_ms,
        req_headers: headers,
        req_body,
        resp_headers,
        resp_body,
    });
    Ok(())
}

fn content_length(buf: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(buf).ok()?;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        let (k, v) = line.split_once(':')?;
        if k.eq_ignore_ascii_case("content-length") {
            return v.trim().parse().ok();
        }
    }
    None
}

fn parse_head(s: &str) -> Option<(String, String, Vec<(String, String)>, usize)> {
    let mut lines = s.lines();
    let reqline = lines.next()?;
    let mut parts = reqline.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    for line in s.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        let (k, v) = line.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let body_start = s.find("\r\n\r\n").map(|p| p + 4).unwrap_or(s.len());
    Some((method, target, headers, body_start))
}

async fn write_response(stream: &mut TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        _ => "OK",
    };
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = write!(out, "HTTP/1.1 {status} {reason}\r\n");
    let _ = write!(out, "Content-Length: {}\r\n", body.len());
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}
