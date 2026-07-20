//! Core data models for Verve.
//!
//! All types here are `Serialize`/`Deserialize` so a project (including its
//! folders, requests, environments, variables and history) can be persisted to
//! a single JSON file under the user data directory.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A request method. Ordered so the common methods appear first in pickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[derive(Default)]
pub enum RequestMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl RequestMethod {
    pub fn all() -> &'static [RequestMethod] {
        &[
            RequestMethod::Get,
            RequestMethod::Post,
            RequestMethod::Put,
            RequestMethod::Delete,
            RequestMethod::Patch,
            RequestMethod::Head,
            RequestMethod::Options,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RequestMethod::Get => "GET",
            RequestMethod::Post => "POST",
            RequestMethod::Put => "PUT",
            RequestMethod::Delete => "DELETE",
            RequestMethod::Patch => "PATCH",
            RequestMethod::Head => "HEAD",
            RequestMethod::Options => "OPTIONS",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "DELETE" => Some(Self::Delete),
            "PATCH" => Some(Self::Patch),
            "HEAD" => Some(Self::Head),
            "OPTIONS" => Some(Self::Options),
            _ => None,
        }
    }
}

impl std::fmt::Display for RequestMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The wire protocol a request uses. Only `Http` and `Sse`/`WebSocket` are
/// executed; the rest are selectable placeholders (PRD roadmap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Protocol {
    #[default]
    Http,
    Sse,
    WebSocket,
    Tcp,
    Grpc,
    SocketIo,
    /// GraphQL (over HTTP) — selectable; executed as HTTP POST.
    Graphql,
    /// A Markdown document node (not a request).
    Markdown,
    /// A directory/folder node.
    Directory,
}

impl Protocol {
    pub fn all() -> &'static [Protocol] {
        &[
            Protocol::Http,
            Protocol::Sse,
            Protocol::WebSocket,
            Protocol::Tcp,
            Protocol::Grpc,
            Protocol::SocketIo,
            Protocol::Graphql,
            Protocol::Markdown,
            Protocol::Directory,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Http => "HTTP",
            Protocol::Sse => "SSE",
            Protocol::WebSocket => "WebSocket",
            Protocol::Tcp => "TCP",
            Protocol::Grpc => "gRPC",
            Protocol::SocketIo => "Socket.IO",
            Protocol::Graphql => "GraphQL",
            Protocol::Markdown => "Markdown",
            Protocol::Directory => "目录",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "http" => Some(Self::Http),
            "sse" => Some(Self::Sse),
            "websocket" | "ws" => Some(Self::WebSocket),
            "tcp" => Some(Self::Tcp),
            "grpc" => Some(Self::Grpc),
            "socket.io" | "socketio" => Some(Self::SocketIo),
            "graphql" => Some(Self::Graphql),
            "markdown" => Some(Self::Markdown),
            "目录" | "directory" => Some(Self::Directory),
            _ => None,
        }
    }

    /// Whether this protocol uses an HTTP method selector.
    pub fn uses_http_method(&self) -> bool {
        matches!(self, Protocol::Http | Protocol::Graphql)
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A column shown in the folder interface list. The set of visible columns is
/// user-customizable and persisted in the layout file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IfaceColumn {
    /// 接口名称 — request name.
    Name,
    /// 请求类型 — HTTP method.
    Method,
    /// 接口路径 — request URL.
    Path,
    /// 接口目录 — owning folder path.
    Folder,
    /// 创建人.
    CreatedBy,
    /// 创建时间.
    CreatedAt,
    /// 最后修改人.
    UpdatedBy,
    /// 修改时间.
    UpdatedAt,
    /// 状态 — lifecycle status.
    Status,
    /// 标签 — free-form tags.
    Tags,
}

impl IfaceColumn {
    /// All selectable columns, in the order shown by the column picker.
    pub fn all() -> &'static [IfaceColumn] {
        &[
            IfaceColumn::Name,
            IfaceColumn::Method,
            IfaceColumn::Path,
            IfaceColumn::Folder,
            IfaceColumn::CreatedBy,
            IfaceColumn::CreatedAt,
            IfaceColumn::UpdatedBy,
            IfaceColumn::UpdatedAt,
            IfaceColumn::Status,
            IfaceColumn::Tags,
        ]
    }

    /// The user-facing column label.
    pub fn label(&self) -> &'static str {
        match self {
            IfaceColumn::Name => "接口名称",
            IfaceColumn::Method => "请求类型",
            IfaceColumn::Path => "接口路径",
            IfaceColumn::Folder => "接口目录",
            IfaceColumn::CreatedBy => "创建人",
            IfaceColumn::CreatedAt => "创建时间",
            IfaceColumn::UpdatedBy => "最后修改人",
            IfaceColumn::UpdatedAt => "修改时间",
            IfaceColumn::Status => "状态",
            IfaceColumn::Tags => "标签",
        }
    }

    /// Relative width weight used by the table layout (flex grow).
    pub fn width_weight(&self) -> f32 {
        match self {
            IfaceColumn::Name => 2.0,
            IfaceColumn::Path => 2.4,
            IfaceColumn::Tags => 1.6,
            IfaceColumn::Method => 0.9,
            _ => 1.1,
        }
    }

    /// Parse from the serde lowercase name.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "name" => Some(Self::Name),
            "method" => Some(Self::Method),
            "path" => Some(Self::Path),
            "folder" => Some(Self::Folder),
            "createdby" => Some(Self::CreatedBy),
            "createdat" => Some(Self::CreatedAt),
            "updatedby" => Some(Self::UpdatedBy),
            "updatedat" => Some(Self::UpdatedAt),
            "status" => Some(Self::Status),
            "tags" => Some(Self::Tags),
            _ => None,
        }
    }

    /// The default visible column set.
    pub fn defaults() -> Vec<IfaceColumn> {
        vec![
            IfaceColumn::Name,
            IfaceColumn::Method,
            IfaceColumn::Path,
            IfaceColumn::CreatedAt,
        ]
    }

    /// The serde-style lowercase name (matches `parse`).
    pub fn as_key(&self) -> &'static str {
        match self {
            IfaceColumn::Name => "name",
            IfaceColumn::Method => "method",
            IfaceColumn::Path => "path",
            IfaceColumn::Folder => "folder",
            IfaceColumn::CreatedBy => "createdby",
            IfaceColumn::CreatedAt => "createdat",
            IfaceColumn::UpdatedBy => "updatedby",
            IfaceColumn::UpdatedAt => "updatedat",
            IfaceColumn::Status => "status",
            IfaceColumn::Tags => "tags",
        }
    }
}

