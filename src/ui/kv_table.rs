//! Reusable editable key-value row rendering.
//!
//! `KvTable` is a **stateless** `RenderOnce` component. The owning view holds
//! the per-row `InputState` entities (created where a `Window` is available)
//! and passes bound `cx.listener(...)` closures for toggle/delete/add so events
//! dispatch back to it directly. This matches gpui-component's stateless
//! `RenderOnce` philosophy.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex, v_flex,
};

use crate::state::models::{FieldType, KeyValue};

/// Approximate display width of `s` inside a `small` value cell, in "units"
/// where an ASCII char = 1 and a wide char (CJK / fullwidth / emoji) = 2.
/// Used to guess whether the value overflows the fixed-width column so we can
/// expand it on focus. Counts by `char` (never byte-slices), per AGENTS.md.
fn estimate_display_units(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if (0x1100..=0x115F).contains(&cp)
                || (0x2E80..=0xA4CF).contains(&cp)
                || (0xAC00..=0xD7A3).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFE30..=0xFE4F).contains(&cp)
                || (0xFF00..=0xFF60).contains(&cp)
                || (0xFFE0..=0xFFE6).contains(&cp)
                || (0x1F300..=0x1FAFF).contains(&cp)
                || (0x20000..=0x3FFFD).contains(&cp)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Approximate pixels per display unit in a `small` value input (~8px per
/// ascii char). Used to derive how many units fit a value column of a given
/// width, so the focus-expansion only triggers when content truly overflows.
const VALUE_UNIT_PX: f32 = 8.0;

/// One editable row held by the owning view.
#[derive(Clone)]
pub struct KvRow {
    pub enabled: bool,
    pub key: Entity<InputState>,
    pub value: Entity<InputState>,
    pub file_path: Option<String>,
    pub field_type: FieldType,
    pub required: bool,
    pub description: Entity<InputState>,
}

impl KvRow {
    pub fn new(kv: &KeyValue, window: &mut Window, cx: &mut App) -> Self {
        let key = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("key");
            s.set_value(kv.key.clone(), window, cx);
            s
        });
        let value = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("value")
                // multi_line + soft_wrap so a focused, overflowing value can
                // expand to wrap and reveal its full content. A short value
                // stays a single line at the natural `small` height.
                .multi_line(true)
                .soft_wrap(true);
            s.set_value(kv.value.clone(), window, cx);
            s
        });
        let description = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("描述");
            s.set_value(kv.description.clone(), window, cx);
            s
        });
        Self {
            enabled: kv.enabled,
            key,
            value,
            file_path: kv.file_path.clone(),
            field_type: kv.field_type,
            required: kv.required,
            description,
        }
    }

    /// Empty enabled row (used as a trailing input slot). New rows default to
    /// enabled so that anything the user types immediately takes effect (e.g.
    /// shows up in generated curl, is sent with the request) without requiring
    /// an extra click on the checkbox.
    pub fn empty(window: &mut Window, cx: &mut App) -> Self {
        let mut kv = KeyValue::default();
        kv.enabled = true;
        kv.required = true;
        Self::new(&kv, window, cx)
    }

    pub fn to_kv(&self, cx: &App) -> KeyValue {
        KeyValue {
            enabled: self.enabled,
            key: self.key.read(cx).value().to_string(),
            value: self.value.read(cx).value().to_string(),
            file_path: self.file_path.clone(),
            field_type: self.field_type,
            required: self.required,
            description: self.description.read(cx).value().to_string(),
        }
    }

    /// Returns true if this row has a non-empty key (used for count badges).
    pub fn has_content(&self, cx: &App) -> bool {
        !self.key.read(cx).value().trim().is_empty()
    }
}

