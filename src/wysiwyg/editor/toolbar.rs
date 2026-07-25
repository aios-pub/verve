//! Toolbar state and logic for the editor's title-bar formatting toolbar.
//!
//! The rendering lives in the host title bar (`src/ui/app.rs`); this module
//! owns the data the title bar reads (current heading level, active inline
//! styles, current block kind, open dropdown) and the actions it dispatches.
//! Every action reuses the same block-level methods the keyboard shortcuts
//! use, so the title-bar buttons, dropdowns, and shortcuts behave identically.

use gpui::*;

use super::Editor;
use crate::wysiwyg::components::markdown::inline::{InlineScript, InlineStyle};
use crate::wysiwyg::components::{BlockKind, BlockRecord, InlineFormat};

/// Which dropdown is open, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarDropdown {
    Paragraph,
    Format,
}

/// State for the toolbar (open dropdown). Lives on the Editor so it survives
/// re-renders and stays co-located with the editing state.
pub struct ToolbarState {
    pub open_dropdown: Option<ToolbarDropdown>,
}

impl ToolbarState {
    pub fn new() -> Self {
        Self { open_dropdown: None }
    }
}

/// Read-only snapshot of whatever the title-bar toolbar needs to render the
/// active states. Built from the focused block.
#[derive(Clone, Debug, Default)]
pub struct ToolbarSnapshot {
    /// Heading level of the focused block, if it's a heading.
    pub heading_level: Option<u8>,
    /// Current block kind (for the Paragraph dropdown's checkmarks).
    pub block_kind: Option<BlockKindSnapshot>,
    /// Aggregated inline style over the selection (active = whole selection).
    pub style: InlineStyle,
    /// Whether the editor is in Rendered mode (toolbar shown) or Source.
    pub rendered: bool,
}

/// A copy of the relevant `BlockKind` discriminants for the Paragraph dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKindSnapshot {
    Paragraph,
    Separator,
    Heading(u8),
    BulletedList,
    NumberedList,
    TaskList,
    Quote,
    CodeBlock,
    MathBlock,
    HtmlBlock,
    MermaidBlock,
    Other,
}

impl From<&BlockKind> for BlockKindSnapshot {
    fn from(kind: &BlockKind) -> Self {
        match kind {
            BlockKind::Paragraph => Self::Paragraph,
            BlockKind::Separator => Self::Separator,
            BlockKind::Heading { level } => Self::Heading(*level),
            BlockKind::BulletedListItem => Self::BulletedList,
            BlockKind::NumberedListItem => Self::NumberedList,
            BlockKind::TaskListItem { .. } => Self::TaskList,
            BlockKind::Quote => Self::Quote,
            BlockKind::CodeBlock { .. } => Self::CodeBlock,
            BlockKind::MathBlock => Self::MathBlock,
            BlockKind::HtmlBlock => Self::HtmlBlock,
            BlockKind::MermaidBlock => Self::MermaidBlock,
            _ => Self::Other,
        }
    }
}

/// All toolbar actions. The title bar dispatches one of these per click.
#[derive(Clone, Debug)]
pub enum ToolbarAction {
    /// Toggle heading level N on the focused block (revert to paragraph if
    /// already that level).
    Heading(u8),
    PromoteHeading,
    DemoteHeading,
    Paragraph,
    Quote,
    BulletedList,
    NumberedList,
    TaskList,
    CodeBlock,
    /// Insert a structural block (math/html) after the focused block.
    InsertMathBlock,
    InsertHtmlBlock,
    InsertTable,
    InsertHorizontalRule,
    /// Inline formats.
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Superscript,
    Subscript,
    Highlight,
    InlineCode,
    InlineMath,
    ClearFormatting,
    /// Open the link/image prompts.
    InsertLink,
    InsertImage,
}

impl Editor {
    /// Build a toolbar snapshot from the focused block (no Window needed; used
    /// by the host title bar at render time).
    pub fn toolbar_snapshot(&self, cx: &App) -> ToolbarSnapshot {
        use super::ViewMode;
        let mut snap = ToolbarSnapshot {
            rendered: self.view_mode == ViewMode::Rendered,
            ..Default::default()
        };
        let Some(block) = focused_block_entity(self, cx) else {
            return snap;
        };
        let block = block.read(cx);
        snap.block_kind = Some(BlockKindSnapshot::from(&block.kind()));
        snap.heading_level = match block.kind() {
            BlockKind::Heading { level } => Some(level),
            _ => None,
        };
        snap.style = aggregate_style(block);
        snap
    }

