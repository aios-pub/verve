//! Reusable Markdown renderer — converts parsed MdNode tree into GPUI elements.
//!
//! Theme-aware: colors and fonts come from the active GPUI theme.

use std::cell::Cell;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{h_flex, v_flex, ActiveTheme, Sizable as _};
use pulldown_cmark::Alignment;

use crate::markdown_render::{MdInline, MdNode};

/// Render markdown source into a GPUI element.
pub fn render_markdown(md: &str, cx: &App) -> AnyElement {
    let nodes = crate::markdown_render::parse_markdown(md);
    render_nodes(&nodes, cx)
}

/// Render a slice of MdNode blocks into a GPUI element.
pub fn render_nodes(nodes: &[MdNode], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let link_id_counter = Cell::new(0usize);
    let mut children = Vec::new();
    for node in nodes {
        children.push(render_node(node, cx, 0, &link_id_counter));
    }
    v_flex()
        .w_full()
        .gap(px(4.))
        .text_size(px(14.))
        .text_color(theme.foreground)
        .children(children)
        .into_any_element()
}

fn render_node(node: &MdNode, cx: &App, depth: u32, link_id_counter: &Cell<usize>) -> AnyElement {
    let theme = cx.theme();
    match node {
        MdNode::Heading(level, inlines) => {
            // Match gpui-component / Zed heading scale (rems × 14px base):
            //   H1: 2.00rem = 28px BOLD, bottom border
            //   H2: 1.50rem = 21px SEMIBOLD, bottom border
            //   H3: 1.25rem = 18px SEMIBOLD
            //   H4: 1.125rem = 16px SEMIBOLD
            //   H5: 1.00rem = 14px SEMIBOLD
            //   H6: 1.00rem = 14px MEDIUM, muted color
            let (size, weight, mt, mb, border_b, color) = match level {
                1 => (
                    px(28.),
                    FontWeight::BOLD,
                    px(20.),
                    px(10.),
                    true,
                    theme.foreground,
                ),
                2 => (
                    px(21.),
                    FontWeight::SEMIBOLD,
                    px(16.),
                    px(8.),
                    true,
                    theme.foreground,
                ),
                3 => (
                    px(18.),
                    FontWeight::SEMIBOLD,
                    px(14.),
                    px(6.),
                    false,
                    theme.foreground,
                ),
                4 => (
                    px(16.),
                    FontWeight::SEMIBOLD,
                    px(12.),
                    px(4.),
                    false,
                    theme.foreground,
                ),
                5 => (
                    px(14.),
                    FontWeight::SEMIBOLD,
                    px(10.),
                    px(4.),
                    false,
                    theme.foreground,
                ),
                _ => (
                    px(14.),
                    FontWeight::MEDIUM,
                    px(8.),
                    px(4.),
                    false,
                    theme.muted_foreground,
                ),
            };
            let mut el = div()
                .w_full()
                .mt(mt)
                .mb(mb)
                .text_size(size)
                .font_weight(weight)
                .text_color(color)
                .children(render_inlines(inlines, cx, link_id_counter));
            if border_b {
                el = el.pb(px(6.)).border_b_1().border_color(theme.border);
            }
            el.into_any_element()
        }
        MdNode::Paragraph(inlines) => div()
            .w_full()
            .py(px(2.))
            .text_size(px(14.))
            .text_color(theme.foreground)
            .children(render_inlines(inlines, cx, link_id_counter))
            .into_any_element(),
        MdNode::List {
            ordered,
            start,
            items,
        } => {
            let mut children = Vec::new();
            for (i, blocks) in items.iter().enumerate() {
                let bullet = if *ordered {
                    format!("{}. ", (*start as usize) + i)
                } else {
                    "• ".to_string()
                };
                children.push(
                    h_flex()
                        .w_full()
                        .gap(px(8.))
                        .items_start()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(14.))
                                .text_color(theme.muted_foreground)
                                .child(bullet),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .children(
                                    blocks
                                        .iter()
                                        .map(|b| render_node(b, cx, depth + 1, link_id_counter)),
                                ),
                        )
                        .into_any_element(),
                );
            }
            v_flex()
                .w_full()
                .gap(px(4.))
                // Indent nested lists by an extra 16px per depth level.
                .ml(if depth == 0 { px(0.) } else { px(16.) })
                .pl(px(16.))
                .children(children)
                .into_any_element()
        }
        MdNode::CodeBlock { lang: _, code } => div()
            .w_full()
            .my(px(8.))
            .rounded_md()
            .bg(theme.muted)
            .overflow_x_scrollbar()
            .child(
                div()
                    .w_full()
                    .p_3()
                    .text_size(px(13.))
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .whitespace_nowrap()
                    .child(code.clone()),
            )
            .into_any_element(),
        MdNode::Blockquote(blocks) => div()
            .w_full()
            .my(px(4.))
            .pl_3()
            .border_l_2()
            .border_color(theme.border)
            .child(
                v_flex()
                    .w_full()
                    .gap(px(4.))
                    .text_size(px(14.))
                    .children(
                        blocks
                            .iter()
                            .map(|b| render_node(b, cx, depth, link_id_counter)),
                    ),
            )
            .text_color(theme.muted_foreground)
            .into_any_element(),
        MdNode::Rule => div()
            .w_full()
            .h(px(1.))
            .my_2()
            .bg(theme.border)
            .into_any_element(),
        MdNode::Table {
            header,
            rows,
            alignments,
        } => {
            let mut children = Vec::new();

            /// Apply alignment tailwind-style methods to a div based on pulldown-cmark Alignment.
            fn apply_align<D: Styled>(mut el: D, align: Alignment) -> D {
                match align {
                    Alignment::Left | Alignment::None => el = el.justify_start().text_left(),
                    Alignment::Center => el = el.justify_center().text_center(),
                    Alignment::Right => el = el.justify_end().text_right(),
                }
                el
            }

            // Header row.
            let header_cells: Vec<AnyElement> = header
                .iter()
                .enumerate()
                .map(|(col_idx, cell)| {
                    let align = alignments.get(col_idx).copied().unwrap_or(Alignment::None);
                    apply_align(
                        div()
                            .flex_1()
                            .min_w(px(80.))
                            .px_2()
                            .py_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .border_b_1()
                            .border_color(theme.border)
                            .flex(),
                        align,
                    )
                    .children(render_inlines(cell, cx, link_id_counter))
                    .into_any_element()
                })
                .collect();
            children.push(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .children(header_cells)
                    .into_any_element(),
            );
            // Data rows.
            for row in rows {
                let cells: Vec<AnyElement> = row
                    .iter()
                    .enumerate()
                    .map(|(col_idx, cell)| {
                        let align = alignments.get(col_idx).copied().unwrap_or(Alignment::None);
                        apply_align(
                            div()
                                .flex_1()
                                .min_w(px(80.))
                                .px_2()
                                .py_1()
                                .flex(),
                            align,
                        )
                        .children(render_inlines(cell, cx, link_id_counter))
                        .into_any_element()
                    })
                    .collect();
                children.push(
                    h_flex()
                        .w_full()
                        .gap(px(8.))
                        .children(cells)
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .my(px(8.))
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .overflow_x_scrollbar()
                .child(
                    v_flex()
                        .w_full()
                        .p(px(8.))
                        .children(children),
                )
                .into_any_element()
        }
    }
}

