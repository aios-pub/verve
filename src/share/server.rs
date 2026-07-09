//! HTTP server hosting shared API documentation.
//!
//! A `smol` + `TcpListener` responder (no extra dependencies), shared by two run
//! modes through a single [`ServerState`] + [`ProjectProvider`] abstraction:
//!
//! - **Desktop** (embedded): binds `127.0.0.1:<port>`, projects come from GPUI
//!   state via a closure, configs from `shares.json`. Started by `VerveApp`.
//! - **Cloud** (`verve-server` binary): binds `0.0.0.0:<port>`, projects + shares
//!   come from the file-backed [`crate::share::cloud::CloudStore`], and an admin
//!   UI at `/admin` lets operators upload projects and manage shares.
//!
//! **Strict access control** is enforced server-side on every `/s/<id>` route:
//! 1. **Expiration** — past `created_at + expire.days*86400`, returns `410 Gone`.
//! 2. **Password** — if `!access.public`, a valid signed cookie is required;
//!    otherwise the client is redirected to `/s/<id>/password`.
//! 3. **Scope** — `Request` scope only ever serves the target request's doc;
//!    `Folder` scope only serves that folder's subtree.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use smol::io::{AsyncReadExt, AsyncWriteExt};
use smol::net::{TcpListener, TcpStream};

use crate::share::html;
use crate::share::models::{now_ts, ShareConfig};
use crate::state::models::Project;

/// Default share-server port (mock server uses 3059).
pub const DEFAULT_PORT: u16 = 3097;

/// The server's data sources: a config store plus a project resolver. Both the
/// desktop closure provider and the cloud file store implement this through a
/// single boxed trait, so [`route`] is identical in either mode.
pub struct ServerState {
    /// Share configs (hot-swappable; shared with the UI / admin API).
    pub configs: Arc<RwLock<Vec<ShareConfig>>>,
    /// Resolves a project id → `Project`. In desktop mode this reads GPUI
    /// state; in cloud mode it reads the file store.
    pub provider: Box<dyn ProjectProvider + Send + Sync>,
    /// When set (cloud mode), the server also serves the admin UI and accepts
    /// project uploads. `None` in desktop mode (no admin surface).
    pub cloud: Option<Arc<crate::share::cloud::CloudStore>>,
}

/// Abstracts project resolution so the same router works for desktop (closure
/// over GPUI state) and cloud (file store). Kept as an object trait so the
/// server doesn't depend on GPUI.
pub trait ProjectProvider {
    fn get_project(&self, id: &str) -> Option<Project>;
}

/// Adapter wrapping a closure into a [`ProjectProvider`] (used by the desktop
/// app, which resolves projects from GPUI state / `workspace.json`).
pub struct ClosureProvider<F>(pub F)
where
    F: Fn(&str) -> Option<Project> + Send + Sync;

impl<F> ProjectProvider for ClosureProvider<F>
where
    F: Fn(&str) -> Option<Project> + Send + Sync,
{
    fn get_project(&self, id: &str) -> Option<Project> {
        (self.0)(id)
    }
}

impl ProjectProvider for crate::share::cloud::CloudStore {
    fn get_project(&self, id: &str) -> Option<Project> {
        crate::share::cloud::CloudStore::get_project(self, id)
    }
}

/// A live share server: the running accept-loop task plus its shared state.
pub struct ShareServer {
    pub task: smol::Task<()>,
    pub state: Arc<ServerState>,
}

impl ShareServer {
    /// Stop the server (dropping the task cancels the accept loop).
    pub fn stop(self) {
        self.task.detach();
    }
}

/// Build the shared config store preloaded with the given configs.
pub fn config_store(configs: Vec<ShareConfig>) -> Arc<RwLock<Vec<ShareConfig>>> {
    Arc::new(RwLock::new(configs))
}

/// Start the server in **desktop** mode: localhost-only, projects from a
/// closure, no admin UI. The desktop app calls this.
pub fn start_desktop<F>(
    port: u16,
    configs: Arc<RwLock<Vec<ShareConfig>>>,
    project_provider: F,
) -> ShareServer
where
    F: Fn(&str) -> Option<Project> + Send + Sync + 'static,
{
    let state = Arc::new(ServerState {
        configs,
        provider: Box::new(ClosureProvider(project_provider)),
        cloud: None,
    });
    start_with_state("127.0.0.1".to_string(), port, state)
}

