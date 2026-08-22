//! Tool registry exposed to the model, and dispatch onto the project tree.
//!
//! Read tools answer immediately from the tree. Write tools all funnel into
//! [`ChangeOp`] → [`crate::ai::ops`] so plan mode (`propose_changes`, which
//! only *validates* here — the panel applies on approval) and full-access
//! mode (direct tools, applied immediately by the caller) share one
//! executor, one validation pass and one id/ref discipline.

use serde_json::{Value, json};

use super::super::chat::ToolDef;
use super::super::config::AgentMode;
use super::super::ops::{self, ChangeOp};
use crate::state::models::{Project, Protocol};

/// Op vocabulary documentation shared by the write tools' descriptions.
const OP_DOC: &str = r#"变更操作(op)速查 —— 严格 JSON，字段名蛇形：
- create_folder: {name, parent?, description?, ref?}
- create_request: {name, method(get/post/put/delete/patch/head/options), url, protocol?(http/sse/websocket/tcp/grpc/socket_io/graphql), description?, tags?, params?:[{key,value,enabled?,description?}], headers?:[同上], body?:{type:none|form_data|urlencoded|raw, raw_language:json|xml|text|html|javascript, raw, urlencoded?, form_data?}, auth?:{type:none|bearer|basic|api_key, token?, username?, password?, key?, value?, add_to:header|query}, parent?, ref?}
- create_doc: {name, markdown, links?:[节点引用], parent?, ref?}（Markdown 需求文档节点）
- update_request: {id, 要改的字段…}（name/url/method/description/tags/params/headers/body/auth，提供才改）
- rename_node: {id, new_name}
- move_node: {id, folder?|before?|after?}（folder=移入该目录；before/after=排到某接口前/后；三者全缺省=移到根目录；引用均可用节点名称或 id）
- link_doc / unlink_doc: {doc, targets:[节点引用]}
- delete_node: {id}
节点引用：已有节点用真实 id（{"id":"..."} 或直接字符串）；同一计划内新建的节点用别名（{"ref":"auth_folder"}）。新建节点可选声明 ref 供后续引用。"#;

fn fn_schema() -> Value {
    json!({"type": "object", "additionalProperties": true})
}

/// Tool definitions for the current mode.
pub fn tool_defs(mode: AgentMode) -> Vec<ToolDef> {
    let mut defs = vec![
        ToolDef {
            name: "update_plan".into(),
            description: "维护一份贯穿整个会话的工作计划（Codex 风格）。计划会随每次请求原样注入系统提示，自动压缩不会丢失——多步任务的具体细节（要建/改哪些节点、字段取值、归类规则）必须写进各步骤的 detail，而不是留在对话正文里。规则：任务开始时创建计划；每完成一步立即更新状态；同一时刻只能有一个 in_progress；不要把步骤从 pending 直接改成 completed。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "description": "计划步骤（≤12）",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": {"type": "string", "description": "一句话步骤标题"},
                                "detail": {"type": "string", "description": "本步骤关键细节：节点名称、字段取值、规则等"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                            },
                            "required": ["step", "status"]
                        }
                    },
                    "note": {"type": "string", "description": "可选的整体备注"}
                },
                "required": ["steps"]
            }),
        },
        ToolDef {
            name: "use_skill".into(),
            description: "切换到指定技能并获取其完整提示词（渐进式加载：系统提示里只有技能索引，全文按需拉取）。任务性质变化时主动切换：提取→extractor、文档/接口全链路设计→doc-designer、单模块接口设计→api-designer、目录整理→organizer、闲聊问答→general。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "enum": ["general", "extractor", "api-designer", "doc-designer", "organizer"]}
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "list_workspace".into(),
            description: "轻量列出接口树（仅 id/名称/类型，不含方法/URL/参数等详情）。默认只列根级；传 parent=<目录id> 进入子目录，recursive=true 展开全部。修改任何东西之前先调用它获取真实 id；单节点详情用 get_request / get_doc。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "parent": {"type": "string", "description": "目录 id（列出其直接子级）；缺省列根级"},
                    "recursive": {"type": "boolean", "description": "是否递归展开子目录，默认 false"}
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "search_workspace".into(),
            description: "按关键词搜索工作区（匹配接口/文档/目录的名称，可选匹配 URL，不区分大小写），返回 id、名称、方法、URL 与所在目录路径。找特定接口（如「老师相关」）优先用它定位，再用 get_request 读详情。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "关键词（子串匹配）"},
                    "include_url": {"type": "boolean", "description": "是否同时匹配 URL，默认 true"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "get_request".into(),
            description: "读取一个接口的完整定义（参数/请求头/请求体/认证/描述）。参数：{\"id\": \"接口id或唯一名称\"}".into(),
            parameters: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
        },
        ToolDef {
            name: "get_doc".into(),
            description: "读取一个 Markdown 需求文档的内容。参数：{\"id\": \"文档id或唯一名称\"}".into(),
            parameters: json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}),
        },
    ];
    match mode {
        AgentMode::Plan => defs.push(ToolDef {
            name: "propose_changes".into(),
            description: format!("提交一份变更计划（写操作唯一入口），等待用户确认应用。ops 为变更操作数组，顺序执行。\n\n{OP_DOC}"),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ops": {"type": "array", "description": "变更操作列表，见上方速查", "items": {"type": "object"}}
                },
                "required": ["ops"]
            }),
        }),
        AgentMode::FullAccess => {
            for (name, desc) in [
                ("create_folder", "新建目录并立即生效。字段同 create_folder 速查（parent/description/ref 可选）。"),
                ("create_request", "新建接口并立即生效，返回新 id。字段同 create_request 速查。"),
                ("create_doc", "新建 Markdown 需求文档并立即生效，返回新 id。字段同 create_doc 速查。"),
                ("update_request", "修改接口字段并立即生效。字段同 update_request 速查。"),
                ("rename_node", "重命名接口/目录/文档并立即生效。"),
                ("move_node", "移动/排序节点并立即生效。"),
                ("link_doc", "把需求文档与接口/目录建立双向关联并立即生效。"),
                ("unlink_doc", "取消需求文档关联并立即生效。"),
                ("delete_node", "删除节点（目录级联删除）。删除前必须征得用户同意！"),
            ] {
                defs.push(ToolDef {
                    name: name.into(),
                    description: format!("{desc}\n\n{OP_DOC}"),
                    parameters: fn_schema(),
                });
            }
        }
    }
    defs
}

