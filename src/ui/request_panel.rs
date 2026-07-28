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

/// Shared stop-flags for in-flight streaming requests (SSE/WebSocket), keyed by
/// request id. A background task polls its flag and aborts once it goes true.
static ACTIVE_STOP_FLAGS: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    >,
> = std::sync::OnceLock::new();

fn stop_flags() -> &'static std::sync::Mutex<
    std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
> {
    ACTIVE_STOP_FLAGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Register a stop flag for a request id and return it for the background task.
fn register_stop(id: &str) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    stop_flags()
        .lock()
        .unwrap()
        .insert(id.to_string(), flag.clone());
    flag
}

/// Remove a request's stop flag once its task completes.
fn unregister_stop(id: &str) {
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
    fn on_body_type_change(
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
    fn on_auth_change(
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
    fn on_input_blur(&mut self, _src: Entity<InputState>, ev: &InputEvent, cx: &mut Context<Self>) {
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

    fn on_state_event(&mut self, _src: Entity<AppState>, ev: &AppEvent, cx: &mut Context<Self>) {
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
    fn load_active_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
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
    fn commit_folder(&mut self, cx: &mut Context<Self>) {
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
    fn commit_to_model(&mut self, cx: &mut Context<Self>) {
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
    fn effective_vars(&self, cx: &App) -> BTreeMap<String, String> {
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
    fn generate_curl(&self, cx: &mut Context<Self>) -> String {
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
                let val = kv.value.read(cx).value().to_string();
                if kv.enabled && !key.trim().is_empty() {
                    parts.push(format!("-F '{}={}'", key, val));
                    has_form_field = true;
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

    pub fn send(&mut self, cx: &mut Context<Self>) {
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        self.commit_to_model(cx);

        // Resolve the request fields from the model (post-commit).
        let (pre_script, tests_script, req_clone, mut vars) = {
            let st = self.state.read(cx);
            let project = match st.active_project() {
                Some(p) => p,
                None => return,
            };
            let (chain, req) = match project.find_request(&id) {
                Some(x) => x,
                None => return,
            };
            let folder_vars: Vec<KeyValue> = project
                .folder_variables_chain(&chain)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            // System variables: mock_server address
            let mut system = BTreeMap::new();
            let remote_cfg = crate::state::persistence::load_remote_config();
            system.insert(
                "mock_server".to_string(),
                crate::share::remote::mock_server_url(&remote_cfg),
            );
            let mut vars = crate::state::models::effective_variables(
                &system,
                &project.global_variables,
                project.active_env_variables(),
                &folder_vars,
                &req.variables,
            );
            // Inject folder base_url into vars so {{baseUrl}} and relative
            // path resolution both work. The base_url value itself is a URL
            // like "https://api.example.com" — it gets substituted into any
            // {{baseUrl}} placeholder AND used for relative path prefixing.
            // A per-request base_url override (selected from the dropdown)
            // takes precedence over the folder's base_url.
            let req_base_raw = self.req_base_url.read(cx).value().to_string();
            if !req_base_raw.trim().is_empty() {
                // Substitute any {{var}} placeholders in the per-request
                // base_url against the current effective vars.
                let resolved_req_base = crate::http::variable::substitute(&req_base_raw, &vars);
                if !resolved_req_base.trim().is_empty() {
                    vars.insert(
                        "__folder_base_url__".to_string(),
                        resolved_req_base.trim_end_matches('/').to_string(),
                    );
                    vars.entry("baseUrl".to_string())
                        .or_insert(resolved_req_base);
                }
            } else if let Some(base) = resolve_folder_base_url(project, &chain) {
                vars.insert("__folder_base_url__".to_string(), base.clone());
                vars.entry("baseUrl".to_string()).or_insert(base);
            }
            (
                req.pre_script.clone(),
                req.tests_script.clone(),
                req.clone(),
                vars,
            )
        };

        // Dispatch by protocol. HTTP/SSE/GraphQL share the HTTP prepare path;
        // the rest are handled (or stubbed) in their own branches.
        match req_clone.protocol {
            Protocol::Sse => {
                self.send_sse(id, req_clone, vars, pre_script, cx);
                return;
            }
            Protocol::WebSocket => {
                self.send_websocket(id, req_clone, vars, cx);
                return;
            }
            Protocol::Tcp => {
                self.send_tcp(id, req_clone, vars, cx);
                return;
            }
            Protocol::Grpc => {
                self.send_grpc(id, req_clone, vars, cx);
                return;
            }
            Protocol::SocketIo => {
                self.send_socketio(id, req_clone, vars, cx);
                return;
            }
            Protocol::Markdown | Protocol::Directory => {
                self.send_placeholder(id, req_clone.protocol, cx);
                return;
            }
            // HTTP and GraphQL execute via the HTTP path below.
            Protocol::Http | Protocol::Graphql => {}
        }

        // --- Pre-request script (PRD §5.2): runs before the request is sent.
        //     Variable mutations are merged into `vars` so they get substituted.
        let pre_result = crate::scripting::run_pre_request(&pre_script, &vars);
        let mut script_logs: Vec<String> = Vec::new();
        if !pre_script.trim().is_empty() {
            script_logs.push("— Pre-request Script —".to_string());
            script_logs.extend(pre_result.logs.clone());
            if let Some(e) = &pre_result.error {
                script_logs.push(format!("Pre-script error: {e}"));
            }
            // Apply SetVariable effects to the vars map (for substitution).
            for effect in &pre_result.effects {
                if let crate::scripting::SideEffect::SetVariable { name, value, .. } = effect {
                    vars.insert(name.clone(), value.clone());
                }
            }
        }

        let prepared = match http::prepare(
            req_clone.method,
            &req_clone.url,
            &req_clone.params,
            &req_clone.headers,
            &req_clone.path,
            &req_clone.cookies,
            &req_clone.auth,
            &req_clone.body,
            &vars,
            30,
        ) {
            Ok(p) => p,
            Err(e) => {
                let err_resp = Response {
                    error: Some(format!("{e}")),
                    ..Default::default()
                };
                self.state.update(cx, |s, _cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id) {
                            r.last_response = Some(err_resp.clone());
                            apply_autosave_example(r, &err_resp);
                        }
                    }
                });
                // 必须在可变借用释放后再emit，避免双重借用panic。
                // spawn异步任务在当前同步流程结束后执行，借用已释放。
                let state = self.state.clone();
                cx.spawn(async move |_, cx| {
                    let _ = state.update(cx, |s, cx| {
                        let _ = s;
                        cx.emit(AppEvent::ResponseUpdated(id.clone()));
                    });
                })
                .detach();
                return;
            }
        };

        let client = cx.http_client();
        let id_clone = id.clone();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));
        let env_id = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| p.active_environment.clone());
        cx.spawn(async move |this, cx| {
            let mut resp = http::execute(client.as_ref(), prepared, 30).await;

            // --- Post-request / Tests script (PRD §5.2): reads `response`,
            //     extracts data (e.g. token) into variables, runs assertions.
            if !tests_script.trim().is_empty() {
                let post_result = crate::scripting::run_post_request(&tests_script, &vars, &resp);
                script_logs.push("— Tests Script —".to_string());
                script_logs.extend(post_result.logs.clone());
                if let Some(e) = &post_result.error {
                    script_logs.push(format!("Tests error: {e}"));
                }
                // Persist SetVariable effects into the active environment.
                let env_effects: Vec<(String, String)> = post_result
                    .effects
                    .iter()
                    .filter_map(|e| match e {
                        crate::scripting::SideEffect::SetVariable {
                            scope: crate::scripting::VarScope::Environment,
                            name,
                            value,
                        } => Some((name.clone(), value.clone())),
                        _ => None,
                    })
                    .collect();
                if !env_effects.is_empty() {
                    let _ = this.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| {
                            if let Some(project) = s.active_project_mut() {
                                if let Some(env_id) = &env_id {
                                    if let Some(env) =
                                        project.environments.iter_mut().find(|e| &e.id == env_id)
                                    {
                                        for (k, v) in &env_effects {
                                            if let Some(kv) =
                                                env.variables.iter_mut().find(|kv| &kv.key == k)
                                            {
                                                kv.value = v.clone();
                                            } else {
                                                env.variables.push(
                                                    crate::state::models::KeyValue::new(k, v),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            s.notify_edited(cx);
                        });
                    });
                }
                // Surface assertion summary on the response.
                if post_result.assertions_failed > 0 || post_result.assertions_passed > 0 {
                    let summary = format!(
                        "{} passed, {} failed",
                        post_result.assertions_passed, post_result.assertions_failed
                    );
                    if let Some(err) = &mut resp.error {
                        err.push_str(&format!(" [{summary}]"));
                    } else if post_result.assertions_failed > 0 {
                        resp.error = Some(format!("Assertions: {summary}"));
                    }
                }
            }

            // Attach script logs to the response body as a footer so they show
            // up in the response panel. (A dedicated script-console is a
            // future enhancement.)
            if !script_logs.is_empty() {
                let mut combined = resp.body.clone();
                if !combined.is_empty() {
                    combined.push_str("\n\n");
                }
                combined.push_str("// ── Script Output ──\n");
                combined.push_str(&script_logs.join("\n"));
                resp.body = combined;
            }

            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    // Look up the request once so we can both store last_response
                    // and build a rich history entry from its authored fields.
                    let (project_id, name, method, url, query_params, request_headers) = {
                        match s.active_project_mut() {
                            Some(project) => {
                                let pid = project.id.clone();
                                match project.find_request_mut(&id_clone) {
                                    Some((_, r)) => {
                                        r.last_response = Some(resp.clone());
                                        apply_autosave_example(r, &resp);
                                        let qp: Vec<(String, String)> = r
                                            .params
                                            .iter()
                                            .filter(|kv| kv.enabled && !kv.key.is_empty())
                                            .take(crate::state::HISTORY_KV_MAX)
                                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                                            .collect();
                                        let rh: Vec<(String, String)> = r
                                            .headers
                                            .iter()
                                            .filter(|kv| kv.enabled && !kv.key.is_empty())
                                            .take(crate::state::HISTORY_KV_MAX)
                                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                                            .collect();
                                        (pid, r.name.clone(), r.method, r.url.clone(), qp, rh)
                                    }
                                    None => (
                                        pid,
                                        String::new(),
                                        RequestMethod::Get,
                                        String::new(),
                                        Vec::new(),
                                        Vec::new(),
                                    ),
                                }
                            }
                            None => (
                                String::new(),
                                String::new(),
                                RequestMethod::Get,
                                String::new(),
                                Vec::new(),
                                Vec::new(),
                            ),
                        }
                    };

                    // Truncate response body at char boundary.
                    let (response_body, response_truncated) = truncate_history_body(&resp.body);

                    // Append to history.
                    let entry = HistoryEntry {
                        id: new_id(),
                        project_id,
                        request_id: Some(id_clone.clone()),
                        name,
                        method,
                        url,
                        status: resp.status,
                        status_text: resp.status_text.clone(),
                        time_ms: resp.time_ms,
                        size: resp.size,
                        at: chrono::Utc::now().to_rfc3339(),
                        error: resp.error.clone(),
                        query_params,
                        request_headers,
                        response_body,
                        response_truncated,
                    };
                    s.data.history.insert(0, entry);
                    if s.data.history.len() > 200 {
                        s.data.history.truncate(200);
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// SSE: stream events, appending each to the response body in real time.
    fn send_sse(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        pre_script: String,
        cx: &mut Context<Self>,
    ) {
        // Pre-request script (same as HTTP).
        let mut script_logs: Vec<String> = Vec::new();
        let mut vars = vars;
        if !pre_script.trim().is_empty() {
            let pre_result = crate::scripting::run_pre_request(&pre_script, &vars);
            script_logs.extend(pre_result.logs);
            for effect in &pre_result.effects {
                if let crate::scripting::SideEffect::SetVariable { name, value, .. } = effect {
                    vars.insert(name.clone(), value.clone());
                }
            }
        }

        let prepared = match http::prepare(
            req.method,
            &req.url,
            &req.params,
            &req.headers,
            &req.path,
            &req.cookies,
            &req.auth,
            &req.body,
            &vars,
            30,
        ) {
            Ok(p) => p,
            Err(e) => {
                self.set_error_response(&id, format!("{e}"), cx);
                return;
            }
        };

        let client = cx.http_client();
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));

        cx.spawn(async move |this, cx| {
            // Seed an empty streaming response so the UI shows the "停止" state.
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            r.last_response = Some(Response {
                                status_text: "SSE 流式接收中…".into(),
                                streaming: true,
                                ..Default::default()
                            });
                        }
                    }
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
            });

            // Shared accumulator + last-seen length, polled by a timer task to
            // push live updates into the model (the stream future itself is Send
            // and can't capture the !Send GPUI entity handle).
            let acc = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let acc_for_stream = acc.clone();
            let acc_for_poll = acc.clone();
            let mut last_len = 0usize;

            let mut stream_fut = Box::pin(crate::http::sse::stream(
                client.clone(),
                prepared,
                30,
                stop.clone(),
                acc_for_stream,
            ));

            // Poll loop: interleave the stream with periodic UI-sync ticks.
            let result = loop {
                let timer = cx
                    .background_executor()
                    .timer(std::time::Duration::from_millis(120));
                match futures::future::select(&mut stream_fut, Box::pin(timer)).await {
                    futures::future::Either::Left((res, _)) => break res,
                    futures::future::Either::Right(((), _)) => {
                        // UI-sync tick: mirror new accumulator content into the model.
                        let snapshot = acc_for_poll.lock().map(|s| s.clone()).unwrap_or_default();
                        if snapshot.len() != last_len {
                            last_len = snapshot.len();
                            let _ = this.update(cx, |this, cx| {
                                this.state.update(cx, |s, cx| {
                                    if let Some(project) = s.active_project_mut() {
                                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                            if let Some(resp) = r.last_response.as_mut() {
                                                resp.body = snapshot.clone();
                                                resp.size = snapshot.len() as u64;
                                            }
                                        }
                                    }
                                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                                });
                            });
                        }
                    }
                }
            };

            let mut resp = match result {
                Ok(r) => r,
                Err(e) => crate::state::models::Response {
                    error: Some(format!("{e}")),
                    ..Default::default()
                },
            };
            // Append script logs.
            if !script_logs.is_empty() {
                resp.body.push_str("\n\n// ── 预执行脚本输出 ──\n");
                resp.body.push_str(&script_logs.join("\n"));
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            r.last_response = Some(resp.clone());
                            apply_autosave_example(r, &resp);
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// WebSocket: connect and drain incoming messages into the response body.
    fn send_websocket(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let url = crate::http::variable::substitute(&req.url, &vars);
        let url = crate::http::normalize_url_with_default(&url, "ws");
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));

        // Seed a streaming placeholder response.
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "WebSocket 连接中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        // Connect off the GPUI thread; the connection runs on its own tokio rt.
        let conn = crate::http::ws::connect(&url);

        cx.spawn(async move |this, cx| {
            let mut log = String::new();
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log.push_str("\n[已断开]\n");
                    break;
                }
                match conn.frames.recv().await {
                    Ok(crate::http::ws::WsFrame::Status(s)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("• {s}"));
                    }
                    Ok(crate::http::ws::WsFrame::Message(m)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&crate::http::ws::format_message("收到", &m));
                    }
                    Ok(crate::http::ws::WsFrame::Done(err)) => {
                        if let Some(e) = err {
                            log.push_str(&format!("\n• {e}"));
                        }
                        break;
                    }
                    Err(_) => break,
                }
                // Push the current log into the model on each frame.
                let snapshot = log.clone();
                let _ = this.update(cx, |this, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                if let Some(resp) = r.last_response.as_mut() {
                                    resp.body = snapshot.clone();
                                    resp.size = snapshot.len() as u64;
                                }
                            }
                        }
                        cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                    });
                });
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            if let Some(resp) = r.last_response.as_mut() {
                                resp.streaming = false;
                                resp.body = log.clone();
                                resp.size = log.len() as u64;
                            }
                            // Clone the response and apply autosave separately to avoid double-borrow.
                            if let Some(resp) = r.last_response.clone() {
                                apply_autosave_example(r, &resp);
                            }
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// TCP: connect and drain incoming data into the response body.
    fn send_tcp(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let url = crate::http::variable::substitute(&req.url, &vars);
        let url = crate::http::normalize_url_with_default(&url, "tcp");
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));

        // Seed a streaming placeholder response.
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "TCP 连接中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        let conn = crate::http::tcp::connect(&url);

        cx.spawn(async move |this, cx| {
            let mut log = String::new();
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log.push_str("\n[已断开]");
                    break;
                }
                match conn.frames.recv().await {
                    Ok(crate::http::tcp::TcpFrame::Status(s)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("• {s}"));
                    }
                    Ok(crate::http::tcp::TcpFrame::Data(d)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("← {d}"));
                    }
                    Ok(crate::http::tcp::TcpFrame::Done(err)) => {
                        if let Some(e) = err {
                            log.push_str(&format!("\n• {e}"));
                        } else {
                            log.push_str("\n• 连接已关闭");
                        }
                        break;
                    }
                    Err(_) => break,
                }
                let snapshot = log.clone();
                let _ = this.update(cx, |this, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                if let Some(resp) = r.last_response.as_mut() {
                                    resp.body = snapshot.clone();
                                    resp.size = snapshot.len() as u64;
                                }
                            }
                        }
                        cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                    });
                });
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            if let Some(resp) = r.last_response.as_mut() {
                                resp.streaming = false;
                                resp.body = log.clone();
                                resp.size = log.len() as u64;
                            }
                            if let Some(resp) = r.last_response.clone() {
                                apply_autosave_example(r, &resp);
                            }
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// gRPC: send via gRPC-Web (HTTP POST + framed protobuf/JSON).
    fn send_grpc(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let client = cx.http_client();
        let id_clone = id.clone();
        self.state.update(cx, |s, cx| {
            s.sending = Some(id.clone());
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "gRPC 调用中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        let body_json = req.body.raw.clone();
        let headers = req.headers.clone();
        let url = crate::http::normalize_url_with_default(&req.url, "grpc");

        cx.spawn(async move |this, cx| {
            let resp = crate::http::grpc::execute_grpc_web(
                client.as_ref(),
                &url,
                &headers,
                &body_json,
                &vars,
                30,
            )
            .await;

            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            r.last_response = Some(resp.clone());
                            apply_autosave_example(r, &resp);
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Socket.IO: connect via engine.io WebSocket transport.
    fn send_socketio(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        // Socket.IO runs over WebSocket with engine.io protocol.
        // Convert socket:// to ws:// and connect.
        let ws_url = req
            .url
            .replacen("socket://", "ws://", 1)
            .replacen("socketio://", "ws://", 1);
        let ws_url = crate::http::variable::substitute(&ws_url, &vars);
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, cx| {
            s.sending = Some(id.clone());
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "Socket.IO 连接中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        // Socket.IO uses engine.io: first message is "0" (open), then we
        // upgrade to WebSocket. For simplicity, connect directly via WS
        // and send/receive engine.io frames.
        let conn = crate::http::ws::connect(&format!(
            "{}/socket.io/?EIO=4&transport=websocket",
            ws_url.trim_end_matches('/')
        ));

        cx.spawn(async move |this, cx| {
            let mut log = String::new();
            // Send engine.io connect probe.
            let _ = conn.tx.send("40".into()).await; // Socket.IO CONNECT frame
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log.push_str("\n[已断开]");
                    break;
                }
                match conn.frames.recv().await {
                    Ok(crate::http::ws::WsFrame::Status(s)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("• {s}"));
                    }
                    Ok(crate::http::ws::WsFrame::Message(m)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("← {m}"));
                    }
                    Ok(crate::http::ws::WsFrame::Done(err)) => {
                        if let Some(e) = err {
                            log.push_str(&format!("\n• {e}"));
                        }
                        break;
                    }
                    Err(_) => break,
                }
                let snapshot = log.clone();
                let _ = this.update(cx, |this, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                if let Some(resp) = r.last_response.as_mut() {
                                    resp.body = snapshot.clone();
                                    resp.size = snapshot.len() as u64;
                                }
                            }
                        }
                        cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                    });
                });
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            if let Some(resp) = r.last_response.as_mut() {
                                resp.streaming = false;
                                resp.body = log.clone();
                                resp.size = log.len() as u64;
                            }
                            if let Some(resp) = r.last_response.clone() {
                                apply_autosave_example(r, &resp);
                            }
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// gRPC / Socket.IO placeholder (only for unimplemented edge cases).
    fn send_placeholder(&mut self, id: String, protocol: Protocol, cx: &mut Context<Self>) {
        let name = protocol.to_string();
        let placeholder_resp = Response {
            status_text: name.clone(),
            error: Some(format!("{name} 协议支持开发中，敬请期待。")),
            ..Default::default()
        };
        self.state.update(cx, |s, _cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(placeholder_resp.clone());
                    apply_autosave_example(r, &placeholder_resp);
                }
            }
        });
        // 必须在可变借用释放后再emit，避免双重借用panic。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::ResponseUpdated(id));
            });
        })
        .detach();
        cx.notify();
    }

    /// Helper: write a plain error response onto the request.
    fn set_error_response(&mut self, id: &str, msg: String, cx: &mut Context<Self>) {
        let err_resp = Response {
            error: Some(msg),
            ..Default::default()
        };
        self.state.update(cx, |s, _cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(id) {
                    r.last_response = Some(err_resp.clone());
                    apply_autosave_example(r, &err_resp);
                }
            }
        });
        // 必须在可变借用释放后再emit，避免双重借用panic。
        // spawn异步任务在当前同步流程结束后执行，借用已释放。
        let state = self.state.clone();
        let emit_id = id.to_string();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::ResponseUpdated(emit_id));
            });
        })
        .detach();
    }
}

