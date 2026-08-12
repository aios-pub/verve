//! Right pane: response viewer (status/time/size + Body + Headers).

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    ActiveTheme, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};

use crate::state::{AppEvent, AppState};

pub struct ResponsePanel {
    pub state: Entity<AppState>,
    pub body_view: Entity<InputState>,
    /// Pretty or raw body rendering.
    pub pretty: bool,
    /// Active response sub-tab.
    pub active_tab: RespTab,
    /// Set when a response/selection changed and the body must be reloaded;
    /// reconciled at the top of render where a Window is available.
    pub pending_refresh: bool,
    /// Editable body editor for the (single) success example, rebuilt only
    /// when the example's `saved_at` version changes (e.g. autosave), so user
    /// edits keep focus and aren't clobbered mid-typing.
    pub success_editor: Option<Entity<InputState>>,
    /// `saved_at` snapshot used to detect external success-example changes.
    pub success_version: Option<String>,
    /// Editable body editors for failure examples, one per example.
    pub fail_editors: Vec<Entity<InputState>>,
    /// `saved_at` snapshots parallel to `fail_editors`, for change detection.
    pub fail_versions: Vec<String>,
    /// The request id the editors were last reconciled against. A change here
    /// forces a rebuild so switching requests always refreshes the editors,
    /// even if two requests' examples share a `saved_at` by coincidence.
    pub reconciled_req_id: Option<String>,
    _subs: Vec<gpui::Subscription>,
    focus_handle: FocusHandle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RespTab {
    /// 实时响应 — the live response body (also shows SSE/WS streams).
    Realtime,
    /// 响应头.
    Headers,
    /// Cookie — parsed from Set-Cookie response headers.
    Cookie,
    /// 响应示例 — example/mock response body.
    Example,
    /// 实际请求 — the actual request that was sent.
    ActualRequest,
    /// 控制台 — script/console output.
    Console,
}

impl RespTab {
    fn label(self) -> &'static str {
        match self {
            RespTab::Realtime => "实时响应",
            RespTab::Headers => "响应头",
            RespTab::Cookie => "Cookie",
            RespTab::Example => "响应示例",
            RespTab::ActualRequest => "实际请求",
            RespTab::Console => "控制台",
        }
    }
    fn all() -> [RespTab; 6] {
        [
            RespTab::Realtime,
            RespTab::Headers,
            RespTab::Cookie,
            RespTab::Example,
            RespTab::ActualRequest,
            RespTab::Console,
        ]
    }
    fn index(self) -> usize {
        match self {
            RespTab::Realtime => 0,
            RespTab::Headers => 1,
            RespTab::Cookie => 2,
            RespTab::Example => 3,
            RespTab::ActualRequest => 4,
            RespTab::Console => 5,
        }
    }
    fn from_index(i: usize) -> Self {
        match i {
            1 => RespTab::Headers,
            2 => RespTab::Cookie,
            3 => RespTab::Example,
            4 => RespTab::ActualRequest,
            5 => RespTab::Console,
            _ => RespTab::Realtime,
        }
    }
}

