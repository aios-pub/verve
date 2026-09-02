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

/// One structural `multipart/form-data` part, kept alongside the raw wire
/// bytes so the "实际请求" display and the curl export can render the parts
/// (file names, sizes) without re-parsing the binary body.
pub enum PreparedFormPart {
    Field { name: String, value: String },
    File {
        name: String,
        filename: String,
        /// Absolute/source path the bytes were read from (used by `-F`).
        path: String,
        mime: &'static str,
        size: usize,
    },
}

/// A fully-resolved request ready to send.
pub struct PreparedRequest {
    pub method: RequestMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Non-empty only for `multipart/form-data` bodies.
    pub form_parts: Vec<PreparedFormPart>,
}

/// Cap for the body text shown in the "实际请求" tab / curl snapshot so huge
/// uploads (e.g. file parts) don't bloat the response model.
const ACTUAL_BODY_DISPLAY_MAX: usize = 64 * 1024;

/// Lossy UTF-8 body for display, truncated at a char boundary when huge.
fn display_body(body: &[u8]) -> String {
    let owned = String::from_utf8_lossy(body).into_owned();
    if owned.len() <= ACTUAL_BODY_DISPLAY_MAX {
        return owned;
    }
    let mut end = ACTUAL_BODY_DISPLAY_MAX;
    while end > 0 && !owned.is_char_boundary(end) {
        end -= 1;
    }
    let mut head = owned[..end].to_string();
    head.push_str(&format!("\n…（已截断，完整请求体共 {} 字节）", body.len()));
    head
}

impl PreparedRequest {
    /// Render the request that was actually sent: request line (method +
    /// final URL with substituted variables and appended query params),
    /// headers, and body. This is the post-`prepare()` truth, not the
    /// authored template.
    pub fn request_text(&self) -> String {
        let mut text = format!("{} {}", self.method, self.url);
        if !self.headers.is_empty() {
            text.push_str("\n\n[Headers]");
            for (k, v) in &self.headers {
                text.push_str(&format!("\n{k}: {v}"));
            }
        }
        if !self.form_parts.is_empty() {
            // Render multipart bodies structurally: file bytes are binary and
            // would only show as U+FFFD noise in a lossy dump.
            let boundary = self
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .and_then(|(_, v)| extract_boundary(v))
                .unwrap_or_default();
            text.push_str("\n\n[Body] multipart/form-data\n");
            for part in &self.form_parts {
                match part {
                    PreparedFormPart::Field { name, value } => text.push_str(&format!(
                        "--{boundary}\nContent-Disposition: form-data; name=\"{name}\"\n\n{value}\n"
                    )),
                    PreparedFormPart::File {
                        name,
                        filename,
                        mime,
                        size,
                        ..
                    } => text.push_str(&format!(
                        "--{boundary}\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\nContent-Type: {mime}\n\n<{filename}，{size} 字节二进制>\n"
                    )),
                }
            }
            text.push_str(&format!("--{boundary}--\n"));
        } else if !self.body.is_empty() {
            text.push_str("\n\n[Body]\n");
            text.push_str(&display_body(&self.body));
        }
        text
    }

