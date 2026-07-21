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

/// Prefix under which custom Verve icons are exposed to the UI layer. A
/// button builds an icon with e.g. `Icon::new(IconName::Redo).path(VERVE_ICON_SHARE)`
/// and this source resolves it to the embedded SVG.
pub const PREFIX: &str = "icons/verve/";

pub const SHARE: &str = "icons/verve/share-2.svg";
pub const DOCS: &str = "icons/verve/file-text.svg";
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
            // through to our own bundle for verve-specific paths.
            _ => {
                if let Some(rest) = path.strip_prefix(PREFIX) {
                    if let Some(file) = VerveIcons::get(rest) {
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
        Ok(out)
    }
}