/// Truncate a response body to HISTORY_BODY_MAX_CHARS at a char boundary,
/// returning (truncated_option, was_truncated). Returns (None, false) when empty.
fn truncate_history_body(body: &str) -> (Option<String>, bool) {
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
fn tab_label(
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

impl RequestPanel {
    fn render_active_tab(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        match self.active_tab {
            ReqTab::Headers => self
                .render_kv(&self.headers_rows.clone(), false, false, cx)
                .into_any_element(),
            ReqTab::Query => self
                .render_kv(&self.params_rows.clone(), true, false, cx)
                .into_any_element(),
            ReqTab::Path => {
                v_flex()
                    .size_full()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(
                                "路径变量：在 URL 中用 {{key}} 引用，此处填写的值会在发送时替换。",
                            ),
                    )
                    .child(self.render_kv(&self.path_rows.clone(), false, false, cx))
                    .into_any_element()
            }
            ReqTab::Cookie => self
                .render_kv(&self.cookie_rows.clone(), false, false, cx)
                .into_any_element(),
            ReqTab::Body => {
                let bt = self.body_type_select.clone();
                let lang = self.body_lang_select.clone();
                let body_type = self.body_type;
                let rows = self.body_rows.clone();
                v_flex()
                    .size_full()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(200.))
                                    .child(Select::new(&bt).small().placeholder("Body type")),
                            )
                            .when(body_type == BodyType::Raw, |this| {
                                this.child(
                                    div().w(px(140.)).child(Select::new(&lang).small()),
                                )
                            }),
                    )
                    .child(match body_type {
                        BodyType::None => div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.muted_foreground)
                            .child("该请求没有请求体。")
                            .into_any_element(),
                        BodyType::Raw => {
                            let lang_val = self
                                .body_lang_select
                                .read(cx)
                                .selected_value()
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            let is_json = lang_val == "json";
                            let visual = self.body_visual_mode && is_json;
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                // Toolbar: visual/code toggle (JSON only).
                                .when(is_json, |col| {
                                    col.child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Button::new("body-mode-code")
                                                    .ghost()
                                                    .xsmall()
                                                    .selected(!visual)
                                                    .label("代码编辑")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if this.body_visual_mode {
                                                            // Sync fields → raw before switching.
                                                            this.sync_visual_to_raw(cx);
                                                        }
                                                        this.body_visual_mode = false;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("body-mode-visual")
                                                    .ghost()
                                                    .xsmall()
                                                    .selected(visual)
                                                    .label("可视化编辑")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if !this.body_visual_mode {
                                                            // Parse raw → fields.
                                                            this.sync_raw_to_visual(cx);
                                                        }
                                                        this.body_visual_mode = true;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                })
                                .child(if visual {
                                    // Visual field table for the parsed JSON.
                                    self.render_raw_visual(cx).into_any_element()
                                } else {
                                    // Plain code editor + JSON validation error.
                                    let raw_text = self.body_editor.read(cx).text().to_string();
                                    let json_error = if is_json && !raw_text.trim().is_empty() {
                                        match serde_json::from_str::<serde_json::Value>(&raw_text) {
                                            Ok(_) => None,
                                            Err(e) => Some(e.to_string()),
                                        }
                                    } else {
                                        None
                                    };
                                    v_flex()
                                        .flex_1()
                                        .min_h_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .child(
                                                    Input::new(&self.body_editor)
                                                        .h_full()
                                                        .font_family(theme.mono_font_family.clone())
                                                        .text_size(theme.mono_font_size)
                                                        // Apply a red border when JSON is invalid.
                                                        .when_some(json_error.as_ref(), |inp, _| {
                                                            inp.border_color(theme.danger)
                                                        }),
                                                ),
                                        )
                                        // Show JSON parse error below the editor.
                                        .when_some(json_error, |col, err| {
                                            col.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.danger)
                                                    .child(format!("JSON 解析错误: {err}")),
                                            )
                                        })
                                        .into_any_element()
                                })
                                .into_any_element()
                        }
                        BodyType::FormData | BodyType::Urlencoded => self
                            .render_kv(
                                &rows,
                                true,
                                self.body_type == BodyType::FormData,
                                cx,
                            )
                            .into_any_element(),
                    })
                    .into_any_element()
            }
            ReqTab::Auth => self.render_auth_tab(cx).into_any_element(),
            ReqTab::PreRequest => v_flex()
                .size_full()
                .min_h_0()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            "预执行脚本（发送前运行，可设置变量）。apt.setVariable(k,v) · apt.getVariable(k) · apt.echo(...)",
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Input::new(&self.pre_script_editor)
                                .h_full()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size),
                        ),
                )
                .into_any_element(),
            ReqTab::PostRequest => v_flex()
                .size_full()
                .min_h_0()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            "后执行脚本（响应后运行，可读取响应并断言）。response.{status,body,json,headers,time} · apt.assert(cond,msg)",
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Input::new(&self.tests_editor)
                                .h_full()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size),
                        ),
                )
                .into_any_element(),
            ReqTab::Mock => {
                self.render_mock_tab(&theme, cx)
            }
            ReqTab::Curl => {
                let curl = self.generate_curl(cx);
                let curl_for_btn = curl.clone();
                let theme_c = theme.clone();
                v_flex()
                    .size_full()
                    .min_h_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("生成的 curl 命令，可直接复制到终端使用"),
                            )
                            .child(
                                Button::new("copy-curl")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label("复制")
                                    .tooltip("复制到剪贴板")
                                    .on_click(move |_ev, window, cx: &mut App| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(curl_for_btn.clone()));
                                        window.push_notification(
                                            gpui_component::notification::Notification::new()
                                                .title("已复制")
                                                .message("cURL 命令已复制到剪贴板。")
                                                .autohide(true),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("curl-code")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .bg(theme_c.muted)
                            .rounded_md()
                            .child(
                                div()
                                    .font_family(theme_c.mono_font_family.clone())
                                    .text_size(theme_c.mono_font_size)
                                    .text_color(theme_c.foreground)
                                    .whitespace_nowrap()                                    .child(curl),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    /// Render the Mock configuration tab.
    fn render_mock_tab(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme.clone();

        // 如果没有选中任何接口，显示全局引导页面
        if self.request_id.is_none() {
            return v_flex()
                .size_full()
                .p_6()
                .gap_4()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(24.))
                        .child("🧪")
                )
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Mock 服务")
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .text_center()
                        .max_w(px(400.))
                        .child("本地运行的接口模拟服务，支持自定义状态码、延迟、响应内容，帮助你快速开发和测试异常场景。")
                )
                .child(
                    v_flex()
                        .gap_2()
                        .p_4()
                        .w(px(400.))
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted)
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("快速开始")
                        )
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("1. 在左侧接口树中选择一个接口"))
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("2. 开启Mock开关，配置状态码、延迟、响应体"))
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("3. 直接在URL中使用 {{mock_server}}/api/path，系统会自动替换为当前Mock服务地址（本地/远程）"))
                        .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("💡 点击左侧活动栏的Mock图标，可以一键为所有接口生成规则、批量模拟异常场景"))
                )
                .into_any_element();
        }

        let enabled = self.mock_enabled;
        let ent = cx.entity();
        let ent_toggle = ent.clone();
        let ent_copy = ent.clone();
        let ent_del1 = ent.clone();
        let ent_add1 = ent.clone();
        let ent_del2 = ent.clone();
        let ent_add2 = ent.clone();
        let ent_del3 = ent.clone();
        let ent_add3 = ent.clone();
        let ent_template = ent.clone();
        let remote_cfg = crate::state::persistence::load_remote_config();
        let copy_url = crate::share::remote::mock_server_url(&remote_cfg);
        let is_remote = crate::share::remote::is_cloud_mode(&remote_cfg);

        v_flex()
            .size_full()
            .gap_3()
            .p_2()
            // 全局Mock引导卡片
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.primary.opacity(0.3))
                    .bg(theme.primary.opacity(0.05))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("💡 Mock 服务使用指南")
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(if is_remote {
                                format!("当前使用远程Mock服务：{}，开启规则后，你可以：", copy_url)
                            } else {
                                format!("本地Mock服务运行在 {}，开启规则后，你可以：", copy_url)
                            })
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .px_2()
                            .child(div().text_size(px(12.)).child("1. 将前端/测试用例的API基础地址替换为上面的服务地址"))
                            .child(div().text_size(px(12.)).child("2. 配置状态码、延迟、响应体模拟各种异常场景"))
                            .child(div().text_size(px(12.)).child("3. 在左侧Mock活动栏可以一键生成所有规则、批量切换异常场景"))
                    )
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .id("mock-enable-toggle")
                                    .w(px(36.))
                                    .h(px(20.))
                                    .rounded(px(10.))
                                    .flex()
                                    .items_center()
                                    .px(px(2.))
                                    .cursor_pointer()
                                    .when(enabled, |d| d.bg(theme.primary).justify_end())
                                    .when(!enabled, |d| d.bg(theme.muted_foreground.opacity(0.4)).justify_start())
                                    .child(
                                        div()
                                            .w(px(16.))
                                            .h(px(16.))
                                            .rounded_full()
                                            .bg(theme.background),
                                    )
                                    .on_click(move |_, _w, cx: &mut App| {
                                        let _ = ent_toggle.update(cx, |this, cx| {
                                            this.mock_enabled = !this.mock_enabled;
                                            this.commit_to_model(cx);
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("启用 Mock"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child("服务地址:"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_family(theme.mono_font_family.clone())
                                    .child(copy_url.clone()),
                            )
                            .child(
                                Button::new("copy-mock-url")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .tooltip("复制服务地址")
                                    .on_click(move |_, window, cx: &mut App| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_url.clone()));
                                        window.push_notification(
                                            gpui_component::notification::Notification::new()
                                                .title("已复制")
                                                .message("Mock服务地址已复制到剪贴板")
                                                .autohide(true),
                                            cx,
                                        );
                                    }),
                            ),
                    ),
            )
            .when(enabled, |c| {
                c.child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .w(px(120.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("响应状态码"),
                                        )
                                        .child(Input::new(&self.mock_status_input).small()),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .w(px(120.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("响应延迟(ms)"),
                                        )
                                        .child(Input::new(&self.mock_delay_input).small()),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .w(px(160.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("匹配方法"),
                                        )
                                        .child(Select::new(&self.mock_match_method_select).small().appearance(true)),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("路径匹配模式"),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .w(px(100.))
                                                        .child(Select::new(&self.mock_path_pattern_select).small().appearance(true)),
                                                )
                                                .child(Input::new(&self.mock_match_path_input).small().flex_1()),
                                        ),
                                ),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("响应头"),
                                )
                                .child(
                                    crate::ui::kv_table::KvTable::new(
                                        "mock-headers",
                                        self.mock_headers_rows.clone(),
                                        crate::ui::kv_table::KvHandlers {
                                            on_toggle: Arc::new(|_, _, _, _| {}),
                                            on_delete: Arc::new(move |i, _, cx| {
                                                let _ = ent_del1.update(cx, |this, cx| {
                                                    this.mock_headers_rows.remove(i);
                                                    this.commit_to_model(cx);
                                                    cx.notify();
                                                });
                                            }),
                                            on_add: Arc::new(move |window, cx| {
                                                let _ = ent_add1.update(cx, |this, cx| {
                                                    this.mock_headers_rows.push(KvRow::empty(window, cx));
                                                    this.commit_to_model(cx);
                                                    cx.notify();
                                                });
                                            }),
                                            on_type_change: Arc::new(|_, _, _, _| {}),
                                            on_required_toggle: Arc::new(|_, _, _, _| {}),
                                            on_file_pick: Arc::new(|_, _, _| {}),
                                        },
                                    )
                                ),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_1()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("响应体"),
                                )
                                .child(
                                    Input::new(&self.mock_body_editor)
                                        .flex_1()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_size(theme.mono_font_size),
                                ),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("mock-template-toggle")
                                                .w(px(28.))
                                                .h(px(16.))
                                                .rounded(px(8.))
                                                .flex()
                                                .items_center()
                                                .px(px(2.))
                                                .cursor_pointer()
                                                .when(self.mock_enable_templates, |d| d.bg(theme.primary).justify_end())
                                                .when(!self.mock_enable_templates, |d| d.bg(theme.muted_foreground.opacity(0.4)).justify_start())
                                                .child(
                                                    div()
                                                        .w(px(12.))
                                                        .h(px(12.))
                                                        .rounded_full()
                                                        .bg(theme.background),
                                                )
                                                .on_click(move |_, _w, cx: &mut App| {
                                                    let _ = ent_template.update(cx, |this, cx| {
                                                        this.mock_enable_templates = !this.mock_enable_templates;
                                                        this.commit_to_model(cx);
                                                        cx.notify();
                                                    });
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .child("启用动态模板"),
                                        ),
                                )
                                .when(self.mock_enable_templates, |c| {
                                    c.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child("可用变量: {{mock.request.path}} · {{mock.request.method}} · {{mock.request.query.参数名}} · {{mock.request.header.请求头名}}"),
                                    )
                                }),
                        )
                        .child(div().h(px(1.)).w_full().bg(theme.border))
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("高级匹配条件（可选）"),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("必须匹配的查询参数"),
                                        )
                                        .child(
                                            crate::ui::kv_table::KvTable::new(
                                                "mock-match-query",
                                                self.mock_match_query_rows.clone(),
                                                crate::ui::kv_table::KvHandlers {
                                                    on_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_delete: Arc::new(move |i, _, cx| {
                                                        let _ = ent_del2.update(cx, |this, cx| {
                                                            this.mock_match_query_rows.remove(i);
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_add: Arc::new(move |window, cx| {
                                                        let _ = ent_add2.update(cx, |this, cx| {
                                                            this.mock_match_query_rows.push(KvRow::empty(window, cx));
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_type_change: Arc::new(|_, _, _, _| {}),
                                                    on_required_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_file_pick: Arc::new(|_, _, _| {}),
                                                },
                                            )
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("必须匹配的请求头"),
                                        )
                                        .child(
                                            crate::ui::kv_table::KvTable::new(
                                                "mock-match-headers",
                                                self.mock_match_header_rows.clone(),
                                                crate::ui::kv_table::KvHandlers {
                                                    on_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_delete: Arc::new(move |i, _, cx| {
                                                        let _ = ent_del3.update(cx, |this, cx| {
                                                            this.mock_match_header_rows.remove(i);
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_add: Arc::new(move |window, cx| {
                                                        let _ = ent_add3.update(cx, |this, cx| {
                                                            this.mock_match_header_rows.push(KvRow::empty(window, cx));
                                                            this.commit_to_model(cx);
                                                            cx.notify();
                                                        });
                                                    }),
                                                    on_type_change: Arc::new(|_, _, _, _| {}),
                                                    on_required_toggle: Arc::new(|_, _, _, _| {}),
                                                    on_file_pick: Arc::new(|_, _, _| {}),
                                                },
                                            )
                                        ),
                                ),
                        ),
                )
            })
            .when(!enabled, |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("💡 开启后，本地Mock服务会按照上方配置返回模拟响应，支持自定义状态码、延迟、响应内容，方便你测试前端异常处理逻辑。"),
                )
            })
            .into_any_element()
    }

    /// Render the 认证 (Auth) tab: a type selector + per-type inputs.
    fn render_auth_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let auth_type = self.auth_type;
        // Field label + input row helper.
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(Input::new(input).small())
        };

        v_flex()
            .size_full()
            .gap_3()
            .p_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(180.))
                            .child(Select::new(&self.auth_type_select).small().appearance(true)),
                    )
                    .when(auth_type == AuthType::ApiKey, |row| {
                        row.child(
                            div().w(px(140.)).child(
                                Select::new(&self.auth_target_select)
                                    .small()
                                    .appearance(true),
                            ),
                        )
                    }),
            )
            .child(match auth_type {
                AuthType::None => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child("该请求不需要认证。")
                    .into_any_element(),
                AuthType::Bearer => h_flex()
                    .child(field("Token", &self.auth_token))
                    .into_any_element(),
                AuthType::Basic => h_flex()
                    .gap_2()
                    .child(field("用户名", &self.auth_username))
                    .child(field("密码", &self.auth_password))
                    .into_any_element(),
                AuthType::ApiKey => h_flex()
                    .gap_2()
                    .child(field("Key", &self.auth_key))
                    .child(field("Value", &self.auth_value))
                    .into_any_element(),
            })
    }

    /// Render the kv editor for params/headers rows.
    fn render_kv(
        &self,
        rows: &[KvRow],
        show_type: bool,
        allow_files: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let view_file = cx.entity();
        let handlers = crate::ui::kv_table::KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    this.toggle_kv(ix, val, cx);
                });
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| {
                    this.delete_kv(ix, cx);
                });
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.pending_kv_add = true;
                    cx.notify();
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| {
                    this.change_kv_type(ix, ft, cx);
                });
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    this.toggle_required(ix, val, cx);
                });
            }),
            on_file_pick: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_file.update(cx, |this, cx| {
                    this.pending_file_pick = Some(ix);
                    cx.notify();
                });
            }),
        };
        crate::ui::kv_table::KvTable::new("req-kv", rows.to_vec(), handlers)
            .show_type(show_type)
            .show_required(true)
            .show_description(true)
            .allow_files(allow_files)
            .into_any_element()
    }

    /// Render the kv editor for a folder section (params/headers/variables).
    fn render_folder_kv(
        &self,
        section: FolderKvSection,
        show_type: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rows = self.folder_rows(section).to_vec();
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let handlers = crate::ui::kv_table::KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    this.folder_toggle_kv(section, ix, val, cx);
                });
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| {
                    this.folder_delete_kv(section, ix, cx);
                });
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.folder_add_kv(section, cx);
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| {
                    this.folder_change_kv_type(section, ix, ft, cx);
                });
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    this.folder_toggle_required(section, ix, val, cx);
                });
            }),
            // File picking isn't meaningful at the folder level.
            on_file_pick: Arc::new(move |_, _, _| {}),
        };
        let id = match section {
            FolderKvSection::Params => "folder-kv-params",
            FolderKvSection::Headers => "folder-kv-headers",
            FolderKvSection::Variables => "folder-kv-vars",
        };
        crate::ui::kv_table::KvTable::new(id, rows, handlers)
            .show_type(show_type)
            .show_required(true)
            .into_any_element()
    }

    /// Render the full folder detail view (directory settings / params /
    /// interface list). Shown in the center column when a folder is selected.
    fn render_folder_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let folder_id = self.folder_id.clone().unwrap_or_default();
        let active_tab = self.folder_tab;

        // Collect the requests in this folder for the interface list.
        let iface_list = self
            .state
            .read(cx)
            .active_project()
            .map(|p| {
                p.find_folder(&folder_id)
                    .map(|(_, f)| {
                        // (id, name, method, protocol) for each request.
                        collect_iface_entries(f)
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let selected_request = self.state.read(cx).selected_request.clone();
        let view_for_open = cx.entity();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .child(
                // Header: folder icon + name + tab strip.
                v_flex()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .px_3()
                            .h(px(36.))
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(IconName::FolderOpen),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("目录"),
                            ),
                    )
                    .child(
                        h_flex()
                            .px_3()
                            .gap_1()
                            .child(folder_tab_label(
                                "目录设置",
                                active_tab == FolderTab::Settings,
                                FolderTab::Settings,
                                &theme,
                                cx,
                            ))
                            .child(folder_tab_label(
                                "目录参数",
                                active_tab == FolderTab::Params,
                                FolderTab::Params,
                                &theme,
                                cx,
                            ))
                            .child(folder_tab_label(
                                "接口列表",
                                active_tab == FolderTab::InterfaceList,
                                FolderTab::InterfaceList,
                                &theme,
                                cx,
                            )),
                    ),
            )
            .child(
                // Active tab content (scrollable).
                div()
                    .id("folder-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(match active_tab {
                        FolderTab::Settings => v_flex()
                            .gap_3()
                            .max_w(px(720.))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("目录名称"),
                                    )
                                    .child(
                                        Input::new(&self.folder_name).small(),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("前置 URL（Base URL）"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "设置后，该目录下所有接口的 URL 会自动拼接此前置地址。留空则不注入。",
                                        ),
                                    )
                                    .child(
                                        Input::new(&self.folder_base_url).small(),
                                    )
                                    .child(
                                        // Show the resolved (substituted) value when the stored
                                        // base_url contains {{var}} placeholders, so the user sees
                                        // the actual URL that will be used.
                                        {
                                            let raw = self.folder_base_url.read(cx).value().to_string();
                                            if raw.contains("{{") {
                                                // Build vars for substitution: globals + active env.
                                                let st = self.state.read(cx);
                                                let mut vars: BTreeMap<String, String> = BTreeMap::new();
                                                if let Some(p) = st.active_project() {
                                                    for kv in &p.global_variables {
                                                        if kv.enabled && !kv.key.trim().is_empty() {
                                                            vars.insert(kv.key.clone(), kv.value.clone());
                                                        }
                                                    }
                                                    for kv in p.active_env_variables() {
                                                        if kv.enabled && !kv.key.trim().is_empty() {
                                                            vars.insert(kv.key.clone(), kv.value.clone());
                                                        }
                                                    }
                                                }
                                                let resolved = crate::http::variable::substitute(&raw, &vars);
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(format!("当前生效值：{}", if resolved.trim().is_empty() { "（未解析）".to_string() } else { resolved }))
                                            } else {
                                                div()
                                            }
                                        },
                                    )
                                    .child(
                                        // Dropdown to select base URL from environment variables.
                                        {
                                            let active_env = self.state.read(cx).active_project()
                                                .and_then(|p| p.active_environment.as_ref())
                                                .and_then(|eid| {
                                                    self.state.read(cx).active_project()
                                                        .and_then(|p| p.environments.iter().find(|e| &e.id == eid))
                                                });
                                            let env_vars: Vec<(String, String)> = active_env
                                                .map(|env| {
                                                    env.variables.iter()
                                                        .filter(|kv| kv.enabled && (kv.value.starts_with("http://") || kv.value.starts_with("https://")))
                                                        .map(|kv| (kv.key.clone(), kv.value.clone()))
                                                        .collect()
                                                })
                                                .unwrap_or_default();

                                            if env_vars.is_empty() {
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child("（当前环境没有 URL 变量）")
                                                    .into_any_element()
                                            } else {
                                                let open = self.folder_baseurl_open;
                                                let folder_id_upd = self.folder_id.clone();
                                                let state_upd = self.state.clone();
                                                let fb_input = self.folder_base_url.clone();
                                                let theme_c = theme.clone();

                                                let folder_id = folder_id_upd.clone();
                                                let state = state_upd.clone();
                                                let fb_input_clone = fb_input.clone();
                                                let panel_entity = cx.entity();

                                                Popover::new("folder-baseurl-popover")
                                                    .anchor(gpui::Anchor::BottomLeft)
                                                    .open(open)
                                                    .on_open_change(cx.listener(|this, open, _, cx| {
                                                        this.folder_baseurl_open = *open;
                                                        cx.notify();
                                                    }))
                                                    .trigger(
                                                        Button::new("folder-baseurl-trigger")
                                                            .ghost()
                                                            .small()
                                                            .icon(IconName::ChevronDown)
                                                            .label("选择环境变量"),
                                                    )
                                                    .p(px(4.))
                                                    .child(
                                                        v_flex()
                                                            .w(px(300.))
                                                            .gap(px(2.))
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_weight(FontWeight::SEMIBOLD)
                                                                    .text_color(theme_c.muted_foreground)
                                                                    .px_2()
                                                                    .py_1()
                                                                    .child("从环境变量选择"),
                                                            )
                                                            .children(env_vars.iter().enumerate().map(|(i, (k, v))| {
                                                                let val_display = v.trim_end_matches('/').to_string();
                                                                let key = k.clone();
                                                                // Store the {{key}} placeholder so the stored
                                                                // value stays in sync when the env var changes;
                                                                // the literal value is only shown in the
                                                                // dropdown as a preview.
                                                                let placeholder = format!("{{{{{}}}}}", key);
                                                                let fid = folder_id.clone();
                                                                let st = state.clone();
                                                                let tc = theme_c.clone();
                                                                let fb_in = fb_input_clone.clone();
                                                                let panel = panel_entity.clone();

                                                                div()
                                                                    .id(("baseurl-opt", i))
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
                                                                                    .font_weight(FontWeight::SEMIBOLD)
                                                                                    .child(format!("{{{{{}}}}}", key)),
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .text_xs()
                                                                                    .text_color(tc.muted_foreground)
                                                                                    .child(val_display),
                                                                            ),
                                                                    )
                                                                    .on_mouse_down(MouseButton::Left, move |_ev, window, cx: &mut App| {
                                                                        let fid_clone = fid.clone().unwrap_or_default();
                                                                        let val_set = placeholder.clone();
                                                                        let _ = st.update(cx, |s, cx| {
                                                                            if let Some(p) = s.active_project_mut() {
                                                                                set_folder_base_url(&mut p.folders, &fid_clone, Some(val_set.clone()));
                                                                            }
                                                                            s.notify_edited(cx);
                                                                        });
                                                                        let _ = fb_in.update(cx, |input, cx| {
                                                                            input.set_value(&val_set, window, cx);
                                                                        });
                                                                        // Close the popover by setting folder_baseurl_open = false.
                                                                        let _ = panel.update(cx, |this, cx| {
                                                                            this.folder_baseurl_open = false;
                                                                            cx.notify();
                                                                        });
                                                                        window.refresh();
                                                                    })
                                                            })),
                                                    )
                                                    .into_any_element()
                                            }
                                        },
                                    )
                                    .child(
                                        {
                                            let state = self.state.clone();
                                            let folder_id = self.folder_id.clone();
                                            Button::new("clear-base-url")
                                                .ghost()
                                                .xsmall()
                                                .label("清除")
                                                .on_click(move |_, _, cx: &mut App| {
                                                    let fid = folder_id.clone().unwrap_or_default();
                                                    let _ = state.update(cx, |s, cx| {
                                                        if let Some(p) = s.active_project_mut() {
                                                            set_folder_base_url(&mut p.folders, &fid, None);
                                                        }
                                                        s.notify_edited(cx);
                                                    });
                                                })
                                        },
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("目录描述"),
                                            ),
                                    )
                                    .child(
                                        Input::new(&self.folder_desc)
                                            .small(),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("目录变量"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "该目录下的所有接口共享这些变量。同名变量时，接口级变量覆盖目录级变量。",
                                        ),
                                    )
                                    .child(self.render_folder_kv(FolderKvSection::Variables, true, cx)),
                            )
                            .into_any_element(),
                        FolderTab::Params => v_flex()
                            .gap_4()
                            .max_w(px(960.))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("公共 Query 参数"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "该目录下接口自动合并这些 query 参数（接口级同名参数覆盖）。",
                                        ),
                                    )
                                    .child(self.render_folder_kv(FolderKvSection::Params, true, cx)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("公共 Header"),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            "该目录下接口自动合并这些请求头（接口级同名头覆盖）。",
                                        ),
                                    )
                                    .child(self.render_folder_kv(FolderKvSection::Headers, false, cx)),
                            )
                            .into_any_element(),
                        FolderTab::InterfaceList => {
                            let theme2 = theme.clone();
                            let selected_req = selected_request.clone();
                            let is_empty = iface_list.is_empty();
                            let total = iface_list.len();
                            // The visible columns (cloned so the closure can move
                            // a snapshot; the source of truth is self.iface_columns).
                            let columns = self.iface_columns.clone();
                            // Pagination: a fixed page size. Clamp the current
                            // page to the valid range (handles deletions).
                            let page_size = 20usize;
                            let last_page = if total == 0 {
                                0
                            } else {
                                (total - 1) / page_size
                            };
                            let page = self.iface_page.min(last_page);
                            let start = page * page_size;
                            let end = (start + page_size).min(total);
                            let page_entries: Vec<IfaceEntry> =
                                iface_list[start..end].to_vec();
                            let has_prev = page > 0;
                            let has_next = page < last_page;
                            let view_paging = view_for_open.clone();
                            let view_cols = view_for_open.clone();
                            let columns_popover_open = self.iface_columns_popover_open;

                            // Build a single table row (header or data) from a
                            // list of (column, text, color_override, is_header)
                            // cell specs.
                            let build_row = |cells: Vec<(IfaceColumn, String, Option<Hsla>, bool)>,
                                             row_id: String,
                                             is_sel: bool,
                                             theme2: &gpui_component::Theme| {
                                let mut row = h_flex()
                                    .id(row_id.clone())
                                    .px_2()
                                    .py(px(4.))
                                    .gap_2()
                                    .rounded(theme2.radius)
                                    .items_center();
                                if is_sel {
                                    row = row.bg(theme2.primary.opacity(0.18));
                                }
                                for (col, text, color_override, is_header) in cells {
                                    let weight = col.width_weight();
                                    let is_method = col == IfaceColumn::Method;
                                    let default_color = if is_header {
                                        theme2.muted_foreground
                                    } else {
                                        theme2.foreground
                                    };
                                    let text_color = color_override.unwrap_or(default_color);
                                    let cell = div()
                                        .flex_shrink_0()
                                        .w(px(96. * weight))
                                        .min_w(px(48.))
                                        .overflow_hidden()
                                        .text_color(text_color)
                                        .text_size(if is_header || is_method {
                                            px(11.)
                                        } else {
                                            px(13.)
                                        })
                                        .font_weight(if is_header || is_method {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .child(text);
                                    row = row.child(cell);
                                }
                                row
                            };

                            v_flex()
                                .gap_1()
                                .max_w(px(1100.))
                                // Toolbar: count on the left, column-picker on the right.
                                .child(
                                    h_flex()
                                        .mb_1()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(format!("共 {} 个接口", total)),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            Popover::new("iface-columns")
                                                .anchor(gpui::Anchor::BottomRight)
                                                .open(columns_popover_open)
                                                .on_open_change(cx.listener(
                                                    |this, open, _, cx| {
                                                        this.iface_columns_popover_open = *open;
                                                        cx.notify();
                                                    },
                                                ))
                                                .trigger(
                                                    Button::new("iface-cols-btn")
                                                        .ghost()
                                                        .xsmall()
                                                        .icon(IconName::Settings2)
                                                        .label("列设置"),
                                                )
                                                .p(px(6.))
                                                .child({
                                                    // Column picker: a checkbox list
                                                    // of every available column.
                                                    let theme_p = theme2.clone();
                                                    let view = view_cols.clone();
                                                    let active: Vec<IfaceColumn> =
                                                        self.iface_columns.clone();
                                                    v_flex()
                                                        .w(px(180.))
                                                        .gap(px(2.))
                                                        .child(
                                                            div()
                                                                .px_2()
                                                                .py_1()
                                                                .text_xs()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(theme_p.foreground)
                                                                .child("显示的列"),
                                                        )
                                                        .children(IfaceColumn::all().iter().map(
                                                            move |col| {
                                                                let checked = active.contains(col);
                                                                let col = *col;
                                                                let view = view.clone();
                                                                h_flex()
                                                                    .id(format!(
                                                                        "col-toggle-{}",
                                                                        col.as_key()
                                                                    ))
                                                                    .px_2()
                                                                    .py(px(3.))
                                                                    .gap_2()
                                                                    .items_center()
                                                                    .rounded(px(4.))
                                                                    .hover(|d| {
                                                                        d.bg(theme_p
                                                                            .accent
                                                                            .opacity(0.3))
                                                                    })
                                                                    .child(
                                                                        Checkbox::new(
                                                                            "col-cb-".to_string() + col.as_key(),
                                                                        )
                                                                        .checked(checked)
                                                                        .on_click(move |val: &bool, _window, cx: &mut App| {
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.toggle_iface_column(col, *val, cx);
                                                                            });
                                                                        }),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_sm()
                                                                            .text_color(theme_p.foreground)
                                                                            .child(col.label()),
                                                                    )
                                                            },
                                                        ))
                                                }),
                                        ),
                                )
                                .when(!columns.is_empty(), |col| {
                                    // Table header row.
                                    let header_cells: Vec<(IfaceColumn, String, Option<Hsla>, bool)> =
                                        columns.iter().map(|c| (*c, c.label().to_string(), None, true)).collect();
                                    col.child(
                                        build_row(
                                            header_cells,
                                            "iface-header".to_string(),
                                            false,
                                            &theme2,
                                        )
                                        .border_b_1()
                                        .border_color(theme2.border),
                                    )
                                })
                                .when(is_empty, |col| {
                                    col.child(
                                        div()
                                            .py_4()
                                            .text_color(theme.muted_foreground)
                                            .text_sm()
                                            .child("该目录下暂无接口，点击目录旁的 + 新建。"),
                                    )
                                })
                                .children(page_entries.iter().enumerate().map(|(i, e)| {
                                    let is_sel = selected_req.as_deref() == Some(&e.id);
                                    // Pre-compute the method badge color for this row.
                                    let method_color = Some(crate::ui::method_colors::badge_color(e.method, cx));
                                    let method_str = e.method.as_str().to_string();
                                    let cells: Vec<(IfaceColumn, String, Option<Hsla>, bool)> = columns
                                        .iter()
                                        .map(|c| {
                                            let text = if *c == IfaceColumn::Method {
                                                method_str.clone()
                                            } else {
                                                e.cell_text(*c)
                                            };
                                            let color = if *c == IfaceColumn::Method {
                                                method_color
                                            } else {
                                                None
                                            };
                                            (*c, text, color, false)
                                        })
                                        .collect();
                                    let view = view_for_open.clone();
                                    let id_for_open = e.id.clone();
                                    let theme_r = theme2.clone();
                                    let mut row = build_row(
                                        cells,
                                        format!("iface-row-{i}"),
                                        is_sel,
                                        &theme_r,
                                    );
                                    row = row
                                        .hover(|d| d.bg(theme_r.accent.opacity(0.4)))
                                        .on_click(move |_, _window, cx: &mut App| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.state.update(cx, |s, cx| {
                                                    s.open_or_focus_tab(&id_for_open, cx);
                                                });
                                            });
                                        });
                                    row
                                }))
                                // Pagination footer: prev / "page x of n" / next.
                                .when(!is_empty && last_page > 0, |col| {
                                    let view_prev = view_paging.clone();
                                    let view_next = view_paging.clone();
                                    col.child(
                                        h_flex()
                                            .mt_2()
                                            .items_center()
                                            .justify_center()
                                            .gap_2()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(
                                                Button::new("iface-prev")
                                                    .ghost()
                                                    .xsmall()
                                                    .icon(IconName::ChevronLeft)
                                                    .disabled(!has_prev)
                                                    .on_click(move |_, _window, cx: &mut App| {
                                                        let _ = view_prev.update(cx, |this, cx| {
                                                            if this.iface_page > 0 {
                                                                this.iface_page -= 1;
                                                                cx.notify();
                                                            }
                                                        });
                                                    }),
                                            )
                                            .child(div().child(format!(
                                                "{}/{} · 共 {} 个",
                                                page + 1,
                                                last_page + 1,
                                                total
                                            )))
                                            .child(
                                                Button::new("iface-next")
                                                    .ghost()
                                                    .xsmall()
                                                    .icon(IconName::ChevronRight)
                                                    .disabled(!has_next)
                                                    .on_click(move |_, _window, cx: &mut App| {
                                                        let _ = view_next.update(cx, |this, cx| {
                                                            this.iface_page += 1;
                                                            cx.notify();
                                                        });
                                                    }),
                                            ),
                                    )
                                })
                                .into_any_element()
                        }
                    }),
            )
    }
}