impl std::fmt::Display for IfaceColumn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_key())
    }
}

/// The body type selected for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum BodyType {
    /// No body.
    #[default]
    None,
    /// `multipart/form-data`, may include file parts.
    FormData,
    /// `application/x-www-form-urlencoded`.
    Urlencoded,
    /// A raw text body whose language is given by `raw_language`.
    Raw,
}

/// Language/format for a raw body. Maps to code-editor highlighter language names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RawLanguage {
    #[default]
    Json,
    Xml,
    Text,
    Html,
    Javascript,
}

impl RawLanguage {
    pub fn all() -> &'static [RawLanguage] {
        &[
            RawLanguage::Json,
            RawLanguage::Xml,
            RawLanguage::Text,
            RawLanguage::Html,
            RawLanguage::Javascript,
        ]
    }

    /// Highlighter language name understood by `InputState::code_editor`.
    pub fn highlight(&self) -> &'static str {
        match self {
            RawLanguage::Json => "json",
            RawLanguage::Xml => "xml",
            RawLanguage::Text => "text",
            RawLanguage::Html => "html",
            RawLanguage::Javascript => "javascript",
        }
    }

    /// Default `Content-Type` for the raw language.
    pub fn content_type(&self) -> &'static str {
        match self {
            RawLanguage::Json => "application/json",
            RawLanguage::Xml => "application/xml",
            RawLanguage::Text => "text/plain",
            RawLanguage::Html => "text/html",
            RawLanguage::Javascript => "application/javascript",
        }
    }

    /// Lowercase name used as a label and JSON code-fence tag.
    pub fn lower_name(&self) -> &'static str {
        match self {
            RawLanguage::Json => "json",
            RawLanguage::Xml => "xml",
            RawLanguage::Text => "text",
            RawLanguage::Html => "html",
            RawLanguage::Javascript => "javascript",
        }
    }

    /// Parse from the lowercase name.
    pub fn parse_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "xml" => Some(Self::Xml),
            "text" => Some(Self::Text),
            "html" => Some(Self::Html),
            "javascript" => Some(Self::Javascript),
            _ => None,
        }
    }
}

/// The value type of a key/value field (form-data / query param). Used to drive
/// type-aware serialization and to show a file picker when `File`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    #[default]
    Text,
    File,
    Number,
    Bool,
    Array,
    Decimal,
    Object,
}

impl FieldType {
    pub fn all() -> &'static [FieldType] {
        &[
            FieldType::Text,
            FieldType::File,
            FieldType::Number,
            FieldType::Bool,
            FieldType::Array,
            FieldType::Decimal,
            FieldType::Object,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FieldType::Text => "string",
            FieldType::File => "file",
            FieldType::Number => "number",
            FieldType::Bool => "bool",
            FieldType::Array => "array",
            FieldType::Decimal => "decimal",
            FieldType::Object => "object",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "string" | "text" => Some(Self::Text),
            "file" => Some(Self::File),
            "number" => Some(Self::Number),
            "bool" => Some(Self::Bool),
            "array" => Some(Self::Array),
            "decimal" => Some(Self::Decimal),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    /// Next type in the cycle (for click-to-cycle UI).
    pub fn next(self) -> Self {
        let all = Self::all();
        let i = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }
}

/// A single key/value pair (query param, header, form field). Empty key rows
/// are ignored when building a request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyValue {
    /// Whether this entry is included in the request.
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub key: String,
    pub value: String,
    /// Only used for `form-data` file parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Value type (form-data / query). Defaults to plain text.
    #[serde(default)]
    pub field_type: FieldType,
    /// Whether this field is required (metadata; shown as a red `*` in docs).
    /// Defaults to `true` (postman convention: parameters are required unless
    /// explicitly marked optional).
    #[serde(default = "default_true")]
    pub required: bool,
    /// Free-form description (shown in the manager tables, like postman).
    #[serde(default)]
    pub description: String,
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            key: key.into(),
            value: value.into(),
            file_path: None,
            field_type: FieldType::Text,
            required: true,
            description: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.key.trim().is_empty()
    }
}

fn default_true() -> bool {
    true
}

