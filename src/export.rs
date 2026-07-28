//! Exporters: render a project to Markdown, JSON, or postman format.

use crate::state::models::{
    ApiRequest, AuthConfig, AuthType, BodyType, Folder, KeyValue, Project, RequestMethod,
};
use serde_json::json;

/// Export format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
    /// Apipost 7+ export JSON (round-trip compatible with `import::postman`).
    Apipost,
    /// Standard Postman Collection v2.1.0 JSON.
    PostmanV2_1,
    /// OpenAPI 2.0 (fka Swagger) JSON.
    Swagger,
    /// OpenAPI 3.0 JSON.
    OpenApi3,
}

impl Format {
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Markdown => "md",
            _ => "json",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Format::Markdown => "Markdown",
            Format::Json => "JSON",
            Format::Apipost => "Apipost",
            Format::PostmanV2_1 => "Postman v2.1",
            Format::Swagger => "Swagger 2.0",
            Format::OpenApi3 => "OpenAPI 3",
        }
    }

    /// All formats in dropdown order.
    pub const ALL: &'static [Format] = &[
        Format::Markdown,
        Format::Json,
        Format::Apipost,
        Format::PostmanV2_1,
        Format::Swagger,
        Format::OpenApi3,
    ];
}

/// Render a project as a Markdown API document (PRD §5.4 offline export).
pub fn project_to_markdown(project: &Project) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", project.name));
    if !project.description.trim().is_empty() {
        out.push_str(&project.description);
        out.push_str("\n\n");
    }
    if let Some(env) = project
        .active_environment
        .as_ref()
        .and_then(|id| project.environments.iter().find(|e| &e.id == id))
    {
        out.push_str(&format!("> Active environment: **{}**\n\n", env.name));
    }

    out.push_str("## Requests\n\n");
    for req in &project.requests {
        render_request_md(&mut out, req, 3);
    }
    for folder in &project.folders {
        render_folder_md(&mut out, folder, 3);
    }
    out
}

fn render_folder_md(out: &mut String, folder: &Folder, level: usize) {
    let heading = "#".repeat(level);
    out.push_str(&format!("{heading} {}\n\n", folder.name));
    for req in &folder.requests {
        render_request_md(out, req, level + 1);
    }
    for sub in &folder.folders {
        render_folder_md(out, sub, level + 1);
    }
}

fn render_request_md(out: &mut String, req: &crate::state::models::ApiRequest, level: usize) {
    let heading = "#".repeat(level);
    out.push_str(&format!("{heading} `{}` {}\n\n", req.method, req.name));
    out.push_str(&format!("- **URL:** `{}`\n", req.url));
    out.push_str(&format!("- **Method:** {}\n", method_label(req.method)));
    if !req.params.is_empty() {
        out.push_str("- **Query params:**\n\n");
        out.push_str("| Key | Value | Enabled |\n| --- | --- | --- |\n");
        for p in &req.params {
            out.push_str(&format!("| {} | {} | {} |\n", p.key, p.value, p.enabled));
        }
        out.push('\n');
    }
    if !req.headers.is_empty() {
        out.push_str("- **Headers:**\n\n");
        out.push_str("| Key | Value |\n| --- | --- |\n");
        for h in req.headers.iter().filter(|h| h.enabled) {
            out.push_str(&format!("| {} | {} |\n", h.key, h.value));
        }
        out.push('\n');
    }
    if req.auth.is_active() {
        out.push_str(&format!("- **Auth:** {}\n", req.auth.auth_type.as_str()));
    }
    if !req.cookies.is_empty() {
        out.push_str("- **Cookies:**\n\n");
        out.push_str("| Key | Value |\n| --- | --- |\n");
        for c in req.cookies.iter().filter(|c| c.enabled) {
            out.push_str(&format!("| {} | {} |\n", c.key, c.value));
        }
        out.push('\n');
    }
    if req.body.body_type != BodyType::None {
        out.push_str(&format!("- **Body** ({:?}):\n\n", req.body.body_type));
        match req.body.body_type {
            BodyType::Raw => {
                out.push_str(&format!(
                    "```{}\n{}\n```\n\n",
                    req.body.raw_language.lower_name(),
                    req.body.raw
                ));
            }
            BodyType::Urlencoded | BodyType::FormData => {
                let rows = if req.body.body_type == BodyType::FormData {
                    &req.body.form_data
                } else {
                    &req.body.urlencoded
                };
                out.push_str("| Key | Value |\n| --- | --- |\n");
                for kv in rows.iter().filter(|kv| kv.enabled) {
                    out.push_str(&format!("| {} | {} |\n", kv.key, kv.value));
                }
                out.push('\n');
            }
            BodyType::None => {}
        }
    }
    if !req.description.trim().is_empty() {
        out.push_str(&req.description);
        out.push_str("\n\n");
    }
    out.push_str("---\n\n");
}

fn method_label(m: RequestMethod) -> &'static str {
    match m {
        RequestMethod::Get => "GET",
        RequestMethod::Post => "POST",
        RequestMethod::Put => "PUT",
        RequestMethod::Delete => "DELETE",
        RequestMethod::Patch => "PATCH",
        RequestMethod::Head => "HEAD",
        RequestMethod::Options => "OPTIONS",
    }
}

/// Serialize the project to pretty JSON (the Verve-native interchange format).
pub fn project_to_json(project: &Project) -> serde_json::Result<String> {
    serde_json::to_string_pretty(project)
}

// ===========================================================================
// postman 7+ export format
// ===========================================================================

/// Render a project as an Apipost-compatible JSON string. The output is
/// round-trip compatible: importing it back via `import::postman` reproduces
/// the same folder/request tree, bodies, params, and auth.
pub fn project_to_apipost(project: &Project) -> serde_json::Result<String> {
    let doc = build_postman_doc(project);
    serde_json::to_string_pretty(&doc)
}