/// A flattened request entry in a folder's interface list. Carries all the
/// column data (name/method/path/audit metadata) so the table renders any
/// combination of columns.
#[derive(Clone)]
struct IfaceEntry {
    id: String,
    name: String,
    method: RequestMethod,
    protocol: Protocol,
    url: String,
    /// Owning folder name (immediate parent).
    folder_name: String,
    created_by: String,
    created_at: String,
    updated_by: String,
    updated_at: String,
    status: String,
    tags: Vec<String>,
}

impl IfaceEntry {
    /// Render the cell text for a column.
    fn cell_text(&self, col: IfaceColumn) -> String {
        match col {
            IfaceColumn::Name => self.name.clone(),
            IfaceColumn::Method => self.method.as_str().to_string(),
            IfaceColumn::Path => self.url.clone(),
            IfaceColumn::Folder => self.folder_name.clone(),
            IfaceColumn::CreatedBy => self.created_by.clone(),
            IfaceColumn::CreatedAt => self.created_at.clone(),
            IfaceColumn::UpdatedBy => self.updated_by.clone(),
            IfaceColumn::UpdatedAt => self.updated_at.clone(),
            IfaceColumn::Status => self.status.clone(),
            IfaceColumn::Tags => self.tags.join(", "),
        }
    }
}

