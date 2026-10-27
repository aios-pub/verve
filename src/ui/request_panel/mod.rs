//! Center pane: the request editor workbench.
//!
//! Method selector + URL bar + Send, and tabs for Params / Headers / Body /
//! Scripts / Docs. On Send, resolves variables, executes the request via the
//! shared HTTP client, and writes the response back into the request's
//! `last_response`, emitting [`AppEvent::ResponseUpdated`].

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Icon;
use gpui_component::WindowExt as _;
use gpui_component::input::{Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    popover::Popover,
    v_flex,
};

use crate::http;
use crate::state::models::*;
use crate::state::{AppEvent, AppState};
use crate::ui::kv_table::{self, KvRow};

// Action bound to Cmd/Ctrl+Enter to send the active request.
gpui::actions!(verve, [SendRequest]);

// ---- sibling impl modules (split by responsibility) ----
pub(crate) mod folder_helpers;
mod kv;
mod send;
mod tabs;

use folder_helpers::resolve_folder_base_url;

/// Shared stop-flags for in-flight streaming requests (SSE/WebSocket), keyed by
/// request id. A background task polls its flag and aborts once it goes true.
static ACTIVE_STOP_FLAGS: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    >,
> = std::sync::OnceLock::new();

pub(super) fn stop_flags() -> &'static std::sync::Mutex<
    std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
> {
    ACTIVE_STOP_FLAGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Register a stop flag for a request id and return it for the background task.
pub(super) fn register_stop(id: &str) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    stop_flags()
        .lock()
        .unwrap()
        .insert(id.to_string(), flag.clone());
    flag
}

/// Remove a request's stop flag once its task completes.
pub(super) fn unregister_stop(id: &str) {
    stop_flags().lock().unwrap().remove(id);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReqTab {
    Headers,
    Query,
    Path,
    Body,
    Auth,
    Cookie,
    /// Pre-request script.
    PreRequest,
    /// Post-request / tests script.
    PostRequest,
    /// Generated curl code.
    Curl,
    /// Mock rule configuration.
    Mock,
}

pub struct RequestPanel {
    pub state: Entity<AppState>,
    pub request_id: Option<String>,
    // --- editable editor entities (rebuilt when the selected request changes)
    pub url: Entity<InputState>,
    /// Base URL prefix for the current request (independent of folder config).
    pub req_base_url: Entity<InputState>,
    /// Tri-state override mode mirroring `ApiRequest::base_url_override`:
    /// `None` = inherit folder config; `Some(None)` = explicit no prefix;
    /// `Some(Some(url))` = use this url. Driving resolution off this (instead
    /// of treating an empty input as "inherit") is what lets the user clear
    /// the prefix without it silently falling back to the folder base_url.
    pub req_base_mode: Option<Option<String>>,
    /// Whether the base URL popover is open.
    pub req_baseurl_open: bool,
    /// Whether the tab-bar overflow ("»") dropdown popover is open.
    pub tab_overflow_open: bool,
    pub name: Entity<InputState>,
    pub method_select: Entity<SelectState<Vec<String>>>,
    pub params_rows: Vec<KvRow>,
    pub headers_rows: Vec<KvRow>,
    pub path_rows: Vec<KvRow>,
    pub cookie_rows: Vec<KvRow>,
    pub body_rows: Vec<KvRow>,
    pub body_editor: Entity<EditorState>,
    pub body_lang_select: Entity<SelectState<Vec<String>>>,
    pub body_type_select: Entity<SelectState<Vec<String>>>,
    pub pre_script_editor: Entity<TextareaState>,
    pub tests_editor: Entity<TextareaState>,
    // --- mock fields
    pub mock_enabled: bool,
    pub mock_status_input: Entity<InputState>,
    pub mock_delay_input: Entity<InputState>,
    pub mock_body_editor: Entity<EditorState>,
    pub mock_headers_rows: Vec<KvRow>,
    pub mock_match_method_select: Entity<SelectState<Vec<String>>>,
    pub mock_path_pattern_select: Entity<SelectState<Vec<String>>>,
    pub mock_match_path_input: Entity<InputState>,
    pub mock_enable_templates: bool,
    pub mock_match_query_rows: Vec<KvRow>,
    pub mock_match_header_rows: Vec<KvRow>,
    // --- auth fields
    pub auth_type_select: Entity<SelectState<Vec<String>>>,
    pub auth_target_select: Entity<SelectState<Vec<String>>>,
    pub auth_token: Entity<InputState>,
    pub auth_username: Entity<InputState>,
    pub auth_password: Entity<InputState>,
    pub auth_key: Entity<InputState>,
    pub auth_value: Entity<InputState>,
    pub auth_type: AuthType,
    pub auth_target: AuthTarget,
    pub protocol: Protocol,
    pub active_tab: ReqTab,
    pub body_type: BodyType,
    /// Whether the Raw JSON body is shown as a code editor (false) or a visual
    /// field table (true). Visual mode parses the JSON into editable fields.
    pub body_visual_mode: bool,
    /// Persistent KvRow inputs for the visual Raw-body editor. Like
    /// `body_rows`, these are created once at reload (where a Window is
    /// available) and reused across renders so typing doesn't lose focus.
    pub raw_visual_rows: Vec<kv_table::KvRow>,
    /// Set by the visual-mode "Add row" handler (lacks a Window); a new KvRow
    /// with fresh InputState entities is appended on the next render.
    pub pending_visual_add: bool,
    /// Set by the kv "Add row" handler (which lacks a Window); reconciled in
    /// render where a Window is available.
    pub pending_kv_add: bool,
    /// Set by the file-pick handler (lacks a Window) to the row index; the
    /// path dialog is opened in render where a Window is available.
    pub pending_file_pick: Option<usize>,
    /// Set when selection changed and the active request must be reloaded; the
    /// reload needs a Window, so it runs at the top of render.
    pub pending_reload: bool,
    // --- folder detail view state (rebuilt when the selected folder changes)
    pub folder_id: Option<String>,
    pub folder_name: Entity<InputState>,
    pub folder_desc: Entity<TextareaState>,
    /// Whether the folder base URL Popover is open.
    pub folder_baseurl_open: bool,
    pub folder_param_rows: Vec<KvRow>,
    pub folder_header_rows: Vec<KvRow>,
    pub folder_var_rows: Vec<KvRow>,
    pub folder_tab: FolderTab,
    /// Which folder kv section has a pending "add row" (reconciled in render).
    pub pending_folder_kv_add: Option<FolderKvSection>,
    /// Current page (0-based) of the interface list.
    pub iface_page: usize,
    /// User-customized columns shown in the interface list.
    pub iface_columns: Vec<IfaceColumn>,
    /// Whether the column-picker popover is open.
    pub iface_columns_popover_open: bool,
    _subs: Vec<gpui::Subscription>,
    /// Subscriptions to each KV row's value `InputState` Focus/Blur events,
    /// so focusing a value input re-renders the panel and the row can expand
    /// to reveal overflowing content (see `reconcile_kv_focus_subs`).
    kv_focus_subs: Vec<gpui::Subscription>,
    /// Entity ids currently subscribed in `kv_focus_subs`; used to skip
    /// rebuilding when the set of rows is unchanged.
    kv_focus_ids: HashSet<gpui::EntityId>,
    /// Subscriptions to params_rows key/value inputs' Change events so edits
    /// re-render and the URL bar recomposes (only while the Query tab is
    /// shown).
    kv_query_sync_subs: Vec<gpui::Subscription>,
    /// Entity ids currently subscribed in `kv_query_sync_subs`.
    kv_query_sync_ids: HashSet<gpui::EntityId>,
    /// Query suffix (`"?a=1&b=2"`) currently injected into the URL input as a
    /// live preview of the params table; `None` when the input shows the raw
    /// path. Tracked so the preview can be stripped again before the value is
    /// persisted to the model or rendered into a curl command.
    url_query_preview: Option<String>,
    /// Whether the current Query-tab activation already folded a query string
    /// found in the URL into the params rows (runs once per activation).
    query_tab_ingested: bool,
    /// Curl-tab preview cache: (input signature, rendered command). Dynamic
    /// variables (`{{$random}}`, `{{$uuid}}` …) regenerate on every
    /// substitution, so without a cache the preview churns on each re-render.
    /// Regenerated only when the request inputs actually change.
    curl_preview_cache: Option<(String, String)>,
    focus_handle: FocusHandle,
    /// Focus handle bound to the tab-overflow popover content. The popover
    /// renders in a deferred overlay and steals focus on open, so `Cmd+W`
    /// (`CloseFile`) dispatches inside the overlay and never reaches the app
    /// root's `on_close_file`. By making this handle the popover's focus
    /// target (via `Popover::track_focus`) and tracking it on the dropdown
    /// list, an `on_action(CloseFile)` on that list reliably fires and closes
    /// the active tab while the dropdown is open.
    tab_overflow_focus: FocusHandle,
}

/// Which folder kv table a row operation targets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FolderKvSection {
    Params,
    Headers,
    Variables,
}

/// Active tab within the folder detail view.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FolderTab {
    /// 目录设置 — name + description + variables.
    Settings,
    /// 目录参数 — folder-level query params + headers.
    Params,
    /// 接口列表 — all requests inside the folder.
    InterfaceList,
}

