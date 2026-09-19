//! The Verve MCP server: tools that let external AI clients read and manage
//! the API workspace over stdio.
//!
//! Read tools always load the latest `workspace.json`; write tools reuse the
//! in-app AI change vocabulary ([`ai::ops::ChangeOp`]) so behavior (id
//! generation, timestamps, name/id resolution, validation) stays identical to
//! the built-in AI agent, and persist through the guarded [`store::mutate`].

use std::sync::Arc;

use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ai::ops::{
    AuthSpec, BodySpec, ChangeOp, KvSpec, NodeRef, OpProtocol, apply_change_ops,
};
use crate::state::models::{
    ApiRequest, Environment, Folder, KeyValue, Project, WorkspaceData,
};

use super::config::ClientKind;
use super::{config, dto, store};

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// MCP server handler. Cloneable because rmcp may wrap it per connection;
/// state is the workspace files on disk, and [`VerveMcpServer::write`]
/// serializes concurrent writes.
#[derive(Clone)]
pub struct VerveMcpServer {
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl VerveMcpServer {
    pub fn new() -> Self {
        Self {
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Run a guarded write, serialized against other MCP write calls.
    async fn write<F>(&self, f: F) -> Result<String, ErrorData>
    where
        F: FnOnce(&mut WorkspaceData) -> Result<Value, String> + Send,
    {
        let _guard = self.write_lock.lock().await;
        let value = store::mutate(f).map_err(internal_error)?;
        pretty(&value)
    }
}

#[tool_router]
impl VerveMcpServer {
    /// 列出工作区中的所有项目（id、名称、描述、接口/目录/环境数量）。
    #[tool(description = "列出 Verve 工作区中的所有项目，返回 id、名称、描述、接口数、目录数、环境数以及当前激活项目。")]
    async fn list_projects(&self) -> Result<String, ErrorData> {
        let data = store::load();
        // Report the *effective* active project: fall back to the first project
        // when the file never recorded an active id (fresh demo workspace).
        let active_id = data
            .active_project_id
            .clone()
            .or_else(|| data.projects.first().map(|p| p.id.clone()));
        pretty(&json!({
            "active_project_id": active_id,
            "projects": data
                .projects
                .iter()
                .map(dto::project_summary)
                .collect::<Vec<_>>(),
        }))
    }

    /// 列出项目中的接口（轻量摘要）。
    #[tool(
        description = "列出项目中的接口（轻量摘要：id、名称、方法、协议、URL、所在目录、标签、状态）。可通过 folder_id 限定目录子树，通过 query 按名称/URL/标签模糊过滤。需要完整定义（请求头、请求体、认证等）时再调用 get_request。"
    )]
    async fn list_requests(
        &self,
        Parameters(p): Parameters<ListRequestsParams>,
    ) -> Result<String, ErrorData> {
        let data = store::load();
        let idx = store::resolve_project(&data, p.project_id.as_deref()).map_err(internal_error)?;
        let project = data
            .projects
            .get(idx)
            .ok_or_else(|| internal_error("项目不存在"))?;

        let mut rows: Vec<Value> = Vec::new();
        if let Some(fsel) = p.folder_id.as_deref() {
            let (_chain, folder) = find_folder_by_id_or_name(project, fsel)
                .ok_or_else(|| internal_error(format!("目录不存在: {fsel}")))?;
            let folder_name = folder.name.clone();
            collect_subtree(folder, &folder_name, &mut rows);
        } else {
            for (path, req) in project.iter_all_requests() {
                rows.push(dto::request_summary(&path, req));
            }
        }
        if let Some(needle) = p.query.as_deref().map(|s| s.to_ascii_lowercase()) {
            rows.retain(|v| {
                let hay = format!("{} {} {:?}", v["name"], v["url"], v["tags"]).to_ascii_lowercase();
                hay.contains(&needle)
            });
        }
        pretty(&json!({
            "project_id": project.id,
            "project_name": project.name,
            "count": rows.len(),
            "requests": rows,
        }))
    }

    /// 获取单个接口的完整定义。
    #[tool(
        description = "按 id（或唯一接口名）获取接口的完整定义：URL、查询参数、请求头、Cookie、请求体、认证、变量、说明、前后置脚本、标签状态等。默认不返回历史响应；需要时置 include_responses=true。"
    )]
    async fn get_request(
        &self,
        Parameters(p): Parameters<GetRequestParams>,
    ) -> Result<String, ErrorData> {
        let data = store::load();
        let idx = store::resolve_project(&data, p.project_id.as_deref()).map_err(internal_error)?;
        let project = data
            .projects
            .get(idx)
            .ok_or_else(|| internal_error("项目不存在"))?;
        let (chain, req) = find_request_tolerant(project, &p.id)
            .ok_or_else(|| internal_error(format!("接口不存在: {}", p.id)))?;
        let detail = dto::request_detail(&chain, req, p.include_responses.unwrap_or(false));
        pretty(&json!({"project_id": project.id, "request": detail}))
    }

    /// 列出目录树。
    #[tool(
        description = "列出项目的完整目录树（id、名称、描述、base_url、子目录、目录内接口 id）。用于创建/移动接口时选择 parent。"
    )]
    async fn list_folders(
        &self,
        Parameters(p): Parameters<ProjectIdParam>,
    ) -> Result<String, ErrorData> {
        let data = store::load();
        let idx = store::resolve_project(&data, p.project_id.as_deref()).map_err(internal_error)?;
        let project = data
            .projects
            .get(idx)
            .ok_or_else(|| internal_error("项目不存在"))?;
        pretty(&json!({
            "project_id": project.id,
            "project_name": project.name,
            "folders": project.folders.iter().map(dto::folder_tree_node).collect::<Vec<_>>(),
        }))
    }

    /// 列出环境及变量。
    #[tool(
        description = "列出项目的全部环境（id、名称、是否激活）及其变量（键值对，包含可能的密钥值）。"
    )]
    async fn list_environments(
        &self,
        Parameters(p): Parameters<ProjectIdParam>,
    ) -> Result<String, ErrorData> {
        let data = store::load();
        let idx = store::resolve_project(&data, p.project_id.as_deref()).map_err(internal_error)?;
        let project = data
            .projects
            .get(idx)
            .ok_or_else(|| internal_error("项目不存在"))?;
        let envs = project
            .environments
            .iter()
            .map(|e| {
                dto::environment_view(
                    e,
                    project.active_environment.as_deref() == Some(e.id.as_str()),
                )
            })
            .collect::<Vec<_>>();
        pretty(&json!({"project_id": project.id, "environments": envs}))
    }

    /// 新建项目。
    #[tool(description = "在当前工作区新建一个项目（项目名不可重复），返回新项目 id。")]
    async fn create_project(
        &self,
        Parameters(p): Parameters<CreateProjectParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let name = p.name.trim();
            if name.is_empty() {
                return Err("项目名不能为空".into());
            }
            if data.projects.iter().any(|x| x.name == name) {
                return Err(format!("同名项目已存在: {name}"));
            }
            let mut project = Project::new(name);
            project.description = p.description.unwrap_or_default();
            let id = project.id.clone();
            let is_first = data.projects.is_empty();
            data.projects.push(project);
            if is_first || data.active_project_id.is_none() {
                data.active_project_id = Some(id.clone());
            }
            Ok(json!({
                "ok": true,
                "summary": format!("新建项目「{name}」"),
                "created_id": id,
            }))
        })
        .await
    }

    /// 新建目录。
    #[tool(description = "在项目中新建目录（可指定父目录，缺省建在根目录），返回新目录 id。")]
    async fn create_folder(
        &self,
        Parameters(p): Parameters<CreateFolderParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let op = ChangeOp::CreateFolder {
                parent: p.parent_id.map(NodeRef::Bare),
                name: p.name.trim().to_string(),
                description: p.description,
                r#ref: None,
            };
            op_results(apply_change_ops(project, &[op]))
        })
        .await
    }

    /// 新建接口。
    #[tool(
        description = "在项目中新建一个 API 接口（请求定义），返回新接口 id。method 为 GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS；protocol 为 http/sse/websocket/tcp/grpc/socketio/graphql（默认 http）；folder_id 缺省建在根目录；URL 可含 {{变量}} 占位符。"
    )]
    async fn create_request(
        &self,
        Parameters(p): Parameters<CreateRequestParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let op = ChangeOp::CreateRequest {
                parent: p.folder_id.map(NodeRef::Bare),
                r#ref: None,
                name: p.name.trim().to_string(),
                method: parse_enum("HTTP 方法", &p.method)?,
                protocol: p
                    .protocol
                    .as_deref()
                    .map(|x| parse_enum::<OpProtocol>("协议", x))
                    .transpose()?,
                url: p.url,
                description: p.description,
                tags: p.tags.unwrap_or_default(),
                params: p
                    .params
                    .unwrap_or_default()
                    .into_iter()
                    .map(kv_to_spec)
                    .collect(),
                headers: p
                    .headers
                    .unwrap_or_default()
                    .into_iter()
                    .map(kv_to_spec)
                    .collect(),
                body: p.body.map(body_to_spec).transpose()?,
                auth: p.auth.map(auth_to_spec).transpose()?,
            };
            op_results(apply_change_ops(project, &[op]))
        })
        .await
    }

    /// 更新接口（部分字段）。
    #[tool(
        description = "按 id（或唯一接口名）部分更新接口：名称、URL、方法、协议、说明、标签、状态标签（status，如 已发布/开发中/废弃）、查询参数、请求头、请求体、认证。任一字段缺省即不修改；params/headers/body/auth 一旦给出则整体替换。"
    )]
    async fn update_request(
        &self,
        Parameters(p): Parameters<UpdateRequestParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let method = p
                .method
                .as_deref()
                .map(|m| parse_enum("HTTP 方法", m))
                .transpose()?;
            let op = ChangeOp::UpdateRequest {
                id: NodeRef::Bare(p.id.clone()),
                name: p.name.map(|n| n.trim().to_string()),
                url: p.url,
                method,
                description: p.description,
                tags: p.tags,
                params: p
                    .params
                    .map(|v| v.into_iter().map(kv_to_spec).collect()),
                headers: p
                    .headers
                    .map(|v| v.into_iter().map(kv_to_spec).collect()),
                body: p.body.map(body_to_spec).transpose()?,
                auth: p.auth.map(auth_to_spec).transpose()?,
            };
            let mut result = op_results(apply_change_ops(project, &[op]))?;

            // protocol / status are not covered by ChangeOp::UpdateRequest.
            let mut touched = false;
            if let Some(proto) = p.protocol.as_deref() {
                let proto: OpProtocol = parse_enum("协议", proto)?;
                let (_, req) = find_request_mut_tolerant(project, &p.id)
                    .ok_or_else(|| format!("接口不存在: {}", p.id))?;
                req.protocol = convert_enum(proto)?;
                touched = true;
            }
            if let Some(status) = p.status {
                let (_, req) = find_request_mut_tolerant(project, &p.id)
                    .ok_or_else(|| format!("接口不存在: {}", p.id))?;
                req.status = status;
                touched = true;
            }
            if touched {
                let (_, req) = find_request_mut_tolerant(project, &p.id)
                    .ok_or_else(|| format!("接口不存在: {}", p.id))?;
                req.updated_by = "AI".to_string();
                req.updated_at = now_stamp();
            }
            result["updated_id"] = json!(p.id);
            Ok(result)
        })
        .await
    }

    /// 重命名接口或目录。
    #[tool(description = "重命名一个接口或目录（id 或唯一名称）。")]
    async fn rename_node(
        &self,
        Parameters(p): Parameters<RenameParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let op = ChangeOp::RenameNode {
                id: NodeRef::Bare(p.id),
                new_name: p.new_name.trim().to_string(),
            };
            op_results(apply_change_ops(project, &[op]))
        })
        .await
    }

    /// 移动接口或目录。
    #[tool(
        description = "把接口或目录移动到指定目录下；folder_id 缺省表示移动到项目根目录。不能移动到自身的子目录内。"
    )]
    async fn move_node(&self, Parameters(p): Parameters<MoveParams>) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let op = ChangeOp::MoveNode {
                id: NodeRef::Bare(p.id),
                folder: p.folder_id.map(NodeRef::Bare),
                before: None,
                after: None,
            };
            op_results(apply_change_ops(project, &[op]))
        })
        .await
    }

    /// 删除接口或目录。
    #[tool(
        description = "删除一个接口，或删除一个目录（目录内全部子内容一并删除）。写入前会自动备份 workspace.json。"
    )]
    async fn delete_node(
        &self,
        Parameters(p): Parameters<NodeParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let op = ChangeOp::DeleteNode {
                id: NodeRef::Bare(p.id),
            };
            op_results(apply_change_ops(project, &[op]))
        })
        .await
    }

    /// 设置（创建或更新）环境变量。
    #[tool(
        description = "在指定环境中新增或更新一个变量（环境按 id/名称匹配，不存在则按名称自动创建）。用于维护 baseUrl、token 等 {{变量}}。"
    )]
    async fn set_environment_variable(
        &self,
        Parameters(p): Parameters<EnvVarParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let env_name = p.environment.trim();
            if env_name.is_empty() {
                return Err("环境名不能为空".into());
            }
            let key = p.key.trim();
            if key.is_empty() {
                return Err("变量名不能为空".into());
            }
            let env_idx = match project
                .environments
                .iter()
                .position(|e| e.id == env_name || e.name == env_name)
            {
                Some(i) => i,
                None => {
                    project.environments.push(Environment::new(env_name));
                    project.environments.len().saturating_sub(1)
                }
            };
            let env = project
                .environments
                .get_mut(env_idx)
                .ok_or_else(|| "环境不存在".to_string())?;
            match env.variables.iter_mut().find(|kv| kv.key == key) {
                Some(kv) => {
                    kv.value = p.value.clone();
                    kv.enabled = p.enabled.unwrap_or(kv.enabled);
                }
                None => {
                    let mut kv = KeyValue::new(key, &p.value);
                    kv.enabled = p.enabled.unwrap_or(true);
                    env.variables.push(kv);
                }
            }
            let env_id = env.id.clone();
            let env_display = env.name.clone();
            Ok(json!({
                "ok": true,
                "summary": format!("环境「{env_display}」变量 {key} 已保存"),
                "environment_id": env_id,
            }))
        })
        .await
    }

    /// 删除环境变量。
    #[tool(description = "删除指定环境中的一个变量。")]
    async fn delete_environment_variable(
        &self,
        Parameters(p): Parameters<DeleteEnvVarParams>,
    ) -> Result<String, ErrorData> {
        self.write(move |data| {
            let idx = store::resolve_project(data, p.project_id.as_deref())?;
            let project = data
                .projects
                .get_mut(idx)
                .ok_or_else(|| "项目不存在".to_string())?;
            let env_name = p.environment.trim();
            let key = p.key.trim();
            let env = project
                .environments
                .iter_mut()
                .find(|e| e.id == env_name || e.name == env_name)
                .ok_or_else(|| format!("环境不存在: {env_name}"))?;
            let before = env.variables.len();
            env.variables.retain(|kv| kv.key != key);
            if env.variables.len() == before {
                return Err(format!("环境「{env_name}」中不存在变量 {key}"));
            }
            Ok(json!({
                "ok": true,
                "summary": format!("环境「{env_name}」变量 {key} 已删除"),
            }))
        })
        .await
    }
}

