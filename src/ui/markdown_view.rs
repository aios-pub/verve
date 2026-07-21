//! Reusable Markdown renderer — converts parsed MdNode tree into GPUI elements.
//!
//! Theme-aware: colors and fonts come from the active GPUI theme.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{h_flex, v_flex, ActiveTheme, Sizable as _};

use crate::markdown_render::{MdInline, MdNode};

/// Render markdown source into a GPUI element.
pub fn render_markdown(md: &str, cx: &App) -> AnyElement {
    let nodes = crate::markdown_render::parse_markdown(md);
    render_nodes(&nodes, cx)
}

/// Render a slice of MdNode blocks into a GPUI element.
pub fn render_nodes(nodes: &[MdNode], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut children = Vec::new();
    for node in nodes {
        children.push(render_node(node, cx));
    }
    v_flex()
        .w_full()
        .gap(px(4.))
        .text_size(px(14.))
        .text_color(theme.foreground)
        .children(children)
        .into_any_element()
}

fn render_node(node: &MdNode, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match node {
        MdNode::Heading(level, inlines) => {
            let (size, weight, margin) = match level {
                1 => (px(28.), FontWeight::BOLD, px(16.)),
                2 => (px(24.), FontWeight::BOLD, px(12.)),
                3 => (px(20.), FontWeight::SEMIBOLD, px(10.)),
                4 => (px(17.), FontWeight::SEMIBOLD, px(8.)),
                5 => (px(15.), FontWeight::SEMIBOLD, px(6.)),
                _ => (px(14.), FontWeight::SEMIBOLD, px(4.)),
            };
            div()
                .w_full()
                .mt(margin)
                .mb(px(4.))
                .text_size(size)
                .font_weight(weight)
                .text_color(theme.foreground)
                .children(render_inlines(inlines, cx))
                .into_any_element()
        }
        MdNode::Paragraph(inlines) => div()
            .w_full()
            .py(px(2.))
            .children(render_inlines(inlines, cx))
            .into_any_element(),
        MdNode::List(ordered, items) => {
            let mut children = Vec::new();
            for (i, blocks) in items.iter().enumerate() {
                let bullet = if *ordered {
                    format!("{}. ", i + 1)
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
                                .children(blocks.iter().map(|b| render_node(b, cx))),
                        )
                        .into_any_element(),
                );
            }
            v_flex()
                .w_full()
                .gap(px(4.))
                .pl(px(16.))
                .children(children)
                .into_any_element()
        }
        MdNode::CodeBlock { lang: _, code } => div()
            .w_full()
            .my(px(8.))
            .p_3()
            .rounded_md()
            .bg(theme.muted)
            .child(
                div()
                    .w_full()
                    .text_size(px(13.))
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(code.clone()),
            )
            .into_any_element(),
        MdNode::Blockquote(blocks) => div()
            .w_full()
            .my(px(4.))
            .pl_3()
            .border_l_2()
            .border_color(theme.border)
            .child(render_nodes(blocks, cx))
            .text_color(theme.muted_foreground)
            .into_any_element(),
        MdNode::Rule => div()
            .w_full()
            .h(px(1.))
            .my_2()
            .bg(theme.border)
            .into_any_element(),
        MdNode::Table { header, rows } => {
            let mut children = Vec::new();
            // Header row.
            let header_cells: Vec<AnyElement> = header
                .iter()
                .map(|cell| {
                    div()
                        .flex_1()
                        .min_w(px(80.))
                        .px_2()
                        .py_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .border_b_1()
                        .border_color(theme.border)
                        .children(render_inlines(cell, cx))
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
                    .map(|cell| {
                        div()
                            .flex_1()
                            .min_w(px(80.))
                            .px_2()
                            .py_1()
                            .children(render_inlines(cell, cx))
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
            v_flex()
                .w_full()
                .my(px(8.))
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .p(px(8.))
                .children(children)
                .into_any_element()
        }
    }
}

fn render_inlines(inlines: &[MdInline], cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let mut out = Vec::new();
    for inline in inlines {
        match inline {
            MdInline::Text(t) => out.push(
                div()
                    .text_size(px(14.))
                    .text_color(theme.foreground)
                    .child(t.clone())
                    .into_any_element(),
            ),
            MdInline::Bold(inner) => out.push(
                div()
                    .font_weight(FontWeight::BOLD)
                    .children(render_inlines(inner, cx))
                    .into_any_element(),
            ),
            MdInline::Italic(inner) => out.push(
                // Use emphasis via color/weight variation since font_style may not be available.
                div()
                    .text_color(theme.muted_foreground)
                    .children(render_inlines(inner, cx))
                    .into_any_element(),
            ),
            MdInline::Strikethrough(inner) => out.push(
                div()
                    .text_color(theme.muted_foreground)
                    .opacity(0.7)
                    .children(render_inlines(inner, cx))
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
                out.push(
                    Button::new(format!("link-{}", url))
                        .ghost()
                        .small()
                        .label(text.clone())
                        .on_click(move |_ev, _window, cx| {
                            cx.open_url(&url_clone);
                        })
                        .into_any_element(),
                );
            }
            MdInline::Image { alt, url } => out.push(
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
            ),
        }
    }
    out
}