/// Which tool names are read-only (execute freely, both modes).
pub fn is_read_tool(name: &str) -> bool {
    matches!(
        name,
        "list_workspace" | "search_workspace" | "get_request" | "get_doc"
    )
}

/// Find a request by exact id, falling back to a unique name match.
fn lookup_request(project: &Project, id_or_name: &str) -> Option<String> {
    if project.find_request(id_or_name).is_some() {
        return Some(id_or_name.to_string());
    }
    let hits: Vec<String> = project
        .iter_all_requests()
        .into_iter()
        .filter(|(_, r)| r.name == id_or_name)
        .map(|(_, r)| r.id.clone())
        .collect();
    match hits.len() {
        1 => hits.into_iter().next(),
        _ => None,
    }
}

/// Execute a read tool against the tree.
pub fn dispatch_read(name: &str, args: &Value, project: &Project) -> Result<Value, String> {
    match name {
        "search_workspace" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("缺少参数 query")?;
            if query.trim().is_empty() {
                return Err("query 不能为空".into());
            }
            let include_url = args
                .get("include_url")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(super::context::workspace_search(project, query, include_url))
        }
        "list_workspace" => {
            let parent = args.get("parent").and_then(Value::as_str);
            let recursive = args
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(super::context::workspace_listing(project, parent, recursive))
        }
        "get_request" => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("缺少参数 id")?;
            let rid = lookup_request(project, id)
                .ok_or_else(|| format!("接口不存在或名称不唯一: {id}"))?;
            let Some((_, req)) = project.find_request(&rid) else {
                return Err(format!("接口不存在: {rid}"));
            };
            Ok(super::context::request_detail(req))
        }
        "get_doc" => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or("缺少参数 id")?;
            let rid = lookup_request(project, id)
                .ok_or_else(|| format!("文档不存在或名称不唯一: {id}"))?;
            let Some((_, doc)) = project.find_request(&rid) else {
                return Err(format!("文档不存在: {rid}"));
            };
            if doc.protocol != Protocol::Markdown {
                return Err(format!("节点「{}」不是 Markdown 文档", doc.name));
            }
            Ok(json!({
                "id": doc.id,
                "name": doc.name,
                "markdown": doc.description,
                "links": doc.doc_links,
            }))
        }
        _ => Err(format!("未知读工具: {name}")),
    }
}

/// Plan-mode `propose_changes`: parse + validate, but never mutate. The
/// panel applies the same `ops` after user approval.
pub fn propose_changes(args: &Value, project: &Project) -> Result<(Vec<ChangeOp>, Vec<ops::OpResult>), String> {
    let ops_json = args.get("ops").ok_or_else(|| {
        let received: String = args.to_string().chars().take(200).collect();
        format!(
            "缺少参数 ops（变更操作数组）。收到的参数：{received}。\
ops 必须是对象数组；若单次计划过大，请分批提交（每批 ≤ 8 个 op）再逐批确认。"
        )
    })?;
    let plan: Vec<ChangeOp> = serde_json::from_value(ops_json.clone())
        .map_err(|e| format!("ops 解析失败: {e}"))?;
    if plan.is_empty() {
        return Err("ops 不能为空".into());
    }
    let results = ops::validate_ops(project, &plan);
    Ok((plan, results))
}

