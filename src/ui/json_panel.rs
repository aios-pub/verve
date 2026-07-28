//! JSON Formatter panel — left pane raw input, right pane formatted collapsible code view.
//! Supports expand/collapse per node, all expand/collapse, copy result, and compact mode.
//!
//! # Performance note
//! The output view is a directly-virtualized `uniform_list` over a flat `Vec<FlatRow>`.
//! We deliberately avoid `gpui_component::tree` here: its `TreeItem` is `#[derive(Clone)]`
//! with `children: Vec<TreeItem>`, so every `set_items`/`rebuild_entries` deep-clones each
//! node's entire subtree — O(N·depth) regardless of virtualization. By flattening the JSON
//! into a one-dimensional row list ourselves and rendering only the visible range, we get
//! true O(visible) rendering with O(N) flattening and zero deep cloning.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::{ActiveTheme, Sizable as _, h_flex, v_flex};

/// Threshold above which a non-root container is rendered collapsed by default,
/// so opening a 100k-element array never eagerly flattens the whole thing.
const COLLAPSE_THRESHOLD: usize = 200;

/// Identifies a node by its path of child indices from the root, e.g. the 3rd element
/// of the 2nd element of the root array is `[2, 3]`. Used as the expansion-state key.
/// (Named `NodePath` to avoid clashing with `gpui::Path` brought in by `use gpui::*`.)
type NodePath = Vec<u32>;

/// A single rendered line in the flattened JSON view.
#[derive(Clone)]
struct FlatRow {
    depth: usize,
    /// Stable identity for this row (its path); used as the element id and to toggle
    /// expansion when the row is clicked.
    path: NodePath,
    kind: RowKind,
}

/// What a [`FlatRow`] shows. This carries the same information the old code encoded into
/// `TreeItem` label strings (e.g. `"key[|count"`) but typed, so the render closure does no
/// parsing per visible row.
#[derive(Clone)]
enum RowKind {
    /// Opening line of an object: `key{` (expanded) or `key{...count}` (collapsed).
    Object {
        key: String,
        count: usize,
        expanded: bool,
    },
    /// Opening line of an array: `key[` (expanded) or `key[...count]` (collapsed).
    Array {
        key: String,
        count: usize,
        expanded: bool,
    },
    /// A primitive value: `key: value` (or just `value` for root/array elements).
    /// `needs_comma` appends a trailing comma.
    Primitive {
        key: String,
        value: String,
        ty: ValueTy,
        needs_comma: bool,
    },
    /// Closing `}` or `]`, dedented to the container's depth.
    Close {
        bracket: char,
        needs_comma: bool,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum ValueTy {
    String,
    Number,
    Bool,
    Null,
}

pub struct JsonPanel {
    input: Entity<InputState>,
    /// Latest (possibly compacted) parsed value, kept for expand/collapse rebuilds and copy.
    value: Option<serde_json::Value>,
    /// Pretty-printed form, computed lazily only when the user copies — NOT on every parse
    /// (avoids a ~1s `to_string_pretty` cost on large documents).
    formatted_json: Option<String>,
    /// True while a background parse is in flight. Drives the "解析中…" placeholder so the
    /// user never stares at a blank panel during a multi-second parse.
    parsing: bool,
    error: Option<String>,
    notice: Option<String>,
    /// When true, every array is truncated to its first element (recursively).
    /// This is a *view* toggle — it never alters the raw input. Editing/pasting
    /// new content always reverts to the full (non-compacted) view.
    compact: bool,
    /// Flattened rows currently on display, shared with the render closure via `Arc` so
    /// rendering never deep-clones them (only an atomic refcount bump per frame).
    /// Recomputed from `value` + `expanded` in `rebuild_rows`.
    rows: Arc<Vec<FlatRow>>,
    /// Set of object/array paths whose children are currently shown. A container not in
    /// this set is collapsed.
    expanded: HashSet<NodePath>,
    /// Debounce timer for input-driven formatting; reassigning cancels the prior task.
    format_timer: Option<Task<()>>,
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

        // Auto-format when input changes (paste/edit).
        // - Blur: format immediately.
        // - Change (keystroke/paste/IME): debounce so large documents don't re-parse
        //   on every keystroke. Editing always reverts to the full (non-compacted) view
        //   so pasting fresh JSON is never silently truncated.
        let input_clone = input.clone();
        let input_sub = cx.subscribe(&input, move |this: &mut Self, _src, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Blur) {
                let raw = input_clone.read(cx).value().to_string();
                this.compact = false;
                this.cancel_format_timer();
                this.do_format(&raw, false, cx);
            } else if matches!(ev, InputEvent::Change) {
                let raw = input_clone.read(cx).value().to_string();
                this.schedule_format(raw, cx);
            }
        });

