//! Native-safe HTML classification for Markdown raw HTML blocks.
//!
//! The parser keeps the original source as the serialization truth and builds
//! a conservative semantic tree only for tags that can be rendered safely in
//! GPUI. Anything risky, unknown, malformed, or ambiguous becomes raw text.
//!
//! Parsing is done by `html5ever` (servo's spec-compliant HTML5 tokenizer +
//! tree builder). It correctly handles the Markdown-text edge cases that a
//! hand-written tokenizer cannot — e.g. a `<` followed by a digit (`<1 秒`,
//! `<100MB`) is text per the HTML5 spec (tag names must start with an ASCII
//! letter), and a stray `>` from a Markdown blockquote is just a character.

use std::ops::Range;

use cssparser::color::{parse_hash_color, parse_named_color};
use html5ever::driver::{parse_fragment, ParseOpts};
use html5ever::tendril::TendrilSink;
use html5ever::{local_name, namespace_url, ns, QualName};
use markup5ever_rcdom::{NodeData, RcDom};
use std::rc::Rc;

/// Safety classification for an HTML fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HtmlSafetyClass {
    /// The fragment has at least one safe semantic node.
    Semantic,
    /// The entire fragment must be shown and stored as plain raw text.
    RawTextBlock,
}

/// Broad rendering category of a parsed HTML node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HtmlNodeKind {
    /// Safe inline tag or text that can be represented with text runs.
    InlineSemantic,
    /// Safe block tag that maps to a native block-like GPUI element.
    BlockSemantic,
    /// Opaque raw source that must not be interpreted as HTML.
    RawTextBlock,
}

/// One source attribute from an HTML tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HtmlAttr {
    /// Lowercase attribute name used for safety checks.
    pub(crate) name: String,
    /// Parsed attribute value without surrounding quotes.
    pub(crate) value: Option<String>,
    /// Exact attribute source text.
    pub(crate) raw_source: String,
}

/// Parsed CSS color value from a safe inline `style` attribute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HtmlCssColor {
    /// The CSS `currentColor` keyword.
    CurrentColor,
    /// An sRGB color with alpha.
    Rgba(HtmlCssRgba),
}

/// RGBA channels normalized enough for both GPUI rendering and export CSS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HtmlCssRgba {
    pub(crate) red: u8,
    pub(crate) green: u8,
    pub(crate) blue: u8,
    pub(crate) alpha: f32,
}

/// Parsed CSS font-size value from a safe inline `style` attribute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HtmlCssFontSize {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Keyword(HtmlCssFontSizeKeyword),
}

/// CSS absolute and relative font-size keywords supported by rendered HTML.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HtmlCssFontSizeKeyword {
    XxSmall,
    XSmall,
    Small,
    Medium,
    Large,
    XLarge,
    XxLarge,
    Smaller,
    Larger,
}

/// Whitelisted visual CSS parsed from a safe HTML `style` attribute.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HtmlInlineStyle {
    pub(crate) color: Option<HtmlCssColor>,
    pub(crate) background_color: Option<HtmlCssColor>,
    pub(crate) font_size: Option<HtmlCssFontSize>,
    pub(crate) text_align: Option<HtmlTextAlign>,
    pub(crate) padding: Option<HtmlBoxEdges>,
    pub(crate) margin: Option<HtmlBoxEdges>,
    pub(crate) width: Option<HtmlLength>,
    pub(crate) max_width: Option<HtmlLength>,
    pub(crate) border_width: Option<HtmlBoxEdges>,
    pub(crate) border_color: Option<HtmlCssColor>,
    pub(crate) border_radius: Option<HtmlLength>,
}

impl Eq for HtmlInlineStyle {}

/// CSS text-align keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum HtmlTextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Four-sided box edge widths (px). Used for padding / margin / border-width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct HtmlBoxEdges {
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
    pub(crate) left: f32,
}

impl HtmlBoxEdges {
    /// Uniform edges from a single value.
    fn uniform(v: f32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }
}

/// A CSS length with a unit we can resolve against a parent font-size / width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HtmlLength {
    Px(f32),
    Em(f32),
    Percent(f32),
}

/// Safe data extracted from a standalone HTML `<img>` block.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HtmlImageBlock {
    pub(crate) src: String,
    pub(crate) alt: String,
    pub(crate) zoom: f32,
}

impl HtmlImageBlock {
    pub(crate) fn zoom_factor(&self) -> f32 {
        self.zoom.clamp(0.1, 3.0)
    }

    pub(crate) fn to_sanitized_html_with_src(&self, src: &str) -> String {
        let mut html = format!("<img src=\"{}\"", escape_html_attr(src));
        if !self.alt.is_empty() {
            html.push_str(" alt=\"");
            html.push_str(&escape_html_attr(&self.alt));
            html.push('"');
        }
        if (self.zoom_factor() - 1.0).abs() > f32::EPSILON {
            html.push_str(" style=\"zoom: ");
            html.push_str(&css_number(self.zoom_factor() * 100.0));
            html.push_str("%;\"");
        }
        html.push('>');
        html
    }
}

/// A classified HTML node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HtmlNode {
    /// Rendering category selected by the safety policy.
    pub(crate) kind: HtmlNodeKind,
    /// Lowercase tag name, or `#text` for text nodes.
    pub(crate) tag_name: String,
    /// Safe attributes retained as semantic data.
    pub(crate) attrs: Vec<HtmlAttr>,
    /// Classified child nodes. Empty for raw text nodes.
    pub(crate) children: Vec<HtmlNode>,
    /// Exact source text covered by this node.
    pub(crate) raw_source: String,
    /// Byte range in the original HTML fragment.
    pub(crate) source_range: Range<usize>,
}

/// Classified HTML fragment plus its preserved source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlDocument {
    /// Exact source string used for serialization and raw editing.
    pub(crate) raw_source: String,
    /// Root-level classified nodes.
    pub(crate) nodes: Vec<HtmlNode>,
    /// Overall fragment safety.
    pub(crate) safety: HtmlSafetyClass,
}

impl HtmlDocument {
    pub(crate) fn raw(raw_source: impl Into<String>) -> Self {
        let raw_source = raw_source.into();
        let len = raw_source.len();
        Self {
            nodes: vec![HtmlNode {
                kind: HtmlNodeKind::RawTextBlock,
                tag_name: "#raw".into(),
                attrs: Vec::new(),
                children: Vec::new(),
                raw_source: raw_source.clone(),
                source_range: 0..len,
            }],
            safety: HtmlSafetyClass::RawTextBlock,
            raw_source,
        }
    }

    pub(crate) fn is_semantic(&self) -> bool {
        self.safety == HtmlSafetyClass::Semantic
    }
}