/// A request body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestBody {
    #[serde(default)]
    pub body_type: BodyType,
    #[serde(default)]
    pub raw_language: RawLanguage,
    #[serde(default)]
    pub raw: String,
    #[serde(default)]
    pub form_data: Vec<KeyValue>,
    #[serde(default)]
    pub urlencoded: Vec<KeyValue>,
    /// Visual field breakdown of a Raw JSON body (postman `raw_parameter`).
    /// Populated when the user edits the Raw body via the "可视化编辑" mode.
    /// Each entry corresponds to a top-level key in the JSON object. Editing
    /// these syncs back to `raw` and vice versa.
    #[serde(default)]
    pub raw_parameter: Vec<KeyValue>,
}

impl RequestBody {
    pub fn is_empty(&self) -> bool {
        match self.body_type {
            BodyType::None => true,
            BodyType::Raw => self.raw.trim().is_empty(),
            BodyType::FormData => self.form_data.iter().all(|kv| kv.is_empty()),
            BodyType::Urlencoded => self.urlencoded.iter().all(|kv| kv.is_empty()),
        }
    }

    /// Parse the Raw JSON body into [`raw_parameter`](Self::raw_parameter) fields.
    /// Each top-level key of the JSON object becomes one `KeyValue` row whose
    /// `value` is the JSON-stringified value. Non-JSON or non-object bodies are
    /// left untouched (the returned vec is empty). Existing `raw_parameter`
    /// entries are reused so the user's required/description edits are preserved
    /// for keys that still exist.
    pub fn sync_raw_to_fields(&mut self) {
        if self.raw_language != RawLanguage::Json {
            return;
        }
        let trimmed = self.raw.trim();
        if trimmed.is_empty() {
            return;
        }
        let Ok(obj) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            return;
        };
        let Some(map) = obj.as_object() else {
            return;
        };
        let old: std::collections::HashMap<String, KeyValue> = self
            .raw_parameter
            .drain(..)
            .map(|kv| (kv.key.clone(), kv))
            .collect();
        self.raw_parameter = map
            .iter()
            .map(|(key, val)| {
                let mut kv = old
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| KeyValue::new(key.clone(), String::new()));
                kv.key = key.clone();
                kv.value = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                kv.field_type = json_type_to_field(val);
                kv
            })
            .collect();
    }

    /// Serialize [`raw_parameter`](Self::raw_parameter) back into the Raw JSON
    /// body. Each row's `value` is attempted as JSON, falling back to a string.
    /// The result is pretty-printed JSON.
    pub fn sync_fields_to_raw(&mut self) {
        if self.raw_language != RawLanguage::Json || self.raw_parameter.is_empty() {
            return;
        }
        let mut map = serde_json::Map::new();
        for kv in &self.raw_parameter {
            if kv.key.trim().is_empty() {
                continue;
            }
            let parsed = serde_json::from_str::<serde_json::Value>(&kv.value)
                .unwrap_or_else(|_| serde_json::Value::String(kv.value.clone()));
            map.insert(kv.key.clone(), parsed);
        }
        if let Ok(json) = serde_json::to_string_pretty(&serde_json::Value::Object(map)) {
            self.raw = json;
        }
    }
}

/// Infer a [`FieldType`] from a JSON value (for visual field editing).
fn json_type_to_field(val: &serde_json::Value) -> FieldType {
    match val {
        serde_json::Value::Null => FieldType::Text,
        serde_json::Value::Bool(_) => FieldType::Bool,
        serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => FieldType::Number,
        serde_json::Value::Number(_) => FieldType::Decimal,
        serde_json::Value::String(_) => FieldType::Text,
        serde_json::Value::Array(_) => FieldType::Array,
        serde_json::Value::Object(_) => FieldType::Object,
    }
}

/// Where an API-key auth credential is injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AuthTarget {
    #[default]
    Header,
    Query,
}

/// The authentication scheme selected for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AuthType {
    #[default]
    None,
    Bearer,
    Basic,
    ApiKey,
}

impl AuthType {
    pub fn all() -> &'static [AuthType] {
        &[
            AuthType::None,
            AuthType::Bearer,
            AuthType::Basic,
            AuthType::ApiKey,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AuthType::None => "No Auth",
            AuthType::Bearer => "Bearer Token",
            AuthType::Basic => "Basic Auth",
            AuthType::ApiKey => "API Key",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "No Auth" => Some(Self::None),
            "Bearer Token" => Some(Self::Bearer),
            "Basic Auth" => Some(Self::Basic),
            "API Key" => Some(Self::ApiKey),
            _ => None,
        }
    }
}

/// Authentication configuration for a request. Translated into headers/query
/// at request-prepare time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub auth_type: AuthType,
    /// Bearer token value.
    #[serde(default)]
    pub token: String,
    /// Basic-auth username.
    #[serde(default)]
    pub username: String,
    /// Basic-auth password.
    #[serde(default)]
    pub password: String,
    /// API-key parameter name.
    #[serde(default)]
    pub key: String,
    /// API-key parameter value.
    #[serde(default)]
    pub value: String,
    /// Where to inject the API key.
    #[serde(default)]
    pub add_to: AuthTarget,
}

impl AuthConfig {
    pub fn is_active(&self) -> bool {
        !matches!(self.auth_type, AuthType::None)
    }
}

