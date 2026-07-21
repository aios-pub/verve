//! Importers: convert external formats (Postman v2.1, OpenAPI 3, postman)
//! into the Verve project model.

use crate::state::models::*;

/// A minimal slice of a Postman v2.1 collection — enough to extract the
/// folder/request tree. Unknown fields are ignored.
#[derive(serde::Deserialize)]
struct PostmanCollection {
    info: Option<PostmanInfo>,
    item: Vec<PostmanItem>,
}

#[derive(serde::Deserialize)]
struct PostmanInfo {
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(serde::Deserialize)]
struct PostmanItem {
    name: String,
    #[serde(default)]
    item: Vec<PostmanItem>,
    #[serde(default)]
    request: Option<PostmanRequest>,
}

#[derive(serde::Deserialize)]
struct PostmanRequest {
    method: Option<String>,
    url: Option<PostmanUrl>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum PostmanUrl {
    Raw(String),
    Object { raw: Option<String> },
}

impl PostmanUrl {
    fn raw(&self) -> String {
        match self {
            PostmanUrl::Raw(s) => s.clone(),
            PostmanUrl::Object { raw } => raw.clone().unwrap_or_default(),
        }
    }
}

/// Convert a Postman v2.1 collection JSON string into a Verve project.
pub fn postman_v2_1(json: &str) -> anyhow::Result<Project> {
    let collection: PostmanCollection = serde_json::from_str(json)?;
    let name = collection
        .info
        .as_ref()
        .and_then(|i| i.name.clone())
        .unwrap_or_else(|| "Imported Project".to_string());
    let description = collection.info.as_ref().and_then(|i| i.description.clone());
    let mut project = Project::new(name);
    project.description = description.unwrap_or_default();

    let (folders, root_requests) = convert_items(&collection.item);
    project.folders = folders;
    project.requests = root_requests;
    Ok(project)
}

fn convert_items(items: &[PostmanItem]) -> (Vec<Folder>, Vec<ApiRequest>) {
    let mut folders = Vec::new();
    let mut requests = Vec::new();
    for item in items {
        if item.item.is_empty() {
            if let Some(req) = &item.request {
                let method = req
                    .method
                    .as_deref()
                    .and_then(RequestMethod::parse)
                    .unwrap_or(RequestMethod::Get);
                let url = req.url.as_ref().map(|u| u.raw()).unwrap_or_default();
                let mut api = ApiRequest::new(&item.name, method, url);
                api.description = String::new();
                requests.push(api);
            }
        } else {
            let mut folder = Folder::new(&item.name);
            let (sub_folders, sub_requests) = convert_items(&item.item);
            folder.folders = sub_folders;
            folder.requests = sub_requests;
            // A Postman item can both have children and a request; fold the
            // request in as a root-level entry of the folder.
            if let Some(req) = &item.request {
                let method = req
                    .method
                    .as_deref()
                    .and_then(RequestMethod::parse)
                    .unwrap_or(RequestMethod::Get);
                let url = req.url.as_ref().map(|u| u.raw()).unwrap_or_default();
                folder
                    .requests
                    .push(ApiRequest::new(&item.name, method, url));
            }
            folders.push(folder);
        }
    }
    (folders, requests)
}

/// A minimal slice of an OpenAPI 3 document — enough to enumerate operations.
#[derive(serde::Deserialize)]
struct OpenApiDoc {
    info: Option<OpenApiInfo>,
    #[serde(default)]
    paths: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    servers: Vec<OpenApiServer>,
}

#[derive(serde::Deserialize)]
struct OpenApiInfo {
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(serde::Deserialize)]
struct OpenApiServer {
    #[serde(default)]
    url: Option<String>,
}

/// Convert an OpenAPI 3 JSON document into a Verve project. Each operation
/// becomes a request under a folder named by its first path tag.
pub fn openapi_v3(json: &str) -> anyhow::Result<Project> {
    let doc: OpenApiDoc = serde_json::from_str(json)?;
    let name = doc
        .info
        .as_ref()
        .and_then(|i| i.title.clone())
        .unwrap_or_else(|| "Imported OpenAPI".to_string());
    let mut project = Project::new(name);
    project.description = doc.info.and_then(|i| i.description).unwrap_or_default();

    // Seed a baseUrl environment variable from the first server, if any.
    if let Some(server) = doc.servers.first().and_then(|s| s.url.as_deref()) {
        let mut env = Environment::new("Imported");
        env.variables = vec![KeyValue::new("baseUrl", server)];
        project.environments = vec![env.clone()];
        project.active_environment = Some(env.id);
    }

    let mut by_tag: std::collections::BTreeMap<String, Vec<ApiRequest>> =
        std::collections::BTreeMap::new();

    for (path, value) in &doc.paths {
        let obj = match value.as_object() {
            Some(o) => o,
            None => continue,
        };
        for (method_str, op) in obj {
            let method = match RequestMethod::parse(method_str) {
                Some(m) => m,
                None => continue, // skip `parameters`, `summary`, etc.
            };
            let tag = op
                .get("tags")
                .and_then(|t| t.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("Default")
                .to_string();
            let op_id = op
                .get("operationId")
                .and_then(|v| v.as_str())
                .or_else(|| op.get("summary").and_then(|v| v.as_str()))
                .unwrap_or(method_str)
                .to_string();
            let req = ApiRequest::new(op_id, method, format!("{{{{baseUrl}}}}{path}"));
            by_tag.entry(tag).or_default().push(req);
        }
    }

    project.folders = by_tag
        .into_iter()
        .map(|(tag, reqs)| {
            let mut f = Folder::new(tag);
            f.requests = reqs;
            f
        })
        .collect();

    Ok(project)
}

// ===========================================================================
// postman (postman 7+ export format)
// ===========================================================================
//
// The postman JSON has this top-level shape (only the fields we use are
// modelled; everything else is ignored via `#[serde(default)]`):
//
//   {
//     "name": "项目名",
//     "intro": "描述",
//     "global": {
//       "envs": [{ "env_id", "name", "env_var_list": { "<key>": "<value>" } }],
//       "global_param": { "header": { "parameter": [...] }, "query": { ... } }
//     },
//     "apis": [
//       { "target_type": "folder"|"api", "target_id", "parent_id", "name",
//         "method", "url", "request": { "body": {...}, "header": {...}, ... },
//         "response": { "example": [...] }, "mark_id", "description" }
//     ]
//   }
//
// Folder/request nesting is via `parent_id` ("0" = root). We build a flat
// list then assemble the tree.

/// Top-level postman export document.
#[derive(serde::Deserialize)]
struct postmanDoc {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    intro: Option<String>,
    #[serde(default)]
    global: postmanGlobal,
    #[serde(default)]
    apis: Vec<postmanNode>,
}

#[derive(serde::Deserialize, Default)]
struct postmanGlobal {
    #[serde(default)]
    envs: Vec<postmanEnv>,
    #[serde(default)]
    global_param: postmanGlobalParam,
    /// Global server definitions (server_id → name).
    #[serde(default)]
    servers: Vec<postmanServerDef>,
    // Status-code dictionary / describe library / marks are ignored for now.
}

#[derive(serde::Deserialize, Default)]
struct postmanServerDef {
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sort: i32,
}

#[derive(serde::Deserialize)]
struct postmanEnv {
    #[serde(default)]
    name: Option<String>,
    /// env_var_list is a free-form map of key → value strings.
    #[serde(default)]
    env_var_list: serde_json::Map<String, serde_json::Value>,
    /// Per-environment server list mapping server_id → URI.
    #[serde(default)]
    server_list: Vec<postmanServerEntry>,
}

#[derive(serde::Deserialize)]
struct postmanServerEntry {
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    uri: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct postmanGlobalParam {
    #[serde(default)]
    header: postmanParamSection,
    #[serde(default)]
    query: postmanParamSection,
}

#[derive(serde::Deserialize, Default)]
struct postmanParamSection {
    #[serde(default)]
    parameter: Vec<postmanParam>,
}

/// A single key/value parameter (header / query / form-data / cookie).
#[derive(serde::Deserialize)]
struct postmanParam {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<String>,
    /// 1 = enabled, -1 = disabled (postman convention).
    #[serde(default)]
    is_checked: i32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    not_null: i32,
    #[serde(default)]
    field_type: Option<String>,
    /// File path for form-data file parts.
    #[serde(default)]
    file_name: Option<String>,
}

impl postmanParam {
    fn to_kv(&self) -> KeyValue {
        let mut kv = KeyValue::new(
            self.key.clone().unwrap_or_default(),
            self.value.clone().unwrap_or_default(),
        );
        kv.enabled = self.is_checked != -1;
        kv.description = self.description.clone().unwrap_or_default();
        kv.required = self.not_null == 1;
        kv.field_type = match self.field_type.as_deref().unwrap_or("string") {
            "file" => FieldType::File,
            "number" | "integer" => FieldType::Number,
            "boolean" => FieldType::Bool,
            "array" => FieldType::Array,
            "decimal" => FieldType::Decimal,
            "object" => FieldType::Object,
            _ => FieldType::Text,
        };
        kv.file_path = self.file_name.clone().filter(|s| !s.is_empty());
        kv
    }
}

/// A node in the apis array — either a folder or an API.
#[derive(serde::Deserialize)]
struct postmanNode {
    /// "folder" or "api" (sometimes "http-api").
    #[serde(default)]
    target_type: Option<String>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    mark_id: Option<String>,
    /// For folders: references a server_id from global.servers (0 = inherit).
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    request: postmanRequest,
    /// Tags array (strings).
    #[serde(default)]
    tags: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize, Default)]
struct postmanRequest {
    #[serde(default)]
    body: postmanBody,
    #[serde(default)]
    header: postmanParamSection,
    #[serde(default)]
    query: postmanQuerySection,
    #[serde(default)]
    cookie: postmanParamSection,
    #[serde(default)]
    restful: postmanParamSection,
    #[serde(default)]
    auth: postmanAuth,
}

#[derive(serde::Deserialize, Default)]
struct postmanQuerySection {
    #[serde(default)]
    parameter: Vec<postmanParam>,
}

#[derive(serde::Deserialize, Default)]
struct postmanBody {
    /// "none" | "form-data" | "urlencoded" | "raw" | "binary"
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    parameter: Vec<postmanParam>,
    #[serde(default)]
    raw: Option<String>,
    /// Visual field descriptions for raw JSON bodies (postman raw_parameter).
    #[serde(default)]
    raw_parameter: Vec<postmanParam>,
}

#[derive(serde::Deserialize, Default)]
struct postmanAuth {
    #[serde(default)]
    kv: postmanAuthKv,
    #[serde(default)]
    bearer: postmanAuthBearer,
    #[serde(default)]
    basic: postmanAuthBasic,
}

#[derive(serde::Deserialize, Default)]
struct postmanAuthKv {
    #[serde(default)]
    #[allow(dead_code)]
    key: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    value: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    r#in: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct postmanAuthBearer {
    #[serde(default)]
    key: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct postmanAuthBasic {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// Convert an postman export JSON string into a Verve project. Maps the
/// project name, description, environments, global params, and the full
/// folder/request tree with body/headers/query/auth.
pub fn postman(json: &str) -> anyhow::Result<Project> {
    // Parse with serde_json::Value first so we can pull the auth `type` field
    // (a Rust keyword) that the typed structs above can't capture directly.
    let root: serde_json::Value = serde_json::from_str(json)?;
    let doc: postmanDoc = serde_json::from_value(root.clone())?;

    let name = doc.name.unwrap_or_else(|| "Imported postman".to_string());
    let mut project = Project::new(name);
    project.description = doc.intro.unwrap_or_default();

    // Build a map of server_id → variable name from global server definitions.
    // Server with id "1" is the default; map it to "baseUrl" for convenience.
    let mut server_var_names: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for srv in &doc.global.servers {
        if let Some(sid) = &srv.server_id {
            let var_name = if sid == "1" {
                "baseUrl".to_string()
            } else {
                format!("server_{}", sid)
            };
            server_var_names.insert(sid.clone(), var_name);
        }
    }

    // Environments — include both env_var_list entries and server URIs as variables.
    project.environments = doc
        .global
        .envs
        .iter()
        .map(|env| {
            let mut e = Environment::new(env.name.clone().unwrap_or_else(|| "Unnamed".to_string()));
            e.variables = env
                .env_var_list
                .iter()
                .map(|(k, v)| KeyValue::new(k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect();
            // Add server URIs as variables
            for srv in &env.server_list {
                if let (Some(sid), Some(uri)) = (&srv.server_id, &srv.uri) {
                    if !uri.trim().is_empty() {
                        let var_name = server_var_names
                            .get(sid)
                            .cloned()
                            .unwrap_or_else(|| format!("server_{}", sid));
                        e.variables.push(KeyValue::new(var_name, uri.clone()));
                    }
                }
            }
            e
        })
        .collect();
    if let Some(first) = project.environments.first() {
        project.active_environment = Some(first.id.clone());
    }

    // Global params → project-level headers / query.
    project.global_headers = doc
        .global
        .global_param
        .header
        .parameter
        .iter()
        .map(|p| p.to_kv())
        .collect();
    project.global_params = doc
        .global
        .global_param
        .query
        .parameter
        .iter()
        .map(|p| p.to_kv())
        .collect();

    // Build the folder/request tree from the flat apis list.
    // Build a map of server_id → env var name for folder base_url resolution.
    let (folders, requests) = build_postman_tree(&doc.apis, &root, &server_var_names);
    project.folders = folders;
    project.requests = requests;

    Ok(project)
}

/// Recursively build the (folders, root_requests) tree from the flat apis
/// array, using `parent_id` to establish nesting.
fn build_postman_tree(
    nodes: &[postmanNode],
    root: &serde_json::Value,
    server_var_names: &std::collections::BTreeMap<String, String>,
) -> (Vec<Folder>, Vec<ApiRequest>) {
    // Index nodes by id for quick lookup, separating folders from APIs.
    let mut folders: Vec<&postmanNode> = Vec::new();
    let mut apis: Vec<&postmanNode> = Vec::new();
    for node in nodes {
        match node.target_type.as_deref() {
            Some("folder") => folders.push(node),
            _ => apis.push(node), // "api", "http-api", or unknown → treat as API
        }
    }

    // Root-level: parent_id is "0" or absent.
    let root_apis: Vec<ApiRequest> = apis
        .iter()
        .filter(|n| is_root(n.parent_id.as_deref()))
        .map(|n| convert_postman_api(n, root))
        .collect();

    let root_folders: Vec<Folder> = folders
        .iter()
        .filter(|n| is_root(n.parent_id.as_deref()))
        .map(|f| build_postman_folder(f, &folders, &apis, root, server_var_names))
        .collect();

    (root_folders, root_apis)
}

fn is_root(parent_id: Option<&str>) -> bool {
    matches!(parent_id, None | Some("0") | Some(""))
}

/// Recursively build a folder, collecting its direct API children and
/// recursing into sub-folders.
fn build_postman_folder(
    folder: &postmanNode,
    all_folders: &[&postmanNode],
    all_apis: &[&postmanNode],
    root: &serde_json::Value,
    server_var_names: &std::collections::BTreeMap<String, String>,
) -> Folder {
    let fid = folder.target_id.clone().unwrap_or_default();
    let mut f = Folder::new(folder.name.clone().unwrap_or_else(|| "Unnamed".to_string()));
    f.description = folder.description.clone().unwrap_or_default();

    // Set folder base_url from server_id:
    // - server_id "0" or None → inherit from parent (leave as None)
    // - otherwise → use {{server_<id>}} or {{baseUrl}} for default server
    if let Some(sid) = &folder.server_id {
        if sid != "0" {
            if let Some(var_name) = server_var_names.get(sid) {
                f.base_url = Some(format!("{{{{{}}}}}", var_name));
            }
        }
    }

    f.requests = all_apis
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(fid.as_str()))
        .map(|n| convert_postman_api(n, root))
        .collect();

    f.folders = all_folders
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(fid.as_str()))
        .map(|sub| build_postman_folder(sub, all_folders, all_apis, root, server_var_names))
        .collect();

    f
}

/// Convert a single postman API node into a Verve ApiRequest.
fn convert_postman_api(node: &postmanNode, root: &serde_json::Value) -> ApiRequest {
    let method = node
        .method
        .as_deref()
        .and_then(RequestMethod::parse)
        .unwrap_or(RequestMethod::Get);
    let url = node.url.clone().unwrap_or_default();
    let name = node.name.clone().unwrap_or_else(|| "Unnamed".to_string());
    let mut api = ApiRequest::new(name, method, url);
    api.description = node.description.clone().unwrap_or_default();

    // Status from mark_id (maps to a status label).
    api.status = match node.mark_id.as_deref() {
        Some("1") => "开发中".to_string(),
        Some("2") => "已完成".to_string(),
        Some("3") => "需修改".to_string(),
        Some("4") => "已废弃".to_string(),
        _ => String::new(),
    };

    // Tags.
    api.tags = node
        .tags
        .iter()
        .filter_map(|t| t.as_str().map(|s| s.to_string()))
        .collect();

    // Protocol from target_type (postman encodes the protocol kind there).
    api.protocol = match node.target_type.as_deref() {
        Some("socketio") => Protocol::SocketIo,
        Some("sse") => Protocol::Sse,
        Some("websocket") | Some("websocket2") => Protocol::WebSocket,
        _ => Protocol::Http, // "api" and unknowns
    };

    // Headers / query / cookies / path.
    api.headers = node
        .request
        .header
        .parameter
        .iter()
        .map(|p| p.to_kv())
        .collect();
    api.params = node
        .request
        .query
        .parameter
        .iter()
        .map(|p| p.to_kv())
        .collect();
    api.cookies = node
        .request
        .cookie
        .parameter
        .iter()
        .map(|p| p.to_kv())
        .collect();
    api.path = node
        .request
        .restful
        .parameter
        .iter()
        .map(|p| p.to_kv())
        .collect();

    // Body.
    let body = &node.request.body;
    api.body = match body.mode.as_deref().unwrap_or("none") {
        "form-data" => RequestBody {
            body_type: BodyType::FormData,
            form_data: body.parameter.iter().map(|p| p.to_kv()).collect(),
            ..Default::default()
        },
        "urlencoded" => RequestBody {
            body_type: BodyType::Urlencoded,
            urlencoded: body.parameter.iter().map(|p| p.to_kv()).collect(),
            ..Default::default()
        },
        // postman uses "json" (not "raw") for JSON bodies — the text lives in
        // the `raw` field. "xml" and "text" are analogous for their types.
        "raw" | "json" | "xml" | "text" | "html" | "javascript" => {
            let raw_text = body.raw.clone().unwrap_or_default();
            let lang = match body.mode.as_deref() {
                Some("xml") => RawLanguage::Xml,
                Some("text") => RawLanguage::Text,
                Some("html") => RawLanguage::Html,
                Some("javascript") => RawLanguage::Javascript,
                _ => RawLanguage::Json, // "raw" and "json" default to JSON
            };
            RequestBody {
                body_type: BodyType::Raw,
                raw_language: lang,
                raw: raw_text,
                raw_parameter: body.raw_parameter.iter().map(|p| p.to_kv()).collect(),
                ..Default::default()
            }
        }
        _ => RequestBody::default(),
    };

    // Auth — pull the `type` from the raw JSON (it's a Rust keyword).
    let auth_type_str = root
        .get("apis")
        .and_then(|a| a.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|a| {
                if a.get("target_id").and_then(|v| v.as_str()) == node.target_id.as_deref() {
                    a.get("request")
                        .and_then(|r| r.get("auth"))
                        .and_then(|a| a.get("type"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
        })
        .unwrap_or_default();

    api.auth = convert_postman_auth(&auth_type_str, &node.request.auth);

    api
}

/// Map postman auth type → Verve AuthConfig.
fn convert_postman_auth(type_str: &str, auth: &postmanAuth) -> AuthConfig {
    match type_str {
        "bearer" => AuthConfig {
            auth_type: AuthType::Bearer,
            token: auth.bearer.key.clone().unwrap_or_default(),
            ..Default::default()
        },
        "basic" => AuthConfig {
            auth_type: AuthType::Basic,
            username: auth.basic.username.clone().unwrap_or_default(),
            password: auth.basic.password.clone().unwrap_or_default(),
            ..Default::default()
        },
        "apikey" => AuthConfig {
            auth_type: AuthType::ApiKey,
            key: auth.kv.key.clone().unwrap_or_default(),
            value: auth.kv.value.clone().unwrap_or_default(),
            add_to: match auth.kv.r#in.as_deref() {
                Some("query") => AuthTarget::Query,
                _ => AuthTarget::Header,
            },
            ..Default::default()
        },
        // "inherit", "noauth", "" → No auth (inherit is handled at send time).
        _ => AuthConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postman_v2_1_import() {
        let json = r#"{
            "info": {"name": "Demo", "description": "d"},
            "item": [
                {"name": "List", "request": {"method": "GET", "url": {"raw": "https://x.test/items"}}},
                {"name": "Auth", "item": [
                    {"name": "Login", "request": {"method": "POST", "url": "https://x.test/login"}}
                ]}
            ]
        }"#;
        let project = postman_v2_1(json).unwrap();
        assert_eq!(project.name, "Demo");
        assert_eq!(project.requests.len(), 1);
        assert_eq!(project.requests[0].method, RequestMethod::Get);
        assert_eq!(project.folders.len(), 1);
        assert_eq!(project.folders[0].name, "Auth");
        assert_eq!(project.folders[0].requests.len(), 1);
        assert_eq!(project.folders[0].requests[0].method, RequestMethod::Post);
    }

    #[test]
    fn openapi_v3_import() {
        let json = r#"{
            "openapi": "3.0.0",
            "info": {"title": "Pets API"},
            "servers": [{"url": "https://petstore.test"}],
            "paths": {
                "/pets": {
                    "get": {"tags": ["pets"], "operationId": "listPets"},
                    "post": {"tags": ["pets"], "summary": "Create pet"}
                },
                "/users/{id}": {
                    "get": {"operationId": "getUser"}
                }
            }
        }"#;
        let project = openapi_v3(json).unwrap();
        assert_eq!(project.name, "Pets API");
        assert!(project
            .environments
            .first()
            .map(|e| e.variables.iter().any(|v| v.key == "baseUrl"))
            .unwrap_or(false));
        let pets = project.folders.iter().find(|f| f.name == "pets").unwrap();
        assert_eq!(pets.requests.len(), 2);
        assert_eq!(
            project
                .folders
                .iter()
                .find(|f| f.name == "Default")
                .unwrap()
                .requests
                .len(),
            1
        );
    }

    #[test]
    fn postman_import() {
        // A trimmed version of the real postman export format (same structure,
        // fewer auth sub-objects / examples for brevity).
        let json = r#"{
            "project_id": "1fe63e",
            "name": "李靖的私有项目",
            "intro": "测试描述",
            "global": {
                "envs": [
                    {"name": "默认环境", "env_var_list": {"baseUrl": "https://api.test"}},
                    {"name": "Mock环境", "env_var_list": {}}
                ],
                "global_param": {
                    "header": {"parameter": [
                        {"key": "requestid", "value": "abc", "is_checked": 1, "description": "请求ID", "not_null": 1, "field_type": "string"}
                    ]},
                    "query": {"parameter": []}
                }
            },
            "apis": [
                {
                    "target_id": "folder1", "parent_id": "0", "target_type": "folder",
                    "name": "用户模块", "description": "用户相关接口"
                },
                {
                    "target_id": "api1", "parent_id": "0", "target_type": "api",
                    "name": "新建接口", "method": "POST", "url": "aaa/doAction",
                    "mark_id": "1", "description": "一个测试接口",
                    "request": {
                        "body": {
                            "mode": "form-data",
                            "parameter": [
                                {"key": "username", "value": "a", "is_checked": 1, "not_null": 1, "field_type": "string"},
                                {"key": "password", "value": "b", "is_checked": 1, "not_null": 1, "field_type": "string", "description": "密码"}
                            ]
                        },
                        "header": {"parameter": []},
                        "query": {"parameter": [{"key": "debug", "value": "1", "is_checked": 1}]},
                        "cookie": {"parameter": []},
                        "restful": {"parameter": []},
                        "auth": {"type": "bearer", "bearer": {"key": "my-token"}}
                    },
                    "tags": ["v1", "重要"]
                },
                {
                    "target_id": "api2", "parent_id": "folder1", "target_type": "api",
                    "name": "登录", "method": "GET", "url": "/user/login",
                    "request": {
                        "body": {"mode": "raw", "raw": "{\"k\":1}"},
                        "header": {"parameter": []},
                        "query": {"parameter": []},
                        "cookie": {"parameter": []},
                        "restful": {"parameter": []},
                        "auth": {"type": "noauth"}
                    }
                }
            ]
        }"#;

        let project = postman(json).unwrap();

        // Project metadata.
        assert_eq!(project.name, "李靖的私有项目");
        assert_eq!(project.description, "测试描述");

        // Environments.
        assert_eq!(project.environments.len(), 2);
        assert_eq!(project.environments[0].name, "默认环境");
        assert_eq!(project.environments[0].variables.len(), 1);
        assert_eq!(project.environments[0].variables[0].key, "baseUrl");
        assert_eq!(
            project.environments[0].variables[0].value,
            "https://api.test"
        );

        // Global header.
        assert_eq!(project.global_headers.len(), 1);
        assert_eq!(project.global_headers[0].key, "requestid");
        assert!(project.global_headers[0].required);

        // Root API (api1).
        assert_eq!(project.requests.len(), 1);
        let api1 = &project.requests[0];
        assert_eq!(api1.name, "新建接口");
        assert_eq!(api1.method, RequestMethod::Post);
        assert_eq!(api1.url, "aaa/doAction");
        assert_eq!(api1.status, "开发中");
        assert_eq!(api1.tags, vec!["v1", "重要"]);
        // Body = form-data with 2 fields.
        assert_eq!(api1.body.body_type, BodyType::FormData);
        assert_eq!(api1.body.form_data.len(), 2);
        assert_eq!(api1.body.form_data[0].key, "username");
        assert_eq!(api1.body.form_data[1].description, "密码");
        // Query param.
        assert_eq!(api1.params.len(), 1);
        assert_eq!(api1.params[0].key, "debug");
        // Bearer auth.
        assert_eq!(api1.auth.auth_type, AuthType::Bearer);
        assert_eq!(api1.auth.token, "my-token");

        // Folder with nested API.
        assert_eq!(project.folders.len(), 1);
        let folder = &project.folders[0];
        assert_eq!(folder.name, "用户模块");
        assert_eq!(folder.description, "用户相关接口");
        assert_eq!(folder.requests.len(), 1);
        let api2 = &folder.requests[0];
        assert_eq!(api2.name, "登录");
        assert_eq!(api2.method, RequestMethod::Get);
        assert_eq!(api2.body.body_type, BodyType::Raw);
        assert_eq!(api2.body.raw, "{\"k\":1}");
        // noauth → default (None).
        assert_eq!(api2.auth.auth_type, AuthType::None);
    }

    #[test]
    fn postman_import_with_servers() {
        // Apipost format with server_list in environments and server_id on folders.
        let json = r#"{
            "project_id": "35ff847188e5000",
            "name": "李靖个人项目",
            "global": {
                "servers": [
                    {"server_id": "1", "name": "默认服务", "sort": 1000}
                ],
                "envs": [
                    {
                        "env_id": "1",
                        "name": "默认环境",
                        "server_list": [
                            {"server_id": "1", "name": "默认服务", "uri": ""}
                        ],
                        "env_var_list": {}
                    },
                    {
                        "env_id": "2",
                        "name": "Mock环境",
                        "server_list": [
                            {"server_id": "1", "name": "默认服务", "uri": "https://mock.apipost.net/mock/35ff847188e5000"}
                        ],
                        "env_var_list": {}
                    },
                    {
                        "env_id": "3",
                        "name": "本地环境",
                        "server_list": [
                            {"server_id": "1", "name": "默认服务", "uri": "localhost:3002"}
                        ],
                        "env_var_list": {}
                    }
                ],
                "global_param": {"header": {"parameter": []}, "query": {"parameter": []}}
            },
            "apis": [
                {
                    "target_id": "folder1", "parent_id": "0", "target_type": "folder",
                    "name": "AgentOS", "server_id": "0"
                },
                {
                    "target_id": "folder2", "parent_id": "0", "target_type": "folder",
                    "name": "系统功能测试", "server_id": "1"
                },
                {
                    "target_id": "api1", "parent_id": "folder2", "target_type": "api",
                    "name": "用户登录", "method": "POST", "url": "/api/auth/login",
                    "request": {"body": {"mode": "none"}, "header": {"parameter": []}, "query": {"parameter": []}, "cookie": {"parameter": []}, "restful": {"parameter": []}, "auth": {"type": "inherit"}}
                }
            ]
        }"#;

        let project = postman(json).unwrap();

        // Project metadata.
        assert_eq!(project.name, "李靖个人项目");

        // 3 environments, each with server URIs as variables.
        assert_eq!(project.environments.len(), 3);

        // 默认环境: empty URI → no baseUrl var
        assert_eq!(project.environments[0].name, "默认环境");
        assert!(project.environments[0].variables.iter().find(|v| v.key == "baseUrl").is_none());

        // Mock环境: has baseUrl = mock server URI
        assert_eq!(project.environments[1].name, "Mock环境");
        let mock_base = project.environments[1].variables.iter().find(|v| v.key == "baseUrl").unwrap();
        assert_eq!(mock_base.value, "https://mock.apipost.net/mock/35ff847188e5000");

        // 本地环境: baseUrl = localhost:3002
        assert_eq!(project.environments[2].name, "本地环境");
        let local_base = project.environments[2].variables.iter().find(|v| v.key == "baseUrl").unwrap();
        assert_eq!(local_base.value, "localhost:3002");

        // Folder AgentOS has server_id "0" → inherit → no base_url set
        let agentos = project.folders.iter().find(|f| f.name == "AgentOS").unwrap();
        assert_eq!(agentos.base_url, None);

        // Folder 系统功能测试 has server_id "1" → {{baseUrl}}
        let sys_folder = project.folders.iter().find(|f| f.name == "系统功能测试").unwrap();
        assert_eq!(sys_folder.base_url, Some("{{baseUrl}}".to_string()));

        // Child API URL remains relative (will be prefixed at send time).
        assert_eq!(sys_folder.requests[0].url, "/api/auth/login");
    }
}
