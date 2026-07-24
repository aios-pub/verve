//! gRPC client — gRPC-Web mode over HTTP/1.1.
//!
//! Uses the existing reqwest HTTP client to send a gRPC-Web request
//! (Content-Type: application/grpc-web+proto). The request body is the
//! gRPC framing: [compressed(1)] [length(4)] [protobuf payload]. Since we
//! don't have a .proto compiler at runtime, the user's JSON body is sent as
//! a best-effort JSON message with `Content-Type: application/grpc-web+json`,
//! which gRPC-Gateway / some proxies accept. For native gRPC servers,
//! the user can paste raw protobuf bytes (hex-encoded) in the body.

use std::time::{Duration, Instant};

use futures::AsyncReadExt as _;
use http_client::{AsyncBody, Builder, HttpClient, HttpRequestExt as _, Method, RedirectPolicy};

use crate::state::models::{KeyValue, Response};

/// Build the gRPC-Web URL from the base URL + method path.
/// The request URL should be like: `grpc://host:port/package.Service/Method`
/// We convert it to `http://host:port/package.Service/Method`.
fn grpc_url(raw: &str) -> String {
    let url = raw
        .strip_prefix("grpc://")
        .or_else(|| raw.strip_prefix("://"))
        .unwrap_or(raw);
    format!("http://{url}")
}

/// Execute a gRPC-Web call. The body is sent as `application/grpc-web+json`
/// (gRPC-Web JSON mode). The response is decoded and displayed.
pub async fn execute_grpc_web(
    client: &dyn HttpClient,
    url: &str,
    headers: &[KeyValue],
    body_json: &str,
    vars: &std::collections::BTreeMap<String, String>,
    timeout_secs: u64,
) -> Response {
    let start = Instant::now();
    let real_url = grpc_url(url);

    // Build headers: gRPC-Web JSON content type.
    let mut out_headers: Vec<(String, String)> = vec![
        ("Content-Type".into(), "application/grpc-web+json".into()),
        ("X-Grpc-Web".into(), "1".into()),
        ("X-User-Agent".into(), "grpc-web-rust/0.1".into()),
    ];
    for h in headers.iter().filter(|h| h.enabled && !h.is_empty()) {
        let k = crate::http::variable::substitute(&h.key, vars);
        let v = crate::http::variable::substitute(&h.value, vars);
        out_headers.push((k, v));
    }

    let mut builder = Builder::new()
        .method(Method::POST)
        .uri(&real_url)
        .follow_redirects(RedirectPolicy::FollowAll);
    for (k, v) in &out_headers {
        builder = builder.header(k.clone(), v.clone());
    }

    // gRPC-Web framing: [compressed_flag=0 (1 byte)] [length (4 bytes BE)] [payload]
    let payload = body_json.as_bytes();
    let len = payload.len() as u32;
    let mut framed = Vec::with_capacity(5 + payload.len());
    framed.push(0u8); // not compressed
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(payload);

    let req = match builder.body(AsyncBody::from(framed)) {
        Ok(r) => r,
        Err(e) => {
            return Response {
                status: 0,
                status_text: "gRPC Error".into(),
                error: Some(format!("build request: {e}")),
                ..Default::default()
            };
        }
    };

    // Race against timeout.
    let send_fut = client.send(req);
    let result = smol::future::or(
        async {
            smol::Timer::after(Duration::from_secs(timeout_secs.max(1))).await;
            Err(anyhow::anyhow!("gRPC 请求超时（{timeout_secs}s）"))
        },
        send_fut,
    )
    .await;

    let time_ms = start.elapsed().as_millis() as u64;
    let resp = match result {
        Ok(r) => r,
        Err(e) => {
            return Response {
                status: 0,
                status_text: "gRPC Error".into(),
                time_ms,
                error: Some(format!(
                    "连接失败：{e}\n\n请确认 gRPC 服务是否在运行，且支持 gRPC-Web。\nURL: {real_url}"
                )),
                ..Default::default()
            };
        }
    };

    let status = resp.status().as_u16();
    let status_text = if status == 200 {
        "gRPC OK".to_string()
    } else {
        format!(
            "gRPC HTTP {}",
            resp.status().canonical_reason().unwrap_or("")
        )
    };

    // Read and decode the gRPC-Web response body.
    let mut body = resp.into_body();
    let mut buf = Vec::new();
    let _ = body.read_to_end(&mut buf).await;

    let resp_headers: Vec<KeyValue> = vec![]; // resp headers consumed by into_body
    let (body_text, grpc_status) = decode_grpc_web_response(&buf);

    Response {
        status,
        status_text: if let Some(gs) = grpc_status {
            if gs == 0 {
                format!("{status_text} (gRPC {gs} OK)")
            } else {
                format!("{status_text} (gRPC {gs})")
            }
        } else {
            status_text
        },
        time_ms,
        size: body_text.len() as u64,
        headers: resp_headers,
        body: body_text,
        is_json: false,
        error: if grpc_status.unwrap_or(0) != 0 {
            Some(format!("gRPC 状态码: {}", grpc_status.unwrap()))
        } else {
            None
        },
        streaming: false,
    }
}

/// Decode a gRPC-Web response body: strip the 5-byte framing prefix
/// [compressed(1)] [length(4)] and return the payload as a string.
/// Also extracts the gRPC status from trailers if present.
fn decode_grpc_web_response(buf: &[u8]) -> (String, Option<u32>) {
    if buf.len() < 5 {
        // Maybe it's plain text or an error page.
        return (String::from_utf8_lossy(buf).to_string(), None);
    }

    let mut pos = 0;
    let mut payload = String::new();
    let mut grpc_status: Option<u32> = None;

    while pos + 5 <= buf.len() {
        let compressed = buf[pos];
        let len =
            u32::from_be_bytes([buf[pos + 1], buf[pos + 2], buf[pos + 3], buf[pos + 4]]) as usize;
        pos += 5;

        if pos + len > buf.len() {
            // Truncated frame; take what we have.
            let chunk = &buf[pos..];
            if compressed == 0 {
                payload.push_str(&String::from_utf8_lossy(chunk));
            }
            break;
        }

        let frame = &buf[pos..pos + len];
        pos += len;

        if compressed == 0x80 {
            // Trailer frame (gRPC-Web): parse key:value pairs.
            let trailer = String::from_utf8_lossy(frame).to_string();
            for line in trailer.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim() == "grpc-status" {
                        grpc_status = v.trim().parse().ok();
                    }
                }
            }
        } else if compressed == 0 {
            payload.push_str(&String::from_utf8_lossy(frame));
        }
    }

    (payload, grpc_status)
}
