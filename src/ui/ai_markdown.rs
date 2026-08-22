//! Lightweight markdown renderer for AI assistant messages.
//!
//! Re-parses on every render — chat bubbles are a few KB and pulldown-cmark
//! parses that in microseconds, so streaming just re-renders the growing
//! text without any per-message entity bookkeeping. Covers what chat models
//! actually emit: headings, paragraphs with bold/italic/inline-code/links,
//! fenced code blocks, ordered & nested bullet lists, block quotes, rules;
//! tables fall back to plain monospace lines.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{Theme, h_flex, v_flex};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

// ---------------------------------------------------------------------------
// Pure parse layer (unit-testable, no gpui types)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct InlineStyle {
    bold: bool,
    italic: bool,
    code: bool,
    link: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    text: String,
    style: InlineStyle,
}

/// One rendered block.
#[derive(Clone, Debug, PartialEq)]
pub enum MdBlock {
    Paragraph { segs: Vec<Seg>, quoted: bool },
    Heading { level: u8, segs: Vec<Seg> },
    Code { lang: Option<String>, text: String },
    ListItem { depth: usize, ordered: bool, number: u64, segs: Vec<Seg> },
    Rule,
    /// Tables (rare from chat models) render as plain lines.
    PlainLine(String),
}

/// Parse markdown into blocks. Inline styles are captured per segment.
pub fn parse_blocks(text: &str) -> Vec<MdBlock> {
    let mut blocks: Vec<MdBlock> = Vec::new();
    let mut style = InlineStyle::default();
    let mut line: Vec<Seg> = Vec::new();
    // Heading level while inside a heading (0 = none).
    let mut heading_level: u8 = 0;
    let mut quote_depth: usize = 0;
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut in_item = false;
    // Code block accumulation.
    let mut code_lang: Option<Option<String>> = None;
    let mut code_buf = String::new();
    // Table accumulation (fallback rendering).
    let mut table_depth: usize = 0;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_cell = String::new();
    let mut table_lines: Vec<String> = Vec::new();

    let push_seg = |line: &mut Vec<Seg>, text: &str, style: InlineStyle| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = line.last_mut() {
            if last.style == style {
                last.text.push_str(text);
                return;
            }
        }
        line.push(Seg {
            text: text.to_string(),
            style,
        });
    };

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    for ev in Parser::new_ext(text, options) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph { .. } => {}
                Tag::Heading { level, .. } => heading_level = level as u8,
                Tag::BlockQuote(_) => quote_depth += 1,
                Tag::CodeBlock(kind) => {
                    code_lang = Some(match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(info) => info
                            .split(',')
                            .next()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                        pulldown_cmark::CodeBlockKind::Indented => None,
                    });
                    code_buf.clear();
                }
                Tag::List(start) => list_stack.push(start),
                Tag::Item => {
                    in_item = true;
                    line.clear();
                }
                Tag::Table(_) => table_depth += 1,
                Tag::TableRow | Tag::TableHead => {}
                Tag::TableCell => table_cell.clear(),
                Tag::Emphasis | Tag::Strikethrough => style.italic = !style.italic,
                Tag::Strong => style.bold = !style.bold,
                Tag::Link { .. } => style.link = true,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if in_item {
                        // Paragraph inside a list item — keep appending inline.
                    } else {
                        blocks.push(MdBlock::Paragraph {
                            segs: std::mem::take(&mut line),
                            quoted: quote_depth > 0,
                        });
                    }
                }
                TagEnd::Heading(_) => {
                    blocks.push(MdBlock::Heading {
                        level: heading_level,
                        segs: std::mem::take(&mut line),
                    });
                    heading_level = 0;
                }
                TagEnd::BlockQuote(_) => quote_depth = quote_depth.saturating_sub(1),
                TagEnd::CodeBlock => {
                    blocks.push(MdBlock::Code {
                        lang: code_lang.take().flatten(),
                        text: std::mem::take(&mut code_buf),
                    });
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                }
                TagEnd::Item => {
                    let depth = list_stack.len().saturating_sub(1);
                    let (ordered, number) = match list_stack.last() {
                        Some(Some(n)) => (true, *n),
                        _ => (false, 0),
                    };
                    if let Some(Some(n)) = list_stack.last_mut() {
                        *n += 1;
                    }
                    blocks.push(MdBlock::ListItem {
                        depth,
                        ordered,
                        number,
                        segs: std::mem::take(&mut line),
                    });
                    in_item = false;
                }
                TagEnd::Table => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 {
                        blocks.extend(
                            std::mem::take(&mut table_lines)
                                .into_iter()
                                .map(MdBlock::PlainLine),
                        );
                    }
                }
                TagEnd::TableRow | TagEnd::TableHead => {
                    if !table_row.is_empty() {
                        table_lines.push(table_row.join(" | "));
                        table_row.clear();
                    }
                }
                TagEnd::TableCell => {
                    table_row.push(std::mem::take(&mut table_cell).trim().to_string());
                }
                TagEnd::Emphasis | TagEnd::Strikethrough => style.italic = !style.italic,
                TagEnd::Strong => style.bold = !style.bold,
                TagEnd::Link => style.link = false,
                _ => {}
            },
            Event::Text(t) => {
                if code_lang.is_some() {
                    code_buf.push_str(&t);
                } else if table_depth > 0 {
                    table_cell.push_str(&t);
                } else {
                    push_seg(&mut line, &t, style);
                }
            }
            Event::Code(t) => push_seg(&mut line, &t, InlineStyle { code: true, ..style }),
            Event::SoftBreak | Event::HardBreak => {
                if code_lang.is_some() {
                    code_buf.push('\n');
                } else {
                    push_seg(&mut line, " ", style);
                }
            }
            Event::Rule => blocks.push(MdBlock::Rule),
            _ => {}
        }
    }
    // Trailing paragraph without closing tag (shouldn't happen, be safe).
    if !line.is_empty() {
        blocks.push(MdBlock::Paragraph {
            segs: line,
            quoted: quote_depth > 0,
        });
    }
    blocks
}

