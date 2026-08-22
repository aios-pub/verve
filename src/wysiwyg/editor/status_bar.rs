//! Bottom status bar: sidebar toggle, mode switch, cursor position,
//! word count, and custom buttons.

use std::sync::OnceLock;

use gpui::*;
use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

use super::Editor;
use crate::wysiwyg::config::preferences::{StatusBarButton, StatusBarPreferences};
use crate::wysiwyg::i18n::I18nStrings;
use crate::wysiwyg::theme::Theme;

#[derive(Default)]
pub(super) struct StatusBarState {
    pub sidebar_hovered: bool,
    pub mode_hovered: bool,
    custom_button_hovered: Option<String>,
}

impl Editor {
    pub(super) fn render_status_bar(
        &mut self,
        theme: &Theme,
        strings: &I18nStrings,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prefs = self.status_bar_preferences(cx);
        if !prefs.enabled {
            return None;
        }

        let c = &theme.colors;
        let d = &theme.dimensions;

        let mut left_items: Vec<AnyElement> = Vec::new();

        // The sidebar (workspace drawer) toggle in the status bar is only shown
        // when the editor owns its window (standalone). In the embedded
        // side-view mode the host title bar provides its own outline toggle, so
        // a second one here would be redundant.
        if prefs.show_sidebar_toggle && self.owns_window {
            left_items.push(render_sidebar_toggle(
                &mut self.status_bar,
                self.workspace.is_open,
                theme,
                strings,
                cx,
            ));
        }

        // The mode switch lives in the host title bar for the embedded
        // side-view; only the standalone (window-owning) editor shows it here.
        if prefs.show_mode_switch && self.owns_window {
            left_items.push(render_mode_switch(
                &mut self.status_bar,
                self.view_mode,
                theme,
                strings,
                cx,
            ));
        }

        let mut right_items: Vec<AnyElement> = Vec::new();

        if prefs.show_cursor_position && self.view_mode == super::ViewMode::Source {
            right_items.push(render_cursor(
                self.compute_source_cursor_position(cx),
                theme,
            ));
        }

        // Word count lives in the host title bar for the embedded side-view;
        // only the standalone (window-owning) editor shows it here.
        if prefs.show_word_count && self.owns_window {
            let (total_count, selection_count) = self.word_count(cx);
            right_items.push(render_word_count(
                selection_count,
                total_count,
                theme,
                strings,
            ));
        }

        for button in &prefs.custom_buttons {
            right_items.push(render_custom_button(
                &mut self.status_bar,
                button,
                theme,
                cx,
            ));
        }

        // If nothing is left to show (embedded side-view moved mode/word to the
        // title bar, and there's no cursor position or custom buttons), hide the
        // bar entirely so the editor fills the full height.
        if left_items.is_empty() && right_items.is_empty() {
            return None;
        }

        let bar = div()
            .id("status-bar")
            .h(px(d.status_bar_height))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px(px(d.status_bar_padding_x))
            .bg(c.status_bar_background)
            .border_t(px(1.0))
            .border_color(c.dialog_border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(d.status_bar_item_gap))
                    .children(left_items),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(d.status_bar_item_gap))
                    .children(right_items),
            )
            .into_any_element();

        Some(bar)
    }

    fn status_bar_preferences(&self, cx: &App) -> StatusBarPreferences {
        crate::wysiwyg::config::preferences::EditorSettings::status_bar_preferences(cx)
    }

    /// Returns (line, col), both 1-based, from the source-mode selection snapshot.
    fn compute_source_cursor_position(&self, cx: &App) -> (usize, usize) {
        let snapshot = self.capture_source_selection_snapshot(cx);
        let cursor_offset = snapshot.range.end;
        let text = self.document.raw_source_text(cx);
        let clamped = cursor_offset.min(text.len());

        let line = text[..clamped].matches('\n').count() + 1;
        let last_newline = text[..clamped].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = text[last_newline..clamped].graphemes(true).count() + 1;
        (line, col)
    }
}