#[tool_handler(
    router = Self::tool_router(),
    name = "verve",
    instructions = "Verve 本地 API 工作台的 MCP 服务。通过这些工具可以读取并管理当前工作区的项目、目录、接口（HTTP 请求定义）与环境变量。读操作总是返回最新内容；写操作立即持久化到本地 workspace.json（写入前自动备份，写坏自动回滚），运行中的 Verve 桌面端会自动热重载。接口 URL 可含 {{变量}} 占位符，变量值来自项目全局变量、目录变量与环境变量；认证配置与环境变量中包含可用于实际调用的密钥值，该服务仅在本机通过 stdio 暴露。"
)]
impl ServerHandler for VerveMcpServer {}

// ---------------------------------------------------------------------------
// Parameter types
// ---------------------------------------------------------------------------

/// 项目选择参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ProjectIdParam {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
}

/// list_requests 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ListRequestsParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 仅列出该目录（含其子目录）下的接口；目录 id 或名称。
    #[serde(default)]
    folder_id: Option<String>,
    /// 关键字过滤（匹配接口名、URL、标签，不区分大小写）。
    #[serde(default)]
    query: Option<String>,
}

/// get_request 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct GetRequestParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 接口 id 或唯一接口名。
    id: String,
    /// 是否同时返回最近一次响应、成功/失败示例（默认 false，内容可能很大）。
    #[serde(default)]
    include_responses: Option<bool>,
}

