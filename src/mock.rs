//! Local Mock server.
//!
//! A minimal HTTP/1.1 responder that serves per-request [`MockRule`]s from the
//! active project. Listens on `127.0.0.1:<port>` (default [`DEFAULT_PORT`]).
//!
//! Matching dimensions (in priority order):
//! 1. HTTP method (if `match_method` is set on a rule it must equal the request)
//! 2. Path, evaluated as Exact → Prefix → Regex; Exact wins over Prefix wins over Regex
//! 3. Query parameters in `match_query` must be present; non-empty value must equal
//! 4. Headers in `match_headers` must be present; non-empty value must equal
//!
//! When multiple rules could match, the highest-priority one (Exact > Prefix > Regex,
//! then earliest in declaration order) wins.
//!
//! Dynamic `{{...}}` template substitution is added in the next iteration.

use std::collections::HashMap;
use std::sync::Arc;

use regex::Regex;
use smol::io::{AsyncReadExt, AsyncWriteExt};
use smol::net::{TcpListener, TcpStream};

use crate::state::models::{KeyValue, MockRule, PathPattern, Project, RequestMethod};

/// Default Mock server port.
pub const DEFAULT_PORT: u16 = 3059;

/// One parsed incoming HTTP request (request line + headers + query).
#[derive(Debug)]
struct IncomingRequest {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
}

/// A single rule entry with priority pre-computed so we can sort candidates.
#[derive(Clone)]
pub struct RuleEntry {
    rule: MockRule,
    /// Lower = higher priority. Exact=0, Prefix=1, Regex=2.
    priority: u8,
    /// The pre-resolved match value (the Exact/Prefix/Regex pattern string).
    pattern: String,
    /// Compiled regex if `priority == 2`.
    regex: Option<Regex>,
    /// Name of the request this rule was attached to (for logging).
    #[allow(dead_code)]
    name: String,
}

/// Build the rule lookup table from a project. Each rule carries its compiled
/// regex (if any) and a priority score.
///
/// Rules whose `match_path` is an empty Exact get backfilled from the request's
/// URL path (preserving the legacy v0.1 behavior where the path was derived from
/// the URL automatically).
pub fn rule_map(project: &Project) -> Arc<Vec<RuleEntry>> {
    let mut entries = Vec::new();
    let mut collect = |name: String, url: String, mock: &MockRule| {
        if !mock.enabled {
            return;
        }
        let mut rule = mock.clone();
        // Backfill empty path pattern from the request's URL path (legacy behavior).
        if let PathPattern::Exact(s) = &rule.match_path {
            if s.is_empty() {
                if let Some(auto) = path_of(&url) {
                    rule.match_path = PathPattern::Exact(auto);
                }
            }
        }
        let (priority, pattern, regex) = match &rule.match_path {
            PathPattern::Exact(s) => (0u8, s.clone(), None),
            PathPattern::Prefix(s) => (1u8, s.clone(), None),
            PathPattern::Regex(s) => {
                let anchored = format!("^(?:{s})$");
                match Regex::new(&anchored) {
                    Ok(r) => (2u8, s.clone(), Some(r)),
                    Err(e) => {
                        log::warn!("mock: bad regex {s:?}: {e}; skipping rule");
                        return;
                    }
                }
            }
        };
        entries.push(RuleEntry {
            rule,
            priority,
            pattern,
            regex,
            name,
        });
    };
    for req in &project.requests {
        if let Some(m) = req.mock.as_ref() {
            collect(req.name.clone(), req.url.clone(), m);
        }
    }
    for folder in &project.folders {
        walk_folder(folder, &mut collect);
    }
    // Stable sort: lower priority first; keep source order within a tier.
    entries.sort_by_key(|e| e.priority);
    Arc::new(entries)
}

fn walk_folder(folder: &crate::state::models::Folder, collect: &mut impl FnMut(String, String, &MockRule)) {
    for req in &folder.requests {
        if let Some(m) = req.mock.as_ref() {
            collect(req.name.clone(), req.url.clone(), m);
        }
    }
    for sub in &folder.folders {
        walk_folder(sub, collect);
    }
}