/// Start the server in **cloud** mode: binds the given host (typically
/// `0.0.0.0`), projects + shares from the file store, admin UI enabled.
pub fn start_cloud(
    host: String,
    port: u16,
    store: Arc<crate::share::cloud::CloudStore>,
) -> ShareServer {
    let configs = Arc::new(RwLock::new(store.shares()));
    let state = Arc::new(ServerState {
        configs,
        provider: Box::new(crate::share::cloud::CloudStore::clone(&store)),
        cloud: Some(store),
    });
    start_with_state(host, port, state)
}

/// Shared accept loop used by both modes.
fn start_with_state(host: String, port: u16, state: Arc<ServerState>) -> ShareServer {
    let task_state = state.clone();
    let task = smol::spawn(async move {
        let addr = format!("{host}:{port}");
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("verve-server bind failed on {addr}: {e}");
                return;
            }
        };
        log::info!("verve-server listening on http://{addr}");
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let state = task_state.clone();
            smol::spawn(handle(stream, state)).detach();
        }
    });
    ShareServer { task, state }
}

async fn handle(mut stream: TcpStream, state: Arc<ServerState>) {
    let mut buf = vec![0u8; 1_048_576]; // 1 MiB — admin uploads can be large
    let n = match stream.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let raw = String::from_utf8_lossy(&buf[..n]).to_string();
    let req = match parse_request(&raw) {
        Ok(r) => r,
        Err(_) => {
            let resp = response(400, "text/plain", b"Bad Request");
            let _ = stream.write_all(&resp).await;
            let _ = stream.flush().await;
            return;
        }
    };

    let resp = route(&req, &state);
    let _ = stream.write_all(&resp).await;
    let _ = stream.flush().await;
}

/// A minimal parsed HTTP request (enough for our routes + password POST).
struct ParsedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

fn parse_request(raw: &str) -> Result<ParsedRequest, &'static str> {
    let mut lines = raw.split("\r\n");
    let request_line = lines.next().ok_or("no request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("no method")?.to_string();
    let path = parts.next().ok_or("no path")?.to_string();

    let mut headers = HashMap::new();
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let body = lines.collect::<Vec<_>>().join("\r\n");
    Ok(ParsedRequest { method, path, headers, body })
}

/// Build the full HTTP/1.1 response (status line + headers + body).
fn route(req: &ParsedRequest, state: &ServerState) -> Vec<u8> {
    let path = req.path.split('?').next().unwrap_or(&req.path);

    // ---- Admin UI (cloud mode only) --------------------------------------
    if path == "/admin" || path.starts_with("/admin/") {
        return route_admin(req, state);
    }

    // ---- Static / health --------------------------------------------------
    if path == "/" {
        // A friendly landing page: cloud mode shows the admin link, desktop
        // mode shows a plain "server running" notice.
        return response_bytes(
            200,
            "text/html; charset=utf-8",
            landing_html(state).into_bytes(),
        );
    }
    if path == "/health" {
        return response(200, "application/json", b"{\"status\":\"ok\"}");
    }

    // ---- /s/<id>/... -------------------------------------------------------
    let rest = match path.strip_prefix("/s/") {
        Some(r) => r,
        None => return response(404, "text/plain", b"Not Found"),
    };
    let (id, sub) = match rest.split_once('/') {
        Some((id, sub)) => (id, Some(sub)),
        None => (rest, None),
    };

    // Look up the share config (clone out to release the lock early).
    let cfg = state
        .configs
        .read()
        .ok()
        .and_then(|guard| guard.iter().find(|c| c.id == id).cloned());

    let Some(cfg) = cfg else {
        return response(404, "text/html; charset=utf-8", NOT_FOUND_HTML.as_bytes());
    };

    // ---- Strict enforcement: expiration -----------------------------------
    let now = now_ts();
    if !cfg.is_valid_at(now) {
        return response(410, "text/html; charset=utf-8", expired_html(&cfg).as_bytes());
    }

    // ---- /s/<id>/password (GET form + POST verify) ------------------------
    if sub == Some("password") {
        return handle_password(&cfg, req);
    }

    // ---- Strict enforcement: password -------------------------------------
    if !cfg.access.public {
        let cookie = req.headers.get("cookie").cloned().unwrap_or_default();
        if !valid_auth_cookie(&cfg, &cookie) {
            // Redirect to the password page.
            let mut out = String::new();
            out.push_str("HTTP/1.1 302 Found\r\n");
            out.push_str(&format!("Location: /s/{}/password\r\n", cfg.id));
            out.push_str("Content-Length: 0\r\n\r\n");
            return out.into_bytes();
        }
    }

    // ---- Resolve the project ----------------------------------------------
    let project = match state.provider.get_project(&cfg.project_id) {
        Some(p) => p,
        None => {
            return response(
                410,
                "text/html; charset=utf-8",
                project_gone_html(&cfg).as_bytes(),
            );
        }
    };

    // ---- Route by sub-path -------------------------------------------------
    match sub {
        None => {
            // Record a visit (best-effort, ignore lock errors).
            record_visit(&state.configs, &cfg.id);
            let html = html::render_doc_html(&cfg, &project);
            response_bytes(200, "text/html; charset=utf-8", html.into_bytes())
        }
        Some("logo") => {
            if let Some(path) = &cfg.logo_path {
                match std::fs::read(path) {
                    Ok(bytes) => {
                        let mime = mime_from_ext(path).unwrap_or("image/png");
                        response_bytes(200, mime, bytes)
                    }
                    Err(_) => response(404, "text/plain", b"logo not found"),
                }
            } else {
                response(404, "text/plain", b"no logo")
            }
        }
        Some("export.html") => {
            let html = html::render_doc_html(&cfg, &project);
            let mut out = String::new();
            out.push_str("HTTP/1.1 200 OK\r\n");
            out.push_str("Content-Type: text/html; charset=utf-8\r\n");
            out.push_str(&format!(
                "Content-Disposition: attachment; filename=\"{}.html\"\r\n",
                sanitize_filename(&cfg.display_title())
            ));
            out.push_str(&format!("Content-Length: {}\r\n", html.len()));
            out.push_str("Connection: close\r\n\r\n");
            out.push_str(&html);
            out.into_bytes()
        }
        Some(other) if other.starts_with("doc/") => {
            let request_id = &other[4..];
            // Strict scope enforcement: only serve docs within the share's scope.
            if !in_scope(&cfg, &project, request_id) {
                return response(403, "text/plain", b"Forbidden: out of share scope");
            }
            if let Some((_, req)) = project.find_request(request_id) {
                let fragment = html::render_request_fragment(req, &cfg.field_display);
                response_bytes(200, "text/html; charset=utf-8", fragment.into_bytes())
            } else {
                response(404, "text/plain", b"request not found")
            }
        }
        Some("api") => {
            // JSON tree of in-scope requests (for dynamic sidebars).
            let json = build_tree_json(&cfg, &project);
            response_bytes(200, "application/json", json.into_bytes())
        }
        Some(_) => response(404, "text/plain", b"Not Found"),
    }
}