/// create_folder 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct CreateFolderParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 父目录 id/名称；留空建在项目根目录。
    #[serde(default)]
    parent_id: Option<String>,
    /// 目录名称。
    name: String,
    /// 目录描述。
    #[serde(default)]
    description: Option<String>,
}

/// create_project 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct CreateProjectParams {
    /// 项目名称。
    name: String,
    /// 项目描述。
    #[serde(default)]
    description: Option<String>,
}

/// 键值行（查询参数 / 请求头 / 表单字段）。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct KvParam {
    /// 键名（请求头名称 / 查询参数名 / 表单字段名）。
    key: String,
    /// 键值。
    value: String,
    /// 是否启用该行，默认 true。
    #[serde(default)]
    enabled: Option<bool>,
    /// 备注说明。
    #[serde(default)]
    description: Option<String>,
}

/// 请求体定义。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct BodyParam {
    /// 请求体类型：none / raw / form_data / urlencoded；缺省时按提供的字段推断。
    #[serde(default)]
    r#type: Option<String>,
    /// raw 语言（type=raw 时）：json / xml / text / html / javascript，默认 json。
    #[serde(default)]
    raw_language: Option<String>,
    /// raw 请求体文本。
    #[serde(default)]
    raw: Option<String>,
    /// multipart/form-data 字段（type=form_data）。
    #[serde(default)]
    form_data: Option<Vec<KvParam>>,
    /// application/x-www-form-urlencoded 字段（type=urlencoded）。
    #[serde(default)]
    urlencoded: Option<Vec<KvParam>>,
}