/// Collect an entry for every request that is a **direct child** of the
/// folder (its own `requests` only — not those of nested sub-folders). The
/// 接口目录 column shows this folder's name, since all listed interfaces live
/// in it directly.
fn collect_iface_entries(folder: &Folder) -> Vec<IfaceEntry> {
    folder
        .requests
        .iter()
        .map(|req| IfaceEntry {
            id: req.id.clone(),
            name: req.name.clone(),
            method: req.method,
            protocol: req.protocol,
            url: req.url.clone(),
            folder_name: folder.name.clone(),
            created_by: req.created_by.clone(),
            created_at: req.created_at.clone(),
            updated_by: req.updated_by.clone(),
            updated_at: req.updated_at.clone(),
            status: req.status.clone(),
            tags: req.tags.clone(),
        })
        .collect()
}

/// Build a clickable folder-tab label.
fn folder_tab_label(
    label: &'static str,
    is_active: bool,
    tab: FolderTab,
    theme: &gpui_component::Theme,
    cx: &mut Context<RequestPanel>,
) -> impl IntoElement {
    div()
        .id(label.to_string())
        .px_3()
        .py_2()
        .text_sm()
        .text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .when(is_active, |this| {
            this.border_b_2().border_color(theme.primary)
        })
        .hover(|this| this.bg(theme.accent.opacity(0.5)))
        .child(label.to_string())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.folder_tab = tab;
            cx.notify();
        }))
}