fn render_sidebar_toggle(
    state: &mut StatusBarState,
    _is_open: bool,
    theme: &Theme,
    strings: &I18nStrings,
    cx: &mut Context<Editor>,
) -> AnyElement {
    let c = &theme.colors;
    let d = &theme.dimensions;

    div()
        .id("status-bar-sidebar-toggle")
        .h(px(d.status_bar_height - 4.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .bg(if state.sidebar_hovered {
            c.status_bar_button_hover
        } else {
            hsla(0., 0., 0., 0.)
        })
        .cursor_pointer()
        .text_size(px(d.status_bar_text_size))
        .text_color(c.status_bar_text)
        .child(strings.status_bar_files.clone())
        .on_hover(cx.listener(
            |editor: &mut Editor,
             hovered: &bool,
             _window: &mut Window,
             cx: &mut Context<Editor>| {
                editor.status_bar.sidebar_hovered = *hovered;
                cx.notify();
            },
        ))
        .on_click(cx.listener(
            |editor: &mut Editor,
             _: &gpui::ClickEvent,
             window: &mut Window,
             cx: &mut Context<Editor>| {
                editor.toggle_workspace_drawer(window, cx);
            },
        ))
        .into_any_element()
}

fn render_mode_switch(
    state: &mut StatusBarState,
    view_mode: super::ViewMode,
    theme: &Theme,
    strings: &I18nStrings,
    cx: &mut Context<Editor>,
) -> AnyElement {
    let c = &theme.colors;
    let d = &theme.dimensions;

    let label = match view_mode {
        super::ViewMode::Source => strings.status_bar_mode_rendered.clone(),
        super::ViewMode::Rendered => strings.status_bar_mode_source.clone(),
    };

    div()
        .id("status-bar-mode-switch")
        .h(px(d.status_bar_height - 4.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .bg(if state.mode_hovered {
            c.status_bar_button_hover
        } else {
            hsla(0., 0., 0., 0.)
        })
        .cursor_pointer()
        .text_size(px(d.status_bar_text_size))
        .text_color(c.status_bar_text)
        .child(label)
        .on_hover(cx.listener(
            |editor: &mut Editor,
             hovered: &bool,
             _window: &mut Window,
             cx: &mut Context<Editor>| {
                editor.status_bar.mode_hovered = *hovered;
                cx.notify();
            },
        ))
        .on_click(cx.listener(
            |editor: &mut Editor,
             _: &gpui::ClickEvent,
             _window: &mut Window,
             cx: &mut Context<Editor>| {
                editor.toggle_view_mode_from_ui(cx);
            },
        ))
        .into_any_element()
}

fn render_cursor((line, col): (usize, usize), theme: &Theme) -> AnyElement {
    let c = &theme.colors;
    let d = &theme.dimensions;

    let label = format!("{} : {}", line, col);

    div()
        .text_size(px(d.status_bar_text_size))
        .text_color(c.status_bar_text)
        .child(label)
        .into_any_element()
}

fn render_word_count(
    selection_count: Option<usize>,
    total_count: usize,
    theme: &Theme,
    strings: &I18nStrings,
) -> AnyElement {
    let c = &theme.colors;
    let d = &theme.dimensions;

    let label = if let Some(sel) = selection_count {
        format!(
            "{} / {} {}",
            sel, total_count, strings.status_bar_word_count_suffix
        )
    } else {
        format!("{} {}", total_count, strings.status_bar_word_count_suffix)
    };

    div()
        .text_size(px(d.status_bar_text_size))
        .text_color(c.status_bar_text_dim)
        .child(label)
        .into_any_element()
}

fn render_custom_button(
    state: &mut StatusBarState,
    button: &StatusBarButton,
    theme: &Theme,
    cx: &mut Context<Editor>,
) -> AnyElement {
    let c = &theme.colors;
    let d = &theme.dimensions;

    let id = button.id.clone();
    let hovered = state.custom_button_hovered.as_deref() == Some(&button.id);

    div()
        .id(ElementId::Name(
            format!("status-bar-custom-button-{}", button.id).into(),
        ))
        .h(px(d.status_bar_height - 4.0))
        .px(px(6.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .bg(if hovered {
            c.status_bar_button_hover
        } else {
            hsla(0., 0., 0., 0.)
        })
        .cursor_pointer()
        .text_size(px(d.status_bar_text_size))
        .text_color(c.status_bar_text)
        .child(button.label.clone())
        .on_hover(cx.listener(
            move |editor: &mut Editor,
                  hovered: &bool,
                  _window: &mut Window,
                  cx: &mut Context<Editor>| {
                if *hovered {
                    editor.status_bar.custom_button_hovered = Some(id.clone());
                } else if editor.status_bar.custom_button_hovered.as_deref() == Some(&id) {
                    editor.status_bar.custom_button_hovered = None;
                }
                cx.notify();
            },
        ))
        .into_any_element()
}

/// Count words in mixed CJK / Latin Markdown text.
///
/// Every CJK character counts as one word; Latin words are split on
/// whitespace and (fullwidth) punctuation. Markdown syntax is stripped first
/// so headings, list/task markers, table separators, emphasis, images, link
/// URLs, HTML tags, fences, and YAML frontmatter do not inflate the count.
/// Fenced-code *content* is still counted as plain words.
pub fn count_words(text: &str) -> usize {
    let (image_re, link_re, html_re) = inline_patterns();
    let mut total = 0usize;
    let mut in_fence = false;
    let mut fence_marker = '\0';
    let mut in_frontmatter = false;
    let mut first_line = true;

    for line in text.lines() {
        let t = line.trim();

        // YAML frontmatter block (only when the document starts with `---`).
        if first_line {
            first_line = false;
            if t == "---" {
                in_frontmatter = true;
                continue;
            }
        }
        if in_frontmatter {
            if t == "---" || t == "..." {
                in_frontmatter = false;
            }
            continue;
        }

        // Fenced code blocks: fence lines themselves carry no words; content
        // lines are counted as plain text (code identifiers are still words).
        if let Some(marker) = fence_marker_char(t) {
            if !in_fence {
                in_fence = true;
                fence_marker = marker;
                continue;
            }
            if marker == fence_marker && is_bare_fence(t, marker) {
                in_fence = false;
                fence_marker = '\0';
                continue;
            }
        }
        if in_fence {
            total += count_plain_words(t);
            continue;
        }

        if is_horizontal_rule(t) || is_table_separator(t) {
            continue;
        }

        let content = strip_block_markers(t);
        let without_images = image_re.replace_all(content, " ");
        let with_link_text = link_re.replace_all(&without_images, "$1");
        let without_html = html_re.replace_all(&with_link_text, " ");
        let plain: String = without_html
            .chars()
            .filter(|&c| !matches!(c, '*' | '_' | '~' | '`' | '|'))
            .collect();
        total += count_plain_words(&plain);
    }
    total
}

/// Count words in plain (non-Markdown) text: CJK chars individually, Latin
/// runs as words, split on whitespace and CJK/fullwidth punctuation.
fn count_plain_words(text: &str) -> usize {
    let mut count = 0;
    let mut in_latin_word = false;

    for ch in text.chars() {
        if is_cjk_char(ch) {
            if in_latin_word {
                count += 1;
                in_latin_word = false;
            }
            count += 1;
        } else if is_word_separator(ch) {
            if in_latin_word {
                count += 1;
                in_latin_word = false;
            }
        } else {
            in_latin_word = true;
        }
    }
    if in_latin_word {
        count += 1;
    }
    count
}

/// Whitespace plus the CJK/fullwidth punctuation blocks that previously fell
/// into the Latin-word bucket and inflated counts (`，。！？：；（）《》【】…—`).
fn is_word_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch as u32,
            // CJK symbols & punctuation: 、。〈〉《》【】〔〕
            0x3000..=0x303F
            // Fullwidth forms: ，。！？：；（）＂＇
            | 0xFF00..=0xFFEF
            // General punctuation band: – — ‘ ’ “ ” • …
            | 0x2014..=0x2026
            // Middle dot
            | 0x00B7
        )
}

/// Inline Markdown patterns, compiled once. The patterns are compile-time
/// constants, so `Regex::new` cannot fail (AGENTS.md exemption 1).
fn inline_patterns() -> &'static (Regex, Regex, Regex) {
    static PATTERNS: OnceLock<(Regex, Regex, Regex)> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let images = Regex::new(r"!\[[^\]]*\]\([^)]*\)")
            .expect("literal pattern always compiles");
        let links = Regex::new(r"\[([^\]]*)\]\([^)]*\)")
            .expect("literal pattern always compiles");
        let html = Regex::new(r"</?[A-Za-z][^<>]*>")
            .expect("literal pattern always compiles");
        (images, links, html)
    })
}

/// If the line opens a fenced code block, its marker character.
fn fence_marker_char(t: &str) -> Option<char> {
    for marker in ['`', '~'] {
        let run = t.chars().take_while(|&c| c == marker).count();
        if run >= 3 {
            return Some(marker);
        }
    }
    None
}

/// A closing fence: only the marker run (and whitespace).
fn is_bare_fence(t: &str, marker: char) -> bool {
    t.chars().all(|c| c == marker || c.is_whitespace()) && t.chars().filter(|&c| c == marker).count() >= 3
}

/// `---`, `***`, `___` (optionally spaced) — no words.
fn is_horizontal_rule(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    let first = compact.chars().next();
    matches!(first, Some('-') | Some('*') | Some('_'))
        && compact.len() >= 3
        && compact.chars().all(|c| Some(c) == first)
}

/// A table separator row like `| --- | :---: |` — no words.
fn is_table_separator(t: &str) -> bool {
    if !t.contains('|') {
        return false;
    }
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    !compact.is_empty()
        && compact.chars().all(|c| matches!(c, '|' | ':' | '-'))
        && compact.contains('-')
}

/// Strip leading block markers (headings, quotes, list bullets, task
/// checkboxes, ordered numbers) so only the sentence content remains.
fn strip_block_markers(mut t: &str) -> &str {
    loop {
        let trimmed = t.trim_start();
        let rest = trimmed;

        // ATX heading marker.
        let hashes = rest.chars().take_while(|&c| c == '#').count();
        if hashes >= 1 && hashes <= 6 {
            let after = &rest[hashes..];
            if after.is_empty() || after.starts_with(' ') {
                t = after.trim_start();
                continue;
            }
        }

        // Block quote marker.
        if let Some(after) = rest.strip_prefix('>') {
            t = after.trim_start();
            continue;
        }

        // Task checkbox `[ ]` / `[x]`.
        let lower: Vec<char> = rest.chars().take(3).collect();
        if lower.len() == 3
            && lower[0] == '['
            && matches!(lower[1], ' ' | 'x' | 'X')
            && lower[2] == ']'
        {
            t = rest[3..].trim_start();
            continue;
        }

        // Bullet or ordered-list marker (`-`, `+`, `*`, `12.`, `12)`).
        let bullet = rest
            .chars()
            .next()
            .is_some_and(|c| matches!(c, '-' | '+' | '*'));
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let ordered = !digits.is_empty()
            && rest[digits.len()..]
                .chars()
                .next()
                .is_some_and(|c| c == '.' || c == ')');
        if bullet || ordered {
            let skip = if bullet { 1 } else { digits.len() + 1 };
            t = rest[skip..].trim_start();
            continue;
        }

        t = rest;
        return t;
    }
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        // CJK Unified Ideographs
        0x4E00..=0x9FFF
        // CJK Unified Ideographs Extension A
        | 0x3400..=0x4DBF
        // CJK Unified Ideographs Extension B
        | 0x20000..=0x2A6DF
        // CJK Compatibility Ideographs
        | 0xF900..=0xFAFF
        // CJK Radicals Supplement / Kangxi Radicals
        | 0x2E80..=0x2EFF
        | 0x2F00..=0x2FDF
        // Hiragana / Katakana (Japanese)
        | 0x3040..=0x309F
        | 0x30A0..=0x30FF
        // Hangul Syllables (Korean)
        | 0xAC00..=0xD7AF
    )
}

