//! Markdown deserialization tests (extracted from document.rs).

#![cfg(all(test, feature = "wysiwyg-gpui-tests"))]

use gpui::{AppContext, TestAppContext};

use super::{
    collect_block_html_region, find_matching_closing_fence, is_closing_fence,
    is_reference_definition_start, parse_list_marker, parse_opening_fence,
    strip_indented_code_prefix, strip_one_quote_level,
};
use crate::wysiwyg::components::{BlockKind, CalloutVariant, HtmlCssColor};
use crate::wysiwyg::editor::Editor;

    fn closing_fence_must_match_exact_opening_run_length() {
        let opener = parse_opening_fence("````rust").expect("opening fence");

        assert!(is_closing_fence("````", &opener));
        assert!(is_closing_fence("  ````   ", &opener));
        assert!(!is_closing_fence("```", &opener));
        assert!(!is_closing_fence("`````", &opener));
    }

    #[test]
    fn fence_detection_rejects_indent_beyond_three_spaces() {
        assert!(parse_opening_fence("    ```rust").is_none());

        let opener = parse_opening_fence("```rust").expect("opening fence");
        assert!(!is_closing_fence("    ```", &opener));
    }

    #[test]
    fn unmatched_opening_fence_does_not_form_code_block() {
        let lines = vec![
            "```rust".to_string(),
            "fn main() {}".to_string(),
            "plain tail".to_string(),
        ];
        let opener = parse_opening_fence(&lines[0]).expect("opening fence");
        assert_eq!(find_matching_closing_fence(&lines, 0, &opener), None);
    }

    #[test]
    fn matching_closing_fence_can_skip_inner_non_closing_backtick_runs() {
        let lines = vec![
            "```rust".to_string(),
            "````".to_string(),
            "body".to_string(),
            "```".to_string(),
        ];
        let opener = parse_opening_fence(&lines[0]).expect("opening fence");
        assert_eq!(find_matching_closing_fence(&lines, 0, &opener), Some(3));
    }

    #[test]
    fn fence_closes_at_first_match_even_before_a_later_opener() {
        // The first closing fence ends the block; later fences belong to
        // whatever follows, not to this block (issue #58).
        let lines = vec![
            "```rust".to_string(),
            "```".to_string(),
            "body".to_string(),
            "```".to_string(),
            "```ts".to_string(),
        ];
        let opener = parse_opening_fence(&lines[0]).expect("opening fence");
        assert_eq!(find_matching_closing_fence(&lines, 0, &opener), Some(1));
    }

    #[test]
    fn empty_language_fence_closes_at_first_match() {
        // Adjacent empty-language blocks must stay separate rather than the
        // first absorbing the second's fences as body content (issue #58).
        let lines = vec![
            "```".to_string(),
            "first".to_string(),
            "```".to_string(),
            "```".to_string(),
            "second".to_string(),
            "```".to_string(),
        ];
        let opener = parse_opening_fence(&lines[0]).expect("opening fence");
        assert_eq!(find_matching_closing_fence(&lines, 0, &opener), Some(2));
    }

    #[test]
    fn info_tagged_fence_does_not_absorb_following_empty_blocks() {
        // An info-string opener must still close at its own fence instead of
        // swallowing later empty-language blocks (issue #58).
        let lines = vec![
            "```bash".to_string(),
            "git clone url".to_string(),
            "```".to_string(),
            "```".to_string(),
            "cargo build".to_string(),
            "```".to_string(),
        ];
        let opener = parse_opening_fence(&lines[0]).expect("opening fence");
        assert_eq!(find_matching_closing_fence(&lines, 0, &opener), Some(2));
    }

    #[test]
    fn next_opening_without_prior_closing_leaves_fence_unmatched() {
        let lines = vec![
            "```rust".to_string(),
            "body".to_string(),
            "```ts".to_string(),
            "```".to_string(),
        ];
        let opener = parse_opening_fence(&lines[0]).expect("opening fence");
        assert_eq!(find_matching_closing_fence(&lines, 0, &opener), None);
    }

    #[test]
    fn parses_indented_code_blocks() {
        assert_eq!(strip_indented_code_prefix("    code"), Some("code"));
        assert_eq!(strip_indented_code_prefix("\tcode"), Some("code"));
        assert_eq!(strip_indented_code_prefix("  code"), None);
    }

    #[test]
    fn parses_original_unordered_list_markers() {
        assert_eq!(
            parse_list_marker("- item").unwrap().kind,
            BlockKind::BulletedListItem
        );
        assert_eq!(
            parse_list_marker("* item").unwrap().kind,
            BlockKind::BulletedListItem
        );
        assert_eq!(
            parse_list_marker("+ item").unwrap().kind,
            BlockKind::BulletedListItem
        );
        assert_eq!(
            parse_list_marker("- [ ] item").unwrap().kind,
            BlockKind::TaskListItem { checked: false }
        );
        assert_eq!(
            parse_list_marker("* [x] item").unwrap().kind,
            BlockKind::TaskListItem { checked: true }
        );
        assert_eq!(
            parse_list_marker("+ [X] item").unwrap().kind,
            BlockKind::TaskListItem { checked: true }
        );
    }

    #[test]
    fn parses_commonmark_ordered_list_markers() {
        let dot = parse_list_marker("1. item").expect("dot marker");
        assert_eq!(dot.kind, BlockKind::NumberedListItem);
        assert_eq!(dot.text, "item");
        assert_eq!(dot.content_indent_columns, 3);

        let paren = parse_list_marker("12) item").expect("paren marker");
        assert_eq!(paren.kind, BlockKind::NumberedListItem);
        assert_eq!(paren.text, "item");
        assert_eq!(paren.content_indent_columns, 4);

        let tab = parse_list_marker("1)\titem").expect("tab separator");
        assert_eq!(tab.kind, BlockKind::NumberedListItem);
        assert_eq!(tab.text, "item");
        assert_eq!(tab.content_indent_columns, 4);

        assert!(parse_list_marker("1)item").is_none());
        assert!(parse_list_marker("1234567890) item").is_none());
    }

    #[test]
    fn strips_one_quote_level_per_line() {
        assert_eq!(strip_one_quote_level("> quote"), Some("quote".to_string()));
        assert_eq!(
            strip_one_quote_level("   > quote"),
            Some("quote".to_string())
        );
        assert_eq!(
            strip_one_quote_level(">> nested"),
            Some("> nested".to_string())
        );
    }

    #[test]
    fn recognizes_reference_definition_lines() {
        assert!(is_reference_definition_start("[id]: http://example.com"));
        assert!(is_reference_definition_start(
            "   [id]: <http://example.com/>"
        ));
        assert!(!is_reference_definition_start("[id] http://example.com"));
    }

    #[test]
    fn block_html_region_runs_until_blank_line() {
        let lines = vec![
            "<table>".to_string(),
            "<tr><td>x</td></tr>".to_string(),
            "</table>".to_string(),
            "".to_string(),
            "tail".to_string(),
        ];
        assert_eq!(collect_block_html_region(&lines, 0), 3);
    }

    #[gpui::test]
    async fn imports_setext_headings_and_grouped_paragraphs(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "Heading\n-------\n\nfirst line\nsecond line".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Heading { level: 2 }
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "Heading");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "first line\nsecond line"
            );
            assert_eq!(
                editor.document.markdown_text(cx),
                "## Heading\n\nfirst line\nsecond line"
            );
        });
    }

    #[gpui::test]
    async fn imports_indented_code_blocks_and_serializes_fenced(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "    let x = 1;\n    println!(\"hi\");".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert!(visible[0].entity.read(cx).kind().is_code_block());
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "let x = 1;\nprintln!(\"hi\");"
            );
            assert_eq!(
                editor.document.markdown_text(cx),
                "```\nlet x = 1;\nprintln!(\"hi\");\n```"
            );
        });
    }

    #[gpui::test]
    async fn imports_consecutive_code_blocks_without_merging(cx: &mut TestAppContext) {
        // An info-tagged block followed by language-less blocks: each must
        // parse as its own code block rather than being merged (issue #58).
        let source = "```bash\ngit clone url\n```\n\n```\ncargo build\n```\n\n```\nmake\n```";
        let editor = cx.new(|cx| Editor::from_markdown(cx, source.to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            let code_blocks: Vec<_> = visible
                .iter()
                .filter(|block| block.entity.read(cx).kind().is_code_block())
                .collect();
            assert_eq!(code_blocks.len(), 3);
            assert_eq!(
                code_blocks[0].entity.read(cx).display_text(),
                "git clone url"
            );
            assert_eq!(code_blocks[1].entity.read(cx).display_text(), "cargo build");
            assert_eq!(code_blocks[2].entity.read(cx).display_text(), "make");
        });
    }

    #[gpui::test]
    async fn preserves_hard_break_spaces_in_paragraph_round_trip(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "alpha  \nbeta".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha  \nbeta");
            assert_eq!(editor.document.markdown_text(cx), "alpha  \nbeta");

            editor.toggle_view_mode(cx);
            editor.toggle_view_mode(cx);

            let visible = editor.document.visible_blocks();
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha  \nbeta");
            assert_eq!(editor.document.markdown_text(cx), "alpha  \nbeta");
        });
    }

    #[gpui::test]
    async fn preserves_tibetan_spaces_in_paragraph_round_trip(cx: &mut TestAppContext) {
        let tibetan = "༄༅།།དཔལ་ལྡན་རྩ་བའི་བླ་མ་རིན་པོ་ཆེ།། བདག་གི་སྤྱི་བོར་པདྨའི་གདན་བཞུགས་ནས།། ";
        let editor = cx.new(|cx| Editor::from_markdown(cx, tibetan.to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).display_text(), tibetan);
            assert!(visible[0].entity.read(cx).display_text().contains("།། བདག"));
            assert!(visible[0].entity.read(cx).display_text().ends_with(' '));
            assert_eq!(editor.document.markdown_text(cx), tibetan);

            editor.toggle_view_mode(cx);
            editor.toggle_view_mode(cx);

            let visible = editor.document.visible_blocks();
            assert_eq!(visible[0].entity.read(cx).display_text(), tibetan);
            assert_eq!(editor.document.markdown_text(cx), tibetan);
        });
    }

    #[gpui::test]
    async fn preserves_chinese_spaces_in_paragraph_round_trip(cx: &mut TestAppContext) {
        let chinese = "中文 文本 ";
        let editor = cx.new(|cx| Editor::from_markdown(cx, chinese.to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).display_text(), chinese);
            assert_eq!(editor.document.markdown_text(cx), chinese);
        });
    }

    #[gpui::test]
    async fn preserves_hard_break_spaces_in_simple_quote(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "> alpha  \n> beta".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha  \nbeta");
            assert_eq!(editor.document.markdown_text(cx), "> alpha  \n> beta");
        });
    }

    #[gpui::test]
    async fn preserves_hard_break_spaces_in_list_item_continuation(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "- alpha  \n  beta".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha  \nbeta");
            assert_eq!(editor.document.markdown_text(cx), "- alpha  \n  beta");
        });
    }

    #[gpui::test]
    async fn imports_nested_list_children_as_native_blocks(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "- parent\n  - nested bullet\n  - [x] nested task".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "parent");
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).display_text(), "nested bullet");
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::TaskListItem { checked: true }
            );
            assert_eq!(visible[2].entity.read(cx).display_text(), "nested task");
        });
    }

    #[gpui::test]
    async fn imports_indented_code_block_as_native_list_child(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "- item with code block\n\n      let x = 1;\n      let y = 2;".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::CodeBlock { language: None }
            );
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "let x = 1;\nlet y = 2;"
            );
            assert_eq!(
                editor.document.markdown_text(cx),
                "- item with code block\n  ```\n  let x = 1;\n  let y = 2;\n  ```"
            );

            editor.toggle_view_mode(cx);
            editor.toggle_view_mode(cx);

            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::CodeBlock { language: None }
            );
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "let x = 1;\nlet y = 2;"
            );
        });
    }

    #[gpui::test]
    async fn imports_fenced_code_block_as_native_list_child(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "- item with fenced code\n  ```rust\n  fn main() {}\n  ```".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::CodeBlock {
                    language: Some("rust".into())
                }
            );
            assert_eq!(visible[1].entity.read(cx).display_text(), "fn main() {}");
        });
    }

    #[gpui::test]
    async fn imports_simple_quote_as_native_list_child(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "1. item with nested quote\n\n   > quoted text\n   >\n   > quoted paragraph two"
                    .to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "item with nested quote"
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "quoted text\n\nquoted paragraph two"
            );
            assert_eq!(
                editor.document.markdown_text(cx),
                "1. item with nested quote\n  > quoted text\n  > \n  > quoted paragraph two"
            );

            editor.toggle_view_mode(cx);
            editor.toggle_view_mode(cx);

            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "quoted text\n\nquoted paragraph two"
            );
        });
    }

    #[gpui::test]
    async fn separated_numbered_list_runs_restart_at_one_after_blank_line(cx: &mut TestAppContext) {
        let editor = cx
            .new(|cx| Editor::from_markdown(cx, "1. aa\n2. bb\n3. cc\n\n1. dd".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 5);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(visible[0].entity.read(cx).list_ordinal, Some(1));
            assert_eq!(visible[1].entity.read(cx).list_ordinal, Some(2));
            assert_eq!(visible[2].entity.read(cx).list_ordinal, Some(3));
            assert_eq!(visible[3].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[3].entity.read(cx).display_text(), "");
            assert_eq!(
                visible[4].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(visible[4].entity.read(cx).display_text(), "dd");
            assert_eq!(visible[4].entity.read(cx).list_ordinal, Some(1));
            assert_eq!(
                editor.document.markdown_text(cx),
                "1. aa\n2. bb\n3. cc\n\n1. dd"
            );
        });
    }

    #[gpui::test]
    async fn imports_parenthesized_ordered_lists_and_serializes_canonical_dot_markers(
        cx: &mut TestAppContext,
    ) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "1) one\n2) two".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "one");
            assert_eq!(visible[1].entity.read(cx).display_text(), "two");
            assert_eq!(visible[0].entity.read(cx).list_ordinal, Some(1));
            assert_eq!(visible[1].entity.read(cx).list_ordinal, Some(2));
            assert_eq!(editor.document.markdown_text(cx), "1. one\n2. two");
        });
    }

    #[gpui::test]
    async fn imports_nested_parenthesized_ordered_list_children(cx: &mut TestAppContext) {
        let editor =
            cx.new(|cx| Editor::from_markdown(cx, "1) parent\n   1) child".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "parent");
            assert_eq!(visible[1].entity.read(cx).display_text(), "child");
            assert_eq!(visible[1].entity.read(cx).render_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), "1. parent\n  1. child");
        });
    }

    #[gpui::test]
    async fn imports_nested_quotes_as_native_blocks(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(cx, "> level1\n>> level2\n>>> level3".to_string(), None)
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "level1");
            assert_eq!(visible[0].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[1].entity.read(cx).display_text(), "level2");
            assert_eq!(visible[1].entity.read(cx).quote_depth, 2);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[2].entity.read(cx).display_text(), "level3");
            assert_eq!(visible[2].entity.read(cx).quote_depth, 3);
            assert_eq!(
                editor.document.markdown_text(cx),
                "> level1\n> > level2\n> > > level3"
            );
        });
    }

    #[gpui::test]
    async fn literal_blank_line_splits_quote_groups(cx: &mut TestAppContext) {
        let editor =
            cx.new(|cx| Editor::from_markdown(cx, "> first\n\n> second".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "first");
            assert_eq!(visible[0].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[1].entity.read(cx).display_text(), "second");
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), "> first\n\n> second");
        });
    }

    #[gpui::test]
    async fn quoted_blank_line_stays_inside_same_quote_group(cx: &mut TestAppContext) {
        let editor =
            cx.new(|cx| Editor::from_markdown(cx, "> first\n>\n> second".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "first\n\nsecond");
            assert_eq!(editor.document.markdown_text(cx), "> first\n> \n> second");
        });
    }

    #[gpui::test]
    async fn imports_quote_with_list_children(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "> Quote with list:\n> - item 1\n> - [ ] task item".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "Quote with list:"
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).display_text(), "item 1");
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::TaskListItem { checked: false }
            );
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(
                editor.document.markdown_text(cx),
                "> Quote with list:\n> - item 1\n> - [ ] task item"
            );
        });
    }

    #[gpui::test]
    async fn imports_quote_with_code_block_child(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "> Quote with code block:\n>\n>     fn main() {\n>         println!(\"hi\");\n>     }"
                    .to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "Quote with code block:"
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::CodeBlock { language: None }
            );
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(
                visible[2].entity.read(cx).display_text(),
                "fn main() {\n    println!(\"hi\");\n}"
            );
            assert_eq!(
                editor.document.markdown_text(cx),
                "> Quote with code block:\n> \n> ```\n> fn main() {\n>     println!(\"hi\");\n> }\n> ```"
            );
        });
    }

    #[gpui::test]
    async fn imports_quote_with_standalone_image_child(cx: &mut TestAppContext) {
        let markdown = "> ![alt](./img.png)".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_bulleted_list_item_with_standalone_image_title(cx: &mut TestAppContext) {
        let markdown = "- ![alt](./img.png)".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert!(visible[0].entity.read(cx).children.is_empty());
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_list_item_with_standalone_image_child(cx: &mut TestAppContext) {
        let markdown = "- item\n  ![alt](./img.png)".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "item");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).render_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_list_image_title_with_native_child_paragraph(cx: &mut TestAppContext) {
        let markdown = "- ![alt](./img.png)\n  child text".to_string();
        let canonical_markdown = "- ![alt](./img.png)\n\n  child text";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "child text");
            assert_eq!(visible[1].entity.read(cx).render_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn imports_quote_with_numbered_list_image_item(cx: &mut TestAppContext) {
        let markdown = "> 1. ![alt](./img.png)".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::NumberedListItem
            );
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_callout_with_task_list_image_item_and_child(cx: &mut TestAppContext) {
        let markdown = "> [!NOTE]\n> - [ ] ![cover][img]\n>   ![detail](./detail.png)\n>\n> [img]: ./cover.png".to_string();
        let canonical_markdown = "> [!NOTE]\n> - [ ] ![cover][img]\n>   ![detail](./detail.png)\n> \n> [img]: ./cover.png";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Note)
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::TaskListItem { checked: false }
            );
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[1].entity.read(cx).callout_depth, 1);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[2].entity.read(cx).render_depth, 1);
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[2].entity.read(cx).callout_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn imports_callout_with_standalone_image_child(cx: &mut TestAppContext) {
        let markdown = "> [!NOTE]\n> ![alt](./img.png)".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Note)
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[1].entity.read(cx).callout_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_quote_list_item_with_native_child_paragraph(cx: &mut TestAppContext) {
        let markdown = "> - item\n>\n>     child text".to_string();
        let canonical_markdown = "> - item\n> \n>   child text";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[2].entity.read(cx).display_text(), "child text");
            assert_eq!(visible[2].entity.read(cx).render_depth, 1);
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn imports_callout_list_item_with_native_child_paragraph(cx: &mut TestAppContext) {
        let markdown = "> [!NOTE]\n> - item\n>\n>     child text".to_string();
        let canonical_markdown = "> [!NOTE]\n> - item\n> \n>   child text";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Note)
            );
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[1].entity.read(cx).callout_depth, 1);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[2].entity.read(cx).display_text(), "child text");
            assert_eq!(visible[2].entity.read(cx).render_depth, 1);
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[2].entity.read(cx).callout_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn quote_does_not_promote_multiline_image_paragraph_to_child(cx: &mut TestAppContext) {
        let markdown = "> ![alt](./img.png)\n> tail".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_callout_from_quote_header(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "> [!NOTE]".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Note)
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "");
            assert!(visible[0].entity.read(cx).children.is_empty());
            assert_eq!(visible[0].entity.read(cx).quote_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), "> [!NOTE]");
        });
    }

    #[gpui::test]
    async fn imports_important_callout_case_insensitively(cx: &mut TestAppContext) {
        let editor = cx
            .new(|cx| Editor::from_markdown(cx, "> [!important] Optional title".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Important)
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "Optional title");
            assert_eq!(
                editor.document.markdown_text(cx),
                "> [!IMPORTANT] Optional title"
            );
        });
    }

    #[gpui::test]
    async fn imports_callout_title_and_nested_quote_child(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "> [!WARNING] Custom title\n> body\n> > nested".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Warning)
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "Custom title");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "body");
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[2].entity.read(cx).display_text(), "nested");
            assert_eq!(visible[2].entity.read(cx).quote_depth, 2);
            assert_eq!(
                editor.document.markdown_text(cx),
                "> [!WARNING] Custom title\n> body\n> > nested"
            );
        });
    }

    #[gpui::test]
    async fn imports_callout_with_multiline_nested_quote_child(cx: &mut TestAppContext) {
        let markdown = [
            "> [!WARNING] Custom title",
            "> body",
            "> > inner one",
            "> >",
            "> > inner two",
            "> after",
        ]
        .join("\n");
        let canonical_markdown = [
            "> [!WARNING] Custom title",
            "> body",
            "> > inner one",
            "> > ",
            "> > inner two",
            "> after",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 4);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Warning)
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "body");
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[2].entity.read(cx).display_text(),
                "inner one\n\ninner two"
            );
            assert_eq!(visible[2].entity.read(cx).quote_depth, 2);
            assert!(
                visible[2]
                    .entity
                    .read(cx)
                    .visible_quote_group_anchor
                    .is_some()
            );
            assert_eq!(visible[3].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[3].entity.read(cx).display_text(), "after");
            assert_eq!(visible[3].entity.read(cx).quote_depth, 1);
            assert!(
                visible[3]
                    .entity
                    .read(cx)
                    .visible_quote_group_anchor
                    .is_none()
            );
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn unknown_callout_marker_stays_plain_quote(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "> [!UNKNOWN]".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "[!UNKNOWN]");
            assert_eq!(editor.document.markdown_text(cx), "> [!UNKNOWN]");
        });
    }

    #[gpui::test]
    async fn preserves_separator_between_quote_title_and_nested_child(cx: &mut TestAppContext) {
        let editor =
            cx.new(|cx| Editor::from_markdown(cx, "> outer\n>\n>> inner".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "outer");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[2].entity.read(cx).display_text(), "inner");
            assert_eq!(visible[2].entity.read(cx).quote_depth, 2);
            assert_eq!(editor.document.markdown_text(cx), "> outer\n> \n> > inner");
        });
    }

    #[gpui::test]
    async fn imports_quote_with_native_table_child(cx: &mut TestAppContext) {
        let markdown = "> Quote with table:\n> | A | B |\n> | --- | --- |\n> | 1 | 2 |".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "Quote with table:"
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Table);
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            let table = visible[1]
                .entity
                .read(cx)
                .record
                .table
                .as_ref()
                .expect("native nested table");
            assert_eq!(table.header.len(), 2);
            assert_eq!(table.rows.len(), 1);
            assert_eq!(table.rows[0].len(), 2);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn invalid_table_inside_quote_preserves_outer_quote_and_raw_child(
        cx: &mut TestAppContext,
    ) {
        let markdown = "> Quote with broken table:\n> | A |\n> | --- | --- |\n> | 1 |".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "Quote with broken table:"
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::RawMarkdown);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "| A |\n| --- | --- |\n| 1 |"
            );
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn final_mixed_mega_block_preserves_important_callout_with_native_table_and_native_footnote(
        cx: &mut TestAppContext,
    ) {
        let markdown = "> [!IMPORTANT]\n> Final mixed block that combines:\n>\n> - **bold**\n> - *italic*\n> - `inline code`\n> - [link](https://example.com)\n> - ![image](https://example.com/image.png)\n> - ~~strike~~\n>\n> And a table:\n>\n> | k | v |\n> | --- | --- |\n> | a | 1 |\n> | b | 2 |\n>\n> And a fenced code block:\n>\n> ```ts\n> export const answer = 42;\n> ```\n>\n> And a footnote reference.[^final]\n>\n> [^final]: Final footnote text with nested list:\n>   - one\n>   - two".to_string();
        let canonical_markdown = "> [!IMPORTANT]\n> Final mixed block that combines:\n> \n> - **bold**\n> - *italic*\n> - `inline code`\n> - [link](https://example.com)\n> - ![image](https://example.com/image.png)\n> - ~~strike~~\n> \n> And a table:\n> \n> | k | v |\n> | --- | --- |\n> | a | 1 |\n> | b | 2 |\n> \n> And a fenced code block:\n> \n> ```ts\n> export const answer = 42;\n> ```\n> \n> And a footnote reference.[^final]\n> \n> [^final]: Final footnote text with nested list:\n> \n>     - one\n>     - two";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Important)
            );
            assert!(visible.iter().any(|visible| {
                let block = visible.entity.read(cx);
                block.kind() == BlockKind::BulletedListItem && block.quote_depth == 1
            }));
            assert!(visible.iter().any(|visible| {
                let block = visible.entity.read(cx);
                block.kind()
                    == BlockKind::CodeBlock {
                        language: Some("ts".into()),
                    }
                    && block.display_text().contains("export const answer = 42;")
            }));
            assert!(visible.iter().any(|visible| {
                let block = visible.entity.read(cx);
                block.kind() == BlockKind::Table
                    && block.quote_depth == 1
                    && block.record.table.as_ref().is_some_and(|table| {
                        table.header.len() == 2
                            && table.rows.len() == 2
                            && table.header[0].serialize_markdown() == "k"
                            && table.rows[1][1].serialize_markdown() == "2"
                    })
            }));
            assert!(visible.iter().any(|visible| {
                let block = visible.entity.read(cx);
                block.kind() == BlockKind::Paragraph
                    && block.display_text().contains("And a table:")
                    && block.quote_depth == 1
            }));
            assert!(visible.iter().any(|visible| {
                let block = visible.entity.read(cx);
                block.kind() == BlockKind::FootnoteDefinition
                    && block.display_text() == "final"
                    && block.quote_depth == 1
            }));
            assert!(visible.iter().any(|visible| {
                let block = visible.entity.read(cx);
                block.kind() == BlockKind::Paragraph
                    && block.display_text() == "Final footnote text with nested list:"
                    && block.footnote_anchor.is_some()
                    && block.quote_depth == 1
            }));
            assert!(
                visible
                    .iter()
                    .filter(|visible| {
                        let block = visible.entity.read(cx);
                        block.kind() == BlockKind::BulletedListItem
                            && block.footnote_anchor.is_some()
                            && block.quote_depth == 1
                    })
                    .count()
                    >= 2
            );
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn unsupported_nested_block_preserves_native_list_item_with_raw_child(
        cx: &mut TestAppContext,
    ) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "- native before\n- raw item\n  <div>\n  inner\n  </div>\n- native after"
                    .to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 4);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "native before");
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).display_text(), "raw item");
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::HtmlBlock);
            assert!(visible[2].entity.read(cx).display_text().contains("<div>"));
            assert_eq!(
                visible[3].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[3].entity.read(cx).display_text(), "native after");
        });
    }

    #[gpui::test]
    async fn imports_and_canonicalizes_task_lists(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "- [ ] todo\n* [x] done\n+ [X] shipped".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::TaskListItem { checked: false }
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "todo");
            assert_eq!(
                visible[1].entity.read(cx).kind(),
                BlockKind::TaskListItem { checked: true }
            );
            assert_eq!(
                editor.document.markdown_text(cx),
                "- [ ] todo\n- [x] done\n- [x] shipped"
            );
        });
    }

    #[gpui::test]
    async fn parses_root_level_pipe_table_as_native_table(cx: &mut TestAppContext) {
        let markdown = "| A | B |\n| --- | --- |\n| 1 | 2 |".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Table);
            let table = visible[0]
                .entity
                .read(cx)
                .record
                .table
                .as_ref()
                .expect("native table data");
            assert_eq!(table.header.len(), 2);
            assert_eq!(table.rows.len(), 1);
            assert_eq!(table.rows[0].len(), 2);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn broken_root_level_table_degrades_to_plain_text_lines(cx: &mut TestAppContext) {
        let markdown = "| A | B |\n| nope | --- |\n| 1 | 2 |".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[0].entity.read(cx).display_text(), "| A | B |");
            assert_eq!(visible[1].entity.read(cx).display_text(), "| nope | --- |");
            assert_eq!(visible[2].entity.read(cx).display_text(), "| 1 | 2 |");
            assert_eq!(
                editor.document.markdown_text(cx),
                "| A | B |\n\n| nope | --- |\n\n| 1 | 2 |"
            );
        });
    }

    #[gpui::test]
    async fn imports_display_math_block_as_native_math_block(cx: &mut TestAppContext) {
        let markdown = "$$\n\\int_0^1 x^2 dx\n$$".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::MathBlock);
            assert_eq!(visible[0].entity.read(cx).display_text(), markdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_single_line_display_math_between_paragraphs(cx: &mut TestAppContext) {
        let markdown = "before\n$$x^2$$\nafter".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::MathBlock);
            assert_eq!(visible[1].entity.read(cx).display_text(), "$$x^2$$");
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(
                editor.document.markdown_text(cx),
                "before\n\n$$x^2$$\n\nafter"
            );
        });
    }

    #[gpui::test]
    async fn unclosed_display_math_stays_raw(cx: &mut TestAppContext) {
        let markdown = "$$\n\\int_0^1 x^2 dx".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::RawMarkdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_mermaid_fence_as_native_mermaid_block(cx: &mut TestAppContext) {
        let markdown = "before\n```mermaid\nflowchart LR\nA --> B\n```\nafter".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::MermaidBlock);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "```mermaid\nflowchart LR\nA --> B\n```"
            );
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(
                editor.document.markdown_text(cx),
                "before\n\n```mermaid\nflowchart LR\nA --> B\n```\n\nafter"
            );
        });
    }

    #[gpui::test]
    async fn imports_tilde_mmd_fence_as_native_mermaid_block(cx: &mut TestAppContext) {
        let markdown = "~~~MMD\nflowchart LR\nA --> B\n~~~".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::MermaidBlock);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn regular_fenced_code_is_not_mermaid(cx: &mut TestAppContext) {
        let markdown = "```rust\nfn main() {}\n```".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert!(matches!(
                visible[0].entity.read(cx).kind(),
                BlockKind::CodeBlock { .. }
            ));
        });
    }

    #[gpui::test]
    async fn imports_details_html_block_with_blank_lines_as_native_html_block(
        cx: &mut TestAppContext,
    ) {
        let markdown =
            "<details>\n<summary>Title</summary>\n\nHidden content with `code`.\n\n</details>"
                .to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::HtmlBlock);
            assert_eq!(visible[0].entity.read(cx).display_text(), markdown);
            assert!(
                visible[0]
                    .entity
                    .read(cx)
                    .record
                    .html
                    .as_ref()
                    .is_some_and(|html| html.is_semantic())
            );
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_safe_inline_html_line_as_native_html_block(cx: &mut TestAppContext) {
        let markdown = "<span style='color:blue;'>Anaconda</span>: https://example.com".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            let block = visible[0].entity.read(cx);
            assert_eq!(block.kind(), BlockKind::HtmlBlock);
            assert_eq!(block.display_text(), markdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_standalone_html_image_as_native_html_block(cx: &mut TestAppContext) {
        let markdown =
            "<img src=\"./assets/pic.png\" alt=\"alt text\" style=\"zoom:80%;\" />".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            let block = visible[0].entity.read(cx);
            assert_eq!(block.kind(), BlockKind::HtmlBlock);
            assert_eq!(block.display_text(), markdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn editable_html_wrapper_imports_and_round_trips(cx: &mut TestAppContext) {
        // A wrapper (<div align=center>…markdown…</div>) imports as a single
        // HtmlBlock whose disk serialization is the raw HTML. When focused it
        // surfaces its inner Markdown as N independent child blocks (see
        // Editor focus handling in html_wrapper_edit.rs); this test only guards
        // the import + serialization invariants the child-block layer relies on.
        let markdown = "<div align=\"center\">\n\n# Title\n\n**bold** text\n\n</div>".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            let block = visible[0].entity.read(cx);
            assert_eq!(block.kind(), BlockKind::HtmlBlock);
            // Unfocused display_text is the raw HTML.
            assert_eq!(block.display_text(), markdown);
            // Disk serialization round-trips the wrapper verbatim.
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn non_wrapper_html_block_stays_raw_html(cx: &mut TestAppContext) {
        // A standalone <img> block is not an editable wrapper; it stays a raw
        // HtmlBlock whose display/serialization is the raw HTML.
        let markdown = "<img src=\"a.png\" alt=\"a\" />".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let block = editor.document.visible_blocks()[0].entity.read(cx);
            assert_eq!(block.kind(), BlockKind::HtmlBlock);
            assert_eq!(block.display_text(), markdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_list_items_with_inline_span_style_as_text_not_links(cx: &mut TestAppContext) {
        let markdown = [
            "- Anaconda的安装需要留意<span style='color:blue;'>磁盘预留空间、系统环境变量</span>等问题",
            "- Pycharm的安装需要留意<span style='color:blue;'>专业版破解、python解释器关联</span>等问题",
            "- GPU版本的 Pytorch v1.5.0安装需要留意本机<span style='color:blue;'>英伟达驱动`CUDA+cuDNN`</span>",
        ]
        .join("\n");
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            for block in visible {
                assert_eq!(block.entity.read(cx).kind(), BlockKind::BulletedListItem);
            }

            let first = visible[0].entity.read(cx);
            let span_start = "Anaconda的安装需要留意".len();
            assert_eq!(first.inline_link_at(span_start), None);
            assert!(matches!(
                first
                    .inline_html_style_at(span_start)
                    .and_then(|style| style.color),
                Some(HtmlCssColor::Rgba(color))
                    if color.red == 0 && color.green == 0 && color.blue == 255
            ));
            assert_eq!(
                first.display_text(),
                "Anaconda的安装需要留意磁盘预留空间、系统环境变量等问题"
            );

            let third = visible[2].entity.read(cx);
            let code_start = "GPU版本的 Pytorch v1.5.0安装需要留意本机英伟达驱动".len();
            assert!(third.inline_style_at(code_start).code);
            assert_eq!(third.inline_link_at(code_start), None);
            assert!(third.inline_html_style_at(code_start).is_some());
        });
    }

    #[gpui::test]
    async fn risky_html_tag_stays_raw_markdown(cx: &mut TestAppContext) {
        let markdown = "<script>alert(1)</script>".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::RawMarkdown);
            assert_eq!(visible[0].entity.read(cx).display_text(), markdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn safe_html_with_risky_child_uses_html_block_and_preserves_source(
        cx: &mut TestAppContext,
    ) {
        let markdown = "<div>safe<script>alert(1)</script>tail</div>".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            let block = visible[0].entity.read(cx);
            assert_eq!(block.kind(), BlockKind::HtmlBlock);
            assert!(
                block
                    .record
                    .html
                    .as_ref()
                    .is_some_and(|html| html.is_semantic())
            );
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn imports_closed_html_comment_as_native_comment_block(cx: &mut TestAppContext) {
        let markdown = "<!--\n xxx \n-->".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Comment);
            assert_eq!(visible[0].entity.read(cx).display_text(), markdown);
            assert_eq!(editor.document.markdown_text(cx), markdown);
        });
    }

    #[gpui::test]
    async fn html_comment_closes_at_first_marker_and_resumes_block_parsing(
        cx: &mut TestAppContext,
    ) {
        let markdown = "before\n<!--\na\n--> trailing\n# after".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Comment);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "<!--\na\n--> trailing"
            );
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::Heading { level: 1 }
            );
            assert_eq!(visible[2].entity.read(cx).display_text(), "after");
            assert_eq!(
                editor.document.markdown_text(cx),
                "before\n\n<!--\na\n--> trailing\n\n# after"
            );
        });
    }

    #[gpui::test]
    async fn unclosed_html_comment_stays_raw_and_does_not_absorb_following_paragraph(
        cx: &mut TestAppContext,
    ) {
        let markdown = "<!--\na\n\nparagraph".to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::RawMarkdown);
            assert_eq!(visible[0].entity.read(cx).display_text(), "<!--\na");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "paragraph");
            assert_eq!(editor.document.markdown_text(cx), "<!--\na\n\nparagraph");
        });
    }

    #[gpui::test]
    async fn imports_comment_blocks_inside_list_quote_and_callout(cx: &mut TestAppContext) {
        let list_editor =
            cx.new(|cx| Editor::from_markdown(cx, "- item\n  <!--\n  list\n  -->".into(), None));
        list_editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Comment);
            assert_eq!(visible[1].entity.read(cx).display_text(), "<!--\nlist\n-->");
            assert_eq!(visible[1].entity.read(cx).render_depth, 1);
            assert_eq!(
                editor.document.markdown_text(cx),
                "- item\n  <!--\n  list\n  -->"
            );
        });

        let quote_editor = cx.new(|cx| {
            Editor::from_markdown(cx, "> quote\n>\n> <!--\n> quoted\n> -->".into(), None)
        });
        quote_editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Comment);
            assert_eq!(
                visible[2].entity.read(cx).display_text(),
                "<!--\nquoted\n-->"
            );
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(
                editor.document.markdown_text(cx),
                "> quote\n> \n> <!--\n> quoted\n> -->"
            );
        });

        let callout_editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "> [!NOTE] Title\n>\n> <!--\n> callout\n> -->".into(),
                None,
            )
        });
        callout_editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::Callout(CalloutVariant::Note)
            );
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Comment);
            assert_eq!(
                visible[2].entity.read(cx).display_text(),
                "<!--\ncallout\n-->"
            );
            assert_eq!(visible[2].entity.read(cx).callout_depth, 1);
            assert_eq!(
                editor.document.markdown_text(cx),
                "> [!NOTE] Title\n> \n> <!--\n> callout\n> -->"
            );
        });
    }

    #[gpui::test]
    async fn parses_multiline_root_footnote_definition_as_native_block(cx: &mut TestAppContext) {
        let markdown = "[^note]: Footnote text with **bold**\n    - item 1\n    - item 2\n\n    Second paragraph.".to_string();
        let canonical_markdown = "[^note]: Footnote text with **bold**\n\n    - item 1\n    - item 2\n\n    Second paragraph.";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown.clone(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 5);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::FootnoteDefinition
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "note");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(
                visible[1].entity.read(cx).display_text(),
                "Footnote text with bold"
            );
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[2].entity.read(cx).display_text(), "item 1");
            assert_eq!(
                visible[3].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[3].entity.read(cx).display_text(), "item 2");
            assert_eq!(visible[4].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(
                visible[4].entity.read(cx).display_text(),
                "Second paragraph."
            );
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn nested_quote_footnote_definition_upgrades_to_native_block(cx: &mut TestAppContext) {
        let markdown = "> outer\n>\n> [^note]: nested footnote".to_string();
        let canonical_markdown = "> outer\n> \n> [^note]: nested footnote";
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 4);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "outer");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[1].entity.read(cx).quote_depth, 1);
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::FootnoteDefinition
            );
            assert_eq!(visible[2].entity.read(cx).display_text(), "note");
            assert_eq!(visible[2].entity.read(cx).quote_depth, 1);
            assert_eq!(visible[3].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[3].entity.read(cx).display_text(), "nested footnote");
            assert_eq!(visible[3].entity.read(cx).quote_depth, 1);
            assert!(visible[3].entity.read(cx).footnote_anchor.is_some());
            assert_eq!(editor.document.markdown_text(cx), canonical_markdown);
        });
    }

    #[gpui::test]
    async fn test_md_fixture_keeps_mixed_supported_and_raw_sections_visible(
        cx: &mut TestAppContext,
    ) {
        let markdown = include_str!("../../../test.md").to_string();
        let editor = cx.new(|cx| Editor::from_markdown(cx, markdown, None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert!(visible.len() > 40);

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::Heading { level: 1 }
                    && block.display_text() == "Markdown Rendering Test Suite"
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::Quote
                    && block.display_text().contains("Blockquote paragraph one.")
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind().is_code_block()
                    && block
                        .display_text()
                        .contains("println!(\"fenced code block\");")
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::TaskListItem { checked: false }
                    && block.display_text().contains("Unchecked task")
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::BulletedListItem
                    && block.display_text() == "Mixed list item"
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind().is_code_block() && block.display_text().contains("let x = 1;")
            }));

            let multiline_code = visible
                .iter()
                .find(|block| {
                    block
                        .entity
                        .read(cx)
                        .display_text()
                        .starts_with("Code span across line breaks:")
                })
                .expect("multiline inline code sample")
                .entity
                .read(cx);
            assert!(multiline_code.display_text().contains("line 1\nline 2"));
            let multiline_prefix = "Code span across line breaks:\n".len();
            assert!(multiline_code.inline_spans().iter().any(|span| {
                span.style.code
                    && span.range == (multiline_prefix..multiline_prefix + "line 1\nline 2".len())
            }));

            let backtick_sample = visible
                .iter()
                .find(|block| {
                    block
                        .entity
                        .read(cx)
                        .display_text()
                        .starts_with("Backticks in normal text:")
                })
                .expect("literal backtick sample")
                .entity
                .read(cx);
            assert_eq!(
                backtick_sample.display_text(),
                "Backticks in normal text: ` and `` and ```"
            );
            let backtick_prefix = "Backticks in normal text: ".len();
            let expected_code_ranges = vec![
                backtick_prefix..backtick_prefix + 1,
                backtick_prefix + 6..backtick_prefix + 8,
                backtick_prefix + 13..backtick_prefix + 16,
            ];
            let actual_code_ranges = backtick_sample
                .inline_spans()
                .iter()
                .filter(|span| span.style.code)
                .map(|span| span.range.clone())
                .collect::<Vec<_>>();
            assert_eq!(actual_code_ranges, expected_code_ranges);
            assert!(!backtick_sample.inline_style_at(backtick_prefix + 2).code);
            assert!(!backtick_sample.inline_style_at(backtick_prefix + 9).code);

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::Quote
                    && block.display_text().contains("quoted paragraph two")
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::Table
                    && block
                        .record
                        .table
                        .as_ref()
                        .is_some_and(|table| table.header.len() == 3 && table.rows.len() >= 2)
            }));

            assert!(visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::HtmlBlock && block.display_text().contains("<details>")
            }));

            assert!(!visible.iter().any(|block| {
                let block = block.entity.read(cx);
                block.kind() == BlockKind::RawMarkdown
                    && block.display_text().contains("- Mixed list item")
            }));
        });
    }

    #[gpui::test]
    async fn list_followed_by_blank_line_and_root_paragraph_stays_separate(
        cx: &mut TestAppContext,
    ) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "- item\n\ntext".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "item");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "text");
            assert_eq!(visible[1].entity.read(cx).render_depth, 0);
        });
    }

    #[gpui::test]
    async fn mode_switch_preserves_root_paragraph_after_list(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "- item\n\ntext".to_string(), None));

        editor.update(cx, |editor, cx| {
            editor.toggle_view_mode(cx);
            assert!(matches!(editor.view_mode, super::super::ViewMode::Source));
            editor.toggle_view_mode(cx);
            assert!(matches!(editor.view_mode, super::super::ViewMode::Rendered));

            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "text");
            assert_eq!(visible[1].entity.read(cx).render_depth, 0);
        });
    }

    #[gpui::test]
    async fn list_empty_root_and_following_paragraph_stay_outside_list(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "- item\n\n\ntext".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[1].entity.read(cx).render_depth, 0);
            assert_eq!(visible[2].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[2].entity.read(cx).display_text(), "text");
            assert_eq!(visible[2].entity.read(cx).render_depth, 0);
        });
    }

    #[gpui::test]
    async fn blank_line_then_indented_text_upgrades_to_native_list_child_paragraph(
        cx: &mut TestAppContext,
    ) {
        let editor =
            cx.new(|cx| Editor::from_markdown(cx, "- item\n\n    child text".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(
                visible[0].entity.read(cx).kind(),
                BlockKind::BulletedListItem
            );
            assert_eq!(visible[0].entity.read(cx).display_text(), "item");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "child text");
            assert_eq!(visible[1].entity.read(cx).render_depth, 1);
            assert_eq!(editor.document.markdown_text(cx), "- item\n\n  child text");
        });
    }

    #[gpui::test]
    async fn preserves_reference_definitions_and_stops_quote_at_first_non_quoted_line(
        cx: &mut TestAppContext,
    ) {
        let reference_editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "[id]: http://example.com/\n    \"Title\"".to_string(),
                None,
            )
        });
        reference_editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::RawMarkdown);
            assert_eq!(
                editor.document.markdown_text(cx),
                "[id]: http://example.com/\n    \"Title\""
            );
        });

        let quote_editor =
            cx.new(|cx| Editor::from_markdown(cx, "> quoted\ncontinued".to_string(), None));
        quote_editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "quoted");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "continued");
            assert_eq!(editor.document.markdown_text(cx), "> quoted\n\ncontinued");
        });
    }

    #[gpui::test]
    async fn simple_quote_does_not_consume_following_root_blocks(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(
                cx,
                "> quoted line\n> second line\n\n---\n\n## Next".to_string(),
                None,
            )
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(
                visible[0].entity.read(cx).display_text(),
                "quoted line\nsecond line"
            );
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Separator);
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::Heading { level: 2 }
            );
            assert_eq!(visible[2].entity.read(cx).display_text(), "Next");
        });
    }

    #[gpui::test]
    async fn non_quoted_line_after_quote_becomes_plain_paragraph_before_heading(
        cx: &mut TestAppContext,
    ) {
        let editor = cx.new(|cx| {
            Editor::from_markdown(cx, "> quoted\ncontinued\n\n## Next".to_string(), None)
        });

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Quote);
            assert_eq!(visible[0].entity.read(cx).display_text(), "quoted");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "continued");
            assert_eq!(
                visible[2].entity.read(cx).kind(),
                BlockKind::Heading { level: 2 }
            );
            assert_eq!(visible[2].entity.read(cx).display_text(), "Next");
        });
    }

    #[gpui::test]
    async fn preserves_empty_root_blocks_across_round_trip(cx: &mut TestAppContext) {
        let editor =
            cx.new(|cx| Editor::from_markdown(cx, "alpha\n\n\nbeta\n\n".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 4);
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha");
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[2].entity.read(cx).display_text(), "beta");
            assert_eq!(visible[3].entity.read(cx).display_text(), "");
            assert_eq!(editor.document.markdown_text(cx), "alpha\n\n\nbeta\n\n");
        });

        editor.update(cx, |editor, cx| {
            editor.toggle_view_mode(cx);
            assert!(matches!(editor.view_mode, super::super::ViewMode::Source));
            editor.toggle_view_mode(cx);
            assert!(matches!(editor.view_mode, super::super::ViewMode::Rendered));

            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 4);
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha");
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[2].entity.read(cx).display_text(), "beta");
            assert_eq!(visible[3].entity.read(cx).display_text(), "");
        });
    }

    #[gpui::test]
    async fn imports_blank_line_inside_inline_code_as_single_paragraph(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "`line 1\n\nline 2`".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            let block = visible[0].entity.read(cx);
            let text = "line 1\n\nline 2";
            assert_eq!(block.kind(), BlockKind::Paragraph);
            assert_eq!(block.display_text(), text);
            assert!(
                block
                    .inline_spans()
                    .iter()
                    .any(|span| { span.style.code && span.range == (0..text.len()) })
            );
            assert_eq!(editor.document.markdown_text(cx), "`line 1\n\nline 2`");
        });
    }

    #[gpui::test]
    async fn unclosed_inline_code_does_not_absorb_blank_line_paragraph(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "`line 1\n\nline 2".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 2);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[0].entity.read(cx).display_text(), "`line 1");
            assert_eq!(visible[1].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[1].entity.read(cx).display_text(), "line 2");
        });
    }

    #[gpui::test]
    async fn preserves_multiple_leading_blank_lines_as_empty_blocks(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "\n\nalpha".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).display_text(), "");
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[2].entity.read(cx).display_text(), "alpha");
            assert_eq!(editor.document.markdown_text(cx), "\n\nalpha");
        });
    }

    #[gpui::test]
    async fn preserves_multiple_trailing_blank_lines_as_empty_blocks(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "alpha\n\n\n".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 3);
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha");
            assert_eq!(visible[1].entity.read(cx).display_text(), "");
            assert_eq!(visible[2].entity.read(cx).display_text(), "");
            assert_eq!(editor.document.markdown_text(cx), "alpha\n\n\n");
        });
    }

    #[gpui::test]
    async fn single_trailing_newline_does_not_create_visible_empty_block(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, "alpha\n".to_string(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).display_text(), "alpha");
            assert_eq!(editor.document.markdown_text(cx), "alpha");
        });
    }

    #[gpui::test]
    async fn empty_document_keeps_single_editable_empty_block(cx: &mut TestAppContext) {
        let editor = cx.new(|cx| Editor::from_markdown(cx, String::new(), None));

        editor.update(cx, |editor, cx| {
            let visible = editor.document.visible_blocks();
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].entity.read(cx).kind(), BlockKind::Paragraph);
            assert_eq!(visible[0].entity.read(cx).display_text(), "");
            assert_eq!(editor.document.markdown_text(cx), "");
        });
    }
