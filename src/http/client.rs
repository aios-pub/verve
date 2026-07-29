//! Request execution: turn an `ApiRequest` into an HTTP transaction.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, Builder, HttpClient, HttpRequestExt, Method, RedirectPolicy};
use url::Url;

use crate::state::models::{
    AuthConfig, AuthTarget, AuthType, BodyType, KeyValue, RequestMethod, Response,
};

/// A fully-resolved request ready to send.
pub struct PreparedRequest {
    pub method: RequestMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Resolve an [`crate::state::models::ApiRequest`] against a variable map into
/// a concrete `PreparedRequest`: substitute path variables + variables, build
/// the query string, apply headers/cookies/auth, and serialize the body.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    method: RequestMethod,
    raw_url: &str,
    params: &[KeyValue],
    headers: &[KeyValue],
    path: &[KeyValue],
    cookies: &[KeyValue],
    auth: &AuthConfig,
    body: &crate::state::models::RequestBody,
    vars: &BTreeMap<String, String>,
    timeout_secs: u64,
) -> Result<PreparedRequest> {
    // Path-template variables are merged into the variable map at the highest
    // priority so `{{key}}` in the URL resolves to the path value.
    let mut url_vars = vars.clone();
    for kv in path {
        if kv.enabled && !kv.is_empty() {
            url_vars.insert(kv.key.trim().to_string(), kv.value.clone());
        }
    }
    let mut url = super::variable::substitute(raw_url, &url_vars);

    // If the URL is a relative path (no scheme), prepend the folder/request
    // base_url resolved by the caller (request_panel) and stashed in the
    // "__folder_base_url__" variable. This MUST run before normalize_url,
    // which would otherwise turn "/api/users" into "http://api/users" and
    // mask the relative-path case so the base_url never gets applied.
    if !url.contains("://") {
        if let Some(base) = url_vars.get("__folder_base_url__") {
            if !base.is_empty() {
                let base = base.trim_end_matches('/');
                let path = url.trim_start_matches('/');
                url = format!("{}/{}", base, path);
            }
        }
    }

    // Auto-fix the URL: strip redundant leading slashes and prepend a default
    // protocol if the user omitted it (e.g. "www.baidu.com" → "http://www.baidu.com").
    // By this point a relative path has already been joined onto the base_url,
    // so only truly scheme-less hosts (or the no-base-url case) get a default scheme.
    url = normalize_url(&url, method);

    // Attach enabled query params (after substitution) to the URL.
    let mut url = if params.iter().any(|p| p.enabled && !p.is_empty()) {
        let mut parsed = Url::parse(&url).context("invalid url")?;
        {
            let mut q = parsed.query_pairs_mut();
            for p in params {
                if p.enabled && !p.is_empty() {
                    let k = super::variable::substitute(&p.key, vars);
                    let v = super::variable::substitute(&p.value, vars);
                    q.append_pair(&k, &v);
                }
            }
        }
        parsed.to_string()
    } else {
        url
    };

    // Headers (substituted).
    let mut out_headers: Vec<(String, String)> = headers
        .iter()
        .filter(|h| h.enabled && !h.is_empty())
        .map(|h| {
            (
                super::variable::substitute(&h.key, vars),
                super::variable::substitute(&h.value, vars),
            )
        })
        .collect();

    // Cookies → single `Cookie: k=v; k=v` header.
    let cookie_pairs: Vec<String> = cookies
        .iter()
        .filter(|c| c.enabled && !c.is_empty())
        .map(|c| {
            let k = super::variable::substitute(&c.key, vars);
            let v = super::variable::substitute(&c.value, vars);
            format!("{k}={v}")
        })
        .collect();
    if !cookie_pairs.is_empty() && !ensure_header(&out_headers, "cookie") {
        out_headers.push(("Cookie".into(), cookie_pairs.join("; ")));
    }

    // Authentication → Authorization header (Bearer/Basic) or API key.
    inject_auth(&mut out_headers, auth, &mut url, vars);

    // Body + ensure a Content-Type.
    let mut body_bytes: Vec<u8> = Vec::new();
    match body.body_type {
        BodyType::None => {}
        BodyType::Raw => {
            if !ensure_header(&out_headers, "content-type") {
                out_headers.push((
                    "Content-Type".into(),
                    body.raw_language.content_type().into(),
                ));
            }
            body_bytes = super::variable::substitute(&body.raw, vars).into_bytes();
        }
        BodyType::Urlencoded => {
            if !ensure_header(&out_headers, "content-type") {
                out_headers.push((
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ));
            }
            let pairs: Vec<(String, String)> = body
                .urlencoded
                .iter()
                .filter(|kv| kv.enabled && !kv.is_empty())
                .map(|kv| {
                    (
                        super::variable::substitute(&kv.key, vars),
                        super::variable::substitute(&kv.value, vars),
                    )
                })
                .collect();
            body_bytes = serde_urlencode(&pairs).into_bytes();
        }
        BodyType::FormData => {
            // Use a simple multipart boundary. File uploads are read from disk.
            let boundary = format!("verve-{}", uuid::Uuid::new_v4().simple());
            out_headers.push((
                "Content-Type".into(),
                format!("multipart/form-data; boundary={boundary}"),
            ));
            for kv in &body.form_data {
                if !kv.enabled || kv.is_empty() {
                    continue;
                }
                let name = super::variable::substitute(&kv.key, vars);
                if let Some(path) = &kv.file_path {
                    let path = super::variable::substitute(path, vars);
                    match std::fs::read(&path) {
                        Ok(data) => {
                            let filename = std::path::Path::new(&path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("file")
                                .to_string();
                            // Infer a per-part Content-Type from the
                            // extension (RFC 7578). Strict servers reject file
                            // parts that omit it. Falls back to a binary stream.
                            let mime = guess_mime(&filename);
                            body_bytes.extend_from_slice(
                                format!(
                                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n"
                                )
                                .as_bytes(),
                            );
                            body_bytes.extend_from_slice(&data);
                            body_bytes.extend_from_slice(b"\r\n");
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("read file {path}: {e}"));
                        }
                    }
                } else {
                    let value = super::variable::substitute(&kv.value, vars);
                    body_bytes.extend_from_slice(
                        format!(
                            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                        )
                        .as_bytes(),
                    );
                }
            }
            body_bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        }
    }

    let _ = timeout_secs; // applied in `execute`
    Ok(PreparedRequest {
        method,
        url,
        headers: out_headers,
        body: body_bytes,
    })
}