    /// Open dropdown getter.
    pub fn toolbar_open_dropdown(&self) -> Option<ToolbarDropdown> {
        self.toolbar.open_dropdown
    }

    /// Execute a toolbar action against the focused block.
    pub fn toolbar_apply(
        &mut self,
        action: ToolbarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::ViewMode;
        // All editing actions require Rendered mode (matches the block-format
        // shortcuts, which only register in Rendered mode).
        if self.view_mode != ViewMode::Rendered {
            return;
        }
        // Close any open dropdown after an action fires.
        self.close_toolbar_dropdown(cx);
        match action {
            ToolbarAction::Heading(level) => {
                let current = self.toolbar_snapshot(cx).heading_level;
                if current == Some(level) {
                    self.toolbar_apply_block(ToolbarBlockKind::Paragraph, cx);
                } else {
                    self.toolbar_convert_heading(level, cx);
                }
            }
            ToolbarAction::PromoteHeading => self.toolbar_shift_heading(-1, cx),
            ToolbarAction::DemoteHeading => self.toolbar_shift_heading(1, cx),
            ToolbarAction::Paragraph => self.toolbar_apply_block(ToolbarBlockKind::Paragraph, cx),
            ToolbarAction::Quote => self.toolbar_apply_block(ToolbarBlockKind::Quote, cx),
            ToolbarAction::BulletedList => {
                self.toolbar_apply_block(ToolbarBlockKind::BulletedList, cx)
            }
            ToolbarAction::NumberedList => {
                self.toolbar_apply_block(ToolbarBlockKind::NumberedList, cx)
            }
            ToolbarAction::TaskList => self.toolbar_apply_block(ToolbarBlockKind::TaskList, cx),
            ToolbarAction::CodeBlock => self.toolbar_apply_block(ToolbarBlockKind::CodeBlock, cx),
            ToolbarAction::InsertMathBlock => {
                self.toolbar_insert_structural(BlockRecord::math("$$\n\n$$"), window, cx);
            }
            ToolbarAction::InsertHtmlBlock => {
                self.toolbar_insert_structural(BlockRecord::html("<div>\n\n</div>"), window, cx);
            }
            ToolbarAction::InsertTable => {
                self.on_insert_table(&crate::wysiwyg::components::InsertTable, window, cx);
            }
            ToolbarAction::InsertHorizontalRule => {
                self.on_insert_horizontal_rule(
                    &crate::wysiwyg::components::InsertHorizontalRule,
                    window,
                    cx,
                );
            }
            ToolbarAction::Bold => self.toolbar_toggle_inline(InlineFormat::Bold, cx),
            ToolbarAction::Italic => self.toolbar_toggle_inline(InlineFormat::Italic, cx),
            ToolbarAction::Underline => self.toolbar_toggle_inline(InlineFormat::Underline, cx),
            ToolbarAction::Strikethrough => {
                self.toolbar_toggle_inline(InlineFormat::Strikethrough, cx)
            }
            ToolbarAction::Superscript => {
                self.toolbar_toggle_inline(InlineFormat::Superscript, cx)
            }
            ToolbarAction::Subscript => self.toolbar_toggle_inline(InlineFormat::Subscript, cx),
            ToolbarAction::Highlight => self.toolbar_toggle_inline(InlineFormat::Highlight, cx),
            ToolbarAction::InlineCode => self.toolbar_toggle_inline(InlineFormat::Code, cx),
            ToolbarAction::InlineMath => self.toolbar_insert_inline_math(cx),
            ToolbarAction::ClearFormatting => self.toolbar_clear_formatting(cx),
            ToolbarAction::InsertLink => {
                if let Some(block) = focused_block_entity(self, cx) {
                    block.update(cx, |b, cx| {
                        b.on_insert_link(&crate::wysiwyg::components::InsertLink, window, cx)
                    });
                }
            }
            ToolbarAction::InsertImage => {
                self.on_insert_image(&crate::wysiwyg::components::InsertImage, window, cx);
            }
        }
    }

    pub fn toolbar_toggle_dropdown(&mut self, dropdown: ToolbarDropdown, cx: &mut Context<Self>) {
        self.toolbar.open_dropdown = if self.toolbar.open_dropdown == Some(dropdown) {
            None
        } else {
            Some(dropdown)
        };
        cx.notify();
    }

    /// Close any open dropdown (called after an action fires, or by the
    /// dropdown's outside-click overlay).
    pub fn close_toolbar_dropdown(&mut self, cx: &mut Context<Self>) {
        if self.toolbar.open_dropdown.take().is_some() {
            cx.notify();
        }
    }
}