impl ResponsePanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body_view = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("json")
                .placeholder("响应体将在发送后显示。")
        });
        let sub = cx.subscribe(&state, Self::on_state_event);
        let focus_handle = cx.focus_handle();
        Self {
            state,
            body_view,
            pretty: true,
            active_tab: RespTab::Realtime,
            pending_refresh: false,
            success_editor: None,
            success_version: None,
            fail_editors: Vec::new(),
            fail_versions: Vec::new(),
            reconciled_req_id: None,
            _subs: vec![sub],
            focus_handle,
        }
    }

    fn on_state_event(&mut self, _src: Entity<AppState>, _ev: &AppEvent, cx: &mut Context<Self>) {
        // refresh_body needs a Window, which subscribe handlers lack; defer.
        self.pending_refresh = true;
        cx.notify();
    }

    fn refresh_body(&mut self, _id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let resp = self.state.read(cx).active_project().and_then(|p| {
            p.find_request(self.state.read(cx).selected_request.as_deref()?)
                .map(|(_, r)| r.last_response.clone())
        });
        match resp {
            Some(Some(r)) => {
                // If there's an error and no body, show the error in the body
                // area so the user sees WHY the request failed.
                let text = if r.body.is_empty() {
                    if let Some(err) = &r.error {
                        format!("❌ 请求失败\n\n{err}")
                    } else {
                        String::new()
                    }
                } else {
                    r.body.clone()
                };
                self.body_view.update(cx, |s, cx| {
                    s.set_value(&text, window, cx);
                });
            }
            _ => {
                self.body_view.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                });
            }
        }
        cx.notify();
    }

    /// Snapshot the active request's saved examples: success `(body, saved_at)`
    /// and the failure list of `(body, saved_at)`. Returns `None` when no
    /// request is selected.
    fn snapshot_examples(&self, cx: &App) -> ExampleSnapshot {
        let st = self.state.read(cx);
        let req = st.active_project().and_then(|p| {
            p.find_request(st.selected_request.as_deref()?)
                .map(|(_, r)| r)
        });
        match req {
            Some(r) => ExampleSnapshot {
                success: r
                    .success_example
                    .as_ref()
                    .map(|e| (e.body.clone(), e.saved_at.clone())),
                fails: r
                    .fail_examples
                    .iter()
                    .map(|e| (e.body.clone(), e.saved_at.clone()))
                    .collect(),
            },
            None => ExampleSnapshot {
                success: None,
                fails: Vec::new(),
            },
        }
    }

    /// Synchronise the example body editors with the model:
    ///
    /// 1. **Commit** — when an existing editor's value differs from the model
    ///    (user typed something), write it back and persist. This runs only when
    ///    the editor set is *stable* (no rebuild needed), so indices line up.
    /// 2. **Rebuild** — when the example set changed externally (autosave added
    ///    /overwrote an example, detected via `saved_at` version mismatch),
    ///    recreate editors from the model so they reflect the new content.
    ///
    /// Editing the body does not change `saved_at`, so a user edit never
    /// triggers a rebuild — focus and cursor position are preserved.
    fn reconcile_examples(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let req_id = match self.state.read(cx).selected_request.clone() {
            Some(id) => id,
            None => {
                if self.success_editor.is_some() || !self.fail_editors.is_empty() {
                    self.success_editor = None;
                    self.success_version = None;
                    self.fail_editors.clear();
                    self.fail_versions.clear();
                    self.reconciled_req_id = None;
                }
                return;
            }
        };

        // Switching requests always forces a full rebuild.
        let req_changed = self.reconciled_req_id.as_deref() != Some(req_id.as_str());

        let snap = self.snapshot_examples(cx);

        // --- Determine whether a rebuild is needed (before any commit) -------
        let success_need_rebuild = req_changed
            || match (&snap.success, &self.success_version) {
                (Some((_, ver)), sv) => {
                    self.success_editor.is_none() || sv.as_deref() != Some(ver.as_str())
                }
                (None, _) => self.success_editor.is_some(),
            };
        let fail_need_rebuild = req_changed
            || self.fail_editors.len() != snap.fails.len()
            || snap
                .fails
                .iter()
                .zip(self.fail_versions.iter())
                .any(|((_, v), ev)| v != ev);

        // --- Commit user edits (only when the editor set is stable) ----------
        let mut new_success_body: Option<String> = None;
        let mut fail_edits: Vec<(usize, String)> = Vec::new();
        if !success_need_rebuild {
            if let (Some(editor), Some((body, _))) = (&self.success_editor, &snap.success) {
                let v = editor.read(cx).value().to_string();
                if v != *body {
                    new_success_body = Some(v);
                }
            }
        }
        if !fail_need_rebuild {
            for (i, editor) in self.fail_editors.iter().enumerate() {
                if let Some((body, _)) = snap.fails.get(i) {
                    let v = editor.read(cx).value().to_string();
                    if v != *body {
                        fail_edits.push((i, v));
                    }
                }
            }
        }

        let changed = new_success_body.is_some() || !fail_edits.is_empty();
        if changed {
            self.state.update(cx, |s, cx| {
                if let Some(project) = s.active_project_mut() {
                    if let Some((_, r)) = project.find_request_mut(&req_id) {
                        if let Some(b) = new_success_body {
                            if let Some(ex) = r.success_example.as_mut() {
                                ex.body = b;
                            }
                        }
                        for (i, b) in &fail_edits {
                            if let Some(ex) = r.fail_examples.get_mut(*i) {
                                ex.body = b.clone();
                            }
                        }
                    }
                }
                s.notify_edited(cx);
            });
        }

        // --- Rebuild editors when the example set changed externally ---------
        if success_need_rebuild {
            match &snap.success {
                Some((body, ver)) => {
                    let editor = cx.new(|cx| {
                        let mut s = InputState::new(window, cx).code_editor("json");
                        s.set_value(body, window, cx);
                        s
                    });
                    self.success_editor = Some(editor);
                    self.success_version = Some(ver.clone());
                }
                None => {
                    self.success_editor = None;
                    self.success_version = None;
                }
            }
        }

        if fail_need_rebuild {
            self.fail_editors = snap
                .fails
                .iter()
                .map(|(body, _)| {
                    cx.new(|cx| {
                        let mut s = InputState::new(window, cx).code_editor("json");
                        s.set_value(body, window, cx);
                        s
                    })
                })
                .collect();
            self.fail_versions = snap.fails.iter().map(|(_, v)| v.clone()).collect();
        }

        self.reconciled_req_id = Some(req_id);
    }

    /// Read display metadata (status, status_text, saved_at) for the saved
    /// examples, in model order. Used for group headers; the editable bodies
    /// come from the editor entities.
    fn example_metas(&self, cx: &App) -> (Option<ExampleMeta>, Vec<ExampleMeta>) {
        let st = self.state.read(cx);
        let req = st.active_project().and_then(|p| {
            p.find_request(st.selected_request.as_deref()?)
                .map(|(_, r)| r)
        });
        match req {
            Some(r) => (
                r.success_example.as_ref().map(ExampleMeta::from),
                r.fail_examples.iter().map(ExampleMeta::from).collect(),
            ),
            None => (None, Vec::new()),
        }
    }

    /// Render the 响应示例 tab: success and failure examples as two
    /// side-by-side columns, each with its own vertical scroll. Every body is
    /// editable. Editors must be reconciled first.
    fn render_example_tab(&self, theme: &gpui_component::Theme, cx: &App) -> AnyElement {
        let (success_meta, fail_metas) = self.example_metas(cx);

        let has_success = success_meta.is_some() && self.success_editor.is_some();
        let has_fail = !fail_metas.is_empty() && !self.fail_editors.is_empty();

        if !has_success && !has_fail {
            return empty_state(
                "暂无响应示例。在“发送后设置”中开启自动保存以自动收录示例。",
                theme,
            );
        }

        // Two independent, side-by-side columns so success and failure
        // examples each scroll on their own instead of sharing one scroll
        // position. A column renders only when it has content: when both exist
        // they split the width 50/50, otherwise the lone column fills it.
        let mut columns: Vec<AnyElement> = Vec::new();

        // ---- 成功示例 column ----
        if has_success {
            if let (Some(meta), Some(editor)) = (&success_meta, &self.success_editor) {
                columns.push(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .gap_2()
                        // Group title stays fixed above the scroll area.
                        .child(div().flex_shrink_0().child(group_header(
                            "成功示例",
                            GroupKind::Success,
                            theme,
                        )))
                        .child(
                            v_flex()
                                .id("resp-success-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .gap_2()
                                .child(example_card(
                                    Some(meta),
                                    GroupKind::Success,
                                    0,
                                    editor,
                                    theme,
                                )),
                        )
                        .into_any_element(),
                );
            }
        }

        // ---- 失败示例 column ----
        if has_fail {
            let mut cards = v_flex().w_full().gap_2();
            for (i, (meta, editor)) in fail_metas.iter().zip(self.fail_editors.iter()).enumerate() {
                cards = cards.child(example_card(
                    Some(meta),
                    GroupKind::Failure,
                    i,
                    editor,
                    theme,
                ));
            }
            columns.push(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_2()
                    .child(div().flex_shrink_0().child(group_header(
                        "失败示例",
                        GroupKind::Failure,
                        theme,
                    )))
                    .child(
                        v_flex()
                            .id("resp-fail-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .gap_2()
                            .child(cards),
                    )
                    .into_any_element(),
            );
        }

        h_flex()
            .size_full()
            .gap_4()
            .children(columns)
            .into_any_element()
    }
}

/// Display metadata for a saved example (everything except the body, which is
/// edited live in an `InputState` editor).
#[derive(Clone)]
struct ExampleMeta {
    status: u16,
    status_text: String,
    saved_at: String,
}

/// Snapshot of the saved examples for reconciliation: `(body, saved_at)` pairs.
/// The `saved_at` acts as a version so external changes (autosave) trigger an
/// editor rebuild, while in-place user edits (which don't touch `saved_at`) do not.
struct ExampleSnapshot {
    success: Option<(String, String)>,
    fails: Vec<(String, String)>,
}

impl From<&crate::state::models::ResponseExample> for ExampleMeta {
    fn from(e: &crate::state::models::ResponseExample) -> Self {
        Self {
            status: e.status,
            status_text: e.status_text.clone(),
            saved_at: e.saved_at.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Success,
    Failure,
}

/// A group title row: colored chip + label.
fn group_header(label: &str, kind: GroupKind, theme: &gpui_component::Theme) -> AnyElement {
    let chip_bg = match kind {
        GroupKind::Success => gpui::hsla(0.33, 0.6, 0.35, 1.0),
        GroupKind::Failure => gpui::hsla(0.0, 0.65, 0.5, 1.0),
    };
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .px_2()
                .py(px(2.))
                .rounded_md()
                .bg(chip_bg)
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::white())
                .child(label.to_string()),
        )
        .child(div().flex_1().h(px(1.)).bg(theme.border))
        .into_any_element()
}

/// One example card: a status/saved-at label row plus an editable body editor.
fn example_card(
    meta: Option<&ExampleMeta>,
    kind: GroupKind,
    index: usize,
    editor: &Entity<InputState>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let badge_bg = match kind {
        GroupKind::Success => gpui::hsla(0.33, 0.6, 0.35, 1.0),
        GroupKind::Failure => gpui::hsla(0.0, 0.65, 0.5, 1.0),
    };
    let badge_label = match kind {
        GroupKind::Success => "成功".to_string(),
        // Failures are stored newest-first, so index 0 is the most recent → #1.
        GroupKind::Failure => format!("#{}", index + 1),
    };
    let status_line = meta
        .map(|m| {
            if m.status_text.is_empty() {
                format!("{}", m.status)
            } else {
                format!("{} {}", m.status, m.status_text)
            }
        })
        .unwrap_or_default();
    let saved_at = meta.map(|m| m.saved_at.clone()).unwrap_or_default();

    v_flex()
        .w_full()
        .gap_1()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded_md()
                        .bg(badge_bg)
                        .text_size(px(11.))
                        .text_color(gpui::white())
                        .child(badge_label),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(status_line),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(saved_at),
                ),
        )
        .child(
            div()
                .w_full()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .child(
                    Input::new(editor)
                        .h(px(220.))
                        .bordered(false)
                        .appearance(false)
                        .font_family(theme.mono_font_family.clone())
                        .text_size(theme.mono_font_size),
                ),
        )
        .into_any_element()
}

