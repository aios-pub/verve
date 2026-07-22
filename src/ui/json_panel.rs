//! JSON Formatter panel — left pane raw input, right pane formatted collapsible tree.
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
                let items = value_to_tree_items(&value, true);
                let items = set_all_expanded(items, true);
                self.tree.update(cx, |t, cx| t.set_items(items, cx));
            }
        }
    }

    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ref json_str) = self.formatted_json {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                // When collapsing all, top-level items remain expanded but their children collapse
                let items = value_to_tree_items(&value, false);
                let items = set_all_expanded(items, false);
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

                // Convert JSON value to TreeItems, top-level keys expanded
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

/// Recursively convert a serde_json::Value into TreeItems.
/// For the root object/array, we directly return its children instead of wrapping
/// them in a "{ Object }" label, so keys render at top level like FeHelper.
fn value_to_tree_items(value: &serde_json::Value, expanded_all: bool) -> Vec<TreeItem> {
    match value {
        serde_json::Value::Object(map) => {
            map.iter().map(|(k, v)| {
                json_to_tree_item(v, k.clone(), 0, expanded_all)
            }).collect()
        }
        serde_json::Value::Array(arr) => {
            arr.iter().enumerate().map(|(i, v)| {
                json_to_tree_item(v, i.to_string(), 0, expanded_all)
            }).collect()
        }
        // Primitive root value - single item
        _ => vec![json_to_tree_item(value, "root".to_string(), 0, expanded_all)],
    }
}

/// Recursively convert a serde_json::Value into a TreeItem.
fn json_to_tree_item(value: &serde_json::Value, key: String, depth: usize, expanded_all: bool) -> TreeItem {
    let id = format!("{}-{}", depth, key);
    match value {
        serde_json::Value::Object(map) => {
            let label = format!("\"{}\": {{ {} }}", key, map.len());
            let mut children = Vec::new();
            for (k, v) in map {
                children.push(json_to_tree_item(v, k.clone(), depth + 1, expanded_all));
            }
            TreeItem::new(id, label)
                .children(children)
                .expanded(expanded_all || depth == 0)
        }
        serde_json::Value::Array(arr) => {
            let label = format!("\"{}\": [ {} ]", key, arr.len());
            let mut children = Vec::new();
            for (i, v) in arr.iter().enumerate() {
                children.push(json_to_tree_item(v, i.to_string(), depth + 1, expanded_all));
            }
            TreeItem::new(id, label)
                .children(children)
                .expanded(expanded_all || depth == 0)
        }
        serde_json::Value::String(s) => {
            let label = format!("\"{}\": \"{}\"", key, s);
            TreeItem::new(id, label)
        }
        serde_json::Value::Number(n) => {
            let label = format!("\"{}\": {}", key, n);
            TreeItem::new(id, label)
        }
        serde_json::Value::Bool(b) => {
            let label = format!("\"{}\": {}", key, b);
            TreeItem::new(id, label)
        }
        serde_json::Value::Null => {
            let label = format!("\"{}\": null", key);
            TreeItem::new(id, label)
        }
    }
}

/// Recursively set expanded state on all tree items.
fn set_all_expanded(items: Vec<TreeItem>, expanded: bool) -> Vec<TreeItem> {
    items.into_iter().map(|item| {
        let children = item.children.clone();
        item.children(set_all_expanded(children, expanded)).expanded(expanded)
    }).collect()
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
                            tree(&self.tree, move |ix, entry, _selected, _window, _cx| {
                                let item = entry.item();
                                let depth = entry.depth();
                                let is_folder = entry.is_folder();
                                let is_expanded = entry.is_expanded();
                                let label_str = item.label.to_string();
                                let tree_for_click = tree_entity.clone();

                                gpui_component::list::ListItem::new(SharedString::from(format!("json-row-{}", item.id)))
                                    .pl(px(8.) * depth)
                                    .when(is_folder, |row| {
                                        let icon = if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight };
                                        let tree_chevron = tree_for_click.clone();
                                        let tree_row = tree_for_click.clone();
                                        row.child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .child(
                                                    Button::new(SharedString::from(format!("json-chevron-{}", item.id)))
                                                        .icon(icon)
                                                        .ghost()
                                                        .small()
                                                        .on_click(move |_, _, cx| {
                                                            tree_chevron.update(cx, |t, cx| t.toggle_expand(ix, cx));
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_color(fg)
                                                        .font_family(mono_font.clone())
                                                        .text_sm()
                                                        .child(label_str.clone()),
                                                ),
                                        )
                                        .on_click(move |_, _, cx| {
                                            tree_row.update(cx, |t, cx| t.toggle_expand(ix, cx));
                                        })
                                    })
                                    .when(!is_folder, |row| {
                                        // Determine color for primitive value
                                        let value_color = if label_str.contains("true") || label_str.contains("false") {
                                            warn // boolean
                                        } else if label_str.contains("null") {
                                            muted_fg // null
                                        } else if label_str.contains(": \"") {
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

                                        row.child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .pl(px(28.))
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
                                        )
                                    })
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