/// Extract the URL path from a (possibly templated) URL string.
pub(crate) fn path_of(url: &str) -> Option<String> {
    let url = if url.starts_with("{{") {
        if let Some(end) = url.find("}}") {
            &url[end + 2..]
        } else {
            url
        }
    } else {
        url
    };
    let url = if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        match rest.find('/') {
            Some(p) => &rest[p..],
            None => return None,
        }
    } else {
        url
    };
    let path = url.split('?').next().unwrap_or(url);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Run the Mock server until the returned task is dropped.
pub fn serve(port: u16, rules: Arc<Vec<RuleEntry>>) -> smol::Task<()> {
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

async fn handle(mut stream: TcpStream, rules: Arc<Vec<RuleEntry>>) {
    // Read up to 64KB so we capture the header block (and small bodies if any).
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    let mut headers_done = false;
    loop {
        let n = match stream.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return,
        };
        buf.extend_from_slice(&tmp[..n]);
        // Stop reading once we've seen header end.
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            headers_done = true;
            break;
        }
        if buf.len() > 65536 {
            break;
        }
    }
    let _ = headers_done;
    let req = match parse_request(&buf) {
        Some(r) => r,
        None => {
            let _ = write_response(&mut stream, 400, &[KeyValue::new("Content-Type", "text/plain")], b"Bad Request", 0).await;
            return;
        }
    };

    // Find best-matching rule.
    let entry = rules.iter().find(|e| rule_matches(e, &req));

    let entry = match entry {
        Some(e) => e,
        None => {
            let body = format!("No mock rule matched {} {}\n", req.method, req.path);
            let _ = write_response(
                &mut stream,
                404,
                &[KeyValue::new("Content-Type", "text/plain")],
                body.as_bytes(),
                0,
            )
            .await;
            return;
        }
    };
    let rule = &entry.rule;

    if rule.delay_ms > 0 {
        smol::Timer::after(std::time::Duration::from_millis(rule.delay_ms)).await;
    }

    let _ = write_response(&mut stream, rule.status, &rule.headers, rule.body.as_bytes(), 0).await;
}

fn rule_matches(entry: &RuleEntry, req: &IncomingRequest) -> bool {
    // Method.
    if let Some(m) = &entry.rule.match_method {
        if req.method.to_ascii_uppercase() != m.as_str() {
            return false;
        }
    }
    // Path.
    let path_match = match &entry.rule.match_path {
        PathPattern::Exact(s) => req.path == *s,
        PathPattern::Prefix(s) => req.path.starts_with(s.as_str()),
        PathPattern::Regex(_) => entry.regex.as_ref().map(|r| r.is_match(&req.path)).unwrap_or(false),
    };
    if !path_match {
        return false;
    }
    // Query.
    for kv in &entry.rule.match_query {
        if !kv.enabled || kv.key.is_empty() {
            continue;
        }
        let present = req.query.get(&kv.key.to_ascii_lowercase());
        match present {
            Some(v) if !kv.value.is_empty() && v != &kv.value => return false,
            None => return false,
            _ => {}
        }
    }
    // Headers.
    for kv in &entry.rule.match_headers {
        if !kv.enabled || kv.key.is_empty() {
            continue;
        }
        let present = req.headers.get(&kv.key.to_ascii_lowercase());
        match present {
            Some(v) if !kv.value.is_empty() && v != &kv.value => return false,
            None => return false,
            _ => {}
        }
    }
    true
}

/// Parse a raw HTTP/1.1 request (request line + headers). Bodies are ignored.
fn parse_request(buf: &[u8]) -> Option<IncomingRequest> {
    let text = String::from_utf8_lossy(buf);
    let mut lines = text.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let raw_target = parts.next()?.to_string();
    // Split target into path + query.
    let (path, query) = match raw_target.find('?') {
        Some(i) => (raw_target[..i].to_string(), &raw_target[i + 1..]),
        None => (raw_target, ""),
    };
    let mut query_map = HashMap::new();
    if !query.is_empty() {
        for pair in query.split('&') {
            let (k, v) = match pair.find('=') {
                Some(i) => (&pair[..i], &pair[i + 1..]),
                None => (pair, ""),
            };
            if !k.is_empty() {
                query_map.insert(url_decode(k).to_ascii_lowercase(), url_decode(v));
            }
        }
    }
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some(IncomingRequest {
        method,
        path,
        query: query_map,
        headers,
    })
}