impl RequestPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://..."));
        let req_base_url = cx.new(|cx| InputState::new(window, cx).placeholder("前置URL"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("接口名称"));
        let method_options: Vec<String> =
            RequestMethod::all().iter().map(|m| m.to_string()).collect();
        let method_select = cx.new(|cx| {
            SelectState::new(
                method_options,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        let body_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(RawLanguage::Json.highlight())
                .placeholder("Request body (JSON, XML, ...)")
                .default_value("{}")
        });
        let lang_options: Vec<String> = RawLanguage::all()
            .iter()
            .map(|l| l.lower_name().to_string())
            .collect();
        let body_lang_select = cx.new(|cx| {
            SelectState::new(
                lang_options,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        // Body type selector: none / form-data / urlencoded / raw.
        let body_type_options: Vec<String> = vec![
            "none".to_string(),
            "form-data".to_string(),
            "x-www-form-urlencoded".to_string(),
            "raw".to_string(),
        ];
        let body_type_select = cx.new(|cx| {
            SelectState::new(
                body_type_options,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        // React to body-type changes.
        let bt_sub = cx.subscribe(&body_type_select, Self::on_body_type_change);
        let pre_script_editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("// 预执行脚本：apt.setVariable(k,v) · apt.getVariable(k)")
        });
        let tests_editor = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder(
                "// 后执行脚本：response.{status,body,json,headers,time} · apt.assert(cond,msg)",
            )
        });
        // Mock fields.
        let mock_status_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("200")
                .default_value("200")
        });
        let mock_delay_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0")
                .default_value("0")
        });
        let mock_body_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(RawLanguage::Json.highlight())
                .placeholder("{}")
                .default_value("{}")
        });
        let method_options: Vec<String> = RequestMethod::all()
            .iter()
            .map(|m| m.to_string())
            .chain(std::iter::once("不限制".to_string()))
            .collect();
        let mock_match_method_select = cx.new(|cx| {
            SelectState::new(
                method_options,
                Some(gpui_component::IndexPath::new(7)), // 默认选"不限制"（第8个，索引7）
                window,
                cx,
            )
        });
        let path_pattern_options: Vec<String> =
            vec!["精确".to_string(), "前缀".to_string(), "正则".to_string()];
        let mock_path_pattern_select = cx.new(|cx| {
            SelectState::new(
                path_pattern_options,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        let mock_match_path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("/api/path"));
        // Auth fields.
        let auth_type_options: Vec<String> = AuthType::all()
            .iter()
            .map(|a| a.as_str().to_string())
            .collect();
        let auth_type_select = cx.new(|cx| {
            SelectState::new(
                auth_type_options,
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        let auth_target_select = cx.new(|cx| {
            SelectState::new(
                vec!["Header".to_string(), "Query".to_string()],
                Some(gpui_component::IndexPath::default()),
                window,
                cx,
            )
        });
        let auth_token = cx.new(|cx| InputState::new(window, cx).placeholder("Token"));
        let auth_username = cx.new(|cx| InputState::new(window, cx).placeholder("用户名"));
        let auth_password = cx.new(|cx| InputState::new(window, cx).placeholder("密码"));
        let auth_key = cx.new(|cx| InputState::new(window, cx).placeholder("Key"));
        let auth_value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));

        // Folder-detail editors.
        let folder_name = cx.new(|cx| InputState::new(window, cx).placeholder("目录名称"));
        let folder_desc = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("目录描述（可选）：说明该目录下接口的用途、归属模块等")
        });
        let panel = Self {
            state: state.clone(),
            request_id: None,
            url,
            req_base_url,
            req_base_mode: None,
            req_baseurl_open: false,
            tab_overflow_open: false,
            name,
            method_select,
            params_rows: Vec::new(),
            headers_rows: Vec::new(),
            path_rows: Vec::new(),
            cookie_rows: Vec::new(),
            body_rows: Vec::new(),
            body_editor,
            body_lang_select,
            body_type_select,
            pre_script_editor,
            tests_editor,
            mock_enabled: false,
            mock_status_input,
            mock_delay_input,
            mock_body_editor,
            mock_headers_rows: Vec::new(),
            mock_match_method_select,
            mock_path_pattern_select,
            mock_match_path_input,
            mock_enable_templates: false,
            mock_match_query_rows: Vec::new(),
            mock_match_header_rows: Vec::new(),
            auth_type_select,
            auth_target_select,
            auth_token,
            auth_username,
            auth_password,
            auth_key,
            auth_value,
            auth_type: AuthType::None,
            auth_target: AuthTarget::Header,
            protocol: Protocol::Http,
            active_tab: ReqTab::Headers,
            body_type: BodyType::None,
            body_visual_mode: false,
            raw_visual_rows: Vec::new(),
            pending_visual_add: false,
            pending_kv_add: false,
            pending_file_pick: None,
            pending_reload: false,
            folder_id: None,
            folder_name,
            folder_desc,
            folder_baseurl_open: false,
            folder_param_rows: Vec::new(),
            folder_header_rows: Vec::new(),
            folder_var_rows: Vec::new(),
            folder_tab: FolderTab::Settings,
            pending_folder_kv_add: None,
            iface_page: 0,
            iface_columns: crate::state::persistence::load_iface_columns(),
            iface_columns_popover_open: false,
            _subs: Vec::new(),
            kv_focus_subs: Vec::new(),
            kv_focus_ids: HashSet::new(),
            kv_query_sync_subs: Vec::new(),
            kv_query_sync_ids: HashSet::new(),
            url_query_preview: None,
            query_tab_ingested: false,
            curl_preview_cache: None,
            focus_handle: cx.focus_handle(),
            tab_overflow_focus: cx.focus_handle(),
        };
        // Subscribe to selection changes to load the active request.
        let sub = cx.subscribe(&state, Self::on_state_event);
        // Auth type/target change subscriptions (panel is built now).
        let auth_type_sub = cx.subscribe(&panel.auth_type_select, Self::on_auth_change);
        let auth_target_sub = cx.subscribe(&panel.auth_target_select, Self::on_auth_change);
        let mut subs = vec![sub, bt_sub, auth_type_sub, auth_target_sub];
        // Subscribe to each input's blur to commit edits back to the model.
        subs.push(cx.subscribe(&panel.url.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.name.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.body_editor.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.pre_script_editor.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.tests_editor.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.mock_status_input.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.mock_delay_input.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.mock_body_editor.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.mock_match_path_input.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.auth_token.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.auth_username.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.auth_password.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.auth_key.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.auth_value.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.folder_name.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        subs.push(cx.subscribe(&panel.folder_desc.clone(), |this, _, ev: &InputEvent, cx| this.on_input_blur(ev, cx)));
        let mut panel = panel;
        panel._subs = subs;
        panel.load_active_request(window, cx);
        panel
    }

    /// React to the body-type selector changing the active body type.
    pub(super) fn on_body_type_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(val) = src.read(cx).selected_value() {
            self.body_type = match val.as_str() {
                "form-data" => BodyType::FormData,
                "x-www-form-urlencoded" => BodyType::Urlencoded,
                "raw" => BodyType::Raw,
                _ => BodyType::None,
            };
            self.commit_to_model(cx);
            cx.notify();
        }
    }

    /// React to the auth type/target selectors changing.
    pub(super) fn on_auth_change(
        &mut self,
        _src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(val) = self.auth_type_select.read(cx).selected_value() {
            if let Some(t) = AuthType::parse(val) {
                self.auth_type = t;
            }
        }
        if let Some(val) = self.auth_target_select.read(cx).selected_value() {
            self.auth_target = match val.as_str() {
                "Query" => AuthTarget::Query,
                _ => AuthTarget::Header,
            };
        }
        self.commit_to_model(cx);
        cx.notify();
    }

    /// Abort the active streaming/SSE/WebSocket request for this request.
    pub fn stop_active(&mut self, cx: &mut Context<Self>) {
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        // Signal the stop flag, then clear the streaming marker.
        self.state.update(cx, |s, _cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    if let Some(resp) = r.last_response.as_mut() {
                        resp.streaming = false;
                    }
                }
            }
            s.sending = None;
        });
        // 必须在可变借用释放后再emit，避免双重借用panic。
        // 使用 spawn 异步任务，在当前同步事件处理完成后执行，借用已释放。
        let state = self.state.clone();
        let emit_id = id.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::ResponseUpdated(emit_id));
            });
        })
        .detach();
        // Flip the shared stop flag if one is set for this request.
        if let Some(flag) = stop_flags().lock().unwrap().get(&id) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        cx.notify();
    }

    /// On input change/blur, commit the editors back into the active model
    /// (request or folder) so edits apply live (name/path/etc. update in the
    /// tree and lists as you type). Saving is debounced inside notify_edited.
    pub(super) fn on_input_blur(&mut self, ev: &InputEvent, cx: &mut Context<Self>) {
        if matches!(ev, InputEvent::Blur | InputEvent::Change) {
            // If a folder is selected, commit the folder editors; otherwise
            // commit the request editors.
            if self.folder_id.is_some() {
                self.commit_folder(cx);
            } else {
                self.commit_to_model(cx);
            }
        }
    }

    /// All KV row `value` InputState entities across every KV table the panel
    /// owns. Used to (re)subscribe for focus-driven value expansion.
    fn collect_kv_value_entities(&self) -> Vec<Entity<EditorState>> {
        let mut v = Vec::new();
        for rows in [
            &self.params_rows,
            &self.headers_rows,
            &self.path_rows,
            &self.cookie_rows,
            &self.body_rows,
            &self.raw_visual_rows,
            &self.mock_headers_rows,
            &self.mock_match_query_rows,
            &self.mock_match_header_rows,
            &self.folder_param_rows,
            &self.folder_header_rows,
            &self.folder_var_rows,
        ] {
            for r in rows {
                v.push(r.value.clone());
            }
        }
        v
    }

    /// Subscribe to each KV value input's Focus/Blur so the panel re-renders
    /// and `KvTable` can expand the focused, overflowing value. Rebuilds only
    /// when the set of value entities changed (rows added/removed/request
    /// switched), which avoids per-frame churn. Called from `render`.
    fn reconcile_kv_focus_subs(&mut self, cx: &mut Context<Self>) {
        let ents = self.collect_kv_value_entities();
        let new_ids: HashSet<gpui::EntityId> = ents.iter().map(|e| e.entity_id()).collect();
        if new_ids == self.kv_focus_ids {
            return;
        }
        let subs = ents
            .into_iter()
            .map(|e| cx.subscribe(&e, |this, _, ev: &InputEvent, cx| this.on_kv_value_focus(ev, cx)))
            .collect();
        self.kv_focus_subs = subs;
        self.kv_focus_ids = new_ids;
    }

    /// Focus/Blur on a KV value input: just trigger a re-render so each row's
    /// height is recomputed from its current focus state.
    fn on_kv_value_focus(&mut self, _ev: &InputEvent, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// Keep the URL bar composed with the Query tab's live params. While the
    /// Query tab is active the URL input shows `path?query` (a display-only
    /// preview — the model always stores the query-free path); on any other
    /// tab the preview is stripped back off. Also (re)subscribes to the
    /// params rows' key/value inputs so edits re-render. Called from `render`,
    /// which owns the `&mut Window` needed by `set_value` (never touch the
    /// window from event closures — re-entrant window updates panic).
    fn reconcile_kv_query_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab != ReqTab::Query {
            if self.url_query_preview.is_some() {
                let base = self.url_base_value(cx);
                self.url_query_preview = None;
                if self.url.read(cx).value().as_ref() != base.as_str() {
                    self.url.update(cx, |s, cx| s.set_value(base, window, cx));
                }
            }
            self.query_tab_ingested = false;
            self.kv_query_sync_subs.clear();
            self.kv_query_sync_ids.clear();
            return;
        }
        if !self.query_tab_ingested {
            self.query_tab_ingested = true;
            self.ingest_url_query_into_params(window, cx);
        }
        // Keys are single-line InputState, values are EditorState — subscribe
        // each kind separately; both just re-render.
        let new_ids: HashSet<gpui::EntityId> = self
            .params_rows
            .iter()
            .flat_map(|r| [r.key.entity_id(), r.value.entity_id()])
            .collect();
        if new_ids != self.kv_query_sync_ids {
            let mut subs = Vec::with_capacity(self.params_rows.len() * 2);
            for r in &self.params_rows {
                subs.push(cx.subscribe(
                    &r.key,
                    |this, _, ev: &InputEvent, cx| this.on_kv_query_change(ev, cx),
                ));
                subs.push(cx.subscribe(
                    &r.value,
                    |this, _, ev: &InputEvent, cx| this.on_kv_query_change(ev, cx),
                ));
            }
            self.kv_query_sync_subs = subs;
            self.kv_query_sync_ids = new_ids;
        }
        // Idempotent: skips `set_value` when the input already shows exactly
        // the composed URL, so typing in the URL bar never loses the cursor.
        self.update_url_with_query(window, cx);
    }

    /// Any edit in the params table just re-renders; `reconcile_kv_query_sync`
    /// recomposes the URL bar from the fresh rows on the next frame.
    fn on_kv_query_change(&mut self, _ev: &InputEvent, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// The URL input's query-free value: the displayed text minus the tracked
    /// preview suffix. While the preview is live the Query tab owns the query
    /// string, so an edit past the preview cuts at the first `?`.
    fn url_base_value(&self, cx: &App) -> String {
        let value = self.url.read(cx).value().to_string();
        match &self.url_query_preview {
            Some(suffix) => value
                .strip_suffix(suffix.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| value.split('?').next().unwrap_or(&value).to_string()),
            None => value,
        }
    }

    /// On entering the Query tab, fold a query string already present in the
    /// URL (`path?a=1`) into the params table so nothing the user stored is
    /// dropped and the table becomes the single source of the query. Existing
    /// keys are kept; the URL input is reset to the query-free path.
    fn ingest_url_query_into_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.url.read(cx).value().to_string();
        let Some((path, query)) = value.split_once('?') else {
            return;
        };
        self.url
            .update(cx, |s, cx| s.set_value(path, window, cx));
        if query.is_empty() {
            return;
        }
        let existing: HashSet<String> = self
            .params_rows
            .iter()
            .map(|r| r.key.read(cx).value().trim().to_string())
            .collect();
        let mut added: Vec<KeyValue> = Vec::new();
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            let (k, v) = (k.trim().to_string(), v.to_string());
            if k.is_empty() || existing.contains(&k) {
                continue;
            }
            added.push(KeyValue {
                enabled: true,
                key: k,
                value: v,
                ..KeyValue::default()
            });
        }
        if added.is_empty() {
            return;
        }
        let mut pairs = kv_table::pairs_from_rows(&self.params_rows, cx);
        pairs.extend(added);
        self.params_rows = kv_table::rows_from_pairs(&pairs, self.state.clone(), window, cx);
        cx.notify();
    }

    /// Compose the URL bar from the current params rows: `base?k=v&…`,
    /// percent-encoded exactly like the send path (`url::form_urlencoded`).
    /// Enabled rows with a non-empty key participate, mirroring
    /// `http::prepare`; with none enabled the plain path is restored.
    fn update_url_with_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base = self.url_base_value(cx);
        let pairs: Vec<(String, String)> = self
            .params_rows
            .iter()
            .filter(|r| r.enabled)
            .map(|r| {
                (
                    r.key.read(cx).value().trim().to_string(),
                    r.value.read(cx).value().to_string(),
                )
            })
            .filter(|(k, _)| !k.is_empty())
            .collect();
        let desired = if pairs.is_empty() {
            self.url_query_preview = None;
            base
        } else {
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            self.url_query_preview = Some(format!("?{encoded}"));
            format!("{base}?{encoded}")
        };
        if self.url.read(cx).value().as_ref() != desired.as_str() {
            self.url.update(cx, |s, cx| s.set_value(desired, window, cx));
        }
    }

    /// Handle `Cmd+W` (`CloseFile`) while the tab-overflow popover is open.
    ///
    /// The popover renders inside a deferred overlay and takes focus when it
    /// opens, so the action dispatches inside that overlay and never reaches
    /// the app root's `on_close_file`. The dropdown list is registered as the
    /// popover's focus target (see `tab_overflow_focus`), so this handler —
    /// placed on that list — reliably fires: close the active request tab and
    /// dismiss the dropdown. When the dropdown is closed, focus is back in the
    /// main tree and the root handler closes the tab as usual.
    fn on_overflow_close_tab(
        &mut self,
        _: &crate::ui::app::CloseFile,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.state.read(cx).active_tab_id.clone();
        if let Some(id) = active {
            self.state.update(cx, |s, cx| {
                s.close_tab(&id, cx);
            });
        }
        self.tab_overflow_open = false;
        cx.notify();
    }

    pub(super) fn on_state_event(
        &mut self,
        _src: Entity<AppState>,
        ev: &AppEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            ev,
            AppEvent::SelectionChanged | AppEvent::EnvironmentChanged
        ) {
            // Reload the active request/folder so the URL base_url display
            // and all variable-dependent fields pick up the new env values.
            self.pending_reload = true;
            // Close the overflow dropdown so it doesn't linger on stale state.
            self.tab_overflow_open = false;
            cx.notify();
        }
    }

    /// Load whichever node is selected (request OR folder) into the editors.
    /// Called from the top of render() where a Window is available.
    pub fn load_active_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If a folder is selected, load it and bail out (the request editor
        // isn't shown in that case). Clear any prior request selection so the
        // render branch switches to the folder view.
        if self.load_active_folder(window, cx) {
            self.request_id = None;
            return;
        }
        // Otherwise clear folder state and load the request (or none).
        self.folder_id = None;
        let id = self.state.read(cx).selected_request.clone();
        // Find the request in state.
        let req = self.state.read(cx).active_project().and_then(|p| {
            id.as_ref()
                .and_then(|id| p.find_request(id).map(|(_, r)| r.clone()))
        });

        self.request_id = id.clone();
        match req {
            Some(r) => {
                // Split URL into base + path for display in two separate inputs.
                // Resolution at send time is driven by `req_base_mode` (mirrors
                // `ApiRequest::base_url_override`), NOT by whether the input is
                // empty — so the three states are distinguishable:
                //   inherit (None) / explicit-disable (Some(None)) / override (Some(Some)).
                let (base_display, path_display) = {
                    // An explicit per-request override always takes precedence
                    // over URL-based splitting, so a user-selected prefix URL is
                    // never silently discarded on reload (even when the stored URL
                    // is absolute). Only in the inherit case (None) do we split an
                    // absolute URL into host→base for display.
                    match &r.base_url_override {
                        // Explicitly disable any prefix for this request.
                        Some(None) => {
                            self.req_base_mode = Some(None);
                            (String::new(), r.url.clone())
                        }
                        // Override with a specific value (literal or {{var}}).
                        Some(Some(u)) => {
                            self.req_base_mode = Some(Some(u.clone()));
                            if u.trim().is_empty() {
                                (String::new(), r.url.clone())
                            } else {
                                (u.clone(), r.url.clone())
                            }
                        }
                        // Inherit: the mode stays None. For display, split an
                        // absolute URL into host→base input, or resolve the
                        // folder chain's base_url for a relative path.
                        None => {
                            self.req_base_mode = None;
                            if r.url.starts_with("http://") || r.url.starts_with("https://") {
                                // Split at the first '/' after the host.
                                let scheme_end = if r.url.starts_with("https://") { 8 } else { 7 };
                                if let Some(slash) = r.url[scheme_end..].find('/') {
                                    let base = r.url[..scheme_end + slash].to_string();
                                    let path = r.url[scheme_end + slash..].to_string();
                                    (base, path)
                                } else {
                                    (r.url.clone(), String::new())
                                }
                            } else {
                                let base = {
                                    let st = self.state.read(cx);
                                    st.active_project().and_then(|p| {
                                        p.find_request(&r.id).and_then(|(chain, _)| {
                                            resolve_folder_base_url(p, &chain)
                                        })
                                    })
                                };
                                match base {
                                    Some(b) => (b, r.url.clone()),
                                    None => (String::new(), r.url.clone()),
                                }
                            }
                        }
                    }
                };
                self.req_base_url
                    .update(cx, |s, cx| s.set_value(base_display, window, cx));
                // A reload replaces the URL from the model, so any live query
                // preview is stale; the next reconcile recomposes if needed.
                self.url_query_preview = None;
                self.query_tab_ingested = false;
                self.url
                    .update(cx, |s, cx| s.set_value(path_display, window, cx));
                self.name
                    .update(cx, |s, cx| s.set_value(r.name.clone(), window, cx));
                // Protocol drives whether the method selector is shown.
                self.protocol = r.protocol;
                let method_str = r.method.to_string();
                self.method_select
                    .update(cx, |s, cx| s.set_selected_value(&method_str, window, cx));
                // Query/Headers/Path/Cookie rows.
                self.params_rows = kv_table::rows_from_pairs(&r.params, self.state.clone(), window, cx);
                self.headers_rows = kv_table::rows_from_pairs(&r.headers, self.state.clone(), window, cx);
                self.path_rows = kv_table::rows_from_pairs(&r.path, self.state.clone(), window, cx);
                self.cookie_rows = kv_table::rows_from_pairs(&r.cookies, self.state.clone(), window, cx);
                // Body.
                self.body_type = r.body.body_type;
                self.body_editor.update(cx, |s, cx| {
                    s.set_value(r.body.raw.clone(), window, cx);
                });
                self.body_rows = kv_table::rows_from_pairs(
                    match self.body_type {
                        BodyType::FormData => &r.body.form_data,
                        _ => &r.body.urlencoded,
                    },
                    self.state.clone(),
                    window,
                    cx,
                );
                // Visual-mode rows for the Raw body (reused across renders so
                // typing in the field inputs doesn't lose focus).
                self.raw_visual_rows = kv_table::rows_from_pairs(&r.body.raw_parameter, self.state.clone(), window, cx);
                // Body type select.
                let bt_str = match r.body.body_type {
                    BodyType::None => "none",
                    BodyType::FormData => "form-data",
                    BodyType::Urlencoded => "x-www-form-urlencoded",
                    BodyType::Raw => "raw",
                }
                .to_string();
                self.body_type_select
                    .update(cx, |s, cx| s.set_selected_value(&bt_str, window, cx));
                // Body language select.
                let lang_str = r.body.raw_language.lower_name().to_string();
                self.body_lang_select
                    .update(cx, |s, cx| s.set_selected_value(&lang_str, window, cx));
                self.pre_script_editor
                    .update(cx, |s, cx| s.set_value(r.pre_script.clone(), window, cx));
                self.tests_editor
                    .update(cx, |s, cx| s.set_value(r.tests_script.clone(), window, cx));
                // Auth.
                self.auth_type = r.auth.auth_type;
                self.auth_target = r.auth.add_to;
                let auth_type_str = r.auth.auth_type.as_str().to_string();
                self.auth_type_select
                    .update(cx, |s, cx| s.set_selected_value(&auth_type_str, window, cx));
                let auth_target_str = match r.auth.add_to {
                    AuthTarget::Header => "Header".to_string(),
                    AuthTarget::Query => "Query".to_string(),
                };
                self.auth_target_select.update(cx, |s, cx| {
                    s.set_selected_value(&auth_target_str, window, cx)
                });
                self.auth_token
                    .update(cx, |s, cx| s.set_value(r.auth.token.clone(), window, cx));
                self.auth_username
                    .update(cx, |s, cx| s.set_value(r.auth.username.clone(), window, cx));
                self.auth_password
                    .update(cx, |s, cx| s.set_value(r.auth.password.clone(), window, cx));
                self.auth_key
                    .update(cx, |s, cx| s.set_value(r.auth.key.clone(), window, cx));
                self.auth_value
                    .update(cx, |s, cx| s.set_value(r.auth.value.clone(), window, cx));
                // Load mock rule data.
                if let Some(mock) = &r.mock {
                    self.mock_enabled = mock.enabled;
                    self.mock_status_input
                        .update(cx, |s, cx| s.set_value(mock.status.to_string(), window, cx));
                    self.mock_delay_input.update(cx, |s, cx| {
                        s.set_value(mock.delay_ms.to_string(), window, cx)
                    });
                    self.mock_body_editor
                        .update(cx, |s, cx| s.set_value(mock.body.clone(), window, cx));
                    self.mock_headers_rows = kv_table::rows_from_pairs(&mock.headers, self.state.clone(), window, cx);
                    // Match method: "不限制" is index 7, others match their position in RequestMethod::all().
                    let method_idx = match &mock.match_method {
                        Some(m) => RequestMethod::all()
                            .iter()
                            .position(|x| x == m)
                            .unwrap_or(7),
                        None => 7,
                    };
                    self.mock_match_method_select.update(cx, |s, cx| {
                        s.set_selected_value(
                            &RequestMethod::all()
                                .get(method_idx)
                                .map(|m| m.to_string())
                                .unwrap_or_else(|| "不限制".to_string()),
                            window,
                            cx,
                        )
                    });
                    // Path pattern: 0=Exact, 1=Prefix, 2=Regex.
                    let (pattern_idx, pattern_str) = match &mock.match_path {
                        crate::state::models::PathPattern::Exact(s) => (0, s.clone()),
                        crate::state::models::PathPattern::Prefix(s) => (1, s.clone()),
                        crate::state::models::PathPattern::Regex(s) => (2, s.clone()),
                    };
                    let pattern_str_val = match pattern_idx {
                        0 => "精确",
                        1 => "前缀",
                        _ => "正则",
                    }
                    .to_string();
                    self.mock_path_pattern_select.update(cx, |s, cx| {
                        s.set_selected_value(&pattern_str_val, window, cx)
                    });
                    self.mock_match_path_input
                        .update(cx, |s, cx| s.set_value(pattern_str, window, cx));
                    self.mock_enable_templates = mock.enable_templates;
                    self.mock_match_query_rows =
                        kv_table::rows_from_pairs(&mock.match_query, self.state.clone(), window, cx);
                    self.mock_match_header_rows =
                        kv_table::rows_from_pairs(&mock.match_headers, self.state.clone(), window, cx);
                } else {
                    self.mock_enabled = false;
                    self.mock_status_input
                        .update(cx, |s, cx| s.set_value("200".to_string(), window, cx));
                    self.mock_delay_input
                        .update(cx, |s, cx| s.set_value("0".to_string(), window, cx));
                    self.mock_body_editor
                        .update(cx, |s, cx| s.set_value("{}".to_string(), window, cx));
                    self.mock_headers_rows = kv_table::rows_from_pairs(
                        &[crate::state::models::KeyValue::new(
                            "Content-Type",
                            "application/json",
                        )],
                        self.state.clone(),
                        window,
                        cx,
                    );
                    self.mock_match_method_select.update(cx, |s, cx| {
                        s.set_selected_value(&"不限制".to_string(), window, cx)
                    });
                    self.mock_path_pattern_select.update(cx, |s, cx| {
                        s.set_selected_value(&"精确".to_string(), window, cx)
                    });
                    // Auto-extract path from request URL if available.
                    let auto_path = crate::mock::path_of(&r.url).unwrap_or_else(|| "/".into());
                    self.mock_match_path_input
                        .update(cx, |s, cx| s.set_value(auto_path, window, cx));
                    self.mock_enable_templates = false;
                    self.mock_match_query_rows = Vec::new();
                    self.mock_match_header_rows = Vec::new();
                }
            }
            None => {
                self.url_query_preview = None;
                self.query_tab_ingested = false;
                self.url
                    .update(cx, |s, cx| s.set_value(String::new(), window, cx));
                self.name
                    .update(cx, |s, cx| s.set_value(String::new(), window, cx));
                self.params_rows = Vec::new();
                self.headers_rows = Vec::new();
                self.path_rows = Vec::new();
                self.cookie_rows = Vec::new();
                self.body_rows = Vec::new();
                self.mock_enabled = false;
                self.mock_headers_rows = Vec::new();
                self.mock_match_query_rows = Vec::new();
                self.mock_match_header_rows = Vec::new();
            }
        }
        cx.notify();
    }

    /// If a folder is currently selected, load its name/description/params/
    /// headers/variables into the folder editors and return true. Returns
    /// false when no folder is selected (so the caller can load a request).
    pub(super) fn load_active_folder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = self.state.read(cx).selected_folder.clone();
        if id.is_none() {
            return false;
        }
        let folder = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| p.find_folder(id.as_deref()?).map(|(_, f)| f.clone()));
        match folder {
            Some(f) => {
                self.folder_id = id.clone();
                self.iface_page = 0;
                self.folder_name
                    .update(cx, |s, cx| s.set_value(f.name.clone(), window, cx));
                self.folder_desc
                    .update(cx, |s, cx| s.set_value(f.description.clone(), window, cx));
                self.folder_param_rows = kv_table::rows_from_pairs(&f.params, self.state.clone(), window, cx);
                self.folder_header_rows = kv_table::rows_from_pairs(&f.headers, self.state.clone(), window, cx);
                self.folder_var_rows = kv_table::rows_from_pairs(&f.variables, self.state.clone(), window, cx);
                cx.notify();
                true
            }
            None => {
                // Stale selection (folder was deleted): clear it.
                self.folder_id = None;
                false
            }
        }
    }

    /// Commit the folder editors back into the folder model.
    pub(super) fn commit_folder(&mut self, cx: &mut Context<Self>) {
        let id = match self.folder_id.clone() {
            Some(id) => id,
            None => return,
        };
        let name = self.folder_name.read(cx).value().to_string();
        let description = self.folder_desc.read(cx).text().to_string();
        let params = kv_table::pairs_from_rows(&self.folder_param_rows, cx);
        let headers = kv_table::pairs_from_rows(&self.folder_header_rows, cx);
        let variables = kv_table::pairs_from_rows(&self.folder_var_rows, cx);
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, folder)) = project.find_folder_mut(&id) {
                    folder.name = name;
                    folder.description = description;
                    // base_url 由前置URL下拉动作（set_folder_base_url）维护，
                    // 此处不覆盖，避免清空已设置的 {{var}} 引用。
                    folder.params = params;
                    folder.headers = headers;
                    folder.variables = variables;
                }
            }
            s.dirty = true;
            s.schedule_save(cx);
        });
        // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免订阅者
        // （如 project_tree_panel）尝试 read state 时造成双重借用 panic。
        // 使用 spawn 异步任务，它会在当前同步事件处理完成后才执行，此时借用已释放。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::RequestEdited);
            });
        })
        .detach();
    }

    /// Read the current editor state back into the request model.
    pub(super) fn commit_to_model(&mut self, cx: &mut Context<Self>) {
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        let url = self.url_base_value(cx);
        let name = self.name.read(cx).value().to_string();
        let protocol = self.protocol;
        let method = self
            .method_select
            .read(cx)
            .selected_value()
            .cloned()
            .and_then(|s| RequestMethod::parse(&s))
            .unwrap_or(RequestMethod::Get);
        let params = kv_table::pairs_from_rows(&self.params_rows, cx);
        let headers = kv_table::pairs_from_rows(&self.headers_rows, cx);
        let path = kv_table::pairs_from_rows(&self.path_rows, cx);
        let cookies = kv_table::pairs_from_rows(&self.cookie_rows, cx);
        let raw_body = self.body_editor.read(cx).text().to_string();
        let body_lang = self
            .body_lang_select
            .read(cx)
            .selected_value()
            .cloned()
            .and_then(|s| RawLanguage::parse_name(&s))
            .unwrap_or_default();
        let pre_script = self.pre_script_editor.read(cx).text().to_string();
        let tests_script = self.tests_editor.read(cx).text().to_string();
        // Auth snapshot.
        let auth = AuthConfig {
            auth_type: self.auth_type,
            token: self.auth_token.read(cx).value().to_string(),
            username: self.auth_username.read(cx).value().to_string(),
            password: self.auth_password.read(cx).value().to_string(),
            key: self.auth_key.read(cx).value().to_string(),
            value: self.auth_value.read(cx).value().to_string(),
            add_to: self.auth_target,
        };
        // Snapshot the per-request base-URL override mode. In override mode the
        // persisted value follows the live editor input so the user can type a
        // literal URL (not just pick a {{var}} placeholder).
        let base_url_override = {
            let live = self.req_base_url.read(cx).value().to_string();
            match &self.req_base_mode {
                None => None,
                Some(None) => Some(None),
                Some(Some(_)) => Some(Some(live)),
            }
        };

        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, req)) = project.find_request_mut(&id) {
                    req.url = url;
                    req.name = name;
                    req.protocol = protocol;
                    req.method = method;
                    req.params = params;
                    req.headers = headers;
                    req.path = path;
                    req.cookies = cookies;
                    req.base_url_override = base_url_override;
                    req.body.body_type = self.body_type;
                    req.body.raw = raw_body;
                    req.body.raw_language = body_lang;
                    // Sync visual-mode fields when in Raw body mode.
                    if self.body_type == BodyType::Raw {
                        req.body.raw_parameter =
                            kv_table::pairs_from_rows(&self.raw_visual_rows, cx);
                    }
                    match self.body_type {
                        BodyType::FormData => {
                            req.body.form_data = kv_table::pairs_from_rows(&self.body_rows, cx)
                        }
                        BodyType::Urlencoded => {
                            req.body.urlencoded = kv_table::pairs_from_rows(&self.body_rows, cx)
                        }
                        _ => {}
                    }
                    req.auth = auth;
                    req.pre_script = pre_script;
                    req.tests_script = tests_script;
                    // Save mock rule.
                    let mock_status = self
                        .mock_status_input
                        .read(cx)
                        .value()
                        .parse::<u16>()
                        .unwrap_or(200);
                    let mock_delay = self
                        .mock_delay_input
                        .read(cx)
                        .value()
                        .parse::<u64>()
                        .unwrap_or(0);
                    let mock_body = self.mock_body_editor.read(cx).text().to_string();
                    let mock_headers = kv_table::pairs_from_rows(&self.mock_headers_rows, cx);
                    let selected_method_str = self
                        .mock_match_method_select
                        .read(cx)
                        .selected_value()
                        .cloned()
                        .unwrap_or_else(|| "不限制".to_string());
                    let match_method = if selected_method_str == "不限制" {
                        None
                    } else {
                        RequestMethod::parse(&selected_method_str)
                    };
                    let selected_pattern_str = self
                        .mock_path_pattern_select
                        .read(cx)
                        .selected_value()
                        .cloned()
                        .unwrap_or_else(|| "精确".to_string());
                    let match_path_str = self.mock_match_path_input.read(cx).value().to_string();
                    let match_path = match selected_pattern_str.as_str() {
                        "前缀" => crate::state::models::PathPattern::Prefix(match_path_str),
                        "正则" => crate::state::models::PathPattern::Regex(match_path_str),
                        _ => crate::state::models::PathPattern::Exact(match_path_str),
                    };
                    let match_query = kv_table::pairs_from_rows(&self.mock_match_query_rows, cx);
                    let match_headers = kv_table::pairs_from_rows(&self.mock_match_header_rows, cx);
                    let mock_rule = crate::state::models::MockRule {
                        enabled: self.mock_enabled,
                        status: mock_status,
                        headers: mock_headers,
                        body: mock_body,
                        delay_ms: mock_delay,
                        match_method,
                        match_path,
                        match_query,
                        match_headers,
                        enable_templates: self.mock_enable_templates,
                    };
                    req.mock = Some(mock_rule);
                }
            }
            s.dirty = true;
            s.schedule_save(cx);
        });
        // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免订阅者
        // （如 project_tree_panel）尝试 read state 时造成双重借用 panic。
        // 使用 spawn 异步任务，它会在当前同步事件处理完成后才执行，此时借用已释放。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::RequestEdited);
            });
        })
        .detach();
    }

    /// Build the effective variable map for the active request.
    pub(super) fn effective_vars(&self, cx: &App) -> BTreeMap<String, String> {
        let id = match &self.request_id {
            Some(id) => id.clone(),
            None => return BTreeMap::new(),
        };
        let st = self.state.read(cx);
        let project = match st.active_project() {
            Some(p) => p,
            None => return BTreeMap::new(),
        };
        let (chain, req) = match project.find_request(&id) {
            Some(x) => x,
            None => return BTreeMap::new(),
        };
        let folder_slices = project.folder_variables_chain(&chain);
        let global = project.global_variables.clone();
        // Flatten folder slices into one owned vec for the helper.
        let mut folder_vars: Vec<KeyValue> = Vec::new();
        for slice in folder_slices {
            folder_vars.extend_from_slice(slice);
        }
        let env: Vec<KeyValue> = project.active_env_variables().to_vec();
        log::info!(
            "EFFECTIVE_VARS: env_count={} keys={:?}",
            env.len(),
            env.iter().map(|kv| kv.key.clone()).collect::<Vec<_>>()
        );
        let req_vars = req.variables.clone();
        // System variables: mock_server address (local/remote based on config)
        let mut system = BTreeMap::new();
        system.insert(
            "mock_server".to_string(),
            format!("http://127.0.0.1:{}", crate::share::server::DEFAULT_PORT),
        );
        let mut map = crate::state::models::effective_variables(
            &system,
            &global,
            &env,
            &folder_vars,
            &req_vars,
        );
        log::info!(
            "EFFECTIVE_VARS: final map keys={:?}",
            map.keys().collect::<Vec<_>>()
        );
        // Inject the effective base URL as "__folder_base_url__" (used for
        // relative-path prefixing in the HTTP client) and as "baseUrl" (used
        // for {{baseUrl}} substitution). Resolution is driven by the tri-state
        // `req_base_mode`: inherit / explicit-disable / override. A returned
        // None simply leaves both keys unset, so a relative URL stays relative.
        let req_base_raw = self.req_base_url.read(cx).value().to_string();
        if let Some(base) = crate::ui::request_panel::folder_helpers::resolve_effective_base_url(
            &self.req_base_mode,
            &req_base_raw,
            &map,
            project,
            &chain,
        ) {
            map.insert("__folder_base_url__".to_string(), base.clone());
            map.entry("baseUrl".to_string()).or_insert(base);
        }
        map
    }

    /// Generate a curl command from the current request's editable fields.
    /// Mirrors the real send path (`http::client::prepare`): variables are
    /// substituted, query params are appended to the URL (never moved into a
    /// body), cookies collapse into a Cookie header, and the method flag is
    /// always emitted so a GET with a body stays a GET in curl.
    ///
    /// The result is cached against an input signature: dynamic variables
    /// (`{{$random}}` requestids etc.) regenerate on every substitution, so
    /// without a cache the preview would churn on each re-render. With it,
    /// values stay stable while the request is unchanged; actual sends keep
    /// generating fresh values per click (see the send path / 实际请求 tab).
    pub(super) fn generate_curl(&mut self, cx: &mut Context<Self>) -> String {
        let vars = self.effective_vars(cx);
        let sig = self.curl_input_signature(&vars, cx);
        if let Some((cached_sig, cached)) = &self.curl_preview_cache {
            if *cached_sig == sig {
                return cached.clone();
            }
        }
        let curl = self.render_curl(&vars, cx);
        self.curl_preview_cache = Some((sig, curl.clone()));
        curl
    }

    /// Build a signature covering every input the curl preview depends on:
    /// method/URL, all kv rows, body, auth, base-URL mode, and the effective
    /// variable map (so environment/global/folder edits also count as
    /// changes). Identical signature ⇒ the cached preview is still current.
    fn curl_input_signature(
        &self,
        vars: &BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) -> String {
        let rows_sig = |rows: &Vec<KvRow>, sig: &mut String| {
            for row in rows {
                sig.push_str(if row.enabled { "1" } else { "0" });
                sig.push('\u{1}');
                sig.push_str(&row.key.read(cx).value());
                sig.push('\u{1}');
                sig.push_str(&row.value.read(cx).value());
                sig.push('\u{1}');
            }
        };
        let mut sig = String::new();
        sig.push_str(
            &self
                .method_select
                .read(cx)
                .selected_value()
                .map(|s| s.to_string())
                .unwrap_or_default(),
        );
        sig.push('\u{1}');
        sig.push_str(&self.url_base_value(cx));
        sig.push('\u{1}');
        rows_sig(&self.path_rows, &mut sig);
        rows_sig(&self.params_rows, &mut sig);
        rows_sig(&self.headers_rows, &mut sig);
        rows_sig(&self.cookie_rows, &mut sig);
        // Project-global rows are merged into the rendered command (see
        // render_curl), so they must participate in the cache signature too —
        // otherwise editing a global header would leave a stale preview.
        let kvs_sig = |kvs: &[KeyValue], sig: &mut String| {
            for kv in kvs {
                sig.push_str(if kv.enabled { "1" } else { "0" });
                sig.push('\u{1}');
                sig.push_str(&kv.key);
                sig.push('\u{1}');
                sig.push_str(&kv.value);
                sig.push('\u{1}');
            }
        };
        {
            let st = self.state.read(cx);
            if let Some(project) = st.active_project() {
                kvs_sig(&project.global_params, &mut sig);
                kvs_sig(&project.global_headers, &mut sig);
                kvs_sig(&project.global_cookies, &mut sig);
            }
        }
        sig.push_str(&format!("{:?}", self.body_type));
        sig.push('\u{1}');
        sig.push_str(
            &self
                .body_lang_select
                .read(cx)
                .selected_value()
                .map(|s| s.to_string())
                .unwrap_or_default(),
        );
        sig.push('\u{1}');
        sig.push_str(&self.body_editor.read(cx).text().to_string());
        sig.push('\u{1}');
        for row in &self.body_rows {
            sig.push_str(if row.enabled { "1" } else { "0" });
            sig.push('\u{1}');
            sig.push_str(&row.key.read(cx).value());
            sig.push('\u{1}');
            sig.push_str(&row.value.read(cx).value());
            sig.push('\u{1}');
            sig.push_str(&format!("{:?}", row.field_type));
            sig.push('\u{1}');
            sig.push_str(row.file_path.as_deref().unwrap_or(""));
            sig.push('\u{1}');
        }
        sig.push_str(&format!(
            "{:?}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{:?}\u{1}",
            self.auth_type,
            self.auth_token.read(cx).value(),
            self.auth_username.read(cx).value(),
            self.auth_password.read(cx).value(),
            self.auth_key.read(cx).value(),
            self.auth_value.read(cx).value(),
            self.auth_target,
        ));
        sig.push_str(&format!("{:?}", self.req_base_mode));
        sig.push('\u{1}');
        sig.push_str(&self.req_base_url.read(cx).value());
        sig.push('\u{1}');
        // BTreeMap iterates in key order, so the map's fingerprint is stable.
        for (k, v) in vars {
            sig.push_str(k);
            sig.push('\u{2}');
            sig.push_str(v);
            sig.push('\u{1}');
        }
        sig
    }

    /// Substitute `vars` into the live request fields and render the curl
    /// command (uncached; see [`Self::generate_curl`]).
    fn render_curl(&self, vars: &BTreeMap<String, String>, cx: &mut Context<Self>) -> String {
        use crate::http::curl::{CurlBody, CurlFormPart, CurlSpec};

        let sub = |s: &str| crate::http::variable::substitute(s, vars);
        let method = self
            .method_select
            .read(cx)
            .selected_value()
            .map(|s| s.to_string())
            .and_then(|s| RequestMethod::parse(&s))
            .unwrap_or(RequestMethod::Get);

        // Path variables substitute into the URL at top priority (prepare()).
        let mut url_vars = vars.clone();
        for row in &self.path_rows {
            let k = row.key.read(cx).value().to_string();
            if row.enabled && !k.trim().is_empty() {
                let v = row.value.read(cx).value().to_string();
                url_vars.insert(k.trim().to_string(), v);
            }
        }

        // URL: substitute → join base_url for relative paths → normalize scheme.
        let url_raw = self.url_base_value(cx);
        let mut url = crate::http::variable::substitute(&url_raw, &url_vars);
        if !url.contains("://") {
            if let Some(base) = url_vars.get("__folder_base_url__") {
                if !base.is_empty() {
                    let base = base.trim_end_matches('/');
                    let path = url.trim_start_matches('/');
                    url = format!("{}/{}", base, path);
                }
            }
        }
        url = crate::http::normalize_url(&url, method);

        // Project-global params/headers/cookies merge into the request's own
        // rows BEFORE substitution (mirrors the send path in `send()`), so the
        // generated command carries them too: globals land first, a same-named
        // request row overrides, and dynamic values like `{{$random}}` resolve
        // only after the merge — otherwise the curl preview would silently
        // drop every global row the real request sends.
        let (global_params, global_headers, global_cookies) = {
            let st = self.state.read(cx);
            match st.active_project() {
                Some(project) => (
                    project.global_params.clone(),
                    project.global_headers.clone(),
                    project.global_cookies.clone(),
                ),
                None => (Vec::new(), Vec::new(), Vec::new()),
            }
        };
        let row_kvs = |rows: &Vec<KvRow>| -> Vec<KeyValue> {
            rows.iter()
                .filter(|row| row.enabled)
                .filter_map(|row| {
                    let k = row.key.read(cx).value().to_string();
                    if k.trim().is_empty() {
                        return None;
                    }
                    let v = row.value.read(cx).value().to_string();
                    Some(KeyValue::new(k, v))
                })
                .collect()
        };
        let sub_kvs = |kvs: &[KeyValue]| -> Vec<(String, String)> {
            kvs.iter().map(|kv| (sub(&kv.key), sub(&kv.value))).collect()
        };
        let params = sub_kvs(&merge_kv(&global_params, &row_kvs(&self.params_rows)));
        let headers = sub_kvs(&merge_kv(&global_headers, &row_kvs(&self.headers_rows)));
        let cookies = sub_kvs(&merge_kv(&global_cookies, &row_kvs(&self.cookie_rows)));

        let auth = AuthConfig {
            auth_type: self.auth_type,
            token: sub(&self.auth_token.read(cx).value()),
            username: sub(&self.auth_username.read(cx).value()),
            password: sub(&self.auth_password.read(cx).value()),
            key: sub(&self.auth_key.read(cx).value()),
            value: sub(&self.auth_value.read(cx).value()),
            add_to: self.auth_target,
        };

        let body_lang = self
            .body_lang_select
            .read(cx)
            .selected_value()
            .cloned()
            .and_then(|s| RawLanguage::parse_name(&s))
            .unwrap_or_default();
        let body = match self.body_type {
            BodyType::None => CurlBody::None,
            BodyType::Raw => {
                let raw = self.body_editor.read(cx).text().to_string();
                if raw.trim().is_empty() {
                    CurlBody::None
                } else {
                    CurlBody::Raw {
                        text: sub(&raw),
                        content_type: body_lang.content_type().to_string(),
                    }
                }
            }
            BodyType::Urlencoded => {
                let pairs = sub_kvs(&row_kvs(&self.body_rows));
                if pairs.is_empty() {
                    CurlBody::None
                } else {
                    CurlBody::Urlencoded(pairs)
                }
            }
            BodyType::FormData => {
                let mut items = Vec::new();
                for row in &self.body_rows {
                    if !row.enabled {
                        continue;
                    }
                    let name = row.key.read(cx).value().to_string();
                    if name.trim().is_empty() {
                        continue;
                    }
                    if row.field_type == FieldType::File {
                        let path = row.file_path.clone().unwrap_or_default();
                        if !path.trim().is_empty() {
                            items.push(CurlFormPart::File {
                                name: sub(&name),
                                path: sub(&path),
                            });
                        }
                    } else {
                        let v = row.value.read(cx).value().to_string();
                        items.push(CurlFormPart::Field {
                            name: sub(&name),
                            value: sub(&v),
                        });
                    }
                }
                if items.is_empty() {
                    CurlBody::None
                } else {
                    CurlBody::Form(items)
                }
            }
        };

        crate::http::curl::render(&CurlSpec {
            method,
            url,
            params,
            headers,
            cookies,
            auth,
            body,
        })
    }
}