impl HtmlCssColor {
    pub(crate) fn to_css(self) -> String {
        match self {
            Self::CurrentColor => "currentColor".to_string(),
            Self::Rgba(color) => format!(
                "rgba({},{},{},{:.3})",
                color.red,
                color.green,
                color.blue,
                color.alpha.clamp(0.0, 1.0)
            ),
        }
    }
}

impl HtmlCssFontSize {
    pub(crate) fn resolve(self, parent_px: f32, root_px: f32) -> f32 {
        let resolved = match self {
            Self::Px(value) => value,
            Self::Em(value) => parent_px * value,
            Self::Rem(value) => root_px * value,
            Self::Percent(value) => parent_px * value / 100.0,
            Self::Keyword(keyword) => match keyword {
                HtmlCssFontSizeKeyword::XxSmall => root_px * 0.6,
                HtmlCssFontSizeKeyword::XSmall => root_px * 0.75,
                HtmlCssFontSizeKeyword::Small => root_px * 0.875,
                HtmlCssFontSizeKeyword::Medium => root_px,
                HtmlCssFontSizeKeyword::Large => root_px * 1.125,
                HtmlCssFontSizeKeyword::XLarge => root_px * 1.5,
                HtmlCssFontSizeKeyword::XxLarge => root_px * 2.0,
                HtmlCssFontSizeKeyword::Smaller => parent_px * 0.833,
                HtmlCssFontSizeKeyword::Larger => parent_px * 1.2,
            },
        };

        if resolved.is_finite() {
            resolved.clamp(6.0, 96.0)
        } else {
            parent_px
        }
    }

    pub(crate) fn to_css(self) -> String {
        match self {
            Self::Px(value) => format!("{}px", css_number(value)),
            Self::Em(value) => format!("{}em", css_number(value)),
            Self::Rem(value) => format!("{}rem", css_number(value)),
            Self::Percent(value) => format!("{}%", css_number(value)),
            Self::Keyword(keyword) => match keyword {
                HtmlCssFontSizeKeyword::XxSmall => "xx-small",
                HtmlCssFontSizeKeyword::XSmall => "x-small",
                HtmlCssFontSizeKeyword::Small => "small",
                HtmlCssFontSizeKeyword::Medium => "medium",
                HtmlCssFontSizeKeyword::Large => "large",
                HtmlCssFontSizeKeyword::XLarge => "x-large",
                HtmlCssFontSizeKeyword::XxLarge => "xx-large",
                HtmlCssFontSizeKeyword::Smaller => "smaller",
                HtmlCssFontSizeKeyword::Larger => "larger",
            }
            .to_string(),
        }
    }
}

impl HtmlInlineStyle {
    pub(crate) fn is_empty(&self) -> bool {
        self.color.is_none() && self.background_color.is_none() && self.font_size.is_none()
    }

    pub(crate) fn to_css(self) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        let mut declarations = Vec::new();
        if let Some(color) = self.color {
            declarations.push(format!("color: {}", color.to_css()));
        }
        if let Some(color) = self.background_color {
            declarations.push(format!("background-color: {}", color.to_css()));
        }
        if let Some(font_size) = self.font_size {
            declarations.push(format!("font-size: {}", font_size.to_css()));
        }
        Some(format!("{};", declarations.join("; ")))
    }
}

/// Parses and classifies a raw HTML fragment using the spec-compliant html5ever
/// parser. The returned document always preserves `raw_source` exactly (as the
/// document's `raw_source`); per-node `raw_source` is reconstructed from the
/// parsed structure (see `dom_node_to_html_node`).
pub(crate) fn parse_html_document(raw_source: &str) -> HtmlDocument {
    if raw_source.trim().is_empty() {
        return HtmlDocument::raw(raw_source);
    }

    // Parse as a fragment with a <div> context. Fragment mode does not inject
    // <html>/<head>/<body>, so the children of the context are exactly the
    // fragment's top-level nodes.
    let context = QualName::new(None, ns!(html), local_name!("div"));
    let dom = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        context,
        Vec::new(),
    )
    .one(raw_source.to_string());

    if !dom.errors.is_empty() {
        log::debug!(
            "html5ever reported {} parse error(s) on fragment; continuing (lenient)",
            dom.errors.len()
        );
    }

    // html5ever wraps a fragment in <html><head></head><body>...</body></html>.
    // Walk down to the <body> (or <html>) and treat ITS children as the
    // fragment's top-level nodes.
    let top_level = fragment_top_level_children(&dom);

    let nodes: Vec<HtmlNode> = top_level
        .iter()
        .filter_map(dom_node_to_html_node)
        .collect();

    // No usable nodes → treat the whole fragment as raw text (preserves the
    // exact source for display/export).
    if nodes.is_empty() {
        return HtmlDocument::raw(raw_source);
    }

    // If every top-level node is raw text (no semantic element at all), keep
    // the document as raw text so it renders as a single source block rather
    // than a pile of raw-text children.
    let any_semantic = nodes.iter().any(|n| {
        !matches!(n.kind, HtmlNodeKind::RawTextBlock) && n.tag_name != "#text"
    });
    if !any_semantic {
        return HtmlDocument::raw(raw_source);
    }

    HtmlDocument {
        raw_source: raw_source.to_string(),
        nodes,
        safety: HtmlSafetyClass::Semantic,
    }
}

/// Returns the RcDom nodes that represent the fragment's top-level content.
///
/// html5ever's `parse_fragment` (in the 0.27 line) still wraps the input in a
/// synthetic `<html><head></head><body>…</body></html>` document, so the real
/// fragment nodes are the children of `<body>` (falling back to the document's
/// children if no wrapper is present).
fn fragment_top_level_children(dom: &RcDom) -> Vec<Rc<markup5ever_rcdom::Node>> {
    // document → html. Clone the Rc<Node> out of the borrowed Ref so it lives
    // long enough to inspect further.
    let html = dom
        .document
        .children
        .borrow()
        .iter()
        .find(|child| {
            matches!(&child.data,
                NodeData::Element { name, .. } if name.local.as_ref() == "html")
        })
        .cloned();
    let Some(html) = html else {
        return dom.document.children.borrow().clone();
    };
    // html → body
    let body = html
        .children
        .borrow()
        .iter()
        .find(|child| {
            matches!(&child.data,
                NodeData::Element { name, .. } if name.local.as_ref() == "body")
        })
        .cloned();
    match body {
        Some(body) => body.children.borrow().clone(),
        None => html.children.borrow().clone(),
    }
}

