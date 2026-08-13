//! Async upload bridge: runs S3 work on the shared media Tokio runtime and
//! reports the result back to the GPUI thread via a oneshot channel.
//!
//! On success the receiver gets the public S3 URL (and the local copy is
//! removed). On failure it gets a desensitized error and the local file is left
//! intact, so no media is lost.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::sync::oneshot;

use crate::media::config::S3Config;
use crate::media::runtime::media_runtime;
use crate::media::s3;
/// A single image upload task.
pub struct ImageUploadRequest {
    /// Local file written by the paste/import path; removed on success.
    pub local_path: PathBuf,
    /// Object key (`<prefix>/<filename>`).
    pub key: String,
    /// S3 credentials + endpoint config.
    pub s3: S3Config,
}

/// Enqueue an image upload on the media runtime.
///
/// Returns a receiver that yields `Ok(url)` on success (the local copy is then
/// deleted) or `Err(..)` on failure (local copy preserved).
pub fn enqueue_image_upload(req: ImageUploadRequest) -> oneshot::Receiver<Result<String>> {
    let (tx, rx) = oneshot::channel();
    media_runtime().spawn(async move {
        let res = run_image_upload(req).await;
        let _ = tx.send(res);
    });
    rx
}

async fn run_image_upload(req: ImageUploadRequest) -> Result<String> {
    let bytes = tokio::fs::read(&req.local_path)
        .await
        .with_context(|| format!("read local media {}", req.local_path.display()))?;
    let content_type = guess_image_content_type(&req.key);
    let url = s3::put_object(&req.s3, &req.key, bytes, content_type).await?;
    // Success: only the cloud copy remains.
    let _ = tokio::fs::remove_file(&req.local_path).await;
    Ok(url)
}

// ---------------------------------------------------------------------------
// Video (multipart) uploads
// ---------------------------------------------------------------------------

/// Per-update event emitted while a video upload runs. Polled by the GPUI
/// thread via a `std::sync::mpsc` channel (timer-based), since tokio mpsc
/// cannot be awaited outside a tokio runtime.
pub enum VideoUploadEvent {
    /// `done` parts uploaded out of `total`.
    Progress { done: u32, total: u32 },
    /// Final outcome. `Ok(url)` on success (local copy then removed),
    /// `Err(..)` on failure (local copy preserved).
    Done(Result<String>),
}

/// A single video multipart-upload task.
pub struct VideoUploadRequest {
    pub local_path: PathBuf,
    pub key: String,
    pub s3: S3Config,
    pub part_size_mb: u64,
}

/// Enqueue a video multipart upload on the media runtime. The returned receiver
/// yields `Progress` events during the upload and a final `Done`.
pub fn enqueue_video_upload(
    req: VideoUploadRequest,
) -> std::sync::mpsc::Receiver<VideoUploadEvent> {
    let (tx, rx) = std::sync::mpsc::channel::<VideoUploadEvent>();
    media_runtime().spawn(async move {
        let res = run_video_upload(req, tx.clone()).await;
        let _ = tx.send(VideoUploadEvent::Done(res));
    });
    rx
}

async fn run_video_upload(
    req: VideoUploadRequest,
    tx: std::sync::mpsc::Sender<VideoUploadEvent>,
) -> Result<String> {
    let bytes = tokio::fs::read(&req.local_path)
        .await
        .with_context(|| format!("read local media {}", req.local_path.display()))?;
    let content_type = guess_video_content_type(&req.key);
    let part_size = (req.part_size_mb as usize)
        .saturating_mul(1024 * 1024)
        .max(5 * 1024 * 1024); // S3 minimum part size (except the last part).
    let url = s3::put_object_multipart(&req.s3, &req.key, bytes, part_size, content_type, |done, total| {
        let _ = tx.send(VideoUploadEvent::Progress { done, total });
    })
    .await?;
    let _ = tokio::fs::remove_file(&req.local_path).await;
    Ok(url)
}

/// Map a video object key's extension to a MIME type (best-effort).
fn guess_video_content_type(key: &str) -> &'static str {
    let ext = key.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

/// Map an object key's extension to a MIME type (best-effort).
fn guess_image_content_type(key: &str) -> &'static str {
    let ext = key.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "tiff" | "tif" => "image/tiff",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Best-effort S3 object deletion (note-deletion media cleanup)
// ---------------------------------------------------------------------------

/// Enqueue deletion of multiple S3 objects on the media runtime. The receiver
/// yields `Ok((deleted, failed))` once all keys have been attempted; drop the
/// receiver to fire-and-forget.
pub fn enqueue_s3_object_deletes(
    s3: S3Config,
    keys: Vec<String>,
) -> oneshot::Receiver<Result<(usize, usize)>> {
    let (tx, rx) = oneshot::channel();
    media_runtime().spawn(async move {
        let mut deleted = 0usize;
        let mut failed = 0usize;
        for key in &keys {
            match s3::delete_object(&s3, key).await {
                Ok(()) => deleted += 1,
                Err(_) => failed += 1,
            }
        }
        let _ = tx.send(Ok((deleted, failed)));
    });
    rx
}