/// A captured response. Stored on the request for the response panel, and a
/// compact copy is appended to history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    /// Human status text e.g. "OK".
    #[serde(default)]
    pub status_text: String,
    pub time_ms: u64,
    pub size: u64,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: String,
    /// Whether `body` is JSON (controls pretty-print / highlighting).
    #[serde(default)]
    pub is_json: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// True while a streaming protocol (SSE/WebSocket) is actively receiving.
    #[serde(default)]
    pub streaming: bool,
}

/// An API request definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub method: RequestMethod,
    /// Wire protocol (HTTP/SSE/WebSocket/...). Defaults to HTTP.
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    /// URL path-template variables (`{{key}}` → value), substituted into `url`.
    #[serde(default)]
    pub path: Vec<KeyValue>,
    /// Cookies, serialized into a `Cookie` header at prepare time.
    #[serde(default)]
    pub cookies: Vec<KeyValue>,
    #[serde(default)]
    pub body: RequestBody,
    /// Authentication configuration, injected at prepare time.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Markdown description for the docs view.
    #[serde(default)]
    pub description: String,
    /// Request-level variables (highest priority scope).
    #[serde(default)]
    pub variables: Vec<KeyValue>,
    /// Pre-request script source (runs before send, can set variables).
    #[serde(default)]
    pub pre_script: String,
    /// Tests script source (runs after the response, can assert + extract).
    #[serde(default)]
    pub tests_script: String,
    /// Mock rule for the local mock server.
    #[serde(default)]
    pub mock: Option<MockRule>,
    /// Last captured response (for re-opening the app with state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_response: Option<Response>,
    /// Creator display name (audit metadata).
    #[serde(default)]
    pub created_by: String,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: String,
    /// Last editor display name.
    #[serde(default)]
    pub updated_by: String,
    /// ISO-8601 last-update timestamp.
    #[serde(default)]
    pub updated_at: String,
    /// Free-form status label (e.g. "已发布"/"开发中"/"废弃").
    #[serde(default)]
    pub status: String,
    /// Free-form comma/space separated tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl ApiRequest {
    pub fn new(name: impl Into<String>, method: RequestMethod, url: impl Into<String>) -> Self {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        Self {
            id: new_id(),
            name: name.into(),
            method,
            protocol: Protocol::Http,
            url: url.into(),
            params: Vec::new(),
            headers: Vec::new(),
            path: Vec::new(),
            cookies: Vec::new(),
            body: RequestBody::default(),
            auth: AuthConfig::default(),
            description: String::new(),
            variables: Vec::new(),
            pre_script: String::new(),
            tests_script: String::new(),
            mock: None,
            last_response: None,
            created_by: "me".to_string(),
            created_at: now.clone(),
            updated_by: "me".to_string(),
            updated_at: now,
            status: String::new(),
            tags: Vec::new(),
        }
    }
}

/// A mock rule used by the local mock server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockRule {
    pub enabled: bool,
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    pub body: String,
    /// Delay in milliseconds before responding.
    #[serde(default)]
    pub delay_ms: u64,
}

impl Default for MockRule {
    fn default() -> Self {
        Self {
            enabled: true,
            status: 200,
            headers: vec![KeyValue::new("Content-Type", "application/json")],
            body: "{}".to_string(),
            delay_ms: 0,
        }
    }
}

/// A folder groups requests (and nested folders).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// Folder description shown in the folder detail view.
    #[serde(default)]
    pub description: String,
    /// Folder-level query parameters inherited by child requests.
    #[serde(default)]
    pub params: Vec<KeyValue>,
    /// Folder-level headers inherited by child requests.
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub folders: Vec<Folder>,
    #[serde(default)]
    pub requests: Vec<ApiRequest>,
    /// Folder-level variables.
    #[serde(default)]
    pub variables: Vec<KeyValue>,
    /// Base URL prefix for this folder. When set, child requests' URLs are
    /// automatically prefixed with this value at send time if they don't
    /// already start with http:// or https://.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            folders: Vec::new(),
            requests: Vec::new(),
            variables: Vec::new(),
            base_url: None,
        }
    }
}

/// An environment: a named set of variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub variables: Vec<KeyValue>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            variables: Vec::new(),
        }
    }
}

/// A project is the top-level unit, persisted as one JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// Top-level requests not in any folder.
    #[serde(default)]
    pub requests: Vec<ApiRequest>,
    #[serde(default)]
    pub environments: Vec<Environment>,
    /// Active environment id.
    #[serde(default)]
    pub active_environment: Option<String>,
    /// Project-global variables (lowest priority scope).
    #[serde(default)]
    pub global_variables: Vec<KeyValue>,
    /// Project-global query parameters (applied to every request).
    #[serde(default)]
    pub global_params: Vec<KeyValue>,
    /// Project-global request headers (applied to every request).
    #[serde(default)]
    pub global_headers: Vec<KeyValue>,
    /// Project-global cookies (jar) keyed by domain.
    #[serde(default)]
    pub global_cookies: Vec<KeyValue>,
    /// Merge requests tracked in the project-management surface.
    #[serde(default)]
    pub merge_requests: Vec<MergeRequest>,
    /// Shared HTTP status code dictionary (公共资源维护 → 状态码字典).
    #[serde(default)]
    pub status_codes: Vec<StatusCodeEntry>,
    /// OpenAPI access tokens (对外能力).
    #[serde(default)]
    pub api_tokens: Vec<ApiToken>,
    /// Automated test suites (collection-runner style test cases + steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_suites: Vec<crate::autotest::TestSuite>,
    /// Soft-deleted/archived flag.
    #[serde(default)]
    pub archived: bool,
}