impl Render for ResponsePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_refresh {
            self.pending_refresh = false;
            self.refresh_body(None, window, cx);
        }
        let theme = cx.theme().clone();

        let active_request = self.state.read(cx).active_project().and_then(|p| {
            let sel = self.state.read(cx).selected_request.clone();
            sel.and_then(|id| p.find_request(&id).map(|(_, r)| r.clone()))
        });
        let response = active_request
            .as_ref()
            .and_then(|r| r.last_response.clone());

        let status_line = match &response {
            Some(r) if r.status > 0 => format!("{} {}", r.status, r.status_text),
            Some(r) if !r.status_text.is_empty() => r.status_text.clone(),
            Some(r) => r.error.clone().unwrap_or_else(|| "暂无响应".into()),
            None => "暂无响应".to_string(),
        };
        let is_streaming = response.as_ref().map(|r| r.streaming).unwrap_or(false);
        // True when the currently-shown request is in flight (a non-streaming
        // HTTP/GraphQL send). Drives the "请求中…" loading state.
        let sending = {
            let st = self.state.read(cx);
            st.sending
                .as_deref()
                .map(|s| Some(s) == st.selected_request.as_deref())
                .unwrap_or(false)
        };
        let time = response
            .as_ref()
            .map(|r| format!("{} ms", r.time_ms))
            .unwrap_or_default();
        let size = response
            .as_ref()
            .map(|r| format_bytes(r.size))
            .unwrap_or_default();

        let status_color = status_color(response.as_ref().map(|r| r.status).unwrap_or(0));

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(is_streaming, |bar| {
                        bar.child(div().size_2().rounded_full().bg(theme.primary).text_xs())
                    })
                    .child(
                        div()
                            .text_color(status_color)
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(status_line)),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .text_sm()
                            .child(time),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .text_sm()
                            .child(size),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("pretty-toggle")
                            .ghost()
                            .small()
                            .icon(IconName::Menu)
                            .selected(self.pretty)
                            .tooltip("Pretty-print")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pretty = !this.pretty;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().px_3().pt_2().child(
                    TabBar::new("response-tabs")
                        .selected_index(self.active_tab.index())
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.active_tab = RespTab::from_index(*ix);
                            cx.notify();
                        }))
                        .children(RespTab::all().iter().map(|t| Tab::new().label(t.label()))),
                ),
            )
            .child({
                let active_tab = self.active_tab;
                let headers = response
                    .as_ref()
                    .map(|r| r.headers.clone())
                    .unwrap_or_default();
                let error = response.as_ref().and_then(|r| r.error.clone());
                // Cookies parsed from Set-Cookie response headers.
                let cookies: Vec<crate::state::models::KeyValue> = response
                    .as_ref()
                    .map(|r| parse_response_cookies(&r.headers))
                    .unwrap_or_default();
                div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .child(match active_tab {
                        RespTab::Realtime => {
                            if sending && !is_streaming {
                                // In-flight non-streaming request: the previous
                                // body was cleared, so show a perceivable
                                // loading indicator instead of an empty editor.
                                v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .gap_2()
                                    .text_color(theme.muted_foreground)
                                    .child(gpui_component::spinner::Spinner::new())
                                    .child(div().text_sm().child("请求中…"))
                                    .into_any_element()
                            } else {
                                Input::new(&self.body_view)
                                    .h_full()
                                    .bordered(false)
                                    .appearance(false)
                                    .font_family(theme.mono_font_family.clone())
                                    .text_size(theme.mono_font_size)
                                    .into_any_element()
                            }
                        }
                        RespTab::Headers => render_headers(&headers, &theme).into_any_element(),
                        RespTab::Cookie => render_cookies(&cookies, &theme).into_any_element(),
                        RespTab::Example => {
                            self.reconcile_examples(window, cx);
                            self.render_example_tab(&theme, cx).into_any_element()
                        }
                        RespTab::ActualRequest => {
                            render_actual_request(&active_request, &theme).into_any_element()
                        }
                        RespTab::Console => render_console(&response, &theme).into_any_element(),
                    })
                    .when_some(error, |this, err| {
                        this.border_t_1().border_color(theme.danger).child(
                            div()
                                .p_3()
                                .text_color(theme.danger)
                                .child(format!("Error: {err}")),
                        )
                    })
            })
            .when(active_request.is_none(), |this| {
                this.border_color(theme.border)
            })
    }
}

