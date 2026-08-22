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

/// Incremental SSE wire-format parser.
///
/// Feed it raw body bytes as they arrive; it buffers partial lines and
/// returns every complete event (blank-line delimited) completed by that
/// chunk. Shared by the SSE request panel and the AI chat client so both
/// speak exactly the same `event:`/`data:`/`id:` dialect.
#[derive(Debug, Default)]
pub struct SseParser {
    pending_event: SseEvent,
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            pending_event: SseEvent::default(),
            buf: Vec::with_capacity(8192),
        }
    }

    /// Feed raw body bytes; returns events completed by this chunk.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line_bytes = self.buf.drain(..=pos).collect::<Vec<_>>();
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
                if !self.pending_event.is_empty() {
                    out.push(std::mem::take(&mut self.pending_event));
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                self.pending_event.event = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !self.pending_event.data.is_empty() {
                    self.pending_event.data.push('\n');
                }
                self.pending_event.data.push_str(rest.trim_start_matches(' '));
            } else if let Some(rest) = line.strip_prefix("id:") {
                self.pending_event.id = Some(rest.trim().to_string());
            }
            // Comments (lines starting with ':') are ignored.
        }
        out
    }
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
        let mut chunk = [0u8; 4096];
        let mut parser = SseParser::new();
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
            for ev in parser.feed(&chunk[..n]) {
                let rendered = event_to_string(&ev);
                if !acc.is_empty() {
                    acc.push('\n');
                }
                acc.push_str(&rendered);
                // Mirror into the shared buffer for live polling.
                if let Ok(mut shared) = acc_shared.lock() {
                    *shared = acc.clone();
                }
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
            received_at: Some(Response::now_stamp()),
        })
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::{SseEvent, SseParser, event_to_string};

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

    #[test]
    fn parser_handles_chunk_splits_and_crlf() {
        let mut p = SseParser::new();
        // "data: a\r\n\r\n" split across feeds at arbitrary byte offsets.
        let bytes = b"data: a\r\n\r\ndata: b\n\n";
        let mut all = Vec::new();
        for b in bytes.iter() {
            all.extend(p.feed(&[*b]));
        }
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].data, "a");
        assert_eq!(all[1].data, "b");
    }

    #[test]
    fn parser_joins_multi_data_lines_and_ignores_comments() {
        let mut p = SseParser::new();
        let events = p.feed(b": keepalive\ndata: l1\ndata: l2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "l1\nl2");
    }

    #[test]
    fn parser_keeps_partial_line_between_feeds() {
        let mut p = SseParser::new();
        assert!(p.feed(b"dat").is_empty());
        let events = p.feed(b"a: x\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn parser_captures_event_and_id_fields() {
        let mut p = SseParser::new();
        let events = p.feed(b"event: add\ndata: 1\nid: 7\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "add");
        assert_eq!(events[0].id.as_deref(), Some("7"));
    }
}

// Silence unused-import warnings for items the caller threads through but this
// module doesn't directly use.
#[allow(dead_code)]
fn _unused(_a: AuthConfig, _v: &BTreeMap<String, String>) {}
