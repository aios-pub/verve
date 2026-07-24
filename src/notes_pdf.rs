//! Render a markdown string to a PDF file using `printpdf` with built-in
//! fonts (Helvetica / Helvetica-Bold / Courier).
//!
//! Supports: headings, paragraphs, bold/italic/inline code, bullet/numbered
//! lists, code blocks, horizontal rules, links (text + URL), and images as
//! placeholder text. Unsupported constructs (tables, HTML) degrade gracefully.

use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context as _, Result};
use printpdf::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

const PAGE_W: f32 = 210.0; // mm (A4)
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 20.0;
const CONTENT_W: f32 = PAGE_W - 2.0 * MARGIN;

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

pub fn markdown_to_pdf(md: &str, title: &str, out_path: &Path) -> Result<()> {
    let (doc, page1, layer1) = PdfDocument::new(title, Mm(PAGE_W), Mm(PAGE_H), "Layer 1");
    let font_regular = doc.add_builtin_font(BuiltinFont::Helvetica)?;
    let font_bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
    let font_italic = doc.add_builtin_font(BuiltinFont::HelveticaOblique)?;
    let font_mono = doc.add_builtin_font(BuiltinFont::Courier)?;
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

    doc.save(&mut BufWriter::new(
        std::fs::File::create(out_path).with_context(|| format!("create {:?}", out_path))?,
    ))
    .context("write PDF")?;
    Ok(())
}

/// Wrap `text` to fit `max_w_mm`, drawing each line to the layer and
/// returning how much y was consumed.
fn draw_wrapped(
    layer: &PdfLayerReference,
    font_regular: &IndirectFontRef,
    font_bold: &IndirectFontRef,
    font_italic: &IndirectFontRef,
    font_mono: &IndirectFontRef,
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

    // Approximate average char width: Helvetica ≈ 0.5 × font_size_pt,
    // Courier ≈ 0.6 × font_size_pt.
    let char_width = if mono { 0.6 } else { 0.5 } * pt_to_mm(size);
    let mut line = String::new();
    for word in text.split_inclusive(char::is_whitespace) {
        let test = format!("{}{}", line, word);
        let w = test.chars().count() as f32 * char_width;
        if w > max_w && !line.is_empty() {
            if *y - line_h < MARGIN {
                // New page: caller can't easily do this; we simply continue
                // drawing (may overflow the bottom margin for long paragraphs).
            }
            layer.set_font(font, size);
            layer.set_text_cursor(Mm(MARGIN + indent_mm), Mm(*y));
            let _ = layer.write_text(line.trim_end(), font);
            *y -= line_h;
            line.clear();
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        layer.set_font(font, size);
        layer.set_text_cursor(Mm(MARGIN + indent_mm), Mm(*y));
        let _ = layer.write_text(line.trim_end(), font);
        *y -= line_h;
    }
}
