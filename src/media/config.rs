//! Layered media-storage configuration persisted at `<data_dir>/media_storage.toml`.
//!
//! All fields are `#[serde(default)]` so a config file from an older version
//! that is missing a whole section (e.g. `[image]`) deserializes cleanly with
//! defaults filled in — no manual migration step required.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How a given media kind (image or video) is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageMethod {
    /// Store on the local filesystem only.
    Local,
    /// Upload to S3. On upload failure the media falls back to local storage so
    /// no file is lost (see `upload` module).
    S3,
}

impl Default for StorageMethod {
    fn default() -> Self {
        StorageMethod::Local
    }
}

impl StorageMethod {
    /// Lowercase wire label used both for serde and the settings dropdown.
    pub fn label(self) -> &'static str {
        match self {
            StorageMethod::Local => "local",
            StorageMethod::S3 => "s3",
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label.trim().to_ascii_lowercase().as_str() {
            "s3" | "cloud" => StorageMethod::S3,
            _ => StorageMethod::Local,
        }
    }
}

// ---------------------------------------------------------------------------
// [global]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Media root directory. May be absolute, or relative to the data dir
    /// (default `media` → `<data_dir>/media`).
    #[serde(default = "default_media_root")]
    pub media_root: String,
    /// When true, `media_storage.toml` is tracked by git; when false it is
    /// written to `.gitignore` (machine-local only). Media files themselves are
    /// always gitignored. Defaults to **false** so S3 credentials are not pushed
    /// to a shared remote — enable explicitly to sync config across machines.
    #[serde(default = "default_git_sync")]
    pub git_sync: bool,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            media_root: default_media_root(),
            git_sync: default_git_sync(),
        }
    }
}

fn default_media_root() -> String {
    "media".to_string()
}

fn default_git_sync() -> bool {
    // Off by default: keep S3 credentials machine-local until the user opts in.
    false
}

// ---------------------------------------------------------------------------
// [image]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageConfig {
    /// S3 key prefix for images (e.g. `images`).
    #[serde(default = "default_image_prefix")]
    pub s3_prefix: String,
    /// Local vs S3 storage selection for images.
    #[serde(default)]
    pub storage: StorageMethod,
    /// Soft per-file size cap in MiB; exceeding it shows a pre-import warning
    /// (no compression — the user is asked to handle it).
    #[serde(default = "default_image_max_mb")]
    pub max_size_mb: u64,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            s3_prefix: default_image_prefix(),
            storage: StorageMethod::default(),
            max_size_mb: default_image_max_mb(),
        }
    }
}

fn default_image_prefix() -> String {
    "images".to_string()
}

fn default_image_max_mb() -> u64 {
    25
}

// ---------------------------------------------------------------------------
// [video]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoConfig {
    /// S3 key prefix for videos (e.g. `videos`).
    #[serde(default = "default_video_prefix")]
    pub s3_prefix: String,
    /// Local vs S3 storage selection for videos.
    #[serde(default)]
    pub storage: StorageMethod,
    /// Files at or above this size (MiB) use multipart upload.
    #[serde(default = "default_multipart_threshold_mb")]
    pub multipart_threshold_mb: u64,
    /// Part size (MiB) for multipart uploads.
    #[serde(default = "default_multipart_part_mb")]
    pub multipart_part_mb: u64,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            s3_prefix: default_video_prefix(),
            storage: StorageMethod::default(),
            multipart_threshold_mb: default_multipart_threshold_mb(),
            multipart_part_mb: default_multipart_part_mb(),
        }
    }
}

fn default_video_prefix() -> String {
    "videos".to_string()
}

fn default_multipart_threshold_mb() -> u64 {
    100
}

fn default_multipart_part_mb() -> u64 {
    8
}

// ---------------------------------------------------------------------------
// [s3] — shared auth for both image and video uploads.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Config {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub bucket: String,
    /// Optional region; empty string means "none" (common for MinIO).
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default = "default_use_ssl")]
    pub use_ssl: bool,
    /// Path-style addressing (`https://endpoint/bucket/...`). Default true for
    /// MinIO / self-hosted S3-compatible stores.
    #[serde(default = "default_path_style")]
    pub path_style: bool,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            bucket: String::new(),
            region: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            use_ssl: default_use_ssl(),
            path_style: default_path_style(),
        }
    }
}