fn url_decode(s: &str) -> String {
    // Minimal percent decode — sufficient for mock match keys (JSON tokens, simple
    // ids). Full urlencoding is overkill here.
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut out = format!("HTTP/1.1 {status} {status_text}\r\n");
    let mut has_content_type = false;
    for h in headers {
        if h.enabled {
            if h.key.eq_ignore_ascii_case("content-type") {
                has_content_type = true;
            }
            out.push_str(&format!("{}: {}\r\n", h.key, h.value));
        }
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
    use crate::state::models::RequestMethod;

    fn rule_exact(path: &str) -> MockRule {
        MockRule {
            match_path: PathPattern::Exact(path.into()),
            ..Default::default()
        }
    }
    fn rule_prefix(path: &str) -> MockRule {
        MockRule {
            match_path: PathPattern::Prefix(path.into()),
            ..Default::default()
        }
    }
    fn rule_regex(pattern: &str) -> MockRule {
        MockRule {
            match_path: PathPattern::Regex(pattern.into()),
            ..Default::default()
        }
    }

    fn req(method: &str, path: &str) -> IncomingRequest {
        let (p, q) = match path.find('?') {
            Some(i) => (path[..i].to_string(), &path[i + 1..]),
            None => (path.to_string(), ""),
        };
        let mut query = HashMap::new();
        for pair in q.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            query.insert(k.to_ascii_lowercase(), v.to_string());
        }
        IncomingRequest {
            method: method.into(),
            path: p,
            query,
            headers: HashMap::new(),
        }
    }

    #[test]
    fn path_extraction() {
        assert_eq!(path_of("{{baseUrl}}/api/login"), Some("/api/login".into()));
        assert_eq!(path_of("https://x.test/users/1"), Some("/users/1".into()));
        assert_eq!(path_of("{{baseUrl}}/items?detail=1"), Some("/items".into()));
    }

    #[test]
    fn exact_matches_only_exact_path() {
        let e = RuleEntry {
            rule: rule_exact("/a"),
            priority: 0,
            pattern: "/a".into(),
            regex: None,
            name: "t".into(),
        };
        assert!(rule_matches(&e, &req("GET", "/a")));
        assert!(!rule_matches(&e, &req("GET", "/a/b")));
    }

    #[test]
    fn prefix_matches_subpaths() {
        let e = RuleEntry {
            rule: rule_prefix("/api"),
            priority: 1,
            pattern: "/api".into(),
            regex: None,
            name: "t".into(),
        };
        assert!(rule_matches(&e, &req("GET", "/api")));
        assert!(rule_matches(&e, &req("GET", "/api/users")));
        assert!(!rule_matches(&e, &req("GET", "/other")));
    }

    #[test]
    fn regex_matches_pattern() {
        let e = RuleEntry {
            rule: rule_regex(r"/api/users/\d+"),
            priority: 2,
            pattern: r"/api/users/\d+".into(),
            regex: Some(Regex::new("^(?:/api/users/\\d+)$").unwrap()),
            name: "t".into(),
        };
        assert!(rule_matches(&e, &req("GET", "/api/users/42")));
        assert!(!rule_matches(&e, &req("GET", "/api/users/abc")));
    }

    #[test]
    fn method_filter_works() {
        let mut r = rule_exact("/a");
        r.match_method = Some(RequestMethod::Post);
        let e = RuleEntry { rule: r, priority: 0, pattern: "/a".into(), regex: None, name: "t".into() };
        assert!(!rule_matches(&e, &req("GET", "/a")));
        assert!(rule_matches(&e, &req("POST", "/a")));
    }

    #[test]
    fn query_match_requires_presence_and_value() {
        let mut r = rule_exact("/a");
        r.match_query = vec![KeyValue::new("debug", "1")];
        let e = RuleEntry { rule: r, priority: 0, pattern: "/a".into(), regex: None, name: "t".into() };
        assert!(!rule_matches(&e, &req("GET", "/a")));
        assert!(rule_matches(&e, &req("GET", "/a?debug=1")));
        assert!(!rule_matches(&e, &req("GET", "/a?debug=2")));
    }
}
