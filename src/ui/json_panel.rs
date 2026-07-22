//! JSON Formatter panel — left pane raw input, right pane formatted collapsible code view.
//! Supports expand/collapse per node, all expand/collapse, copy result.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::tree::{tree, TreeItem, TreeState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};

pub struct JsonPanel {
    input: Entity<InputState>,
    tree: Entity<TreeState>,
    formatted_json: Option<String>,
    error: Option<String>,
    copy_status: Option<String>,
    _subs: Vec<Subscription>,
}

impl JsonPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .multi_line(true)
                .placeholder(rust_i18n::t!("json.input_ph").to_string());
            s.set_value(String::new(), window, cx);
            s
        });

        let tree = cx.new(|cx| TreeState::new(cx));

        // Auto-format when input changes (paste/edit)
        let input_clone = input.clone();
        let tree_clone = tree.clone();
        let input_sub = cx.subscribe(&input, move |this: &mut Self, _src, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                let raw = input_clone.read(cx).value().to_string();
                this.do_format(&raw, cx);
            }
        });

        Self {
            input,
            tree,
            formatted_json: None,
            error: None,
            copy_status: None,
            _subs: vec![input_sub],
        }
    }

    // Public methods called from title bar buttons
    pub fn format_from_title(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().to_string();
        self.do_format(&raw, cx);
    }

    pub fn expand_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ref json_str) = self.formatted_json {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                // Rebuild tree with all nodes expanded
                let items = value_to_tree_items_expanded(&value, true);
                self.tree.update(cx, |t, cx| t.set_items(items, cx));
            }
        }
    }

    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ref json_str) = self.formatted_json {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                // Rebuild tree with only root expanded
                let items = value_to_tree_items_expanded(&value, false);
                self.tree.update(cx, |t, cx| t.set_items(items, cx));
            }
        }
    }

    pub fn copy_result(&mut self, cx: &mut Context<Self>) {
        if let Some(ref json_str) = self.formatted_json {
            cx.write_to_clipboard(ClipboardItem::new_string(json_str.clone()));
            self.copy_status = Some(rust_i18n::t!("json.copy_success").to_string());
            cx.notify();
        }
    }

    fn do_format(&mut self, raw: &str, cx: &mut Context<Self>) {
        if raw.trim().is_empty() {
            self.error = None;
            self.formatted_json = None;
            self.tree.update(cx, |t, cx| t.set_items(vec![], cx));
            cx.notify();
            return;
        }

        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(value) => {
                let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
                self.formatted_json = Some(pretty);
                self.error = None;

                // Convert JSON value to TreeItems
                let items = value_to_tree_items(&value, false);
                self.tree.update(cx, |t, cx| {
                    t.set_items(items, cx);
                });
            }
            Err(e) => {
                self.error = Some(format!("{}: {}", rust_i18n::t!("json.invalid_json"), e));
                self.formatted_json = None;
                self.tree.update(cx, |t, cx| t.set_items(vec![], cx));
            }
        }
        self.copy_status = None;
        cx.notify();
    }
}

/// Recursively convert a serde_json::Value into TreeItems, with root always expanded.
/// We create a virtual root node so the outer braces are visible and collapsible.
fn value_to_tree_items(value: &serde_json::Value, expanded_all: bool) -> Vec<TreeItem> {
    value_to_tree_items_expanded(value, expanded_all)
}

/// Build tree with specified expanded state for all nodes
fn value_to_tree_items_expanded(value: &serde_json::Value, expand_all: bool) -> Vec<TreeItem> {
    match value {
        serde_json::Value::Object(map) => {
            let label = format!("{}|{}", "{", map.len());
            let mut children = Vec::new();
            let child_count = map.len();
            let mut i = 0;
            for (k, v) in map {
                i += 1;
                let trailing_comma = i < child_count;
                let mut child = json_to_tree_item_expanded(v, k.clone(), 1, expand_all);
                if trailing_comma {
                    child.label = SharedString::from(format!("{},", child.label));
                }
                children.push(child);
            }
            // Add closing brace as last child
            children.push(TreeItem::new("root-close".to_string(), "}"));
            vec![
                TreeItem::new("root".to_string(), label)
                    .children(children)
                    .expanded(true)  // root always expanded
            ]
        }
        serde_json::Value::Array(arr) => {
            let label = format!("{}|{}", "[", arr.len());
            let mut children = Vec::new();
            let child_count = arr.len();
            for (i, v) in arr.iter().enumerate() {
                let trailing_comma = i < child_count - 1;
                let mut child = json_to_tree_item_expanded(v, i.to_string(), 1, expand_all);
                if trailing_comma {
                    child.label = SharedString::from(format!("{},", child.label));
                }
                children.push(child);
            }
            // Add closing bracket as last child
            children.push(TreeItem::new("root-close".to_string(), "]"));
            vec![
                TreeItem::new("root".to_string(), label)
                    .children(children)
                    .expanded(true)  // root always expanded
            ]
        }
        // Primitive root value - single item
        _ => vec![json_to_tree_item_expanded(value, String::new(), 0, expand_all)],
    }
}