/// Maps an html5ever `RcDom` node into our `HtmlNode`.
///
/// - `Element`: a tag. Classified `InlineSemantic`/`BlockSemantic` by the tag
///   whitelist; unsafe tags (script/style/…) or tags with dangerous attrs
///   (on*/javascript:) become `RawTextBlock` carrying the element's source.
/// - `Text`: a `#text` node whose `raw_source` is the exact text content.
/// - `Comment`: a `#comment` raw node.
/// - `Document`/`Doctype`/`ProcessingInstruction`: skipped (fragment mode
///   doesn't emit these at the top level anyway).
fn dom_node_to_html_node(node: &Rc<markup5ever_rcdom::Node>) -> Option<HtmlNode> {
    Some(match &node.data {
        NodeData::Text { contents } => {
            let text = contents.borrow().to_string();
            // Skip empty text nodes (whitespace-only between tags is kept so
            // layout doesn't collapse, but truly empty strings are dropped).
            if text.is_empty() {
                return None;
            }
            HtmlNode {
                kind: HtmlNodeKind::InlineSemantic,
                tag_name: "#text".into(),
                attrs: Vec::new(),
                children: Vec::new(),
                raw_source: text,
                source_range: 0..0,
            }
        }
        NodeData::Comment { contents } => {
            let raw = format!("<!--{}-->", contents);
            HtmlNode {
                kind: HtmlNodeKind::RawTextBlock,
                tag_name: "#comment".into(),
                attrs: Vec::new(),
                children: Vec::new(),
                raw_source: raw,
                source_range: 0..0,
            }
        }
        NodeData::Element {
            name,
            attrs,
            ..
        } => {
            let tag_name = name.local.as_ref().to_string();
            let html_attrs: Vec<HtmlAttr> = attrs
                .borrow()
                .iter()
                .map(|attr| HtmlAttr {
                    name: attr.name.local.as_ref().to_string(),
                    value: Some(attr.value.to_string()),
                    raw_source: serialize_attr(attr.name.local.as_ref(), &attr.value),
                })
                .collect();

            // Unsafe tag or dangerous attribute → raw text node carrying the
            // element's (re-serialized) source so it shows as code, not as a
            // live element.
            if !is_safe_tag(&tag_name) || has_dangerous_attrs(&html_attrs) {
                let raw = serialize_element(&tag_name, &html_attrs, node);
                return Some(HtmlNode {
                    kind: HtmlNodeKind::RawTextBlock,
                    tag_name: "#raw".into(),
                    attrs: Vec::new(),
                    children: Vec::new(),
                    raw_source: raw,
                    source_range: 0..0,
                });
            }

            let kind = if is_inline_tag(&tag_name) {
                HtmlNodeKind::InlineSemantic
            } else {
                HtmlNodeKind::BlockSemantic
            };

            let children: Vec<HtmlNode> = node
                .children
                .borrow()
                .iter()
                .filter_map(dom_node_to_html_node)
                .collect();

            let raw_source = if is_void_tag(&tag_name) {
                serialize_open_tag(&tag_name, &html_attrs, true)
            } else {
                let inner: String = children
                    .iter()
                    .map(node_raw_source)
                    .collect::<Vec<_>>()
                    .join("");
                format!(
                    "{}{inner}{}",
                    serialize_open_tag(&tag_name, &html_attrs, false),
                    serialize_close_tag(&tag_name),
                )
            };

            HtmlNode {
                kind,
                tag_name,
                attrs: html_attrs,
                children,
                raw_source,
                source_range: 0..0,
            }
        }
        // Document / Doctype / ProcessingInstruction — not part of a fragment.
        _ => return None,
    })
}

/// Returns the `raw_source` of a node (text content for `#text`, re-serialized
/// source for elements). Used when reconstructing a parent's `raw_source`.
fn node_raw_source(node: &HtmlNode) -> &str {
    &node.raw_source
}

fn serialize_attr(name: &str, value: &str) -> String {
    if value.is_empty() {
        name.to_string()
    } else {
        format!("{name}=\"{}\"", escape_html_attr(value))
    }
}

fn serialize_open_tag(tag: &str, attrs: &[HtmlAttr], self_closing: bool) -> String {
    let mut s = format!("<{tag}");
    for attr in attrs {
        s.push(' ');
        s.push_str(&attr.raw_source);
    }
    if self_closing {
        s.push_str(" />");
    } else {
        s.push('>');
    }
    s
}

fn serialize_close_tag(tag: &str) -> String {
    format!("</{tag}>")
}

/// Re-serializes an element's full source (open tag + children + close tag),
/// used for unsafe/raw-text nodes where we want to show the original markup.
fn serialize_element(tag: &str, attrs: &[HtmlAttr], node: &Rc<markup5ever_rcdom::Node>) -> String {
    let open = serialize_open_tag(tag, attrs, false);
    let inner: String = node
        .children
        .borrow()
        .iter()
        .filter_map(dom_node_to_html_node)
        .map(|n| n.raw_source)
        .collect::<Vec<_>>()
        .join("");
    // If the tag is void (br/hr/img) there's no close tag.
    if is_void_tag(tag) {
        serialize_open_tag(tag, attrs, true)
    } else {
        format!("{open}{inner}{}", serialize_close_tag(tag))
    }
}

/// Rewrites an HTML fragment for document export: safe semantic nodes keep
/// their HTML shape, while raw text nodes are escaped so browsers cannot
/// execute or interpret them.
pub(crate) fn sanitize_html_for_export(raw_source: &str) -> String {
    if let Some(image) = parse_html_image_block(raw_source) {
        return image.to_sanitized_html_with_src(&image.src);
    }

    let document = parse_html_document(raw_source);
    if !document.is_semantic() {
        return format!(
            "<pre class=\"vlt-raw-html\">{}</pre>",
            escape_html(raw_source)
        );
    }

    document
        .nodes
        .iter()
        .map(sanitize_node_for_export)
        .collect::<String>()
}

/// Parses the safe visual subset of a semantic node's `style` attribute.
pub(crate) fn style_for_node(node: &HtmlNode) -> HtmlInlineStyle {
    if node.kind == HtmlNodeKind::RawTextBlock {
        return HtmlInlineStyle::default();
    }

    // The `style` attribute is authoritative; if absent the node has no CSS
    // box model. The legacy `align` attribute (e.g. `<div align="center">`)
    // only contributes text-align and only when `style` did not set one.
    let style_attr = attr_value(node, "style");
    let align_attr = attr_value(node, "align");

    let mut style = match style_attr {
        Some(css) => parse_inline_style(css),
        None => HtmlInlineStyle::default(),
    };

    if style.text_align.is_none() {
        if let Some(align) = align_attr {
            style.text_align = parse_text_align(align);
        }
    }

    // If neither style nor align produced anything, treat as default (empty).
    style
}

