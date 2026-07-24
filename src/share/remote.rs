//! Push shares to a remote `verve_server` instance over HTTP.
//!
//! When the user configures a remote service address (in Settings → Remote),
//! sharing a document uploads the project JSON to the remote server's admin
//! API and creates the share there, producing a publicly-accessible URL
//! (`http://<remote>/s/<id>`) instead of a localhost link.
//!
//! Admin endpoints (mirrors of `server.rs::route_admin`):
//! - `POST /admin/api/projects`  — body = raw Project JSON
//! - `POST /admin/api/shares`    — form-encoded (project_id, title, public, password)
//!
//! Both are protected by an optional bearer token (`Authorization: Bearer <token>`).

use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, Builder, HttpClient, HttpRequestExt as _, Method, RedirectPolicy};
use serde::Deserialize;

use crate::share::models::ShareConfig;
use crate::state::models::Project;
use crate::state::persistence::RemoteConfig;

/// JSON response from `POST /admin/api/projects`.
#[derive(Deserialize)]
struct ProjectUploadResponse {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    name: String,
}

/// JSON response from `POST /admin/api/shares`.
#[derive(Deserialize)]
struct ShareCreateResponse {
    id: String,
}

/// Build the base URL for a remote server from `RemoteConfig`.
/// Returns `None` when remote is disabled or no public host is configured.
fn remote_base_url(cfg: &RemoteConfig) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let host = cfg.public_host.as_ref()?.trim();
    if host.is_empty() {
        return None;
    }
    // Allow users to paste with or without http:// prefix.
    let host = if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("http://{}", host)
    };
    // Strip trailing slash.
    let host = host.trim_end_matches('/').to_string();
    Some(host)
}

/// Return true if the configured remote points to an external cloud server
/// (i.e. public_host is set and non-empty), in which case shares should be
/// pushed via HTTP rather than served from the local embedded server.
pub fn is_cloud_mode(cfg: &RemoteConfig) -> bool {
    remote_base_url(cfg).is_some()
}

/// Build the public share URL (displayed to the user) for a given share id.
pub fn share_url(cfg: &RemoteConfig, id: &str) -> Option<String> {
    remote_base_url(cfg).map(|base| format!("{}/s/{}", base, id))
}

/// Build the mock server base URL for the given configuration:
/// - Remote mode (public_host set): returns the configured remote URL
/// - Local/LAN mode: returns http://<host>:<port>, normalizing 0.0.0.0 to 127.0.0.1
pub fn mock_server_url(cfg: &RemoteConfig) -> String {
    if let Some(base) = remote_base_url(cfg) {
        return base;
    }
    let host = if cfg.host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        cfg.host.as_str()
    };
    format!("http://{}:{}", host, cfg.port)
}

/// Upload a project to the remote server, then create a share there.
///
/// Returns the id of the created share (the server may return a different id
/// but typically mirrors the client-generated one).
pub async fn push_share(
    client: Arc<dyn HttpClient>,
    cfg: &RemoteConfig,
    project: &Project,
    share: &ShareConfig,
) -> Result<String> {
    let base = remote_base_url(cfg).context("remote not configured")?;
    let token = cfg.auth_token.as_deref().unwrap_or("");

    // 1. Upload the project JSON.
    let project_json = serde_json::to_string(project)?;
    let url = format!("{}/admin/api/projects", base);
    let mut builder = Builder::new()
        .uri(&url)
        .method(Method::POST)
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            format!("verve/{}", crate::updater::CURRENT_VERSION),
        )
        .follow_redirects(RedirectPolicy::FollowAll);
    if !token.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }
    let req = builder.body(AsyncBody::from(project_json.into_bytes()))?;
    let resp = client.send(req).await.context("upload project")?;
    let status = resp.status();
    let mut body = Vec::new();
    resp.into_body().read_to_end(&mut body).await.ok();
    if !status.is_success() {
        anyhow::bail!(
            "upload project failed ({}): {}",
            status,
            String::from_utf8_lossy(&body)
        );
    }
    let _upload_resp: ProjectUploadResponse =
        serde_json::from_slice(&body).context("parse upload response")?;

    // 2. Create the share on the remote server.
    let public = share.access.public;
    let password = share.access.password.clone();
    let mut form_parts = vec![
        ("project_id".to_string(), share.project_id.clone()),
        ("title".to_string(), share.title.clone()),
        (
            "public".to_string(),
            if public {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
    ];
    if let Some(pw) = password {
        form_parts.push(("password".to_string(), pw));
    }
    let form_body = url_encode_form(&form_parts);

    let url = format!("{}/admin/api/shares", base);
    let mut builder = Builder::new()
        .uri(&url)
        .method(Method::POST)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header(
            "User-Agent",
            format!("verve/{}", crate::updater::CURRENT_VERSION),
        )
        .follow_redirects(RedirectPolicy::FollowAll);
    if !token.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }
    let req = builder.body(AsyncBody::from(form_body.into_bytes()))?;
    let resp = client.send(req).await.context("create share")?;
    let status = resp.status();
    let mut body = Vec::new();
    resp.into_body().read_to_end(&mut body).await?;
    if !status.is_success() {
        anyhow::bail!(
            "create share failed ({}): {}",
            status,
            String::from_utf8_lossy(&body)
        );
    }
    let resp: ShareCreateResponse =
        serde_json::from_slice(&body).context("parse share response")?;
    Ok(resp.id)
}

/// Minimal `application/x-www-form-urlencoded` encoder.
fn url_encode_form(parts: &[(String, String)]) -> String {
    fn pct_encode(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{:02X}", b));
            }
        }
        out
    }
    parts
        .iter()
        .map(|(k, v)| format!("{}={}", pct_encode(k), pct_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}
