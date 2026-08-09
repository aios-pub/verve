//! Markdown → PDF export for the Notes panel and the standalone Markdown editor.
//!
//! Backed by [`mdxport`], which parses Markdown (comrak) and compiles it to PDF
//! in-process via Typst. Typst writes font descriptors correctly for both CFF
//! (OpenType) and TrueType outlines and subsets the used glyphs automatically,
//! so CJK content renders without the "font type mismatch" blank-page failure
//! the previous `printpdf` pipeline produced.
//!
//! ## Faithful ("原样") export
//!
//! We override mdxport's built-in `modern-tech` template (which adds heading
//! auto-numbering `0.1`, page numbers, a centered title block and US-letter
//! paper) with [`MINIMAL_TEMPLATE`]: the Markdown is rendered as-is — A4 paper,
//! no heading numbering, no page numbers, no injected title block — while still
//! keeping a CJK font fallback chain and sensible rendering for code blocks,
//! tables, quotes and links (these are Markdown semantics, not decoration).
//!
//! ## Font strategy
//!
//! `mdxport` loads fonts from the OS-installed system font directories (plus
//! the Latin-only fonts bundled with `typst-assets`). The template ships a CJK
//! fallback chain (`PingFang SC` → `Microsoft YaHei` → `Noto Sans CJK SC` → …),
//! so Chinese/Japanese/Korean render on macOS and Windows out of the box. On
//! Linux the user must have a CJK font installed (e.g. `fonts-noto-cjk`).
//!
//! Both public functions keep the original signatures so callers
//! (`ui::notes_panel` and `wysiwyg::export::pdf`) are unchanged.

use std::path::Path;

use anyhow::{Context as _, Result};

/// Minimal, decoration-free Typst template.
///
/// Replaces mdxport's styled templates to export the Markdown faithfully:
/// - A4 paper with a 20mm margin (no page numbers).
/// - No heading auto-numbering (headings render as written).
/// - No centered title block — the body is rendered verbatim. `title` is only
///   used as PDF metadata, not as visible content.
/// - CJK font fallback chain kept (required for Chinese to render).
/// - Sensible `show` rules for code blocks, tables, quotes and links, which
///   express Markdown semantics rather than cosmetic styling.
const MINIMAL_TEMPLATE: &str = r##"
#let body-fonts = ("PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", "Noto Sans SC", "Noto Sans CJK JP", "Yu Gothic", "Noto Sans CJK KR", "Malgun Gothic", "Apple SD Gothic Neo", "WenQuanYi Micro Hei", "IBM Plex Sans", "Inter", "Libertinus Serif")
#let code-fonts = ("JetBrains Mono", "DejaVu Sans Mono", "SFMono-Regular", "Consolas", "Menlo")

#let article(
  title: none,
  authors: (),
  lang: "en",
  toc: false,
  body,
) = {
  set text(font: body-fonts, lang: lang)
  set page(paper: "a4", margin: 20mm)

  // No heading numbering — headings render exactly as authored.
  set heading(numbering: none, supplement: none)
  show heading: set text(font: body-fonts, weight: "bold")

  // Markdown semantics (not cosmetic decoration): monospace code, link color,
  // quote blocks and plain table rules.
  show raw.where(block: true): set text(font: code-fonts, size: 9.5pt)
  show raw.where(block: false): set text(font: code-fonts)
  show link: set text(fill: rgb("#1E88E5"))
  show quote: block.with(
    fill: luma(245),
    stroke: (left: 2pt + rgb("#9E9E9E")),
    inset: (x: 1em, y: 0.4em),
  )
  show table: set table(
    stroke: (_, y) => if y > 0 { (top: 0.5pt + luma(220)) },
    inset: (x: 8pt, y: 8pt),
  )
  show table.header: strong

  body
}
"##;

/// Render `md` to in-memory PDF bytes.
///
/// `title` is used only as PDF document metadata (not rendered as visible
/// content) so the Markdown is exported faithfully — see [`MINIMAL_TEMPLATE`].
pub fn markdown_to_pdf_bytes(md: &str, title: &str) -> Result<Vec<u8>> {
    let trimmed = title.trim();
    let mut options = mdxport::Options {
        custom_template: Some(MINIMAL_TEMPLATE.to_string()),
        ..mdxport::Options::default()
    };
    if !trimmed.is_empty() {
        options.title = Some(trimmed.to_string());
    }
    mdxport::markdown_to_pdf(md, &options).context("markdown → PDF (mdxport/Typst) failed")
}

/// Render `md` and write the resulting PDF to `out_path`.
///
/// Thin wrapper over [`markdown_to_pdf_bytes`]; kept as the file-based entry
/// point for the Notes panel export flow. Creates the parent directory if it
/// does not exist.
pub fn markdown_to_pdf(md: &str, title: &str, out_path: &Path) -> Result<()> {
    let bytes = markdown_to_pdf_bytes(md, title)?;
    if let Some(parent) = out_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir for '{}'", out_path.display()))?;
    }
    std::fs::write(out_path, &bytes)
        .with_context(|| format!("failed to write '{}'", out_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_api_produces_valid_pdf_header() {
        let md =
            "# 标题 Heading\n\n你好，世界！Hello 中文 mixed 内容。\n\n- 列表项一\n- 列表项二\n";
        let bytes = markdown_to_pdf_bytes(md, "测试标题").expect("pdf bytes");
        assert!(
            bytes.starts_with(b"%PDF"),
            "PDF should start with %PDF header, got: {:?}",
            bytes.get(..8)
        );
        // A valid PDF ends with %%EOF (possibly followed by a trailing newline).
        assert!(
            bytes.windows(5).any(|w| w == b"%%EOF"),
            "PDF should end with %%EOF marker"
        );
    }

    #[test]
    fn cjk_only_heading_produces_non_empty_pdf() {
        // Regression: a heading containing only CJK characters previously
        // produced a structurally valid but visually blank PDF (the old
        // printpdf pipeline embedded a CFF font under a TrueType descriptor).
        // The mdxport/Typst backend must emit a real, non-empty document.
        let md = "## 所见即所得";
        let bytes = markdown_to_pdf_bytes(md, "测试").expect("pdf bytes");
        assert!(bytes.starts_with(b"%PDF"), "valid PDF header");
        assert!(
            bytes.len() > 1_000,
            "PDF with content should be non-trivial, got {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn file_api_writes_readable_pdf() {
        let dir = tempfile_dir();
        let out = dir.join("cjk.pdf");
        let md = "# 标题\n\n你好世界，Hello 混合内容！\n";
        markdown_to_pdf(md, "测试", &out).expect("write pdf");
        let bytes = std::fs::read(&out).expect("read pdf");
        assert!(bytes.starts_with(b"%PDF"), "valid PDF header");
    }

    /// Minimal temp dir that does not pull in the `tempfile` crate.
    fn tempfile_dir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "verve_notes_pdf_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("create temp dir");
        p
    }
}