        Self {
            input,
            value: None,
            formatted_json: None,
            parsing: false,
            error: None,
            notice: None,
            compact: false,
            rows: Arc::new(Vec::new()),
            expanded: HashSet::new(),
            format_timer: None,
            _subs: vec![input_sub],
        }
    }

    /// Whether compact mode (truncate each array to one element) is currently shown.
    pub fn is_compact_active(&self) -> bool {
        self.compact
    }

    /// Toggle compact mode and re-format against the current input.
    pub fn toggle_compact(&mut self, cx: &mut Context<Self>) {
        self.compact = !self.compact;
        let raw = self.input.read(cx).value().to_string();
        self.do_format(&raw, self.compact, cx);
        // Surface the outcome so the user knows the toggle was applied.
        // do_format clears `notice`; only set a message when it actually produced output.
        if self.formatted_json.is_some() {
            let key = if self.compact {
                "json.compact_on"
            } else {
                "json.compact_off"
            };
            self.notice = Some(rust_i18n::t!(key).to_string());
            cx.notify();
        }
    }

    /// "Format" always reverts to the full (non-compacted) view, so a user can
    /// recover the complete data after toggling compact.
    pub fn format_from_title(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().to_string();
        self.compact = false;
        self.cancel_format_timer();
        self.do_format(&raw, false, cx);
    }

    pub fn expand_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ref value) = self.value {
            self.expanded.clear();
            collect_all_paths(value, &mut Vec::new(), &mut self.expanded);
            self.rebuild_rows();
            cx.notify();
        }
    }

    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.rebuild_rows();
        cx.notify();
    }

    pub fn copy_result(&mut self, cx: &mut Context<Self>) {
        // Pretty-print is computed lazily here (not on every parse) since it is the only
        // consumer and costs ~1s on large documents.
        let Some(ref value) = self.value else { return };
        if self.formatted_json.is_none() {
            self.formatted_json = Some(serde_json::to_string_pretty(value).unwrap_or_default());
        }
        if let Some(ref json_str) = self.formatted_json {
            cx.write_to_clipboard(ClipboardItem::new_string(json_str.clone()));
            self.notice = Some(rust_i18n::t!("json.copy_success").to_string());
            cx.notify();
        }
    }

    /// Debounce the input-driven format by 300ms. Re-assigning the timer cancels
    /// any pending run, so rapid keystrokes coalesce into a single parse pass.
    fn schedule_format(&mut self, raw: String, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        // Editing always reverts to the full view.
        let timer = cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = weak.update(cx, |this, cx| {
                this.do_format(&raw, false, cx);
            });
        });
        self.format_timer = Some(timer);
    }

    fn cancel_format_timer(&mut self) {
        // Task dropped -> pending timer cancelled.
        self.format_timer.take();
    }

    fn do_format(&mut self, raw: &str, compact: bool, cx: &mut Context<Self>) {
        if raw.trim().is_empty() {
            self.error = None;
            self.formatted_json = None;
            self.value = None;
            self.parsing = false;
            self.rows = Arc::new(Vec::new());
            self.expanded.clear();
            cx.notify();
            return;
        }

        // Immediate feedback: clear stale output and show "解析中…" so the user never waits
        // in front of a blank panel. The actual parse runs off-thread.
        self.error = None;
        self.notice = None;
        self.parsing = true;
        self.value = None;
        self.rows = Arc::new(Vec::new());
        cx.notify();

        // Parse + simplify off the UI thread. We intentionally do NOT `to_string_pretty`
        // here — that costs ~1s on large docs and is only needed for "Copy", which computes
        // it lazily (see `copy_result`).
        let raw_owned = raw.to_string();
        let weak = cx.weak_entity();
        cx.spawn(async move |_this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    match serde_json::from_str::<serde_json::Value>(&raw_owned) {
                        Ok(value) => {
                            let value = if compact { simplify(&value) } else { value };
                            Ok(value)
                        }
                        Err(e) => Err(e),
                    }
                })
                .await;

            let _ = weak.update(cx, |this, cx| this.apply_format_result(parsed, cx));
        })
        .detach();
    }

    /// Apply a background-parsed result on the UI thread: flatten into rows and update state.
    fn apply_format_result(
        &mut self,
        parsed: Result<serde_json::Value, serde_json::Error>,
        cx: &mut Context<Self>,
    ) {
        match parsed {
            Ok(value) => {
                // Pretty form is computed lazily on copy; drop any stale cached string.
                self.formatted_json = None;
                self.value = Some(value);
                self.error = None;
                self.parsing = false;
                // Reset to the default expansion: root shown, deep large containers collapsed.
                self.expanded.clear();
                if let Some(ref v) = self.value {
                    seed_default_expanded(v, &mut Vec::new(), &mut self.expanded);
                }
                self.rebuild_rows();
            }
            Err(e) => {
                self.error = Some(format!("{}: {}", rust_i18n::t!("json.invalid_json"), e));
                self.formatted_json = None;
                self.value = None;
                self.parsing = false;
                self.rows = Arc::new(Vec::new());
            }
        }
        self.notice = None;
        cx.notify();
    }

    /// Toggle the container at `path` and recompute the visible rows.
    fn toggle_path(&mut self, path: &[u32], cx: &mut Context<Self>) {
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_vec());
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// Rebuild [`self.rows`] from [`self.value`] and the current expansion set.
    fn rebuild_rows(&mut self) {
        let Some(value) = &self.value else {
            self.rows = Arc::new(Vec::new());
            return;
        };
        let mut rows = Vec::new();
        flatten(
            value,
            "",
            0,
            true, // root: no trailing comma, and depth 0 containers expand by default
            &self.expanded,
            &mut Vec::new(),
            &mut rows,
        );
        // Wrap once in an Arc; the render closure only bumps the refcount per frame.
        self.rows = Arc::new(rows);
    }
}