/// Find the server URL for an environment.
///
/// A "server" is any environment variable whose value looks like a base URL
/// (starts with `http://` or `https://`). This includes the legacy manually
/// named `baseUrl`/`base_url` variables as well as imported servers whose
/// variable key is the server's display name (e.g. `默认服务`, `user-center`).
/// Returns the first match.
fn env_server_url(env: &crate::state::models::Environment) -> Option<String> {
    env.variables.iter().find_map(|v| {
        if !v.enabled {
            return None;
        }
        let val = v.value.trim();
        // Legacy names kept for back-compat with older project files.
        let is_legacy_name = v
            .key
            .eq_ignore_ascii_case("baseurl")
            || v.key.eq_ignore_ascii_case("base_url");
        // Imported servers carry their display name as the key; we can't list
        // them by name, so detect by value (same heuristic the request-panel
        // base-URL dropdown uses).
        let is_url_value = val.starts_with("http://") || val.starts_with("https://");
        if (is_legacy_name || is_url_value) && !val.is_empty() {
            Some(v.value.clone())
        } else {
            None
        }
    })
}

/// Build the postman JSON document (`serde_json::Value`) from a Verve project.
fn build_postman_doc(project: &Project) -> serde_json::Value {
    use serde_json::{Map, json};

    // --- global.envs ---
    let envs: Vec<serde_json::Value> = project
        .environments
        .iter()
        .enumerate()
        .map(|(i, env)| {
            let mut vars = Map::new();
            // Pull the server URI out into server_list, leaving the rest of the
            // env vars in env_var_list. A "server" is any variable whose value
            // is an http(s) URL OR is named baseUrl/base_url (legacy).
            let mut server_uri = String::new();
            for v in &env.variables {
                let is_legacy_name = v
                    .key
                    .eq_ignore_ascii_case("baseurl")
                    || v.key.eq_ignore_ascii_case("base_url");
                let val = v.value.trim();
                let is_url_value = val.starts_with("http://") || val.starts_with("https://");
                if server_uri.is_empty() && (is_legacy_name || is_url_value) && !val.is_empty() {
                    server_uri = v.value.clone();
                } else {
                    vars.insert(v.key.clone(), json!(v.value));
                }
            }
            json!({
                "env_id": (i + 1).to_string(),
                "name": env.name,
                "is_private": -1,
                "sort": i,
                "server_list": [{
                    "server_id": "1",
                    "name": "默认服务",
                    "sort": 0,
                    "uri": server_uri
                }],
                "env_var_list": vars,
            })
        })
        .collect();

    // --- global.global_param ---
    let global_param = json!({
        "header": json!({ "parameter": kvs_to_postman_params(&project.global_headers) }),
        "query": json!({ "parameter": kvs_to_postman_params(&project.global_params) }),
        "body": json!({ "parameter": [] }),
        "cookie": json!({ "parameter": [] }),
        "auth": empty_postman_auth(),
        "pre_tasks": [],
        "post_tasks": [],
    });

    // --- apis[] (flat list of folders + apis with parent_id) ---
    let mut apis: Vec<serde_json::Value> = Vec::new();

    // Root-level requests (parent_id = "0").
    for req in &project.requests {
        apis.push(api_to_postman_node(req, "0"));
    }
    // Walk folders recursively, emitting folder nodes + their request children.
    for folder in &project.folders {
        folder_to_postman_nodes(folder, "0", &mut apis);
    }

    json!({
        "project_id": &project.id[..6.min(project.id.len())],
        "name": project.name,
        "intro": project.description,
        "global": {
            "envs": envs,
            "servers": [{ "server_id": "1", "name": "默认服务", "sort": 1000 }],
            "global_vars": {},
            "global_param": global_param,
            "codes": [],
            "marks": [
                { "mark_id": "1", "name": "开发中", "color": "#2857FF", "is_sys_default": 1, "is_default_mark": 1 },
                { "mark_id": "2", "name": "已完成", "color": "#26CEA4", "is_sys_default": 1 },
                { "mark_id": "3", "name": "需修改", "color": "#FFC01E", "is_sys_default": 1 },
                { "mark_id": "4", "name": "已废弃", "color": "#FF2200", "is_sys_default": 1 },
            ],
            "attributes": [],
            "mock_custom_rules": [],
            "db_link": [],
            "describe_library": [],
            "custom_func": [],
        },
        "models": [],
        "apis": apis,
        "samples": [],
        "automated_testings": [],
    })
}

/// Recursively emit a folder node + its API children + sub-folder nodes.
fn folder_to_postman_nodes(folder: &Folder, parent_id: &str, out: &mut Vec<serde_json::Value>) {
    let folder_id = format!("folder_{}", &folder.id[..8.min(folder.id.len())]);
    // Map folder.base_url → server_id:
    // - Some("{{<server name>}}") (any server placeholder) → server_id "1"
    //   (the single default server we emit in `global.servers`).
    // - None / empty / non-placeholder → server_id "0" (inherit from parent).
    let server_id = match &folder.base_url {
        Some(url) => {
            let t = url.trim();
            if t.starts_with("{{") && t.ends_with("}}") && t.len() > 4 {
                "1"
            } else {
                "0"
            }
        }
        _ => "0",
    };
    out.push(json!({
        "target_id": folder_id,
        "parent_id": parent_id,
        "target_type": "folder",
        "name": folder.name,
        "description": folder.description,
        "server_id": server_id,
    }));
    for req in &folder.requests {
        out.push(api_to_postman_node(req, &folder_id));
    }
    for sub in &folder.folders {
        folder_to_postman_nodes(sub, &folder_id, out);
    }
}

/// Convert a Verve ApiRequest into an postman API node.
fn api_to_postman_node(req: &ApiRequest, parent_id: &str) -> serde_json::Value {
    use crate::state::models::Protocol;

    // Map Verve Protocol → postman target_type. postman distinguishes protocol
    // kinds via target_type ("api" = HTTP, "socketio", "sse", "websocket2").
    let (target_type, protocol_str) = match req.protocol {
        Protocol::SocketIo => ("socketio", ""),
        Protocol::Sse => ("sse", "http/1.1"),
        Protocol::WebSocket => ("websocket2", ""),
        _ => ("api", "http/1.1"),
    };

    let mark_id = match req.status.as_str() {
        "已完成" => "2",
        "需修改" => "3",
        "已废弃" => "4",
        _ => "1", // "开发中" or empty
    };

    json!({
        "target_id": req.id,
        "parent_id": parent_id,
        "target_type": target_type,
        "name": req.name,
        "method": method_label(req.method),
        "url": req.url,
        "protocol": protocol_str,
        "mark_id": mark_id,
        "description": req.description,
        "request": {
            "auth": auth_to_postman(&req.auth),
            "body": body_to_postman(&req.body),
            "header": json!({ "parameter": kvs_to_postman_params(&req.headers) }),
            "query": {
                "query_add_equal": 1,
                "parameter": kvs_to_postman_params(&req.params),
            },
            "cookie": {
                "cookie_encode": 1,
                "parameter": kvs_to_postman_params(&req.cookies),
            },
            "restful": json!({ "parameter": kvs_to_postman_params(&req.path) }),
            "pre_tasks": [],
            "post_tasks": [],
        },
        "response": { "example": [], "is_check_result": 1 },
        "mock_server_enable": -1,
        "tags": req.tags.iter().map(|t| json!(t)).collect::<Vec<_>>(),
    })
}