fn sanitize_node_for_export(node: &HtmlNode) -> String {
    if node.kind == HtmlNodeKind::RawTextBlock {
        return format!(
            "<span class=\"vlt-raw-html\">{}</span>",
            escape_html(&node.raw_source)
        );
    }

    if node.tag_name == "#text" {
        // html5ever decodes entities (e.g. `&lt;` → `<`), so the text node's
        // `raw_source` holds decoded text. Re-escape it for HTML output so the
        // round-trip through the export pipeline preserves the visible markup.
        return escape_html(&node.raw_source);
    }

    if is_void_tag(&node.tag_name) {
        return sanitized_open_tag(node);
    }

    let Some(_open_end) = node.raw_source.find('>').map(|index| index + 1) else {
        return escape_html(&node.raw_source);
    };
    let close_start =
        find_closing_tag_start(&node.raw_source, &node.tag_name).unwrap_or(node.raw_source.len());
    let close = &node.raw_source[close_start..];
    let children = node
        .children
        .iter()
        .map(sanitize_node_for_export)
        .collect::<String>();
    format!("{}{children}{close}", sanitized_open_tag(node))
}

fn sanitized_open_tag(node: &HtmlNode) -> String {
    if node.tag_name == "img"
        && let Some(image) = parse_html_image_block(&node.raw_source)
    {
        return image.to_sanitized_html_with_src(&image.src);
    }

    let mut open = format!("<{}", node.tag_name);
    for attr in &node.attrs {
        if attr.name == "style" {
            continue;
        }
        open.push(' ');
        open.push_str(&attr.raw_source);
    }
    if let Some(style) = style_for_node(node).to_css() {
        open.push_str(" style=\"");
        open.push_str(&escape_html_attr(&style));
        open.push('"');
    }
    open.push('>');
    open
}

fn find_closing_tag_start(raw_source: &str, tag_name: &str) -> Option<usize> {
    let needle = format!("</{tag_name}");
    raw_source.to_ascii_lowercase().rfind(&needle)
}

fn escape_html_attr(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Peek the next char at `index` without advancing. Returns `None` at EOF.
#[inline]
fn peek_char(source: &str, index: usize) -> Option<char> {
    source[index..].chars().next()
}

/// Advance `index` past the next char and return it. Returns `None` at EOF.
/// Encapsulates the byte-index ↔ UTF-8-boundary invariant so callers that
/// don't need the char's value can't drift into a panic by hand-incrementing
/// `index` by anything other than `ch.len_utf8()`. Loops that *do* need the
/// char for a check should peek with [`peek_char`], inspect the value, and
/// then advance with `index += ch.len_utf8()` — see [`parse_html_attrs`] —
/// so the char is read only once per iteration.
#[inline]
fn advance_char(source: &str, index: &mut usize) -> Option<char> {
    let ch = source[*index..].chars().next()?;
    *index += ch.len_utf8();
    Some(ch)
}

pub(crate) fn parse_html_attrs(source: &str) -> Vec<HtmlAttr> {
    let mut attrs = Vec::new();
    let mut index = 0usize;
    while index < source.len() {
        while let Some(ch) = peek_char(source, index).filter(|c| c.is_whitespace() || *c == '/') {
            index += ch.len_utf8();
        }
        if index >= source.len() {
            break;
        }

        let start = index;
        while let Some(ch) = peek_char(source, index) {
            if ch.is_whitespace() || ch == '=' || ch == '/' {
                break;
            }
            index += ch.len_utf8();
        }
        let name_end = index;
        if name_end == start {
            // Lone separator we couldn't classify — consume one char and retry.
            advance_char(source, &mut index);
            continue;
        }

        while let Some(ch) = peek_char(source, index).filter(|c| c.is_whitespace()) {
            index += ch.len_utf8();
        }

        let mut value = None;
        if source[index..].starts_with('=') {
            index += 1;
            while let Some(ch) = peek_char(source, index).filter(|c| c.is_whitespace()) {
                index += ch.len_utf8();
            }

            if let Some(quote) = peek_char(source, index).filter(|c| *c == '"' || *c == '\'') {
                index += quote.len_utf8();
                let value_start = index;
                while let Some(ch) = peek_char(source, index) {
                    if ch == quote {
                        break;
                    }
                    index += ch.len_utf8();
                }
                value = Some(source[value_start..index].to_string());
                if index < source.len() {
                    index += quote.len_utf8();
                }
            } else if peek_char(source, index).is_some() {
                let value_start = index;
                while let Some(ch) = peek_char(source, index) {
                    if ch.is_whitespace() || ch == '/' {
                        break;
                    }
                    index += ch.len_utf8();
                }
                value = Some(source[value_start..index].to_string());
            }
        }

        attrs.push(HtmlAttr {
            name: source[start..name_end].to_ascii_lowercase(),
            value,
            raw_source: source[start..index].to_string(),
        });
    }

    attrs
}

pub(crate) fn has_dangerous_attrs(attrs: &[HtmlAttr]) -> bool {
    attrs.iter().any(|attr| {
        attr.name.starts_with("on")
            || attr.value.as_deref().is_some_and(|value| {
                let normalized = value
                    .chars()
                    .filter(|ch| !ch.is_whitespace() && *ch != '\0')
                    .collect::<String>()
                    .to_ascii_lowercase();
                matches!(
                    attr.name.as_str(),
                    "href" | "src" | "action" | "formaction" | "xlink:href"
                ) && normalized.starts_with("javascript:")
            })
    })
}

pub(crate) fn attr_value<'a>(node: &'a HtmlNode, name: &str) -> Option<&'a str> {
    node.attrs
        .iter()
        .find(|attr| attr.name == name)
        .and_then(|attr| attr.value.as_deref())
}

pub(crate) fn parse_html_image_block(raw_source: &str) -> Option<HtmlImageBlock> {
    let trimmed = raw_source.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Parse the fragment and look for a single top-level <img> element. Using
    // html5ever (instead of the old hand-written tag tokenizer) keeps this
    // robust against attribute quoting/ordering variations.
    let context = QualName::new(None, ns!(html), local_name!("div"));
    let dom = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        context,
        Vec::new(),
    )
    .one(trimmed.to_string());

    // The fragment's real top-level nodes live under the <body> wrapper that
    // html5ever synthesizes (see `fragment_top_level_children`).
    let top_level = fragment_top_level_children(&dom);

    // Find the first <img> among them (skipping whitespace text nodes).
    let img_node = top_level.iter().find(|child| {
        matches!(
            &child.data,
            NodeData::Element { name, .. } if name.local.as_ref() == "img"
        )
    })?;

    let attrs = match &img_node.data {
        NodeData::Element { attrs, .. } => attrs.borrow().clone(),
        _ => return None,
    };
    let html_attrs: Vec<HtmlAttr> = attrs
        .iter()
        .map(|attr| HtmlAttr {
            name: attr.name.local.as_ref().to_string(),
            value: Some(attr.value.to_string()),
            raw_source: serialize_attr(attr.name.local.as_ref(), &attr.value),
        })
        .collect();

    if has_dangerous_attrs(&html_attrs) {
        return None;
    }

    let src = attr_value_in_attrs(&html_attrs, "src")?.trim().to_string();
    if src.is_empty() {
        return None;
    }

    let alt = attr_value_in_attrs(&html_attrs, "alt")
        .unwrap_or_default()
        .to_string();
    let zoom = attr_value_in_attrs(&html_attrs, "style")
        .and_then(parse_html_zoom)
        .unwrap_or(1.0);

    Some(HtmlImageBlock { src, alt, zoom })
}