    /// The actually-sent request as an executable curl command. Auth is
    /// already baked into the headers/URL by `prepare()`, and query params
    /// are already encoded into the URL.
    pub fn to_curl(&self) -> String {
        if !self.form_parts.is_empty() {
            // Multipart must export as `-F` parts — the raw wire bytes contain
            // binary file data that a `-d '...'` literal can't carry. Drop the
            // (auto-generated) multipart Content-Type so curl derives its own
            // boundary for `-F`; a pinned `-H` boundary would desync from it.
            let headers: Vec<(String, String)> = self
                .headers
                .iter()
                .filter(|(k, v)| {
                    !(k.eq_ignore_ascii_case("content-type")
                        && v.to_ascii_lowercase().starts_with("multipart/"))
                })
                .cloned()
                .collect();
            let parts = self
                .form_parts
                .iter()
                .map(|p| match p {
                    PreparedFormPart::Field { name, value } => super::curl::CurlFormPart::Field {
                        name: name.clone(),
                        value: value.clone(),
                    },
                    PreparedFormPart::File { name, path, .. } => super::curl::CurlFormPart::File {
                        name: name.clone(),
                        path: path.clone(),
                    },
                })
                .collect();
            return super::curl::render(&super::curl::CurlSpec {
                method: self.method,
                url: self.url.clone(),
                params: Vec::new(),
                headers,
                cookies: Vec::new(),
                auth: AuthConfig::default(),
                body: super::curl::CurlBody::Form(parts),
            });
        }
        let content_type = self
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "text/plain".to_string());
        let body = if self.body.is_empty() {
            super::curl::CurlBody::None
        } else {
            super::curl::CurlBody::Raw {
                text: display_body(&self.body),
                content_type,
            }
        };
        super::curl::render(&super::curl::CurlSpec {
            method: self.method,
            url: self.url.clone(),
            params: Vec::new(),
            headers: self.headers.clone(),
            cookies: Vec::new(),
            auth: AuthConfig::default(),
            body,
        })
    }
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
    let mut form_parts: Vec<PreparedFormPart> = Vec::new();
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
            // Exactly one Content-Type header must reach the wire. `Builder::
            // header()` appends, so blindly pushing a second one (next to a
            // stale user `application/json` or a Postman-imported bare
            // `multipart/form-data`) makes servers parse the body with the
            // wrong type/boundary. If the user set a Content-Type, honour its
            // `boundary=` when present; otherwise replace the value in place
            // (Postman semantics: the body type decides the Content-Type).
            let boundary = match out_headers
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                Some((_k, v)) => match extract_boundary(v) {
                    Some(b) => b,
                    None => {
                        let b = format!("verve-{}", uuid::Uuid::new_v4().simple());
                        *v = format!("multipart/form-data; boundary={b}");
                        b
                    }
                },
                None => {
                    let b = format!("verve-{}", uuid::Uuid::new_v4().simple());
                    out_headers.push((
                        "Content-Type".into(),
                        format!("multipart/form-data; boundary={b}"),
                    ));
                    b
                }
            };
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
                                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n",
                                    escape_header_param(&name),
                                    escape_header_param(&filename)
                                )
                                .as_bytes(),
                            );
                            body_bytes.extend_from_slice(&data);
                            body_bytes.extend_from_slice(b"\r\n");
                            form_parts.push(PreparedFormPart::File {
                                name,
                                filename,
                                path,
                                mime,
                                size: data.len(),
                            });
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("read file {path}: {e}"));
                        }
                    }
                } else {
                    let value = super::variable::substitute(&kv.value, vars);
                    body_bytes.extend_from_slice(
                        format!(
                            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{value}\r\n",
                            escape_header_param(&name)
                        )
                        .as_bytes(),
                    );
                    form_parts.push(PreparedFormPart::Field { name, value });
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
        form_parts,
    })
}

fn ensure_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

