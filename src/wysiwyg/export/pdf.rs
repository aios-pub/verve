//! PDF export.
//!
//! velotype renders PDFs by driving a headless Chromium via `chromiumoxide`.
//! verve ships its own PDF pipeline (`crate::notes_pdf`, backed by Pdfium), so
//! the embedded WYSIWYG editor does not pull in Chromium here. The render
//! entry points are kept as signature-compatible stubs that return an error;
//! stage-4 integration routes PDF export through verve's native pipeline.

use std::path::Path;

use crate::wysiwyg::theme::Theme;

/// Returns an error in the embedded build.
///
/// Wire PDF export to verve's `crate::notes_pdf` from the notes panel instead.
pub(crate) fn render_pdf(
    _markdown: &str,
    _theme: &Theme,
    _title: &str,
    _base_path: Option<&Path>,
) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!(
        "WYSIWYG PDF export is handled by verve's notes_pdf module; \
         the velotype chromiumoxide pipeline is not available in the embedded build"
    )
}

/// Returns an error in the embedded build.
pub(crate) async fn render_pdf_async(_html_path: std::path::PathBuf) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!(
        "WYSIWYG PDF export is handled by verve's notes_pdf module; \
         the velotype chromiumoxide pipeline is not available in the embedded build"
    )
}