/// Convert a slice of KeyValue into postman parameter objects.
fn kvs_to_postman_params(kvs: &[KeyValue]) -> Vec<serde_json::Value> {
    kvs.iter()
        .map(|kv| {
            json!({
                "param_id": crate::state::models::new_id(),
                "key": kv.key,
                "value": kv.value,
                "description": kv.description,
                "field_type": field_type_to_str(kv.field_type),
                "is_checked": if kv.enabled { 1 } else { -1 },
                "not_null": if kv.required { 1 } else { -1 },
                "content_type": "",
                "file_name": kv.file_path.as_deref().unwrap_or(""),
                "file_base64": "",
                "schema": { "type": field_type_to_str(kv.field_type) },
            })
        })
        .collect()
}

/// Map Verve FieldType → postman field_type string.
fn field_type_to_str(ft: crate::state::models::FieldType) -> &'static str {
    use crate::state::models::FieldType;
    match ft {
        FieldType::Text => "string",
        FieldType::File => "file",
        FieldType::Number => "number",
        FieldType::Bool => "boolean",
        FieldType::Array => "array",
        FieldType::Decimal => "decimal",
        FieldType::Object => "object",
    }
}

/// Convert a Verve RequestBody into the postman body object.
fn body_to_postman(body: &crate::state::models::RequestBody) -> serde_json::Value {
    match body.body_type {
        BodyType::None => json!({
            "mode": "none",
            "parameter": [],
            "raw": "",
            "raw_parameter": [],
            "binary": {},
        }),
        BodyType::FormData => json!({
            "mode": "form-data",
            "parameter": kvs_to_postman_params(&body.form_data),
            "raw": "",
            "raw_parameter": [],
            "binary": {},
        }),
        BodyType::Urlencoded => json!({
            "mode": "urlencoded",
            "parameter": kvs_to_postman_params(&body.urlencoded),
            "raw": "",
            "raw_parameter": [],
            "binary": {},
        }),
        BodyType::Raw => {
            // postman uses "json" (not "raw") as the mode for JSON bodies —
            // the dominant case. Map the language back to postman's mode names.
            let mode = match body.raw_language {
                crate::state::models::RawLanguage::Json => "json",
                crate::state::models::RawLanguage::Xml => "xml",
                crate::state::models::RawLanguage::Text => "text",
                crate::state::models::RawLanguage::Html => "html",
                crate::state::models::RawLanguage::Javascript => "javascript",
            };
            // Serialize visual field descriptions (raw_parameter) for JSON bodies.
            let raw_parameter: Vec<serde_json::Value> = body
                .raw_parameter
                .iter()
                .flat_map(|kv| kvs_to_postman_params(std::slice::from_ref(kv)))
                .collect();
            json!({
                "mode": mode,
                "parameter": [],
                "raw": body.raw,
                "raw_parameter": raw_parameter,
                "raw_schema": { "type": "object" },
                "binary": {},
            })
        }
    }
}

/// Convert a Verve AuthConfig into the postman auth object.
fn auth_to_postman(auth: &AuthConfig) -> serde_json::Value {
    match auth.auth_type {
        AuthType::Bearer => json!({
            "type": "bearer",
            "bearer": { "key": auth.token },
            "kv": { "key": "", "value": "", "in": "header" },
            "basic": { "username": "", "password": "" },
        }),
        AuthType::Basic => json!({
            "type": "basic",
            "basic": { "username": auth.username, "password": auth.password },
            "bearer": { "key": "" },
            "kv": { "key": "", "value": "", "in": "header" },
        }),
        AuthType::ApiKey => json!({
            "type": "apikey",
            "kv": {
                "key": auth.key,
                "value": auth.value,
                "in": if auth.add_to == crate::state::models::AuthTarget::Query { "query" } else { "header" },
            },
            "bearer": { "key": "" },
            "basic": { "username": "", "password": "" },
        }),
        AuthType::None => json!({
            "type": "noauth",
            "bearer": { "key": "" },
            "kv": { "key": "", "value": "", "in": "header" },
            "basic": { "username": "", "password": "" },
        }),
    }
}

/// An empty postman auth object (for global_param where type = "").
fn empty_postman_auth() -> serde_json::Value {
    json!({
        "type": "",
        "kv": { "key": "", "value": "", "in": "" },
        "bearer": { "key": "" },
        "basic": { "username": "", "password": "" },
    })
}

// ===========================================================================
// OpenAPI 3.0 export
// ===========================================================================

/// Render a project as an OpenAPI 3.0.3 JSON document.
///
/// Output is intentionally best-effort: paths/methods/parameters/request body/
/// auth are mapped; response schemas are not inferred beyond a generic 200
/// example, because Verve stores last_response per-request but not multiple
/// named examples. Import via the existing `import::openapi_v3` round-trips.
pub fn project_to_openapi(project: &Project) -> serde_json::Result<String> {
    let doc = build_openapi_doc(project);
    serde_json::to_string_pretty(&doc)
}