/// 认证定义。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AuthParam {
    /// 认证类型：none / bearer / basic / api_key。
    r#type: String,
    /// Bearer token（type=bearer）。
    #[serde(default)]
    token: Option<String>,
    /// Basic 用户名（type=basic）。
    #[serde(default)]
    username: Option<String>,
    /// Basic 密码（type=basic）。
    #[serde(default)]
    password: Option<String>,
    /// API Key 参数名（type=api_key）。
    #[serde(default)]
    key: Option<String>,
    /// API Key 参数值（type=api_key）。
    #[serde(default)]
    value: Option<String>,
    /// API Key 注入位置（type=api_key）：header / query，默认 header。
    #[serde(default)]
    add_to: Option<String>,
}

/// create_request 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct CreateRequestParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 所属目录 id/名称；留空建在项目根目录。
    #[serde(default)]
    folder_id: Option<String>,
    /// 接口名称。
    name: String,
    /// HTTP 方法：GET / POST / PUT / DELETE / PATCH / HEAD / OPTIONS。
    method: String,
    /// 协议：http / sse / websocket / tcp / grpc / socketio / graphql，默认 http。
    #[serde(default)]
    protocol: Option<String>,
    /// 接口 URL（可含 {{变量}} 占位符）。
    url: String,
    /// 接口说明（Markdown）。
    #[serde(default)]
    description: Option<String>,
    /// 标签列表。
    #[serde(default)]
    tags: Option<Vec<String>>,
    /// 查询参数。
    #[serde(default)]
    params: Option<Vec<KvParam>>,
    /// 请求头。
    #[serde(default)]
    headers: Option<Vec<KvParam>>,
    /// 请求体。
    #[serde(default)]
    body: Option<BodyParam>,
    /// 认证配置。
    #[serde(default)]
    auth: Option<AuthParam>,
}

/// update_request 参数（全部字段可选）。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct UpdateRequestParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 接口 id 或唯一接口名。
    id: String,
    /// 新接口名。
    #[serde(default)]
    name: Option<String>,
    /// 新 URL。
    #[serde(default)]
    url: Option<String>,
    /// 新 HTTP 方法。
    #[serde(default)]
    method: Option<String>,
    /// 新协议。
    #[serde(default)]
    protocol: Option<String>,
    /// 新说明（Markdown，整体替换）。
    #[serde(default)]
    description: Option<String>,
    /// 新标签（整体替换）。
    #[serde(default)]
    tags: Option<Vec<String>>,
    /// 自由状态标签，如「已发布」「开发中」「废弃」。
    #[serde(default)]
    status: Option<String>,
    /// 查询参数（整体替换）。
    #[serde(default)]
    params: Option<Vec<KvParam>>,
    /// 请求头（整体替换）。
    #[serde(default)]
    headers: Option<Vec<KvParam>>,
    /// 请求体（整体替换）。
    #[serde(default)]
    body: Option<BodyParam>,
    /// 认证配置（整体替换）。
    #[serde(default)]
    auth: Option<AuthParam>,
}