/// Recursively convert a serde_json::Value into a TreeItem.
/// For containers we encode count in label as "prefix{|count" or "prefix[|count".
fn json_to_tree_item(value: &serde_json::Value, key: String, depth: usize, expanded_all: bool) -> TreeItem {
    json_to_tree_item_expanded(value, key, depth, expanded_all)
}

/// Build tree item with specified expand_all state
fn json_to_tree_item_expanded(value: &serde_json::Value, key: String, depth: usize, expand_all: bool) -> TreeItem {
    let id = format!("{}-{}", depth, key);
    let key_prefix = if key.is_empty() {
        String::new()
    } else {
        format!("\"{}\": ", key)
    };

    match value {
        serde_json::Value::Object(map) => {
            let label = format!("{}{}|{}", key_prefix, "{", map.len());
            let mut children = Vec::new();
            let child_count = map.len();
            let mut i = 0;
            for (k, v) in map {
                i += 1;
                let trailing_comma = i < child_count;
                let mut child = json_to_tree_item_expanded(v, k.clone(), depth + 1, expand_all);
                if trailing_comma {
                    child.label = SharedString::from(format!("{},", child.label));
                }
                children.push(child);
            }
            // Add closing brace as last child
            let close_id = format!("{}-{}-close", depth, key);
            children.push(TreeItem::new(close_id, "}"));
            TreeItem::new(id, label)
                .children(children)
                .expanded(expand_all || depth == 0)
        }
        serde_json::Value::Array(arr) => {
            let label = format!("{}[|{}", key_prefix, arr.len());
            let mut children = Vec::new();
            let child_count = arr.len();
            for (i, v) in arr.iter().enumerate() {
                let trailing_comma = i < child_count - 1;
                let mut child = json_to_tree_item_expanded(v, i.to_string(), depth + 1, expand_all);
                if trailing_comma {
                    child.label = SharedString::from(format!("{},", child.label));
                }
                children.push(child);
            }
            // Add closing bracket as last child
            let close_id = format!("{}-{}-close", depth, key);
            children.push(TreeItem::new(close_id, "]"));
            TreeItem::new(id, label)
                .children(children)
                .expanded(expand_all || depth == 0)
        }
        serde_json::Value::String(s) => {
            let label = format!("{}\"{}\"", key_prefix, s);
            TreeItem::new(id, label)
        }
        serde_json::Value::Number(n) => {
            let label = format!("{}{}", key_prefix, n);
            TreeItem::new(id, label)
        }
        serde_json::Value::Bool(b) => {
            let label = format!("{}{}", key_prefix, b);
            TreeItem::new(id, label)
        }
        serde_json::Value::Null => {
            let label = format!("{}null", key_prefix);
            TreeItem::new(id, label)
        }
    }
}