/// Render the response headers as a read-only key/value list.
fn render_headers(
    headers: &[crate::state::models::KeyValue],
    theme: &gpui_component::Theme,
) -> AnyElement {
    let rows: Vec<AnyElement> = headers
        .iter()
        .enumerate()
        .map(|(i, kv)| {
            h_flex()
                .w_full()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .w(px(220.))
                        .text_color(theme.muted_foreground)
                        .child(kv.key.clone()),
                )
                .child(div().flex_1().child(kv.value.clone()))
                .id(("resp-header", i))
                .into_any_element()
        })
        .collect();
    // Wrap in a scrollable container when there are many headers.
    if headers.len() > 16 {
        v_flex()
            .w_full()
            .id("resp-headers-scroll")
            .overflow_y_scroll()
            .gap_1()
            .children(rows)
            .into_any_element()
    } else {
        v_flex().w_full().gap_1().children(rows).into_any_element()
    }
}

/// Extract cookie name/value pairs from Set-Cookie response headers.
fn parse_response_cookies(
    headers: &[crate::state::models::KeyValue],
) -> Vec<crate::state::models::KeyValue> {
    headers
        .iter()
        .filter(|h| h.key.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|h| {
            // A Set-Cookie value looks like: `name=value; Path=/; HttpOnly`.
            let first = h.value.split(';').next()?.trim();
            let (k, v) = first.split_once('=')?;
            Some(crate::state::models::KeyValue::new(k.trim(), v.trim()))
        })
        .collect()
}