/// Flatten a JSON value into display rows.
///
/// `path` is the node's index-path from the root (built up during recursion).
/// `key` is the rendered key prefix (`"name": `) for object children, or empty for array
/// elements / the root. `is_last` indicates whether this node is the final child of its
/// parent (controls trailing commas). This is O(N) over the *visible* subtree — collapsed
/// containers emit a single summary row and stop recursing.
fn flatten(
    value: &serde_json::Value,
    key: &str,
    depth: usize,
    is_last: bool,
    expanded: &HashSet<NodePath>,
    path: &mut Vec<u32>,
    out: &mut Vec<FlatRow>,
) {
    let key_prefix = if key.is_empty() {
        String::new()
    } else {
        format!("\"{}\": ", key)
    };

    match value {
        serde_json::Value::Object(map) => {
            let is_expanded = depth == 0 || expanded.contains(path);
            out.push(FlatRow {
                depth,
                path: path.clone(),
                kind: RowKind::Object {
                    key: key_prefix.clone(),
                    count: map.len(),
                    expanded: is_expanded,
                },
            });
            if is_expanded {
                let count = map.len();
                for (i, (k, v)) in map.iter().enumerate() {
                    path.push(i as u32);
                    let last = i + 1 == count;
                    flatten(v, k, depth + 1, last, expanded, path, out);
                    path.pop();
                }
                out.push(FlatRow {
                    depth,
                    path: {
                        let mut p = path.clone();
                        p.push(u32::MAX); // close marker, unique within parent
                        p
                    },
                    kind: RowKind::Close {
                        bracket: '}',
                        needs_comma: !is_last,
                    },
                });
            }
        }
        serde_json::Value::Array(arr) => {
            let is_expanded = depth == 0 || expanded.contains(path);
            out.push(FlatRow {
                depth,
                path: path.clone(),
                kind: RowKind::Array {
                    key: key_prefix.clone(),
                    count: arr.len(),
                    expanded: is_expanded,
                },
            });
            if is_expanded {
                let count = arr.len();
                for (i, v) in arr.iter().enumerate() {
                    path.push(i as u32);
                    let last = i + 1 == count;
                    flatten(v, "", depth + 1, last, expanded, path, out);
                    path.pop();
                }
                out.push(FlatRow {
                    depth,
                    path: {
                        let mut p = path.clone();
                        p.push(u32::MAX);
                        p
                    },
                    kind: RowKind::Close {
                        bracket: ']',
                        needs_comma: !is_last,
                    },
                });
            }
        }
        serde_json::Value::String(s) => out.push(FlatRow {
            depth,
            path: path.clone(),
            kind: RowKind::Primitive {
                key: key_prefix.clone(),
                value: format!("\"{}\"", s),
                ty: ValueTy::String,
                needs_comma: !is_last,
            },
        }),
        serde_json::Value::Number(n) => out.push(FlatRow {
            depth,
            path: path.clone(),
            kind: RowKind::Primitive {
                key: key_prefix.clone(),
                value: n.to_string(),
                ty: ValueTy::Number,
                needs_comma: !is_last,
            },
        }),
        serde_json::Value::Bool(b) => out.push(FlatRow {
            depth,
            path: path.clone(),
            kind: RowKind::Primitive {
                key: key_prefix.clone(),
                value: b.to_string(),
                ty: ValueTy::Bool,
                needs_comma: !is_last,
            },
        }),
        serde_json::Value::Null => out.push(FlatRow {
            depth,
            path: path.clone(),
            kind: RowKind::Primitive {
                key: key_prefix.clone(),
                value: "null".to_string(),
                ty: ValueTy::Null,
                needs_comma: !is_last,
            },
        }),
    }
}