/// Full-access direct write tool: wrap args as a single ChangeOp (tool name
/// equals the serde op tag) and apply immediately. `project` is expected to
/// be the caller's active project; persistence/notification is the caller's
/// job.
pub fn dispatch_write(name: &str, args: &Value, project: &mut Project) -> Result<Value, String> {
    let mut wrapped = args.clone();
    if let Some(obj) = wrapped.as_object_mut() {
        obj.insert("op".into(), json!(name));
    } else {
        return Err(format!("工具 {name} 参数必须是对象"));
    }
    let ops: Vec<ChangeOp> = vec![serde_json::from_value(wrapped)
        .map_err(|e| format!("参数解析失败: {e}"))?];
    if let Some(first) = ops.first() {
        // Early name/ref validation for friendlier errors.
        let check = ops::validate_ops(project, std::slice::from_ref(first));
        if let Some(bad) = check.iter().find(|r| !r.ok) {
            return Err(bad.error.clone().unwrap_or_else(|| "校验失败".into()));
        }
    }
    let results = ops::apply_change_ops(project, &ops);
    let Some(r0) = results.first() else {
        return Err("执行失败".into());
    };
    if !r0.ok {
        return Err(r0.error.clone().unwrap_or_else(|| "执行失败".into()));
    }
    Ok(serde_json::to_value(results).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::super::config::AgentMode;
    use super::{dispatch_read, dispatch_write, propose_changes, tool_defs};
    use crate::state::models::{Project, Protocol, RequestMethod};

    fn sample() -> Project {
        let mut p = Project::new("测试");
        p.requests
            .push(crate::state::models::ApiRequest::new(
                "登录",
                RequestMethod::Post,
                "/login",
            ));
        p
    }

    #[test]
    fn tool_defs_follow_mode() {
        let plan = tool_defs(AgentMode::Plan);
        assert!(plan.iter().any(|t| t.name == "propose_changes"));
        assert!(!plan.iter().any(|t| t.name == "delete_node"));
        let full = tool_defs(AgentMode::FullAccess);
        assert!(full.iter().any(|t| t.name == "delete_node"));
        assert!(!full.iter().any(|t| t.name == "propose_changes"));
        assert!(full.iter().any(|t| t.name == "search_workspace"));
        assert_eq!(full.len(), 4 + 2 + 9);
    }

    #[test]
    fn list_and_get_read_tools() {
        let p = sample();
        let snap = dispatch_read("list_workspace", &json!({}), &p).unwrap();
        assert!(snap["listing"].as_array().map_or(false, |a| !a.is_empty()));
        // Drill-down by parent id: nonexistent parent errors cleanly (the
        // sample project has no folders).
        let inside = dispatch_read("list_workspace", &json!({"parent": "ghost"}), &p).unwrap();
        assert!(inside["error"].as_str().unwrap_or("").contains("不存在"));
        let id = p.requests[0].id.clone();
        let detail = dispatch_read("get_request", &json!({"id": id}), &p).unwrap();
        assert_eq!(detail["method"], "POST");
        // Name lookup works when unique.
        let by_name = dispatch_read("get_request", &json!({"id": "登录"}), &p).unwrap();
        assert_eq!(by_name["name"], "登录");
        assert!(dispatch_read("get_request", &json!({"id": "不存在"}), &p).is_err());
        // Search tool.
        let found = dispatch_read("search_workspace", &json!({"query": "登录"}), &p).unwrap();
        assert_eq!(found["total"], 1);
        assert!(dispatch_read("search_workspace", &json!({"query": ""}), &p).is_err());
    }

    #[test]
    fn propose_validates_without_mutating() {
        let p = sample();
        let login_id = p.requests[0].id.clone();
        let args = json!({"ops": [
            {"op": "create_folder", "name": "F", "ref": "f"},
            {"op": "move_node", "id": login_id, "folder": {"ref": "f"}}
        ]});
        let (plan, results) = propose_changes(&args, &p).unwrap();
        assert_eq!(plan.len(), 2);
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        // No mutation happened.
        assert_eq!(p.folders.len(), 0);
    }

    #[test]
    fn propose_rejects_bad_ops() {
        let p = sample();
        let args = json!({"ops": [{"op": "rename_node", "id": "ghost", "new_name": "x"}]});
        let (_, results) = propose_changes(&args, &p).unwrap();
        assert!(!results[0].ok);
        // Missing ops entirely.
        assert!(propose_changes(&json!({}), &p).is_err());
    }

    #[test]
    fn direct_write_applies_immediately() {
        let mut p = sample();
        let out = dispatch_write(
            "create_doc",
            &json!({"name": "需求", "markdown": "# m", "links": []}),
            &mut p,
        )
        .unwrap();
        assert!(out[0]["created_id"].is_string());
        assert!(p
            .iter_all_requests()
            .into_iter()
            .any(|(_, r)| r.protocol == Protocol::Markdown));
        // Bogus op tag fails cleanly.
        assert!(dispatch_write("nope", &json!({}), &mut p).is_err());
    }
}
