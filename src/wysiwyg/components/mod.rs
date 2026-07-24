//! Shared UI components and Markdown editing primitives (ported from velotype).
//!
//! velotype's standalone `switch` widget and the `actions` keybinding module
//! are not ported here: verve uses gpui-component's own widgets and registers
//! the block-editor keybindings from the editor layer (stage 3).

pub(crate) mod actions;
pub(crate) mod block;
pub(crate) mod latex;
pub(crate) mod markdown;
pub(crate) mod mermaid;

pub use actions::*;

/// Registers the WYSIWYG block-editor keybindings with default keys.
pub fn init(cx: &mut gpui::App) {
    actions::init(cx);
}

/// Registers the WYSIWYG block-editor keybindings with user overrides from
/// layout.json. Called at startup by main.rs.
pub fn init_with_keybindings(
    cx: &mut gpui::App,
    config: &std::collections::BTreeMap<String, Vec<String>>,
) {
    actions::init_with_keybindings(cx, config);
}
pub use block::*;
#[allow(unused_imports)]
pub(crate) use latex::*;
pub(crate) use markdown::code_highlight::*;
#[allow(unused_imports)]
pub(crate) use markdown::footnote::*;
#[allow(unused_imports)]
pub(crate) use markdown::html::*;
#[allow(unused_imports)]
pub(crate) use markdown::image::*;
pub use markdown::inline::*;
#[allow(unused_imports)]
pub(crate) use markdown::link::*;
pub use markdown::table::*;
#[allow(unused_imports)]
pub(crate) use mermaid::*;