// ===========================================================================
// Admin UI (cloud mode)
// ===========================================================================

/// Route admin requests. Only reachable when `state.cloud` is set (cloud mode).
fn route_admin(req: &ParsedRequest, state: &ServerState) -> Vec<u8> {
    let Some(store) = state.cloud.clone() else {
        return response(404, "text/plain", b"admin disabled (desktop mode)");
    };
    let path = req.path.split('?').next().unwrap_or(&req.path);

    match (req.method.as_str(), path) {
        // ---- Dashboard ----
        ("GET", "/admin") => {
            let html = admin_html(&store);
            response_bytes(200, "text/html; charset=utf-8", html.into_bytes())
        }

        // ---- Upload a project (POST body = raw project JSON) ----
        ("POST", "/admin/api/projects") => {
            // The body may carry a Content-Length larger than our read buffer;
            // for very large files the admin should pre-split. Here we accept
            // up to the buffer size (1 MiB), which covers typical collections.
            let project = match serde_json::from_str::<Project>(&req.body) {
                Ok(p) => p,
                Err(e) => {
                    return response_bytes(
                        400,
                        "application/json",
                        format!(r#"{{"error":"invalid project json: {e}"}}"#)
                            .into_bytes(),
                    );
                }
            };
            let id = project.id.clone();
            let name = project.name.clone();
            store.upsert_project(project);
            log::info!("admin: uploaded project {id} ({name})");
            response_bytes(
                200,
                "application/json",
                format!(r#"{{"id":"{id}","name":"{name}"}}"#).into_bytes(),
            )
        }

        // ---- Delete a project ----
        ("POST", p) if p.starts_with("/admin/api/projects/") => {
            let id = p.trim_start_matches("/admin/api/projects/");
            store.delete_project(id);
            log::info!("admin: deleted project {id}");
            response_bytes(200, "application/json", b"{\"ok\":true}".to_vec())
        }

        // ---- Create a share (POST form-encoded: project_id, scope, ...) ----
        ("POST", "/admin/api/shares") => {
            let project_id = form_field(&req.body, "project_id").unwrap_or_default();
            let title = form_field(&req.body, "title").unwrap_or_default();
            let public = form_field(&req.body, "public").map(|v| v == "1").unwrap_or(true);
            let password = form_field(&req.body, "password");

            if store.get_project(&project_id).is_none() {
                return response_bytes(
                    400,
                    "application/json",
                    format!(r#"{{"error":"project not found: {project_id}"}}"#).into_bytes(),
                );
            }
            let project_name = store
                .get_project(&project_id)
                .map(|p| p.name)
                .unwrap_or_default();
            let mut cfg = crate::share::models::ShareConfig::new(&project_id, &project_name);
            cfg.title = title;
            cfg.access = if public {
                crate::share::models::AccessControl::public()
            } else {
                crate::share::models::AccessControl::password(password.unwrap_or_default())
            };
            let id = cfg.id.clone();
            store.upsert_share(cfg);
            // Reflect into the server's live config store.
            if let Ok(mut guard) = state.configs.write() {
                *guard = store.shares();
            }
            log::info!("admin: created share {id} for project {project_id}");
            response_bytes(
                200,
                "application/json",
                format!(r#"{{"id":"{id}"}}"#).into_bytes(),
            )
        }

        // ---- Delete a share ----
        ("POST", p) if p.starts_with("/admin/api/shares/") => {
            let id = p.trim_start_matches("/admin/api/shares/");
            store.delete_share(id);
            if let Ok(mut guard) = state.configs.write() {
                *guard = store.shares();
            }
            log::info!("admin: deleted share {id}");
            response_bytes(200, "application/json", b"{\"ok\":true}".to_vec())
        }

        _ => response(404, "text/plain", b"Not Found"),
    }
}

// ===========================================================================
// Password handling
// ===========================================================================

/// Handle the password form (GET = render, POST = verify + set cookie).
fn handle_password(cfg: &ShareConfig, req: &ParsedRequest) -> Vec<u8> {
    if req.method == "GET" {
        let html = password_form_html(cfg);
        return response_bytes(200, "text/html; charset=utf-8", html.into_bytes());
    }
    if req.method == "POST" {
        // Parse application/x-www-form-urlencoded body.
        let candidate = form_field(&req.body, "password");
        if cfg.access.accepts(candidate.as_deref()) {
            // Issue a signed cookie and redirect to the doc.
            let token = auth_cookie_token(cfg);
            let mut out = String::new();
            out.push_str("HTTP/1.1 302 Found\r\n");
            out.push_str(&format!("Location: /s/{}\r\n", cfg.id));
            out.push_str(&format!(
                "Set-Cookie: verve_auth_{}={}; Path=/s/{}; HttpOnly; Max-Age=86400; SameSite=Strict\r\n",
                cfg.id, token, cfg.id
            ));
            out.push_str("Content-Length: 0\r\n\r\n");
            return out.into_bytes();
        }
        // Wrong password: re-render with an error.
        let html = password_form_html_err(cfg, "密码错误，请重试。");
        return response_bytes(401, "text/html; charset=utf-8", html.into_bytes());
    }
    response(405, "text/plain", b"Method Not Allowed")
}

/// Whether `request_id` is within the share's scope.
fn in_scope(cfg: &ShareConfig, project: &Project, request_id: &str) -> bool {
    use crate::share::models::ShareScope;
    match cfg.scope {
        ShareScope::Project => project.find_request(request_id).is_some(),
        ShareScope::Request => cfg.target_id.as_deref() == Some(request_id),
        ShareScope::Folder => {
            let Some(folder_id) = cfg.target_id.as_deref() else {
                return false;
            };
            let Some((_, folder)) = project.find_folder(folder_id) else {
                return false;
            };
            request_in_folder(folder, request_id)
        }
    }
}

fn request_in_folder(folder: &crate::state::models::Folder, request_id: &str) -> bool {
    if folder.requests.iter().any(|r| r.id == request_id) {
        return true;
    }
    folder.folders.iter().any(|f| request_in_folder(f, request_id))
}

/// Build a JSON array of `{id, name, method, path}` for the in-scope requests.
fn build_tree_json(cfg: &ShareConfig, project: &Project) -> String {
    use crate::share::models::ShareScope;
    let mut entries: Vec<(String, &crate::state::models::ApiRequest)> = Vec::new();
    match cfg.scope {
        ShareScope::Project => {
            for req in &project.requests {
                entries.push((String::new(), req));
            }
            for folder in &project.folders {
                collect_tree(folder, "", &mut entries);
            }
        }
        ShareScope::Request => {
            if let Some(target) = cfg.target_id.as_deref() {
                if let Some((chain, req)) = project.find_request(target) {
                    let path = chain_names(project, &chain);
                    entries.push((path, req));
                }
            }
        }
        ShareScope::Folder => {
            if let Some(target) = cfg.target_id.as_deref() {
                if let Some((_, folder)) = project.find_folder(target) {
                    collect_tree(folder, "", &mut entries);
                }
            }
        }
    }
    let mut out = String::from("[");
    for (i, (path, req)) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"id":"{}","name":"{}","method":"{}","path":"{}"}}"#,
            json_escape(&req.id),
            json_escape(&req.name),
            req.method.badge_label(),
            json_escape(path)
        ));
    }
    out.push(']');
    out
}