impl Focusable for RequestPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for RequestPanel {}

impl RequestPanel {
    /// Return the active kv row list mutably, based on the current tab.
    fn active_rows_mut(&mut self) -> Option<&mut Vec<KvRow>> {
        match self.active_tab {
            ReqTab::Query => Some(&mut self.params_rows),
            ReqTab::Headers => Some(&mut self.headers_rows),
            ReqTab::Path => Some(&mut self.path_rows),
            ReqTab::Cookie => Some(&mut self.cookie_rows),
            ReqTab::Body => match self.body_type {
                BodyType::FormData | BodyType::Urlencoded => Some(&mut self.body_rows),
                _ => None,
            },
            _ => None,
        }
    }

    /// Borrow the kv rows for a folder section.
    fn folder_rows(&self, section: FolderKvSection) -> &[KvRow] {
        match section {
            FolderKvSection::Params => &self.folder_param_rows,
            FolderKvSection::Headers => &self.folder_header_rows,
            FolderKvSection::Variables => &self.folder_var_rows,
        }
    }

    /// Mutably borrow the kv rows for a folder section.
    fn folder_rows_mut(&mut self, section: FolderKvSection) -> &mut Vec<KvRow> {
        match section {
            FolderKvSection::Params => &mut self.folder_param_rows,
            FolderKvSection::Headers => &mut self.folder_header_rows,
            FolderKvSection::Variables => &mut self.folder_var_rows,
        }
    }