pub(super) fn truncate_history_body(body: &str) -> (Option<String>, bool) {
    use crate::state::HISTORY_BODY_MAX_CHARS;
    if body.is_empty() {
        return (None, false);
    }
    if body.chars().count() <= HISTORY_BODY_MAX_CHARS {
        return (Some(body.to_string()), false);
    }
    let mut s: String = body.chars().take(HISTORY_BODY_MAX_CHARS).collect();
    s.push_str("…");
    (Some(s), true)
}


impl Render for RequestPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_reload {
            self.pending_reload = false;
            self.load_active_request(window, cx);
        }
        self.reconcile_pending_add(window, cx);
        self.reconcile_folder_kv(window, cx);
        self.reconcile_kv_focus_subs(cx);
        self.reconcile_kv_query_sync(window, cx);
        let theme = cx.theme().clone();
        let has_request = self.request_id.is_some();
        let has_folder = self.folder_id.is_some();
        let sending = self
            .state
            .read(cx)
            .sending
            .as_deref()
            .map(|s| Some(s) == self.request_id.as_deref())
            .unwrap_or(false);

        let active_tab = self.active_tab;

        // Read open tabs for the tab bar.
        let open_tabs: Vec<(String, String, RequestMethod, Protocol)> = {
            let st = self.state.read(cx);
            st.open_request_ids
                .iter()
                .filter_map(|id| {
                    st.active_project()
                        .and_then(|p| p.find_request(id))
                        .map(|(_, r)| (r.id.clone(), r.name.clone(), r.method, r.protocol))
                })
                .collect()
        };
        let active_tab_id = self.state.read(cx).active_tab_id.clone();
        let has_tabs = !open_tabs.is_empty();

        // --- Overflow handling: fold tabs that don't fit into a "»" dropdown ---
        // Estimate how many tabs fit from the window width (rail + sidebar ≈
        // 340px). Conservative per-tab width (150px) so the "»" appears slightly
        // early rather than clipping tabs.
        let avail_w = (window.viewport_size().width.as_f32() - 340.0).max(180.0);
        let per_tab = 150.0_f32;
        let max_visible = ((avail_w / per_tab).floor() as usize)
            .max(1)
            .min(open_tabs.len());
        let overflow = open_tabs.len() > max_visible;
        // Indices of the tabs to render in the visible strip. When overflowing,
        // guarantee the active tab is among them (swap it into the last slot if
        // it would otherwise be hidden) so the active highlight is always on
        // screen and Cmd+W closes the tab the user is looking at.
        let active_idx = open_tabs
            .iter()
            .position(|(id, _, _, _)| active_tab_id.as_deref() == Some(id.as_str()));
        let visible: Vec<usize> = if !overflow {
            (0..open_tabs.len()).collect()
        } else {
            let mut v: Vec<usize> = (0..max_visible).collect();
            if let Some(ai) = active_idx {
                if ai >= max_visible {
                    if let Some(slot) = v.get_mut(max_visible - 1) {
                        *slot = ai;
                    }
                }
            }
            v
        };
        let hidden_count = open_tabs.len().saturating_sub(visible.len());

        // Precompute per-tab metadata that needs `cx` (badge label + color)
        // up front so the builder closures below only use `cx.listener` (an
        // immutable borrow) and don't conflict with each other or the sibling
        // `.when(has_folder,…)` closure that borrows `self` mutably.
        let tabs_meta: Vec<(String, String, gpui::Hsla, &'static str)> = open_tabs
            .iter()
            .map(|(id, name, method, protocol)| {
                let (label, color) = crate::ui::method_colors::badge_for(*protocol, *method, cx);
                (id.clone(), name.clone(), color, label)
            })
            .collect();
        let panel_entity = cx.entity();
        let tab_overflow_open = self.tab_overflow_open;
        let tab_ov_focus = self.tab_overflow_focus.clone();
        let theme_c = theme.clone();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            // Open-tabs bar: always visible when any tabs are open (Apifox-style).
            .when(has_tabs, |this| {
                this.child(
                    h_flex()
                        .id("request-tab-bar")
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(2.))
                        .px(px(6.))
                        .py(px(4.))
                        .h(px(34.))
                        .border_b_1()
                        .border_color(theme.border)
                        .bg(theme.tab_bar)
                        // Visible tab strip: takes the available width, clips any
                        // residual overflow (the "»" dropdown handles the rest).
                        .child(
                            h_flex()
                                .id("req-tab-strip")
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .items_center()
                                .gap(px(2.))
                                .children(
                                    visible
                                        .iter()
                                        .filter_map(|i| tabs_meta.get(*i).map(|t| (*i, t)))
                                        .map(|(i, (id, name, badge_color, badge_label))| {
                                            let is_active =
                                                active_tab_id.as_deref() == Some(id.as_str());
                                            let display_name = if name.chars().count() > 14 {
                                                let truncated: String =
                                                    name.chars().take(14).collect();
                                                format!("{}…", truncated)
                                            } else {
                                                name.clone()
                                            };
                                            let id_focus = id.clone();
                                            let id_close = id.clone();
                                            h_flex()
                                                .id(("req-tab", i))
                                                .flex_shrink_0()
                                                .items_center()
                                                .gap(px(6.))
                                                .px(px(8.))
                                                .py(px(3.))
                                                .rounded(px(6.))
                                                .cursor_pointer()
                                                .when(is_active, |d| {
                                                    d.bg(theme.background)
                                                        .border_1()
                                                        .border_color(theme.border)
                                                })
                                                .when(!is_active, |d| {
                                                    d.text_color(theme.muted_foreground).hover(
                                                        |s| s.bg(theme.accent.opacity(0.15)),
                                                    )
                                                })
                                                // Colored badge (method or protocol).
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(*badge_color)
                                                        .child(*badge_label),
                                                )
                                                // Tab name.
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .when(is_active, |d| {
                                                            d.text_color(theme.foreground)
                                                        })
                                                        .child(display_name),
                                                )
                                                // Close button.
                                                .child(
                                                    Button::new(("req-tab-close", i))
                                                        .ghost()
                                                        .xsmall()
                                                        .label("×")
                                                        .text_size(px(14.))
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                // `cx.listener` already leases
                                                                // RequestPanel — use `this`
                                                                // directly instead of
                                                                // entity.update (double lease
                                                                // panics).
                                                                this.state.update(cx, |s, cx| {
                                                                    s.close_tab(&id_close, cx);
                                                                });
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    // Click on the tab body (not close button) focuses it.
                                                    this.state.update(cx, |s, cx| {
                                                        s.set_active_tab(&id_focus, cx);
                                                    });
                                                }))
                                        }),
                                ),
                        )
                        // Overflow dropdown ("»"): lists every open tab so the
                        // hidden ones stay reachable. Shown only when folding.
                        .when(overflow, |bar| {
                            bar.child(
                                Popover::new("req-tab-overflow")
                                    .anchor(gpui::Anchor::BottomRight)
                                    .open(tab_overflow_open)
                                    .on_open_change(cx.listener(|this, open, _, cx| {
                                        this.tab_overflow_open = *open;
                                        cx.notify();
                                    }))
                                    // Make the popover focus the dropdown list
                                    // handle on open (see `tab_overflow_focus`),
                                    // so the `on_action(CloseFile)` below fires
                                    // even though the popover is a deferred
                                    // overlay outside the app root's dispatch
                                    // path.
                                    .track_focus(&tab_ov_focus)
                                    .trigger(
                                        Button::new("req-tab-ov-trig")
                                            .ghost()
                                            .xsmall()
                                            .label(format!("» {}", hidden_count))
                                            .tooltip("其余标签页"),
                                    )
                                    .p(px(4.))
                                    .child(
                                        v_flex()
                                            .id("req-tab-ov-scroll")
                                            .track_focus(&tab_ov_focus)
                                            .on_action(cx.listener(
                                                Self::on_overflow_close_tab,
                                            ))
                                            .w(px(280.))
                                            .max_h(px(360.))
                                            .overflow_y_scroll()
                                            .gap(px(1.))
                                            .children(
                                                tabs_meta.iter().enumerate().map(
                                                    |(i, (id, name, badge_color, badge_label))| {
                                                        let is_active = active_tab_id.as_deref()
                                                            == Some(id.as_str());
                                                        let display_name =
                                                            if name.chars().count() > 24 {
                                                                let truncated: String =
                                                                    name.chars().take(24).collect();
                                                                format!("{}…", truncated)
                                                            } else {
                                                                name.clone()
                                                            };
                                                        let id_focus = id.clone();
                                                        let id_close = id.clone();
                                                        let panel_entity_close = panel_entity.clone();
                                                        let panel_entity_row = panel_entity.clone();
                                                        let theme_ov = theme_c.clone();
                                                        div()
                                                            .id(("req-tab-ov-row", i))
                                                            .w_full()
                                                            .px(px(8.))
                                                            .py(px(5.))
                                                            .rounded(px(4.))
                                                            .cursor_pointer()
                                                            .flex()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap(px(8.))
                                                            .when(is_active, |d| {
                                                                d.bg(theme_ov.accent.opacity(0.2))
                                                            })
                                                            .when(!is_active, |d| {
                                                                d.hover(|s| s.bg(theme_ov.muted))
                                                            })
                                                            .child(
                                                                div()
                                                                    .text_size(px(10.))
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(*badge_color)
                                                                    .child(*badge_label),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(px(12.))
                                                                    .text_color(if is_active {
                                                                        theme_ov.foreground
                                                                    } else {
                                                                        theme_ov.muted_foreground
                                                                    })
                                                                    .child(display_name),
                                                            )
                                                            .child(
                                                                div()
                                                                    .id(("req-tab-ov-close", i))
                                                                    .ml_auto()
                                                                    .px(px(4.))
                                                                    .text_size(px(14.))
                                                                    .text_color(
                                                                        theme_ov.muted_foreground,
                                                                    )
                                                                    .child("×")
                                                                    .on_mouse_down(
                                                                        MouseButton::Left,
                                                                        move |_, _window,
                                                                              cx: &mut App| {
                                                                            cx.stop_propagation();
                                                                            let id_close_clone =
                                                                                id_close.clone();
                                                                            let _ =
                                                                                panel_entity_close.update(cx, move |this, cx| {
                                                                                    this.state.update(cx, |s, cx| {
                                                                                        s.close_tab(&id_close_clone, cx);
                                                                                    });
                                                                                    this.tab_overflow_open = false;
                                                                                    cx.notify();
                                                                                });
                                                                        },
                                                                    ),
                                                            )
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                move |_, _window, cx: &mut App| {
                                                                    cx.stop_propagation();
                                                                    let id_focus_clone =
                                                                        id_focus.clone();
                                                                    let _ = panel_entity_row.update(cx, move |this, cx| {
                                                                        this.state.update(cx, |s, cx| {
                                                                            s.set_active_tab(
                                                                                &id_focus_clone,
                                                                                cx,
                                                                            );
                                                                        });
                                                                        this.tab_overflow_open = false;
                                                                        cx.notify();
                                                                    });
                                                                },
                                                            )
                                                    },
                                                ),
                                            ),
                                    ),
                            )
                        }),
                )
            })
            // Folder detail view takes over the whole center column.
            .when(has_folder, |this| this.child(self.render_folder_detail(cx)))
            .when(!has_request && !has_folder, |this| {
                this.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .text_color(theme.muted_foreground)
                        .child("Select or create a request to begin."),
                )
            })
            .when(has_request, |this| {
                // The active method/protocol drive the chip + Send button color.
                let method = self
                    .method_select
                    .read(cx)
                    .selected_value()
                    .and_then(|s| RequestMethod::parse(s))
                    .unwrap_or(RequestMethod::Get);
                let method_fill = crate::ui::method_colors::fill_color(method, cx);
                let protocol = self.protocol;
                let streaming = self
                    .state
                    .read(cx)
                    .active_project()
                    .and_then(|p| {
                        p.find_request(self.request_id.as_deref()?)
                            .and_then(|(_, r)| r.last_response.as_ref())
                    })
                    .map(|r| r.streaming)
                    .unwrap_or(false);

                this.child(
                    // 接口名 (request name) bar — fills the panel full width,
                    // flush against the top edge (no outer margins).
                    h_flex()
                        .px_3()
                        .h(px(28.))
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .child(
                            // Protocol badge (read-only, set at creation time).
                            // Width matches the method selector below so the
                            // name input aligns with the URL input.
                            div()
                                .w(px(110.))
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .bg(theme.accent.opacity(0.5))
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(protocol.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.name).small().appearance(false)),
                        )
                        .child(
                            Button::new("locate")
                                .ghost()
                                .xsmall()
                                .icon(Icon::from(IconName::Redo).path(crate::assets::LOCATE))
                                .tooltip("在树中定位")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |_, cx| {
                                        cx.emit(crate::state::AppEvent::LocateActive);
                                    });
                                })),
                        )
                        .child(
                            // Share this single API's documentation (scope =
                            // Request). Emits an AppEvent handled by VerveApp,
                            // which opens the share-config dialog pre-scoped.
                            Button::new("share-request")
                                .ghost()
                                .xsmall()
                                .icon(Icon::from(IconName::Redo).path(crate::assets::SHARE))
                                .tooltip("分享当前接口文档")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(id) = this.request_id.clone() {
                                        this.state.update(cx, |_, cx| {
                                            cx.emit(crate::state::AppEvent::ShareRequest(id));
                                        });
                                    }
                                })),
                        ),
                )
                .child(
                    // Combined URL bar (postman-style): method chip on the left,
                    // URL in the middle, Send/Stop on the right.
                    h_flex()
                        .px_3()
                        .pt(px(4.))
                        .pb_2()
                        .gap_2()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .when(protocol.uses_http_method(), |bar| {
                            // HTTP/GraphQL method selector (hidden for stream/
                            // socket protocols — those don't use HTTP methods).
                            // Rendered as a bold, method-tinted chip so the
                            // verb (GET/POST/…) reads at a glance, Apifox-style.
                            let method_text = crate::ui::method_colors::badge_color(method, cx);
                            bar.child(
                                div().w(px(110.)).child(
                                    Select::new(&self.method_select)
                                        .small()
                                        .appearance(true)
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(method_text),
                                ),
                            )
                        })
                        .child(
                            // Base URL selector: a controlled Popover with
                            // open/close management so selecting an option
                            // closes the dropdown automatically.
                            {
                                let active_env = self
                                    .state
                                    .read(cx)
                                    .active_project()
                                    .and_then(|p| p.active_environment.as_ref())
                                    .and_then(|eid| {
                                        self.state.read(cx).active_project().and_then(|p| {
                                            p.environments.iter().find(|e| &e.id == eid)
                                        })
                                    });
                                let env_urls: Vec<(String, String)> = active_env
                                    .map(|env| {
                                        env.variables
                                            .iter()
                                            .filter(|kv| {
                                                kv.enabled
                                                    && (kv.value.starts_with("http://")
                                                        || kv.value.starts_with("https://"))
                                            })
                                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let theme_c = theme.clone();
                                let panel_entity = cx.entity();
                                let open = self.req_baseurl_open;
                                let base_val_raw = self.req_base_url.read(cx).value().to_string();
                                // The button label reflects the tri-state mode:
                                //   Some(None)              → "不使用前置URL"
                                //   Some(Some(url))         → the resolved url
                                //   None (inherit)          → resolved folder base_url, or "前置URL"
                                let label_text = match &self.req_base_mode {
                                    Some(None) => "不使用前置URL".to_string(),
                                    // Inherit the folder's base_url: resolve the
                                    // folder chain and show the actual URL.
                                    None => {
                                        let req_id = self.request_id.clone();
                                        let resolved = self
                                            .state
                                            .read(cx)
                                            .active_project()
                                            .and_then(|p| {
                                                let id = req_id.as_deref()?;
                                                let (chain, _) = p.find_request(id)?;
                                                resolve_folder_base_url(p, &chain)
                                            });
                                        match resolved {
                                            Some(url) if !url.trim().is_empty() => url,
                                            _ => "前置URL".to_string(),
                                        }
                                    }
                                    // Override with a specific value (literal or
                                    // {{var}} placeholder): substitute so the label
                                    // shows the resolved URL, not raw placeholder text.
                                    Some(Some(_)) => {
                                        let base_val = if base_val_raw.contains("{{") {
                                            let vars = self.effective_vars(cx);
                                            crate::http::variable::substitute(&base_val_raw, &vars)
                                        } else {
                                            base_val_raw
                                        };
                                        if base_val.trim().is_empty() {
                                            "前置URL".to_string()
                                        } else {
                                            base_val
                                        }
                                    }
                                };

                                Popover::new("req-base-pop")
                                    .anchor(gpui::Anchor::BottomLeft)
                                    .open(open)
                                    .on_open_change(cx.listener(|this, open, _, cx| {
                                        this.req_baseurl_open = *open;
                                        cx.notify();
                                    }))
                                    .trigger(
                                        Button::new("req-base-trig")
                                            .outline()
                                            .small()
                                            .w(px(200.))
                                            .icon(IconName::ChevronDown)
                                            .label(label_text)
                                            .tooltip("点击选择前置URL"),
                                    )
                                    .p(px(4.))
                                    .child(
                                        v_flex()
                                            .w(px(280.))
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme_c.muted_foreground)
                                                    .px_2()
                                                    .py_1()
                                                    .child("选择前置URL"),
                                            )
                                            .child({
                                                let pe = panel_entity.clone();
                                                div()
                                                    .id("req-base-clear")
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme_c.muted))
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(theme_c.muted_foreground)
                                                            .child("（空）使用目录配置"),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_ev, window, cx: &mut App| {
                                                            let _ = pe.update(cx, |this, cx| {
                                                                // Inherit: reset to the folder's
                                                                // base_url. Clear the input; on the
                                                                // next render the label falls back
                                                                // to "前置URL" or the resolved folder
                                                                // value, and send-time resolution
                                                                // uses resolve_folder_base_url.
                                                                this.req_base_mode = None;
                                                                this.req_base_url.update(
                                                                    cx,
                                                                    |input, cx| {
                                                                        input.set_value(
                                                                            "", window, cx,
                                                                        );
                                                                    },
                                                                );
                                                                this.commit_to_model(cx);
                                                                this.req_baseurl_open = false;
                                                                cx.notify();
                                                            });
                                                            window.refresh();
                                                        },
                                                    )
                                            })
                                            .child({
                                                let pe = panel_entity.clone();
                                                div()
                                                    .id("req-base-disable")
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme_c.muted))
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(theme_c.muted_foreground)
                                                            .child("不使用前置URL"),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_ev, window, cx: &mut App| {
                                                            let _ = pe.update(cx, |this, cx| {
                                                                // Explicit disable: this request
                                                                // carries no prefix at all, even
                                                                // if a folder base_url is set.
                                                                this.req_base_mode = Some(None);
                                                                this.req_base_url.update(
                                                                    cx,
                                                                    |input, cx| {
                                                                        input.set_value(
                                                                            "", window, cx,
                                                                        );
                                                                    },
                                                                );
                                                                this.commit_to_model(cx);
                                                                this.req_baseurl_open = false;
                                                                cx.notify();
                                                            });
                                                            window.refresh();
                                                        },
                                                    )
                                            })
                                            .child(
                                                div()
                                                    .id("req-base-scroll")
                                                    .max_h(px(300.))
                                                    .overflow_y_scroll()
                                                    .children(env_urls.iter().enumerate().map(
                                                        |(i, (k, v))| {
                                                            let val_display =
                                                                v.trim_end_matches('/').to_string();
                                                            let key = k.clone();
                                                            // Store the {{key}} placeholder so the value
                                                            // follows env var changes; the literal value is
                                                            // only shown in the dropdown as a preview.
                                                            let placeholder =
                                                                format!("{{{{{}}}}}", key);
                                                            let tc = theme_c.clone();
                                                            let pe = panel_entity.clone();

                                                            div()
                                                        .id(("req-base-opt", i))
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .cursor_pointer()
                                                        .hover(|s| s.bg(tc.muted))
                                                        .child(
                                                            v_flex()
                                                                .gap(px(1.))
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .font_weight(
                                                                            FontWeight::SEMIBOLD,
                                                                        )
                                                                        .child(key.clone()),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(
                                                                            tc.muted_foreground,
                                                                        )
                                                                        .child(val_display),
                                                                ),
                                                        )
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            move |_ev, window, cx: &mut App| {
                                                                let _ =
                                                                    pe.update(cx, |this, cx| {
                                                                        // Override with the
                                                                        // selected {{var}} value.
                                                                        this.req_base_mode =
                                                                            Some(Some(
                                                                                placeholder.clone(),
                                                                            ));
                                                                        this.req_base_url.update(
                                                                            cx,
                                                                            |input, cx| {
                                                                                input.set_value(
                                                                                    &placeholder,
                                                                                    window,
                                                                                    cx,
                                                                                );
                                                                            },
                                                                        );
                                                                        this.commit_to_model(cx);
                                                                        this.req_baseurl_open =
                                                                            false;
                                                                        cx.notify();
                                                                    });
                                                                window.refresh();
                                                            },
                                                        )
                                                        },
                                                    )),
                                            ),
                                    )
                                    .into_any_element()
                            },
                        )
                        .child(
                            // Path input (the part after the base URL).
                            div().flex_1().child(Input::new(&self.url).small()),
                        )
                        .child({
                            // Send/Stop: a bold, method-colored pill that is
                            // noticeably taller and wider than the other small
                            // controls so it reads as the primary action. A
                            // custom button variant supplies the fill, hover
                            // and active colors (the base Button sets its own
                            // hover state, so colors must go through the
                            // variant rather than `.bg().hover()`).
                            let (send_bg, send_hover, send_active) = if streaming {
                                (theme.danger, theme.danger_hover, theme.danger_active)
                            } else {
                                let lighter = gpui::hsla(
                                    method_fill.h,
                                    method_fill.s,
                                    (method_fill.l + 0.06).min(0.9),
                                    method_fill.a,
                                );
                                let darker = gpui::hsla(
                                    method_fill.h,
                                    method_fill.s,
                                    (method_fill.l - 0.06).max(0.0),
                                    method_fill.a,
                                );
                                (method_fill, lighter, darker)
                            };
                            let variant = ButtonCustomVariant::new(cx)
                                .color(send_bg)
                                .foreground(gpui::white())
                                .hover(send_hover)
                                .active(send_active);
                            Button::new("send")
                                .custom(variant)
                                .w(px(88.))
                                .h(px(32.))
                                .rounded(px(6.))
                                .text_size(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .shadow_md()
                                .label(if streaming {
                                    "停止"
                                } else if sending {
                                    "请求中"
                                } else {
                                    "发送"
                                })
                                .icon(if streaming {
                                    IconName::Close
                                } else if sending {
                                    IconName::Loader
                                } else {
                                    IconName::Play
                                })
                                .disabled(sending && !streaming)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    // Re-check streaming state at click time.
                                    let streaming_now = this
                                        .state
                                        .read(cx)
                                        .active_project()
                                        .and_then(|p| {
                                            p.find_request(this.request_id.as_deref()?)
                                                .and_then(|(_, r)| r.last_response.as_ref())
                                        })
                                        .map(|r| r.streaming)
                                        .unwrap_or(false);
                                    if streaming_now {
                                        this.stop_active(cx);
                                    } else {
                                        this.send(cx);
                                    }
                                }))
                        }),
                )
                .child({
                    // Compute non-empty row counts for count badges.
                    let hdr_count = self
                        .headers_rows
                        .iter()
                        .filter(|r| r.has_content(cx))
                        .count();
                    let qry_count = self
                        .params_rows
                        .iter()
                        .filter(|r| r.has_content(cx))
                        .count();
                    let path_count = self.path_rows.iter().filter(|r| r.has_content(cx)).count();
                    let body_count = if self.body_type != BodyType::None {
                        1
                    } else {
                        0
                    };
                    let auth_count = if self.auth_type != AuthType::None {
                        1
                    } else {
                        0
                    };
                    let cookie_count = self
                        .cookie_rows
                        .iter()
                        .filter(|r| r.has_content(cx))
                        .count();
                    // Tab strip
                    h_flex()
                        .px_3()
                        .py(px(4.))
                        .gap_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(tab_label(
                            "Header",
                            Some(hdr_count),
                            active_tab == ReqTab::Headers,
                            ReqTab::Headers,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "Query",
                            Some(qry_count),
                            active_tab == ReqTab::Query,
                            ReqTab::Query,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "Path",
                            Some(path_count),
                            active_tab == ReqTab::Path,
                            ReqTab::Path,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "Body",
                            Some(body_count),
                            active_tab == ReqTab::Body,
                            ReqTab::Body,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "认证",
                            Some(auth_count),
                            active_tab == ReqTab::Auth,
                            ReqTab::Auth,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "Cookie",
                            Some(cookie_count),
                            active_tab == ReqTab::Cookie,
                            ReqTab::Cookie,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "预执行操作",
                            None,
                            active_tab == ReqTab::PreRequest,
                            ReqTab::PreRequest,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "后执行操作",
                            None,
                            active_tab == ReqTab::PostRequest,
                            ReqTab::PostRequest,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "Curl",
                            None,
                            active_tab == ReqTab::Curl,
                            ReqTab::Curl,
                            &theme,
                            cx,
                        ))
                        .child(tab_label(
                            "Mock",
                            None,
                            active_tab == ReqTab::Mock,
                            ReqTab::Mock,
                            &theme,
                            cx,
                        ))
                })
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .p_3()
                        .child(self.render_active_tab(window, cx)),
                )
            })
    }
}

