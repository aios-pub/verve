//! Block runtime and semantic state.
//!
//! This module groups the block entity itself, block-level Markdown parsing,
//! inline text-tree handling, rendering, input bridging, and interaction
//! handlers. A block owns local editing state while the editor owns tree
//! structure and cross-block mutations.

mod element;
mod input;
mod interactions;
mod render;
mod runtime;
mod state;

pub(crate) use crate::wysiwyg::components::markdown::code_highlight::*;
pub(crate) use crate::wysiwyg::components::markdown::footnote::*;
pub(crate) use crate::wysiwyg::components::markdown::image::*;
pub use crate::wysiwyg::components::markdown::inline::*;
pub(crate) use crate::wysiwyg::components::markdown::link::*;
pub use runtime::*;
pub use state::*;

// Pure inline-layout helpers re-exported so they are testable from the
// markdown module's unit tests (the block render test module is gated behind
// a feature flag that is not always enabled).
pub(crate) use render::{inline_word_chunks, is_cjk_break_char};

// Text-layout helpers used by the editor's HTML-wrapper first-click
// resolution (mapping a click point to a character offset in a re-shaped
/// visual line, then onto a wrapper child block).
pub(crate) use element::{hard_line_ranges, wrapped_line_for_y};