fn build_openapi_doc(project: &Project) -> serde_json::Value {
    use serde_json::{Map, Value};

    // -- servers from the active environment's server URL --
    // A "server" is any env var holding an http(s) URL (covers both legacy
    // `baseUrl` variables and imported servers keyed by display name).
    let mut servers = Vec::new();
    if let Some(env) = project
        .active_environment
        .as_ref()
        .and_then(|id| project.environments.iter().find(|e| &e.id == id))
    {
        if let Some(url) = env_server_url(env) {
            if !url.trim().is_empty() {
                servers.push(json!({ "url": url, "description": env.name }));
            }
        }
    }
    if servers.is_empty() {
        // Fallback: scan all environments.
        for env in &project.environments {
            if let Some(url) = env_server_url(env) {
                if !url.trim().is_empty() {
                    servers.push(json!({ "url": url, "description": env.name }));
                }
            }
        }
    }

    // -- paths --
    let mut paths: Map<String, Value> = Map::new();
    let mut has_bearer = false;
    let mut has_basic = false;
    let mut has_apikey_header = false;
    let mut has_apikey_query = false;

    // Collect a flat list of all requests with folder path prefix so we can
    // build OpenAPI tags for top-level folders.
    let mut all_requests: Vec<&ApiRequest> = Vec::new();
    for r in &project.requests {
        all_requests.push(r);
    }
    for f in &project.folders {
        collect_folder_requests(f, &mut all_requests);
    }

    for req in &all_requests {
        let entry = openapi_for_request(
            req,
            &mut has_bearer,
            &mut has_basic,
            &mut has_apikey_header,
            &mut has_apikey_query,
        );
        let (path_str, method, op) = match entry {
            Some(v) => v,
            None => continue,
        };

        // Split {baseUrl}/foo → /foo (strip scheme+host+vars at start).
        let clean = openapi_path_from_url(&path_str);
        let path_item = paths.entry(clean).or_insert_with(|| json!({}));
        if let Some(obj) = path_item.as_object_mut() {
            obj.insert(method.to_lowercase(), op);
        }
    }

    // -- components.securitySchemes --
    let mut sec_schemes: Map<String, Value> = Map::new();
    if has_bearer {
        sec_schemes.insert(
            "bearerAuth".into(),
            json!({ "type": "http", "scheme": "bearer", "bearerFormat": "JWT" }),
        );
    }
    if has_basic {
        sec_schemes.insert(
            "basicAuth".into(),
            json!({ "type": "http", "scheme": "basic" }),
        );
    }
    if has_apikey_header {
        sec_schemes.insert(
            "apiKeyHeader".into(),
            json!({ "type": "apiKey", "in": "header", "name": "X-API-Key" }),
        );
    }
    if has_apikey_query {
        sec_schemes.insert(
            "apiKeyQuery".into(),
            json!({ "type": "apiKey", "in": "query", "name": "api_key" }),
        );
    }

    let components = if sec_schemes.is_empty() {
        Value::Null
    } else {
        json!({ "securitySchemes": Value::Object(sec_schemes) })
    };

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": project.name,
            "version": "1.0.0",
            "description": project.description,
        },
        "servers": servers,
        "paths": Value::Object(paths),
        "components": components,
    })
}

fn collect_folder_requests<'a>(folder: &'a Folder, out: &mut Vec<&'a ApiRequest>) {
    for r in &folder.requests {
        out.push(r);
    }
    for sub in &folder.folders {
        collect_folder_requests(sub, out);
    }
}

/// Convert a Verve URL like "{{baseUrl}}/users/{id}" to an OpenAPI path "/users/{id}".
fn openapi_path_from_url(url: &str) -> String {
    let s = url.trim();
    // Strip scheme://host if present (best effort).
    let after_host = if let Some(idx) = s.find("://") {
        let rest = &s[idx + 3..];
        match rest.find('/') {
            Some(slash) => &rest[slash..],
            None => "/",
        }
    } else {
        s
    };
    // Strip leading {{...}}/ prefix if any (variable like {{baseUrl}}).
    let mut path = after_host.to_string();
    while let Some(rest) = path.strip_prefix("{{") {
        if let Some(end) = rest.find("}}") {
            let after = &rest[end + 2..];
            let trimmed = after.trim_start_matches('/');
            path = format!("/{}", trimmed);
        } else {
            break;
        }
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    // Collapse duplicate slashes.
    while path.contains("//") {
        path = path.replace("//", "/");
    }
    path
}

/// Returns (path_string, method_lowercase, operation_object) or None for
/// non-HTTP protocols we can't represent as OpenAPI.
fn openapi_for_request(
    req: &ApiRequest,
    has_bearer: &mut bool,
    has_basic: &mut bool,
    has_apikey_header: &mut bool,
    has_apikey_query: &mut bool,
) -> Option<(String, String, serde_json::Value)> {
    use crate::state::models::Protocol;
    // Only emit HTTP-family; gRPC/SSE/WS/TCP don't fit OpenAPI.
    match req.protocol {
        Protocol::Http | Protocol::Graphql => {}
        _ => return None,
    }

    let mut parameters: Vec<serde_json::Value> = Vec::new();

    // Query params.
    for kv in req.params.iter().filter(|kv| kv.enabled) {
        parameters.push(json!({
            "name": kv.key,
            "in": "query",
            "required": kv.required,
            "description": kv.description,
            "schema": { "type": openapi_field_type(kv.field_type), "example": kv.value },
        }));
    }
    // Headers (skip host/content-type which are handled elsewhere).
    for kv in req.headers.iter().filter(|kv| kv.enabled) {
        if kv.key.eq_ignore_ascii_case("content-type")
            || kv.key.eq_ignore_ascii_case("host")
            || kv.key.is_empty()
        {
            continue;
        }
        parameters.push(json!({
            "name": kv.key,
            "in": "header",
            "required": kv.required,
            "description": kv.description,
            "schema": { "type": openapi_field_type(kv.field_type), "example": kv.value },
        }));
    }
    // Path variables — OpenAPI uses {name}; our path-var KeyValue list informs
    // required/description but the URL already contains {name}.
    for kv in req.path.iter().filter(|kv| kv.enabled) {
        parameters.push(json!({
            "name": kv.key,
            "in": "path",
            "required": true,
            "description": kv.description,
            "schema": { "type": openapi_field_type(kv.field_type), "example": kv.value },
        }));
    }

    // Request body.
    let request_body = openapi_request_body(req);

    // Auth → security.
    let mut security: Vec<serde_json::Value> = Vec::new();
    match req.auth.auth_type {
        AuthType::Bearer => {
            *has_bearer = true;
            security.push(json!({ "bearerAuth": [] }));
        }
        AuthType::Basic => {
            *has_basic = true;
            security.push(json!({ "basicAuth": [] }));
        }
        AuthType::ApiKey => {
            if req.auth.add_to == crate::state::models::AuthTarget::Query {
                *has_apikey_query = true;
                security.push(json!({ "apiKeyQuery": [] }));
            } else {
                *has_apikey_header = true;
                security.push(json!({ "apiKeyHeader": [] }));
            }
        }
        AuthType::None => {}
    }

    // Tags: use request.tags (fallback empty).
    let tags: Vec<serde_json::Value> = req.tags.iter().map(|t| json!(t)).collect();

    let mut op = json!({
        "summary": req.name,
        "description": req.description,
        "tags": tags,
        "responses": {
            "200": {
                "description": "Successful response",
            }
        },
    });
    let obj = op.as_object_mut().unwrap();
    if !parameters.is_empty() {
        obj.insert("parameters".into(), json!(parameters));
    }
    if let Some(rb) = request_body {
        obj.insert("requestBody".into(), rb);
    }
    if !security.is_empty() {
        obj.insert("security".into(), json!(security));
    }

    Some((req.url.clone(), method_label(req.method).to_string(), op))
}

fn openapi_field_type(ft: crate::state::models::FieldType) -> &'static str {
    use crate::state::models::FieldType;
    match ft {
        FieldType::Text => "string",
        FieldType::Number => "number",
        FieldType::Decimal => "number",
        FieldType::Bool => "boolean",
        FieldType::Array => "array",
        FieldType::Object => "object",
        FieldType::File => "string",
    }
}

