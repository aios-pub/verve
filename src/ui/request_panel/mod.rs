//! Center pane: the request editor workbench.
//!
//! Method selector + URL bar + Send, and tabs for Params / Headers / Body /
//! Scripts / Docs. On Send, resolves variables, executes the request via the
//! shared HTTP client, and writes the response back into the request's
//! `last_response`, emitting [`AppEvent::ResponseUpdated`].

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::Icon;
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
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
mod folder_helpers;
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
    /// Whether the base URL popover is open.
    pub req_baseurl_open: bool,
    pub name: Entity<InputState>,
    pub method_select: Entity<SelectState<Vec<String>>>,
    pub params_rows: Vec<KvRow>,
    pub headers_rows: Vec<KvRow>,
    pub path_rows: Vec<KvRow>,
    pub cookie_rows: Vec<KvRow>,
    pub body_rows: Vec<KvRow>,
    pub body_editor: Entity<InputState>,
    pub body_lang_select: Entity<SelectState<Vec<String>>>,
    pub body_type_select: Entity<SelectState<Vec<String>>>,
    pub pre_script_editor: Entity<InputState>,
    pub tests_editor: Entity<InputState>,
    // --- mock fields
    pub mock_enabled: bool,
    pub mock_status_input: Entity<InputState>,
    pub mock_delay_input: Entity<InputState>,
    pub mock_body_editor: Entity<InputState>,
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
    pub folder_desc: Entity<InputState>,
    /// Folder base URL input (folder settings tab).
    pub folder_base_url: Entity<InputState>,
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
    focus_handle: FocusHandle,
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
            InputState::new(window, cx)
                .code_editor(RawLanguage::Json.highlight())
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
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("// 预执行脚本：apt.setVariable(k,v) · apt.getVariable(k)")
        });
        let tests_editor = cx.new(|cx| {
            InputState::new(window, cx).multi_line(true).placeholder(
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
            InputState::new(window, cx)
                .code_editor(RawLanguage::Json.highlight())
                .placeholder("{}")
                .default_value("{}")
                .multi_line(true)
                .rows(8)
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
            InputState::new(window, cx)
                .multi_line(true)
                .rows(4)
                .placeholder("目录描述（可选）：说明该目录下接口的用途、归属模块等")
        });
        let folder_base_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://api.example.com"));

        let panel = Self {
            state: state.clone(),
            request_id: None,
            url,
            req_base_url,
            req_baseurl_open: false,
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
            folder_base_url,
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
            focus_handle: cx.focus_handle(),
        };
        // Subscribe to selection changes to load the active request.
        let sub = cx.subscribe(&state, Self::on_state_event);
        // Auth type/target change subscriptions (panel is built now).
        let auth_type_sub = cx.subscribe(&panel.auth_type_select, Self::on_auth_change);
        let auth_target_sub = cx.subscribe(&panel.auth_target_select, Self::on_auth_change);
        let mut subs = vec![sub, bt_sub, auth_type_sub, auth_target_sub];
        // Subscribe to each input's blur to commit edits back to the model.
        subs.push(cx.subscribe(&panel.url.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.name.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.body_editor.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.pre_script_editor.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.tests_editor.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.mock_status_input.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.mock_delay_input.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.mock_body_editor.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.mock_match_path_input.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.auth_token.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.auth_username.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.auth_password.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.auth_key.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.auth_value.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.folder_name.clone(), Self::on_input_blur));
        subs.push(cx.subscribe(&panel.folder_desc.clone(), Self::on_input_blur));
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
    pub(super) fn on_input_blur(&mut self, _src: Entity<InputState>, ev: &InputEvent, cx: &mut Context<Self>) {
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

    pub(super) fn on_state_event(&mut self, _src: Entity<AppState>, ev: &AppEvent, cx: &mut Context<Self>) {
        if matches!(
            ev,
            AppEvent::SelectionChanged | AppEvent::EnvironmentChanged
        ) {
            // Reload the active request/folder so the URL base_url display
            // and all variable-dependent fields pick up the new env values.
            self.pending_reload = true;
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
                // If URL starts with http:// or https://, extract host as base_url.
                // Otherwise, try folder base_url and show only the path.
                let (base_display, path_display) = {
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
                        // Relative path — check folder base_url.
                        let base = {
                            let st = self.state.read(cx);
                            st.active_project().and_then(|p| {
                                p.find_request(&r.id)
                                    .and_then(|(chain, _)| resolve_folder_base_url(p, &chain))
                            })
                        };
                        match base {
                            Some(b) => (b, r.url.clone()),
                            None => (String::new(), r.url.clone()),
                        }
                    }
                };
                self.req_base_url
                    .update(cx, |s, cx| s.set_value(base_display, window, cx));
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
                self.params_rows = kv_table::rows_from_pairs(&r.params, window, cx);
                self.headers_rows = kv_table::rows_from_pairs(&r.headers, window, cx);
                self.path_rows = kv_table::rows_from_pairs(&r.path, window, cx);
                self.cookie_rows = kv_table::rows_from_pairs(&r.cookies, window, cx);
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
                    window,
                    cx,
                );
                // Visual-mode rows for the Raw body (reused across renders so
                // typing in the field inputs doesn't lose focus).
                self.raw_visual_rows = kv_table::rows_from_pairs(&r.body.raw_parameter, window, cx);
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
                    self.mock_headers_rows = kv_table::rows_from_pairs(&mock.headers, window, cx);
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
                        kv_table::rows_from_pairs(&mock.match_query, window, cx);
                    self.mock_match_header_rows =
                        kv_table::rows_from_pairs(&mock.match_headers, window, cx);
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
    pub(super) fn load_active_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
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
                self.folder_base_url.update(cx, |s, cx| {
                    s.set_value(f.base_url.clone().unwrap_or_default(), window, cx)
                });
                self.folder_param_rows = kv_table::rows_from_pairs(&f.params, window, cx);
                self.folder_header_rows = kv_table::rows_from_pairs(&f.headers, window, cx);
                self.folder_var_rows = kv_table::rows_from_pairs(&f.variables, window, cx);
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
        let base_url_raw = self.folder_base_url.read(cx).value().trim().to_string();
        let base_url = if base_url_raw.is_empty() {
            None
        } else {
            Some(base_url_raw)
        };
        let params = kv_table::pairs_from_rows(&self.folder_param_rows, cx);
        let headers = kv_table::pairs_from_rows(&self.folder_header_rows, cx);
        let variables = kv_table::pairs_from_rows(&self.folder_var_rows, cx);
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, folder)) = project.find_folder_mut(&id) {
                    folder.name = name;
                    folder.description = description;
                    folder.base_url = base_url;
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
        let url = self.url.read(cx).value().to_string();
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
        let remote_cfg = crate::state::persistence::load_remote_config();
        system.insert(
            "mock_server".to_string(),
            crate::share::remote::mock_server_url(&remote_cfg),
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
        // Inject folder base_url as "baseUrl" + special key for relative paths.
        // A per-request base_url override (selected from the dropdown) takes
        // precedence over the folder's base_url.
        let req_base_raw = self.req_base_url.read(cx).value().to_string();
        if !req_base_raw.trim().is_empty() {
            let resolved = crate::http::variable::substitute(&req_base_raw, &map);
            if !resolved.trim().is_empty() {
                map.insert(
                    "__folder_base_url__".to_string(),
                    resolved.trim_end_matches('/').to_string(),
                );
                map.entry("baseUrl".to_string()).or_insert(resolved);
            }
        } else if let Some(base) = resolve_folder_base_url(project, &chain) {
            map.insert("__folder_base_url__".to_string(), base.clone());
            map.entry("baseUrl".to_string()).or_insert(base);
        }
        map
    }

    /// Execute the active request on a background task and store the response.
    /// Generate a curl command from the current request's editable fields.
    pub(super) fn generate_curl(&self, cx: &mut Context<Self>) -> String {
        // For curl display, substitute variables so the user sees the real URL.
        let url_raw = self.url.read(cx).value().to_string();
        let vars = self.effective_vars(cx);
        let url = crate::http::variable::substitute(&url_raw, &vars);
        let method = self
            .method_select
            .read(cx)
            .selected_value()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "GET".into());

        let mut parts: Vec<String> = Vec::new();
        parts.push("curl".into());

        // Method.
        if method != "GET" {
            parts.push(format!("-X {}", method));
        }

        // Headers. Track whether the user already set a Content-Type so we
        // don't override it when auto-injecting one for the body.
        let mut has_content_type = false;
        for kv in &self.headers_rows {
            let key = kv.key.read(cx).value().to_string();
            let val = kv.value.read(cx).value().to_string();
            if kv.enabled && !key.trim().is_empty() {
                if key.eq_ignore_ascii_case("content-type") {
                    has_content_type = true;
                }
                parts.push(format!("-H '{}: {}'", key, val));
            }
        }

        // Auth headers.
        match self.auth_type {
            crate::state::models::AuthType::Bearer => {
                let token = self.auth_token.read(cx).value().to_string();
                if !token.is_empty() {
                    parts.push(format!("-H 'Authorization: Bearer {}'", token));
                }
            }
            crate::state::models::AuthType::Basic => {
                let user = self.auth_username.read(cx).value().to_string();
                let pass = self.auth_password.read(cx).value().to_string();
                if !user.is_empty() {
                    parts.push(format!("-u '{}:{}'", user, pass));
                }
            }
            _ => {}
        }

        // Body. Auto-inject a Content-Type when missing, matching the real
        // HTTP client prepare() logic.
        let body_lang = self
            .body_lang_select
            .read(cx)
            .selected_value()
            .cloned()
            .and_then(|s| RawLanguage::parse_name(&s))
            .unwrap_or_default();
        if self.body_type == crate::state::models::BodyType::Raw {
            let raw = self.body_editor.read(cx).text().to_string();
            if !raw.trim().is_empty() {
                if !has_content_type {
                    let ct = body_lang.content_type();
                    parts.push(format!("-H 'Content-Type: {}'", ct));
                }
                parts.push(format!("-d '{}'", raw.replace('\'', "'\\''")));
            }
        } else if self.body_type == crate::state::models::BodyType::FormData {
            let mut has_form_field = false;
            for kv in &self.body_rows {
                let key = kv.key.read(cx).value().to_string();
                if kv.enabled && !key.trim().is_empty() {
                    // File rows send the file via curl's `@path` syntax; text
                    // rows use the literal value.
                    if kv.field_type == FieldType::File {
                        let path = kv.file_path.clone().unwrap_or_default();
                        if !path.trim().is_empty() {
                            // Quote the path so paths with spaces/special chars
                            // survive the shell. The leading `@` tells curl to
                            // read & upload the file.
                            parts.push(format!("-F '{}=@\"{}\"'", key, path.replace('"', "\\\"")));
                            has_form_field = true;
                        }
                    } else {
                        let val = kv.value.read(cx).value().to_string();
                        parts.push(format!("-F '{}={}'", key, val));
                        has_form_field = true;
                    }
                }
            }
            // curl sets multipart/form-data automatically when using -F, so
            // no need to inject Content-Type manually here.
            let _ = has_form_field;
        } else if self.body_type == crate::state::models::BodyType::Urlencoded {
            let pairs: Vec<String> = self
                .body_rows
                .iter()
                .filter(|kv| kv.enabled)
                .map(|kv| {
                    let k = kv.key.read(cx).value().to_string();
                    let v = kv.value.read(cx).value().to_string();
                    format!("{}={}", k, v)
                })
                .collect();
            if !pairs.is_empty() {
                if !has_content_type {
                    parts.push("-H 'Content-Type: application/x-www-form-urlencoded'".into());
                }
                parts.push(format!("-d '{}'", pairs.join("&")));
            }
        }

        // URL (with base_url prefix and query params appended).
        let final_url = {
            // If the URL is a relative path, prepend the effective base_url
            // (per-request override or folder base_url) so the curl command
            // is complete and runnable.
            let url_with_base = if !url.starts_with("http://") && !url.starts_with("https://") {
                if let Some(base) = vars.get("__folder_base_url__") {
                    if !base.is_empty() {
                        let base = base.trim_end_matches('/');
                        let path = url.trim_start_matches('/');
                        format!("{}/{}", base, path)
                    } else {
                        url.clone()
                    }
                } else {
                    url.clone()
                }
            } else {
                url.clone()
            };
            let mut url_with_params = url_with_base;
            let query_pairs: Vec<String> = self
                .params_rows
                .iter()
                .filter(|kv| kv.enabled)
                .filter_map(|kv| {
                    let k = kv.key.read(cx).value().to_string();
                    if k.trim().is_empty() {
                        return None;
                    }
                    let v = kv.value.read(cx).value().to_string();
                    Some(format!("{}={}", k, v))
                })
                .collect();
            if !query_pairs.is_empty() {
                let sep = if url_with_params.contains('?') {
                    "&"
                } else {
                    "?"
                };
                url_with_params = format!("{}{}{}", url_with_params, sep, query_pairs.join("&"));
            }
            url_with_params
        };

        parts.push(format!("'{}'", final_url));

        parts.join(" \\\n  ")
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
        let open_tabs: Vec<(String, String, RequestMethod)> = {
            let st = self.state.read(cx);
            st.open_request_ids
                .iter()
                .filter_map(|id| {
                    st.active_project()
                        .and_then(|p| p.find_request(id))
                        .map(|(_, r)| (r.id.clone(), r.name.clone(), r.method))
                })
                .collect()
        };
        let active_tab_id = self.state.read(cx).active_tab_id.clone();
        let has_tabs = !open_tabs.is_empty();

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
                        .children(open_tabs.iter().enumerate().map(|(i, (id, name, method))| {
                            let is_active = active_tab_id.as_deref() == Some(id);
                            let method_color = crate::ui::method_colors::badge_color(*method, cx);
                            let method_label = method.badge_label();
                            let display_name = if name.chars().count() > 14 {
                                let truncated: String = name.chars().take(14).collect();
                                format!("{}…", truncated)
                            } else {
                                name.clone()
                            };
                            let id_focus = id.clone();
                            let id_close = id.clone();
                            let panel_entity = cx.entity();
                            h_flex()
                                .id(("req-tab", i))
                                .items_center()
                                .gap(px(6.))
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(is_active, |d| {
                                    d.bg(theme.background).border_1().border_color(theme.border)
                                })
                                .when(!is_active, |d| {
                                    d.text_color(theme.muted_foreground)
                                        .hover(|s| s.bg(theme.accent.opacity(0.15)))
                                })
                                // Colored method badge.
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(method_color)
                                        .child(method_label),
                                )
                                // Tab name.
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .when(is_active, |d| d.text_color(theme.foreground))
                                        .child(display_name),
                                )
                                // Close button.
                                .child(
                                    Button::new(("req-tab-close", i))
                                        .ghost()
                                        .xsmall()
                                        .label("×")
                                        .text_size(px(14.))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            let id_close_clone = id_close.clone();
                                            let _ = panel_entity.update(cx, move |this, cx| {
                                                this.state.update(cx, |s, cx| {
                                                    s.close_tab(&id_close_clone, cx);
                                                });
                                                cx.notify();
                                            });
                                        })),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    // Click on the tab body (not close button) focuses it.
                                    this.state.update(cx, |s, cx| {
                                        s.set_active_tab(&id_focus, cx);
                                    });
                                }))
                        })),
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
                            bar.child(
                                div().w(px(110.)).child(
                                    Select::new(&self.method_select).small().appearance(true),
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
                                // Substitute any {{var}} placeholder so the
                                // button label shows the resolved URL, not the
                                // raw placeholder text.
                                let base_val = if base_val_raw.contains("{{") {
                                    let vars = self.effective_vars(cx);
                                    crate::http::variable::substitute(&base_val_raw, &vars)
                                } else {
                                    base_val_raw
                                };
                                let label_text = if base_val.trim().is_empty() {
                                    "前置URL".to_string()
                                } else {
                                    base_val
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
                                            .ghost()
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
                                                                this.req_base_url.update(
                                                                    cx,
                                                                    |input, cx| {
                                                                        input.set_value(
                                                                            "", window, cx,
                                                                        );
                                                                    },
                                                                );
                                                                this.req_baseurl_open = false;
                                                                cx.notify();
                                                            });
                                                            window.refresh();
                                                        },
                                                    )
                                            })
                                            .children(env_urls.iter().enumerate().map(
                                                |(i, (k, v))| {
                                                    let val_display =
                                                        v.trim_end_matches('/').to_string();
                                                    let key = k.clone();
                                                    // Store the {{key}} placeholder so the value
                                                    // follows env var changes; the literal value is
                                                    // only shown in the dropdown as a preview.
                                                    let placeholder = format!("{{{{{}}}}}", key);
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
                                                                        .child(format!(
                                                                            "{{{{{}}}}}",
                                                                            key
                                                                        )),
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
                                                                        this.req_baseurl_open =
                                                                            false;
                                                                        cx.notify();
                                                                    });
                                                                window.refresh();
                                                            },
                                                        )
                                                },
                                            )),
                                    )
                                    .into_any_element()
                            },
                        )
                        .child(
                            // Path input (the part after the base URL).
                            div().flex_1().child(Input::new(&self.url).small()),
                        )
                        .child(
                            Button::new("send")
                                .small()
                                .label(if streaming { "停止" } else { "发送" })
                                .icon(if streaming {
                                    IconName::Close
                                } else {
                                    IconName::Play
                                })
                                .disabled(sending && !streaming)
                                .when(!sending && !streaming, |btn| {
                                    btn.bg(method_fill).text_color(gpui::white())
                                })
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
                                })),
                        ),
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

pub(super) fn tab_label(
    label: &'static str,
    count: Option<usize>,
    is_active: bool,
    tab: ReqTab,
    theme: &gpui_component::Theme,
    cx: &mut Context<RequestPanel>,
) -> impl IntoElement {
    let has_count = count.map(|c| c > 0).unwrap_or(false);
    div()
        .id(label.to_string())
        .px_2()
        .py_1p5()
        .rounded_sm()
        .gap_1()
        .text_sm()
        .items_center()
        .text_color(if is_active {
            theme.primary
        } else {
            theme.muted_foreground
        })
        .when(is_active, |this| {
            this.font_weight(FontWeight::SEMIBOLD)
                .bg(theme.accent.opacity(0.15))
        })
        .when(!is_active, |this| {
            this.hover(|s| s.bg(theme.accent.opacity(0.1)))
        })
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .child(label.to_string())
                .when(has_count, |c| {
                    c.child(
                        div()
                            .text_size(px(10.))
                            .px(px(4.))
                            .py(px(0.))
                            .rounded_full()
                            .bg(if is_active {
                                theme.primary.opacity(0.2)
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
                }),
        )
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
