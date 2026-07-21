//! Markdown parser — converts CommonMark markdown into an intermediate `MdNode`
//! tree that the UI layer can render as GPUI elements.
//!
//! This module is pure logic (no GPUI dependency), so it can be unit-tested
//! without a window context.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

/// A block-level Markdown node.
#[derive(Debug, Clone, PartialEq)]
pub enum MdNode {
    Heading(u32, Vec<MdInline>),
    Paragraph(Vec<MdInline>),
    /// `ordered` = true for numbered lists; items are themselves lists of blocks.
    List(bool, Vec<Vec<MdNode>>),
    CodeBlock { lang: String, code: String },
    Blockquote(Vec<MdNode>),
    Rule,
    Table {
        header: Vec<Vec<MdInline>>,
        rows: Vec<Vec<Vec<MdInline>>>,
    },
}

/// An inline Markdown span (text with formatting).
#[derive(Debug, Clone, PartialEq)]
pub enum MdInline {
    Text(String),
    Bold(Vec<MdInline>),
    Italic(Vec<MdInline>),
    Strikethrough(Vec<MdInline>),
    Code(String),
    Link { text: String, url: String },
    Image { alt: String, url: String },
}

/// Parse CommonMark markdown source into a tree of `MdNode` blocks.
pub fn parse_markdown(md: &str) -> Vec<MdNode> {
    let parser = Parser::new_ext(
        md,
        pulldown_cmark::Options::all() & !pulldown_cmark::Options::ENABLE_SMART_PUNCTUATION,
    );

    let mut blocks: Vec<MdNode> = Vec::new();
    let mut inline_stack: Vec<Vec<MdInline>> = vec![Vec::new()];
    // Stack of open list contexts: (ordered, completed_items, current_item_blocks)
    let mut list_stack: Vec<(bool, Vec<Vec<MdNode>>, Vec<MdNode>)> = Vec::new();
    let mut blockquote_stack: Vec<Vec<MdNode>> = Vec::new();
    let mut table_state: Option<TableState> = None;
    let mut pending_code_lang: Option<String> = None;
    let mut pending_code: Option<String> = None;
    let mut pending_heading: Option<u32> = None;
    let mut link_stack: Vec<String> = Vec::new();
    let mut image_stack: Vec<String> = Vec::new();
    // When inside an explicit Paragraph, we don't auto-create one at End(Item).
    let mut in_paragraph: bool = false;

    /// Flush accumulated inline text from the top of inline_stack into a Paragraph
    /// block pushed to the current container (list item / blockquote / root).
    fn flush_inlines(
        inline_stack: &mut Vec<Vec<MdInline>>,
        blocks: &mut Vec<MdNode>,
        list_stack: &mut Vec<(bool, Vec<Vec<MdNode>>, Vec<MdNode>)>,
        blockquote_stack: &mut Vec<Vec<MdNode>>,
    ) {
        while inline_stack.len() > 1 {
            let inlines = inline_stack.pop().unwrap_or_default();
            if !inlines.is_empty() {
                if let Some(parent) = inline_stack.last_mut() {
                    parent.extend(inlines);
                }
            }
        }
        let inlines = std::mem::take(inline_stack.first_mut().unwrap());
        if !inlines.is_empty() {
            let node = MdNode::Paragraph(inlines);
            push_block(blocks, list_stack, blockquote_stack, node);
        }
    }

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    pending_heading = Some(heading_level(level));
                    in_paragraph = false;
                }
                Tag::Paragraph => {
                    in_paragraph = true;
                }
                Tag::Emphasis => {
                    inline_stack.push(Vec::new());
                }
                Tag::Strong => {
                    inline_stack.push(Vec::new());
                }
                Tag::Strikethrough => {
                    inline_stack.push(Vec::new());
                }
                Tag::List(start) => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    list_stack.push((start.is_some(), Vec::new(), Vec::new()));
                    in_paragraph = false;
                }
                Tag::Item => {
                    // Starting a new list item. Reset inline accumulator.
                    in_paragraph = false;
                }
                Tag::BlockQuote(_) => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    blockquote_stack.push(Vec::new());
                    in_paragraph = false;
                }
                Tag::CodeBlock(kind) => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    let lang = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(s) => s.to_string(),
                        pulldown_cmark::CodeBlockKind::Indented => String::new(),
                    };
                    pending_code_lang = Some(lang);
                    pending_code = Some(String::new());
                    in_paragraph = false;
                }
                Tag::Table(_) => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    table_state = Some(TableState {
                        header: Vec::new(),
                        rows: Vec::new(),
                        current_row: Vec::new(),
                        in_header: false,
                    });
                    in_paragraph = false;
                }
                Tag::TableHead => {
                    if let Some(ts) = &mut table_state {
                        ts.in_header = true;
                    }
                }
                Tag::TableRow => {
                    if let Some(ts) = &mut table_state {
                        ts.current_row = Vec::new();
                    }
                }
                Tag::TableCell => {
                    inline_stack.push(Vec::new());
                }
                Tag::Link { dest_url, .. } => {
                    link_stack.push(dest_url.to_string());
                    inline_stack.push(Vec::new());
                }
                Tag::Image { dest_url, .. } => {
                    image_stack.push(dest_url.to_string());
                    inline_stack.push(Vec::new());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    if let Some(level) = pending_heading.take() {
                        // Collapse any nested inline formatting into the top level.
                        while inline_stack.len() > 1 {
                            let inner = inline_stack.pop().unwrap_or_default();
                            if !inner.is_empty() {
                                if let Some(parent) = inline_stack.last_mut() {
                                    parent.extend(inner);
                                }
                            }
                        }
                        let inlines = std::mem::take(inline_stack.first_mut().unwrap());
                        let node = MdNode::Heading(level, inlines);
                        push_block(
                            &mut blocks,
                            &mut list_stack,
                            &mut blockquote_stack,
                            node,
                        );
                    }
                    in_paragraph = false;
                }
                TagEnd::Paragraph => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    in_paragraph = false;
                }
                TagEnd::Emphasis => {
                    let inner = inline_stack.pop().unwrap_or_default();
                    inline_stack
                        .last_mut()
                        .unwrap()
                        .push(MdInline::Italic(inner));
                }
                TagEnd::Strong => {
                    let inner = inline_stack.pop().unwrap_or_default();
                    inline_stack
                        .last_mut()
                        .unwrap()
                        .push(MdInline::Bold(inner));
                }
                TagEnd::Strikethrough => {
                    let inner = inline_stack.pop().unwrap_or_default();
                    inline_stack
                        .last_mut()
                        .unwrap()
                        .push(MdInline::Strikethrough(inner));
                }
                TagEnd::List(_) => {
                    // Flush any trailing inline text in current item.
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    if let Some((ordered, items, _)) = list_stack.pop() {
                        let node = MdNode::List(ordered, items);
                        push_block(
                            &mut blocks,
                            &mut list_stack,
                            &mut blockquote_stack,
                            node,
                        );
                    }
                    in_paragraph = false;
                }
                TagEnd::Item => {
                    // Flush text that appeared directly in the item (no Paragraph wrapper).
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    if let Some((_, items, current)) = list_stack.last_mut() {
                        items.push(std::mem::take(current));
                    }
                    in_paragraph = false;
                }
                TagEnd::BlockQuote(_) => {
                    flush_inlines(
                        &mut inline_stack,
                        &mut blocks,
                        &mut list_stack,
                        &mut blockquote_stack,
                    );
                    let inner = blockquote_stack.pop().unwrap_or_default();
                    let node = MdNode::Blockquote(inner);
                    push_block(&mut blocks, &mut list_stack, &mut blockquote_stack, node);
                    in_paragraph = false;
                }
                TagEnd::CodeBlock => {
                    if let (Some(lang), Some(code)) = (pending_code_lang.take(), pending_code.take())
                    {
                        let code = code.trim_end_matches('\n').to_string();
                        let node = MdNode::CodeBlock { lang, code };
                        push_block(
                            &mut blocks,
                            &mut list_stack,
                            &mut blockquote_stack,
                            node,
                        );
                    }
                    in_paragraph = false;
                }
                TagEnd::Table => {
                    if let Some(ts) = table_state.take() {
                        let node = MdNode::Table {
                            header: ts.header,
                            rows: ts.rows,
                        };
                        push_block(
                            &mut blocks,
                            &mut list_stack,
                            &mut blockquote_stack,
                            node,
                        );
                    }
                    in_paragraph = false;
                }
                TagEnd::TableHead => {
                    if let Some(ts) = &mut table_state {
                        ts.header = std::mem::take(&mut ts.current_row);
                        ts.in_header = false;
                    }
                }
                TagEnd::TableRow => {
                    if let Some(ts) = &mut table_state {
                        let row = std::mem::take(&mut ts.current_row);
                        if ts.in_header {
                            ts.header = row;
                        } else {
                            ts.rows.push(row);
                        }
                    }
                }
                TagEnd::TableCell => {
                    while inline_stack.len() > 1 {
                        let inner = inline_stack.pop().unwrap_or_default();
                        if !inner.is_empty() {
                            if let Some(parent) = inline_stack.last_mut() {
                                parent.extend(inner);
                            }
                        }
                    }
                    let inlines = std::mem::take(inline_stack.first_mut().unwrap());
                    if let Some(ts) = &mut table_state {
                        ts.current_row.push(inlines);
                    }
                }
                TagEnd::Link => {
                    let inner = inline_stack.pop().unwrap_or_default();
                    if let Some(url) = link_stack.pop() {
                        let text = collect_text(&inner);
                        inline_stack
                            .last_mut()
                            .unwrap()
                            .push(MdInline::Link { text, url });
                    }
                }
                TagEnd::Image => {
                    let inner = inline_stack.pop().unwrap_or_default();
                    if let Some(url) = image_stack.pop() {
                        let alt = collect_text(&inner);
                        inline_stack
                            .last_mut()
                            .unwrap()
                            .push(MdInline::Image { alt, url });
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if pending_code.is_some() {
                    pending_code.as_mut().unwrap().push_str(&text);
                } else {
                    inline_stack
                        .last_mut()
                        .unwrap()
                        .push(MdInline::Text(text.to_string()));
                }
            }
            Event::Code(text) => {
                inline_stack
                    .last_mut()
                    .unwrap()
                    .push(MdInline::Code(text.to_string()));
            }
            Event::SoftBreak | Event::HardBreak => {
                inline_stack
                    .last_mut()
                    .unwrap()
                    .push(MdInline::Text(" ".to_string()));
            }
            Event::Rule => {
                flush_inlines(
                    &mut inline_stack,
                    &mut blocks,
                    &mut list_stack,
                    &mut blockquote_stack,
                );
                push_block(
                    &mut blocks,
                    &mut list_stack,
                    &mut blockquote_stack,
                    MdNode::Rule,
                );
            }
            Event::FootnoteReference(_) | Event::Html(_) | Event::InlineHtml(_) => {}
            Event::TaskListMarker(_) => {}
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    // Flush any trailing content.
    flush_inlines(
        &mut inline_stack,
        &mut blocks,
        &mut list_stack,
        &mut blockquote_stack,
    );

    blocks
}

