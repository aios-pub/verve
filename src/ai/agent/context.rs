//! Agent context: workspace snapshot for the system prompt, local token
//! estimation, and history-budget folding.

use serde_json::{Value, json};

use crate::state::models::{ApiRequest, Folder, Project, Protocol, RequestMethod};

/// Rough token estimate used when a vendor doesn't report usage: CJK chars
/// ≈ 1 token each, everything else ≈ 4 chars per token.
pub fn estimate_tokens(text: &str) -> u64 {
    let mut cjk = 0u64;
    let mut other = 0u64;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other / 4
}

fn is_cjk(ch: char) -> bool {
    let c = ch as u32;
    (0x4E00..=0x9FFF).contains(&c) // CJK unified ideographs
        || (0x3400..=0x4DBF).contains(&c) // extension A
        || (0x3000..=0x303F).contains(&c) // CJK punctuation
        || (0xFF00..=0xFFEF).contains(&c) // fullwidth forms
        || (0x3040..=0x30FF).contains(&c) // kana
        || (0xAC00..=0xD7AF).contains(&c) // hangul syllables
}

// ---------------------------------------------------------------------------
// Workspace snapshot
// ---------------------------------------------------------------------------

/// Lightweight listing node: kind + id + name ONLY (no method/url/body
/// details — those come from `get_request` / `get_doc`). This is the
/// "list without details" contract of the progressive-loading design.
fn listing_node_request(req: &ApiRequest) -> Value {
    json!({
        "kind": if req.protocol == Protocol::Markdown { "doc" } else { req.protocol.as_str() },
        "id": req.id,
        "name": req.name,
    })
}

fn count_subtree(folder: &Folder) -> (usize, usize) {
    let mut dirs = folder.folders.len();
    let mut reqs = folder.requests.len();
    for sub in &folder.folders {
        let (d, r) = count_subtree(sub);
        dirs += d;
        reqs += r;
    }
    (dirs, reqs)
}

fn listing_node_folder(folder: &Folder, recursive: bool) -> Value {
    let (sub_dirs, sub_reqs) = count_subtree(folder);
    let children: Vec<Value> = if recursive {
        folder
            .folders
            .iter()
            .map(|f| listing_node_folder(f, true))
            .chain(folder.requests.iter().map(listing_node_request))
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "kind": "folder",
        "id": folder.id,
        "name": folder.name,
        "child_folders": folder.folders.len(),
        "child_requests": folder.requests.len(),
        "subtree": format!("{sub_dirs} 目录 / {sub_reqs} 接口"),
        "children": children,
    })
}

/// Resolve a `parent` argument to a folder id: exact id first, then a
/// unique-name match (models frequently pass the folder NAME they just saw).
fn lookup_folder(project: &Project, id_or_name: &str) -> Option<String> {
    if project.find_folder(id_or_name).is_some() {
        return Some(id_or_name.to_string());
    }
    fn walk(folders: &[Folder], name: &str, out: &mut Vec<String>) {
        for f in folders {
            if f.name == name {
                out.push(f.id.clone());
            }
            walk(&f.folders, name, out);
        }
    }
    let mut hits = Vec::new();
    walk(&project.folders, id_or_name, &mut hits);
    match hits.len() {
        1 => hits.into_iter().next(),
        _ => None,
    }
}

/// Progressive listing for the `list_workspace` tool. Default: the project's
/// ROOT level only; pass `parent` (folder id, or its unique name) to list
/// that folder's children; `recursive` expands folders in full. Names/ids
/// only — details come from `get_request` / `get_doc`.
pub fn workspace_listing(project: &Project, parent: Option<&str>, recursive: bool) -> Value {
    let nodes: Vec<Value> = match parent {
        Some(fid) => {
            let Some(real_id) = lookup_folder(project, fid) else {
                return json!({
                    "error": format!(
                        "目录不存在: {fid}（请传入 list_workspace 返回的目录 id；名称仅在全树唯一时可用）"
                    )
                });
            };
            let Some((_, folder)) = project.find_folder(&real_id) else {
                return json!({"error": format!("目录不存在: {real_id}")});
            };
            folder
                .folders
                .iter()
                .map(|f| listing_node_folder(f, recursive))
                .chain(folder.requests.iter().map(listing_node_request))
                .collect()
        }
        None => project
            .folders
            .iter()
            .map(|f| listing_node_folder(f, recursive))
            .chain(project.requests.iter().map(listing_node_request))
            .collect(),
    };
    json!({
        "listing": nodes,
        "hint": "默认仅列出当前层级；目录可传 parent=<目录id> 进入，或 recursive=true 展开全部；接口详情用 get_request，文档内容用 get_doc；按关键词找节点用 search_workspace",
    })
}

