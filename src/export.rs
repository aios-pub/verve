//! Exporters: render a project to Markdown, JSON, or Apipost format.

use crate::state::models::{
    ApiRequest, AuthConfig, AuthType, BodyType, Folder, KeyValue, Project, RequestMethod,
};
use serde_json::json;

/// Export format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
    /// Apipost 7+ export JSON (round-trip compatible with `import::apipost`).
    Apipost,
}

impl Format {
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Json | Format::Apipost => "json",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Format::Markdown => "Markdown",
            Format::Json => "JSON",
            Format::Apipost => "Apipost",
        }
    }
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
// Apipost 7+ export format
// ===========================================================================

/// Render a project as an Apipost-compatible JSON string. The output is
/// round-trip compatible: importing it back via `import::apipost` reproduces
/// the same folder/request tree, bodies, params, and auth.
pub fn project_to_apipost(project: &Project) -> serde_json::Result<String> {
    let doc = build_apipost_doc(project);
    serde_json::to_string_pretty(&doc)
}

/// Build the Apipost JSON document (`serde_json::Value`) from a Verve project.
fn build_apipost_doc(project: &Project) -> serde_json::Value {
    use serde_json::{json, Map};

    // --- global.envs ---
    let envs: Vec<serde_json::Value> = project
        .environments
        .iter()
        .enumerate()
        .map(|(i, env)| {
            let mut vars = Map::new();
            for v in &env.variables {
                vars.insert(v.key.clone(), json!(v.value));
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
                    "uri": ""
                }],
                "env_var_list": vars,
            })
        })
        .collect();

    // --- global.global_param ---
    let global_param = json!({
        "header": json!({ "parameter": kvs_to_apipost_params(&project.global_headers) }),
        "query": json!({ "parameter": kvs_to_apipost_params(&project.global_params) }),
        "body": json!({ "parameter": [] }),
        "cookie": json!({ "parameter": [] }),
        "auth": empty_apipost_auth(),
        "pre_tasks": [],
        "post_tasks": [],
    });

    // --- apis[] (flat list of folders + apis with parent_id) ---
    let mut apis: Vec<serde_json::Value> = Vec::new();

    // Root-level requests (parent_id = "0").
    for req in &project.requests {
        apis.push(api_to_apipost_node(req, "0"));
    }
    // Walk folders recursively, emitting folder nodes + their request children.
    for folder in &project.folders {
        folder_to_apipost_nodes(folder, "0", &mut apis);
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
fn folder_to_apipost_nodes(folder: &Folder, parent_id: &str, out: &mut Vec<serde_json::Value>) {
    let folder_id = format!("folder_{}", &folder.id[..8.min(folder.id.len())]);
    out.push(json!({
        "target_id": folder_id,
        "parent_id": parent_id,
        "target_type": "folder",
        "name": folder.name,
        "description": folder.description,
    }));
    for req in &folder.requests {
        out.push(api_to_apipost_node(req, &folder_id));
    }
    for sub in &folder.folders {
        folder_to_apipost_nodes(sub, &folder_id, out);
    }
}

/// Convert a Verve ApiRequest into an Apipost API node.
fn api_to_apipost_node(req: &ApiRequest, parent_id: &str) -> serde_json::Value {
    use crate::state::models::Protocol;

    // Map Verve Protocol → Apipost target_type. Apipost distinguishes protocol
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
            "auth": auth_to_apipost(&req.auth),
            "body": body_to_apipost(&req.body),
            "header": json!({ "parameter": kvs_to_apipost_params(&req.headers) }),
            "query": {
                "query_add_equal": 1,
                "parameter": kvs_to_apipost_params(&req.params),
            },
            "cookie": {
                "cookie_encode": 1,
                "parameter": kvs_to_apipost_params(&req.cookies),
            },
            "restful": json!({ "parameter": kvs_to_apipost_params(&req.path) }),
            "pre_tasks": [],
            "post_tasks": [],
        },
        "response": { "example": [], "is_check_result": 1 },
        "mock_server_enable": -1,
        "tags": req.tags.iter().map(|t| json!(t)).collect::<Vec<_>>(),
    })
}

/// Convert a slice of KeyValue into Apipost parameter objects.
fn kvs_to_apipost_params(kvs: &[KeyValue]) -> Vec<serde_json::Value> {
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

/// Map Verve FieldType → Apipost field_type string.
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

/// Convert a Verve RequestBody into the Apipost body object.
fn body_to_apipost(body: &crate::state::models::RequestBody) -> serde_json::Value {
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
            "parameter": kvs_to_apipost_params(&body.form_data),
            "raw": "",
            "raw_parameter": [],
            "binary": {},
        }),
        BodyType::Urlencoded => json!({
            "mode": "urlencoded",
            "parameter": kvs_to_apipost_params(&body.urlencoded),
            "raw": "",
            "raw_parameter": [],
            "binary": {},
        }),
        BodyType::Raw => {
            // Apipost uses "json" (not "raw") as the mode for JSON bodies —
            // the dominant case. Map the language back to Apipost's mode names.
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
                .flat_map(|kv| kvs_to_apipost_params(std::slice::from_ref(kv)))
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

/// Convert a Verve AuthConfig into the Apipost auth object.
fn auth_to_apipost(auth: &AuthConfig) -> serde_json::Value {
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

/// An empty Apipost auth object (for global_param where type = "").
fn empty_apipost_auth() -> serde_json::Value {
    json!({
        "type": "",
        "kv": { "key": "", "value": "", "in": "" },
        "bearer": { "key": "" },
        "basic": { "username": "", "password": "" },
    })
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
    fn apipost_export_round_trip() {
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
        let reimported = crate::import::apipost(&json_str).unwrap();

        // Project metadata preserved.
        assert_eq!(reimported.name, "测试项目");
        assert_eq!(reimported.description, "项目描述");

        // Environments preserved.
        assert_eq!(reimported.environments.len(), 1);
        assert_eq!(reimported.environments[0].name, "测试环境");
        assert_eq!(reimported.environments[0].variables[0].key, "baseUrl");

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
}