/// Row-level callbacks bound by the owner with `cx.listener`. Each takes the
/// row index (where relevant); toggle also carries the new enabled state.
pub struct KvHandlers {
    pub on_toggle: Arc<dyn Fn(usize, bool, &mut Window, &mut App)>,
    pub on_delete: Arc<dyn Fn(usize, &mut Window, &mut App)>,
    pub on_add: Arc<dyn Fn(&mut Window, &mut App)>,
    /// Cycle/-set the field type for row `ix`.
    pub on_type_change: Arc<dyn Fn(usize, FieldType, &mut Window, &mut App)>,
    /// Toggle the "required" flag for row `ix`.
    pub on_required_toggle: Arc<dyn Fn(usize, bool, &mut Window, &mut App)>,
    /// Pick a file for row `ix` (form-data file fields).
    pub on_file_pick: Arc<dyn Fn(usize, &mut Window, &mut App)>,
}

/// Stateless key-value table renderer.
#[derive(IntoElement)]
pub struct KvTable {
    pub id: SharedString,
    pub rows: Vec<KvRow>,
    pub allow_files: bool,
    /// Whether to show the leading enabled checkbox on each row.
    pub show_enabled: bool,
    /// Whether to show the type selector (form-data/query).
    pub show_type: bool,
    /// Whether to show the description column (manager tables).
    pub show_description: bool,
    /// Whether to show the required (`*`) toggle — independent of `show_type`
    /// so required can be enabled for all param scopes (headers/cookies/etc).
    pub show_required: bool,
    /// Fixed width of the value column. Defaults to 160px; wider for tables
    /// that hold long values (e.g. environment-variable URLs).
    pub value_width: Option<Pixels>,
    /// When true, the description column flexes (grows/shrinks) instead of a
    /// fixed 180px, so a wide value column can't push the delete button off
    /// the right edge of the table.
    pub description_flex: bool,
    pub handlers: KvHandlers,
}

impl KvTable {
    pub fn new(id: impl Into<SharedString>, rows: Vec<KvRow>, handlers: KvHandlers) -> Self {
        Self {
            id: id.into(),
            rows,
            allow_files: false,
            show_enabled: true,
            show_type: false,
            show_description: false,
            show_required: false,
            value_width: None,
            description_flex: false,
            handlers,
        }
    }

    pub fn allow_files(mut self, allow: bool) -> Self {
        self.allow_files = allow;
        self
    }

    /// Show or hide the leading enabled checkbox on each row (default true).
    pub fn show_enabled(mut self, show: bool) -> Self {
        self.show_enabled = show;
        self
    }

    pub fn show_type(mut self, show: bool) -> Self {
        self.show_type = show;
        self
    }

    pub fn show_description(mut self, show: bool) -> Self {
        self.show_description = show;
        self
    }

    pub fn show_required(mut self, show: bool) -> Self {
        self.show_required = show;
        self
    }

    /// Set a custom fixed width for the value column (default 160px).
    pub fn value_width(mut self, width: impl Into<Pixels>) -> Self {
        self.value_width = Some(width.into());
        self
    }

    /// Make the description column flex (grow/shrink) instead of a fixed
    /// 180px, so it absorbs slack and keeps the delete button visible even
    /// with a wide value column.
    pub fn description_flex(mut self, flex: bool) -> Self {
        self.description_flex = flex;
        self
    }
}