fn openapi_request_body(req: &ApiRequest) -> Option<serde_json::Value> {
    match req.body.body_type {
        BodyType::None => None,
        BodyType::Raw => {
            let ct = match req.body.raw_language {
                crate::state::models::RawLanguage::Json => "application/json",
                crate::state::models::RawLanguage::Xml => "application/xml",
                crate::state::models::RawLanguage::Html => "text/html",
                crate::state::models::RawLanguage::Javascript => "application/javascript",
                crate::state::models::RawLanguage::Text => "text/plain",
            };
            Some(json!({
                "required": true,
                "content": {
                    ct: {
                        "schema": { "type": "string", "example": req.body.raw }
                    }
                }
            }))
        }
        BodyType::Urlencoded => {
            let mut props = serde_json::Map::new();
            for kv in req.body.urlencoded.iter().filter(|kv| kv.enabled) {
                props.insert(
                    kv.key.clone(),
                    json!({ "type": openapi_field_type(kv.field_type), "description": kv.description, "example": kv.value }),
                );
            }
            Some(json!({
                "required": true,
                "content": {
                    "application/x-www-form-urlencoded": {
                        "schema": { "type": "object", "properties": props }
                    }
                }
            }))
        }
        BodyType::FormData => {
            let mut props = serde_json::Map::new();
            for kv in req.body.form_data.iter().filter(|kv| kv.enabled) {
                if kv.field_type == crate::state::models::FieldType::File
                    || kv.file_path.as_deref().map_or(false, |s| !s.is_empty())
                {
                    props.insert(
                        kv.key.clone(),
                        json!({ "type": "string", "format": "binary", "description": kv.description }),
                    );
                } else {
                    props.insert(
                        kv.key.clone(),
                        json!({ "type": openapi_field_type(kv.field_type), "description": kv.description, "example": kv.value }),
                    );
                }
            }
            Some(json!({
                "required": true,
                "content": {
                    "multipart/form-data": {
                        "schema": { "type": "object", "properties": props }
                    }
                }
            }))
        }
    }
}

// ===========================================================================
// Postman Collection v2.1.0 export
// ===========================================================================

/// Render a project as a standard Postman Collection v2.1.0 JSON document.
///
/// Schema: https://schema.getpostman.com/json/collection/v2.1.0/collection.json
pub fn project_to_postman_v2_1(project: &Project) -> serde_json::Result<String> {
    let doc = build_postman_v21_doc(project);
    serde_json::to_string_pretty(&doc)
}

fn build_postman_v21_doc(project: &Project) -> serde_json::Value {
    use serde_json::{Map, json};

    // Convert environments to collection variables.
    let mut variables = Vec::new();
    for env in &project.environments {
        for v in &env.variables {
            variables.push(json!({
                "key": v.key,
                "value": v.value,
                "type": field_type_to_str(v.field_type),
            }));
        }
    }

    // Build item tree: root requests + folders as item groups.
    let mut items = Vec::new();
    for req in &project.requests {
        items.push(postman_v21_request_item(req));
    }
    for folder in &project.folders {
        items.push(postman_v21_folder_item(folder));
    }

    json!({
        "info": {
            "name": project.name,
            "_postman_id": crate::state::models::new_id(),
            "description": project.description,
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
        },
        "item": items,
        "variable": variables,
    })
}

fn postman_v21_folder_item(folder: &Folder) -> serde_json::Value {
    let mut items = Vec::new();
    for req in &folder.requests {
        items.push(postman_v21_request_item(req));
    }
    for sub in &folder.folders {
        items.push(postman_v21_folder_item(sub));
    }
    json!({
        "name": folder.name,
        "description": folder.description,
        "item": items,
    })
}

fn postman_v21_request_item(req: &ApiRequest) -> serde_json::Value {
    use crate::state::models::Protocol;

    // Skip non-HTTP protocols (WebSocket/SSE/gRPC don't map to Postman v2.1 cleanly).
    if matches!(
        req.protocol,
        Protocol::WebSocket | Protocol::Sse | Protocol::SocketIo
    ) {
        return json!({
            "name": req.name,
            "request": { "method": method_label(req.method), "url": req.url }
        });
    }

    let header = kvs_to_postman_v21_headers(&req.headers);
    let query = kvs_to_postman_v21_params(&req.params);

    // Decompose URL into host/path for richer Postman representation.
    let url_obj = postman_v21_url(&req.url, &req.params);

    let body = postman_v21_body(&req.body);

    json!({
        "name": req.name,
        "description": req.description,
        "request": {
            "method": method_label(req.method),
            "header": header,
            "url": url_obj,
            "body": body,
            "auth": postman_v21_auth(&req.auth),
        },
        "response": [],
    })
}

