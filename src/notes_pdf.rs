//! Render a markdown string to a PDF using `printpdf` with an embedded
//! Noto Sans SC font so CJK (Chinese / Japanese / Korean) glyphs render
//! correctly. printpdf's built-in fonts only cover the Windows-1252 (Latin)
//! encoding, so any non-ASCII character is silently dropped by `write_text`.
//!
//! Supports: headings, paragraphs, bold/italic/inline code, bullet/numbered
//! lists, code blocks, horizontal rules, links (text + URL), and images as
//! placeholder text. Unsupported constructs (tables, HTML) degrade gracefully.
//!
//! CJK text has no inter-word spaces, so line breaking cannot rely on
//! whitespace splitting. `draw_wrapped` measures each glyph's real advance
//! width (via `ab_glyph`) and breaks between any CJK characters, while keeping
//! Latin words intact and avoiding line-leading (forbidden-head) punctuation.

use std::io::{BufWriter, Cursor};
use std::path::Path;

use ab_glyph::{Font, FontRef};
use anyhow::{Context as _, Result};
use printpdf::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthChar;

const PAGE_W: f32 = 210.0; // mm (A4)
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 20.0;
const CONTENT_W: f32 = PAGE_W - 2.0 * MARGIN;

// Embedded Noto Sans SC (SIL Open Font License). SubsetOTF covers the commonly
// used Simplified Chinese glyphs plus Latin/ASCII, so both 中英 mixed content
// renders. Two weights are embedded; italics fall back to the regular shape.
const FONT_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/noto-sans-sc/NotoSansSC-Regular.otf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/noto-sans-sc/NotoSansSC-Bold.otf");

/// Punctuation that must not start a line (CJK 禁则 / 行首禁排).
const NO_LINE_START: &str = "，。、；：！？》）】」』,.;:!?)]}\"'…—~";

fn heading_size(level: HeadingLevel) -> f32 {
    match level {
        HeadingLevel::H1 => 20.0,
        HeadingLevel::H2 => 16.0,
        HeadingLevel::H3 => 13.0,
        HeadingLevel::H4 => 12.0,
        HeadingLevel::H5 | HeadingLevel::H6 => 11.0,
    }
}

/// 1 point = 0.352778 mm.
fn pt_to_mm(pt: f32) -> f32 {
    pt * 0.352_778
}

/// Real per-glyph advance-width measurement backed by `ab_glyph`.
///
/// printpdf's own glyph metrics are `pub(crate)`, so we parse the same font
/// bytes here to obtain advance widths for CJK-aware line breaking.
struct GlyphMeasurer {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

impl GlyphMeasurer {
    fn new() -> Result<Self> {
        // SAFETY: `include_bytes!` yields a `'static` slice; the bytes outlive
        // the `FontRef`. Both fonts were validated at build/download time.
        let regular = FontRef::try_from_slice(FONT_REGULAR)
            .context("parse NotoSansSC-Regular for measurement")?;
        let bold = FontRef::try_from_slice(FONT_BOLD)
            .context("parse NotoSansSC-Bold for measurement")?;
        Ok(Self { regular, bold })
    }

    /// Advance width of `ch` in millimetres when set at `size` points.
    ///
    /// Measures the font's unscaled advance and converts to mm at `size`;
    /// falls back to the East-Asian width heuristic (full-width 1em, else
    /// 0.5em) for characters the subset lacks, so layout still leaves room.
    fn char_width_mm(&self, ch: char, size: f32, bold: bool) -> f32 {
        let font = if bold { &self.bold } else { &self.regular };
        let em = font.units_per_em().unwrap_or(1000.0).max(1.0);
        let gid = font.glyph_id(ch);
        let advance = font.h_advance_unscaled(gid);
        let advance = if advance > 0.0 {
            advance
        } else {
            // Fallback: East-Asian width heuristic (full-width 1em, else 0.5em).
            match ch.width() {
                Some(2) => em,
                _ => em * 0.5,
            }
        };
        // advance is in font units; convert to ems then to mm at the given size.
        pt_to_mm(size) * (advance / em)
    }