impl Render for JsonPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        // Build the resizable split: left = input, right = tree output
        let input_el = v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(rust_i18n::t!("json.input_ph").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(4.))
                    .overflow_hidden()
                    .child(
                        Input::new(&self.input)
                            .small()
                            .h_full(),
                    ),
            )
            .when_some(self.error.as_ref(), |flex, err| {
                flex.child(
                    div()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(err.clone()),
                )
            });

        let mono_font = theme.mono_font_family.clone();
        let fg = theme.foreground;
        let warn = theme.warning;
        let muted_fg = theme.muted_foreground;
        let success = theme.success;
        let info = theme.info;
        let tree_entity = self.tree.clone();
        let tree_row = self.tree.clone();

        let output_el = v_flex()
            .size_full()
            .gap_2()
            .p_2()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Formatted Result"),
                    )
                    .when_some(self.copy_status.as_ref(), |flex, status| {
                        flex.child(
                            div()
                                .text_xs()
                                .text_color(theme.success)
                                .child(status.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .id("json-tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(4.))
                    .p_1()
                    .bg(theme.muted.opacity(0.3))
                    .when(self.formatted_json.is_some(), |c| {
                        c.child(
                            tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                                let item = entry.item();
                                let depth = entry.depth();
                                let is_folder = entry.is_folder();
                                let is_expanded = entry.is_expanded();
                                let label_str = item.label.to_string();
                                let tree_for_click = tree_entity.clone();

                                let chevron_size = 20.;
                                let indent_width = 20.;

                                let mut row = gpui_component::list::ListItem::new(SharedString::from(format!("json-row-{}", item.id)))
                                    .pl(px(indent_width) * depth)
                                    .h(px(22.));

                                if is_folder {
                                    let icon = if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight };
                                    let toggle_ix = ix;

                                    // Parse label: we encoded as "prefix{|count" for objects, "prefix[|count" for arrays
                                    let (is_array, key_part, count) = if let Some(pipe_pos) = label_str.rfind('|') {
                                        let count: usize = label_str[pipe_pos+1..].parse().unwrap_or(0);
                                        let prefix = &label_str[..pipe_pos];
                                        let is_arr = prefix.ends_with('[');
                                        let key_part = if is_arr {
                                            prefix.trim_end_matches('[')
                                        } else {
                                            prefix.trim_end_matches('{')
                                        };
                                        (is_arr, key_part.to_string(), count)
                                    } else {
                                        (false, label_str.clone(), 0)
                                    };

                                    let display_label = if is_expanded {
                                        // Expanded: just show opening brace/key
                                        if is_array {
                                            format!("{}[", key_part)
                                        } else {
                                            format!("{}{}", key_part, "{")
                                        }
                                    } else {
                                        // Collapsed: show {...N} or [...N]
                                        if is_array {
                                            format!("{}[...{}]", key_part, count)
                                        } else {
                                            format!("{}{{...{}}}", key_part, count)
                                        }
                                    };

                                    row = row.child(
                                        h_flex()
                                            .gap_0()
                                            .items_center()
                                            .h_full()
                                            .w_full()
                                            .child(
                                                // Chevron character as indicator
                                                div()
                                                    .w(px(chevron_size))
                                                    .h(px(chevron_size))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_color(muted_fg)
                                                    .font_family(mono_font.clone())
                                                    .text_xs()
                                                    .child(if is_expanded { "▼" } else { "▶" }),
                                            )
                                            .child(
                                                div()
                                                    .text_color(fg)
                                                    .font_family(mono_font.clone())
                                                    .text_sm()
                                                    .child(display_label),
                                            ),
                                    );

                                    // Make entire row clickable to toggle
                                    row = row.on_click(move |_, _, cx| {
                                        tree_for_click.update(cx, |t, cx| t.toggle_expand(toggle_ix, cx));
                                    });
                                } else {
                                    // Handle closing brace/bracket
                                    if label_str == "}" || label_str == "]" {
                                        row = row.child(
                                            h_flex()
                                                .gap_0()
                                                .items_center()
                                                .h_full()
                                                .pl(px(chevron_size)) // align with opening chevron
                                                .font_family(mono_font.clone())
                                                .text_sm()
                                                .text_color(fg)
                                                .child(label_str.clone()),
                                        );
                                    } else {
                                        // Regular primitive value
                                        let check_str = label_str.trim_end_matches(',');
                                        let value_color = if check_str.contains("true") || check_str.contains("false") {
                                            warn // boolean
                                        } else if check_str.contains("null") {
                                            muted_fg // null
                                        } else if check_str.contains(": \"") || check_str.starts_with('"') {
                                            success // string
                                        } else {
                                            info // number
                                        };

                                        // Split key and value at first ": "
                                        let (key_part, val_part) = if let Some(pos) = label_str.find(": ") {
                                            (label_str[..pos+2].to_string(), label_str[pos+2..].to_string())
                                        } else {
                                            (String::new(), label_str.clone())
                                        };

                                        row = row.child(
                                            h_flex()
                                                .gap_0()
                                                .items_center()
                                                .h_full()
                                                .pl(px(chevron_size)) // align with folder rows
                                                .font_family(mono_font.clone())
                                                .text_sm()
                                                .child(
                                                    div()
                                                        .text_color(fg)
                                                        .child(key_part),
                                                )
                                                .child(
                                                    div()
                                                        .text_color(value_color)
                                                        .child(val_part),
                                                ),
                                        );
                                    }
                                }

                                row
                            }),
                        )
                    })
                    .when(self.formatted_json.is_none(), |c| {
                        c.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .text_color(theme.muted_foreground)
                                .text_sm()
                                .child(rust_i18n::t!("json.input_ph").to_string()),
                        )
                    }),
            );

        h_resizable("json-split")
            .child(
                resizable_panel()
                    .size(px(400.))
                    .size_range(px(200.)..px(800.))
                    .child(input_el),
            )
            .child(
                resizable_panel()
                    .overflow_hidden()
                    .child(output_el),
            )
    }
}