/// rename_node 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RenameParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 接口或目录 id（或唯一名称）。
    id: String,
    /// 新名称。
    new_name: String,
}

/// 以 id 为唯一参数的节点操作（delete）。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct NodeParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 接口或目录 id（或唯一名称）。
    id: String,
}

/// move_node 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct MoveParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 要移动的接口/目录 id。
    id: String,
    /// 目标目录 id/名称；留空表示移动到项目根目录。
    #[serde(default)]
    folder_id: Option<String>,
}

/// set_environment_variable 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct EnvVarParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 环境 id 或名称；不存在时按名称自动创建。
    environment: String,
    /// 变量名。
    key: String,
    /// 变量值。
    value: String,
    /// 是否启用，默认 true。
    #[serde(default)]
    enabled: Option<bool>,
}

/// delete_environment_variable 参数。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DeleteEnvVarParams {
    /// 项目 id 或唯一项目名；留空使用当前激活项目。
    #[serde(default)]
    project_id: Option<String>,
    /// 环境 id 或名称。
    environment: String,
    /// 要删除的变量名。
    key: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn internal_error(msg: impl Into<String>) -> ErrorData {
    ErrorData::internal_error(msg.into(), None)
}

fn pretty(value: &Value) -> Result<String, ErrorData> {
    serde_json::to_string_pretty(value)
        .map_err(|e| internal_error(format!("序列化结果失败: {e}")))
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// Parse a lowercase serde enum (OpMethod / OpProtocol / ...) from user text,
/// accepting any case.
fn parse_enum<T: serde::de::DeserializeOwned>(label: &str, raw: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(raw.trim().to_ascii_lowercase()))
        .map_err(|_| format!("无效的{label}: {raw}"))
}

/// Convert between two serde enums sharing the same wire representation
/// (here: ai::ops wire enums ↔ state model enums, both lowercase).
fn convert_enum<T: serde::Serialize, U: serde::de::DeserializeOwned>(
    value: T,
) -> Result<U, String> {
    let json = serde_json::to_value(value).map_err(|e| e.to_string())?;
    serde_json::from_value(json).map_err(|e| e.to_string())
}

fn kv_to_spec(k: KvParam) -> KvSpec {
    KvSpec {
        key: k.key,
        value: k.value,
        enabled: k.enabled.unwrap_or(true),
        description: k.description,
    }
}

fn body_to_spec(b: BodyParam) -> Result<BodySpec, String> {
    let form_data: Vec<KvSpec> = b
        .form_data
        .unwrap_or_default()
        .into_iter()
        .map(kv_to_spec)
        .collect();
    let urlencoded: Vec<KvSpec> = b
        .urlencoded
        .unwrap_or_default()
        .into_iter()
        .map(kv_to_spec)
        .collect();
    let raw = b.raw.unwrap_or_default();
    // Infer the body type when the caller only supplied content.
    let kind = b.r#type.clone().unwrap_or_else(|| {
        if !form_data.is_empty() {
            "form_data".to_string()
        } else if !urlencoded.is_empty() {
            "urlencoded".to_string()
        } else if !raw.trim().is_empty() {
            "raw".to_string()
        } else {
            "none".to_string()
        }
    });
    let value = json!({
        "type": kind,
        "raw_language": b.raw_language.unwrap_or_else(|| "json".to_string()),
        "raw": raw,
        "form_data": form_data,
        "urlencoded": urlencoded,
    });
    serde_json::from_value(value).map_err(|e| format!("请求体参数无效: {e}"))
}

fn auth_to_spec(a: AuthParam) -> Result<AuthSpec, String> {
    let value = json!({
        "type": a.r#type,
        "token": a.token.unwrap_or_default(),
        "username": a.username.unwrap_or_default(),
        "password": a.password.unwrap_or_default(),
        "key": a.key.unwrap_or_default(),
        "value": a.value.unwrap_or_default(),
        "add_to": a.add_to.unwrap_or_else(|| "header".to_string()),
    });
    serde_json::from_value(value).map_err(|e| format!("认证参数无效: {e}"))
}

/// Turn [`apply_change_ops`] results into a tool response, failing on the
/// first error.
fn op_results(results: Vec<crate::ai::ops::OpResult>) -> Result<Value, String> {
    if let Some(failed) = results.iter().find(|r| !r.ok) {
        return Err(failed
            .error
            .clone()
            .unwrap_or_else(|| failed.summary.clone()));
    }
    let created = results.iter().find_map(|r| r.created_id.clone());
    let summary = results
        .iter()
        .map(|r| r.summary.as_str())
        .collect::<Vec<_>>()
        .join("；");
    Ok(json!({
        "ok": true,
        "summary": summary,
        "created_id": created,
    }))
}