#[cfg(all(test, feature = "wysiwyg-gpui-tests"))]
mod tests {
    use super::count_words;

    #[test]
    fn empty_text_has_zero_words() {
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn english_words_are_counted() {
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("one two three four"), 4);
    }

    #[test]
    fn cjk_characters_are_counted_individually() {
        assert_eq!(count_words("你好世界"), 4);
        assert_eq!(count_words("中文"), 2);
    }

    #[test]
    fn mixed_cjk_and_english() {
        assert_eq!(count_words("hello 世界"), 3);
        assert_eq!(count_words("你好 world foo"), 4);
    }

    #[test]
    fn whitespace_handling() {
        assert_eq!(count_words("  hello   world  "), 2);
        assert_eq!(count_words("   "), 0);
    }
}

/// Default-run tests for the Markdown-aware word counter (the module above is
/// feature-gated behind `wysiwyg-gpui-tests`; these are pure and always run).
#[cfg(test)]
mod word_count_tests {
    use super::count_words;

    // -- plain text (parity with the original counter) ----------------------

    #[test]
    fn plain_english_words() {
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("  hello   world  "), 2);
    }

    #[test]
    fn plain_cjk_counts_per_character() {
        assert_eq!(count_words("你好世界"), 4);
        assert_eq!(count_words("hello 世界"), 3);
    }

    // -- CJK / fullwidth punctuation is a separator, not a word -------------

    #[test]
    fn fullwidth_punctuation_is_not_counted() {
        assert_eq!(count_words("你好，世界。"), 4);
        assert_eq!(count_words("中文、英文"), 4);
        assert_eq!(count_words("“引号”内容"), 4);
        assert_eq!(count_words("等等……"), 2);
    }

    // -- markdown block syntax is stripped -----------------------------------

    #[test]
    fn headings_and_quotes_strip_markers() {
        assert_eq!(count_words("## 标题"), 2);
        assert_eq!(count_words("# Title here"), 2);
        assert_eq!(count_words("> 引用内容"), 4);
    }

    #[test]
    fn lists_and_tasks_strip_markers() {
        assert_eq!(count_words("- 第一项"), 3);
        assert_eq!(count_words("- [x] 完成任务"), 4);
        assert_eq!(count_words("1. ordered item"), 2);
    }

    #[test]
    fn horizontal_rules_and_table_separators_are_skipped() {
        assert_eq!(count_words("---"), 0);
        assert_eq!(count_words("***"), 0);
        assert_eq!(count_words("| a | b |\n|---|---|\n| 1 | 2 |"), 4);
    }

    #[test]
    fn table_content_rows_count_cells() {
        assert_eq!(count_words("| 名称 | 数量 |\n| --- | :-: |\n| 苹果 | 三个 |"), 8);
    }

    #[test]
    fn fenced_code_counts_content_but_not_fences() {
        // ASCII punctuation attaches to words ("=" standalone counts, like
        // VS Code's word count): let / x / = / 1; → 4.
        assert_eq!(count_words("```rust\nlet x = 1;\n```"), 4);
        assert_eq!(count_words("~~~\n代码\n~~~"), 2);
    }

    #[test]
    fn yaml_frontmatter_is_skipped() {
        assert_eq!(count_words("---\ntitle: hello\n---\n正文开始"), 4);
    }

    // -- inline syntax -------------------------------------------------------

    #[test]
    fn links_keep_text_and_drop_urls() {
        assert_eq!(count_words("[文档](https://example.com/a/b)"), 2);
        assert_eq!(count_words("see [the docs](http://x.y) now"), 4);
    }

    #[test]
    fn images_are_dropped() {
        assert_eq!(count_words("![alt text](./media/images/a.png)"), 0);
        assert_eq!(count_words("前文 ![图](x.png) 后文"), 4);
    }

    #[test]
    fn html_tags_are_dropped() {
        assert_eq!(count_words("<video src=\"./media/videos/a.mp4\"></video>"), 0);
        assert_eq!(count_words("<b>bold</b> words"), 2);
    }

    #[test]
    fn emphasis_markers_do_not_split_words() {
        assert_eq!(count_words("**加粗**文本"), 4);
        assert_eq!(count_words("*italic* word"), 2);
    }

    // -- integration ---------------------------------------------------------

    #[test]
    fn mixed_document_counts_prose_only() {
        let doc = "\
# 项目笔记

这是第一段，包含 English words 混排。

- [x] 完成调研
- [ ] 输出报告

| 列一 | 列二 |
| --- | --- |
| 数据 | data |

![截图](./media/images/s.png)

```rust
let answer = 42;
```

详见[链接](https://example.com)。
";
        let expected = 4  // 项目笔记
            + 11          // 这是第一段，包含 English words 混排。
            + 4           // 完成调研
            + 4           // 输出报告
            + 4           // 列一 列二
            + 3           // 数据 data
                          // （![截图](…) 图片不计入）
            + 4           // let answer = 42;
            + 4;          // 详见 链接
        assert_eq!(count_words(doc), expected);
    }
}