/// A merge request in the project-management surface (合并请求).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRequest {
    pub id: String,
    pub title: String,
    /// Branch the changes come from.
    pub source_branch: String,
    /// Branch the changes merge into.
    pub target_branch: String,
    /// Lifecycle state: "open", "merged", "closed".
    #[serde(default)]
    pub state: String,
    /// Author display name.
    #[serde(default)]
    pub author: String,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: String,
}

impl MergeRequest {
    pub fn new(
        title: impl Into<String>,
        source: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            title: title.into(),
            source_branch: source.into(),
            target_branch: target.into(),
            state: "open".to_string(),
            author: "me".to_string(),
            created_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}

/// A shared status-code dictionary entry (状态码字典).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusCodeEntry {
    pub code: u16,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

impl StatusCodeEntry {
    pub fn new(code: u16, name: impl Into<String>) -> Self {
        Self {
            code,
            name: name.into(),
            description: String::new(),
        }
    }

    /// A small built-in dictionary seeded for new projects so the
    /// 公共资源维护 page isn't empty.
    pub fn defaults() -> Vec<Self> {
        [
            (200, "OK", "请求成功"),
            (201, "Created", "资源创建成功"),
            (204, "No Content", "成功但无返回内容"),
            (301, "Moved Permanently", "永久重定向"),
            (302, "Found", "临时重定向"),
            (304, "Not Modified", "资源未修改，可使用缓存"),
            (400, "Bad Request", "请求参数错误"),
            (401, "Unauthorized", "未授权，缺少或无效的认证"),
            (403, "Forbidden", "禁止访问"),
            (404, "Not Found", "资源不存在"),
            (405, "Method Not Allowed", "请求方法不被允许"),
            (409, "Conflict", "资源冲突"),
            (422, "Unprocessable Entity", "语义错误，无法处理"),
            (429, "Too Many Requests", "请求过于频繁，已限流"),
            (500, "Internal Server Error", "服务器内部错误"),
            (502, "Bad Gateway", "网关错误"),
            (503, "Service Unavailable", "服务暂不可用"),
            (504, "Gateway Timeout", "网关超时"),
        ]
        .iter()
        .map(|(c, n, d)| Self {
            code: *c,
            name: n.to_string(),
            description: d.to_string(),
        })
        .collect()
    }
}

/// An OpenAPI access token (对外能力 → API token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    /// Human label for the token (备注名).
    pub label: String,
    /// The secret value (shown once, masked in the table).
    pub token: String,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: String,
}

impl ApiToken {
    pub fn new(label: impl Into<String>) -> Self {
        // A short opaque token — enough for the manager UI; not cryptographically strong.
        let rand = uuid::Uuid::new_v4().simple().to_string();
        Self {
            id: new_id(),
            label: label.into(),
            token: format!("verve_{}", &rand[..16]),
            created_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: String::new(),
            folders: Vec::new(),
            requests: Vec::new(),
            environments: Vec::new(),
            active_environment: None,
            global_variables: Vec::new(),
            global_params: Vec::new(),
            global_headers: Vec::new(),
            global_cookies: Vec::new(),
            merge_requests: Vec::new(),
            status_codes: StatusCodeEntry::defaults(),
            api_tokens: Vec::new(),
            test_suites: Vec::new(),
            archived: false,
        }
    }

    /// Find the active environment's variables (or empty).
    pub fn active_env_variables(&self) -> &[KeyValue] {
        match &self.active_environment {
            Some(id) => self
                .environments
                .iter()
                .find(|e| &e.id == id)
                .map(|e| e.variables.as_slice())
                .unwrap_or(&[]),
            None => &[],
        }
    }

