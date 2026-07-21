//! GitHub-based auto-update mechanism.
//!
//! On startup (and via a manual "check for updates" button), the app queries
//! the GitHub Releases API for the latest release of `aios-rs/verve`. If the
//! latest version is newer than `CARGO_PKG_VERSION`, an [`UpdateInfo`] is
//! surfaced to the UI so the user can open the release page and download the
//! platform-appropriate installer.
//!
//! The check is purely informational — it does **not** replace the running
//! binary in place. Desktop installers (.dmg / .deb / .msi) handle the actual
//! upgrade once the user downloads and runs them.
//!
//! # Update manifest
//!
//! The CI pipeline attaches a `latest.json` asset to each release. When
//! present, it is preferred over parsing the release's asset list because it
//! carries a curated `version` + per-platform URLs:
//!
//! ```json
//! {
//!   "version": "0.2.0",
//!   "url": "https://github.com/lijingrs/verve/releases/tag/v0.2.0",
//!   "notes": "...",
//!   "platforms": {
//!     "macos":   { "url": "...Verve-0.2.0.dmg" },
//!     "linux":   { "url": "...verve-0.2.0-amd64.deb" },
//!     "windows": { "url": "...Verve-0.2.0.msi" }
//!   }
//! }
//! ```

use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, Builder, HttpClient, HttpRequestExt as _, Method, RedirectPolicy};
use semver::Version;
use serde::Deserialize;

/// The GitHub repository slug used for update checks.
pub const REPO: &str = "aios-rs/verve";

/// The current application version, sourced from `Cargo.toml` at compile time.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A platform key used in the update manifest.
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    /// The current build's platform.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Platform::Macos
        }
        #[cfg(target_os = "linux")]
        {
            Platform::Linux
        }
        #[cfg(target_os = "windows")]
        {
            Platform::Windows
        }
    }

    /// The string key used in `latest.json`'s `platforms` map.
    pub fn key(&self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }
    }

    /// Substrings used to match a release asset filename to this platform when
    /// falling back to direct asset parsing (no `latest.json` present).
    /// The first asset whose name contains any of these wins.
    pub fn asset_hints(&self) -> &'static [&'static str] {
        match self {
            Platform::Macos => &[".dmg", ".app.tar.gz", "macos", "darwin"],
            Platform::Linux => &[".deb", ".AppImage", ".tar.gz", "linux"],
            Platform::Windows => &[".msi", ".zip", "windows", "win"],
        }
    }
}

/// Information about an available update, surfaced to the UI.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    /// The latest released version string (e.g. `"0.2.0"`).
    pub version: String,
    /// The GitHub release HTML page URL (opened in the browser).
    pub release_url: String,
    /// The direct download URL for the current platform's asset, if found.
    pub download_url: Option<String>,
    /// Human-readable release notes (may be markdown, truncated to ~2k chars).
    pub notes: String,
}

/// The outcome of a manual update check.
#[derive(Clone, Debug)]
pub enum UpdateCheckResult {
    /// The running version is the latest.
    UpToDate,
    /// An update is available.
    UpdateAvailable(UpdateInfo),
    /// The check failed (network error, parse error, etc.).
    Error(String),
}

// ─── JSON response shapes ───────────────────────────────────────────────────

/// A single asset in a GitHub release.
#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The GitHub Releases API response for `/releases/latest`.
#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GhAsset>,
}

/// The `latest.json` manifest attached to a release.
#[derive(Deserialize)]
#[allow(dead_code)]
struct UpdateManifest {
    version: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    platforms: std::collections::HashMap<String, ManifestPlatform>,
}

#[derive(Deserialize)]
struct ManifestPlatform {
    url: String,
}

/// Parse a version string, stripping a leading `v` if present.
fn parse_version(raw: &str) -> Result<Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    Version::parse(trimmed).context(format!("parse version {raw:?}"))
}

