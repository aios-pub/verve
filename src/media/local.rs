//! Local media storage: write bytes into `<root>/<subdir>/<sparkid>.<ext>`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::media::id::sparkid_filename;
use crate::media::path::MediaKind;

/// Outcome of storing a media file locally.
pub struct StoredMedia {
    /// The on-disk absolute path that was written.
    pub abs_path: std::path::PathBuf,
    /// The bare filename (`<sparkid>.<ext>`), used to build a portable ref.
    pub filename: String,
}

/// Write `bytes` to `<media_root>/<kind.subdir()>/<sparkid>.<ext>`, creating the
/// subdirectory (and any missing parents) as needed.
///
/// Directories are created on demand; a write-permission failure is propagated
/// as an error so the caller can surface it (and, for S3, fall back).
pub fn store_local(
    media_root: &Path,
    kind: MediaKind,
    bytes: &[u8],
    ext: &str,
) -> Result<StoredMedia> {
    let dir = media_root.join(kind.subdir());
    fs::create_dir_all(&dir)
        .with_context(|| format!("create media directory {}", dir.display()))?;
    let filename = sparkid_filename(ext);
    let abs_path = dir.join(&filename);
    fs::write(&abs_path, bytes)
        .with_context(|| format!("write media file {}", abs_path.display()))?;
    Ok(StoredMedia { abs_path, filename })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_writes_file_under_subdir() {
        let tmp = tempfile_dir();
        let stored = store_local(&tmp, MediaKind::Image, b"\x89PNG\r\n\x1a\n", "png").unwrap();
        assert!(stored.abs_path.starts_with(&tmp));
        assert!(stored.abs_path.to_string_lossy().contains("images"));
        assert!(stored.filename.ends_with(".png"));
        assert!(stored.abs_path.is_file());
        let written = std::fs::read(&stored.abs_path).unwrap();
        assert_eq!(written, b"\x89PNG\r\n\x1a\n");
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "verve-media-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