/// Find a folder by id or unique name (tolerant, mirrors [`NodeRef`]).
fn find_folder_by_id_or_name<'a>(
    project: &'a Project,
    sel: &str,
) -> Option<(Vec<String>, &'a Folder)> {
    if let Some(found) = project.find_folder(sel) {
        return Some(found);
    }
    fn walk<'a>(
        folders: &'a [Folder],
        chain: &[String],
        sel: &str,
        hits: &mut Vec<(Vec<String>, &'a Folder)>,
    ) {
        for folder in folders {
            let mut next = chain.to_vec();
            next.push(folder.id.clone());
            if folder.name == sel {
                hits.push((next.clone(), folder));
            }
            walk(&folder.folders, &next, sel, hits);
        }
    }
    let mut hits = Vec::new();
    walk(&project.folders, &[], sel, &mut hits);
    match hits.as_slice() {
        [only] => Some((only.0.clone(), only.1)),
        _ => None,
    }
}

/// Find a request by id or unique name (read-only).
fn find_request_tolerant<'a>(
    project: &'a Project,
    sel: &str,
) -> Option<(Vec<String>, &'a ApiRequest)> {
    if let Some(found) = project.find_request(sel) {
        return Some(found);
    }
    fn walk<'a>(
        folders: &'a [Folder],
        chain: &mut Vec<String>,
        sel: &str,
        hits: &mut Vec<(Vec<String>, &'a ApiRequest)>,
    ) {
        for folder in folders {
            chain.push(folder.id.clone());
            for req in &folder.requests {
                if req.name == sel {
                    hits.push((chain.clone(), req));
                }
            }
            walk(&folder.folders, chain, sel, hits);
            chain.pop();
        }
    }
    let mut hits: Vec<(Vec<String>, &ApiRequest)> = Vec::new();
    for req in &project.requests {
        if req.name == sel {
            hits.push((Vec::new(), req));
        }
    }
    walk(&project.folders, &mut Vec::new(), sel, &mut hits);
    match hits.as_slice() {
        [only] => Some((only.0.clone(), only.1)),
        _ => None,
    }
}

/// Mutable counterpart of [`find_request_tolerant`].
fn find_request_mut_tolerant<'a>(
    project: &'a mut Project,
    sel: &str,
) -> Option<(Vec<String>, &'a mut ApiRequest)> {
    if project.find_request(sel).is_some() {
        return project.find_request_mut(sel);
    }
    let target_id = {
        let mut hits: Vec<&str> = project
            .iter_all_requests()
            .into_iter()
            .filter(|(_, r)| r.name == sel)
            .map(|(_, r)| r.id.as_str())
            .collect();
        if hits.len() == 1 {
            Some(hits.remove(0).to_string())
        } else {
            None
        }
    };
    target_id.and_then(|id| project.find_request_mut(&id))
}

/// Collect request summaries under a folder (recursive).
fn collect_subtree(folder: &Folder, name_path: &str, out: &mut Vec<Value>) {
    for req in &folder.requests {
        out.push(dto::request_summary(name_path, req));
    }
    for child in &folder.folders {
        let next = if name_path.is_empty() {
            child.name.clone()
        } else {
            format!("{name_path} > {}", child.name)
        };
        collect_subtree(child, &next, out);
    }
}

// ---------------------------------------------------------------------------
// Process entry
// ---------------------------------------------------------------------------

/// Run the MCP server over stdio, blocking until the client closes stdin.
pub fn run_stdio() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("verve-mcp")
        .build()?;
    runtime.block_on(async move {
        let service = VerveMcpServer::new().serve(stdio()).await?;
        service.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

/// `verve mcp` CLI entry. `args` are the arguments after `mcp`.
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    let mut data_dir: Option<std::path::PathBuf> = None;
    let mut print_config: Option<ClientKind> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--data-dir" | "-d" => data_dir = iter.next().map(std::path::PathBuf::from),
            "--print-config" | "-c" => {
                let raw = iter.next().ok_or_else(|| {
                    anyhow::anyhow!("-c/--print-config 需要一个参数：generic|claude-desktop|cursor|vscode")
                })?;
                let kind = ClientKind::parse(raw).ok_or_else(|| {
                    anyhow::anyhow!(
                        "未知配置类型 '{raw}'，可选：generic|claude-desktop|cursor|vscode"
                    )
                })?;
                print_config = Some(kind);
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other if other.starts_with("--data-dir=") => {
                data_dir = Some(std::path::PathBuf::from(
                    other.trim_start_matches("--data-dir="),
                ));
            }
            other => eprintln!("未知参数: {other}（使用 `verve mcp --help` 查看用法）"),
        }
    }

    if let Some(dir) = data_dir {
        crate::state::persistence::set_data_dir(dir);
    }

    if let Some(kind) = print_config {
        // Explicit stdout output is the point of --print-config.
        println!("{}", config::client_config(kind));
        return Ok(());
    }

    // The stdio protocol owns stdout; keep startup chatter on stderr.
    eprintln!("verve mcp: serving Model Context Protocol over stdio");
    run_stdio()
}

