//! PDF export.
//!
//! velotype originally rendered PDFs by driving a headless Chromium via
//! `chromiumoxide`. verve removed that dependency and instead ships a native
//! pure-Rust Markdown→PDF pipeline (`crate::notes_pdf`, backed by Typst via the
//! `mdxport` crate). The WYSIWYG export delegates to that pipeline so both the
//! Notes panel and the standalone Markdown panel produce identical output.
//!
//! `theme` / `base_path` are accepted to match the HTML export signature but
//! are not consumed by the Typst pipeline yet.

use std::path::Path;

use crate::notes_pdf::markdown_to_pdf_bytes;
use crate::wysiwyg::theme::Theme;

/// Renders themed PDF bytes for the current document Markdown via verve's
/// native `notes_pdf` pipeline.
///
/// `theme` and `base_path` are currently unused (the Typst pipeline draws
/// with the OS-installed system fonts and its own layout) and are kept for
/// signature compatibility with the HTML export path.
#[allow(unused_variables)]
pub(crate) fn render_pdf(
    markdown: &str,
    theme: &Theme,
    title: &str,
    base_path: Option<&Path>,
) -> anyhow::Result<Vec<u8>> {
    markdown_to_pdf_bytes(markdown, title)
}

/// Unused in the embedded build.
///
/// Synchronous PDF export goes through [`render_pdf`], which delegates to
/// verve's native `notes_pdf` pipeline. This async variant is retained for
/// signature compatibility with velotype but is not wired to any export path.
pub(crate) async fn render_pdf_async(_html_path: std::path::PathBuf) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!(
        "async WYSIWYG PDF export is not wired up; use the synchronous \
         render_pdf / notes_pdf pipeline instead"
    )
}