    // --- folder kv row handlers (parallel to the request kv handlers) ---

    pub fn folder_toggle_kv(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        val: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.folder_rows_mut(section).get_mut(ix) {
            row.enabled = val;
        }
        self.commit_folder(cx);
        cx.notify();
    }

    pub fn folder_delete_kv(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        cx: &mut Context<Self>,
    ) {
        let rows = self.folder_rows_mut(section);
        if ix < rows.len() {
            rows.remove(ix);
        }
        self.commit_folder(cx);
        cx.notify();
    }

    pub fn folder_add_kv(&mut self, section: FolderKvSection, cx: &mut Context<Self>) {
        // Defer the actual row creation to render where a Window is available.
        self.pending_folder_kv_add = Some(section);
        cx.notify();
    }

    pub fn folder_change_kv_type(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        ft: FieldType,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.folder_rows_mut(section).get_mut(ix) {
            row.field_type = ft;
        }
        self.commit_folder(cx);
        cx.notify();
    }

    pub fn folder_toggle_required(
        &mut self,
        section: FolderKvSection,
        ix: usize,
        req: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.folder_rows_mut(section).get_mut(ix) {
            row.required = req;
        }
        self.commit_folder(cx);
        cx.notify();
    }

    /// Process pending folder kv additions now that a Window is available.
    pub fn reconcile_folder_kv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(section) = self.pending_folder_kv_add.take() {
            self.folder_rows_mut(section).push(KvRow::empty(window, cx));
            cx.notify();
        }
    }

    /// Toggle a column's visibility in the interface list and persist the
    /// customized set. Always keeps at least one column.
    pub fn toggle_iface_column(&mut self, col: IfaceColumn, on: bool, cx: &mut Context<Self>) {
        let idx = self.iface_columns.iter().position(|c| *c == col);
        match (on, idx) {
            (true, None) => {
                // Add it (after the same relative order as IfaceColumn::all()).
                self.iface_columns.push(col);
                self.iface_columns
                    .sort_by_key(|c| IfaceColumn::all().iter().position(|x| x == c).unwrap_or(0));
            }
            (false, Some(i)) => {
                // Never remove the last column.
                if self.iface_columns.len() > 1 {
                    self.iface_columns.remove(i);
                }
            }
            _ => {}
        }
        crate::state::persistence::save_iface_columns(&self.iface_columns);
        cx.notify();
    }

    pub fn toggle_kv(&mut self, ix: usize, val: bool, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if let Some(row) = rows.get_mut(ix) {
                row.enabled = val;
            }
        }
        cx.notify();
    }

    pub fn delete_kv(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if ix < rows.len() {
                rows.remove(ix);
            }
        }
        cx.notify();
    }

    /// Process a pending kv add now that a Window is available (called from render).
    pub fn reconcile_pending_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_kv_add {
            self.pending_kv_add = false;
            if let Some(rows) = self.active_rows_mut() {
                rows.push(KvRow::empty(window, cx));
            }
        }
        // Visual-mode add row (needs a Window to create InputState entities).
        if self.pending_visual_add {
            self.pending_visual_add = false;
            self.raw_visual_rows.push(KvRow::empty(window, cx));
        }
        // File picker for form-data file fields (needs a Window).
        if let Some(ix) = self.pending_file_pick.take() {
            let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择文件".into()),
            });
            let entity = cx.entity();
            cx.spawn(async move |_this, cx| {
                if let Ok(Ok(Some(paths))) = prompt.await {
                    if let Some(path) = paths.first() {
                        let path_str = path.to_string_lossy().to_string();
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(rows) = this.active_rows_mut() {
                                if let Some(row) = rows.get_mut(ix) {
                                    row.file_path = Some(path_str);
                                    row.field_type = FieldType::File;
                                }
                            }
                            this.commit_to_model(cx);
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
        }
    }

    /// Change the field type of a kv row (form-data/query).
    pub fn change_kv_type(&mut self, ix: usize, ft: FieldType, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if let Some(row) = rows.get_mut(ix) {
                row.field_type = ft;
                // Switching away from File clears the file path.
                if ft != FieldType::File {
                    row.file_path = None;
                }
            }
        }
        self.commit_to_model(cx);
        cx.notify();
    }

    /// Toggle the "required" flag of a kv row.
    pub fn toggle_required(&mut self, ix: usize, val: bool, cx: &mut Context<Self>) {
        if let Some(rows) = self.active_rows_mut() {
            if let Some(row) = rows.get_mut(ix) {
                row.required = val;
            }
        }
        self.commit_to_model(cx);
        cx.notify();
    }

    // ---- Raw JSON visual editing ----

    /// Parse the current Raw JSON body into `raw_parameter` fields. Called when
    /// switching to visual mode. Triggers a reload so `raw_visual_rows` are
    /// rebuilt from the freshly parsed model.
    pub fn sync_raw_to_visual(&mut self, cx: &mut Context<Self>) {
        let id = self.request_id.clone();
        if let Some(req) = self.state.read(cx).active_project().and_then(|p| {
            id.as_ref()
                .and_then(|id| p.find_request(id).map(|(_, r)| r.clone()))
        }) {
            let mut body = req.body.clone();
            body.sync_raw_to_fields();
            self.state.update(cx, |s, cx| {
                if let Some(p) = s.active_project_mut() {
                    if let Some((_, r)) = p.find_request_mut(&id.as_ref().unwrap()) {
                        r.body.raw_parameter = body.raw_parameter.clone();
                    }
                }
                s.dirty = true;
                s.schedule_save(cx);
            });
            // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免双重借用 panic。
            // spawn异步任务在当前同步流程结束后执行，借用已释放。
            let state = self.state.clone();
            cx.spawn(async move |_, cx| {
                let _ = state.update(cx, |s, cx| {
                    let _ = s;
                    cx.emit(AppEvent::RequestEdited);
                });
            })
            .detach();
        }
        // Rebuild the persistent row entities on next render.
        self.pending_reload = true;
    }

    /// Serialize the `raw_parameter` fields back into the Raw JSON body. Called
    /// when switching back to code mode. First commits the current row inputs
    /// to the model, then serializes.
    pub fn sync_visual_to_raw(&mut self, cx: &mut Context<Self>) {
        // Commit current row inputs → model, then serialize to raw.
        self.commit_visual_to_model(cx);
        // Reload picks up the updated `body.raw` into the code editor.
        self.pending_reload = true;
    }

    /// Render the Raw body as a visual field table (KvTable over
    /// `raw_visual_rows`). Uses persistent InputState entities so typing works
    /// normally — the rows are built once at reload and only re-synced from the
    /// model when the visual mode is entered.
    fn render_raw_visual(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view_toggle = cx.entity();
        let view_delete = cx.entity();
        let view_add = cx.entity();
        let view_type = cx.entity();
        let view_req = cx.entity();
        let handlers = crate::ui::kv_table::KvHandlers {
            on_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_toggle.update(cx, |this, cx| {
                    this.update_raw_field(ix, |kv| kv.enabled = val, cx);
                });
            }),
            on_delete: Arc::new(move |ix, _window, cx: &mut App| {
                let _ = view_delete.update(cx, |this, cx| {
                    this.remove_raw_field(ix, cx);
                });
            }),
            on_add: Arc::new(move |_window, cx: &mut App| {
                let _ = view_add.update(cx, |this, cx| {
                    this.add_raw_field(cx);
                });
            }),
            on_type_change: Arc::new(move |ix, ft, _window, cx: &mut App| {
                let _ = view_type.update(cx, |this, cx| {
                    this.update_raw_field(ix, |kv| kv.field_type = ft, cx);
                });
            }),
            on_required_toggle: Arc::new(move |ix, val, _window, cx: &mut App| {
                let _ = view_req.update(cx, |this, cx| {
                    this.update_raw_field(ix, |kv| kv.required = val, cx);
                });
            }),
            on_file_pick: Arc::new(|_, _, _| {}),
        };

        v_flex().size_full().min_h_0().gap_1().child(
            crate::ui::kv_table::KvTable::new("raw-visual", self.raw_visual_rows.clone(), handlers)
                .show_type(true)
                .show_required(true)
                .show_description(true),
        )
    }

    /// Sync the visual-mode row inputs back to the model's `raw_parameter`,
    /// then serialize to `raw` JSON. Called after any visual-mode edit.
    fn commit_visual_to_model(&mut self, cx: &mut Context<Self>) {
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        let fields = kv_table::pairs_from_rows(&self.raw_visual_rows, cx);
        self.state.update(cx, |s, cx| {
            if let Some(p) = s.active_project_mut() {
                if let Some((_, r)) = p.find_request_mut(&id) {
                    r.body.raw_parameter = fields;
                    r.body.sync_fields_to_raw();
                }
            }
            s.dirty = true;
            s.schedule_save(cx);
        });
        // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免双重借用 panic。
        // spawn异步任务在当前同步流程结束后执行，借用已释放。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::RequestEdited);
            });
        })
        .detach();
    }

    /// Update a single visual-mode row's plain fields (enabled/type/required),
    /// then commit everything to the model. Key/value/description are edited
    /// in-place via the persistent InputState entities and picked up by the
    /// commit.
    fn update_raw_field(
        &mut self,
        ix: usize,
        f: impl Fn(&mut kv_table::KvRow),
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.raw_visual_rows.get_mut(ix) {
            f(row);
        }
        self.commit_visual_to_model(cx);
        cx.notify();
    }

    fn remove_raw_field(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.raw_visual_rows.len() {
            self.raw_visual_rows.remove(ix);
        }
        self.commit_visual_to_model(cx);
        cx.notify();
    }

    fn add_raw_field(&mut self, cx: &mut Context<Self>) {
        // Flag for rebuild on next render (KvRow needs a Window to create its
        // InputState entities; the handler runs on &mut App without one).
        self.pending_visual_add = true;
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        self.state.update(cx, |s, cx| {
            if let Some(p) = s.active_project_mut() {
                if let Some((_, r)) = p.find_request_mut(&id) {
                    r.body.raw_parameter.push(KeyValue::new("new_field", ""));
                    r.body.sync_fields_to_raw();
                }
            }
            s.dirty = true;
            s.schedule_save(cx);
        });
        // 注意：必须在可变借用 AppState 释放后再 emit 事件，避免双重借用 panic。
        // spawn异步任务在当前同步流程结束后执行，借用已释放。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::RequestEdited);
            });
        })
        .detach();
        cx.notify();
    }
}