// ---------------------------------------------------------------------------
// Render layer
// ---------------------------------------------------------------------------

fn base_font(theme: &Theme) -> Font {
    Font {
        family: theme.font_family.clone(),
        ..Default::default()
    }
}

fn mono_font(theme: &Theme, bold: bool) -> Font {
    Font {
        family: theme.mono_font_family.clone(),
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::default()
        },
        ..Default::default()
    }
}

fn segs_to_styled(segs: &[Seg], theme: &Theme, color: Hsla) -> StyledText {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    for seg in segs {
        let s = &seg.style;
        let run = if s.code {
            TextRun {
                len: seg.text.len(),
                font: mono_font(theme, false),
                color: theme.accent_foreground,
                background_color: Some(theme.muted),
                underline: None,
                strikethrough: None,
            }
        } else {
            let link_color = theme.link;
            let font = Font {
                family: theme.font_family.clone(),
                weight: if s.bold { FontWeight::BOLD } else { FontWeight::default() },
                style: if s.italic { FontStyle::Italic } else { FontStyle::Normal },
                ..Default::default()
            };
            TextRun {
                len: seg.text.len(),
                font,
                color: if s.link { link_color } else { color },
                background_color: None,
                underline: if s.link {
                    Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(link_color),
                        wavy: false,
                    })
                } else {
                    None
                },
                strikethrough: None,
            }
        };
        text.push_str(&seg.text);
        runs.push(run);
    }
    StyledText::new(text).with_runs(runs)
}

fn heading_size(level: u8) -> Pixels {
    match level {
        1 => px(17.),
        2 => px(16.),
        3 => px(15.),
        _ => px(14.),
    }
}

fn render_block(block: &MdBlock, theme: &Theme) -> AnyElement {
    let fg = theme.foreground;
    match block {
        MdBlock::Paragraph { segs, quoted } => {
            let text = div().min_w_0().text_sm().child(segs_to_styled(segs, theme, fg));
            if *quoted {
                div()
                    .border_l_2()
                    .border_color(theme.accent.opacity(0.5))
                    .pl_2()
                    .py_0p5()
                    .text_color(theme.muted_foreground)
                    .child(text)
                    .into_any_element()
            } else {
                text.into_any_element()
            }
        }
        MdBlock::Heading { level, segs } => div()
            .text_size(heading_size(*level))
            .font_weight(FontWeight::SEMIBOLD)
            .child(segs_to_styled(segs, theme, fg))
            .into_any_element(),
        MdBlock::Code { lang, text } => {
            let code_size = theme.mono_font_size - px(1.);
            v_flex()
                .my_1()
                .min_w_0()
                .rounded_md()
                .bg(theme.muted)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .when_some(
                    lang.as_deref().filter(|l| !l.is_empty()),
                    |col, l| {
                        col.child(
                            h_flex()
                                .px_2()
                                .pt_1()
                                .justify_end()
                                .child(div().text_xs().text_color(theme.muted_foreground).child(l.to_string())),
                        )
                    },
                )
                .child(
                    div()
                        .px_2()
                        .pb_1p5()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(code_size)
                        .text_color(fg)
                        .child(StyledText::new(text.clone())),
                )
                .into_any_element()
        }
        MdBlock::ListItem {
            depth,
            ordered,
            number,
            segs,
        } => {
            let marker = if *ordered {
                format!("{number}. ")
            } else {
                "• ".to_string()
            };
            h_flex()
                .pl(px(*depth as f32 * 14.0))
                .gap_1()
                .items_start()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(marker),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .child(segs_to_styled(segs, theme, fg)),
                )
                .into_any_element()
        }
        MdBlock::Rule => div()
            .my_2()
            .h(px(1.))
            .bg(theme.border)
            .into_any_element(),
        MdBlock::PlainLine(line) => div()
            .min_w_0()
            .overflow_hidden()
            .text_xs()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.muted_foreground)
            .child(line.clone())
            .into_any_element(),
    }
}