/// Render response cookies as a key/value list.
fn render_cookies(
    cookies: &[crate::state::models::KeyValue],
    theme: &gpui_component::Theme,
) -> AnyElement {
    if cookies.is_empty() {
        return empty_state("该响应没有 Cookie。", theme);
    }
    let rows: Vec<AnyElement> = cookies
        .iter()
        .enumerate()
        .map(|(i, kv)| {
            h_flex()
                .w_full()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .w(px(220.))
                        .text_color(theme.muted_foreground)
                        .child(kv.key.clone()),
                )
                .child(div().flex_1().child(kv.value.clone()))
                .id(("resp-cookie", i))
                .into_any_element()
        })
        .collect();
    v_flex().w_full().gap_1().children(rows).into_any_element()
}

/// Render the actual-request tab: the method/URL/headers/body that was sent.
fn render_actual_request(
    active_request: &Option<crate::state::models::ApiRequest>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let req = match active_request {
        Some(r) => r,
        None => return empty_state("暂无请求信息。", theme),
    };
    let mut text = format!("{} {} {}\n", req.protocol, req.method, req.url);
    if !req.headers.is_empty() {
        text.push_str("\n[Headers]\n");
        for h in req.headers.iter().filter(|h| h.enabled && !h.is_empty()) {
            text.push_str(&format!("{}: {}\n", h.key, h.value));
        }
    }
    if !req.cookies.is_empty() {
        text.push_str("\n[Cookies]\n");
        for c in req.cookies.iter().filter(|c| c.enabled && !c.is_empty()) {
            text.push_str(&format!("{}={}\n", c.key, c.value));
        }
    }
    if !req.body.raw.is_empty() {
        text.push_str("\n[Body]\n");
        text.push_str(&req.body.raw);
    }
    code_block(&text, theme)
}