fn ensure_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

/// Apply authentication to the outgoing headers (or query string for an API
/// key targeted at the query).
fn inject_auth(
    headers: &mut Vec<(String, String)>,
    auth: &AuthConfig,
    url: &mut String,
    vars: &BTreeMap<String, String>,
) {
    match auth.auth_type {
        AuthType::None => {}
        AuthType::Bearer => {
            let token = super::variable::substitute(&auth.token, vars);
            if !token.is_empty() && !ensure_header(headers, "authorization") {
                headers.push(("Authorization".into(), format!("Bearer {token}")));
            }
        }
        AuthType::Basic => {
            let user = super::variable::substitute(&auth.username, vars);
            let pass = super::variable::substitute(&auth.password, vars);
            let encoded = base64_encode(format!("{user}:{pass}").as_bytes());
            if !ensure_header(headers, "authorization") {
                headers.push(("Authorization".into(), format!("Basic {encoded}")));
            }
        }
        AuthType::ApiKey => {
            let key = super::variable::substitute(&auth.key, vars);
            let value = super::variable::substitute(&auth.value, vars);
            if key.is_empty() {
                return;
            }
            match auth.add_to {
                AuthTarget::Header => {
                    if !ensure_header(headers, &key) {
                        headers.push((key, value));
                    }
                }
                AuthTarget::Query => {
                    if let Ok(mut parsed) = Url::parse(url) {
                        parsed.query_pairs_mut().append_pair(&key, &value);
                        *url = parsed.to_string();
                    }
                }
            }
        }
    }
}

/// Minimal Base64 (standard alphabet) encoder — avoids pulling a crate.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Infer a MIME type from a filename's extension. Covers the common cases
/// enough for `multipart/form-data` file parts (RFC 7578); unknown extensions
/// fall back to a generic binary stream. Kept dependency-free.
fn guess_mime(filename: &str) -> &'static str {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        // Images.
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" | "dib" => "image/bmp",
        "ico" => "image/x-icon",
        "tiff" | "tif" => "image/tiff",
        // Documents.
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "xml" => "text/xml",
        "csv" => "text/csv",
        "txt" | "log" | "md" | "markdown" => "text/plain",
        "json" => "application/json",
        "yaml" | "yml" => "application/x-yaml",
        // Archives / binaries.
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        // Audio / video.
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mpeg" | "mpg" => "video/mpeg",
        "webm" => "video/webm",
        "ogg" => "application/ogg",
        _ => "application/octet-stream",
    }
}

