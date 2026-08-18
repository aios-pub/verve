//! Server-Sent Events (SSE) streaming execution.
//!
//! Issues a normal HTTP request and reads the response body incrementally,
//! parsing the SSE wire format (`event:`/`data:`/`id:` lines, blank-line
//! delimiters) and emitting each event to the caller as it arrives.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, Builder, HttpClient, HttpRequestExt as _, Method, RedirectPolicy};

use crate::http::PreparedRequest;
use crate::state::models::{AuthConfig, KeyValue, RequestMethod, Response};

/// A single parsed SSE event.
#[derive(Debug, Clone, Default)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

impl SseEvent {
    pub fn is_empty(&self) -> bool {
        self.event.is_empty() && self.data.is_empty() && self.id.is_none()
    }
}

/// Render an SSE event as a compact display string for the response panel.
pub fn event_to_string(ev: &SseEvent) -> String {
    let mut parts = Vec::new();
    if !ev.event.is_empty() {
        parts.push(format!("event: {}", ev.event));
    }
    if !ev.data.is_empty() {
        parts.push(format!("data: {}", ev.data));
    }
    if let Some(id) = &ev.id {
        parts.push(format!("id: {}", id));
    }
    parts.join("  │  ")
}

/// Open an SSE stream. Accumulates parsed events into `acc` (a shared buffer)
/// so the caller can poll it for live UI updates; the loop exits when the
/// stream closes, errors, or the `stop` flag is set.
///
/// Returns the final `Response` shell (status + headers filled; body = `acc`).
pub fn stream(
    client: Arc<dyn HttpClient>,
    prepared: PreparedRequest,
    timeout_secs: u64,
    stop: Arc<AtomicBool>,
    acc_shared: Arc<std::sync::Mutex<String>>,
) -> futures::future::BoxFuture<'static, Result<Response>> {
    use futures::FutureExt as _;
    async move {
        let start = Instant::now();
        // Force SSE-friendly headers if the caller didn't set them.
        let mut headers = prepared.headers;
        if !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
        {
            headers.push(("Accept".into(), "text/event-stream".into()));
        }

        let method = match prepared.method {
            RequestMethod::Get => Method::GET,
            RequestMethod::Post => Method::POST,
            RequestMethod::Put => Method::PUT,
            RequestMethod::Delete => Method::DELETE,
            RequestMethod::Patch => Method::PATCH,
            RequestMethod::Head => Method::HEAD,
            RequestMethod::Options => Method::OPTIONS,
        };

        let mut builder = Builder::new()
            .method(method)
            .uri(&prepared.url)
            .follow_redirects(RedirectPolicy::FollowAll);
        for (k, v) in &headers {
            builder = builder.header(k.clone(), v.clone());
        }
        let req = builder
            .body(AsyncBody::from(prepared.body.clone()))
            .context("build sse request")?;

        // Race the initial send against a connect timeout.
        let send_fut = client.send(req);
        let resp = match smol::future::or(
            async {
                smol::Timer::after(Duration::from_secs(timeout_secs.max(1))).await;
                Err(anyhow::anyhow!("连接超时（{timeout_secs}s）"))
            },
            send_fut,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Response {
                    status: 0,
                    status_text: "Error".into(),
                    time_ms: start.elapsed().as_millis() as u64,
                    error: Some(format!("{e}")),
                    streaming: false,
                    ..Default::default()
                });
            }
        };

        let status = resp.status().as_u16();
        let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
        let headers: Vec<KeyValue> = resp
            .headers()
            .iter()
            .map(|(k, v)| KeyValue::new(k.as_str(), v.to_str().unwrap_or("")))
            .collect();

        // Stream the body, parsing SSE lines incrementally.
        let mut body = resp.into_body();
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 4096];
        let mut pending_event = SseEvent::default();
        let mut acc = String::new();

        loop {
            if stop.load(Ordering::SeqCst) {
                acc.push_str("\n[已停止]\n");
                break;
            }
            let n = body.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            // Process complete lines.
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line_bytes = buf.drain(..=pos).collect::<Vec<_>>();
                let mut line = String::from_utf8_lossy(&line_bytes).to_string();
                // Strip trailing \r\n / \n.
                if line.ends_with('\n') {
                    line.pop();
                }
                if line.ends_with('\r') {
                    line.pop();
                }
                if line.is_empty() {
                    // Blank line → dispatch the accumulated event.
                    if !pending_event.is_empty() {
                        let ev = std::mem::take(&mut pending_event);
                        let rendered = event_to_string(&ev);
                        if !acc.is_empty() {
                            acc.push('\n');
                        }
                        acc.push_str(&rendered);
                        // Mirror into the shared buffer for live polling.
                        if let Ok(mut shared) = acc_shared.lock() {
                            *shared = acc.clone();
                        }
                        let _ = ev;
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("event:") {
                    pending_event.event = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !pending_event.data.is_empty() {
                        pending_event.data.push('\n');
                    }
                    pending_event.data.push_str(rest.trim_start_matches(' '));
                } else if let Some(rest) = line.strip_prefix("id:") {
                    pending_event.id = Some(rest.trim().to_string());
                }
                // Comments (lines starting with ':') are ignored.
            }
        }

        Ok(Response {
            status,
            status_text,
            time_ms: start.elapsed().as_millis() as u64,
            size: acc.len() as u64,
            headers,
            body: acc,
            is_json: false,
            error: None,
            streaming: false,
            actual_request: None,
            actual_curl: None,
        })
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_and_event_lines() {
        let mut ev = SseEvent::default();
        // Simulate line processing.
        let line = "event: message";
        if let Some(rest) = line.strip_prefix("event:") {
            ev.event = rest.trim().to_string();
        }
        let line = "data: hello";
        if let Some(rest) = line.strip_prefix("data:") {
            ev.data = rest.trim_start_matches(' ').to_string();
        }
        assert_eq!(ev.event, "message");
        assert_eq!(ev.data, "hello");
        assert_eq!(event_to_string(&ev), "event: message  │  data: hello");
    }

    #[test]
    fn empty_event_is_empty() {
        assert!(SseEvent::default().is_empty());
    }
}

// Silence unused-import warnings for items the caller threads through but this
// module doesn't directly use.
#[allow(dead_code)]
fn _unused(_a: AuthConfig, _v: &BTreeMap<String, String>) {}