// Silence unused import warnings for items used only conditionally.
#[allow(dead_code)]
fn _unused(_a: Arc<()>, _t: Task<()>) {}

/// Recursively set `base_url` on the folder with the given id.
fn set_folder_base_url(
    folders: &mut [crate::state::models::Folder],
    id: &str,
    url: Option<String>,
) -> bool {
    for f in folders.iter_mut() {
        if f.id == id {
            f.base_url = url.clone();
            return true;
        }
        if set_folder_base_url(&mut f.folders, id, url.clone()) {
            return true;
        }
    }
    false
}

/// Resolve the effective base URL for a request by walking up the folder
/// chain. Returns the first non-empty base_url found in the folder hierarchy.
///
/// The stored `base_url` may contain `{{var}}` placeholders (when it was set
/// by picking an environment variable from the dropdown). These are
/// substituted against the active environment + global variables so the
/// resolved value always reflects the current environment.
fn resolve_folder_base_url(
    project: &crate::state::models::Project,
    chain: &[String],
) -> Option<String> {
    // Build a variable map for substitution: globals + active env + folder
    // variables along the chain (so a folder's own variables can also be
    // referenced in a base_url set on that same folder or an ancestor).
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for kv in &project.global_variables {
        if kv.enabled && !kv.key.trim().is_empty() {
            vars.insert(kv.key.clone(), kv.value.clone());
        }
    }
    for kv in project.active_env_variables() {
        if kv.enabled && !kv.key.trim().is_empty() {
            vars.insert(kv.key.clone(), kv.value.clone());
        }
    }
    // Walk the chain from the deepest folder up to find a base_url.
    for depth in (0..chain.len()).rev() {
        let id = &chain[depth];
        if let Some((_, folder)) = project.find_folder(id) {
            if let Some(url) = &folder.base_url {
                if !url.trim().is_empty() {
                    // Substitute any {{var}} placeholders against the vars
                    // collected so far (incl. this folder's own variables).
                    for kv in &folder.variables {
                        if kv.enabled && !kv.key.trim().is_empty() {
                            vars.insert(kv.key.clone(), kv.value.clone());
                        }
                    }
                    let resolved = crate::http::variable::substitute(url, &vars);
                    if !resolved.trim().is_empty() {
                        return Some(resolved.trim_end_matches('/').to_string());
                    }
                }
            }
        }
    }
    None
}