fn attr_value_in_attrs<'a>(attrs: &'a [HtmlAttr], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.name == name)
        .and_then(|attr| attr.value.as_deref())
}

pub(crate) fn parse_html_zoom(style: &str) -> Option<f32> {
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        if !property.trim().eq_ignore_ascii_case("zoom") {
            continue;
        }

        let value = value.trim();
        let parsed = if let Some(percent) = value.strip_suffix('%') {
            parse_css_number(percent)? / 100.0
        } else {
            parse_css_number(value)?
        };
        return Some(parsed.clamp(0.1, 3.0));
    }
    None
}

pub(crate) fn parse_inline_style(style: &str) -> HtmlInlineStyle {
    let mut parsed = HtmlInlineStyle::default();
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim();
        match property.as_str() {
            "color" => {
                if let Some(color) = parse_css_color(value) {
                    parsed.color = Some(color);
                }
            }
            "background-color" | "background" => {
                if let Some(color) = parse_css_color(value) {
                    parsed.background_color = Some(color);
                }
            }
            "font-size" => {
                if let Some(size) = parse_css_font_size(value) {
                    parsed.font_size = Some(size);
                }
            }
            "text-align" => {
                if let Some(align) = parse_text_align(value) {
                    parsed.text_align = Some(align);
                }
            }
            "padding" => {
                if let Some(edges) = parse_box_edges(value) {
                    parsed.padding = Some(edges);
                }
            }
            "padding-top" => set_box_edge(&mut parsed.padding, BoxSide::Top, value),
            "padding-right" => set_box_edge(&mut parsed.padding, BoxSide::Right, value),
            "padding-bottom" => set_box_edge(&mut parsed.padding, BoxSide::Bottom, value),
            "padding-left" => set_box_edge(&mut parsed.padding, BoxSide::Left, value),
            "margin" => {
                if let Some(edges) = parse_box_edges(value) {
                    parsed.margin = Some(edges);
                }
            }
            "margin-top" => set_box_edge(&mut parsed.margin, BoxSide::Top, value),
            "margin-right" => set_box_edge(&mut parsed.margin, BoxSide::Right, value),
            "margin-bottom" => set_box_edge(&mut parsed.margin, BoxSide::Bottom, value),
            "margin-left" => set_box_edge(&mut parsed.margin, BoxSide::Left, value),
            "width" => parsed.width = parse_length(value),
            "max-width" => parsed.max_width = parse_length(value),
            "border" | "border-width" => {
                if let Some(edges) = parse_border_width_shorthand(value) {
                    parsed.border_width = Some(edges);
                }
            }
            "border-color" => {
                if let Some(color) = parse_css_color(value) {
                    parsed.border_color = Some(color);
                }
            }
            "border-radius" => parsed.border_radius = parse_length(value),
            _ => {}
        }
    }
    parsed
}

fn parse_text_align(value: &str) -> Option<HtmlTextAlign> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(HtmlTextAlign::Left),
        "center" => Some(HtmlTextAlign::Center),
        "right" => Some(HtmlTextAlign::Right),
        "justify" => Some(HtmlTextAlign::Justify),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum BoxSide {
    Top,
    Right,
    Bottom,
    Left,
}

/// Sets a single side on an `Option<HtmlBoxEdges>` (creating a zeroed struct on
/// first use) for the `padding-top` / `margin-left` / ... individual props.
fn set_box_edge(slot: &mut Option<HtmlBoxEdges>, side: BoxSide, value: &str) {
    let v = match parse_px(value) {
        Some(v) => v,
        None => return,
    };
    let edges = slot.get_or_insert(HtmlBoxEdges::default());
    match side {
        BoxSide::Top => edges.top = v,
        BoxSide::Right => edges.right = v,
        BoxSide::Bottom => edges.bottom = v,
        BoxSide::Left => edges.left = v,
    }
}

/// Parses CSS box-edges shorthand: `8px` (all), `8px 12px` (v/h),
/// `8px 12px 4px` (t/h/b), `8px 12px 4px 6px` (t/r/b/l).
fn parse_box_edges(value: &str) -> Option<HtmlBoxEdges> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    let nums: Vec<f32> = parts.iter().filter_map(|p| parse_px(p)).collect();
    match nums.len() {
        1 => Some(HtmlBoxEdges::uniform(nums[0])),
        2 => Some(HtmlBoxEdges {
            top: nums[0],
            right: nums[1],
            bottom: nums[0],
            left: nums[1],
        }),
        3 => Some(HtmlBoxEdges {
            top: nums[0],
            right: nums[1],
            bottom: nums[2],
            left: nums[1],
        }),
        4 => Some(HtmlBoxEdges {
            top: nums[0],
            right: nums[1],
            bottom: nums[2],
            left: nums[3],
        }),
        _ => None,
    }
}

/// `border: 1px solid red` shorthand — extract the width part only.
fn parse_border_width_shorthand(value: &str) -> Option<HtmlBoxEdges> {
    // Look for the first token that parses as a px length.
    for token in value.split_whitespace() {
        if let Some(px) = parse_px(token) {
            return Some(HtmlBoxEdges::uniform(px));
        }
    }
    None
}

fn parse_length(value: &str) -> Option<HtmlLength> {
    let value = value.trim();
    if let Some(rest) = value.strip_suffix('%') {
        return rest.trim().parse::<f32>().ok().map(HtmlLength::Percent);
    }
    if let Some(rest) = value.to_ascii_lowercase().strip_suffix("px") {
        return rest.trim().parse::<f32>().ok().map(HtmlLength::Px);
    }
    if let Some(rest) = value.to_ascii_lowercase().strip_suffix("em") {
        return rest.trim().parse::<f32>().ok().map(HtmlLength::Em);
    }
    // Bare number is treated as px.
    value.parse::<f32>().ok().map(HtmlLength::Px)
}

fn parse_px(value: &str) -> Option<f32> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let rest = lower.strip_suffix("px").unwrap_or(value);
    rest.trim().parse::<f32>().ok()
}