fn collect_tree<'a>(
    folder: &'a crate::state::models::Folder,
    prefix: &str,
    out: &mut Vec<(String, &'a crate::state::models::ApiRequest)>,
) {
    let path = if prefix.is_empty() {
        folder.name.clone()
    } else {
        format!("{prefix} > {}", folder.name)
    };
    for req in &folder.requests {
        out.push((path.clone(), req));
    }
    for sub in &folder.folders {
        collect_tree(sub, &path, out);
    }
}

fn chain_names(project: &Project, chain: &[String]) -> String {
    let mut names: Vec<String> = Vec::new();
    for id in chain {
        if let Some((_, f)) = project.find_folder(id) {
            names.push(f.name.clone());
        }
    }
    names.join(" > ")
}

// ===========================================================================
// Auth cookie signing (simple token — offline/cloud tool, single secret)
// ===========================================================================

/// The signed token placed in the auth cookie. Combines the share id + password
/// so changing the password invalidates old cookies.
fn auth_cookie_token(cfg: &ShareConfig) -> String {
    let pw = cfg.access.password.clone().unwrap_or_default();
    format!("{}:{}", cfg.id, simple_hash(&pw))
}

fn valid_auth_cookie(cfg: &ShareConfig, cookie_header: &str) -> bool {
    let key = format!("verve_auth_{}=", cfg.id);
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&key) {
            return value == auth_cookie_token(cfg);
        }
    }
    false
}