/// Apply autosave logic: if the global autosave setting is enabled, save the
/// response as a success or failure example on the request.
///
/// - Success responses (2xx/3xx, no error) → stored as the single `success_example` (overwrite).
/// - Failure responses (4xx/5xx/network error) → appended to `fail_examples`, deduplicated by
///   (status + body) so identical responses don't accumulate duplicates.
fn apply_autosave_example(req: &mut ApiRequest, resp: &Response) {
    use crate::state::persistence::AutoSaveMode;

    let mode = crate::state::persistence::load_autosave_mode();
    if mode == AutoSaveMode::Off {
        return;
    }

    let is_success = ResponseExample::is_success_status(resp.status, resp.error.as_deref());

    // Route by outcome: success → single success example (overwrite);
    // failure → deduplicated failure examples list. A "both" mode handles each.
    if is_success && mode.saves_success() {
        req.success_example = Some(ResponseExample::from_response(resp));
    }
    if !is_success && mode.saves_failure() {
        let example = ResponseExample::from_response(resp);
        // Deduplicate: remove any existing example with the same
        // (status + body) key, then push the new one at the front.
        let key = example.dedup_key();
        req.fail_examples.retain(|e| e.dedup_key() != key);
        req.fail_examples.insert(0, example);
        // Cap failure examples at 20 to prevent unbounded growth.
        if req.fail_examples.len() > 20 {
            req.fail_examples.truncate(20);
        }
    }
}
