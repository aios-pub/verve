//! Composite asset source: bundles gpui-component's stock icon set together
//! with Verve-specific Lucide SVG icons stored under `assets/verve_icons/`.
//!
//! The built-in [`gpui_component_assets::Assets`] is queried first (it ships
//! the icon set that backs [`gpui_component::IconName`]). When a requested
//! path lives under `icons/verve/`, we fall back to the icons embedded by
//! [`VerveIcons`] — this lets the UI render custom icons without touching the
//! upstream fork.

use gpui::{AssetSource, Result, SharedString};
use gpui_component_assets::Assets as StockAssets;
use rust_embed::RustEmbed;
use std::borrow::Cow;

/// Verve-specific Lucide SVG icons (24x24, stroke-based, currentColor).
#[derive(RustEmbed)]
#[folder = "assets/verve_icons"]
#[include = "*.svg"]
struct VerveIcons;

/// Bundled fonts required by gpui's SVG renderer (IBM Plex Sans for sans-serif,
/// Lilex for monospace). Without these, every SVG text render emits a
/// "Bundled font not found" warning.
#[derive(RustEmbed)]
#[folder = "assets/fonts"]
#[include = "*.ttf"]
struct BundledFonts;

/// Prefix under which custom Verve icons are exposed to the UI layer. A
/// button builds an icon with e.g. `Icon::new(IconName::Redo).path(VERVE_ICON_SHARE)`
/// and this source resolves it to the embedded SVG.
pub const PREFIX: &str = "icons/verve/";

pub const SHARE: &str = "icons/verve/share-2.svg";
pub const DOCS: &str = "icons/verve/file-text.svg";
pub const LOCATE: &str = "icons/verve/locate-fixed.svg";
pub const HISTORY: &str = "icons/verve/history.svg";
pub const BRACES: &str = "icons/verve/braces.svg";
pub const KANBAN: &str = "icons/verve/kanban.svg";
pub const REFRESH_CW: &str = "icons/verve/refresh-cw.svg";
pub const DOCKER: &str = "icons/verve/docker.svg";
pub const K8S: &str = "icons/verve/k8s.svg";
pub const IMPORT: &str = "icons/verve/import.svg";
pub const EXPORT: &str = "icons/verve/export.svg";
pub const SERVER: &str = "icons/verve/server.svg";
pub const NOTEBOOK: &str = "icons/verve/notebook-pen.svg";
pub const BRACES_JSON: &str = "icons/verve/braces-json.svg";
pub const FILE_PDF: &str = "icons/verve/file-pdf.svg";
pub const MARKDOWN: &str = "icons/verve/markdown.svg";
pub const SAVE: &str = "icons/verve/save.svg";
pub const SAVE_AS: &str = "icons/verve/save-as.svg";
// WYSIWYG toolbar icons.
pub const TB_TABLE: &str = "icons/verve/table.svg";
pub const TB_IMAGE: &str = "icons/verve/image.svg";
pub const TB_ERASER: &str = "icons/verve/eraser.svg";
pub const TB_LIST_ORDERED: &str = "icons/verve/list-ordered.svg";
pub const TB_LIST: &str = "icons/verve/list.svg";
pub const TB_QUOTE: &str = "icons/verve/quote.svg";
pub const TB_CODE: &str = "icons/verve/code.svg";

/// Prefix under which bundled fonts are exposed to gpui's SVG renderer.
const FONTS_PREFIX: &str = "fonts/";

/// Composite asset source passed to `with_assets(...)`.
pub struct VerveAssets(pub StockAssets);

impl VerveAssets {
    pub fn new() -> Self {
        Self(StockAssets)
    }
}

impl Default for VerveAssets {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetSource for VerveAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // Try the upstream stock icons first.
        match self.0.load(path) {
            Ok(Some(data)) => Ok(Some(data)),
            // Either the upstream returned None (not found) or errored — fall
            // through to our own bundles.
            _ => {
                // Verve-specific icons.
                if let Some(rest) = path.strip_prefix(PREFIX) {
                    if let Some(file) = VerveIcons::get(rest) {
                        return Ok(Some(file.data));
                    }
                }
                // Bundled fonts for gpui's SVG renderer.
                if let Some(rest) = path.strip_prefix(FONTS_PREFIX) {
                    if let Some(file) = BundledFonts::get(rest) {
                        return Ok(Some(file.data));
                    }
                }
                Ok(None)
            }
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out = self.0.list(path).unwrap_or_default();
        if path.is_empty() || PREFIX.starts_with(path) || path.starts_with(PREFIX) {
            let suffix = path.strip_prefix(PREFIX).unwrap_or("");
            for name in VerveIcons::iter() {
                if name.starts_with(suffix) {
                    out.push(format!("{PREFIX}{name}").into());
                }
            }
        }
        // List bundled fonts so the SVG renderer can discover them.
        if path.is_empty() || FONTS_PREFIX.starts_with(path) || path.starts_with(FONTS_PREFIX) {
            let suffix = path.strip_prefix(FONTS_PREFIX).unwrap_or("");
            for name in BundledFonts::iter() {
                if name.starts_with(suffix) {
                    out.push(format!("{FONTS_PREFIX}{name}").into());
                }
            }
        }
        Ok(out)
    }
}
