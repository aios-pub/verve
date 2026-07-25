//! Top formatting toolbar for the WYSIWYG editor.
//!
//! Layout (left → right): H1/H2/H3 → B/I/S/U → Paragraph dropdown → Format
//! dropdown. Rendered as the first row of `main_content`, so it sits at the
//! editor viewport's top edge (the empty title-bar slot). Visible in both the
//! Notes panel and the Markdown side-view (both embed the same `Editor`), and
//! only in Rendered mode.
//!
//! Every button / menu item reuses the same block-level methods the keyboard
//! shortcuts use, so the three entry points (button, menu, shortcut) behave
//! identically. Shortcut keycaps in the dropdowns are read live from the
//! resolved shortcut config.

use std::collections::BTreeMap;

use gpui::*;
use gpui::prelude::FluentBuilder as _;
use gpui_component::kbd::Kbd;
use gpui_component::{h_flex, v_flex, ActiveTheme as _};

use super::Editor;
use crate::wysiwyg::components::actions::{resolved_shortcut_keys, ShortcutCommand};
use crate::wysiwyg::components::markdown::inline::{InlineScript, InlineStyle};
use crate::wysiwyg::components::{
    BlockKind, BlockRecord, InlineFormat, InsertHorizontalRule, InsertImage, InsertLink,
    InsertTable,
};
use crate::wysiwyg::theme::Theme;

/// Which dropdown is open, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarDropdown {
    Paragraph,
    Format,
}

/// State for the toolbar.
pub(super) struct ToolbarState {
    pub open_dropdown: Option<ToolbarDropdown>,
}

impl ToolbarState {
    pub fn new() -> Self {
        Self { open_dropdown: None }
    }
}

