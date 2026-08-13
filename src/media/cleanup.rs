//! Note-deletion media cleanup: scan note content for media references and
//! collect the local files / S3 objects that belong to this app's media store
//! so they can be offered for deletion when a note is removed.

use std::collections::HashSet;
use std::path::PathBuf;

use regex::Regex;

use crate::media::config::{MediaStorageConfig, resolved_media_root};
use crate::media::path::{is_local_media_ref, resolve_local_media_ref};

/// Media belonging to this app that a note references and that can be deleted
/// alongside the note.
pub struct DeletableMedia {
    /// Resolved absolute paths under the media root (`./media/...`).
    pub local_paths: Vec<PathBuf>,
    /// S3 object keys for media uploaded to our configured bucket.
    pub s3_keys: Vec<String>,
}

impl DeletableMedia {
    pub fn total(&self) -> usize {
        self.local_paths.len() + self.s3_keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.local_paths.is_empty() && self.s3_keys.is_empty()
    }
}

/// Scan `content` for media references and classify them:
/// - `./media/...` → resolved local path
/// - `{scheme}://{endpoint}/{bucket}/...` → S3 object key (our bucket only)
/// - anything else (external URLs, data: URLs) → ignored
pub fn collect_deletable_media(content: &str, cfg: &MediaStorageConfig) -> DeletableMedia {
    let srcs = extract_media_srcs(content);
    let root = resolved_media_root(cfg).ok();
    let s3_prefix = s3_object_prefix(cfg);

    let mut local_paths = Vec::new();
    let mut s3_keys = Vec::new();
    for src in &srcs {
        let s = src.trim();
        if is_local_media_ref(s) {
            if let Some(root) = &root {
                if let Some(abs) = resolve_local_media_ref(s, root) {
                    local_paths.push(abs);
                }
            }
        } else if !s3_prefix.is_empty() {
            if let Some(key) = s.strip_prefix(&s3_prefix) {
                let key = key.trim();
                if !key.is_empty() {
                    s3_keys.push(key.to_string());
                }
            }
        }
    }
    DeletableMedia { local_paths, s3_keys }
}

/// URL prefix identifying our own S3 bucket: `{scheme}://{endpoint}/{bucket}/`.
/// Empty when S3 is not configured.
fn s3_object_prefix(cfg: &MediaStorageConfig) -> String {
    let endpoint = cfg.s3.endpoint.trim();
    let bucket = cfg.s3.bucket.trim();
    if endpoint.is_empty() || bucket.is_empty() {
        return String::new();
    }
    format!("{}://{}/{}/", cfg.s3.scheme(), endpoint, bucket)
}

/// Extract all media `src` values from markdown image syntax and HTML
/// `src="..."` / `src='...'` attributes (covers `<img>`, `<video>`, `<source>`),
/// de-duplicated, preserving first-seen order.
pub fn extract_media_srcs(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let md_re = match Regex::new(r"!\[[^\]]*\]\(([^)\s]+)") {
        Ok(r) => r,
        Err(_) => return out,
    };
    let src_re = match Regex::new(r#"(?i)src\s*=\s*(?:"([^"]+)"|'([^']+)')"#) {
        Ok(r) => r,
        Err(_) => return out,
    };

    for caps in md_re.captures_iter(content) {
        if let Some(m) = caps.get(1) {
            push_dedup(&mut out, &mut seen, m.as_str());
        }
    }
    for caps in src_re.captures_iter(content) {
        let m = caps.get(1).or_else(|| caps.get(2));
        if let Some(m) = m {
            push_dedup(&mut out, &mut seen, m.as_str());
        }
    }
    out
}

fn push_dedup(out: &mut Vec<String>, seen: &mut HashSet<String>, value: &str) {
    let t = value.trim().to_string();
    if !t.is_empty() && seen.insert(t.clone()) {
        out.push(t);
    }
}

/// Best-effort synchronous deletion of local media files. Returns
/// `(deleted, failed)`; files already absent are neither. S3 deletion is async
/// and handled separately by the caller.
pub fn delete_local_files(paths: &[PathBuf]) -> (usize, usize) {
    let mut deleted = 0;
    let mut failed = 0;
    for p in paths {
        match std::fs::remove_file(p) {
            Ok(()) => deleted += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => failed += 1,
        }
    }
    (deleted, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_markdown_and_html_srcs_deduped() {
        let content = r#"
text ![cover](./media/images/abc.png) and ![two](https://x.com/y.jpg)
<img src="./media/images/def.png" />
<video src='./media/videos/g.mp4' controls></video>
<img src="./media/images/abc.png" />
"#;
        let srcs = extract_media_srcs(content);
        assert_eq!(srcs.len(), 4);
        assert!(srcs.iter().any(|s| s == "./media/images/abc.png"));
        assert!(srcs.iter().any(|s| s == "./media/videos/g.mp4"));
        assert!(srcs.iter().any(|s| s == "https://x.com/y.jpg"));
        // duplicate abc.png appears only once
        assert_eq!(srcs.iter().filter(|s| s == &"./media/images/abc.png").count(), 1);
    }

    #[test]
    fn empty_content_yields_nothing() {
        assert!(extract_media_srcs("no media here").is_empty());
    }
}