    /// Borrow a request by id. Returns the folder-id chain (root → parent) plus
    /// the request. Use [`folder_variables_chain`] to resolve the variable scope.
    pub fn find_request(&self, id: &str) -> Option<(Vec<String>, &ApiRequest)> {
        fn walk<'a>(
            folders: &'a [Folder],
            path: &mut Vec<String>,
            id: &str,
        ) -> Option<(Vec<String>, &'a ApiRequest)> {
            for folder in folders {
                path.push(folder.id.clone());
                for req in &folder.requests {
                    if req.id == id {
                        let chain = path.clone();
                        return Some((chain, req));
                    }
                }
                if let Some(found) = walk(&folder.folders, path, id) {
                    return Some(found);
                }
                path.pop();
            }
            None
        }
        for req in &self.requests {
            if req.id == id {
                return Some((Vec::new(), req));
            }
        }
        let mut path = Vec::new();
        walk(&self.folders, &mut path, id)
    }

    /// Mutably borrow a request by id, with its folder-id chain.
    pub fn find_request_mut(&mut self, id: &str) -> Option<(Vec<String>, &mut ApiRequest)> {
        fn walk<'a>(
            folders: &'a mut [Folder],
            path: &mut Vec<String>,
            id: &str,
        ) -> Option<(Vec<String>, &'a mut ApiRequest)> {
            for folder in folders {
                path.push(folder.id.clone());
                for req in &mut folder.requests {
                    if req.id == id {
                        let chain = path.clone();
                        return Some((chain, req));
                    }
                }
                if let Some(found) = walk(&mut folder.folders, path, id) {
                    return Some(found);
                }
                path.pop();
            }
            None
        }
        for req in &mut self.requests {
            if req.id == id {
                return Some((Vec::new(), req));
            }
        }
        let mut path = Vec::new();
        walk(&mut self.folders, &mut path, id)
    }

    /// Iterate every request in the project (top-level + nested folders),
    /// yielding the request and the human-readable folder path (`>`-joined).
    /// Used by the management tables (接口属性 / 接口状态 / Mock 服务).
    pub fn iter_all_requests(&self) -> Vec<(String, &ApiRequest)> {
        let mut out: Vec<(String, &ApiRequest)> = Vec::new();
        for req in &self.requests {
            out.push((String::new(), req));
        }
        fn walk<'a>(folders: &'a [Folder], prefix: &str, out: &mut Vec<(String, &'a ApiRequest)>) {
            for folder in folders {
                let path = if prefix.is_empty() {
                    folder.name.clone()
                } else {
                    format!("{prefix} > {}", folder.name)
                };
                for req in &folder.requests {
                    out.push((path.clone(), req));
                }
                walk(&folder.folders, &path, out);
            }
        }
        walk(&self.folders, "", &mut out);
        out
    }

    /// Collect variables along a folder-id chain (root → parent), deepest last.
    pub fn folder_variables_chain(&self, chain: &[String]) -> Vec<&[KeyValue]> {
        let mut out = Vec::new();
        let mut folders = self.folders.as_slice();
        for id in chain {
            if let Some(f) = folders.iter().find(|f| f.id == *id) {
                out.push(f.variables.as_slice());
                folders = f.folders.as_slice();
            } else {
                break;
            }
        }
        out
    }

    /// Borrow a folder by id (anywhere in the tree), returning its ancestor-id
    /// chain (root → parent) plus the folder reference.
    pub fn find_folder(&self, id: &str) -> Option<(Vec<String>, &Folder)> {
        fn walk<'a>(
            folders: &'a [Folder],
            path: &mut Vec<String>,
            id: &str,
        ) -> Option<(Vec<String>, &'a Folder)> {
            for folder in folders {
                path.push(folder.id.clone());
                if folder.id == id {
                    let chain = path.clone();
                    return Some((chain, folder));
                }
                if let Some(found) = walk(&folder.folders, path, id) {
                    return Some(found);
                }
                path.pop();
            }
            None
        }
        let mut path = Vec::new();
        walk(&self.folders, &mut path, id)
    }

    /// Mutably borrow a folder by id, with its ancestor-id chain.
    pub fn find_folder_mut(&mut self, id: &str) -> Option<(Vec<String>, &mut Folder)> {
        fn walk<'a>(
            folders: &'a mut [Folder],
            path: &mut Vec<String>,
            id: &str,
        ) -> Option<(Vec<String>, &'a mut Folder)> {
            for folder in folders.iter_mut() {
                let folder_id = folder.id.clone();
                path.push(folder_id.clone());
                if folder_id == id {
                    let chain = path.clone();
                    return Some((chain, folder));
                }
                if let Some(found) = walk(folder.folders.as_mut_slice(), path, id) {
                    return Some(found);
                }
                path.pop();
            }
            None
        }
        let mut path = Vec::new();
        walk(self.folders.as_mut_slice(), &mut path, id)
    }

    /// Recursively collect all request ids within a folder (its own + nested
    /// folders). Used by the folder-detail "interface list".
    pub fn collect_requests_in_folder(&self, folder_id: &str) -> Vec<(String, String)> {
        // Returns (id, name) tuples; the folder's own requests first, then
        // those of nested folders.
        let mut out = Vec::new();
        fn walk(folder: &Folder, out: &mut Vec<(String, String)>) {
            for req in &folder.requests {
                out.push((req.id.clone(), req.name.clone()));
            }
            for sub in &folder.folders {
                walk(sub, out);
            }
        }
        if let Some((_, folder)) = self.find_folder(folder_id) {
            walk(folder, &mut out);
        }
        out
    }

    /// Detach a request by id from wherever it lives (root or any folder),
    /// returning the owned request. Returns None if not found.
    fn take_request(&mut self, id: &str) -> Option<ApiRequest> {
        // Root level.
        if let Some(pos) = self.requests.iter().position(|r| r.id == id) {
            return Some(self.requests.remove(pos));
        }
        // Any folder (recursive).
        take_request_from_folders(&mut self.folders, id)
    }

    /// Move a request to a new location. `dest` describes where to drop it.
    /// No-op (returns false) when the target is invalid or the move would be
    /// a no-op (e.g. dropping a request onto itself).
    pub fn move_request(&mut self, req_id: &str, dest: &MoveTarget) -> bool {
        // Can't drop onto yourself.
        match dest {
            MoveTarget::BeforeRequest(id) | MoveTarget::AfterRequest(id) if id == req_id => {
                return false;
            }
            _ => {}
        }
        let req = match self.take_request(req_id) {
            Some(r) => r,
            None => return false,
        };
        match dest {
            MoveTarget::ToRoot => {
                self.requests.push(req);
                true
            }
            MoveTarget::IntoFolder(folder_id) => {
                if let Some((_, folder)) = self.find_folder_mut(folder_id) {
                    folder.requests.push(req);
                    true
                } else {
                    // Folder gone — restore at root to avoid data loss.
                    self.requests.push(req);
                    false
                }
            }
            MoveTarget::BeforeRequest(target_id) => {
                insert_request_relative(&mut self.requests, &mut self.folders, req, target_id, true)
            }
            MoveTarget::AfterRequest(target_id) => insert_request_relative(
                &mut self.requests,
                &mut self.folders,
                req,
                target_id,
                false,
            ),
        }
    }

    /// Move a folder to a new location. Prevents dropping a folder into itself
    /// or one of its descendants.
    pub fn move_folder(&mut self, folder_id: &str, dest: &MoveTarget) -> bool {
        // Determine the set of ids that would form a cycle (the folder + all
        // its descendants). Reject any dest referencing them.
        let subtree_ids = self.folder_subtree_ids(folder_id);
        let dest_id = match dest {
            MoveTarget::IntoFolder(id) => Some(id.as_str()),
            _ => None,
        };
        if let Some(d) = dest_id {
            if subtree_ids.contains(d) {
                return false;
            }
        }
        let folder = match take_folder(&mut self.folders, folder_id) {
            Some(f) => f,
            None => return false,
        };
        match dest {
            MoveTarget::IntoFolder(target_id) => {
                if let Some((_, parent)) = self.find_folder_mut(target_id) {
                    parent.folders.push(folder);
                    true
                } else {
                    self.folders.push(folder);
                    false
                }
            }
            MoveTarget::ToRoot => {
                self.folders.push(folder);
                true
            }
            _ => false, // ordering folders before/after requests isn't meaningful
        }
    }

    /// Collect a folder's id plus every descendant folder id.
    fn folder_subtree_ids(&self, folder_id: &str) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        if let Some((_, folder)) = self.find_folder(folder_id) {
            collect_subtree(folder, &mut out);
        }
        out
    }
}