fn postman_v21_url(url: &str, params: &[KeyValue]) -> serde_json::Value {
    // Try to decompose into host/path; if it has variables ({{...}}) or is
    // simple, keep the raw form.
    let raw = url.to_string();
    let after_host = if let Some(idx) = raw.find("://") {
        &raw[idx + 3..]
    } else {
        &raw
    };
    let (host_part, path_part) = match after_host.find('/') {
        Some(slash) => (&after_host[..slash], &after_host[slash..]),
        None => (after_host, "/"),
    };
    let host: Vec<&str> = host_part.split('.').collect();
    let path: Vec<&str> = path_part.split('/').filter(|s| !s.is_empty()).collect();

    let query: Vec<serde_json::Value> = params
        .iter()
        .filter(|kv| kv.enabled)
        .map(|kv| json!({ "key": kv.key, "value": kv.value }))
        .collect();

    json!({
        "raw": raw,
        "host": host,
        "path": path,
        "query": query,
    })
}

fn kvs_to_postman_v21_headers(kvs: &[KeyValue]) -> Vec<serde_json::Value> {
    kvs.iter()
        .filter(|kv| kv.enabled)
        .map(|kv| {
            json!({
                "key": kv.key,
                "value": kv.value,
                "description": kv.description,
                "disabled": !kv.enabled,
            })
        })
        .collect()
}

fn kvs_to_postman_v21_params(kvs: &[KeyValue]) -> Vec<serde_json::Value> {
    kvs.iter()
        .map(|kv| {
            json!({
                "key": kv.key,
                "value": kv.value,
                "disabled": !kv.enabled,
            })
        })
        .collect()
}

fn postman_v21_body(body: &crate::state::models::RequestBody) -> serde_json::Value {
    match body.body_type {
        BodyType::None => json!({ "mode": "none" }),
        BodyType::FormData => {
            let formdata: Vec<serde_json::Value> = body
                .form_data
                .iter()
                .filter(|kv| kv.enabled)
                .map(|kv| {
                    json!({
                        "key": kv.key,
                        "value": kv.value,
                        "type": if kv.field_type == crate::state::models::FieldType::File { "file" } else { "text" },
                        "src": kv.file_path.as_deref().unwrap_or(""),
                    })
                })
                .collect();
            json!({ "mode": "formdata", "formdata": formdata })
        }
        BodyType::Urlencoded => {
            let urlencoded: Vec<serde_json::Value> = body
                .urlencoded
                .iter()
                .filter(|kv| kv.enabled)
                .map(|kv| json!({ "key": kv.key, "value": kv.value }))
                .collect();
            json!({ "mode": "urlencoded", "urlencoded": urlencoded })
        }
        BodyType::Raw => {
            let language = match body.raw_language {
                crate::state::models::RawLanguage::Json => "json",
                crate::state::models::RawLanguage::Xml => "xml",
                crate::state::models::RawLanguage::Html => "html",
                crate::state::models::RawLanguage::Javascript => "javascript",
                crate::state::models::RawLanguage::Text => "text",
            };
            json!({
                "mode": "raw",
                "raw": body.raw,
                "options": {
                    "raw": { "language": language }
                }
            })
        }
    }
}

fn postman_v21_auth(auth: &AuthConfig) -> serde_json::Value {
    match auth.auth_type {
        AuthType::Bearer => json!({
            "type": "bearer",
            "bearer": [{ "key": "token", "value": auth.token, "type": "string" }]
        }),
        AuthType::Basic => json!({
            "type": "basic",
            "basic": [
                { "key": "username", "value": auth.username, "type": "string" },
                { "key": "password", "value": auth.password, "type": "string" }
            ]
        }),
        AuthType::ApiKey => json!({
            "type": "apikey",
            "apikey": [
                { "key": "key", "value": auth.key, "type": "string" },
                { "key": "value", "value": auth.value, "type": "string" },
                { "key": "in", "value": if auth.add_to == crate::state::models::AuthTarget::Query { "query" } else { "header" }, "type": "string" }
            ]
        }),
        AuthType::None => json!({ "type": "noauth" }),
    }
}

// ===========================================================================
// Swagger / OpenAPI 2.0 export
// ===========================================================================

/// Render a project as a Swagger / OpenAPI 2.0 JSON document.
pub fn project_to_swagger(project: &Project) -> serde_json::Result<String> {
    let doc = build_swagger_doc(project);
    serde_json::to_string_pretty(&doc)
}

fn build_swagger_doc(project: &Project) -> serde_json::Value {
    use serde_json::{Map, Value};

    // -- servers: extract host/basePath/schemes from the server URL --
    let mut host = String::new();
    let mut base_path = "/".to_string();
    let mut schemes = vec!["https".to_string()];

    if let Some(env) = project
        .active_environment
        .as_ref()
        .and_then(|id| project.environments.iter().find(|e| &e.id == id))
    {
        if let Some(url) = env_server_url(env) {
            parse_swagger_url(&url, &mut host, &mut base_path, &mut schemes);
        }
    }
    if host.is_empty() {
        for env in &project.environments {
            if let Some(url) = env_server_url(env) {
                if !url.trim().is_empty() {
                    parse_swagger_url(&url, &mut host, &mut base_path, &mut schemes);
                }
            }
        }
    }

    // -- paths --
    let mut paths: Map<String, Value> = Map::new();
    let mut has_bearer = false;
    let mut has_basic = false;
    let mut has_apikey = false;

    let mut all_requests: Vec<&ApiRequest> = Vec::new();
    for r in &project.requests {
        all_requests.push(r);
    }
    for f in &project.folders {
        collect_folder_requests(f, &mut all_requests);
    }

    for req in &all_requests {
        let entry = swagger_for_request(req, &mut has_bearer, &mut has_basic, &mut has_apikey);
        let (path_str, method, op) = match entry {
            Some(v) => v,
            None => continue,
        };
        let clean = openapi_path_from_url(&path_str);
        let path_item = paths.entry(clean).or_insert_with(|| json!({}));
        if let Some(obj) = path_item.as_object_mut() {
            obj.insert(method.to_lowercase(), op);
        }
    }

    // -- securityDefinitions --
    let mut sec_defs: Map<String, Value> = Map::new();
    if has_bearer {
        // Swagger 2.0 has no "http/bearer"; emulate via apiKey named Authorization.
        sec_defs.insert(
            "bearerAuth".into(),
            json!({ "type": "apiKey", "name": "Authorization", "in": "header" }),
        );
    }
    if has_basic {
        sec_defs.insert("basicAuth".into(), json!({ "type": "basic" }));
    }
    if has_apikey {
        sec_defs.insert(
            "apiKeyHeader".into(),
            json!({ "type": "apiKey", "name": "X-API-Key", "in": "header" }),
        );
    }

    json!({
        "swagger": "2.0",
        "info": {
            "title": project.name,
            "version": "1.0.0",
            "description": project.description,
        },
        "host": host,
        "basePath": base_path,
        "schemes": schemes,
        "paths": Value::Object(paths),
        "securityDefinitions": if sec_defs.is_empty() { Value::Null } else { Value::Object(sec_defs) },
    })
}