/// Render a full markdown text as a vertical stack of blocks.
pub fn render_markdown(text: &str, theme: &Theme) -> AnyElement {
    if text.trim().is_empty() {
        return div().into_any_element();
    }
    let theme = theme.clone();
    let blocks = parse_blocks(text);
    let mut col = v_flex().gap_1().w_full().min_w_0();
    for b in &blocks {
        col = col.child(render_block(b, &theme));
    }
    col.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{MdBlock, parse_blocks};

    fn plain(segs: &[(&str, bool, bool)]) -> Vec<super::Seg> {
        segs.iter()
            .map(|&(t, bold, code)| super::Seg {
                text: t.to_string(),
                style: super::InlineStyle {
                    bold,
                    code,
                    ..Default::default()
                },
            })
            .collect()
    }

    #[test]
    fn parses_basic_chat_markdown() {
        let md = "# 标题\n\n**加粗** 与 `代码` 与 [链接](https://x)\n\n- 甲\n- 乙\n\n1. 第一\n2. 第二\n";
        let blocks = parse_blocks(md);
        assert_eq!(blocks.len(), 6, "{blocks:?}");
        assert!(matches!(&blocks[0], MdBlock::Heading { level: 1, .. }));
        let MdBlock::Paragraph { segs, quoted } = &blocks[1] else {
            panic!("paragraph");
        };
        assert!(!quoted);
        // 加粗 | " 与 " | 代码 | " 与 " | 链接文本
        assert_eq!(segs.len(), 5, "{segs:?}");
        assert!(segs[0].style.bold);
        assert!(segs[2].style.code);
        assert!(segs[4].style.link);
        assert!(matches!(&blocks[2], MdBlock::ListItem { ordered: false, .. }));
        assert!(matches!(&blocks[4], MdBlock::ListItem { ordered: true, number: 1, .. }));
    }

    #[test]
    fn parses_code_block_with_lang_and_keeps_newlines() {
        let md = "```json\n{\"a\": 1}\n```";
        let blocks = parse_blocks(md);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            MdBlock::Code { lang, text } => {
                assert_eq!(lang.as_deref(), Some("json"));
                assert_eq!(text, "{\"a\": 1}\n");
            }
            _ => panic!("code block expected"),
        }
    }

    #[test]
    fn quote_list_nesting_and_partial_stream() {
        // Half-streamed fenced block (no closing fence yet).
        let blocks = parse_blocks("说明:\n```rust\nfn main() {");
        assert!(blocks
            .iter()
            .any(|b| matches!(b, MdBlock::Code { .. })), "{blocks:?}");

        // Quote + nested bullet flatten with depth.
        let blocks = parse_blocks("> 引用\n\n- 外层\n  - 内层\n");
        assert!(blocks.iter().any(|b| matches!(
            b,
            MdBlock::ListItem { depth: 1, .. }
        )));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, MdBlock::Paragraph { quoted: true, .. })));
    }

    #[test]
    fn table_falls_back_to_plain_lines() {
        let blocks = parse_blocks("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(blocks
            .iter()
            .any(|b| matches!(b, MdBlock::PlainLine(l) if l.contains("1 | 2"))), "{blocks:?}");
    }

    #[test]
    fn plain_paragraph() {
        let blocks = parse_blocks("普通文本一行");
        assert_eq!(
            blocks,
            vec![MdBlock::Paragraph {
                segs: plain(&[("普通文本一行", false, false)]),
                quoted: false,
            }]
        );
    }
}