struct TableState {
    header: Vec<Vec<MdInline>>,
    rows: Vec<Vec<Vec<MdInline>>>,
    current_row: Vec<Vec<MdInline>>,
    in_header: bool,
}

fn heading_level(level: HeadingLevel) -> u32 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn collect_text(inlines: &[MdInline]) -> String {
    let mut s = String::new();
    for i in inlines {
        match i {
            MdInline::Text(t) => s.push_str(t),
            MdInline::Code(c) => s.push_str(c),
            MdInline::Bold(inner) | MdInline::Italic(inner) | MdInline::Strikethrough(inner) => {
                s.push_str(&collect_text(inner))
            }
            MdInline::Link { text, .. } => s.push_str(text),
            MdInline::Image { alt, .. } => s.push_str(alt),
        }
    }
    s
}

/// Push a block node to the current innermost container (root blocks, list item, or blockquote).
fn push_block(
    blocks: &mut Vec<MdNode>,
    list_stack: &mut [(bool, Vec<Vec<MdNode>>, Vec<MdNode>)],
    blockquote_stack: &mut [Vec<MdNode>],
    node: MdNode,
) {
    if let Some((_, _, current)) = list_stack.last_mut() {
        current.push(node);
    } else if let Some(inner) = blockquote_stack.last_mut() {
        inner.push(node);
    } else {
        blocks.push(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_paragraph() {
        let md = "Hello world";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            MdNode::Paragraph(inlines) => {
                assert_eq!(inlines.len(), 1);
                assert!(matches!(&inlines[0], MdInline::Text(t) if t == "Hello world"));
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn parses_headings() {
        let md = "# Title\n\n## Subtitle";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 2);
        assert!(matches!(&nodes[0], MdNode::Heading(1, _)));
        assert!(matches!(&nodes[1], MdNode::Heading(2, _)));
    }

    #[test]
    fn parses_bold_italic() {
        let md = "This is **bold** and *italic*";
        let nodes = parse_markdown(md);
        match &nodes[0] {
            MdNode::Paragraph(inlines) => {
                assert!(inlines.iter().any(|i| matches!(i, MdInline::Bold(_))));
                assert!(inlines.iter().any(|i| matches!(i, MdInline::Italic(_))));
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn parses_code_block() {
        let md = "```rust\nfn main() {}\n```";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            MdNode::CodeBlock { lang, code } => {
                assert_eq!(lang, "rust");
                assert!(code.contains("fn main()"));
            }
            _ => panic!("expected code block"),
        }
    }

    #[test]
    fn parses_unordered_list() {
        let md = "- item 1\n- item 2\n- item 3";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            MdNode::List(ordered, items) => {
                assert!(!ordered);
                assert_eq!(items.len(), 3, "expected 3 items, got {:?}", items);
                for item in items {
                    assert_eq!(item.len(), 1, "each item should contain one paragraph");
                    assert!(matches!(item[0], MdNode::Paragraph(_)));
                }
            }
            _ => panic!("expected list, got {:?}", nodes),
        }
    }

    #[test]
    fn parses_ordered_list() {
        let md = "1. first\n2. second";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 1);
        match &nodes[0] {
            MdNode::List(ordered, items) => {
                assert!(ordered);
                assert_eq!(items.len(), 2);
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn parses_link() {
        let md = "[click here](https://example.com)";
        let nodes = parse_markdown(md);
        match &nodes[0] {
            MdNode::Paragraph(inlines) => {
                assert!(inlines.iter().any(|i| matches!(
                    i, MdInline::Link { text, url } if text == "click here" && url == "https://example.com"
                )));
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn parses_blockquote() {
        let md = "> quoted text\n> more text";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(&nodes[0], MdNode::Blockquote(_)));
    }

    #[test]
    fn parses_horizontal_rule() {
        let md = "before\n\n---\n\nafter";
        let nodes = parse_markdown(md);
        assert_eq!(nodes.len(), 3);
        assert!(matches!(nodes[1], MdNode::Rule));
    }

    #[test]
    fn parses_inline_code() {
        let md = "Use `println!` for output";
        let nodes = parse_markdown(md);
        match &nodes[0] {
            MdNode::Paragraph(inlines) => {
                assert!(inlines.iter().any(|i| matches!(i, MdInline::Code(c) if c == "println!")));
            }
            _ => panic!("expected paragraph"),
        }
    }
}
