//! WYSIWYG formatting toolbar rendered inside the title bar (H1-3, B/I/S/U,
//! lists/quote/code/table/image, Paragraph & Format dropdowns) plus the
//! root-level dropdown overlay panel.

use gpui::{img, *};
use gpui::prelude::FluentBuilder as _;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable as _, h_flex, v_flex};
use crate::wysiwyg::editor::toolbar::{ToolbarAction, ToolbarDropdown, ToolbarSnapshot};
use super::widgets::{command_keycap, dropdown_panel, dropdown_separator, toolbar_divider};
use super::SideView;
use super::VerveApp;

impl VerveApp {
    pub(super) fn active_editor_entity_for_toolbar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor: Entity<crate::wysiwyg::editor::Editor> = match self.active_view {
            SideView::Notes => self.notes.read(cx).editor().clone(),
            SideView::MarkdownEditor => self.markdown.read(cx).editor().clone(),
            _ => return None,
        };
        self.render_editor_dropdown_overlay(&editor, cx)
    }

}

impl VerveApp {
    pub(super) fn render_editor_toolbar(
        &mut self,
        editor: &Entity<crate::wysiwyg::editor::Editor>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::wysiwyg::components::actions::{ShortcutCommand, resolved_shortcut_keys};
        use crate::wysiwyg::editor::toolbar::{
            BlockKindSnapshot, ToolbarAction, ToolbarDropdown, ToolbarSnapshot,
        };
        use gpui_component::kbd::Kbd;

        let theme = cx.theme().clone();
        let snap = editor.read(cx).toolbar_snapshot(cx);
        let open = editor.read(cx).toolbar_open_dropdown();
        let editor_for_actions = editor.clone();

        // Active background/text colors. The active (highlighted) state must
        // keep the glyph readable: `accent`-on-`accent@0.2` was near-invisible
        // (the icon vanished the moment a format was applied), so use the
        // high-contrast foreground for the glyph and a slightly stronger tint
        // for the active background.
        let active_bg = theme.accent.opacity(0.28);
        let active_fg = theme.foreground;
        let idle_fg = theme.foreground;
        let hover_bg = theme.muted;

        // ---- flat button builder ----
        let mut bar = h_flex().gap(px(2.)).items_center();

        // H1 / H2 / H3
        for level in [1u8, 2, 3] {
            let active = snap.heading_level == Some(level);
            let ed = editor.clone();
            bar = bar.child(
                div()
                    .id(ElementId::NamedInteger("tb-h".into(), level as u64))
                    .px(px(8.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .bg(if active {
                        active_bg
                    } else {
                        hsla(0., 0., 0., 0.)
                    })
                    .hover(|t| t.bg(hover_bg))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if active { active_fg } else { idle_fg })
                    .child(format!("H{level}"))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        let _ = ed.update(cx, |e, cx| {
                            e.toolbar_apply(ToolbarAction::Heading(level), w, cx)
                        });
                        cx.notify();
                    })),
            );
        }
        bar = bar.child(toolbar_divider(&theme));

        // B / I / S / U (glyphs decorated via the format itself)
        let inline_buttons: [(
            char,
            ToolbarAction,
            ShortcutCommand,
            fn(gpui::Div) -> gpui::Div,
        ); 4] = [
            (
                'B',
                ToolbarAction::Bold,
                ShortcutCommand::BoldSelection,
                |d: gpui::Div| d.font_weight(FontWeight::BOLD),
            ),
            (
                'I',
                ToolbarAction::Italic,
                ShortcutCommand::ItalicSelection,
                |d: gpui::Div| d.italic(),
            ),
            (
                'U',
                ToolbarAction::Underline,
                ShortcutCommand::UnderlineSelection,
                |d: gpui::Div| d.underline(),
            ),
            (
                'S',
                ToolbarAction::Strikethrough,
                ShortcutCommand::StrikethroughSelection,
                |d: gpui::Div| d.line_through(),
            ),
        ];
        let style_active = |a: &ToolbarAction| match a {
            ToolbarAction::Bold => snap.style.bold,
            ToolbarAction::Italic => snap.style.italic,
            ToolbarAction::Underline => snap.style.underline,
            ToolbarAction::Strikethrough => snap.style.strikethrough,
            _ => false,
        };
        for (glyph, action, _cmd, decorate) in inline_buttons {
            let active = style_active(&action);
            let ed = editor.clone();
            bar = bar.child(
                div()
                    .id(ElementId::Name(format!("tb-fmt-{glyph}").into()))
                    .w(px(28.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(if active {
                        active_bg
                    } else {
                        hsla(0., 0., 0., 0.)
                    })
                    .hover(|t| t.bg(hover_bg))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(if active { active_fg } else { idle_fg })
                    .child(decorate(div()).child(glyph.to_string()))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        let _ = ed.update(cx, |e, cx| e.toolbar_apply(action.clone(), w, cx));
                        cx.notify();
                    })),
            );
        }
        bar = bar.child(toolbar_divider(&theme));

        // Additional flat buttons: ordered list, bullet list, quote, code block,
        // table, image, clear formatting. Rendered as custom SVG icons (see
        // assets/verve_icons/), inheriting the button's text color.
        let flat_buttons: [(&str, &str, ToolbarAction); 7] = [
            (
                crate::assets::TB_LIST_ORDERED,
                "ordered",
                ToolbarAction::NumberedList,
            ),
            (
                crate::assets::TB_LIST,
                "bullet",
                ToolbarAction::BulletedList,
            ),
            (crate::assets::TB_QUOTE, "quote", ToolbarAction::Quote),
            (crate::assets::TB_CODE, "code", ToolbarAction::CodeBlock),
            (crate::assets::TB_TABLE, "table", ToolbarAction::InsertTable),
            (crate::assets::TB_IMAGE, "image", ToolbarAction::InsertImage),
            (
                crate::assets::TB_ERASER,
                "clear",
                ToolbarAction::ClearFormatting,
            ),
        ];
        let block_kind = &snap.block_kind;
        let flat_active = |a: &ToolbarAction| match a {
            ToolbarAction::NumberedList => {
                matches!(block_kind, Some(BlockKindSnapshot::NumberedList))
            }
            ToolbarAction::BulletedList => {
                matches!(block_kind, Some(BlockKindSnapshot::BulletedList))
            }
            ToolbarAction::Quote => matches!(block_kind, Some(BlockKindSnapshot::Quote)),
            ToolbarAction::CodeBlock => matches!(block_kind, Some(BlockKindSnapshot::CodeBlock)),
            _ => false,
        };
        for (icon_path, id_label, action) in flat_buttons {
            let active = flat_active(&action);
            let ed = editor.clone();
            bar = bar.child(
                div()
                    .id(ElementId::Name(format!("tb-flat-{id_label}").into()))
                    .px(px(6.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(if active {
                        active_bg
                    } else {
                        hsla(0., 0., 0., 0.)
                    })
                    .hover(|t| t.bg(hover_bg))
                    .cursor_pointer()
                    .text_color(if active { active_fg } else { idle_fg })
                    .child(
                        gpui_component::Icon::from(gpui_component::IconName::Redo)
                            .path(icon_path)
                            .small(),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        let _ = ed.update(cx, |e, cx| e.toolbar_apply(action.clone(), w, cx));
                        cx.notify();
                    })),
            );
        }
        bar = bar.child(toolbar_divider(&theme));

        // Paragraph / Format dropdown triggers
        for (label, dropdown) in [
            (
                rust_i18n::t!("toolbar.paragraph").to_string(),
                ToolbarDropdown::Paragraph,
            ),
            (
                rust_i18n::t!("toolbar.format").to_string(),
                ToolbarDropdown::Format,
            ),
        ] {
            let is_open = open == Some(dropdown);
            let ed = editor.clone();
            bar = bar.child(
                div()
                    .id(ElementId::Name(format!("tb-dd-{label}").into()))
                    .px(px(8.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(4.))
                    .bg(if is_open {
                        hover_bg
                    } else {
                        hsla(0., 0., 0., 0.)
                    })
                    .hover(|t| t.bg(hover_bg))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(idle_fg)
                    .child(label.to_string())
                    .child(
                        div()
                            .text_size(px(10.))
                            .child(if is_open { "▲" } else { "▼" }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let _ = ed.update(cx, |e, cx| {
                                e.toolbar_toggle_dropdown(dropdown, f32::from(event.position.x), cx)
                            });
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        // The button bar only. The dropdown panel + dismiss overlay are rendered
        // separately at the root level (see render_editor_dropdown_overlay) so
        // the title bar's 40px height does not clip them.
        let _ = open;
        bar.into_any_element()
    }

    pub(super) fn render_editor_dropdown_overlay(
        &mut self,
        editor: &Entity<crate::wysiwyg::editor::Editor>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let snap = editor.read(cx).toolbar_snapshot(cx);
        let open = editor.read(cx).toolbar_open_dropdown()?;
        let anchor_x = editor.read(cx).dropdown_anchor_x().unwrap_or(260.0);
        let panel = match open {
            ToolbarDropdown::Paragraph => self.render_paragraph_panel(editor, &snap, &theme, cx),
            ToolbarDropdown::Format => self.render_format_panel(editor, &snap, &theme, cx),
        };
        let ed = editor.clone();
        Some(
            div()
                .id("tb-dropdown-overlay")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _, _, cx| {
                        let _ = ed.update(cx, |e, cx| e.close_toolbar_dropdown(cx));
                    }),
                )
                // Anchor the panel just under the title bar, beneath the trigger.
                .child(
                    div()
                        .absolute()
                        .top(px(40.))
                        .left(px(anchor_x))
                        .child(panel),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_paragraph_panel(
        &mut self,
        editor: &Entity<crate::wysiwyg::editor::Editor>,
        snap: &ToolbarSnapshot,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::wysiwyg::components::actions::ShortcutCommand;
        use crate::wysiwyg::editor::toolbar::BlockKindSnapshot;

        let level = snap.heading_level;
        let kind = snap.block_kind.as_ref();
        let mk = |level: u8| match level {
            1 => ShortcutCommand::Heading1,
            2 => ShortcutCommand::Heading2,
            3 => ShortcutCommand::Heading3,
            4 => ShortcutCommand::Heading4,
            5 => ShortcutCommand::Heading5,
            _ => ShortcutCommand::Heading6,
        };
        let items: Vec<(String, bool, Option<AnyElement>, bool, ToolbarAction)> = vec![
            (
                rust_i18n::t!("toolbar.heading_1").to_string(),
                level == Some(1),
                command_keycap(mk(1)),
                false,
                ToolbarAction::Heading(1),
            ),
            (
                rust_i18n::t!("toolbar.heading_2").to_string(),
                level == Some(2),
                command_keycap(mk(2)),
                false,
                ToolbarAction::Heading(2),
            ),
            (
                rust_i18n::t!("toolbar.heading_3").to_string(),
                level == Some(3),
                command_keycap(mk(3)),
                false,
                ToolbarAction::Heading(3),
            ),
            (
                rust_i18n::t!("toolbar.heading_4").to_string(),
                level == Some(4),
                command_keycap(mk(4)),
                false,
                ToolbarAction::Heading(4),
            ),
            (
                rust_i18n::t!("toolbar.heading_5").to_string(),
                level == Some(5),
                command_keycap(mk(5)),
                false,
                ToolbarAction::Heading(5),
            ),
            (
                rust_i18n::t!("toolbar.heading_6").to_string(),
                level == Some(6),
                command_keycap(mk(6)),
                false,
                ToolbarAction::Heading(6),
            ),
            ("".into(), false, None, false, ToolbarAction::Paragraph), // separator marker
            (
                rust_i18n::t!("toolbar.table").to_string(),
                false,
                command_keycap(ShortcutCommand::InsertTable),
                false,
                ToolbarAction::InsertTable,
            ),
            (
                rust_i18n::t!("toolbar.code_fences").to_string(),
                matches!(kind, Some(BlockKindSnapshot::CodeBlock)),
                command_keycap(ShortcutCommand::ConvertToCodeBlock),
                false,
                ToolbarAction::CodeBlock,
            ),
            (
                rust_i18n::t!("toolbar.quote_block").to_string(),
                matches!(kind, Some(BlockKindSnapshot::Quote)),
                command_keycap(ShortcutCommand::ConvertToQuote),
                false,
                ToolbarAction::Quote,
            ),
            (
                rust_i18n::t!("toolbar.math_block").to_string(),
                matches!(kind, Some(BlockKindSnapshot::MathBlock)),
                None,
                false,
                ToolbarAction::InsertMathBlock,
            ),
            (
                rust_i18n::t!("toolbar.html_block").to_string(),
                matches!(kind, Some(BlockKindSnapshot::HtmlBlock)),
                None,
                false,
                ToolbarAction::InsertHtmlBlock,
            ),
            ("".into(), false, None, false, ToolbarAction::Paragraph), // sep
            (
                rust_i18n::t!("toolbar.ordered_list").to_string(),
                matches!(kind, Some(BlockKindSnapshot::NumberedList)),
                command_keycap(ShortcutCommand::ConvertToNumberedList),
                false,
                ToolbarAction::NumberedList,
            ),
            (
                rust_i18n::t!("toolbar.bullet_list").to_string(),
                matches!(kind, Some(BlockKindSnapshot::BulletedList)),
                command_keycap(ShortcutCommand::ConvertToBulletedList),
                false,
                ToolbarAction::BulletedList,
            ),
            (
                rust_i18n::t!("toolbar.task_list").to_string(),
                matches!(kind, Some(BlockKindSnapshot::TaskList)),
                command_keycap(ShortcutCommand::ConvertToTaskList),
                false,
                ToolbarAction::TaskList,
            ),
            ("".into(), false, None, false, ToolbarAction::Paragraph), // sep
            (
                rust_i18n::t!("toolbar.paragraph_restore").to_string(),
                matches!(kind, Some(BlockKindSnapshot::Paragraph)),
                command_keycap(ShortcutCommand::ConvertToParagraph),
                false,
                ToolbarAction::Paragraph,
            ),
            (
                rust_i18n::t!("toolbar.horizontal_rule").to_string(),
                matches!(kind, Some(BlockKindSnapshot::Separator)),
                command_keycap(ShortcutCommand::InsertHorizontalRule),
                false,
                ToolbarAction::InsertHorizontalRule,
            ),
        ];

        let mut rows: Vec<AnyElement> = Vec::new();
        for (label, checked, keycap, disabled, action) in items {
            if label.is_empty() {
                rows.push(dropdown_separator(theme));
            } else {
                rows.push(self.render_dropdown_item(
                    editor, &label, checked, keycap, disabled, action, theme, cx,
                ));
            }
        }
        dropdown_panel("paragraph-dropdown", rows, theme)
    }

    pub(super) fn render_format_panel(
        &mut self,
        editor: &Entity<crate::wysiwyg::editor::Editor>,
        snap: &ToolbarSnapshot,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::wysiwyg::components::actions::ShortcutCommand;
        use crate::wysiwyg::components::markdown::inline::InlineScript;

        let style = &snap.style;
        let items: Vec<(String, bool, Option<AnyElement>, bool, ToolbarAction)> = vec![
            (
                rust_i18n::t!("toolbar.bold").to_string(),
                style.bold,
                command_keycap(ShortcutCommand::BoldSelection),
                false,
                ToolbarAction::Bold,
            ),
            (
                rust_i18n::t!("toolbar.italic").to_string(),
                style.italic,
                command_keycap(ShortcutCommand::ItalicSelection),
                false,
                ToolbarAction::Italic,
            ),
            (
                rust_i18n::t!("toolbar.underline").to_string(),
                style.underline,
                command_keycap(ShortcutCommand::UnderlineSelection),
                false,
                ToolbarAction::Underline,
            ),
            (
                rust_i18n::t!("toolbar.strikethrough").to_string(),
                style.strikethrough,
                command_keycap(ShortcutCommand::StrikethroughSelection),
                false,
                ToolbarAction::Strikethrough,
            ),
            (
                "".into(),
                false,
                None,
                false,
                ToolbarAction::ClearFormatting,
            ), // sep
            (
                rust_i18n::t!("toolbar.superscript").to_string(),
                style.script == InlineScript::Superscript,
                command_keycap(ShortcutCommand::SuperscriptSelection),
                false,
                ToolbarAction::Superscript,
            ),
            (
                rust_i18n::t!("toolbar.subscript").to_string(),
                style.script == InlineScript::Subscript,
                command_keycap(ShortcutCommand::SubscriptSelection),
                false,
                ToolbarAction::Subscript,
            ),
            (
                rust_i18n::t!("toolbar.highlight").to_string(),
                style.highlight,
                command_keycap(ShortcutCommand::HighlightSelection),
                false,
                ToolbarAction::Highlight,
            ),
            (
                "".into(),
                false,
                None,
                false,
                ToolbarAction::ClearFormatting,
            ), // sep
            (
                rust_i18n::t!("toolbar.inline_code").to_string(),
                style.code,
                command_keycap(ShortcutCommand::CodeSelection),
                false,
                ToolbarAction::InlineCode,
            ),
            (
                rust_i18n::t!("toolbar.inline_math").to_string(),
                false,
                None,
                false,
                ToolbarAction::InlineMath,
            ),
            (
                "".into(),
                false,
                None,
                false,
                ToolbarAction::ClearFormatting,
            ), // sep
            (
                rust_i18n::t!("toolbar.hyperlink").to_string(),
                false,
                command_keycap(ShortcutCommand::InsertLink),
                false,
                ToolbarAction::InsertLink,
            ),
            (
                rust_i18n::t!("toolbar.image").to_string(),
                false,
                command_keycap(ShortcutCommand::InsertImage),
                false,
                ToolbarAction::InsertImage,
            ),
            (
                "".into(),
                false,
                None,
                false,
                ToolbarAction::ClearFormatting,
            ), // sep
            (
                rust_i18n::t!("toolbar.clear_formatting").to_string(),
                false,
                None,
                false,
                ToolbarAction::ClearFormatting,
            ),
        ];

        let mut rows: Vec<AnyElement> = Vec::new();
        for (label, checked, keycap, disabled, action) in items {
            if label.is_empty() {
                rows.push(dropdown_separator(theme));
            } else {
                rows.push(self.render_dropdown_item(
                    editor, &label, checked, keycap, disabled, action, theme, cx,
                ));
            }
        }
        dropdown_panel("format-dropdown", rows, theme)
    }

    pub(super) fn render_dropdown_item(
        &mut self,
        editor: &Entity<crate::wysiwyg::editor::Editor>,
        label: &str,
        checked: bool,
        keycap: Option<AnyElement>,
        disabled: bool,
        action: ToolbarAction,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let fg = theme.foreground;
        let hover_bg = theme.muted;
        let mut row = div()
            .id(ElementId::Name(format!("tb-item-{label}").into()))
            .w_full()
            .h(px(28.))
            .px(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .rounded(px(4.));
        if disabled {
            row = row.opacity(0.4);
        } else {
            row = row.hover(|t| t.bg(hover_bg)).cursor_pointer();
        }
        row = row.child(
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .w(px(14.))
                        .text_color(accent)
                        .child(if checked { "✓" } else { "" }),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(fg)
                        .child(label.to_string()),
                ),
        );
        row = row.child(h_flex().gap(px(2.)).when_some(keycap, |t, k| t.child(k)));
        if disabled {
            row.into_any_element()
        } else {
            let editor = editor.clone();
            row.on_click(cx.listener(move |this, _, w, cx| {
                let _ = editor.update(cx, |e, cx| e.toolbar_apply(action.clone(), w, cx));
                cx.notify();
            }))
            .into_any_element()
        }
    }
}