    /// Measured width of an arbitrary string in millimetres.
    fn text_width_mm(&self, text: &str, size: f32, bold: bool) -> f32 {
        text.chars()
            .map(|ch| self.char_width_mm(ch, size, bold))
            .sum()
    }
}

/// Whether `ch` may begin a line (false for CJK forbidden-head punctuation).
fn can_start_line(ch: char) -> bool {
    !NO_LINE_START.contains(ch)
}

/// Render `md` to in-memory PDF bytes using the embedded Noto Sans SC font so
/// CJK content renders. Both the Notes panel (file API) and the WYSIWYG
/// Markdown export (bytes API) share this core.
pub fn markdown_to_pdf_bytes(md: &str, title: &str) -> Result<Vec<u8>> {
    let measurer = GlyphMeasurer::new()?;

    let (doc, page1, layer1) = PdfDocument::new(title, Mm(PAGE_W), Mm(PAGE_H), "Layer 1");
    // External fonts: printpdf maps characters to glyph IDs (rather than
    // WinAnsi encoding), so CJK glyphs are emitted instead of dropped.
    let font_regular = doc.add_external_font(&mut Cursor::new(FONT_REGULAR))?;
    let font_bold = doc.add_external_font(&mut Cursor::new(FONT_BOLD))?;
    // Italic / mono reuse the embedded shapes; there is no italic CJK subset,
    // and code blocks render fine in the regular proportional face.
    let font_italic = doc.add_external_font(&mut Cursor::new(FONT_REGULAR))?;
    let font_mono = doc.add_external_font(&mut Cursor::new(FONT_REGULAR))?;
    let mut layer = doc.get_page(page1).get_layer(layer1);

    let mut y: f32 = PAGE_H - MARGIN;
    let mut indent: u32 = 0;
    let mut ordered_stack: Vec<u64> = Vec::new();
    let mut ordered_mode_stack: Vec<bool> = Vec::new();

    let mut buf = String::new();
    let mut bold = false;
    let mut italic = false;
    let mut mono = false;
    let mut size = 11.0f32;
    let mut in_code_block = false;
    let mut code_buf = String::new();
    // The URL of the currently-open link, to be appended on TagEnd::Link.
    let mut pending_link_url: Option<String> = None;

    let mut options = Options::all();
    options.remove(Options::ENABLE_SMART_PUNCTUATION);
    let mut events = Parser::new_ext(md, options).peekable();

    /// Emit the buffer as wrapped text to the PDF layer.
    macro_rules! flush {
        () => {{
            if !buf.is_empty() {
                draw_wrapped(
                    &layer,
                    &font_regular,
                    &font_bold,
                    &font_italic,
                    &font_mono,
                    &measurer,
                    &buf,
                    bold,
                    italic,
                    mono,
                    size,
                    &mut y,
                    indent,
                    CONTENT_W,
                );
                buf.clear();
            }
        }};
    }

    macro_rules! new_page {
        () => {{
            let (p, l) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "newpage");
            layer = doc.get_page(p).get_layer(l);
            y = PAGE_H - MARGIN;
        }};
    }