fn parse_css_color(value: &str) -> Option<HtmlCssColor> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("currentcolor") {
        return Some(HtmlCssColor::CurrentColor);
    }
    if value.eq_ignore_ascii_case("transparent") {
        return Some(HtmlCssColor::Rgba(HtmlCssRgba {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 0.0,
        }));
    }
    if let Some(hex) = value.strip_prefix('#')
        && let Ok((red, green, blue, alpha)) = parse_hash_color(hex.as_bytes())
    {
        return Some(HtmlCssColor::Rgba(HtmlCssRgba {
            red,
            green,
            blue,
            alpha,
        }));
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphabetic() || ch == '-')
        && let Ok((red, green, blue)) = parse_named_color(value)
    {
        return Some(HtmlCssColor::Rgba(HtmlCssRgba {
            red,
            green,
            blue,
            alpha: 1.0,
        }));
    }
    parse_rgb_color(value).or_else(|| parse_hsl_color(value))
}

fn parse_rgb_color(value: &str) -> Option<HtmlCssColor> {
    let args = css_function_args(value, &["rgb", "rgba"])?;
    let parts = css_function_parts(args);
    if parts.len() < 3 {
        return None;
    }

    let red = parse_rgb_component(&parts[0])?;
    let green = parse_rgb_component(&parts[1])?;
    let blue = parse_rgb_component(&parts[2])?;
    let alpha = parts
        .get(3)
        .and_then(|part| parse_alpha_component(part))
        .unwrap_or(1.0);
    Some(HtmlCssColor::Rgba(HtmlCssRgba {
        red,
        green,
        blue,
        alpha,
    }))
}

fn parse_hsl_color(value: &str) -> Option<HtmlCssColor> {
    let args = css_function_args(value, &["hsl", "hsla"])?;
    let parts = css_function_parts(args);
    if parts.len() < 3 {
        return None;
    }

    let hue = parse_hue(&parts[0])?;
    let saturation = parse_percent_component(&parts[1])?;
    let lightness = parse_percent_component(&parts[2])?;
    let alpha = parts
        .get(3)
        .and_then(|part| parse_alpha_component(part))
        .unwrap_or(1.0);
    let (red, green, blue) = hsl_to_rgb(hue, saturation, lightness);
    Some(HtmlCssColor::Rgba(HtmlCssRgba {
        red,
        green,
        blue,
        alpha,
    }))
}

fn css_function_args<'a>(value: &'a str, names: &[&str]) -> Option<&'a str> {
    let open = value.find('(')?;
    let close = value.rfind(')')?;
    if close <= open || !value[close + 1..].trim().is_empty() {
        return None;
    }
    let name = value[..open].trim();
    names
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
        .then_some(&value[open + 1..close])
}

fn css_function_parts(args: &str) -> Vec<String> {
    if args.contains(',') {
        return args
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
    }

    let normalized = args.replace('/', " / ");
    normalized
        .split_whitespace()
        .filter(|token| *token != "/")
        .map(str::to_string)
        .collect()
}

fn parse_rgb_component(value: &str) -> Option<u8> {
    if let Some(percent) = value.trim().strip_suffix('%') {
        let value = parse_css_number(percent)?;
        return Some((value.clamp(0.0, 100.0) * 255.0 / 100.0).round() as u8);
    }

    let value = parse_css_number(value)?;
    Some(value.clamp(0.0, 255.0).round() as u8)
}

fn parse_percent_component(value: &str) -> Option<f32> {
    let value = value.trim().strip_suffix('%')?;
    Some((parse_css_number(value)? / 100.0).clamp(0.0, 1.0))
}

fn parse_alpha_component(value: &str) -> Option<f32> {
    if let Some(percent) = value.trim().strip_suffix('%') {
        return Some((parse_css_number(percent)? / 100.0).clamp(0.0, 1.0));
    }
    Some(parse_css_number(value)?.clamp(0.0, 1.0))
}

fn parse_hue(value: &str) -> Option<f32> {
    let trimmed = value.trim().to_ascii_lowercase();
    if let Some(value) = trimmed.strip_suffix("deg") {
        return parse_css_number(value);
    }
    if let Some(value) = trimmed.strip_suffix("turn") {
        return Some(parse_css_number(value)? * 360.0);
    }
    if let Some(value) = trimmed.strip_suffix("rad") {
        return Some(parse_css_number(value)? * 180.0 / std::f32::consts::PI);
    }
    parse_css_number(&trimmed)
}

fn hsl_to_rgb(hue_degrees: f32, saturation: f32, lightness: f32) -> (u8, u8, u8) {
    let hue = hue_degrees.rem_euclid(360.0) / 60.0;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (red, green, blue) = match hue.floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    (
        ((red + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((green + m).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((blue + m).clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn parse_css_font_size(value: &str) -> Option<HtmlCssFontSize> {
    let trimmed = value.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "xx-small" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::XxSmall)),
        "x-small" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::XSmall)),
        "small" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::Small)),
        "medium" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::Medium)),
        "large" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::Large)),
        "x-large" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::XLarge)),
        "xx-large" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::XxLarge)),
        "smaller" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::Smaller)),
        "larger" => return Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::Larger)),
        _ => {}
    }

    if let Some(value) = trimmed.strip_suffix("rem") {
        return Some(HtmlCssFontSize::Rem(parse_non_negative_css_number(value)?));
    }
    if let Some(value) = trimmed.strip_suffix("em") {
        return Some(HtmlCssFontSize::Em(parse_non_negative_css_number(value)?));
    }
    if let Some(value) = trimmed.strip_suffix("px") {
        return Some(HtmlCssFontSize::Px(parse_non_negative_css_number(value)?));
    }
    if let Some(value) = trimmed.strip_suffix('%') {
        return Some(HtmlCssFontSize::Percent(parse_non_negative_css_number(
            value,
        )?));
    }
    None
}

fn parse_non_negative_css_number(value: &str) -> Option<f32> {
    let value = parse_css_number(value)?;
    (value >= 0.0).then_some(value)
}

fn parse_css_number(value: &str) -> Option<f32> {
    let value = value.trim().parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn css_number(value: f32) -> String {
    let mut formatted = format!("{:.3}", value);
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
}

fn is_safe_tag(name: &str) -> bool {
    is_inline_tag(name) || is_block_tag(name)
}

pub(crate) fn is_inline_tag(name: &str) -> bool {
    matches!(
        name,
        "a" | "strong"
            | "em"
            | "b"
            | "i"
            | "u"
            | "mark"
            | "del"
            | "ins"
            | "code"
            | "kbd"
            | "sup"
            | "sub"
            | "small"
            | "abbr"
            | "dfn"
            | "time"
            | "q"
            | "span"
    )
}

/// Block-level HTML tags that the markdown importer collects into a single
/// `HtmlBlock`. Used by the inline parser to detect a block-level tag that
/// appears mid-paragraph (where it can't form a real block) and degrade it
/// gracefully instead of leaving the literal `<div>` text on screen.
pub(crate) fn is_block_html_tag(name: &str) -> bool {
    matches!(
        name,
        "div"
            | "p"
            | "section"
            | "article"
            | "aside"
            | "header"
            | "footer"
            | "nav"
            | "main"
            | "figure"
            | "figcaption"
            | "blockquote"
            | "pre"
            | "table"
            | "details"
            | "summary"
    )
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "div"
            | "p"
            | "blockquote"
            | "hr"
            | "br"
            | "details"
            | "summary"
            | "figure"
            | "figcaption"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "img"
            | "pre"
    )
}