/// Render the console tab: script logs embedded in the response body footer
/// (under the `// ── Script Output ──` marker).
fn render_console(
    response: &Option<crate::state::models::Response>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let body = response.as_ref().map(|r| r.body.as_str()).unwrap_or("");
    if let Some(idx) = body.find("── Script Output ──") {
        let logs = &body[idx..];
        code_block(logs, theme)
    } else {
        empty_state("暂无控制台输出。运行预执行/后执行脚本后会在此显示。", theme)
    }
}

/// A monospace code block in a scrollable container.
fn code_block(text: &str, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .id("resp-code-block")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .text_sm()
        .font_family(theme.mono_font_family.clone())
        .text_size(theme.mono_font_size)
        .text_color(theme.foreground)
        .child(text.to_string())
        .into_any_element()
}

/// A centered muted placeholder.
fn empty_state(msg: &str, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.muted_foreground)
        .child(msg.to_string())
        .into_any_element()
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn status_color(status: u16) -> gpui::Hsla {
    // Approximation using theme-agnostic hues.
    use gpui::hsla;
    if status == 0 {
        return hsla(0.0, 0.0, 0.5, 1.0);
    }
    if status >= 200 && status < 300 {
        hsla(0.33, 0.8, 0.4, 1.0) // green
    } else if status >= 300 && status < 400 {
        hsla(0.11, 0.8, 0.45, 1.0) // amber
    } else if status >= 400 {
        hsla(0.0, 0.75, 0.5, 1.0) // red
    } else {
        hsla(0.6, 0.0, 0.5, 1.0)
    }
}

impl Focusable for ResponsePanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for ResponsePanel {}
