//! Serializable JSON views over the workspace model.
//!
//! These keep the MCP wire surface stable even if the internal model changes,
//! and avoid dumping large captured responses by default.

use serde_json::{Value, json};

use crate::state::models::{
    ApiRequest, AuthConfig, Environment, Folder, KeyValue, Project, RequestBody,
};

/// A key/value row as exposed to AI clients.
fn kv(k: &KeyValue) -> Value {
    json!({
        "enabled": k.enabled,
        "key": k.key,
        "value": k.value,
        "description": k.description,
    })
}

/// Body view. `raw_parameter` (visual editor bookkeeping) is intentionally
/// omitted; `raw` already carries the canonical text.
fn body(b: &RequestBody) -> Value {
    json!({
        "type": serde_json::to_value(b.body_type).unwrap_or(Value::Null),
        "raw_language": serde_json::to_value(b.raw_language).unwrap_or(Value::Null),
        "raw": b.raw,
        "form_data": b.form_data.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
        "urlencoded": b.urlencoded.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
    })
}

/// Auth view. Credentials are included because the server runs locally and the
/// user explicitly connected their AI client; the same values are needed to
/// actually call the API.
fn auth(a: &AuthConfig) -> Value {
    json!({
        "type": serde_json::to_value(a.auth_type).unwrap_or(Value::Null),
        "token": a.token,
        "username": a.username,
        "password": a.password,
        "key": a.key,
        "value": a.value,
        "add_to": serde_json::to_value(a.add_to).unwrap_or(Value::Null),
    })
}

/// Project metadata with counts (no request payloads).
pub fn project_summary(p: &Project) -> Value {
    let request_count = p.iter_all_requests().len();
    let folder_count = count_folders(&p.folders);
    json!({
        "id": p.id,
        "name": p.name,
        "description": p.description,
        "folder_count": folder_count,
        "request_count": request_count,
        "environment_count": p.environments.len(),
        "active_environment_id": p.active_environment,
        "archived": p.archived,
    })
}

fn count_folders(folders: &[Folder]) -> usize {
    folders
        .iter()
        .map(|f| 1 + count_folders(&f.folders))
        .sum()
}

/// One row in `list_requests` (lightweight).
pub fn request_summary(folder_path: &str, r: &ApiRequest) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "method": r.method.to_string(),
        "protocol": serde_json::to_value(r.protocol).unwrap_or(Value::Null),
        "url": r.url,
        "folder_path": folder_path,
        "status": r.status,
        "tags": r.tags,
        "updated_at": r.updated_at,
    })
}

/// Full request detail returned by `get_request`.
pub fn request_detail(chain: &[String], r: &ApiRequest, include_responses: bool) -> Value {
    let mut v = json!({
        "id": r.id,
        "name": r.name,
        "method": r.method.to_string(),
        "protocol": serde_json::to_value(r.protocol).unwrap_or(Value::Null),
        "url": r.url,
        "folder_id_chain": chain,
        "params": r.params.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
        "headers": r.headers.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
        "path_variables": r.path.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
        "cookies": r.cookies.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
        "body": body(&r.body),
        "auth": auth(&r.auth),
        "variables": r.variables.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
        "description": r.description,
        "pre_request_script": r.pre_script,
        "test_script": r.tests_script,
        "status": r.status,
        "tags": r.tags,
        "created_by": r.created_by,
        "created_at": r.created_at,
        "updated_by": r.updated_by,
        "updated_at": r.updated_at,
        "has_mock": r.mock.is_some(),
    });
    if include_responses {
        v["last_response"] = r
            .last_response
            .as_ref()
            .and_then(|x| serde_json::to_value(x).ok())
            .unwrap_or(Value::Null);
        v["success_example"] = r
            .success_example
            .as_ref()
            .and_then(|x| serde_json::to_value(x).ok())
            .unwrap_or(Value::Null);
        v["fail_example_count"] = json!(r.fail_examples.len());
    }
    v
}

/// Recursive folder tree node.
pub fn folder_tree_node(f: &Folder) -> Value {
    json!({
        "id": f.id,
        "name": f.name,
        "description": f.description,
        "base_url": f.base_url,
        "request_count": f.requests.len(),
        "request_ids": f.requests.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        "folders": f.folders.iter().map(folder_tree_node).collect::<Vec<_>>(),
    })
}

/// An environment with its variables (values included — local opt-in).
pub fn environment_view(e: &Environment, active: bool) -> Value {
    json!({
        "id": e.id,
        "name": e.name,
        "active": active,
        "variables": e.variables.iter().filter(|k| !k.is_empty()).map(kv).collect::<Vec<_>>(),
    })
}