fn is_void_tag(name: &str) -> bool {
    matches!(name, "br" | "hr" | "img")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_inline_html_classifies_as_semantic() {
        let doc = parse_html_document("<span style='color:blue;'>Blue</span>");
        assert!(doc.is_semantic());
        assert_eq!(doc.nodes[0].tag_name, "span");
        assert_eq!(doc.raw_source, "<span style='color:blue;'>Blue</span>");
    }

    #[test]
    fn risky_tag_classifies_as_raw_text() {
        let doc = parse_html_document("<script>alert(1)</script>");
        assert_eq!(doc.safety, HtmlSafetyClass::RawTextBlock);
        assert_eq!(doc.nodes[0].raw_source, "<script>alert(1)</script>");
    }

    // Regression: a `<` followed by a digit (e.g. "<1 秒", "<100MB") must be
    // treated as literal text, not the start of an unknown tag. Otherwise the
    // whole wrapper degrades to RawTextBlock. (HTML5 spec: tag names must
    // start with an ASCII letter.)
    #[test]
    fn less_than_digit_is_text_not_tag() {
        let doc = parse_html_document("<b>⚡ <1 秒</b>");
        assert_eq!(doc.safety, HtmlSafetyClass::Semantic);
        assert_eq!(doc.nodes[0].tag_name, "b");
    }

    #[test]
    fn dangerous_attribute_classifies_as_raw_text() {
        let doc = parse_html_document("<a href=\"javascript:alert(1)\">bad</a>");
        assert_eq!(doc.safety, HtmlSafetyClass::RawTextBlock);
    }

    // A blockquote inside a wrapper (`<div>> quote</div>`) contains a literal
    // `>` which is valid Markdown but makes tree-sitter-html report an error.
    // The document must still classify as semantic so the inner Markdown
    // renders. Regression guard for the README-style wrapper use case.
    #[test]
    fn div_with_inner_blockquote_quote_renders_semantic() {
        let doc = parse_html_document("<div>> 引用</div>");
        assert_eq!(doc.safety, HtmlSafetyClass::Semantic);
    }

    #[test]
    fn div_with_full_readme_markdown_inner_renders_semantic() {
        // The user's EXACT README block (verbatim).
        let src = "<div align=\"center\">\n\n# ⚡ Verve\n\n**🔥🔥🔥 基于 Rust 构建。**\n\n### 一站式研发工作台\n\n`接口与测试` · `终端与排查`\n\n<br/>\n\n> 🚫 **告别 Postman。**\n\n<br/>\n\n<table>\n  <tr>\n    <td align=\"center\"><b>🦀 Rust 原生</b><br/><sub>非 Electron</sub></td>\n    <td align=\"center\"><b>⚡ <1 秒启动</b><br/><sub>即开即用</sub></td>\n    <td align=\"center\"><b>💾 <100MB 内存</b><br/><sub>轻量</sub></td>\n  </tr>\n</table>\n\n<br/>\n\n[功能特性](#功能特性)\n\n</div>";
        let doc = parse_html_document(src);
        assert_eq!(doc.safety, HtmlSafetyClass::Semantic);
        assert_eq!(doc.nodes[0].tag_name, "div");
    }

    #[test]
    fn parses_standalone_html_image_block() {
        let image = parse_html_image_block(
            "<img src=\"./xxx/abc.png\" alt=\"alt text\" style=\"zoom:80%;\" />",
        )
        .expect("html image");

        assert_eq!(image.src, "./xxx/abc.png");
        assert_eq!(image.alt, "alt text");
        assert_eq!(image.zoom, 0.8);
    }

    #[test]
    fn html_image_zoom_ignores_other_style_declarations() {
        let image = parse_html_image_block(
            "<img src=\"a.png\" alt=\"a\" style=\"color:red; zoom: 120%; width:10px\" />",
        )
        .expect("html image");

        assert_eq!(image.zoom, 1.2);
        assert_eq!(
            image.to_sanitized_html_with_src("a.png"),
            "<img src=\"a.png\" alt=\"a\" style=\"zoom: 120%;\">"
        );
    }

    #[test]
    fn invalid_html_image_blocks_are_not_images() {
        assert!(parse_html_image_block("<img alt=\"missing src\" />").is_none());
        assert!(parse_html_image_block("<img src=\"\" />").is_none());
        assert!(parse_html_image_block("<span><img src=\"x.png\" /></span>").is_none());
    }

    #[test]
    fn risky_child_is_local_raw_inside_safe_parent() {
        let doc = parse_html_document("<div>safe<script>alert(1)</script>tail</div>");
        assert!(doc.is_semantic());
        let div = &doc.nodes[0];
        assert!(
            div.children
                .iter()
                .any(|child| child.kind == HtmlNodeKind::RawTextBlock)
        );
    }

    /// Behavior note: the old hand-written parser + tree-sitter validator
    /// downgraded malformed HTML to `RawTextBlock`. Now that we parse with
    /// html5ever (spec-compliant, lenient), it *repairs* the structure (auto-
    /// closing `<summary>` and `<details>`) and classifies the result as
    /// `Semantic` — which matches user expectations (render what you can
    /// rather than dumping raw source). Genuinely dangerous content
    /// (`<script>`, `javascript:` URLs) is still downgraded — see the other
    /// `*_raw_text` tests.
    #[test]
    fn malformed_html_is_repaired_to_semantic() {
        let doc = parse_html_document("<details><summary>x</details>");
        assert_eq!(doc.safety, HtmlSafetyClass::Semantic);
    }

    #[test]
    fn parses_whitelisted_style_color_background_and_font_size() {
        let doc = parse_html_document(
            "<span style=\"color:blue; background-color:#fff8; font-size:20px\">x</span>",
        );
        let style = style_for_node(&doc.nodes[0]);

        assert_eq!(
            style.color,
            Some(HtmlCssColor::Rgba(HtmlCssRgba {
                red: 0,
                green: 0,
                blue: 255,
                alpha: 1.0,
            }))
        );
        assert_eq!(
            style.background_color,
            Some(HtmlCssColor::Rgba(HtmlCssRgba {
                red: 255,
                green: 255,
                blue: 255,
                alpha: 0.53333336,
            }))
        );
        assert_eq!(style.font_size, Some(HtmlCssFontSize::Px(20.0)));
    }

    #[test]
    fn parses_rgb_hsl_currentcolor_and_font_size_units() {
        let doc = parse_html_document(
            "<span style=\"color:rgba(255, 0, 0, .5); background-color:hsl(120 100% 50% / 25%); font-size:1.25em\">x</span>",
        );
        let style = style_for_node(&doc.nodes[0]);
        assert_eq!(
            style.color,
            Some(HtmlCssColor::Rgba(HtmlCssRgba {
                red: 255,
                green: 0,
                blue: 0,
                alpha: 0.5,
            }))
        );
        assert_eq!(
            style.background_color,
            Some(HtmlCssColor::Rgba(HtmlCssRgba {
                red: 0,
                green: 255,
                blue: 0,
                alpha: 0.25,
            }))
        );
        assert_eq!(style.font_size, Some(HtmlCssFontSize::Em(1.25)));

        let doc = parse_html_document(
            "<span style=\"color:currentColor; font-size:120%; background-color:transparent\">x</span>",
        );
        let style = style_for_node(&doc.nodes[0]);
        assert_eq!(style.color, Some(HtmlCssColor::CurrentColor));
        assert_eq!(style.font_size, Some(HtmlCssFontSize::Percent(120.0)));
        assert_eq!(
            style.background_color,
            Some(HtmlCssColor::Rgba(HtmlCssRgba {
                red: 0,
                green: 0,
                blue: 0,
                alpha: 0.0,
            }))
        );

        let doc = parse_html_document("<span style=\"font-size:large\">x</span>");
        assert_eq!(
            style_for_node(&doc.nodes[0]).font_size,
            Some(HtmlCssFontSize::Keyword(HtmlCssFontSizeKeyword::Large))
        );
    }

    #[test]
    fn ignores_unrecognized_or_invalid_style_declarations() {
        let doc = parse_html_document(
            "<span style=\"background-image:url(javascript:bad); color:not-a-real-color; font-size:-1px\">x</span>",
        );
        let style = style_for_node(&doc.nodes[0]);
        assert_eq!(style, HtmlInlineStyle::default());
        assert!(doc.is_semantic());
    }

    #[test]
    fn export_sanitizes_style_to_whitelisted_declarations() {
        let html = sanitize_html_for_export(
            "<span style=\"color:blue; background-image:url(javascript:bad); background-color:rgb(255 255 0); font-size:120%\">x</span>",
        );

        assert!(html.contains(
            "style=\"color: rgba(0,0,255,1.000); background-color: rgba(255,255,0,1.000); font-size: 120%;\""
        ));
        assert!(!html.contains("background-image"));
    }

    #[test]
    fn export_escapes_risky_html_even_when_style_is_present() {
        let html = sanitize_html_for_export("<script style=\"color:blue\">alert(1)</script>");

        assert!(
            html.contains("&lt;script style=&quot;color:blue&quot;&gt;alert(1)&lt;/script&gt;")
        );
        assert!(!html.contains("<script"));
    }

    #[test]
    fn parses_text_align_from_style() {
        let style = parse_inline_style("text-align: center;");
        assert_eq!(style.text_align, Some(HtmlTextAlign::Center));

        let style = parse_inline_style("TEXT-ALIGN: Right");
        assert_eq!(style.text_align, Some(HtmlTextAlign::Right));

        // Unknown keyword → None.
        let style = parse_inline_style("text-align: middle");
        assert_eq!(style.text_align, None);
    }

    #[test]
    fn align_attribute_fills_text_align_when_style_absent() {
        let doc = parse_html_document("<div align=\"center\">x</div>");
        let style = style_for_node(&doc.nodes[0]);
        assert_eq!(style.text_align, Some(HtmlTextAlign::Center));
    }

    #[test]
    fn style_text_align_overrides_align_attribute() {
        // `style` wins over legacy `align`.
        let doc = parse_html_document("<div align=\"left\" style=\"text-align:right\">x</div>");
        let style = style_for_node(&doc.nodes[0]);
        assert_eq!(style.text_align, Some(HtmlTextAlign::Right));
    }

    #[test]
    fn parses_padding_margin_shorthands() {
        // 1 value → uniform.
        let s = parse_inline_style("padding: 8px;");
        assert_eq!(s.padding, Some(HtmlBoxEdges::uniform(8.0)));

        // 2 values → vertical / horizontal.
        let s = parse_inline_style("margin: 8px 12px;");
        assert_eq!(
            s.margin,
            Some(HtmlBoxEdges { top: 8.0, right: 12.0, bottom: 8.0, left: 12.0 })
        );

        // 4 values → t/r/b/l.
        let s = parse_inline_style("padding: 1px 2px 3px 4px;");
        assert_eq!(
            s.padding,
            Some(HtmlBoxEdges { top: 1.0, right: 2.0, bottom: 3.0, left: 4.0 })
        );

        // Individual sides accumulate into one struct.
        let s = parse_inline_style("padding-top: 5px; padding-left: 7px;");
        let edges = s.padding.expect("padding");
        assert_eq!(edges.top, 5.0);
        assert_eq!(edges.left, 7.0);
        assert_eq!(edges.right, 0.0);
    }

    #[test]
    fn parses_width_max_width_and_border() {
        let s = parse_inline_style("width: 50%; max-width: 800px;");
        assert_eq!(s.width, Some(HtmlLength::Percent(50.0)));
        assert_eq!(s.max_width, Some(HtmlLength::Px(800.0)));

        // border shorthand extracts the px width only.
        let s = parse_inline_style("border: 1px solid red;");
        assert_eq!(s.border_width, Some(HtmlBoxEdges::uniform(1.0)));
        assert_eq!(s.border_color, None);

        // border-color separately.
        let s = parse_inline_style("border: 2px solid; border-color: #00f;");
        assert_eq!(s.border_width, Some(HtmlBoxEdges::uniform(2.0)));
        assert!(matches!(s.border_color, Some(HtmlCssColor::Rgba(_))));

        // border-radius with %.
        let s = parse_inline_style("border-radius: 50%;");
        assert_eq!(s.border_radius, Some(HtmlLength::Percent(50.0)));
    }

    #[test]
    fn unknown_style_properties_are_ignored() {
        let s = parse_inline_style("display: flex; position: absolute; color: red;");
        assert!(s.padding.is_none());
        assert!(s.margin.is_none());
        assert!(s.width.is_none());
        assert!(s.text_align.is_none());
        assert!(s.color.is_some()); // color still parsed
    }

    #[test]
    fn is_block_html_tag_recognizes_containers() {
        assert!(is_block_html_tag("div"));
        assert!(is_block_html_tag("section"));
        assert!(is_block_html_tag("article"));
        assert!(!is_block_html_tag("span"));
        assert!(!is_block_html_tag("strong"));
    }
}