    while let Some(event) = events.next() {
        // Handle code block collection inline.
        if in_code_block {
            match event {
                Event::End(TagEnd::CodeBlock) => {
                    in_code_block = false;
                    for line in code_buf.lines() {
                        if y - pt_to_mm(9.0) * 1.2 < MARGIN {
                            new_page!();
                        }
                        layer.set_font(&font_mono, 9.0);
                        layer.set_text_cursor(Mm(MARGIN + indent as f32 * 5.0), Mm(y));
                        let _ = layer.write_text(line, &font_mono);
                        y -= pt_to_mm(9.0) * 1.2;
                    }
                    y -= pt_to_mm(9.0);
                    code_buf.clear();
                }
                Event::Text(t) => code_buf.push_str(&t),
                Event::SoftBreak | Event::HardBreak => code_buf.push('\n'),
                _ => {}
            }
            continue;
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    size = 11.0;
                    bold = false;
                    italic = false;
                    mono = false;
                }
                Tag::Heading { level, .. } => {
                    size = heading_size(level);
                    bold = true;
                }
                Tag::Emphasis => italic = true,
                Tag::Strong => bold = true,
                Tag::Strikethrough => {}
                Tag::Link { dest_url, .. } => {
                    pending_link_url = Some(dest_url.to_string());
                }
                Tag::Image { .. } => buf.push_str("[image]"),
                Tag::List(start) => {
                    flush!();
                    indent += 1;
                    ordered_mode_stack.push(start.is_some());
                    ordered_stack.push(start.unwrap_or(1));
                }
                Tag::Item => {
                    let prefix = match ordered_mode_stack.last() {
                        Some(true) => {
                            // Safe: only reached right after `ordered_stack.push(...)`.
                            let n = ordered_stack.last_mut().unwrap();
                            let s = format!("{}. ", *n);
                            *n += 1;
                            s
                        }
                        _ => "• ".to_string(),
                    };
                    buf.push_str(&prefix);
                }
                Tag::CodeBlock(kind) => {
                    let _ = kind;
                    in_code_block = true;
                    code_buf.clear();
                }
                Tag::BlockQuote(_) => {
                    flush!();
                    indent += 1;
                }
                Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => {}
                Tag::FootnoteDefinition(_)
                | Tag::HtmlBlock
                | Tag::MetadataBlock(_)
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    flush!();
                    y -= pt_to_mm(11.0) * 0.6;
                }
                TagEnd::Heading(_) => {
                    flush!();
                    y -= pt_to_mm(size) * 0.5;
                    size = 11.0;
                    bold = false;
                }
                TagEnd::Emphasis => italic = false,
                TagEnd::Strong => bold = false,
                TagEnd::Strikethrough => {}
                TagEnd::Link => {
                    if let Some(url) = pending_link_url.take() {
                        buf.push_str(&format!(" ({url})"));
                    }
                }
                TagEnd::Image => {}
                TagEnd::List(_) => {
                    flush!();
                    indent = indent.saturating_sub(1);
                    ordered_stack.pop();
                    ordered_mode_stack.pop();
                    y -= pt_to_mm(11.0) * 0.4;
                }
                TagEnd::Item => {
                    flush!();
                }
                TagEnd::CodeBlock => {}
                TagEnd::BlockQuote(_) => {
                    flush!();
                    indent = indent.saturating_sub(1);
                }
                _ => {}
            },
            Event::Text(t) => buf.push_str(&t),
            Event::Code(t) => {
                buf.push('`');
                buf.push_str(&t);
                buf.push('`');
            }
            Event::SoftBreak | Event::HardBreak => buf.push(' '),
            Event::Rule => {
                flush!();
                if y - 8.0 < MARGIN {
                    new_page!();
                }
                layer.set_outline_color(Color::Rgb(Rgb::new(0.7, 0.7, 0.7, None)));
                layer.set_outline_thickness(0.5);
                let points = vec![
                    (Point::new(Mm(MARGIN), Mm(y - 2.0)), false),
                    (Point::new(Mm(PAGE_W - MARGIN), Mm(y - 2.0)), false),
                ];
                layer.add_line(Line {
                    points,
                    is_closed: false,
                });
                y -= 8.0;
            }
            Event::Html(h) | Event::InlineHtml(h) => buf.push_str(&h),
            Event::FootnoteReference(_) | Event::TaskListMarker(_) => {}
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }
    flush!();

    let mut bytes: Vec<u8> = Vec::new();
    doc.save(&mut BufWriter::new(Cursor::new(&mut bytes)))
        .context("write PDF")?;
    Ok(bytes)
}

/// Render `md` and write the resulting PDF to `out_path`.
///
/// Thin wrapper over [`markdown_to_pdf_bytes`]; kept as the file-based entry
/// point for the Notes panel export flow.
pub fn markdown_to_pdf(md: &str, title: &str, out_path: &Path) -> Result<()> {
    let bytes = markdown_to_pdf_bytes(md, title)?;
    std::fs::write(out_path, &bytes)
        .with_context(|| format!("failed to write '{}'", out_path.display()))?;
    Ok(())
}

/// Break `text` to fit `max_w_mm`, drawing each line to the layer.
///
/// CJK text contains no inter-word spaces, so we split into breakable units
/// where each CJK character is its own unit and Latin words stay together.
/// Width is measured per-glyph (via `measurer`) rather than approximated, and
/// forbidden-head punctuation (，。！？ etc.) is never left at a line start.
fn draw_wrapped(
    layer: &PdfLayerReference,
    font_regular: &IndirectFontRef,
    font_bold: &IndirectFontRef,
    font_italic: &IndirectFontRef,
    font_mono: &IndirectFontRef,
    measurer: &GlyphMeasurer,
    text: &str,
    bold: bool,
    italic: bool,
    mono: bool,
    size: f32,
    y: &mut f32,
    indent: u32,
    max_w_mm: f32,
) {
    let font = if mono {
        font_mono
    } else if bold {
        font_bold
    } else if italic {
        font_italic
    } else {
        font_regular
    };
    let line_h = pt_to_mm(size) * 1.3;
    let indent_mm = indent as f32 * 5.0;
    let max_w = max_w_mm - indent_mm;

    // Split into breakable units: CJK chars stand alone, runs of non-space
    // ASCII/Latin form one unit, whitespace is preserved as glue.
    let units = split_break_units(text);

    let mut line = String::new();
    let mut line_w = 0.0f32;

    let emit_line = |line_str: &str, y: &mut f32| {
        layer.set_font(font, size);
        layer.set_text_cursor(Mm(MARGIN + indent_mm), Mm(*y));
        let _ = layer.write_text(line_str.trim_end(), font);
        *y -= line_h;
    };

    for unit in units {
        let unit_w = measurer.text_width_mm(&unit, size, bold);
        let would_overflow = line_w + unit_w > max_w && !line.is_empty();

        if would_overflow {
            // CJK 禁则 (line-start rule): if the unit that would begin the next
            // line is forbidden-head punctuation, keep it on the current line
            // (accepting slight overflow) rather than starting a line with it.
            let starts_with_punct = unit.chars().next().is_some_and(|c| !can_start_line(c));
            if !starts_with_punct {
                emit_line(&line, y);
                line.clear();
                line_w = 0.0;
            }
        }
        line.push_str(&unit);
        line_w += unit_w;
    }
    if !line.is_empty() {
        emit_line(&line, y);
    }
}