fn serde_urlencode(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Normalize a URL: collapse multiple leading slashes and prepend a default
/// protocol scheme if the user omitted one.
pub fn normalize_url(url: &str, _method: RequestMethod) -> String {
    normalize_url_with_default(url, "http")
}

/// Normalize a URL with a caller-specified default scheme (e.g. "ws", "tcp").
pub fn normalize_url_with_default(url: &str, default_scheme: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }

    // If the URL already has a known scheme, return as-is.
    let known = [
        "http://", "https://", "ws://", "wss://", "tcp://", "grpc://",
    ];
    if known.iter().any(|s| trimmed.to_lowercase().starts_with(s)) || trimmed.contains("://") {
        return trimmed.to_string();
    }

    // Collapse multiple leading slashes into one, then prepend scheme.
    let body = trimmed.trim_start_matches('/');
    format!("{default_scheme}://{body}")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Apply virtual hosts override: if the URL's hostname matches an enabled
/// hosts profile entry, rewrite the URL to point to the override IP and add
/// a `Host:` header so name-based virtual hosting still works.
/// Returns (possibly rewritten url, extra headers).
fn apply_virtual_hosts(
    url_str: &str,
    headers: &[(String, String)],
) -> (String, Vec<(String, String)>) {
    let mut extra = Vec::new();
    let Ok(mut url) = Url::parse(url_str) else {
        return (url_str.to_string(), extra);
    };

    let hostname = url.host_str().unwrap_or("").to_string();
    if hostname.is_empty() {
        return (url_str.to_string(), extra);
    }

    // Load overrides. Note: we call load() here (cheap: one JSON read per request).
    let store = crate::hosts_profiles::load();
    // Determine active env id (we pass None here because env binding is resolved
    // at the UI/per-request level; for simplicity all-enabled overrides apply).
    // A more complete integration would thread the active env id through to here.
    let overrides = crate::hosts_profiles::effective_virtual_overrides(&store, None);

    for (host, ip) in overrides {
        if host == hostname {
            // Rewrite the URL's host to the IP.
            if url.set_host(Some(&ip)).is_ok() {
                // Add Host header with the original hostname unless already set.
                let has_host = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host"));
                if !has_host {
                    extra.push(("Host".to_string(), hostname.clone()));
                }
                return (url.to_string(), extra);
            }
        }
    }

    (url_str.to_string(), extra)
}

/// Execute a prepared request via the given HTTP client and capture a `Response`.
/// Errors (build failure, timeout, transport) are returned as a `Response` with
/// `status == 0` and the `error` field set, so callers always get a value.
pub async fn execute(
    client: &dyn HttpClient,
    prepared: PreparedRequest,
    timeout_secs: u64,
) -> Response {
    let method = match prepared.method {
        RequestMethod::Get => Method::GET,
        RequestMethod::Post => Method::POST,
        RequestMethod::Put => Method::PUT,
        RequestMethod::Delete => Method::DELETE,
        RequestMethod::Patch => Method::PATCH,
        RequestMethod::Head => Method::HEAD,
        RequestMethod::Options => Method::OPTIONS,
    };

    // Apply virtual hosts override (rewrite URL + add Host header if needed).
    let (final_url, extra_headers) = apply_virtual_hosts(&prepared.url, &prepared.headers);

    let mut builder = Builder::new()
        .uri(&final_url)
        .method(method)
        .follow_redirects(RedirectPolicy::FollowAll);

    for (k, v) in &prepared.headers {
        builder = builder.header(k.clone(), v.clone());
    }
    for (k, v) in &extra_headers {
        builder = builder.header(k.clone(), v.clone());
    }

    let req = match builder.body(AsyncBody::from(prepared.body.clone())) {
        Ok(r) => r,
        Err(e) => {
            return Response {
                status: 0,
                status_text: "Error".into(),
                error: Some(format!("build request: {e}")),
                ..Default::default()
            };
        }
    };

    let start = Instant::now();
    let send_fut = client.send(req);
    let result = smol::future::or(
        async {
            smol::Timer::after(Duration::from_secs(timeout_secs.max(1))).await;
            Err(anyhow::anyhow!("request timed out after {timeout_secs}s"))
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
                status_text: "Error".into(),
                time_ms,
                size: 0,
                headers: Vec::new(),
                body: String::new(),
                is_json: false,
                error: Some(format!("{e}")),
                streaming: false,
            };
        }
    };

    let status = resp.status().as_u16();
    let status_text = resp.status().canonical_reason().unwrap_or("").to_string();

    let mut headers: Vec<KeyValue> = Vec::new();
    for (name, value) in resp.headers().iter() {
        headers.push(KeyValue::new(
            name.as_str(),
            value.to_str().unwrap_or("<binary>"),
        ));
    }

    // Read the body fully into memory.
    let mut body = resp.into_body();
    let mut buf = Vec::new();
    let _ = body.read_to_end(&mut buf).await;
    let size = buf.len() as u64;

    let is_json = headers
        .iter()
        .any(|h| h.key.eq_ignore_ascii_case("content-type") && h.value.contains("json"));
    let body_text = String::from_utf8_lossy(&buf).to_string();
    let body_text = if is_json {
        // Best-effort pretty print.
        match serde_json::from_str::<serde_json::Value>(&body_text) {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or(body_text),
            Err(_) => body_text,
        }
    } else {
        body_text
    };

    Response {
        status,
        status_text,
        time_ms,
        size,
        headers,
        body: body_text,
        is_json,
        error: None,
        streaming: false,
    }
}