/// Pull `boundary=...` out of a Content-Type value, honouring quoted values
/// (e.g. a Postman-imported `multipart/form-data; boundary="----abc"`). Only
/// ASCII markers are searched, so byte offsets map 1:1 onto the original
/// string and every slice lands on a char boundary.
fn extract_boundary(content_type: &str) -> Option<String> {
    let start = content_type
        .to_ascii_lowercase()
        .find("boundary=")
        .map(|i| i + "boundary=".len())?;
    let rest = content_type.get(start..)?;
    let end = rest.find(';').unwrap_or(rest.len());
    let trimmed = rest.get(..end)?.trim().trim_matches('"');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Make a string safe to embed inside a quoted Content-Disposition parameter:
/// escape backslash/quote and flatten CR/LF (a header value must stay on one
/// line — unescaped newlines would split/inject headers).
fn escape_header_param(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\r' | '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
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
                actual_request: None,
                actual_curl: None,
                received_at: Some(Response::now_stamp()),
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
        actual_request: None,
        actual_curl: None,
        received_at: Some(Response::now_stamp()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared() -> PreparedRequest {
        PreparedRequest {
            method: RequestMethod::Get,
            url: "https://api.io/users?page=1&kw=%E4%B8%AD".to_string(),
            headers: vec![
                ("Authorization".to_string(), "Bearer tok".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            body: b"{\"a\":1}".to_vec(),
            form_parts: Vec::new(),
        }
    }

    fn form_data_request(
        form: Vec<KeyValue>,
        headers: Vec<KeyValue>,
    ) -> Result<PreparedRequest> {
        form_data_request_at("http://api.io/up", form, headers)
    }

    fn form_data_request_at(
        url: &str,
        form: Vec<KeyValue>,
        headers: Vec<KeyValue>,
    ) -> Result<PreparedRequest> {
        prepare(
            RequestMethod::Post,
            url,
            &[],
            &headers,
            &[],
            &[],
            &AuthConfig::default(),
            &crate::state::models::RequestBody {
                body_type: BodyType::FormData,
                form_data: form,
                ..Default::default()
            },
            &BTreeMap::new(),
            30,
        )
    }

    fn content_types(p: &PreparedRequest) -> Vec<&String> {
        p.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v)
            .collect()
    }

    #[test]
    fn form_data_without_user_ct_injects_exactly_one() {
        let p = form_data_request(vec![KeyValue::new("a", "1")], Vec::new())
            .expect("prepare form-data");
        let cts = content_types(&p);
        assert_eq!(cts.len(), 1, "exactly one Content-Type: {:?}", cts);
        assert!(
            cts[0].starts_with("multipart/form-data; boundary="),
            "{:?}",
            cts[0]
        );
        let boundary = extract_boundary(cts[0]).expect("generated boundary");
        let body = String::from_utf8(p.body.clone()).expect("text-only parts are utf-8");
        assert!(
            body.starts_with(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n")),
            "{body}"
        );
        assert!(body.ends_with(&format!("--{boundary}--\r\n")), "{body}");
        // The display renders parts structurally instead of dumping bytes.
        let text = p.request_text();
        assert!(text.contains("[Body] multipart/form-data"), "{text}");
        assert!(text.contains("name=\"a\"\n\n1\n"), "{text}");
    }

    #[test]
    fn form_data_replaces_user_content_type_without_boundary() {
        // A stale JSON Content-Type must be replaced in place, not appended
        // next to — two Content-Type headers break server-side multipart
        // parsing.
        let p = form_data_request(
            vec![KeyValue::new("a", "1")],
            vec![KeyValue::new("Content-Type", "application/json")],
        )
        .expect("prepare form-data");
        let cts = content_types(&p);
        assert_eq!(cts.len(), 1, "no duplicate Content-Type: {:?}", cts);
        assert!(
            cts[0].starts_with("multipart/form-data; boundary="),
            "{:?}",
            cts[0]
        );
        let boundary = extract_boundary(cts[0]).expect("replaced boundary");
        assert!(String::from_utf8_lossy(&p.body).contains(&format!("--{boundary}\r\n")));
    }

    #[test]
    fn form_data_honours_user_boundary() {
        let p = form_data_request(
            vec![KeyValue::new("a", "1")],
            vec![KeyValue::new(
                "Content-Type",
                "multipart/form-data; boundary=----webkit",
            )],
        )
        .expect("prepare form-data");
        let cts = content_types(&p);
        assert_eq!(cts.len(), 1);
        assert_eq!(cts[0], "multipart/form-data; boundary=----webkit");
        let body = String::from_utf8(p.body.clone()).expect("utf-8");
        assert!(body.contains("------webkit\r\n"), "{body}");
        assert!(body.ends_with("------webkit--\r\n"), "{body}");
    }

    #[test]
    fn form_data_escapes_header_params() {
        let kv = KeyValue {
            enabled: true,
            key: "a\"b\nc".into(),
            value: "v".into(),
            ..KeyValue::default()
        };
        let p = form_data_request(vec![kv], Vec::new()).expect("prepare form-data");
        let body = String::from_utf8(p.body.clone()).expect("utf-8");
        assert!(
            body.contains("name=\"a\\\"b c\""),
            "quotes escaped, newline flattened: {body}"
        );
    }

    #[test]
    fn form_data_file_part_wire_format() {
        let path = std::env::temp_dir().join("verve_multipart_test.png");
        std::fs::write(&path, b"\x89PNG-fake").expect("write temp file");
        let kv = KeyValue {
            enabled: true,
            key: "file".into(),
            value: String::new(),
            file_path: Some(path.to_string_lossy().into_owned()),
            ..KeyValue::default()
        };
        let p = form_data_request(vec![kv], Vec::new()).expect("prepare form-data");
        let boundary = extract_boundary(content_types(&p)[0]).expect("boundary");
        let prefix = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"verve_multipart_test.png\"\r\nContent-Type: image/png\r\n\r\n"
        );
        assert!(
            p.body.starts_with(prefix.as_bytes()),
            "file part header: {}",
            String::from_utf8_lossy(&p.body)
        );
        let hdr_end = prefix.len();
        assert_eq!(&p.body[hdr_end..hdr_end + 9], b"\x89PNG-fake");
        assert_eq!(&p.body[hdr_end + 9..hdr_end + 11], b"\r\n");
        assert!(p.body.ends_with(format!("--{boundary}--\r\n").as_bytes()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn form_data_to_curl_uses_dash_f() {
        let path = std::env::temp_dir().join("verve_multipart_test.png");
        std::fs::write(&path, b"\x89PNG-fake").expect("write temp file");
        let kv = KeyValue {
            enabled: true,
            key: "file".into(),
            value: String::new(),
            file_path: Some(path.to_string_lossy().into_owned()),
            ..KeyValue::default()
        };
        let p = form_data_request(
            vec![kv, KeyValue::new("note", "hi")],
            Vec::new(),
        )
        .expect("prepare form-data");
        let curl = p.to_curl();
        assert!(curl.contains("-F 'file=@\""), "{curl}");
        assert!(curl.contains("-F 'note=hi'"), "{curl}");
        assert!(!curl.contains("-d "), "multipart must not use -d: {curl}");
        assert!(
            !curl.contains("boundary"),
            "curl derives the boundary for -F itself: {curl}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn request_text_shows_final_url_headers_body() {
        let text = prepared().request_text();
        assert!(text.contains("GET https://api.io/users?page=1&kw=%E4%B8%AD"), "{text}");
        assert!(text.contains("[Headers]"), "{text}");
        assert!(text.contains("Authorization: Bearer tok"), "{text}");
        assert!(text.contains("[Body]"), "{text}");
        assert!(text.contains("{\"a\":1}"), "{text}");
    }

    #[test]
    fn to_curl_renders_prepared_request_verbatim() {
        let out = prepared().to_curl();
        // URL (params already encoded in) + substituted headers + body.
        assert!(out.contains("-X GET"), "{out}");
        assert!(
            out.contains("'https://api.io/users?page=1&kw=%E4%B8%AD'"),
            "{out}"
        );
        assert!(out.contains("-H 'Authorization: Bearer tok'"), "{out}");
        // Content-Type already present → not injected twice.
        assert_eq!(out.matches("Content-Type").count(), 1, "{out}");
        assert!(out.contains("-d '{\"a\":1}'"), "{out}");
    }

    #[test]
    fn empty_body_renders_no_data_flag() {
        let mut p = prepared();
        p.body = Vec::new();
        let out = p.to_curl();
        assert!(!out.contains("-d"), "{out}");
        let text = p.request_text();
        assert!(!text.contains("[Body]"), "{text}");
    }

    #[test]
    fn huge_body_is_truncated_at_char_boundary() {
        let mut p = prepared();
        p.body = "中".repeat(40 * 1024).into_bytes(); // 120KB of multibyte chars
        let text = p.request_text();
        assert!(text.contains("已截断"), "{text}");
        // Truncation must not split a UTF-8 char: the lossy decode never
        // produces U+FFFD from our own slicing (we cut at a boundary).
        assert!(!text.contains('\u{FFFD}'), "{text}");
    }

    /// End-to-end: send a real form-data request through `execute()` (the
    /// actual reqwest client) against a local TCP server that dumps the raw
    /// wire bytes. Guards against the send layer mangling the multipart body
    /// (duplicate Content-Type, header/body boundary mismatch, truncation) —
    /// the failure an axum/multipart server reports as "读取表单字段失败".
    #[test]
    fn form_data_wire_bytes_survive_the_real_client() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let raw: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let raw_srv = raw.clone();
        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            sock.set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .ok();
            let mut buf = [0u8; 8192];
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                {
                    let mut g = raw_srv.lock().expect("raw");
                    g.extend_from_slice(&buf[..n]);
                    // Respond once the headers and the full body (per
                    // Content-Length) have arrived, so the client can finish.
                    let Some(header_end) = find_subsequence(&g, b"\r\n\r\n") else {
                        continue;
                    };
                    let head =
                        String::from_utf8_lossy(&g[..header_end]).to_ascii_lowercase();
                    let cl = head.lines().find_map(|l| {
                        l.strip_prefix("content-length:")?
                            .trim()
                            .parse::<usize>()
                            .ok()
                    });
                    let Some(cl) = cl else { continue };
                    if g.len() < header_end + 4 + cl {
                        continue;
                    }
                }
                sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .ok();
                break;
            }
        });

        let file_path = std::env::temp_dir().join("verve_wire_test.png");
        std::fs::write(&file_path, b"\x89PNG-wire").expect("write temp file");
        let form = vec![
            KeyValue::new("note", "hi"),
            KeyValue {
                enabled: true,
                key: "file".into(),
                value: String::new(),
                file_path: Some(file_path.to_string_lossy().into_owned()),
                ..KeyValue::default()
            },
        ];
        // Postman-style imported header WITH its own boundary: the wire body
        // must reuse it, and no second Content-Type may appear.
        let headers = vec![KeyValue::new(
            "Content-Type",
            "multipart/form-data; boundary=postman-bnd",
        )];
        let prepared =
            form_data_request_at(&format!("http://127.0.0.1:{port}/up"), form, headers)
                .expect("prepare form-data");

        let client = reqwest_client::ReqwestClient::user_agent("verve-test").expect("client");
        let resp = smol::block_on(execute(&client, prepared, 10));
        assert_eq!(resp.status, 200, "error: {:?}", resp.error);
        let _ = std::fs::remove_file(&file_path);

        let raw = raw.lock().expect("raw").clone();
        let header_end = find_subsequence(&raw, b"\r\n\r\n").expect("header terminator");
        let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
        let body = raw[header_end + 4..].to_vec();

        // Exactly one Content-Type on the wire.
        let ct_count = head
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("content-type:"))
            .count();
        assert_eq!(ct_count, 1, "one Content-Type on the wire:\n{head}");

        // The boundary in the header matches the delimiters in the body.
        let ct_line = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
            .expect("content-type line");
        assert_eq!(
            extract_boundary(ct_line).expect("wire boundary"),
            "postman-bnd"
        );
        let body_text = String::from_utf8_lossy(&body);
        assert!(body_text.contains("--postman-bnd\r\n"), "{body_text}");
        assert!(
            body.ends_with(b"--postman-bnd--\r\n"),
            "closing delimiter intact: {body_text}"
        );
        assert!(
            find_subsequence(&body, b"\x89PNG-wire").is_some(),
            "file bytes intact"
        );
    }

    fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