/// Split `text` into line-break units suitable for mixed CJK / Latin layout.
///
/// - Each CJK ideograph (East-Asian width == 2) becomes its own unit, allowing
///   a break between any two CJK characters.
/// - A run of ASCII letters/digits forms one unit (no mid-word break).
/// - Whitespace and other characters are individual glue units.
fn split_break_units(text: &str) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    let mut ascii_run = String::new();
    for ch in text.chars() {
        let is_cjk = matches!(ch.width(), Some(2));
        let is_ascii_alnum = ch.is_ascii_alphanumeric();
        if is_ascii_alnum {
            ascii_run.push(ch);
            continue;
        }
        if !ascii_run.is_empty() {
            units.push(std::mem::take(&mut ascii_run));
        }
        if is_cjk {
            units.push(ch.to_string());
        } else {
            // Whitespace, punctuation, etc. — each is its own unit.
            units.push(ch.to_string());
        }
    }
    if !ascii_run.is_empty() {
        units.push(ascii_run);
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_mixed_cjk_and_latin_pdf() {
        let md = "# 标题 Heading\n\n你好，世界！Hello 中文 mixed 内容。\n\n- 列表项一\n- 列表项二\n";
        let bytes = markdown_to_pdf_bytes(md, "测试标题").expect("pdf bytes");
        assert!(
            bytes.starts_with(b"%PDF"),
            "PDF should start with %PDF header, got: {:?}",
            bytes.get(..8)
        );
        // Embedded Noto Sans SC font makes the output sizeable.
        assert!(
            bytes.len() > 100_000,
            "PDF with embedded CJK font should be >100KB, got {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn measurer_gives_nonzero_cjk_width() {
        let m = GlyphMeasurer::new().expect("measurer");
        let cjk = m.char_width_mm('字', 11.0, false);
        let latin = m.char_width_mm('A', 11.0, false);
        assert!(cjk > 0.0, "CJK glyph width should be positive");
        assert!(latin > 0.0, "Latin glyph width should be positive");
        // A CJK ideograph is roughly twice as wide as a Latin capital.
        assert!(
            cjk > latin * 1.3,
            "CJK ({cjk:.3}mm) should be wider than Latin ({latin:.3}mm)"
        );
    }

    #[test]
    fn split_units_separates_cjk_and_words() {
        let units = split_break_units("你好abc世界");
        // 你 / 好 / abc / 世 / 界
        assert_eq!(units, vec!["你", "好", "abc", "世", "界"]);
    }

    #[test]
    fn can_start_line_rejects_cjk_punct() {
        assert!(!can_start_line('，'));
        assert!(!can_start_line('。'));
        assert!(!can_start_line('）'));
        assert!(can_start_line('A'));
        assert!(can_start_line('你'));
    }

    #[test]
    fn file_api_embeds_font_and_renders_cjk() {
        let dir = tempfile_dir();
        let out = dir.join("cjk.pdf");
        let md = "# 标题\n\n你好世界，Hello 混合内容！\n";
        markdown_to_pdf(md, "测试", &out).expect("write pdf");
        let bytes = std::fs::read(&out).expect("read pdf");
        assert!(bytes.starts_with(b"%PDF"), "valid PDF header");
        // An embedded TrueType/OpenType font proves glyphs (incl. CJK) are
        // mapped to glyph IDs rather than dropped via WinAnsi.
        assert!(
            memmem(&bytes, b"/FontFile") || memmem(&bytes, b"/FontFile3"),
            "PDF should embed a font stream (/FontFile or /FontFile3)"
        );
        assert!(
            bytes.len() > 100_000,
            "embedded CJK font makes the file sizeable, got {} bytes",
            bytes.len()
        );
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

    fn memmem(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
}