// ---- private helpers (visibility via accessor) ----

/// The focused block entity, resolved via `active_entity_id` (no Window needed).
fn focused_block_entity(
    editor: &Editor,
    cx: &App,
) -> Option<Entity<crate::wysiwyg::components::Block>> {
    let id = editor.active_entity_id_for_toolbar()?;
    editor.focusable_entity_by_id(id)
}

/// Aggregated inline style over the current selection (Typora semantics: a
/// flag is active only if every span overlapping the selection has it).
fn aggregate_style(block: &crate::wysiwyg::components::Block) -> InlineStyle {
    let sel = block.selected_range.clone();
    if sel.is_empty() {
        return block.inline_style_at(sel.start);
    }
    let mut agg = InlineStyle {
        bold: true,
        italic: true,
        underline: true,
        strikethrough: true,
        highlight: true,
        code: true,
        script: InlineScript::Superscript, // sentinel
    };
    let mut any = false;
    let mut all_super = true;
    let mut all_sub = true;
    for span in block.inline_spans() {
        if span.range.start < sel.end && span.range.end > sel.start {
            any = true;
            let s = span.style;
            agg.bold &= s.bold;
            agg.italic &= s.italic;
            agg.underline &= s.underline;
            agg.strikethrough &= s.strikethrough;
            agg.highlight &= s.highlight;
            agg.code &= s.code;
            if s.script != InlineScript::Superscript {
                all_super = false;
            }
            if s.script != InlineScript::Subscript {
                all_sub = false;
            }
        }
    }
    if !any {
        return InlineStyle::default();
    }
    agg.script = if all_super {
        InlineScript::Superscript
    } else if all_sub {
        InlineScript::Subscript
    } else {
        InlineScript::Normal
    };
    agg
}

/// Which block-conversion a toolbar action targets.
#[derive(Clone, Copy)]
enum ToolbarBlockKind {
    Paragraph,
    Quote,
    BulletedList,
    NumberedList,
    TaskList,
    CodeBlock,
}

impl Editor {
    fn toolbar_toggle_inline(&mut self, format: InlineFormat, cx: &mut Context<Self>) {
        let Some(block) = focused_block_entity(self, cx) else {
            return;
        };
        block.update(cx, |b, cx| b.toggle_inline_format(format, cx));
    }

    fn toolbar_convert_heading(&mut self, level: u8, cx: &mut Context<Self>) {
        let Some(block) = focused_block_entity(self, cx) else {
            return;
        };
        block.update(cx, |b, cx| b.convert_to_heading(level, cx));
    }

    fn toolbar_shift_heading(&mut self, delta: i8, cx: &mut Context<Self>) {
        let Some(level) = self.toolbar_snapshot(cx).heading_level else {
            return;
        };
        let new_level = (level as i8 + delta).clamp(1, 6) as u8;
        if new_level != level {
            self.toolbar_convert_heading(new_level, cx);
        }
    }

    fn toolbar_apply_block(&mut self, kind: ToolbarBlockKind, cx: &mut Context<Self>) {
        let Some(block) = focused_block_entity(self, cx) else {
            return;
        };
        block.update(cx, |b, cx| match kind {
            ToolbarBlockKind::Paragraph => b.convert_to_paragraph(cx),
            ToolbarBlockKind::Quote => b.convert_to_quote(cx),
            ToolbarBlockKind::BulletedList => b.convert_to_bulleted_list(cx),
            ToolbarBlockKind::NumberedList => b.convert_to_numbered_list(cx),
            ToolbarBlockKind::TaskList => b.convert_to_task_list(cx),
            ToolbarBlockKind::CodeBlock => b.enter_code_block(None, cx),
        });
    }

    fn toolbar_insert_structural(
        &mut self,
        record: BlockRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.insert_target_from_focus(window, cx);
        self.insert_structural_block(target, record, cx);
    }

    fn toolbar_clear_formatting(&mut self, cx: &mut Context<Self>) {
        let Some(block) = focused_block_entity(self, cx) else {
            return;
        };
        block.update(cx, |b, cx| b.clear_inline_formatting(cx));
    }

    fn toolbar_insert_inline_math(&mut self, cx: &mut Context<Self>) {
        let Some(block) = focused_block_entity(self, cx) else {
            return;
        };
        block.update(cx, |b, cx| b.insert_inline_math_at_selection(cx));
    }
}