/// Build a clickable tab label with an optional count badge.
///
/// Apifox/Postman-style tab: inactive labels are muted gray; the active label
/// is rendered in the theme primary color with a 2 px underline in the same
/// color, rather than as a filled pill. The optional count badge is a small
/// primary-tinted pill for active tabs and a neutral gray pill otherwise.
pub(super) fn tab_label(
    label: &'static str,
    count: Option<usize>,
    is_active: bool,
    tab: ReqTab,
    theme: &gpui_component::Theme,
    cx: &mut Context<RequestPanel>,
) -> impl IntoElement {
    let has_count = count.map(|c| c > 0).unwrap_or(false);
    h_flex()
        .id(label.to_string())
        .relative()
        .px_2()
        .py_1()
        .gap_1()
        .text_size(px(13.))
        .items_center()
        .cursor_pointer()
        .text_color(if is_active {
            theme.primary
        } else {
            theme.muted_foreground
        })
        .when(is_active, |this| this.font_weight(FontWeight::SEMIBOLD))
        .when(!is_active, |this| {
            this.hover(|s| s.text_color(theme.foreground))
        })
        // Active underline: a 2px primary-colored bar pinned to the bottom.
        .when(is_active, |this| {
            this.child(
                div()
                    .absolute()
                    .left_2()
                    .right_2()
                    .bottom(px(-5.))
                    .h(px(2.))
                    .rounded_full()
                    .bg(theme.primary),
            )
        })
        .child(label.to_string())
        .when(has_count, |c| {
            c.child(
                div()
                    .text_size(px(10.))
                    .min_w(px(16.))
                    .h(px(16.))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(if is_active {
                        theme.primary.opacity(0.18)
                    } else {
                        theme.muted
                    })
                    .text_color(if is_active {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    })
                    .child(count.unwrap().to_string()),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.active_tab = tab;
            cx.notify();
        }))
}

impl Focusable for RequestPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for RequestPanel {}