fn render_inlines(
    inlines: &[MdInline],
    cx: &App,
    link_id_counter: &Cell<usize>,
) -> Vec<AnyElement> {
    let theme = cx.theme();
    let mut out = Vec::new();
    for inline in inlines {
        match inline {
            MdInline::Text(t) => out.push(
                // No text_size / text_color here — inherit from the parent (heading, paragraph, etc.)
                div().child(t.clone()).into_any_element(),
            ),
            MdInline::Bold(inner) => out.push(
                div()
                    .font_weight(FontWeight::BOLD)
                    .children(render_inlines(inner, cx, link_id_counter))
                    .into_any_element(),
            ),
            MdInline::Italic(inner) => out.push(
                div()
                    .italic()
                    .children(render_inlines(inner, cx, link_id_counter))
                    .into_any_element(),
            ),
            MdInline::Strikethrough(inner) => out.push(
                div()
                    .line_through()
                    .children(render_inlines(inner, cx, link_id_counter))
                    .into_any_element(),
            ),
            MdInline::Code(code) => out.push(
                div()
                    .px(px(4.))
                    .py(px(1.))
                    .mx(px(2.))
                    .rounded_sm()
                    .bg(theme.muted)
                    .text_size(px(13.))
                    .font_family(theme.mono_font_family.clone())
                    .child(code.clone())
                    .into_any_element(),
            ),
            MdInline::Link { text, url } => {
                let url_clone = url.clone();
                let id = link_id_counter.get();
                link_id_counter.set(id + 1);
                out.push(
                    Button::new(format!("link-{}", id))
                        .ghost()
                        .small()
                        .label(text.clone())
                        .on_click(move |_ev, _window, cx| {
                            cx.open_url(&url_clone);
                        })
                        .into_any_element(),
                );
            }
            MdInline::Image { alt, url } => {
                // If the URL is a data URI or has a recognized image extension, render with img().
                // Fall back to showing alt text for unsupported/empty URLs.
                let url_lower = url.to_lowercase();
                let is_image_like = url_lower.starts_with("http://")
                    || url_lower.starts_with("https://")
                    || url_lower.starts_with("data:image/")
                    || url_lower.starts_with("file://")
                    || url_lower.ends_with(".png")
                    || url_lower.ends_with(".jpg")
                    || url_lower.ends_with(".jpeg")
                    || url_lower.ends_with(".gif")
                    || url_lower.ends_with(".svg")
                    || url_lower.ends_with(".webp");
                if is_image_like {
                    out.push(
                        div()
                            .py(px(4.))
                            .w_full()
                            .child(
                                img(url.clone())
                                    .max_h(px(400.))
                                    .object_fit(ObjectFit::Contain)
                                    .rounded_md(),
                            )
                            .into_any_element(),
                    );
                } else {
                    out.push(
                        div()
                            .px(px(4.))
                            .py(px(4.))
                            .my(px(4.))
                            .rounded_sm()
                            .bg(theme.muted)
                            .text_color(theme.muted_foreground)
                            .text_size(px(12.))
                            .child(format!("[Image: {} ({})]", alt, url))
                            .into_any_element(),
                    );
                }
            }
            MdInline::HardBreak => {
                // A HardBreak forces a line break; emit a newline text node.
                out.push(div().child("\n").into_any_element());
            }
            MdInline::TaskMarker(checked) => {
                let check_char = if *checked { "✓" } else { "" };
                let bg = if *checked {
                    theme.accent
                } else {
                    theme.background
                };
                let border = if *checked {
                    theme.accent
                } else {
                    theme.border
                };
                out.push(
                    div()
                        .w(px(16.))
                        .h(px(16.))
                        .mr(px(6.))
                        .mt(px(1.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(border)
                        .bg(bg)
                        .text_size(px(11.))
                        .text_color(if *checked {
                            gpui::Hsla::from(gpui::rgb(0xFFFFFF))
                        } else {
                            theme.muted_foreground
                        })
                        .flex_shrink_0()
                        .child(check_char)
                        .into_any_element(),
                );
            }
        }
    }
    out
}