/// The current version as a parsed [`Version`].
pub fn current_version() -> Version {
    parse_version(CURRENT_VERSION).unwrap_or(Version::new(0, 0, 0))
}

/// Check GitHub for the latest release and compare against the running version.
///
/// Accepts an `HttpClient` so it can run on GPUI's background executor. The
/// result is returned synchronously to the caller (which typically forwards it
/// back to the UI entity via `cx.update`).
pub async fn check_for_updates(client: &dyn HttpClient) -> Result<UpdateCheckResult> {
    // Query the GitHub Releases API.
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let req = Builder::new()
        .uri(&url)
        .method(Method::GET)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", format!("verve/{CURRENT_VERSION}"))
        .follow_redirects(RedirectPolicy::FollowAll)
        .body(AsyncBody::empty())?;

    let resp = client.send(req).await.context("github api request")?;
    if !resp.status().is_success() {
        return Ok(UpdateCheckResult::Error(format!(
            "GitHub API returned {}",
            resp.status()
        )));
    }

    let mut body = resp.into_body();
    let mut buf = Vec::new();
    body.read_to_end(&mut buf).await.context("read body")?;
    let text = String::from_utf8_lossy(&buf);

    let release: GhRelease = serde_json::from_str(&text).context("parse release json")?;
    let latest = parse_version(&release.tag_name)?;

    if latest <= current_version() {
        return Ok(UpdateCheckResult::UpToDate);
    }

    // Try to find the `latest.json` manifest asset first (preferred path).
    let manifest = if let Some(asset) = release.assets.iter().find(|a| a.name == "latest.json") {
        fetch_manifest(client, &asset.browser_download_url)
            .await
            .ok()
    } else {
        None
    };

    let (download_url, notes, release_url) = if let Some(m) = manifest {
        let dl = m
            .platforms
            .get(Platform::current().key())
            .map(|p| p.url.clone());
        (
            dl,
            m.notes
                .unwrap_or_else(|| release.body.clone().unwrap_or_default()),
            m.url.unwrap_or_else(|| release.html_url.clone()),
        )
    } else {
        // Fallback: match an asset by platform hints.
        let hints = Platform::current().asset_hints();
        let dl = release
            .assets
            .iter()
            .find(|a| hints.iter().any(|h| a.name.to_lowercase().contains(h)))
            .map(|a| a.browser_download_url.clone());
        (
            dl,
            release.body.clone().unwrap_or_default(),
            release.html_url.clone(),
        )
    };

    // Truncate release notes to keep the UI snappy.
    let notes = if notes.len() > 2000 {
        format!("{}…", &notes[..2000])
    } else {
        notes
    };

    Ok(UpdateCheckResult::UpdateAvailable(UpdateInfo {
        version: latest.to_string(),
        release_url,
        download_url,
        notes,
    }))
}

/// Fetch and parse a `latest.json` manifest asset.
async fn fetch_manifest(client: &dyn HttpClient, url: &str) -> Result<UpdateManifest> {
    let req = Builder::new()
        .uri(url)
        .method(Method::GET)
        .header("User-Agent", format!("verve/{CURRENT_VERSION}"))
        .follow_redirects(RedirectPolicy::FollowAll)
        .body(AsyncBody::empty())?;
    let resp = client.send(req).await?;
    if !resp.status().is_success() {
        anyhow::bail!("manifest status {}", resp.status());
    }
    let mut body = resp.into_body();
    let mut buf = Vec::new();
    body.read_to_end(&mut buf).await?;
    let m: UpdateManifest = serde_json::from_slice(&buf)?;
    Ok(m)
}

/// Run the update check on a background executor and invoke the callback with
/// the result. Designed to be called from `cx.spawn` inside GPUI.
pub async fn run_check(client: Arc<dyn HttpClient>) -> UpdateCheckResult {
    match check_for_updates(client.as_ref()).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("update check failed: {e:?}");
            UpdateCheckResult::Error(e.to_string())
        }
    }
}
