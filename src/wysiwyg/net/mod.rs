//! Remote-image URL detection.
//!
//! Ported from velotype (`src/net/mod.rs`). velotype also installed its own
//! reqwest-backed `HttpClient` global here; verve already installs one
//! (`reqwest_client::ReqwestClient`) in `main.rs`, and GPUI's image loading
//! reads from that single global — so only the pure `is_remote_image_source`
//! helper is needed here.

use std::str::FromStr;

use gpui::http_client;

/// Returns `true` for `http://` / `https://` URLs (remote image sources).
///
/// Local paths (`./img.png`, `assets/photo.jpg`) and `data:` URLs return
/// `false`.
pub(crate) fn is_remote_image_source(source: &str) -> bool {
    http_client::Uri::from_str(source)
        .ok()
        .and_then(|uri| uri.scheme_str().map(str::to_owned))
        .is_some_and(|scheme| scheme == "http" || scheme == "https")
}

#[cfg(test)]
mod tests {
    use super::is_remote_image_source;

    #[test]
    fn detects_remote_http_sources() {
        assert!(is_remote_image_source("https://example.com/image.png"));
        assert!(is_remote_image_source("http://example.com/image.gif"));
        assert!(!is_remote_image_source("./image.png"));
        assert!(!is_remote_image_source("images/photo.jpg"));
    }
}