/// Aggregated inline style over the current selection (Typora semantics: a
/// flag is active only if every span overlapping the selection has it).
fn active_style_over_selection(editor: &Editor, cx: &App) -> InlineStyle {
    let Some(block) = focused_block_entity(editor, cx) else {
        return InlineStyle::default();
    };
    let block = block.read(cx);
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

/// The focused block entity, resolved via `active_entity_id` (no Window needed
/// for the execution helpers, which fire from click handlers).
fn focused_block_entity(editor: &Editor, cx: &App) -> Option<Entity<crate::wysiwyg::components::Block>> {
    let id = editor.active_entity_id_for_toolbar()?;
    editor.focusable_entity_by_id(id)
}

/// The heading level of the focused block, or None if it isn't a heading.
fn focused_heading_level(editor: &Editor, cx: &App) -> Option<u8> {
    let block = focused_block_entity(editor, cx)?;
    match block.read(cx).kind() {
        BlockKind::Heading { level } => Some(level),
        _ => None,
    }
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

fn heading_command(level: u8) -> ShortcutCommand {
    match level {
        1 => ShortcutCommand::Heading1,
        2 => ShortcutCommand::Heading2,
        3 => ShortcutCommand::Heading3,
        4 => ShortcutCommand::Heading4,
        5 => ShortcutCommand::Heading5,
        _ => ShortcutCommand::Heading6,
    }
}

/// Render one keycap from a resolved key string.
fn keycap_for(key: &str) -> Option<AnyElement> {
    Keystroke::parse(key).ok().map(|ks| Kbd::new(ks).into_any_element())
}

/// First keycap for a command (the primary binding), for dropdown rows.
fn primary_keycap(
    config: &BTreeMap<String, Vec<String>>,
    command: ShortcutCommand,
) -> Option<AnyElement> {
    resolved_shortcut_keys(config, command)
        .first()
        .and_then(|k| keycap_for(k))
}

impl Editor {
    // ---- execution helpers (shared with keyboard shortcuts) ----

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
        let Some(level) = focused_heading_level(self, cx) else {
            return;
        };
        let new_level = (level as i8 + delta).clamp(1, 6) as u8;
        if new_level != level {
            self.toolbar_convert_heading(new_level, cx);
        }
    }

    fn toolbar_convert_block(&mut self, kind: ToolbarBlockKind, cx: &mut Context<Self>) {
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

    // ---- dropdown open/close ----

    pub(super) fn toggle_toolbar_dropdown(
        &mut self,
        dropdown: ToolbarDropdown,
        cx: &mut Context<Self>,
    ) {
        self.toolbar.open_dropdown = if self.toolbar.open_dropdown == Some(dropdown) {
            None
        } else {
            Some(dropdown)
        };
        cx.notify();
    }

    pub(super) fn close_toolbar_dropdown(&mut self, cx: &mut Context<Self>) {
        if self.toolbar.open_dropdown.take().is_some() {
            cx.notify();
        }
    }

    // ---- render ----

    pub(super) fn render_toolbar(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use super::ViewMode;
        if self.view_mode != ViewMode::Rendered {
            return None;
        }

        let c = &theme.colors;
        let d = &theme.dimensions;
        let style = active_style_over_selection(self, cx);
        let heading_level = focused_heading_level(self, cx);
        let open = self.toolbar.open_dropdown;

        let bar = h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.))
            .px(px(d.status_bar_padding_x))
            .h(px(d.status_bar_height))
            .border_b(px(1.0))
            .border_color(c.dialog_border)
            .bg(c.status_bar_background)
            // H1 / H2 / H3
            .child(self.render_heading_button("tb-h1", "H1", 1, heading_level, theme, cx))
            .child(self.render_heading_button("tb-h2", "H2", 2, heading_level, theme, cx))
            .child(self.render_heading_button("tb-h3", "H3", 3, heading_level, theme, cx))
            .child(self.render_divider(theme))
            // B / I / S / U
            .child(self.render_format_button(
                "tb-bold", "B", FormatIcon::Bold, style.bold, InlineFormat::Bold, theme, cx,
            ))
            .child(self.render_format_button(
                "tb-italic", "I", FormatIcon::Italic, style.italic, InlineFormat::Italic, theme, cx,
            ))
            .child(self.render_format_button(
                "tb-strike",
                "S",
                FormatIcon::Strike,
                style.strikethrough,
                InlineFormat::Strikethrough,
                theme,
                cx,
            ))
            .child(self.render_format_button(
                "tb-underline",
                "U",
                FormatIcon::Underline,
                style.underline,
                InlineFormat::Underline,
                theme,
                cx,
            ))
            .child(self.render_divider(theme))
            // Paragraph / Format dropdown triggers
            .child(self.render_dropdown_trigger(
                "tb-paragraph",
                "Paragraph",
                open == Some(ToolbarDropdown::Paragraph),
                ToolbarDropdown::Paragraph,
                theme,
                cx,
            ))
            .child(self.render_dropdown_trigger(
                "tb-format",
                "Format",
                open == Some(ToolbarDropdown::Format),
                ToolbarDropdown::Format,
                theme,
                cx,
            ));

        let mut base = v_flex().child(bar);
        if let Some(dropdown) = open {
            let panel = match dropdown {
                ToolbarDropdown::Paragraph => self.render_paragraph_dropdown(theme, window, cx),
                ToolbarDropdown::Format => self.render_format_dropdown(theme, window, cx),
            };
            base = base.child(
                div()
                    .id("toolbar-dropdown-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.close_toolbar_dropdown(cx)),
                    )
                    .child(panel),
            );
        }

        Some(base.into_any_element())
    }

    fn render_divider(&self, theme: &Theme) -> AnyElement {
        div()
            .mx(px(4.))
            .w(px(1.0))
            .h(px(16.0))
            .bg(theme.colors.dialog_border)
            .into_any_element()
    }

    /// H1/H2/H3 button. Active when the focused block is that heading level;
    /// clicking toggles between that level and a plain paragraph.
    fn render_heading_button(
        &mut self,
        id: &'static str,
        label: &'static str,
        level: u8,
        heading_level: Option<u8>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let active = heading_level == Some(level);
        div()
            .id(id)
            .h(px(d.status_bar_height - 6.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .bg(if active {
                c.text_link.opacity(0.2)
            } else {
                hsla(0., 0., 0., 0.)
            })
            .hover(|this| this.bg(c.status_bar_button_hover))
            .cursor_pointer()
            .text_size(px(d.status_bar_text_size))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if active { c.text_link } else { c.status_bar_text })
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, _window, cx| {
                if focused_heading_level(this, cx) == Some(level) {
                    this.toolbar_convert_block(ToolbarBlockKind::Paragraph, cx);
                } else {
                    this.toolbar_convert_heading(level, cx);
                }
            }))
            .into_any_element()
    }

    fn render_format_button(
        &mut self,
        id: &'static str,
        label: &'static str,
        icon: FormatIcon,
        active: bool,
        format: InlineFormat,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let color = if active { c.text_link } else { c.status_bar_text };
        // Decorate the glyph so the icon itself suggests the format. The
        // underline / line-through color follows the text color.
        let mut glyph = div().child(label.to_string());
        glyph = match icon {
            FormatIcon::Bold => glyph.font_weight(FontWeight::BOLD),
            FormatIcon::Italic => glyph.italic(),
            FormatIcon::Underline => glyph.underline(),
            FormatIcon::Strike => glyph.line_through(),
        };
        div()
            .id(id)
            .h(px(d.status_bar_height - 6.0))
            .w(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(if active {
                c.text_link.opacity(0.2)
            } else {
                hsla(0., 0., 0., 0.)
            })
            .hover(|this| this.bg(c.status_bar_button_hover))
            .cursor_pointer()
            .text_size(px(d.status_bar_text_size))
            .text_color(color)
            .child(glyph)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toolbar_toggle_inline(format, cx);
            }))
            .into_any_element()
    }

    fn render_dropdown_trigger(
        &mut self,
        id: &'static str,
        label: &'static str,
        open: bool,
        dropdown: ToolbarDropdown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        div()
            .id(id)
            .h(px(d.status_bar_height - 6.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(4.0))
            .bg(if open {
                c.status_bar_button_hover
            } else {
                hsla(0., 0., 0., 0.)
            })
            .hover(|this| this.bg(c.status_bar_button_hover))
            .cursor_pointer()
            .text_size(px(d.status_bar_text_size))
            .text_color(c.status_bar_text)
            .child(label.to_string())
            .child(div().text_size(px(10.0)).child(if open { "▲" } else { "▼" }))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.toggle_toolbar_dropdown(dropdown, cx);
            }))
            .into_any_element()
    }

    fn render_paragraph_dropdown(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let level = focused_heading_level(self, cx);
        let kind = focused_block_entity(self, cx).map(|b| b.read(cx).kind());
        let config = crate::state::persistence::load_shortcuts();
        let mut items: Vec<AnyElement> = Vec::new();

        for lvl in [4u8, 5, 6] {
            items.push(self.render_item(
                &format!("p-h{lvl}"),
                &format!("Heading {lvl}"),
                level == Some(lvl),
                primary_keycap(&config, heading_command(lvl)),
                false,
                theme,
                cx,
                move |this, _w, cx| this.toolbar_convert_heading(lvl, cx),
            ));
        }
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "p-promote",
            "Promote Heading",
            false,
            None,
            level.is_none() || level == Some(1),
            theme,
            cx,
            |this, _w, cx| this.toolbar_shift_heading(-1, cx),
        ));
        items.push(self.render_item(
            "p-demote",
            "Demote Heading",
            false,
            None,
            level.is_none() || level == Some(6),
            theme,
            cx,
            |this, _w, cx| this.toolbar_shift_heading(1, cx),
        ));
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "p-table",
            "Table",
            false,
            primary_keycap(&config, ShortcutCommand::InsertTable),
            false,
            theme,
            cx,
            |this, w, cx| this.on_insert_table(&InsertTable, w, cx),
        ));
        items.push(self.render_item(
            "p-code",
            "Code Fences",
            matches!(kind, Some(BlockKind::CodeBlock { .. })),
            primary_keycap(&config, ShortcutCommand::ConvertToCodeBlock),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_convert_block(ToolbarBlockKind::CodeBlock, cx),
        ));
        items.push(self.render_item(
            "p-quote",
            "Quote Block",
            matches!(kind, Some(BlockKind::Quote)),
            primary_keycap(&config, ShortcutCommand::ConvertToQuote),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_convert_block(ToolbarBlockKind::Quote, cx),
        ));
        items.push(self.render_item(
            "p-math",
            "Math Block",
            matches!(kind, Some(BlockKind::MathBlock)),
            None,
            false,
            theme,
            cx,
            |this, w, cx| this.toolbar_insert_structural(BlockRecord::math("$$\n\n$$"), w, cx),
        ));
        items.push(self.render_item(
            "p-html",
            "Html Block",
            matches!(kind, Some(BlockKind::HtmlBlock)),
            None,
            false,
            theme,
            cx,
            |this, w, cx| {
                this.toolbar_insert_structural(BlockRecord::html("<div>\n\n</div>"), w, cx)
            },
        ));
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "p-ordered",
            "Ordered List",
            matches!(kind, Some(BlockKind::NumberedListItem)),
            primary_keycap(&config, ShortcutCommand::ConvertToNumberedList),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_convert_block(ToolbarBlockKind::NumberedList, cx),
        ));
        items.push(self.render_item(
            "p-bullet",
            "Bullet List",
            matches!(kind, Some(BlockKind::BulletedListItem)),
            primary_keycap(&config, ShortcutCommand::ConvertToBulletedList),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_convert_block(ToolbarBlockKind::BulletedList, cx),
        ));
        items.push(self.render_item(
            "p-task",
            "Task List",
            matches!(kind, Some(BlockKind::TaskListItem { .. })),
            primary_keycap(&config, ShortcutCommand::ConvertToTaskList),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_convert_block(ToolbarBlockKind::TaskList, cx),
        ));
        items.push(self.render_item(
            "p-loose",
            "Loose List Item",
            false,
            None,
            true, // disabled: spec defers implementation
            theme,
            cx,
            |_, _, _| {},
        ));
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "p-paragraph",
            "Paragraph",
            matches!(kind, Some(BlockKind::Paragraph)),
            primary_keycap(&config, ShortcutCommand::ConvertToParagraph),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_convert_block(ToolbarBlockKind::Paragraph, cx),
        ));
        items.push(self.render_item(
            "p-hr",
            "Horizontal Rule",
            matches!(kind, Some(BlockKind::Separator)),
            primary_keycap(&config, ShortcutCommand::InsertHorizontalRule),
            false,
            theme,
            cx,
            |this, w, cx| this.on_insert_horizontal_rule(&InsertHorizontalRule, w, cx),
        ));

        self.render_panel("paragraph-dropdown", items, theme)
    }

    fn render_format_dropdown(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let style = active_style_over_selection(self, cx);
        let config = crate::state::persistence::load_shortcuts();
        let mut items: Vec<AnyElement> = Vec::new();

        items.push(self.render_item(
            "f-sup",
            "Superscript",
            style.script == InlineScript::Superscript,
            None,
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_toggle_inline(InlineFormat::Superscript, cx),
        ));
        items.push(self.render_item(
            "f-sub",
            "Subscript",
            style.script == InlineScript::Subscript,
            None,
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_toggle_inline(InlineFormat::Subscript, cx),
        ));
        items.push(self.render_item(
            "f-highlight",
            "Highlight",
            style.highlight,
            primary_keycap(&config, ShortcutCommand::HighlightSelection),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_toggle_inline(InlineFormat::Highlight, cx),
        ));
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "f-code",
            "Inline Code",
            style.code,
            primary_keycap(&config, ShortcutCommand::CodeSelection),
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_toggle_inline(InlineFormat::Code, cx),
        ));
        items.push(self.render_item(
            "f-math",
            "Inline Math",
            false,
            None,
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_insert_inline_math(cx),
        ));
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "f-link",
            "Hyperlink",
            false,
            primary_keycap(&config, ShortcutCommand::InsertLink),
            false,
            theme,
            cx,
            |this, w, cx| {
                // on_insert_link is a block-level inline action; dispatch it on
                // the focused block (same handler Cmd-K uses).
                if let Some(block) = focused_block_entity(this, cx) {
                    block.update(cx, |b, cx| b.on_insert_link(&InsertLink, w, cx));
                }
            },
        ));
        items.push(self.render_item(
            "f-image",
            "Image",
            false,
            primary_keycap(&config, ShortcutCommand::InsertImage),
            false,
            theme,
            cx,
            |this, w, cx| this.on_insert_image(&InsertImage, w, cx),
        ));
        items.push(self.render_sep(theme));
        items.push(self.render_item(
            "f-clear",
            "Clear Formatting",
            false,
            None,
            false,
            theme,
            cx,
            |this, _w, cx| this.toolbar_clear_formatting(cx),
        ));

        let _ = window;
        self.render_panel("format-dropdown", items, theme)
    }

    fn render_sep(&self, theme: &Theme) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        div()
            .mx(px(d.menu_separator_margin_x))
            .my(px(d.menu_separator_margin_y))
            .h(px(d.menu_separator_height))
            .bg(c.dialog_border)
            .into_any_element()
    }

    /// One dropdown row: ✓ (when active) + label (left), keycap (right).
    #[allow(clippy::too_many_arguments)]
    fn render_item<F>(
        &mut self,
        id: &str,
        label: &str,
        checked: bool,
        keycap: Option<AnyElement>,
        disabled: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
        on_click: F,
    ) -> AnyElement
    where
        F: Fn(&mut Editor, &mut Window, &mut Context<Editor>) + 'static,
    {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let id_el = ElementId::Name(format!("tb-item-{id}").into());
        let mut row = div()
            .id(id_el)
            .w_full()
            .h(px(d.menu_item_height))
            .px(px(d.menu_item_padding_x))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .rounded(px(d.menu_item_radius));
        if disabled {
            row = row.opacity(0.4);
        } else {
            row = row.hover(|this| this.bg(c.status_bar_button_hover)).cursor_pointer();
        }
        row = row.child(
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(14.0)).text_color(c.text_link).child(if checked { "✓" } else { "" }))
                .child(
                    div()
                        .text_size(px(d.menu_text_size))
                        .text_color(c.status_bar_text)
                        .child(label.to_string()),
                ),
        );
        row = row.child(h_flex().gap(px(2.)).when_some(keycap, |this, k| this.child(k)));
        if disabled {
            row.into_any_element()
        } else {
            row.on_click(cx.listener(move |this, _, window, cx| {
                this.close_toolbar_dropdown(cx);
                on_click(this, window, cx);
            }))
            .into_any_element()
        }
    }

    fn render_panel(&self, id: &'static str, items: Vec<AnyElement>, theme: &Theme) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        div()
            .id(id)
            .absolute()
            .top(px(d.status_bar_height))
            .left(px(d.status_bar_padding_x))
            .w(px(240.0))
            .max_h(px(420.0))
            .overflow_y_scroll()
            .p(px(d.menu_panel_padding))
            .flex()
            .flex_col()
            .gap(px(2.))
            .bg(c.dialog_surface)
            .border(px(d.dialog_border_width))
            .border_color(c.dialog_border)
            .rounded(px(d.menu_panel_radius))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_e, _w, cx| cx.stop_propagation())
            .children(items)
            .into_any_element()
    }
}

/// How to decorate a format button glyph.
#[derive(Clone, Copy)]
enum FormatIcon {
    Bold,
    Italic,
    Underline,
    Strike,
}