/// Keyword search over names (+ URLs) for the `search_workspace` tool — the
/// progressive-loading answer to "老师相关的接口有哪些" without dumping the
/// whole tree. Returns id, kind, name and the folder path (root→parent).
pub fn workspace_search(project: &Project, query: &str, include_url: bool) -> Value {
    let q = query.trim();
    if q.is_empty() {
        return json!({"error": "query 不能为空"});
    }
    let q_lower = q.to_lowercase();
    let mut hits: Vec<Value> = Vec::new();

    fn walk(
        folder: &Folder,
        path: &str,
        q_lower: &str,
        include_url: bool,
        hits: &mut Vec<Value>,
    ) {
        let here = if path.is_empty() {
            folder.name.clone()
        } else {
            format!("{path}/{}", folder.name)
        };
        if folder.name.to_lowercase().contains(q_lower) {
            hits.push(json!({
                "kind": "folder",
                "id": folder.id,
                "name": folder.name,
                "path": here,
            }));
        }
        for r in &folder.requests {
            let name_hit = r.name.to_lowercase().contains(q_lower);
            let url_hit =
                include_url && r.url.to_lowercase().contains(q_lower);
            if name_hit || url_hit {
                hits.push(json!({
                    "kind": if r.protocol == Protocol::Markdown { "doc" } else { r.protocol.as_str() },
                    "id": r.id,
                    "name": r.name,
                    "method": r.method.as_str(),
                    "url": r.url,
                    "path": here,
                }));
            }
        }
        for sub in &folder.folders {
            walk(sub, &here, q_lower, include_url, hits);
        }
    }

    for r in &project.requests {
        let name_hit = r.name.to_lowercase().contains(&q_lower);
        let url_hit = include_url && r.url.to_lowercase().contains(&q_lower);
        if name_hit || url_hit {
            hits.push(json!({
                "kind": if r.protocol == Protocol::Markdown { "doc" } else { r.protocol.as_str() },
                "id": r.id,
                "name": r.name,
                "method": r.method.as_str(),
                "url": r.url,
                "path": "",
            }));
        }
    }
    for f in &project.folders {
        walk(f, "", &q_lower, include_url, &mut hits);
    }

    let total = hits.len();
    hits.truncate(50);
    json!({
        "query": q,
        "total": total,
        "results": hits,
        "hint": if total > 50 { "结果超过 50 条，仅显示前 50；请用更精确的关键词" } else { "命中节点可直接用 id 调 get_request / get_doc 读详情" },
    })
}

/// Tiny overview embedded in the system prompt (progressive loading: the
/// full tree is NOT included — the model pulls it via `list_workspace` when
/// needed). Orientation only: counts + top-level names, no ids, no details.
pub fn workspace_overview(project: &Project) -> Value {
    fn walk(folder: &Folder, dirs: &mut usize, reqs: &mut usize, docs: &mut usize) {
        *dirs += folder.folders.len();
        for r in &folder.requests {
            if r.protocol == Protocol::Markdown {
                *docs += 1;
            } else {
                *reqs += 1;
            }
        }
        for sub in &folder.folders {
            walk(sub, dirs, reqs, docs);
        }
    }
    let (mut dirs, mut reqs, mut docs) = (project.folders.len(), 0, 0);
    for r in &project.requests {
        if r.protocol == Protocol::Markdown {
            docs += 1;
        } else {
            reqs += 1;
        }
    }
    for f in &project.folders {
        walk(f, &mut dirs, &mut reqs, &mut docs);
    }

    let mut var_keys: Vec<String> = project
        .global_variables
        .iter()
        .map(|kv| kv.key.clone())
        .collect();
    var_keys.extend(
        project
            .active_env_variables()
            .iter()
            .map(|kv| kv.key.clone()),
    );
    var_keys.sort();
    var_keys.dedup();

    let top_level: Vec<String> = project
        .folders
        .iter()
        .map(|f| f.name.clone())
        .chain(project.requests.iter().map(|r| r.name.clone()))
        .collect();
    // Cap the orientation list so a huge workspace doesn't bloat the prompt.
    let top_level_capped: Vec<String> =
        top_level.into_iter().take(20).collect();

    json!({
        "project": project.name,
        "counts": {"folders": dirs, "requests": reqs, "docs": docs},
        "top_level_names": top_level_capped,
        "variable_keys": var_keys,
    })
}