/// Where a dragged node should be dropped.
#[derive(Debug, Clone)]
pub enum MoveTarget {
    /// Move to the project root (top level).
    ToRoot,
    /// Drop into a folder (appended to that folder's children).
    IntoFolder(String),
    /// Reorder immediately before this request.
    BeforeRequest(String),
    /// Reorder immediately after this request.
    AfterRequest(String),
}

/// Recursively remove a request by id from a list of folders.
fn take_request_from_folders(folders: &mut [Folder], id: &str) -> Option<ApiRequest> {
    for folder in folders.iter_mut() {
        if let Some(pos) = folder.requests.iter().position(|r| r.id == id) {
            return Some(folder.requests.remove(pos));
        }
        if let Some(found) = take_request_from_folders(folder.folders.as_mut_slice(), id) {
            return Some(found);
        }
    }
    None
}

/// Insert `req` before/after the request with `target_id`, searching the root
/// list and every folder. Returns false if the target wasn't found (the
/// request is then restored at the root to avoid data loss).
fn insert_request_relative(
    root: &mut Vec<ApiRequest>,
    folders: &mut [Folder],
    req: ApiRequest,
    target_id: &str,
    before: bool,
) -> bool {
    // Root list.
    if let Some(pos) = root.iter().position(|r| r.id == target_id) {
        let insert_at = if before { pos } else { pos + 1 };
        root.insert(insert_at, req);
        return true;
    }
    // Folders (recursive). Find the target location first, then insert — this
    // avoids moving `req` into a branch that won't end up using it.
    if let Some((folder, pos)) = find_request_location_mut(folders, target_id) {
        let insert_at = if before { pos } else { pos + 1 };
        folder.requests.insert(insert_at, req);
        return true;
    }
    // Target not found — restore at root.
    root.push(req);
    false
}

/// Borrow the folder + index that holds `target_id`, mutably.
fn find_request_location_mut<'a>(
    folders: &'a mut [Folder],
    target_id: &str,
) -> Option<(&'a mut Folder, usize)> {
    for folder in folders.iter_mut() {
        if let Some(pos) = folder.requests.iter().position(|r| r.id == target_id) {
            return Some((folder, pos));
        }
        if let Some(found) = find_request_location_mut(folder.folders.as_mut_slice(), target_id) {
            return Some(found);
        }
    }
    None
}

/// Recursively remove a folder by id from a list of folders.
fn take_folder(folders: &mut Vec<Folder>, id: &str) -> Option<Folder> {
    if let Some(pos) = folders.iter().position(|f| f.id == id) {
        return Some(folders.remove(pos));
    }
    for folder in folders.iter_mut() {
        if let Some(found) = take_folder(&mut folder.folders, id) {
            return Some(found);
        }
    }
    None
}

fn collect_subtree(folder: &Folder, out: &mut std::collections::HashSet<String>) {
    out.insert(folder.id.clone());
    for sub in &folder.folders {
        collect_subtree(sub, out);
    }
}

/// A compact history entry for the console / history list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub request_id: Option<String>,
    pub name: String,
    pub method: RequestMethod,
    pub url: String,
    pub status: u16,
    pub time_ms: u64,
    pub size: u64,
    /// ISO-8601 timestamp.
    pub at: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// The persisted application state for ONE workspace: its projects plus history.
/// Each workspace's data lives on its own git branch (`verve/<id>`), so this
/// struct is what `workspace.json` serializes — and its content differs per
/// branch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceData {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

