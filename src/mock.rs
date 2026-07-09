//! Local Mock server.
//!
//! A minimal HTTP/1.1 responder that serves the per-request [`MockRule`]s of
//! the active project. It listens on `127.0.0.1:<port>` (default 3059) and
//! matches incoming requests by path against each API request's URL path.
//!
//! This is intentionally small (no streaming, no TLS) — it exists so a frontend
//! can develop against Mock data before the real backend is ready, per PRD §5.5.

use std::collections::HashMap;
use std::sync::Arc;

use smol::io::{AsyncReadExt, AsyncWriteExt};
use smol::net::{TcpListener, TcpStream};

use crate::state::models::{KeyValue, MockRule, Project};

/// Default Mock server port.
pub const DEFAULT_PORT: u16 = 3059;

/// Build a path → mock-rule lookup from a project. The key is the URL path
/// (without query), derived from each request's `url` after stripping any
/// `{{...}}` prefix and the host.
pub fn rule_map(project: &Project) -> HashMap<String, (String, MockRule)> {
    let mut map = HashMap::new();
    let mut collect = |name: String, url: String, mock: &MockRule| {
        if let Some(path) = path_of(&url) {
            map.insert(path, (name, mock.clone()));
        }
    };
    for req in &project.requests {
        if let Some(m) = req.mock.as_ref().filter(|m| m.enabled) {
            collect(req.name.clone(), req.url.clone(), m);
        }
    }
    for folder in &project.folders {
        walk_folder(folder, &mut collect);
    }
    map
}

fn walk_folder(
    folder: &crate::state::models::Folder,
    collect: &mut impl FnMut(String, String, &MockRule),
) {
    for req in &folder.requests {
        if let Some(m) = req.mock.as_ref().filter(|m| m.enabled) {
            collect(req.name.clone(), req.url.clone(), m);
        }
    }
    for sub in &folder.folders {
        walk_folder(sub, collect);
    }
}

/// Extract the URL path from a (possibly templated) URL string.
fn path_of(url: &str) -> Option<String> {
    // Drop {{...}} host prefix if present.
    let url = if url.starts_with("{{") {
        if let Some(end) = url.find("}}") {
            &url[end + 2..]
        } else {
            url
        }
    } else {
        url
    };
    // Drop scheme://host if present.
    let url = if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        match rest.find('/') {
            Some(p) => &rest[p..],
            None => return None,
        }
    } else {
        url
    };
    // Drop query string.
    let path = url.split('?').next().unwrap_or(url);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Run the Mock server until the returned task is dropped.
pub fn serve(port: u16, rules: Arc<HashMap<String, (String, MockRule)>>) -> smol::Task<()> {
    smol::spawn(async move {
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("mock server bind failed on :{port}: {e}");
                return;
            }
        };
        log::info!("verve mock server listening on http://127.0.0.1:{port}");
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let rules = rules.clone();
            smol::spawn(handle(stream, rules)).detach();
        }
    })
}

async fn handle(mut stream: TcpStream, rules: Arc<HashMap<String, (String, MockRule)>>) {
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    // Parse the request line: "GET /path HTTP/1.1".
    let path = req
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();

    let (name, rule) = match rules.get(&path) {
        Some(v) => v.clone(),
        None => {
            let body = format!("No mock rule for path {path}\n");
            let _ = write_response(&mut stream, 404, &[KeyValue::new("Content-Type", "text/plain")], body.as_bytes(), 0).await;
            return;
        }
    };
    let _ = name;

    if rule.delay_ms > 0 {
        smol::Timer::after(std::time::Duration::from_millis(rule.delay_ms)).await;
    }

    let body = rule.body.as_bytes();
    let _ = write_response(&mut stream, rule.status, &rule.headers, body, 0).await;
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[KeyValue],
    body: &[u8],
    _extra: u64,
) -> std::io::Result<()> {
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut out = format!("HTTP/1.1 {status} {status_text}\r\n");
    let mut has_content_type = false;
    for h in headers {
        if h.key.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        out.push_str(&format!("{}: {}\r\n", h.key, h.value));
    }
    if !has_content_type {
        out.push_str("Content-Type: application/json\r\n");
    }
    out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_extraction() {
        assert_eq!(path_of("{{baseUrl}}/api/login"), Some("/api/login".into()));
        assert_eq!(path_of("https://x.test/users/1"), Some("/users/1".into()));
        assert_eq!(
            path_of("{{baseUrl}}/items?detail=1"),
            Some("/items".into())
        );
    }
}