impl RenderOnce for KvTable {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let allow_files = self.allow_files;
        let show_type = self.show_type;
        let show_description = self.show_description;
        let show_required = self.show_required;
        let show_enabled = self.show_enabled;
        let description_flex = self.description_flex;
        let _id = self.id.clone();
        let theme = cx.theme().clone();
        // Fixed value-column width (default 160px) and the corresponding
        // display-unit capacity used by the focus-expansion overflow check.
        // Derived from the width so a wider column (e.g. env-variable URLs)
        // doesn't trigger an expansion for content that already fits.
        let value_w = self.value_width.unwrap_or(px(160.));
        let col_capacity = (((value_w.as_f32() - 16.0) / VALUE_UNIT_PX).floor() as usize).max(8);
        v_flex()
            .w_full()
            .gap_1()
            .children(self.rows.iter().enumerate().map(|(ix, row)| {
                let toggle = self.handlers.on_toggle.clone();
                let delete = self.handlers.on_delete.clone();
                let on_type = self.handlers.on_type_change.clone();
                let on_req = self.handlers.on_required_toggle.clone();
                let on_file = self.handlers.on_file_pick.clone();
                let enabled = row.enabled;
                let field_type = row.field_type;
                let required = row.required;
                let is_file = field_type == FieldType::File;

                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .when(show_enabled, |this| {
                        this.child(Checkbox::new(("kv-enabled", ix)).checked(enabled).on_click(
                            move |checked: &bool, window, cx| {
                                (toggle)(ix, *checked, window, cx);
                            },
                        ))
                    })
                    .child(
                        div()
                            .w(px(140.))
                            .flex_shrink_0()
                            .child(Input::new(&row.key).small().appearance(false)),
                    )
                    .child({
                        // Whether this value overflows the fixed-width column
                        // (approximated by display units) and whether it is
                        // currently focused. A focused, overflowing value
                        // expands to wrap and reveal its full content; an
                        // unfocused overflowing value is clamped back to a
                        // single line so the table stays compact.
                        let val_str = row.value.read(cx).value().to_string();
                        let units = estimate_display_units(&val_str);
                        let overflows = units > col_capacity;
                        let value_focused = row.value.read(cx).focus_handle(cx).is_focused(window);
                        // For form-data file rows that already reference a
                        // file, show the chosen file name as a read-only label
                        // instead of the (always-empty, disabled) value input.
                        // We deliberately avoid mutating the value InputState
                        // so `to_kv()` keeps returning the model's real value
                        // (empty) and only `file_path` carries the file.
                        let file_name: Option<String> = (is_file && allow_files)
                            .then(|| row.file_path.as_deref())
                            .flatten()
                            .and_then(|p| {
                                std::path::Path::new(p)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.to_string())
                            });
                        div()
                            .w(value_w)
                            .flex_shrink_0()
                            .child(if let Some(name) = file_name {
                                div()
                                    .w_full()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(Icon::new(IconName::File).size_3())
                                    .child(
                                        div()
                                            .ml_1()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(name),
                                    )
                                    .into_any_element()
                            } else {
                                let input = Input::new(&row.value)
                                    .small()
                                    .appearance(false)
                                    .disabled(is_file && allow_files);
                                let input = if value_focused && overflows {
                                    // ~20px per wrapped line + 4px padding,
                                    // capped at 8 lines; taller content scrolls
                                    // inside the multi-line editor.
                                    let est_lines =
                                        ((units as f64) / (col_capacity as f64)).ceil() as usize;
                                    let est_lines = est_lines.max(1);
                                    let h_px = (est_lines * 20 + 4).min(160);
                                    input.h(px(h_px as f32))
                                } else if overflows {
                                    // Long but unfocused: keep one compact line.
                                    input.h(px(24.))
                                } else {
                                    // Short value: natural single-line height.
                                    input
                                };
                                input.into_any_element()
                            })
                    })
                    // Type selector dropdown (form-data / query).
                    .when(show_type, |this| {
                        let on_type_inner = on_type.clone();
                        let theme_clone = theme.clone();
                        this.child(
                            gpui_component::popover::Popover::new(("kv-type-pop", ix))
                                .anchor(gpui::Anchor::TopLeft)
                                .trigger(
                                    Button::new(("kv-type", ix))
                                        .ghost()
                                        .xsmall()
                                        .label(field_type.as_str())
                                        .icon(IconName::ChevronDown)
                                        .w(px(72.)),
                                )
                                .p(px(4.))
                                .child(v_flex().w(px(90.)).gap(px(2.)).children(
                                    FieldType::all().iter().map(|&ft| {
                                        let cb = on_type_inner.clone();
                                        let is_current = ft == field_type;
                                        let tc = theme_clone.clone();
                                        div()
                                            .id(format!("kv-type-opt-{ix}-{}", ft.as_str()))
                                            .w_full()
                                            .h(px(22.))
                                            .px(px(8.))
                                            .flex()
                                            .items_center()
                                            .text_size(px(11.))
                                            .rounded(px(4.))
                                            .when(is_current, |d| {
                                                d.bg(tc.accent.opacity(0.5))
                                                    .text_color(tc.foreground)
                                            })
                                            .when(!is_current, |d| {
                                                d.text_color(tc.muted_foreground)
                                            })
                                            .hover(|d| d.bg(tc.accent.opacity(0.3)))
                                            .child(ft.as_str().to_string())
                                            .on_click(move |_, window, cx: &mut App| {
                                                (cb)(ix, ft, window, cx);
                                                // Force the popover to close by refreshing —
                                                // the Popover's internal state resets on
                                                // re-render when it loses focus.
                                                window.refresh();
                                            })
                                    }),
                                )),
                        )
                    })
                    // Required toggle: "*" highlighted when active. Decoupled
                    // from `show_type` so it can appear for all param scopes.
                    .when(show_required, |this| {
                        this.child(
                            div()
                                .id(("kv-req", ix))
                                .w(px(20.))
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .text_size(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .when(required, |t| {
                                    t.bg(theme.danger.opacity(0.2)).text_color(theme.danger)
                                })
                                .when(!required, |t| {
                                    t.text_color(theme.muted_foreground.opacity(0.5))
                                })
                                .hover(|t| t.bg(theme.accent.opacity(0.5)))
                                .child("*")
                                .on_click(move |_, window, cx| {
                                    (on_req)(ix, !required, window, cx);
                                }),
                        )
                    })
                    // Description column (manager tables). When `description_flex`
                    // is set the column grows/shrinks so a wide value can't push
                    // the trailing delete button off-screen.
                    .when(show_description, |this| {
                        let col = if description_flex {
                            div().flex_1().min_w(px(80.))
                        } else {
                            div().w(px(180.)).flex_shrink_0()
                        };
                        this.child(
                            col.child(Input::new(&row.description).small().appearance(false)),
                        )
                    })
                    // File picker button (form-data + File type).
                    .when(allow_files && is_file, |this| {
                        this.child(
                            Button::new(("kv-file", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::File)
                                .tooltip(row.file_path.clone().unwrap_or_else(|| "选择文件".into()))
                                .on_click(move |_, window, cx| {
                                    (on_file)(ix, window, cx);
                                }),
                        )
                    })
                    .child(
                        Button::new(("kv-del", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("移除")
                            .on_click(move |_, window, cx| {
                                (delete)(ix, window, cx);
                            }),
                    )
            }))
            .child({
                let add = self.handlers.on_add.clone();
                h_flex().w_full().child(
                    Button::new("kv-add-row")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label("添加一行")
                        .on_click(move |_, window, cx| {
                            (add)(window, cx);
                        }),
                )
            })
    }
}

/// Helper: build a fresh set of rows from `pairs`, ensuring exactly one
/// trailing empty row for quick entry.
pub fn rows_from_pairs(pairs: &[KeyValue], window: &mut Window, cx: &mut App) -> Vec<KvRow> {
    let mut rows: Vec<KvRow> = pairs.iter().map(|kv| KvRow::new(kv, window, cx)).collect();
    let needs_trailing = rows
        .last()
        .map(|r| {
            !r.key.read(cx).value().trim().is_empty() || !r.value.read(cx).value().trim().is_empty()
        })
        .unwrap_or(true);
    if needs_trailing {
        rows.push(KvRow::empty(window, cx));
    }
    rows
}

/// Collect rows back into pairs, dropping the trailing empty row.
pub fn pairs_from_rows(rows: &[KvRow], cx: &App) -> Vec<KeyValue> {
    rows.iter()
        .filter(|r| {
            !(r.key.read(cx).value().trim().is_empty()
                && r.value.read(cx).value().trim().is_empty())
        })
        .map(|r| r.to_kv(cx))
        .collect()
}