/// Serialize the full request definition for `get_request` results.
pub fn request_detail(req: &ApiRequest) -> Value {
    json!({
        "id": req.id,
        "name": req.name,
        "protocol": req.protocol.as_str(),
        "method": req.method.as_str(),
        "url": req.url,
        "params": req.params,
        "headers": req.headers,
        "path": req.path,
        "cookies": req.cookies,
        "body": {
            "type": serde_json::to_value(req.body.body_type).unwrap_or(Value::Null),
            "raw_language": serde_json::to_value(req.body.raw_language).unwrap_or(Value::Null),
            "raw": req.body.raw,
        },
        "auth_type": req.auth.auth_type.as_str(),
        "description": req.description,
        "tags": req.tags,
        "status": req.status,
    })
}

// ---------------------------------------------------------------------------
// History budget
// ---------------------------------------------------------------------------

/// Placeholder substituted for old tool results when over budget.
pub const FOLDED_PLACEHOLDER: &str = "[已省略]";

/// Fold the oldest tool-result payloads until the estimated total text is
/// within `budget_chars`. System/user/assistant messages are never folded.
pub fn enforce_budget(messages: &mut [super::super::chat::ChatMessage], budget_chars: usize) {
    use super::super::chat::{ChatMessage, MessageContent, Role};

    let mut total: usize = messages.iter().map(|m| m.text().len()).sum();
    if total <= budget_chars {
        return;
    }
    for msg in messages.iter_mut() {
        if total <= budget_chars {
            break;
        }
        if msg.role != Role::Tool {
            continue;
        }
        let Some(MessageContent::Text(t)) = msg.content.as_mut() else {
            continue;
        };
        // Leave short results alone — folding them saves nothing.
        if t.len() <= 64 {
            continue;
        }
        total = total
            .saturating_sub(t.len())
            .saturating_add(FOLDED_PLACEHOLDER.len());
        *t = FOLDED_PLACEHOLDER.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::chat::{ChatMessage, Role};
    use super::{
        estimate_tokens, enforce_budget, workspace_listing, workspace_overview, workspace_search,
    };
    use crate::state::models::{ApiRequest, Folder, Project, Protocol, RequestMethod};

    #[test]
    fn estimate_tokens_cjk_vs_ascii() {
        assert_eq!(estimate_tokens("你好世界"), 4);
        // 8 ASCII chars → 2 tokens.
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        // Mixed: 2 CJK + 4 ASCII → 2 + 1.
        assert_eq!(estimate_tokens("你好abcd"), 3);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn overview_is_lightweight_and_listing_is_progressive() {
        let mut project = Project::new("演示");
        project.global_variables.push(crate::state::models::KeyValue::new(
            "baseUrl", "https://x",
        ));
        let mut folder = Folder::new("用户模块");
        let mut req = ApiRequest::new("登录", RequestMethod::Post, "{{baseUrl}}/login");
        req.protocol = Protocol::Http;
        folder.requests.push(req);
        let mut sub = Folder::new("v2");
        sub.requests
            .push(ApiRequest::new("列表", RequestMethod::Get, "/v2/list"));
        folder.folders.push(sub);
        project.folders.push(folder);
        let mut doc = ApiRequest::new("需求说明", RequestMethod::Get, "");
        doc.protocol = Protocol::Markdown;
        project.requests.push(doc);

        // Overview: counts + top names + var keys, NO ids / methods / urls.
        let ov = workspace_overview(&project);
        assert_eq!(ov["counts"]["folders"], 2);
        assert_eq!(ov["counts"]["requests"], 2);
        assert_eq!(ov["counts"]["docs"], 1);
        assert!(ov["top_level_names"].as_array().unwrap().iter().any(|n| n == "用户模块"));
        assert!(ov.get("tree").is_none());

        // Root listing: top level only, names/ids, no method/url.
        let root = workspace_listing(&project, None, false);
        let nodes = root["listing"].as_array().cloned().unwrap_or_default();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["kind"], "folder");
        assert!(nodes[0].get("method").is_none());
        assert_eq!(nodes[0]["child_requests"], 1);

        // Drill into the folder by id.
        let fid = project.folders[0].id.clone();
        let inside = workspace_listing(&project, Some(&fid), false);
        let kids = inside["listing"].as_array().cloned().unwrap_or_default();
        assert_eq!(kids.len(), 2); // sub folder + 登录
        assert!(kids.iter().all(|n| n.get("url").is_none()));

        // Recursive expands everything, still without details.
        let full = workspace_listing(&project, None, true);
        let s = full.to_string();
        assert!(s.contains("v2"));
        assert!(s.contains("\"列表\""));
        assert!(!s.contains("/v2/list"));

        // Unknown parent errors cleanly (with guidance).
        assert!(workspace_listing(&project, Some("ghost"), false)["error"]
            .as_str()
            .unwrap_or("")
            .contains("不存在"));
        // parent also accepts a unique folder NAME.
        let by_name = workspace_listing(&project, Some("用户模块"), false);
        assert!(by_name["listing"]
            .as_array()
            .map_or(false, |a| !a.is_empty()));
    }

    #[test]
    fn search_finds_nested_by_name_and_url() {
        let mut project = Project::new("搜索");
        let mut folder = Folder::new("教师空间");
        let mut sub = Folder::new("课程");
        sub.requests.push(ApiRequest::new(
            "获取老师列表",
            RequestMethod::Get,
            "/api/teacher/list",
        ));
        folder.folders.push(sub);
        folder.requests.push(ApiRequest::new(
            "老师登录",
            RequestMethod::Post,
            "/api/teacher/login",
        ));
        project.folders.push(folder);
        project.requests.push(ApiRequest::new(
            "无关接口",
            RequestMethod::Get,
            "/api/other",
        ));

        let res = workspace_search(&project, "老师", true);
        assert_eq!(res["total"], 2);
        let s = res.to_string();
        assert!(s.contains("教师空间/课程"), "nested path in results: {s}");
        assert!(s.contains("teacher/login"));

        // URL-only hit when the name doesn't match.
        let mut p2 = Project::new("x");
        p2.requests.push(ApiRequest::new(
            "某接口",
            RequestMethod::Get,
            "/teacher/detail",
        ));
        assert_eq!(workspace_search(&p2, "teacher", true)["total"], 1);
        assert_eq!(workspace_search(&p2, "teacher", false)["total"], 0);

        // Empty query errors; results cap at 50.
        assert!(workspace_search(&project, "", true)["error"].is_string());
    }

    #[test]
    fn budget_folds_oldest_tool_results_only() {
        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user_text("q"),
            ChatMessage {
                role: Role::Tool,
                content: Some(super::super::super::chat::MessageContent::Text("x".repeat(500))),
                tool_calls: None,
                tool_call_id: Some("a".into()),
                name: Some("t1".into()),
            },
            ChatMessage::assistant("answer that is long enough ".repeat(20)),
        ];
        enforce_budget(&mut messages, 300);
        // The tool result was folded; others untouched.
        assert_eq!(messages[2].text(), super::FOLDED_PLACEHOLDER);
        assert_eq!(messages[0].text(), "sys");
        assert_eq!(messages[1].text(), "q");
        assert!(messages[3].text().len() > 300);
    }

    #[test]
    fn budget_noop_when_under() {
        let mut messages = vec![ChatMessage::user_text("hi")];
        enforce_budget(&mut messages, 1000);
        assert_eq!(messages[0].text(), "hi");
    }
}