/// Seed [`expanded`] with the default-visible containers: the root is always expanded,
/// and any container with `<= COLLAPSE_THRESHOLD` children starts expanded. Large/deep
/// containers start collapsed so their subtrees are not eagerly flattened.
fn seed_default_expanded(
    value: &serde_json::Value,
    path: &mut Vec<u32>,
    expanded: &mut HashSet<NodePath>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if path.is_empty() {
                expanded.insert(path.clone());
            } else if map.len() <= COLLAPSE_THRESHOLD {
                expanded.insert(path.clone());
            }
            for (i, (_, v)) in map.iter().enumerate() {
                path.push(i as u32);
                seed_default_expanded(v, path, expanded);
                path.pop();
            }
        }
        serde_json::Value::Array(arr) => {
            if path.is_empty() {
                expanded.insert(path.clone());
            } else if arr.len() <= COLLAPSE_THRESHOLD {
                expanded.insert(path.clone());
            }
            for (i, v) in arr.iter().enumerate() {
                path.push(i as u32);
                seed_default_expanded(v, path, expanded);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Collect the path of every container (used by "Expand All").
fn collect_all_paths(
    value: &serde_json::Value,
    path: &mut Vec<u32>,
    out: &mut HashSet<NodePath>,
) {
    match value {
        serde_json::Value::Object(map) => {
            out.insert(path.clone());
            for (i, (_, v)) in map.iter().enumerate() {
                path.push(i as u32);
                collect_all_paths(v, path, out);
                path.pop();
            }
        }
        serde_json::Value::Array(arr) => {
            out.insert(path.clone());
            for (i, v) in arr.iter().enumerate() {
                path.push(i as u32);
                collect_all_paths(v, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Recursively simplify a JSON value: every array keeps only its first element.
/// Objects are walked key-by-key; primitives are returned as-is. Empty arrays stay empty.
///
/// The result remains valid JSON, so it is safe to expose via copy.
fn simplify(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), simplify(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            if let Some(first) = arr.first() {
                serde_json::Value::Array(vec![simplify(first)])
            } else {
                serde_json::Value::Array(vec![])
            }
        }
        // Primitive: clone unchanged
        _ => value.clone(),
    }
}

impl Render for JsonPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        // Build the resizable split: left = input, right = virtualized output view.
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
                    .child(Input::new(&self.input).small().h_full()),
            )
            .when_some(self.error.as_ref(), |flex, err| {
                flex.child(div().text_sm().text_color(theme.danger).child(err.clone()))
            });

        let mono_font = theme.mono_font_family.clone();
        let fg = theme.foreground;
        let warn = theme.warning;
        let muted_fg = theme.muted_foreground;
        let success = theme.success;
        let info = theme.info;
        // Captured into the (per-frame) uniform_list closure so visible-row clicks can
        // toggle expansion on this panel via its weak handle.
        let weak = cx.weak_entity();

        // Snapshot rows for the render closure (it must be 'static for uniform_list).
        let rows = self.rows.clone();
        let row_count = rows.len();
        let has_output = row_count > 0;
        let parsing = self.parsing;

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
                    .when_some(self.notice.as_ref(), |flex, status| {
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
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(px(4.))
                    .p_1()
                    .bg(theme.muted.opacity(0.3))
                    // Virtualized list: only the visible range of `rows` is ever rendered,
                    // so a 100k-row array scrolls as smoothly as a 50-row one.
                    .child(
                        uniform_list("json-rows", row_count, move |visible_range, _window, cx| {
                            let mono_font = mono_font.clone();
                            visible_range
                                .map(|ix| {
                                    render_row(
                                        ix,
                                        &rows[ix],
                                        &mono_font,
                                        fg,
                                        warn,
                                        muted_fg,
                                        success,
                                        info,
                                        weak.clone(),
                                    )
                                })
                                .collect()
                        })
                        .flex_grow_1()
                        .size_full(),
                    )
                    // Parsing-in-progress placeholder: shown the instant paste happens,
                    // while the background parse is still running. Replaces the blank panel
                    // so the user always knows work is underway.
                    .when(parsing, |c| {
                        c.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .text_color(theme.muted_foreground)
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(gpui_component::spinner::Spinner::new())
                                        .child(
                                            div()
                                                .text_sm()
                                                .child(rust_i18n::t!("json.parsing").to_string()),
                                        ),
                                ),
                        )
                    })
                    .when(!has_output && !parsing, |c| {
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
            .child(resizable_panel().overflow_hidden().child(output_el))
    }
}

/// Render a single flattened row. This runs only for visible rows (virtualized), so the
/// small per-row allocations here are bounded by the viewport regardless of JSON size.
#[allow(clippy::too_many_arguments)]
fn render_row(
    ix: usize,
    row: &FlatRow,
    mono_font: &SharedString,
    fg: Hsla,
    warn: Hsla,
    muted_fg: Hsla,
    success: Hsla,
    info: Hsla,
    weak: WeakEntity<JsonPanel>,
) -> AnyElement {
    let indent_width = 18.;
    let chevron_size = 18.;
    let row_h = 22.;

    let base = div()
        .id(ix)
        .h(px(row_h))
        .pl(px(indent_width) * row.depth)
        .font_family(mono_font.clone())
        .text_sm();

    match &row.kind {
        RowKind::Object {
            key,
            count,
            expanded,
        } => {
            let label = if *expanded {
                format!("{}{}", key, "{")
            } else {
                format!("{}{{...{}}}", key, count)
            };
            base.child(row_open(*expanded, chevron_size, muted_fg, fg, label, mono_font))
                .on_click({
                    let path = row.path.clone();
                    let weak = weak.clone();
                    move |_ev, _window, cx| {
                        let _ = weak.update(cx, |panel, cx| panel.toggle_path(&path, cx));
                    }
                })
                .into_any_element()
        }
        RowKind::Array {
            key,
            count,
            expanded,
        } => {
            let label = if *expanded {
                format!("{}[", key)
            } else {
                format!("{}[...{}]", key, count)
            };
            base.child(row_open(*expanded, chevron_size, muted_fg, fg, label, mono_font))
                .on_click({
                    let path = row.path.clone();
                    let weak = weak.clone();
                    move |_ev, _window, cx| {
                        let _ = weak.update(cx, |panel, cx| panel.toggle_path(&path, cx));
                    }
                })
                .into_any_element()
        }
        RowKind::Primitive {
            key,
            value,
            ty,
            needs_comma,
        } => {
            let color = match ty {
                ValueTy::String => success,
                ValueTy::Number => info,
                ValueTy::Bool => warn,
                ValueTy::Null => muted_fg,
            };
            base.child(
                h_flex()
                    .gap_0()
                    .items_center()
                    .h_full()
                    .pl(px(chevron_size))
                    .child(div().text_color(fg).child(key.clone()))
                    .child(
                        div().text_color(color).child(if *needs_comma {
                            format!("{},", value)
                        } else {
                            value.clone()
                        }),
                    ),
            )
            .into_any_element()
        }
        RowKind::Close {
            bracket,
            needs_comma,
        } => {
            let text = if *needs_comma {
                format!("{},", bracket)
            } else {
                bracket.to_string()
            };
            base.child(
                h_flex()
                    .gap_0()
                    .items_center()
                    .h_full()
                    .pl(px(chevron_size))
                    .text_color(fg)
                    .child(text),
            )
            .into_any_element()
        }
    }
}

/// Build the leading chevron + label flex for an object/array open row.
fn row_open(
    expanded: bool,
    chevron_size: f32,
    muted_fg: Hsla,
    fg: Hsla,
    label: String,
    mono_font: &SharedString,
) -> Div {
    h_flex()
        .gap_0()
        .items_center()
        .h_full()
        .w_full()
        .child(
            div()
                .w(px(chevron_size))
                .h(px(chevron_size))
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted_fg)
                .font_family(mono_font.clone())
                .text_xs()
                .child(if expanded { "▼" } else { "▶" }),
        )
        .child(div().text_color(fg).child(label))
}

#[cfg(test)]
mod tests {
    use super::{
        collect_all_paths, flatten, seed_default_expanded, simplify, FlatRow, RowKind,
        COLLAPSE_THRESHOLD,
    };
    use serde_json::json;
    use std::collections::HashSet;

    /// Helper: flatten `value` with all containers expanded, returning the row labels.
    fn flat_all(value: &serde_json::Value) -> Vec<String> {
        let mut expanded = HashSet::new();
        let mut path = Vec::new();
        collect_all_paths(value, &mut path, &mut expanded);
        let mut rows = Vec::new();
        flatten(value, "", 0, true, &expanded, &mut Vec::new(), &mut rows);
        rows.iter().map(row_label).collect()
    }

    /// Render a row's visible text (mirrors `render_row` logic) for assertion.
    fn row_label(row: &FlatRow) -> String {
        match &row.kind {
            RowKind::Object { key, count, expanded } => {
                if *expanded {
                    format!("{}{}", key, "{")
                } else {
                    format!("{}{{...{}}}", key, count)
                }
            }
            RowKind::Array { key, count, expanded } => {
                if *expanded {
                    format!("{}[", key)
                } else {
                    format!("{}[...{}]", key, count)
                }
            }
            RowKind::Primitive {
                key,
                value,
                needs_comma,
                ..
            } => {
                if *needs_comma {
                    format!("{}{},", key, value)
                } else {
                    format!("{}{}", key, value)
                }
            }
            RowKind::Close {
                bracket,
                needs_comma,
            } => {
                if *needs_comma {
                    format!("{},", bracket)
                } else {
                    bracket.to_string()
                }
            }
        }
    }

    #[test]
    fn flatten_simple_object_all_expanded() {
        let v = json!({"a": 1, "b": "x"});
        let labels = flat_all(&v);
        assert_eq!(labels, vec!["{", "\"a\": 1,", "\"b\": \"x\"", "}"]);
    }

    #[test]
    fn flatten_simple_array_all_expanded() {
        let v = json!([1, 2, 3]);
        let labels = flat_all(&v);
        assert_eq!(labels, vec!["[", "1,", "2,", "3", "]"]);
    }

    #[test]
    fn flatten_nested_object_all_expanded() {
        let v = json!({"o": {"k": true}});
        let labels = flat_all(&v);
        // "k": true is the sole (last) child of the inner object -> no comma.
        assert_eq!(labels, vec!["{", "\"o\": {", "\"k\": true", "}", "}"]);
    }

    #[test]
    fn flatten_collapsed_array_shows_summary_row() {
        // Empty expanded set: the root object auto-expands (depth 0), but the inner array
        // at path [0] is not in the set, so it collapses to a summary row.
        let v = json!({"items": [1, 2, 3]});
        let expanded = HashSet::new();
        let mut rows = Vec::new();
        flatten(&v, "", 0, true, &expanded, &mut Vec::new(), &mut rows);
        let labels: Vec<String> = rows.iter().map(row_label).collect();
        assert_eq!(labels, vec!["{", "\"items\": [...3]", "}"]);
    }

    #[test]
    fn seed_default_collapses_large_containers() {
        let big: Vec<usize> = (0..(COLLAPSE_THRESHOLD + 1)).collect();
        let v = json!({"small": [1, 2], "big": big});
        let mut expanded = HashSet::new();
        seed_default_expanded(&v, &mut Vec::new(), &mut expanded);
        // Root object (path []) and "small" (path [0]) are expanded; "big" (path [1]) is not.
        assert!(expanded.contains(&vec![]));
        assert!(expanded.contains(&vec![0u32]));
        assert!(!expanded.contains(&vec![1u32]));
    }

    #[test]
    fn collect_all_paths_includes_every_container() {
        let v = json!({"a": [1, {"b": 2}]});
        let mut paths = HashSet::new();
        collect_all_paths(&v, &mut Vec::new(), &mut paths);
        assert!(paths.contains(&vec![]));
        assert!(paths.contains(&vec![0u32]));
        assert!(paths.contains(&vec![0u32, 1u32]));
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn flatten_root_primitive() {
        let labels = flat_all(&json!(42));
        assert_eq!(labels, vec!["42"]);
    }

    #[test]
    fn flatten_handles_empty_containers() {
        let labels = flat_all(&json!({"a": [], "b": {}}));
        // "a" is not the last child, so its empty array's ']' keeps a trailing comma.
        assert_eq!(labels, vec!["{", "\"a\": [", "],", "\"b\": {", "}", "}"]);
    }

    #[test]
    fn simplify_truncates_top_level_array_to_one() {
        let v = json!([1, 2, 3, 4]);
        assert_eq!(simplify(&v), json!([1]));
    }

    #[test]
    fn simplify_walks_object_values() {
        let v = json!({"a": [1, 2, 3], "b": "x", "c": {"d": [9, 8]}});
        assert_eq!(simplify(&v), json!({"a": [1], "b": "x", "c": {"d": [9]}}));
    }

    #[test]
    fn simplify_keeps_empty_array_empty() {
        assert_eq!(simplify(&json!([])), json!([]));
    }

    #[test]
    fn simplify_recurses_into_nested_arrays() {
        let v = json!([[1, 2, 3], [4, 5], [6]]);
        assert_eq!(simplify(&v), json!([[1]]));
    }

    #[test]
    fn simplify_array_first_element_is_object() {
        let v = json!([
            {"id": 1, "tags": ["a", "b"]},
            {"id": 2, "tags": ["c"]}
        ]);
        assert_eq!(simplify(&v), json!([{"id": 1, "tags": ["a"]}]));
    }

    #[test]
    fn simplify_primitive_unchanged() {
        assert_eq!(simplify(&json!("hi")), json!("hi"));
        assert_eq!(simplify(&json!(42)), json!(42));
        assert_eq!(simplify(&json!(null)), json!(null));
        assert_eq!(simplify(&json!(true)), json!(true));
    }

    #[test]
    fn simplify_output_is_valid_json() {
        let v = json!({"list": [{"a": [1, 2]}, {"b": 3}], "k": 1});
        let s = simplify(&v);
        let serialized = serde_json::to_string(&s).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed, json!({"list": [{"a": [1]}], "k": 1}));
    }
}
