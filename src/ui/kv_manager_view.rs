//! Generic key-value management dialog view.
//!
//! Reused for the project-level "Global parameters", "Global variables", and
//! "Cookie manager" dialogs. Each edits one `Vec<KeyValue>` slice on the active
//! project, with add/remove/toggle/type rows identical to the request kv table.

use std::sync::Arc;

use gpui::*;
use gpui_component::{
    ActiveTheme, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::state::models::KeyValue;
use crate::state::{AppEvent, AppState};
use crate::ui::kv_table::{self, KvHandlers, KvRow};

/// Which slice of the active project this manager edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KvScope {
    GlobalParams,
    GlobalHeaders,
    GlobalVariables,
    GlobalCookies,
}

impl KvScope {
    pub fn title(&self) -> &'static str {
        match self {
            KvScope::GlobalParams => "全局参数",
            KvScope::GlobalHeaders => "全局请求头",
            KvScope::GlobalVariables => "全局变量",
            KvScope::GlobalCookies => "Cookie 管理器",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            KvScope::GlobalParams => "所有接口自动合并这些 query 参数（接口级同名参数覆盖）。",
            KvScope::GlobalHeaders => "所有接口自动合并这些请求头（接口级同名头覆盖）。",
            KvScope::GlobalVariables => {
                "全项目共享的变量，作用域最低，可被环境/目录/接口级变量覆盖。"
            }
            KvScope::GlobalCookies => {
                "项目级 Cookie，按 key=value 形式自动注入到请求的 Cookie 头。"
            }
        }
    }

    pub fn show_type(&self) -> bool {
        // Type/required selectors only make sense for params/variables.
        matches!(self, KvScope::GlobalParams | KvScope::GlobalVariables)
    }

    pub fn allow_files(&self) -> bool {
        false
    }
}

pub struct KvManagerView {
    pub state: Entity<AppState>,
    pub scope: KvScope,
    pub rows: Vec<KvRow>,
    pub pending_add: bool,
    /// Set when the workspace changed and the rows must be reloaded; the
    /// reload needs a Window so it runs at the top of render.
    pub pending_reload: bool,
    _subs: Vec<gpui::Subscription>,
}

impl KvManagerView {
    pub fn new(
        state: Entity<AppState>,
        scope: KvScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pairs = read_scope(&state, scope, cx);
        let mut view = Self {
            state,
            scope,
            rows: kv_table::rows_from_pairs(&pairs, window, cx),
            pending_add: false,
            pending_reload: false,
            _subs: Vec::new(),
        };
        // Reload rows when the workspace structurally changes. The reload
        // needs a Window, so it's deferred to render via pending_reload.
        view._subs.push(
            cx.subscribe(&view.state.clone(), move |this, _src, ev: &AppEvent, cx| {
                if matches!(ev, AppEvent::WorkspaceChanged) {
                    this.pending_reload = true;
                    cx.notify();
                }
            }),
        );
        view
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let pairs = kv_table::pairs_from_rows(&self.rows, cx);
        let scope = self.scope;
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                match scope {
                    KvScope::GlobalParams => project.global_params = pairs,
                    KvScope::GlobalHeaders => project.global_headers = pairs,
                    KvScope::GlobalVariables => project.global_variables = pairs,
                    KvScope::GlobalCookies => project.global_cookies = pairs,
                }
            }
            s.notify_edited(cx);
        });
    }

    fn toggle(&mut self, ix: usize, val: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(ix) {
            row.enabled = val;
        }
        self.commit(cx);
        cx.notify();
    }

    fn delete(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.rows.len() {
            self.rows.remove(ix);
        }
        self.commit(cx);
        cx.notify();
    }

    fn change_type(
        &mut self,
        ix: usize,
        ft: crate::state::models::FieldType,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.rows.get_mut(ix) {
            row.field_type = ft;
        }
        self.commit(cx);
        cx.notify();
    }

    fn toggle_required(&mut self, ix: usize, req: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get_mut(ix) {
            row.required = req;
        }
        self.commit(cx);
        cx.notify();
    }
}

/// Read the current pairs for a scope out of the active project.
fn read_scope(state: &Entity<AppState>, scope: KvScope, cx: &App) -> Vec<KeyValue> {
    let s = state.read(cx);
    let project = match s.active_project() {
        Some(p) => p,
        None => return Vec::new(),
    };
    match scope {
        KvScope::GlobalParams => project.global_params.clone(),
        KvScope::GlobalHeaders => project.global_headers.clone(),
        KvScope::GlobalVariables => project.global_variables.clone(),
        KvScope::GlobalCookies => project.global_cookies.clone(),
    }
}

impl Render for KvManagerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Reconcile a pending workspace-change reload (needs a Window).
        if self.pending_reload {
            self.pending_reload = false;
            let pairs = read_scope(&self.state, self.scope, cx);
            self.rows = kv_table::rows_from_pairs(&pairs, window, cx);
        }
        // Reconcile a pending add-row (needs a Window).
        if self.pending_add {
            self.pending_add = false;
            self.rows.push(KvRow::empty(window, cx));
        }
        let theme = cx.theme().clone();
        let scope = self.scope;
        let show_type = scope.show_type();
        let allow_files = scope.allow_files();
        let rows = self.rows.clone();
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let handlers = KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| this.toggle(ix, val, cx));
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| this.delete(ix, cx));
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.pending_add = true;
                    cx.notify();
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| this.change_type(ix, ft, cx));
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| this.toggle_required(ix, val, cx));
            }),
            on_file_pick: Arc::new(|_, _, _| {}),
        };

        v_flex()
            .w_full()
            .h(px(420.))
            .gap_2()
            // Header: title + description.
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(scope.title()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(scope.description()),
                    ),
            )
            // Toolbar: add button on the right.
            .child(
                h_flex().child(div().flex_1()).child(
                    Button::new("kv-add")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label("新增")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_add = true;
                            cx.notify();
                        })),
                ),
            )
            // The kv table (scrolls internally if long).
            .child(
                div()
                    .id("kv-manager-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        kv_table::KvTable::new("kv-manager", rows, handlers)
                            .show_type(show_type)
                            .show_required(true)
                            .show_description(true)
                            .allow_files(allow_files),
                    ),
            )
    }
}