fn parse_swagger_url(
    url: &str,
    host: &mut String,
    base_path: &mut String,
    schemes: &mut Vec<String>,
) {
    let s = url.trim().trim_end_matches('/');
    let (scheme, rest) = if let Some(idx) = s.find("://") {
        let sch = s[..idx].to_lowercase();
        (sch, &s[idx + 3..])
    } else {
        ("https".to_string(), s)
    };
    *schemes = vec![scheme];
    match rest.find('/') {
        Some(slash) => {
            *host = rest[..slash].to_string();
            *base_path = rest[slash..].to_string();
            if base_path.is_empty() {
                *base_path = "/".to_string();
            }
        }
        None => {
            *host = rest.to_string();
        }
    }
}

fn swagger_for_request(
    req: &ApiRequest,
    has_bearer: &mut bool,
    has_basic: &mut bool,
    has_apikey: &mut bool,
) -> Option<(String, String, serde_json::Value)> {
    use crate::state::models::Protocol;
    match req.protocol {
        Protocol::Http | Protocol::Graphql => {}
        _ => return None,
    }

    let mut parameters: Vec<serde_json::Value> = Vec::new();

    for kv in req.params.iter().filter(|kv| kv.enabled) {
        parameters.push(json!({
            "name": kv.key,
            "in": "query",
            "required": kv.required,
            "description": kv.description,
            "type": openapi_field_type(kv.field_type),
        }));
    }
    for kv in req.headers.iter().filter(|kv| kv.enabled) {
        if kv.key.eq_ignore_ascii_case("content-type")
            || kv.key.eq_ignore_ascii_case("host")
            || kv.key.is_empty()
        {
            continue;
        }
        parameters.push(json!({
            "name": kv.key,
            "in": "header",
            "required": kv.required,
            "description": kv.description,
            "type": openapi_field_type(kv.field_type),
        }));
    }
    for kv in req.path.iter().filter(|kv| kv.enabled) {
        parameters.push(json!({
            "name": kv.key,
            "in": "path",
            "required": true,
            "description": kv.description,
            "type": openapi_field_type(kv.field_type),
        }));
    }

    // Swagger 2.0 body parameter.
    let body_param = swagger_body_parameter(req);
    if let Some(p) = body_param {
        parameters.push(p);
    }

    let mut security: Vec<serde_json::Value> = Vec::new();
    match req.auth.auth_type {
        AuthType::Bearer => {
            *has_bearer = true;
            security.push(json!({ "bearerAuth": [] }));
        }
        AuthType::Basic => {
            *has_basic = true;
            security.push(json!({ "basicAuth": [] }));
        }
        AuthType::ApiKey => {
            *has_apikey = true;
            security.push(json!({ "apiKeyHeader": [] }));
        }
        AuthType::None => {}
    }

    let tags: Vec<serde_json::Value> = req.tags.iter().map(|t| json!(t)).collect();

    let mut op = json!({
        "summary": req.name,
        "description": req.description,
        "tags": tags,
        "responses": {
            "200": { "description": "Successful response" }
        },
    });
    let obj = op.as_object_mut().unwrap();
    if !parameters.is_empty() {
        obj.insert("parameters".into(), json!(parameters));
    }
    if !security.is_empty() {
        obj.insert("security".into(), json!(security));
    }

    Some((req.url.clone(), method_label(req.method).to_string(), op))
}