fn default_use_ssl() -> bool {
    true
}

fn default_path_style() -> bool {
    true
}

impl S3Config {
    /// True when enough fields are present to attempt an S3 operation.
    pub fn is_configured(&self) -> bool {
        !self.endpoint.trim().is_empty()
            && !self.bucket.trim().is_empty()
            && !self.access_key.is_empty()
            && !self.secret_key.is_empty()
    }

    /// Scheme based on the SSL toggle.
    pub fn scheme(&self) -> &'static str {
        if self.use_ssl {
            "https"
        } else {
            "http"
        }
    }
}

// Credentials must never leak into logs, prompts, or `Debug` output.
impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("use_ssl", &self.use_ssl)
            .field("path_style", &self.path_style)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Top-level config + persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaStorageConfig {
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub image: ImageConfig,
    #[serde(default)]
    pub video: VideoConfig,
    #[serde(default)]
    pub s3: S3Config,
}

/// Path to the media config file: `<data_dir>/media_storage.toml`.
pub fn media_storage_path() -> Result<PathBuf> {
    Ok(crate::state::persistence::data_dir()?.join("media_storage.toml"))
}

/// Load the media config, falling back to defaults when the file is missing or
/// unparseable (which also covers the "old version missing a section" case,
/// since every field defaults).
pub fn load_media_config() -> MediaStorageConfig {
    let path = match media_storage_path() {
        Ok(p) => p,
        Err(_) => return MediaStorageConfig::default(),
    };
    match fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents).unwrap_or_default(),
        Err(_) => MediaStorageConfig::default(),
    }
}

/// Atomically write the media config (temp file + rename).
pub fn save_media_config(cfg: &MediaStorageConfig) -> Result<()> {
    let path = media_storage_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create media config dir {}", parent.display()))?;
    }
    let serialized = toml::to_string_pretty(cfg).context("serialize media config")?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, &serialized).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

/// Resolve the configured media root to an absolute path. Absolute values are
/// used as-is; relative values are joined to the data dir.
pub fn resolved_media_root(cfg: &MediaStorageConfig) -> Result<PathBuf> {
    let raw = &cfg.global.media_root;
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Ok(candidate.to_path_buf());
    }
    Ok(crate::state::persistence::data_dir()?.join(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::persistence;

    fn isolated_data_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "verve-media-gitignore-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn git_sync_off_ignores_config_file() {
        let dir = isolated_data_dir();
        let _guard = persistence::set_thread_data_dir(dir.clone());

        let mut cfg = MediaStorageConfig::default();
        cfg.global.git_sync = false;
        save_media_config(&cfg).unwrap();
        persistence::ensure_gitignore();

        let gi = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(
            gi.contains("media_storage.toml"),
            "git_sync off should add media_storage.toml to .gitignore"
        );
    }

    #[test]
    fn git_sync_on_tracks_config_file() {
        let dir = isolated_data_dir();
        let _guard = persistence::set_thread_data_dir(dir.clone());

        // First ignore it, then switch back to tracked.
        let mut cfg = MediaStorageConfig::default();
        cfg.global.git_sync = false;
        save_media_config(&cfg).unwrap();
        persistence::ensure_gitignore();

        cfg.global.git_sync = true;
        save_media_config(&cfg).unwrap();
        persistence::ensure_gitignore();

        let gi = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(
            !gi.contains("media_storage.toml"),
            "git_sync on should remove media_storage.toml from .gitignore"
        );
    }

    #[test]
    fn default_storage_method_is_local() {
        assert_eq!(StorageMethod::default(), StorageMethod::Local);
        assert_eq!(
            MediaStorageConfig::default().image.storage,
            StorageMethod::Local
        );
        assert_eq!(
            MediaStorageConfig::default().video.storage,
            StorageMethod::Local
        );
    }

    #[test]
    fn default_git_sync_is_off() {
        // Credentials must stay machine-local until the user explicitly opts in.
        assert!(!MediaStorageConfig::default().global.git_sync);
    }
}
