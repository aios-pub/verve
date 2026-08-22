//! Path helpers for media: portable `./media/` references, root mapping, and
//! image format detection (extension-less / pasted bytes).

use std::path::{Path, PathBuf};

/// Which kind of media — determines the on-disk subdirectory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    /// Subdirectory name under the media root.
    pub fn subdir(self) -> &'static str {
        match self {
            MediaKind::Image => "images",
            MediaKind::Video => "videos",
        }
    }
}

/// Portable markdown reference for a local media file: `./media/<subdir>/<file>`.
///
/// This form is stable across machines (git-syncable note text); the renderer
/// maps the `./media/` prefix back to the configured media root at display time.
pub fn local_markdown_ref(kind: MediaKind, filename: &str) -> String {
    format!("./media/{}/{}", kind.subdir(), filename)
}

/// True if `src` is one of our portable local-media references.
pub fn is_local_media_ref(src: &str) -> bool {
    let s = src.trim();
    s.starts_with("./media/") || s.starts_with("media/")
}

/// Resolve a `./media/...` (or `media/...`) reference against the media root.
///
/// Returns `None` if `src` is not a local-media ref or if it attempts a path
/// escape (`..`, absolute, backslashes). The remaining suffix is joined to the
/// root as-is, so `./media/images/abc.png` → `<root>/images/abc.png`.
pub fn resolve_local_media_ref(src: &str, media_root: &Path) -> Option<PathBuf> {
    let s = src.trim();
    let rel = s
        .strip_prefix("./media/")
        .or_else(|| s.strip_prefix("media/"))?;
    let cleaned = rel.trim_start_matches('/');
    // Reject anything that could escape the media root.
    if cleaned.starts_with("..")
        || cleaned.contains("/../")
        || cleaned.contains('\\')
        || cleaned.contains('\0')
    {
        return None;
    }
    Some(media_root.join(cleaned))
}

/// Detect an image format from raw bytes and return its standard lowercase
/// extension (e.g. `"png"`). Returns `None` for unknown / non-image bytes.
pub fn detect_image_extension(bytes: &[u8]) -> Option<&'static str> {
    let format = image::guess_format(bytes).ok()?;
    Some(match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::WebP => "webp",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::Bmp => "bmp",
        image::ImageFormat::Tiff => "tiff",
        _ => return None,
    })
}

/// Detect a video format from raw bytes and return its standard lowercase
/// extension (e.g. `"mp4"`). Returns `None` for unknown / non-video bytes.
pub fn detect_video_extension(bytes: &[u8]) -> Option<&'static str> {
    [
        "mp4", "mov", "webm", "mkv", "avi", "m4v", "wmv", "flv",
    ]
    .into_iter()
    .find(|&ext| infer::is(bytes, ext))
}

/// Whether a filename extension (without leading dot) is a supported image kind.
pub fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext.trim_start_matches('.').to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff"
    )
}

/// Whether a filename extension (without leading dot) is a supported video kind.
pub fn is_video_extension(ext: &str) -> bool {
    matches!(
        ext.trim_start_matches('.').to_ascii_lowercase().as_str(),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v"
    )
}

/// Build a `<video>` markdown block for the given src (local `./media/...` or a
/// full S3 URL).
pub fn video_markdown(src: &str) -> String {
    format!("<video src=\"{src}\" controls width=\"100%\"></video>")
}

/// Build a `file://` URL from an absolute path (spaces percent-encoded) for use
/// with `cx.open_url`. Unix-style absolute paths produce `file:///...`.
fn file_url(path: &Path) -> String {
    let s = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(' ', "%20");
    format!("file://{s}")
}

/// Resolve a media `src` (local `./media/...`, absolute path, or `http(s)://`)
/// to a value `cx.open_url` can open: remote URLs pass through unchanged;
/// local refs/paths become a `file://` URL. Returns `None` if it cannot be
/// resolved.
pub fn resolve_media_src_to_openable(src: &str) -> Option<String> {
    let s = src.trim();
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Some(s.to_string());
    }
    if is_local_media_ref(s) {
        let root = crate::media::config::cached_media_root()?;
        let abs = resolve_local_media_ref(s, &root)?;
        return Some(file_url(&abs));
    }
    let p = Path::new(s);
    if p.is_absolute() {
        return Some(file_url(p));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ref_format() {
        assert_eq!(
            local_markdown_ref(MediaKind::Image, "abc.png"),
            "./media/images/abc.png"
        );
        assert_eq!(
            local_markdown_ref(MediaKind::Video, "abc.mp4"),
            "./media/videos/abc.mp4"
        );
    }

    #[test]
    fn resolve_plain() {
        let root = Path::new("/tmp/media");
        assert_eq!(
            resolve_local_media_ref("./media/images/abc.png", root),
            Some(Path::new("/tmp/media/images/abc.png").to_path_buf())
        );
        assert_eq!(
            resolve_local_media_ref("media/videos/x.mp4", root),
            Some(Path::new("/tmp/media/videos/x.mp4").to_path_buf())
        );
    }

    #[test]
    fn resolve_rejects_escape() {
        let root = Path::new("/tmp/media");
        assert_eq!(resolve_local_media_ref("./media/../etc/passwd", root), None);
        assert_eq!(resolve_local_media_ref("./media/images/../../x", root), None);
        assert_eq!(resolve_local_media_ref("https://x/y", root), None);
    }

    #[test]
    fn detect_png_signature() {
        let png = b"\x89PNG\r\n\x1a\n";
        assert_eq!(detect_image_extension(png), Some("png"));
    }
}