fn swagger_body_parameter(req: &ApiRequest) -> Option<serde_json::Value> {
    match req.body.body_type {
        BodyType::None => None,
        BodyType::Raw => Some(json!({
            "name": "body",
            "in": "body",
            "required": true,
            "schema": { "type": "string", "example": req.body.raw }
        })),
        BodyType::Urlencoded => {
            // Swagger 2.0 uses in: formData for urlencoded params.
            // Emit one formData parameter per key as a simpler alternative to a body schema.
            None // Callers can add formData params directly if needed; keep simple.
        }
        BodyType::FormData => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::models::{ApiRequest, KeyValue, RequestBody};

    #[test]
    fn markdown_export() {
        let mut project = Project::new("Demo");
        let mut req = ApiRequest::new("Login", RequestMethod::Post, "{{baseUrl}}/login");
        req.description = "Logs a user in.".into();
        req.body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: crate::state::models::RawLanguage::Json,
            raw: r#"{"u":"x"}"#.into(),
            ..Default::default()
        };
        req.params.push(KeyValue::new("keep", "1"));
        project.requests.push(req);
        let md = project_to_markdown(&project);
        assert!(md.contains("# Demo"));
        assert!(md.contains("`POST` Login"));
        assert!(md.contains("{{baseUrl}}/login"));
        assert!(md.contains("keep"));
        assert!(md.contains("```json"));
    }

    #[test]
    fn postman_export_round_trip() {
        // Build a project with: 2 root requests (one with form-data + bearer,
        // one with raw body), 1 folder containing 1 request, an environment
        // with variables, and a global header.
        let mut project = Project::new("测试项目");
        project.description = "项目描述".into();

        // Environment.
        let mut env = crate::state::models::Environment::new("测试环境");
        env.variables
            .push(KeyValue::new("baseUrl", "https://api.test"));
        project.environments.push(env);

        // Global header.
        project.global_headers.push(KeyValue::new("X-Trace", "abc"));

        // Root API with form-data body + bearer auth.
        let mut req1 = ApiRequest::new("登录", RequestMethod::Post, "{{baseUrl}}/login");
        req1.status = "开发中".into();
        req1.tags = vec!["v1".into()];
        req1.body = RequestBody {
            body_type: BodyType::FormData,
            form_data: vec![
                KeyValue::new("username", "admin"),
                KeyValue::new("password", "123"),
            ],
            ..Default::default()
        };
        req1.auth = AuthConfig {
            auth_type: AuthType::Bearer,
            token: "tok123".into(),
            ..Default::default()
        };
        req1.headers
            .push(KeyValue::new("Content-Type", "multipart/form-data"));
        project.requests.push(req1);

        // Root API with raw body, no auth.
        let mut req2 = ApiRequest::new("查询", RequestMethod::Get, "{{baseUrl}}/list");
        req2.status = "已完成".into();
        req2.body = RequestBody {
            body_type: BodyType::Raw,
            raw: r#"{"page":1}"#.into(),
            ..Default::default()
        };
        req2.params.push(KeyValue::new("size", "10"));
        project.requests.push(req2);

        // Folder with nested API.
        let mut folder = Folder::new("用户模块");
        let mut req3 = ApiRequest::new("删除", RequestMethod::Delete, "{{baseUrl}}/user/1");
        req3.status = "需修改".into();
        req3.auth = AuthConfig {
            auth_type: AuthType::Basic,
            username: "u".into(),
            password: "p".into(),
            ..Default::default()
        };
        folder.requests.push(req3);
        project.folders.push(folder);

        // --- Export to Apipost JSON ---
        let json_str = project_to_apipost(&project).unwrap();
        assert!(!json_str.is_empty());
        assert!(json_str.contains("\"project_id\""));
        assert!(json_str.contains("\"target_type\""));

        // --- Re-import the exported JSON ---
        let reimported = crate::import::postman(&json_str).unwrap();

        // Project metadata preserved.
        assert_eq!(reimported.name, "测试项目");
        assert_eq!(reimported.description, "项目描述");

        // Environments preserved. The exported `baseUrl` server URL is moved
        // into `global.servers` / `server_list` on export, so on re-import it
        // comes back keyed by the server's display name ("默认服务"), not the
        // legacy "baseUrl" variable name.
        assert_eq!(reimported.environments.len(), 1);
        assert_eq!(reimported.environments[0].name, "测试环境");
        let server_var = reimported.environments[0]
            .variables
            .iter()
            .find(|v| v.key == "默认服务")
            .expect("server var should be keyed by display name 默认服务");
        assert_eq!(server_var.value, "https://api.test");

        // Global header preserved.
        assert_eq!(reimported.global_headers.len(), 1);
        assert_eq!(reimported.global_headers[0].key, "X-Trace");

        // Root requests preserved (2).
        assert_eq!(reimported.requests.len(), 2);
        let r1 = &reimported.requests[0];
        assert_eq!(r1.name, "登录");
        assert_eq!(r1.method, RequestMethod::Post);
        assert_eq!(r1.status, "开发中");
        assert_eq!(r1.tags, vec!["v1"]);
        assert_eq!(r1.body.body_type, BodyType::FormData);
        assert_eq!(r1.body.form_data.len(), 2);
        assert_eq!(r1.auth.auth_type, AuthType::Bearer);
        assert_eq!(r1.auth.token, "tok123");

        let r2 = &reimported.requests[1];
        assert_eq!(r2.name, "查询");
        assert_eq!(r2.method, RequestMethod::Get);
        assert_eq!(r2.status, "已完成");
        assert_eq!(r2.body.body_type, BodyType::Raw);
        assert_eq!(r2.body.raw, r#"{"page":1}"#);
        assert_eq!(r2.params.len(), 1);
        assert_eq!(r2.params[0].key, "size");

        // Folder + nested request preserved.
        assert_eq!(reimported.folders.len(), 1);
        let f = &reimported.folders[0];
        assert_eq!(f.name, "用户模块");
        assert_eq!(f.requests.len(), 1);
        let r3 = &f.requests[0];
        assert_eq!(r3.name, "删除");
        assert_eq!(r3.method, RequestMethod::Delete);
        assert_eq!(r3.status, "需修改");
        assert_eq!(r3.auth.auth_type, AuthType::Basic);
        assert_eq!(r3.auth.username, "u");
        assert_eq!(r3.auth.password, "p");
    }

    #[test]
    fn openapi_export_basic() {
        let mut project = Project::new("Demo API");
        project.description = "demo".into();
        let mut env = crate::state::models::Environment::new("dev");
        env.variables
            .push(KeyValue::new("baseUrl", "https://api.example.com"));
        project.environments.push(env);

        let mut req = ApiRequest::new("ListUsers", RequestMethod::Get, "{{baseUrl}}/users");
        req.params.push(KeyValue::new("page", "1"));
        req.params.push(KeyValue::new("size", "20"));
        req.headers.push(KeyValue::new("X-Trace", "t"));
        project.requests.push(req);

        let mut req2 = ApiRequest::new("CreateUser", RequestMethod::Post, "{{baseUrl}}/users");
        req2.auth = AuthConfig {
            auth_type: AuthType::Bearer,
            token: "abc".into(),
            ..Default::default()
        };
        req2.body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: crate::state::models::RawLanguage::Json,
            raw: r#"{"name":"x"}"#.into(),
            ..Default::default()
        };
        project.requests.push(req2);

        let s = project_to_openapi(&project).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();

        assert_eq!(v["openapi"], "3.0.3");
        assert_eq!(v["info"]["title"], "Demo API");
        assert_eq!(v["servers"][0]["url"], "https://api.example.com");
        // paths: /users exists with get + post.
        assert!(v["paths"]["/users"]["get"].is_object());
        assert!(v["paths"]["/users"]["post"].is_object());
        // query params present.
        let params = v["paths"]["/users"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params.len(), 3); // page, size, X-Trace header
        // bearer auth scheme present.
        assert!(v["components"]["securitySchemes"]["bearerAuth"].is_object());
        // post has security referencing bearerAuth.
        let sec = v["paths"]["/users"]["post"]["security"].as_array().unwrap();
        assert!(sec[0].get("bearerAuth").is_some());
    }

    #[test]
    fn openapi_path_cleaning() {
        assert_eq!(
            openapi_path_from_url("{{baseUrl}}/users/{id}"),
            "/users/{id}"
        );
        assert_eq!(
            openapi_path_from_url("https://example.com/foo/bar"),
            "/foo/bar"
        );
        assert_eq!(openapi_path_from_url("users/1"), "/users/1");
        assert_eq!(openapi_path_from_url("{{host}}/a//b"), "/a/b");
    }
}