/// Metadata for one workspace, stored in the cross-branch `workspaces.json`
/// index (NOT inside `workspace.json`, which is per-branch).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    pub id: String,
    pub name: String,
    /// Git branch backing this workspace: `main` for the default workspace,
    /// `verve/<id>` for others.
    pub branch: String,
    /// The built-in default workspace cannot be deleted.
    #[serde(default)]
    pub is_default: bool,
}

impl WorkspaceMeta {
    /// The default workspace (id="default", branch="main", is_default=true).
    pub fn default_workspace() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default".to_string(),
            branch: "main".to_string(),
            is_default: true,
        }
    }

    /// Create a new (non-default) workspace with a fresh id + `verve/<id>` branch.
    pub fn new(name: impl Into<String>) -> Self {
        let id = new_id();
        let short = id[..8.min(id.len())].to_string();
        Self {
            id,
            name: name.into(),
            branch: format!("verve/{short}"),
            is_default: false,
        }
    }
}

/// The cross-branch index of all workspaces, persisted at
/// `~/.verve/workspaces.json` (excluded from git). `active` holds the id of
/// the currently-selected workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspacesIndex {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceMeta>,
    #[serde(default)]
    pub active: Option<String>,
}

impl WorkspacesIndex {
    /// A fresh index with only the built-in default workspace, set active.
    pub fn with_default() -> Self {
        let default = WorkspaceMeta::default_workspace();
        Self {
            workspaces: vec![default.clone()],
            active: Some(default.id),
        }
    }

    /// Find a workspace by id.
    pub fn find(&self, id: &str) -> Option<&WorkspaceMeta> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    /// The active workspace metadata, if any.
    pub fn active_meta(&self) -> Option<&WorkspaceMeta> {
        self.active.as_ref().and_then(|id| self.find(id))
    }
}

/// Generate a new unique id (uuid v4, simple form).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Build a `BTreeMap` of the effective variables for a single request, applying
/// the scope priority: global < environment < folder chain < request.
///
/// `folder_vars` is the already-flattened variables of the request's ancestor
/// folders (root → parent), built by [`Project::folder_variables_chain`].
pub fn effective_variables(
    global: &[KeyValue],
    env: &[KeyValue],
    folder_vars: &[KeyValue],
    request_vars: &[KeyValue],
) -> BTreeMap<String, String> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let push = |map: &mut BTreeMap<String, String>, vars: &[KeyValue]| {
        for kv in vars {
            if kv.enabled && !kv.key.trim().is_empty() {
                map.insert(kv.key.clone(), kv.value.clone());
            }
        }
    };
    push(&mut map, global);
    push(&mut map, env);
    push(&mut map, folder_vars);
    push(&mut map, request_vars);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_to_fields_parses_json_object() {
        let mut body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: RawLanguage::Json,
            raw: r#"{"name":"admin","age":25,"active":true}"#.into(),
            ..Default::default()
        };
        body.sync_raw_to_fields();
        assert_eq!(body.raw_parameter.len(), 3);
        assert_eq!(body.raw_parameter[0].key, "name");
        assert_eq!(body.raw_parameter[0].value, "admin");
        assert_eq!(body.raw_parameter[0].field_type, FieldType::Text);
        assert_eq!(body.raw_parameter[1].key, "age");
        assert_eq!(body.raw_parameter[1].field_type, FieldType::Number);
        assert_eq!(body.raw_parameter[2].field_type, FieldType::Bool);
    }

    #[test]
    fn fields_to_raw_serializes_json() {
        let mut body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: RawLanguage::Json,
            raw_parameter: vec![
                KeyValue {
                    enabled: true,
                    key: "name".into(),
                    value: "test".into(),
                    required: true,
                    ..Default::default()
                },
                KeyValue {
                    enabled: true,
                    key: "count".into(),
                    value: "42".into(),
                    required: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        body.sync_fields_to_raw();
        let parsed: serde_json::Value = serde_json::from_str(&body.raw).unwrap();
        assert_eq!(parsed["name"], "test");
        assert_eq!(parsed["count"], 42);
    }

    #[test]
    fn sync_preserves_user_edits() {
        // If the user set description/required on a field, re-parsing raw JSON
        // should keep those edits for keys that still exist.
        let mut body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: RawLanguage::Json,
            raw: r#"{"name":"a"}"#.into(),
            raw_parameter: vec![KeyValue {
                enabled: true,
                key: "name".into(),
                value: "a".into(),
                required: false, // user marked optional
                description: "用户名".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        body.sync_raw_to_fields();
        assert_eq!(body.raw_parameter.len(), 1);
        assert!(
            !body.raw_parameter[0].required,
            "user's required=false preserved"
        );
        assert_eq!(body.raw_parameter[0].description, "用户名");
    }

    #[test]
    fn non_json_body_not_parsed() {
        let mut body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: RawLanguage::Xml,
            raw: "<root/>".into(),
            ..Default::default()
        };
        body.sync_raw_to_fields();
        assert!(
            body.raw_parameter.is_empty(),
            "XML bodies should not be parsed into fields"
        );
    }

    #[test]
    fn keyvalue_deserialize_defaults_required_true() {
        // A KeyValue without an explicit `required` field should default to true.
        let json = r#"{"key":"k","value":"v"}"#;
        let kv: KeyValue = serde_json::from_str(json).unwrap();
        assert!(kv.required, "missing `required` should default to true");
    }
}