/// A tiny non-cryptographic string hash (FNV-1a) — enough to obfuscate the
/// password in the cookie without pulling in a hashing crate.
fn simple_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

// ===========================================================================
// HTML pages: landing, admin dashboard, edge cases
// ===========================================================================

fn landing_html(state: &ServerState) -> String {
    let share_count = state.configs.read().map(|g| g.len()).unwrap_or(0);
    let admin_link = if state.cloud.is_some() {
        r#"<p>访问 <a href="/admin">管理后台</a> 上传项目并创建分享。</p>"#
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>Verve Server</title>
<style>body{{font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#f7f8fa;color:#1f2937;text-align:center}}
.card{{background:#fff;padding:40px;border-radius:12px;box-shadow:0 1px 3px rgba(0,0,0,.1)}}
h1{{color:#3b82f6;margin:0 0 8px}}a{{color:#3b82f6}}</style></head>
<body><div class="card"><h1>Verve Server</h1>
<p>文档分享服务运行中（{share_count} 个分享）</p>
{admin_link}</div></body></html>"#
    )
}

const NOT_FOUND_HTML: &str = r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>未找到</title>
<style>body{font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#f7f8fa;color:#6b7280}</style></head>
<body><div style="text-align:center"><h1>404</h1><p>分享文档不存在或已被删除。</p></div></body></html>"#;

fn expired_html(cfg: &ShareConfig) -> String {
    format!(
        r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>文档已过期</title>
<style>body{{font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#f7f8fa;color:#6b7280}}</style></head>
<body><div style="text-align:center"><h1>文档已过期</h1><p>「{}」的分享有效期已结束。</p></div></body></html>"#,
        html_escape(&cfg.display_title())
    )
}

fn project_gone_html(cfg: &ShareConfig) -> String {
    format!(
        r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>项目不可用</title>
<style>body{{font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#f7f8fa;color:#6b7280}}</style></head>
<body><div style="text-align:center"><h1>项目不可用</h1><p>分享对应的项目「{}」当前无法访问。</p></div></body></html>"#,
        html_escape(&cfg.project_name)
    )
}

fn password_form_html(cfg: &ShareConfig) -> String {
    password_form_html_err(cfg, "")
}

fn password_form_html_err(cfg: &ShareConfig, err: &str) -> String {
    format!(
        r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>密码访问</title>
<style>
body{{font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#f7f8fa;color:#1f2937}}
.card{{background:#fff;padding:32px;border-radius:12px;box-shadow:0 1px 3px rgba(0,0,0,0.1);width:320px;text-align:center}}
input{{width:100%;padding:10px;margin:8px 0;border:1px solid #e5e7eb;border-radius:6px;font-size:14px;box-sizing:border-box}}
button{{width:100%;padding:10px;background:#3b82f6;color:#fff;border:none;border-radius:6px;font-size:14px;cursor:pointer}}
.error{{color:#dc2626;font-size:13px;min-height:18px}}
</style></head>
<body><div class="card">
<h2 style="margin:0 0 8px">{}</h2>
<p style="color:#6b7280;font-size:13px;margin:0 0 16px">该文档需要密码访问</p>
<form method="POST" action="/s/{}/password">
<input type="password" name="password" placeholder="请输入密码" autofocus>
<div class="error">{}</div>
<button type="submit">访问文档</button>
</form></div></body></html>"#,
        html_escape(&cfg.display_title()),
        cfg.id,
        html_escape(err)
    )
}

/// The cloud admin dashboard: list projects + shares, upload form, create-share
/// form. A single self-contained page with inline CSS + vanilla JS (fetch).
fn admin_html(store: &crate::share::cloud::CloudStore) -> String {
    let projects = store.list_projects();
    let shares = store.shares();

    let mut project_options = String::new();
    for (id, name) in &projects {
        project_options.push_str(&format!(
            r#"<option value="{id}">{}</option>"#,
            html_escape(name)
        ));
    }

    let mut project_rows = String::new();
    for (id, name) in &projects {
        project_rows.push_str(&format!(
            r#"<tr><td>{}</td><td><code>{}</code></td><td><button class="del" data-url="/admin/api/projects/{id}">删除</button></td></tr>"#,
            html_escape(name),
            &id[..id.len().min(8)]
        ));
    }

    let mut share_rows = String::new();
    let now = now_ts();
    for s in &shares {
        let expired = !s.is_valid_at(now);
        let status = if expired { "已过期" } else { "生效中" };
        let cls = if expired { "expired" } else { "active" };
        let project_name = store
            .get_project(&s.project_id)
            .map(|p| p.name)
            .unwrap_or_else(|| s.project_name.clone());
        share_rows.push_str(&format!(
            r#"<tr class="{cls}"><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><a href="/s/{}" target="_blank">打开</a> <button class="del" data-url="/admin/api/shares/{}">删除</button></td></tr>"#,
            html_escape(&s.display_title()),
            html_escape(&project_name),
            s.scope_label(),
            s.expire_label(),
            status,
            s.id,
            s.id,
        ));
    }

    format!(
        r##"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Verve 文档管理后台</title>
<style>
*{{box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC",sans-serif;margin:0;background:#f7f8fa;color:#1f2937;font-size:14px}}
.container{{max-width:1000px;margin:0 auto;padding:24px}}
h1{{color:#3b82f6;margin:0 0 4px;font-size:22px}}
.subtitle{{color:#6b7280;margin:0 0 24px}}
.card{{background:#fff;border-radius:10px;padding:20px;margin-bottom:20px;box-shadow:0 1px 3px rgba(0,0,0,.06)}}
.card h2{{margin:0 0 14px;font-size:16px;border-bottom:1px solid #e5e7eb;padding-bottom:8px}}
label{{display:block;font-weight:600;margin:8px 0 4px;font-size:13px}}
input[type=text],input[type=password],select,textarea{{width:100%;padding:8px 10px;border:1px solid #e5e7eb;border-radius:6px;font-size:13px}}
textarea{{min-height:120px;font-family:monospace}}
button,.btn{{background:#3b82f6;color:#fff;border:none;border-radius:6px;padding:8px 16px;font-size:13px;cursor:pointer;margin-top:8px}}
button:hover{{background:#2563eb}}
button.del{{background:#ef4444;padding:4px 10px;font-size:12px;margin:0}}
.row{{display:flex;gap:12px;flex-wrap:wrap}}
.row>div{{flex:1;min-width:200px}}
table{{width:100%;border-collapse:collapse;margin-top:8px}}
th,td{{padding:8px 10px;border-bottom:1px solid #e5e7eb;text-align:left;font-size:13px}}
th{{background:#f9fafb;font-weight:600;color:#6b7280}}
tr.expired{{opacity:.5}}
.badge{{display:inline-block;padding:2px 8px;border-radius:10px;font-size:11px;background:#dcfce7;color:#16a34a}}
.badge.expired{{background:#fee2e2;color:#dc2626}}
.form-inline{{display:flex;gap:8px;align-items:end;flex-wrap:wrap}}
.form-inline>div{{flex:1;min-width:140px}}
.msg{{padding:8px 12px;border-radius:6px;margin-top:8px;display:none}}
.msg.ok{{background:#dcfce7;color:#16a34a;display:block}}
.msg.err{{background:#fee2e2;color:#dc2626;display:block}}
</style></head>
<body><div class="container">
<h1>🗂️ Verve 文档管理</h1>
<p class="subtitle">上传项目 · 创建分享 · 在线浏览 API 文档</p>

<div class="card"><h2>📤 上传项目</h2>
<p style="color:#6b7280;font-size:13px;margin:0 0 8px">粘贴 Verve/Postman/Apipost 导出的项目 JSON（单个 <code>Project</code> 对象）。</p>
<textarea id="project-json" placeholder='{{"name":"我的项目","requests":[...]}}'></textarea>
<div id="upload-msg" class="msg"></div>
<button onclick="uploadProject()">上传项目</button>
</div>

<div class="card"><h2>🔗 创建分享</h2>
<form class="form-inline" onsubmit="createShare(event)">
<div><label>项目</label><select id="share-project">{project_options}</select></div>
<div><label>文档标题</label><input type="text" id="share-title" placeholder="可选"></div>
<div><label>访问限制</label><select id="share-public"><option value="1">公开</option><option value="0">密码</option></select></div>
<div id="pw-wrap" style="display:none"><label>密码</label><input type="password" id="share-password"></div>
<div><button type="submit" class="btn">创建分享</button></div>
</form>
<div id="share-msg" class="msg"></div>
</div>

<div class="card"><h2>📋 项目列表（{}）</h2>
<table><thead><tr><th>名称</th><th>ID</th><th>操作</th></tr></thead>
<tbody>{project_rows}</tbody></table>
</div>

<div class="card"><h2>🌐 分享列表（{}）</h2>
<table><thead><tr><th>标题</th><th>项目</th><th>范围</th><th>有效期</th><th>状态</th><th>操作</th></tr></thead>
<tbody>{share_rows}</tbody></table>
</div>

</div>
<script>
// Toggle password field when access-control changes.
document.getElementById('share-public').addEventListener('change', function() {{
  document.getElementById('pw-wrap').style.display = this.value === '0' ? '' : 'none';
}});
// Show a transient message in an element.
function showMsg(id, text, ok) {{
  const el = document.getElementById(id);
  el.textContent = text;
  el.className = 'msg ' + (ok ? 'ok' : 'err');
  setTimeout(() => {{ el.className = 'msg'; }}, 4000);
}}
// Upload a project.
async function uploadProject() {{
  const body = document.getElementById('project-json').value.trim();
  if (!body) {{ showMsg('upload-msg', '请粘贴项目 JSON', false); return; }}
  const r = await fetch('/admin/api/projects', {{ method: 'POST', body }});
  const j = await r.json();
  if (r.ok) {{ showMsg('upload-msg', '上传成功：' + (j.name||'') + '，刷新页面生效', true); }}
  else {{ showMsg('upload-msg', j.error || '上传失败', false); }}
}}
// Create a share.
async function createShare(e) {{
  e.preventDefault();
  const form = new URLSearchParams();
  form.set('project_id', document.getElementById('share-project').value);
  form.set('title', document.getElementById('share-title').value);
  form.set('public', document.getElementById('share-public').value);
  form.set('password', document.getElementById('share-password').value);
  const r = await fetch('/admin/api/shares', {{ method: 'POST', body: form }});
  const j = await r.json();
  if (r.ok) {{ showMsg('share-msg', '创建成功，刷新页面查看', true); }}
  else {{ showMsg('share-msg', j.error || '创建失败', false); }}
}}
// Delete buttons (event delegation).
document.body.addEventListener('click', async (e) => {{
  if (e.target.classList.contains('del')) {{
    if (!confirm('确认删除？此操作不可撤销。')) return;
    const url = e.target.dataset.url;
    const r = await fetch(url, {{ method: 'POST' }});
    if (r.ok) {{ e.target.closest('tr').remove(); }}
  }}
}});
</script>
</body></html>"##,
        projects.len(),
        shares.len(),
    )
}

// ===========================================================================
// HTTP response helpers
// ===========================================================================

fn response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    response_bytes(status, content_type, body.to_vec())
}

fn response_bytes(status: u16, content_type: &str, body: Vec<u8>) -> Vec<u8> {
    let status_text = status_text(status);
    let mut out = format!("HTTP/1.1 {status} {status_text}\r\n");
    out.push_str(&format!("Content-Type: {content_type}\r\n"));
    out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    out.push_str("Connection: close\r\n");
    out.push_str("Access-Control-Allow-Origin: *\r\n");
    // Prevent browsers from caching the doc page so updated server code is
    // always served (avoids showing stale HTML after the app is rebuilt).
    out.push_str("Cache-Control: no-cache, no-store, must-revalidate\r\n");
    out.push_str("Pragma: no-cache\r\n");
    out.push_str("Expires: 0\r\n\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&body);
    bytes
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        410 => "Gone",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// Extract a form field value from an urlencoded body.
fn form_field(body: &str, key: &str) -> Option<String> {
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if url_decode(k) == key {
                return Some(url_decode(v));
            }
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(' '),
            b'%' if i + 2 < bytes.len() => {
                if let Ok(b) = u8::from_str_radix(
                    &String::from_utf8_lossy(&bytes[i + 1..i + 3]),
                    16,
                ) {
                    out.push(b as char);
                    i += 2;
                } else {
                    out.push('%');
                }
            }
            c => out.push(c as char),
        }
        i += 1;
    }
    out
}

fn record_visit(configs: &Arc<RwLock<Vec<ShareConfig>>>, id: &str) {
    if let Ok(mut guard) = configs.write() {
        if let Some(cfg) = guard.iter_mut().find(|c| c.id == id) {
            cfg.visits += 1;
            cfg.last_visit = Some(now_ts());
        }
    }
}

fn mime_from_ext(path: &std::path::Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        _ => Some("image/png"),
    }
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::models::{AccessControl, Expiration};
    use crate::state::models::{ApiRequest, RequestMethod};

    fn make_cfg(public: bool, days: Option<u32>, password: &str) -> ShareConfig {
        let mut cfg = ShareConfig::new("p", "P");
        cfg.access = if public {
            AccessControl::public()
        } else {
            AccessControl::password(password)
        };
        if let Some(d) = days {
            cfg.expire = Expiration::Days(d);
        }
        cfg
    }

    #[test]
    fn expiration_blocks_expired_share() {
        let mut cfg = make_cfg(true, Some(1), "");
        cfg.created_at = now_ts() - 2 * 86_400; // 2 days ago, 1-day expiry
        assert!(!cfg.is_valid_at(now_ts()));
    }

    #[test]
    fn password_form_decodes_correctly() {
        assert_eq!(form_field("password=hello&x=1", "password"), Some("hello".into()));
        assert_eq!(form_field("password=hi%20there", "password"), Some("hi there".into()));
        assert_eq!(form_field("password=a+b", "password"), Some("a b".into()));
    }

    #[test]
    fn cookie_round_trip() {
        let cfg = make_cfg(false, None, "s3cret");
        let token = auth_cookie_token(&cfg);
        let cookie = format!("verve_auth_{}={}", cfg.id, token);
        assert!(valid_auth_cookie(&cfg, &cookie));
        assert!(!valid_auth_cookie(&cfg, "verve_auth_x=wrong"));
        assert!(!valid_auth_cookie(&cfg, ""));
    }

    #[test]
    fn in_scope_project_sees_all() {
        let mut project = crate::state::models::Project::new("P");
        let a = ApiRequest::new("A", RequestMethod::Get, "/a");
        let id_a = a.id.clone();
        project.requests.push(a);

        let cfg = make_cfg(true, None, "");
        assert!(in_scope(&cfg, &project, &id_a));
        assert!(!in_scope(&cfg, &project, "nonexistent"));
    }

    #[test]
    fn in_scope_request_only_target() {
        let mut project = crate::state::models::Project::new("P");
        let a = ApiRequest::new("A", RequestMethod::Get, "/a");
        let b = ApiRequest::new("B", RequestMethod::Get, "/b");
        let id_a = a.id.clone();
        let id_b = b.id.clone();
        project.requests.push(a);
        project.requests.push(b);

        let mut cfg = make_cfg(true, None, "");
        cfg.scope = crate::share::models::ShareScope::Request;
        cfg.target_id = Some(id_b.clone());

        assert!(!in_scope(&cfg, &project, &id_a)); // A is out of scope
        assert!(in_scope(&cfg, &project, &id_b)); // B is the target
    }

    #[test]
    fn simple_hash_is_deterministic() {
        assert_eq!(simple_hash("abc"), simple_hash("abc"));
        assert_ne!(simple_hash("abc"), simple_hash("abd"));
    }

    #[test]
    fn closure_provider_resolves() {
        let mut p = crate::state::models::Project::new("X");
        let id_for_lookup = p.id.clone();
        let id = p.id.clone();
        p.requests.push(ApiRequest::new("R", RequestMethod::Get, "/r"));
        let provider = ClosureProvider(move |qid: &str| {
            if qid == id {
                Some(p.clone())
            } else {
                None
            }
        });
        assert!(provider.get_project(&id_for_lookup).is_some());
        assert!(provider.get_project("nope").is_none());
    }
}