fn print_help() {
    eprintln!(
        "verve mcp — 以 stdio 方式运行 MCP 服务，供外部 AI 客户端（Claude Desktop / Cursor / Claude Code / VS Code 等）管理 Verve 接口\n\
         \n\
         用法:\n  \
         verve mcp [选项]\n\
         \n\
         选项:\n  \
         -c, --print-config <generic|claude-desktop|cursor|vscode>\n        \
         打印对应客户端的配置 JSON 到标准输出后退出\n  \
         -d, --data-dir <目录>   指定工作区数据目录（默认 ~/.verve）\n  \
         -h, --help              显示本帮助\n\
         \n\
         Claude Code 快速接入:\n  \
         {}\n",
        config::claude_code_command(false)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::persistence;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Point persistence at a fresh per-test temp dir (thread-local guard).
    fn temp_data_dir(tag: &str) -> (persistence::ThreadDataDirGuard, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "verve-mcp-test-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let guard = persistence::set_thread_data_dir(dir.clone());
        (guard, dir)
    }

    fn seed_demo() {
        persistence::save(&crate::state::sample_data::demo_workspace()).expect("seed save");
    }

    fn active_project_id() -> String {
        let data = store::load();
        let idx = store::active_project_index(&data);
        data.projects
            .get(idx)
            .map(|p| p.id.clone())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn list_projects_reflects_demo_workspace() {
        let (_g, _d) = temp_data_dir("list");
        seed_demo();
        let server = VerveMcpServer::new();
        let out = server.list_projects().await.unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["projects"].as_array().unwrap().len() >= 1);
        assert!(v["active_project_id"].is_string());
    }

    #[tokio::test]
    async fn create_project_persists_and_is_visible_on_reload() {
        let (_g, _d) = temp_data_dir("create-project");
        seed_demo();
        let server = VerveMcpServer::new();
        let out = server
            .create_project(Parameters(CreateProjectParams {
                name: "MCP 测试项目".to_string(),
                description: Some("by test".to_string()),
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["created_id"].is_string());

        // Fresh load from disk (simulating another process / the GUI watcher).
        let data = store::load();
        assert!(data
            .projects
            .iter()
            .any(|p| p.name == "MCP 测试项目" && p.description == "by test"));
    }

    #[tokio::test]
    async fn request_crud_roundtrip_through_change_ops() {
        let (_g, _d) = temp_data_dir("crud");
        seed_demo();
        let server = VerveMcpServer::new();

        // Create a folder.
        let folder = server
            .create_folder(Parameters(CreateFolderParams {
                project_id: None,
                parent_id: None,
                name: "订单模块".to_string(),
                description: None,
            }))
            .await
            .unwrap();
        let folder: Value = serde_json::from_str(&folder).unwrap();
        let folder_id = folder["created_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Create a POST request inside it.
        let created = server
            .create_request(Parameters(CreateRequestParams {
                project_id: None,
                folder_id: Some(folder_id.clone()),
                name: "创建订单".to_string(),
                method: "post".to_string(),
                protocol: None,
                url: "{{baseUrl}}/orders".to_string(),
                description: Some("下单接口".to_string()),
                tags: Some(vec!["交易".to_string()]),
                params: None,
                headers: Some(vec![KvParam {
                    key: "X-Trace".to_string(),
                    value: "t-1".to_string(),
                    enabled: Some(true),
                    description: None,
                }]),
                body: Some(BodyParam {
                    r#type: Some("raw".to_string()),
                    raw_language: Some("json".to_string()),
                    raw: Some(r#"{"sku":1}"#.to_string()),
                    form_data: None,
                    urlencoded: None,
                }),
                auth: None,
            }))
            .await
            .unwrap();
        let created: Value = serde_json::from_str(&created).unwrap();
        let req_id = created["created_id"].as_str().unwrap().to_string();

        // list_requests with a keyword filter sees it under the folder.
        let listed = server
            .list_requests(Parameters(ListRequestsParams {
                project_id: None,
                folder_id: Some(folder_id.clone()),
                query: Some("订单".to_string()),
            }))
            .await
            .unwrap();
        let listed: Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(listed["count"], 1);
        assert_eq!(listed["requests"][0]["id"], req_id);
        assert_eq!(listed["requests"][0]["method"], "POST");
        assert!(listed["requests"][0]["folder_path"]
            .as_str()
            .unwrap()
            .contains("订单模块"));

        // get_request returns headers + body.
        let detail = server
            .get_request(Parameters(GetRequestParams {
                project_id: None,
                id: req_id.clone(),
                include_responses: None,
            }))
            .await
            .unwrap();
        let detail: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(detail["request"]["url"], "{{baseUrl}}/orders");
        assert_eq!(detail["request"]["headers"][0]["key"], "X-Trace");
        assert_eq!(detail["request"]["body"]["type"], "raw");

        // Update URL + protocol + status.
        let upd = server
            .update_request(Parameters(UpdateRequestParams {
                project_id: None,
                id: req_id.clone(),
                name: None,
                url: Some("{{baseUrl}}/v2/orders".to_string()),
                method: Some("PUT".to_string()),
                protocol: Some("sse".to_string()),
                description: None,
                tags: None,
                status: Some("已发布".to_string()),
                params: None,
                headers: None,
                body: None,
                auth: None,
            }))
            .await
            .unwrap();
        let upd: Value = serde_json::from_str(&upd).unwrap();
        assert_eq!(upd["ok"], true);

        let data = store::load();
        let idx = store::active_project_index(&data);
        let project = data.projects.get(idx).unwrap();
        let (_, req) = project.find_request(&req_id).unwrap();
        assert_eq!(req.url, "{{baseUrl}}/v2/orders");
        assert_eq!(req.method, crate::state::models::RequestMethod::Put);
        assert_eq!(req.status, "已发布");
        assert_eq!(serde_json::to_value(&req.protocol).unwrap(), "sse");

        // Delete removes it and persists.
        let deleted = server
            .delete_node(Parameters(NodeParams {
                project_id: None,
                id: req_id.clone(),
            }))
            .await
            .unwrap();
        let deleted: Value = serde_json::from_str(&deleted).unwrap();
        assert_eq!(deleted["ok"], true);
        let data = store::load();
        let project = &data.projects[store::active_project_index(&data)];
        assert!(project.find_request(&req_id).is_none());
    }

    #[tokio::test]
    async fn invalid_method_is_rejected_without_writing() {
        let (_g, _d) = temp_data_dir("bad-method");
        seed_demo();
        let server = VerveMcpServer::new();
        let err = server
            .create_request(Parameters(CreateRequestParams {
                project_id: None,
                folder_id: None,
                name: "坏请求".to_string(),
                method: "FETCH".to_string(),
                protocol: None,
                url: "/x".to_string(),
                description: None,
                tags: None,
                params: None,
                headers: None,
                body: None,
                auth: None,
            }))
            .await;
        assert!(err.is_err());
        // Nothing named 坏请求 landed on disk.
        let data = store::load();
        let pid = active_project_id();
        let project = data
            .projects
            .iter()
            .find(|p| p.id == pid)
            .unwrap();
        assert!(project
            .iter_all_requests()
            .into_iter()
            .all(|(_, r)| r.name != "坏请求"));
    }

    #[tokio::test]
    async fn environment_variables_set_and_delete() {
        let (_g, _d) = temp_data_dir("env");
        seed_demo();
        let server = VerveMcpServer::new();
        server
            .set_environment_variable(Parameters(EnvVarParams {
                project_id: None,
                environment: "测试环境".to_string(),
                key: "baseUrl".to_string(),
                value: "https://api.example.com".to_string(),
                enabled: None,
            }))
            .await
            .unwrap();

        let data = store::load();
        let project = &data.projects[store::active_project_index(&data)];
        let env = project
            .environments
            .iter()
            .find(|e| e.name == "测试环境")
            .unwrap();
        assert!(env
            .variables
            .iter()
            .any(|kv| kv.key == "baseUrl" && kv.value == "https://api.example.com"));

        // Upsert changes value in place.
        server
            .set_environment_variable(Parameters(EnvVarParams {
                project_id: None,
                environment: "测试环境".to_string(),
                key: "baseUrl".to_string(),
                value: "https://api2.example.com".to_string(),
                enabled: None,
            }))
            .await
            .unwrap();
        let data = store::load();
        let project = &data.projects[store::active_project_index(&data)];
        let env = project
            .environments
            .iter()
            .find(|e| e.name == "测试环境")
            .unwrap();
        assert_eq!(
            env.variables.iter().filter(|kv| kv.key == "baseUrl").count(),
            1
        );
        assert!(env
            .variables
            .iter()
            .any(|kv| kv.value == "https://api2.example.com"));

        // Delete.
        server
            .delete_environment_variable(Parameters(DeleteEnvVarParams {
                project_id: None,
                environment: "测试环境".to_string(),
                key: "baseUrl".to_string(),
            }))
            .await
            .unwrap();
        let data = store::load();
        let project = &data.projects[store::active_project_index(&data)];
        let env = project
            .environments
            .iter()
            .find(|e| e.name == "测试环境")
            .unwrap();
        assert!(!env.variables.iter().any(|kv| kv.key == "baseUrl"));

        // Deleting a missing variable is an error.
        assert!(server
            .delete_environment_variable(Parameters(DeleteEnvVarParams {
                project_id: None,
                environment: "测试环境".to_string(),
                key: "missing".to_string(),
            }))
            .await
            .is_err());
    }

    #[test]
    fn client_config_snippets_are_well_formed() {
        let generic = config::client_config(ClientKind::Generic);
        let v: Value = serde_json::from_str(&generic).unwrap();
        assert!(v["mcpServers"]["verve"]["command"].is_string());
        assert_eq!(v["mcpServers"]["verve"]["args"][0], "mcp");

        let vscode: Value =
            serde_json::from_str(&config::client_config(ClientKind::VsCode)).unwrap();
        assert_eq!(vscode["servers"]["verve"]["type"], "stdio");

        let cli = config::claude_code_command(true);
        assert!(cli.starts_with("claude mcp add verve -s user -- "));
        assert!(cli.ends_with(" mcp"));
    }

    #[test]
    fn resolve_project_rejects_unknown_and_ambiguous_names() {
        let mut data = crate::state::sample_data::demo_workspace();
        assert!(store::resolve_project(&data, Some("不存在的项目")).is_err());
        // Duplicate a project name to force ambiguity.
        if let Some(p) = data.projects.first().cloned() {
            let mut dup = p;
            dup.id = "dup-id".to_string();
            data.projects.push(dup);
            assert!(store::resolve_project(&data, Some(&data.projects[0].name)).is_err());
        }
    }
}
