//! The change-plan vocabulary shared by the LLM (`propose_changes` tool /
//! direct write tools), the plan-review UI, and the executor.
//!
//! Hard rules enforced here:
//! - **ids are executor-generated** via [`crate::state::models::new_id`]. The
//!   model references existing nodes by real id (from the workspace snapshot)
//!   and same-plan creations by `ref` alias; [`NodeRef`] resolution and the
//!   alias map live in this module.
//! - timestamps and `created_by`/`updated_by` are stamped locally.
//! - enums parse strictly — an unknown value fails that op, never the app.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::state::models::{
    ApiRequest, AuthConfig, BodyType, Folder, KeyValue, MoveTarget, Project, Protocol,
    RawLanguage, RequestMethod,
};

// ---------------------------------------------------------------------------
// Node references
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IdRef {
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AliasRef {
    /// The `ref` alias a Create op declared earlier in the same plan.
    pub r#ref: String,
}

/// Reference to a node: an existing id (`{"id": "..."}`), a plan-local alias
/// (`{"ref": "..."}`), or a bare string (treated as id first, then alias).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum NodeRef {
    Id(IdRef),
    Alias(AliasRef),
    Bare(String),
}

impl NodeRef {
    /// Resolve to a real node id. `refs` maps alias → id for creations made
    /// earlier in the same plan (or simulated in dry-run).
    pub fn resolve(&self, project: &Project, refs: &HashMap<String, String>) -> Option<String> {
        let node_exists =
            |id: &str| project.find_request(id).is_some() || project.find_folder(id).is_some();
        let strict = match self {
            NodeRef::Id(r) => node_exists(&r.id).then(|| r.id.clone()),
            NodeRef::Alias(r) => refs.get(&r.r#ref).cloned(),
            NodeRef::Bare(s) => {
                if node_exists(s) {
                    Some(s.clone())
                } else {
                    refs.get(s).cloned()
                }
            }
        };
        strict.or_else(|| {
            // Tolerant fallback: models frequently substitute the node NAME
            // for the id (and garble long ids from memory). Accept the name
            // when it resolves to exactly one node — mirrors get_request's
            // id-or-unique-name behaviour, keeping read and write paths
            // consistent.
            let text = match self {
                NodeRef::Id(r) => r.id.as_str(),
                NodeRef::Bare(s) => s.as_str(),
                NodeRef::Alias(_) => return None,
            };
            let mut hits: Vec<String> = project
                .requests
                .iter()
                .filter(|r| r.name == text)
                .map(|r| r.id.clone())
                .collect();
            hits.extend(
                project
                    .folders
                    .iter()
                    .filter(|f| f.name == text)
                    .map(|f| f.id.clone()),
            );
            match (hits.first(), hits.len()) {
                (Some(only), 1) => Some(only.clone()),
                _ => None,
            }
        })
    }

    /// Human-readable form for summaries.
    pub fn display(&self) -> String {
        match self {
            NodeRef::Id(r) => r.id.clone(),
            NodeRef::Alias(r) => format!("@{}", r.r#ref),
            NodeRef::Bare(s) => s.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Wire enums (strict, lowercase)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl OpMethod {
    fn to_model(self) -> RequestMethod {
        match self {
            OpMethod::Get => RequestMethod::Get,
            OpMethod::Post => RequestMethod::Post,
            OpMethod::Put => RequestMethod::Put,
            OpMethod::Delete => RequestMethod::Delete,
            OpMethod::Patch => RequestMethod::Patch,
            OpMethod::Head => RequestMethod::Head,
            OpMethod::Options => RequestMethod::Options,
        }
    }
}

/// Protocols creatable via `create_request` (docs and folders have their own
/// ops).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpProtocol {
    #[default]
    Http,
    Sse,
    WebSocket,
    Tcp,
    Grpc,
    SocketIo,
    Graphql,
}

impl OpProtocol {
    fn to_model(self) -> Protocol {
        match self {
            OpProtocol::Http => Protocol::Http,
            OpProtocol::Sse => Protocol::Sse,
            OpProtocol::WebSocket => Protocol::WebSocket,
            OpProtocol::Tcp => Protocol::Tcp,
            OpProtocol::Grpc => Protocol::Grpc,
            OpProtocol::SocketIo => Protocol::SocketIo,
            OpProtocol::Graphql => Protocol::Graphql,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpBodyType {
    #[default]
    None,
    FormData,
    Urlencoded,
    Raw,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpRawLanguage {
    #[default]
    Json,
    Xml,
    Text,
    Html,
    Javascript,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpAuthType {
    #[default]
    None,
    Bearer,
    Basic,
    ApiKey,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpAuthTarget {
    #[default]
    Header,
    Query,
}

// ---------------------------------------------------------------------------
// Specs
// ---------------------------------------------------------------------------

/// One key/value row (params / headers / cookies / form fields).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KvSpec {
    pub key: String,
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_true() -> bool {
    true
}

impl KvSpec {
    fn to_model(&self) -> KeyValue {
        let mut kv = KeyValue::new(&self.key, &self.value);
        kv.enabled = self.enabled;
        kv.description = self.description.clone().unwrap_or_default();
        kv
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BodySpec {
    #[serde(default)]
    pub r#type: OpBodyType,
    #[serde(default)]
    pub raw_language: OpRawLanguage,
    #[serde(default)]
    pub raw: String,
    #[serde(default)]
    pub urlencoded: Vec<KvSpec>,
    #[serde(default)]
    pub form_data: Vec<KvSpec>,
}

impl BodySpec {
    fn to_model(&self) -> crate::state::models::RequestBody {
        crate::state::models::RequestBody {
            body_type: match self.r#type {
                OpBodyType::None => BodyType::None,
                OpBodyType::FormData => BodyType::FormData,
                OpBodyType::Urlencoded => BodyType::Urlencoded,
                OpBodyType::Raw => BodyType::Raw,
            },
            raw_language: match self.raw_language {
                OpRawLanguage::Json => RawLanguage::Json,
                OpRawLanguage::Xml => RawLanguage::Xml,
                OpRawLanguage::Text => RawLanguage::Text,
                OpRawLanguage::Html => RawLanguage::Html,
                OpRawLanguage::Javascript => RawLanguage::Javascript,
            },
            raw: self.raw.clone(),
            form_data: self.form_data.iter().map(|k| k.to_model()).collect(),
            urlencoded: self.urlencoded.iter().map(|k| k.to_model()).collect(),
            raw_parameter: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuthSpec {
    #[serde(default)]
    pub r#type: OpAuthType,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub add_to: OpAuthTarget,
}

impl AuthSpec {
    fn to_model(&self) -> AuthConfig {
        AuthConfig {
            auth_type: match self.r#type {
                OpAuthType::None => crate::state::models::AuthType::None,
                OpAuthType::Bearer => crate::state::models::AuthType::Bearer,
                OpAuthType::Basic => crate::state::models::AuthType::Basic,
                OpAuthType::ApiKey => crate::state::models::AuthType::ApiKey,
            },
            token: self.token.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            key: self.key.clone(),
            value: self.value.clone(),
            add_to: match self.add_to {
                OpAuthTarget::Header => crate::state::models::AuthTarget::Header,
                OpAuthTarget::Query => crate::state::models::AuthTarget::Query,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// ChangeOp
// ---------------------------------------------------------------------------

/// One planned mutation. The same struct is produced by the LLM, rendered in
/// the plan card, and applied by [`apply_change_ops`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum ChangeOp {
    CreateFolder {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<NodeRef>,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
    },
    CreateRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<NodeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
        name: String,
        method: OpMethod,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<OpProtocol>,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        params: Vec<KvSpec>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<KvSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodySpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<AuthSpec>,
    },
    /// A requirement-document (Markdown) node.
    CreateDoc {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<NodeRef>,
        name: String,
        markdown: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<NodeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
    },
    UpdateRequest {
        id: NodeRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        method: Option<OpMethod>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tags: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<Vec<KvSpec>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers: Option<Vec<KvSpec>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<BodySpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<AuthSpec>,
    },
    RenameNode {
        id: NodeRef,
        new_name: String,
    },
    /// Flat on purpose: nested `target:{kind,…}` discriminated shapes are a
    /// top source of LLM JSON mistakes. `folder`/`before`/`after` are
    /// mutually exclusive node refs (name-or-id); all absent → move to root.
    MoveNode {
        id: NodeRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<NodeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<NodeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<NodeRef>,
    },
    LinkDoc {
        doc: NodeRef,
        targets: Vec<NodeRef>,
    },
    UnlinkDoc {
        doc: NodeRef,
        targets: Vec<NodeRef>,
    },
    DeleteNode {
        id: NodeRef,
    },
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// Outcome of a single op (also used for pre-validation of the plan card).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpResult {
    pub op_index: usize,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// One-line human summary for the plan card.
    pub summary: String,
    /// Real id of a created node (echoed back to the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_id: Option<String>,
}

/// User-facing labels for node ids: 「name」 for requests, 目录「name」 for
/// folders; unknown ids fall back to the raw id (model-facing diagnostics).
fn readable_labels(project: &Project, ids: &[String]) -> Vec<String> {
    ids.iter()
        .map(|tid| {
            if let Some((_, r)) = project.find_request(tid) {
                format!("「{}」", r.name)
            } else if let Some((_, f)) = project.find_folder(tid) {
                format!("目录「{}」", f.name)
            } else {
                tid.clone()
            }
        })
        .collect()
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

fn summary_of(op: &ChangeOp) -> String {
    match op {
        ChangeOp::CreateFolder { name, .. } => format!("新建目录「{name}」"),
        ChangeOp::CreateRequest { name, method, url, .. } => {
            format!("新建接口 {method:?} {url} ·「{name}」")
        }
        ChangeOp::CreateDoc { name, .. } => format!("新建需求文档「{name}」"),
        ChangeOp::UpdateRequest { id, .. } => format!("修改接口 {}", id.display()),
        ChangeOp::RenameNode { id, new_name } => format!("重命名 {} →「{new_name}」", id.display()),
        ChangeOp::MoveNode { id, .. } => format!("移动 {}", id.display()),
        ChangeOp::LinkDoc { doc, targets } => {
            format!("文档 {} 关联 {} 个节点", doc.display(), targets.len())
        }
        ChangeOp::UnlinkDoc { doc, targets } => {
            format!("文档 {} 取消关联 {} 个节点", doc.display(), targets.len())
        }
        ChangeOp::DeleteNode { id } => format!("删除 {}", id.display()),
    }
}

/// Insert a request into `parent` (None = project root).
fn push_request(project: &mut Project, parent: Option<&str>, req: ApiRequest) -> Result<String, String> {
    let id = req.id.clone();
    match parent {
        None => project.requests.push(req),
        Some(fid) => {
            let Some((_, folder)) = project.find_folder_mut(fid) else {
                return Err(format!("父目录不存在: {fid}"));
            };
            folder.requests.push(req);
        }
    }
    Ok(id)
}

fn push_folder(project: &mut Project, parent: Option<&str>, folder: Folder) -> Result<String, String> {
    let id = folder.id.clone();
    match parent {
        None => project.folders.push(folder),
        Some(fid) => {
            let Some((_, pfolder)) = project.find_folder_mut(fid) else {
                return Err(format!("父目录不存在: {fid}"));
            };
            pfolder.folders.push(folder);
        }
    }
    Ok(id)
}

/// Whether `id` names a request or folder (doc links may target either).
fn node_exists(project: &Project, id: &str) -> bool {
    project.find_request(id).is_some() || project.find_folder(id).is_some()
}

/// Apply (or dry-validate) a plan sequentially. The first hard failure marks
/// all remaining ops as skipped so the model can revise and re-propose.
fn run_ops(project: &mut Project, ops: &[ChangeOp], dry: bool) -> Vec<OpResult> {
    let mut results = Vec::with_capacity(ops.len());
    let mut refs: HashMap<String, String> = HashMap::new();
    let mut failed = false;

    for (ix, op) in ops.iter().enumerate() {
        if failed {
            results.push(OpResult {
                op_index: ix,
                ok: false,
                error: Some("已跳过（前序步骤失败）".into()),
                summary: summary_of(op),
                created_id: None,
            });
            continue;
        }
        match exec_one(project, op, &refs, dry) {
            Ok(outcome) => {
                if let Some((alias, real_id)) = outcome.alias_registration {
                    refs.insert(alias, real_id);
                }
                results.push(OpResult {
                    op_index: ix,
                    ok: true,
                    error: None,
                    summary: outcome.summary.unwrap_or_else(|| summary_of(op)),
                    created_id: outcome.created_id,
                });
            }
            Err(e) => {
                failed = true;
                results.push(OpResult {
                    op_index: ix,
                    ok: false,
                    error: Some(e),
                    summary: summary_of(op),
                    created_id: None,
                });
            }
        }
    }
    results
}

#[derive(Default)]
struct ExecOutcome {
    created_id: Option<String>,
    alias_registration: Option<(String, String)>,
    /// Readable, user-facing summary (names + location, NOT raw ids).
    summary: Option<String>,
}

fn exec_one(
    project: &mut Project,
    op: &ChangeOp,
    refs: &HashMap<String, String>,
    dry: bool,
) -> Result<ExecOutcome, String> {
    // Dry-run creations register `#alias` placeholders that aren't in the
    // tree — existence checks must accept them.
    let is_placeholder = |id: &str| dry && id.starts_with('#');
    match op {
        ChangeOp::CreateFolder {
            parent,
            name,
            description,
            r#ref: alias,
        } => {
            let parent_id = match parent {
                Some(r) => Some(r.resolve(project, refs).ok_or_else(|| {
                    format!("父目录引用无法解析: {}", r.display())
                })?),
                None => None,
            };
            if let Some(pid) = parent_id.as_deref()
                && !is_placeholder(pid)
                && project.find_folder(pid).is_none()
            {
                return Err(format!("父目录不存在: {pid}"));
            }
            if name.trim().is_empty() {
                return Err("目录名不能为空".into());
            }
            if dry {
                return Ok(outcome_for_alias(alias));
            }
            let mut folder = Folder::new(name);
            folder.description = description.clone().unwrap_or_default();
            let real = push_folder(project, parent_id.as_deref(), folder)?;
            Ok(fresh_outcome(alias, real))
        }
        ChangeOp::CreateRequest {
            parent,
            r#ref: alias,
            name,
            method,
            protocol,
            url,
            description,
            tags,
            params,
            headers,
            body,
            auth,
        } => {
            let parent_id = match parent {
                Some(r) => Some(
                    r.resolve(project, refs)
                        .ok_or_else(|| format!("父目录引用无法解析: {}", r.display()))?,
                ),
                None => None,
            };
            if let Some(pid) = parent_id.as_deref()
                && !is_placeholder(pid)
                && project.find_folder(pid).is_none()
            {
                return Err(format!("父目录不存在: {pid}"));
            }
            if name.trim().is_empty() {
                return Err("接口名不能为空".into());
            }
            if dry {
                return Ok(outcome_for_alias(alias));
            }
            let mut req = ApiRequest::new(name, method.to_model(), url);
            req.protocol = protocol.map(OpProtocol::to_model).unwrap_or(Protocol::Http);
            req.description = description.clone().unwrap_or_default();
            req.tags = tags.clone();
            req.params = params.iter().map(|k| k.to_model()).collect();
            req.headers = headers.iter().map(|k| k.to_model()).collect();
            if let Some(b) = body {
                req.body = b.to_model();
            }
            if let Some(a) = auth {
                req.auth = a.to_model();
            }
            req.created_by = "AI".to_string();
            req.updated_by = "AI".to_string();
            let real = push_request(project, parent_id.as_deref(), req)?;
            Ok(fresh_outcome(alias, real))
        }
        ChangeOp::CreateDoc {
            parent,
            name,
            markdown,
            links,
            r#ref: alias,
        } => {
            let parent_id = match parent {
                Some(r) => Some(
                    r.resolve(project, refs)
                        .ok_or_else(|| format!("父目录引用无法解析: {}", r.display()))?,
                ),
                None => None,
            };
            if let Some(pid) = parent_id.as_deref()
                && !is_placeholder(pid)
                && project.find_folder(pid).is_none()
            {
                return Err(format!("父目录不存在: {pid}"));
            }
            let mut resolved_links = Vec::new();
            for l in links {
                let id = l
                    .resolve(project, refs)
                    .ok_or_else(|| format!("关联目标无法解析: {}", l.display()))?;
                resolved_links.push(id);
            }
            if dry {
                return Ok(outcome_for_alias(alias));
            }
            let mut doc = ApiRequest::new(name, RequestMethod::Get, "");
            doc.protocol = Protocol::Markdown;
            doc.description = markdown.clone();
            doc.created_by = "AI".to_string();
            doc.updated_by = "AI".to_string();
            doc.doc_links = resolved_links;
            let real = push_request(project, parent_id.as_deref(), doc)?;
            Ok(fresh_outcome(alias, real))
        }
        ChangeOp::UpdateRequest {
            id,
            name,
            url,
            method,
            description,
            tags,
            params,
            headers,
            body,
            auth,
        } => {
            let rid = id
                .resolve(project, refs)
                .ok_or_else(|| format!("接口不存在: {}", id.display()))?;
            // Readable summary: node name + which fields change.
            let mut fields: Vec<&str> = Vec::new();
            if name.is_some() { fields.push("名称"); }
            if url.is_some() { fields.push("URL"); }
            if method.is_some() { fields.push("方法"); }
            if description.is_some() { fields.push("描述"); }
            if tags.is_some() { fields.push("标签"); }
            if params.is_some() { fields.push("参数"); }
            if headers.is_some() { fields.push("请求头"); }
            if body.is_some() { fields.push("请求体"); }
            if auth.is_some() { fields.push("认证"); }
            let summary = project.find_request(&rid).map(|(_, r)| {
                format!(
                    "修改接口「{}」{}",
                    r.name,
                    if fields.is_empty() {
                        String::new()
                    } else {
                        format!("（{}）", fields.join("、"))
                    }
                )
            });
            if dry {
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            let Some((_, req)) = project.find_request_mut(&rid) else {
                return Err(format!("接口不存在: {rid}"));
            };
            if let Some(v) = name {
                req.name = v.clone();
            }
            if let Some(v) = url {
                req.url = v.clone();
            }
            if let Some(m) = method {
                req.method = m.to_model();
            }
            if let Some(v) = description {
                req.description = v.clone();
            }
            if let Some(v) = tags {
                req.tags = v.clone();
            }
            if let Some(v) = params {
                req.params = v.iter().map(|k| k.to_model()).collect();
            }
            if let Some(v) = headers {
                req.headers = v.iter().map(|k| k.to_model()).collect();
            }
            if let Some(b) = body {
                req.body = b.to_model();
            }
            if let Some(a) = auth {
                req.auth = a.to_model();
            }
            req.updated_by = "AI".to_string();
            req.updated_at = now_stamp();
            Ok(ExecOutcome { summary, ..Default::default() })
        }
        ChangeOp::RenameNode { id, new_name } => {
            let rid = id
                .resolve(project, refs)
                .ok_or_else(|| format!("节点不存在: {}", id.display()))?;
            if new_name.trim().is_empty() {
                return Err("新名称不能为空".into());
            }
            let summary = project
                .find_request(&rid)
                .map(|(_, r)| format!("重命名「{}」→「{new_name}」", r.name))
                .or_else(|| {
                    project
                        .find_folder(&rid)
                        .map(|(_, f)| format!("重命名目录「{}」→「{new_name}」", f.name))
                });
            if dry {
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            if let Some((_, req)) = project.find_request_mut(&rid) {
                req.name = new_name.clone();
                req.updated_by = "AI".to_string();
                req.updated_at = now_stamp();
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            if let Some((_, folder)) = project.find_folder_mut(&rid) {
                folder.name = new_name.clone();
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            Err(format!("节点不存在: {rid}"))
        }
        ChangeOp::MoveNode {
            id,
            folder,
            before,
            after,
        } => {
            let rid = id
                .resolve(project, refs)
                .ok_or_else(|| format!("节点不存在: {}", id.display()))?;
            let is_folder = project.find_folder(&rid).is_some();
            let mut dest_label = "根目录".to_string();
            let node_label = project
                .find_request(&rid)
                .map(|(_, r)| format!("「{}」", r.name))
                .or_else(|| project.find_folder(&rid).map(|(_, f)| format!("目录「{}」", f.name)))
                .unwrap_or_else(|| rid.clone());
            let specified = folder.iter().chain(before.iter()).chain(after.iter()).count();
            if specified > 1 {
                return Err("folder/before/after 只能指定其中一个（全部缺省 = 移到根目录）".into());
            }
            let mv = if let Some(f) = folder {
                let fid = f
                    .resolve(project, refs)
                    .ok_or_else(|| format!("目标目录无法解析: {}", f.display()))?;
                if !is_placeholder(&fid) && project.find_folder(&fid).is_none() {
                    return Err(format!("目标目录不存在: {fid}"));
                }
                dest_label = project
                    .find_folder(&fid)
                    .map(|(_, ff)| format!("目录「{}」", ff.name))
                    .unwrap_or_else(|| "新目录".to_string());
                MoveTarget::IntoFolder(fid)
            } else if let Some(r) = before.as_ref().or(after.as_ref()) {
                let anchor = r
                    .resolve(project, refs)
                    .ok_or_else(|| format!("参照接口无法解析: {}", r.display()))?;
                if !is_placeholder(&anchor) && project.find_request(&anchor).is_none() {
                    return Err(format!("参照接口不存在: {anchor}"));
                }
                let pos = if before.is_some() { "前" } else { "后" };
                dest_label = project
                    .find_request(&anchor)
                    .map(|(_, rr)| format!("接口「{}」{pos}", rr.name))
                    .unwrap_or_else(|| "指定位置".to_string());
                if before.is_some() {
                    MoveTarget::BeforeRequest(anchor)
                } else {
                    MoveTarget::AfterRequest(anchor)
                }
            } else {
                MoveTarget::ToRoot
            };
            let summary = Some(format!("移动 {node_label} → {dest_label}"));
            if dry {
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            let moved = if is_folder {
                project.move_folder(&rid, &mv)
            } else {
                project.move_request(&rid, &mv)
            };
            if moved {
                Ok(ExecOutcome { summary, ..Default::default() })
            } else {
                Err("移动失败（目标位置无效或会造成循环）".into())
            }
        }
        ChangeOp::LinkDoc { doc, targets } => {
            let did = doc
                .resolve(project, refs)
                .ok_or_else(|| format!("文档不存在: {}", doc.display()))?;
            let mut resolved = Vec::new();
            for t in targets {
                let tid = t
                    .resolve(project, refs)
                    .ok_or_else(|| format!("关联目标无法解析: {}", t.display()))?;
                if tid == did {
                    return Err("文档不能关联自身".into());
                }
                resolved.push(tid);
            }
            let doc_label = project
                .find_request(&did)
                .map(|(_, r)| r.name.clone())
                .unwrap_or_else(|| did.clone());
            let summary = Some(format!(
                "文档「{doc_label}」关联 {} 个节点（{}）",
                resolved.len(),
                readable_labels(project, &resolved).join("、")
            ));
            if dry {
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            {
                let Some((_, d)) = project.find_request_mut(&did) else {
                    return Err(format!("文档不存在: {did}"));
                };
                if d.protocol != Protocol::Markdown {
                    return Err(format!("节点 {} 不是 Markdown 文档", did));
                }
            }
            for tid in &resolved {
                if !node_exists(project, tid) {
                    return Err(format!("关联目标不存在: {tid}"));
                }
            }
            if let Some((_, d)) = project.find_request_mut(&did) {
                for tid in resolved {
                    if !d.doc_links.contains(&tid) {
                        d.doc_links.push(tid);
                    }
                }
                d.updated_by = "AI".to_string();
                d.updated_at = now_stamp();
            }
            Ok(ExecOutcome { summary, ..Default::default() })
        }
        ChangeOp::UnlinkDoc { doc, targets } => {
            let did = doc
                .resolve(project, refs)
                .ok_or_else(|| format!("文档不存在: {}", doc.display()))?;
            let mut resolved = Vec::new();
            for t in targets {
                resolved.push(
                    t.resolve(project, refs)
                        .ok_or_else(|| format!("目标无法解析: {}", t.display()))?,
                );
            }
            let doc_label = project
                .find_request(&did)
                .map(|(_, r)| r.name.clone())
                .unwrap_or_else(|| did.clone());
            let summary = Some(format!(
                "文档「{doc_label}」取消关联 {} 个节点（{}）",
                resolved.len(),
                readable_labels(project, &resolved).join("、")
            ));
            if dry {
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            let Some((_, d)) = project.find_request_mut(&did) else {
                return Err(format!("文档不存在: {did}"));
            };
            for tid in resolved {
                d.doc_links.retain(|l| *l != tid);
            }
            d.updated_by = "AI".to_string();
            d.updated_at = now_stamp();
            Ok(ExecOutcome::default())
        }
        ChangeOp::DeleteNode { id } => {
            let rid = id
                .resolve(project, refs)
                .ok_or_else(|| format!("节点不存在: {}", id.display()))?;
            let summary = project
                .find_folder(&rid)
                .map(|(_, f)| Some(format!("删除目录「{}」（含全部子内容）", f.name)))
                .or_else(|| {
                    project
                        .find_request(&rid)
                        .map(|(_, r)| Some(format!("删除「{}」", r.name)))
                })
                .flatten();
            if dry {
                return Ok(ExecOutcome { summary, ..Default::default() });
            }
            if project.remove_node(&rid) {
                Ok(ExecOutcome { summary, ..Default::default() })
            } else {
                Err(format!("节点不存在: {rid}"))
            }
        }
    }
}

/// Dry-run placeholder outcome registering only the alias (if any).
fn outcome_for_alias(alias: &Option<String>) -> ExecOutcome {
    ExecOutcome {
        created_id: alias.as_ref().map(|a| format!("#{a}")),
        alias_registration: alias
            .clone()
            .map(|a| (a.clone(), format!("#{a}"))),
        summary: None,
    }
}

fn fresh_outcome(alias: &Option<String>, real_id: String) -> ExecOutcome {
    ExecOutcome {
        created_id: Some(real_id.clone()),
        alias_registration: alias.clone().map(|a| (a, real_id)),
        summary: None,
    }
}

/// Validate a plan against the current tree without mutating anything.
/// Ops referencing earlier creations resolve via simulated alias ids.
pub fn validate_ops(project: &Project, ops: &[ChangeOp]) -> Vec<OpResult> {
    run_ops(&mut project.clone(), ops, true)
}

/// Apply a plan; ids for new nodes are generated here (never by the model).
pub fn apply_change_ops(project: &mut Project, ops: &[ChangeOp]) -> Vec<OpResult> {
    run_ops(project, ops, false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::collections::HashMap;

    use super::{ChangeOp, NodeRef, apply_change_ops, validate_ops};
    use crate::state::models::{Project, Protocol, RequestMethod};

    fn sample() -> Project {
        let mut p = Project::new("测试");
        p.requests
            .push(crate::state::models::ApiRequest::new(
                "登录",
                RequestMethod::Post,
                "/login",
            ));
        p.folders
            .push(crate::state::models::Folder::new("模块A"));
        p
    }

    #[test]
    fn node_ref_resolves_unique_name_fallback() {
        let p = sample();
        let rid = p.requests.first().unwrap().id.clone();
        // Exact id still wins; a unique name resolves to the real id…
        assert_eq!(
            NodeRef::Id(super::IdRef { id: rid.clone() }).resolve(&p, &HashMap::new()),
            Some(rid.clone())
        );
        assert_eq!(
            NodeRef::Id(super::IdRef { id: "登录".into() }).resolve(&p, &HashMap::new()),
            Some(rid)
        );
        // Ambiguous / unknown names stay unresolved.
        assert_eq!(
            NodeRef::Id(super::IdRef { id: "不存在".into() }).resolve(&p, &HashMap::new()),
            None
        );
    }

    #[test]
    fn move_node_flat_forms_parse_and_run() {
        let mut p = sample();
        p.folders.push(crate::state::models::Folder::new("账号中心"));
        let login_id = p.requests.first().unwrap().id.clone();
        let folder_name = p.folders[1].name.clone();
        // 平铺 + 名称引用：LLM 最自然的写法。
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "move_node", "id": login_id, "folder": folder_name}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(results[0].ok, "{results:?}");
        assert!(results[0].summary.contains("账号中心"));
        // 全缺省 = 移到根。
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "move_node", "id": login_id}
        ]))
        .unwrap();
        let r2 = apply_change_ops(&mut p, &ops);
        assert!(r2[0].ok, "{r2:?}");
        // before/after 排序形式（锚点必须是另一个接口）。
        p.requests
            .push(crate::state::models::ApiRequest::new("注册", RequestMethod::Post, "/register"));
        let reg_id = p.requests.get(1).map(|r| r.id.clone()).unwrap_or_default();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "move_node", "id": login_id, "after": reg_id}
        ]))
        .unwrap();
        let r3 = apply_change_ops(&mut p, &ops);
        assert!(r3[0].ok, "{r3:?}");
        // 同时给 folder + after → 明确报错。
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "move_node", "id": login_id, "folder": "账号中心", "after": login_id}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(!results[0].ok);
        assert!(results[0].error.as_deref().unwrap().contains("只能指定"));
    }

    #[test]
    fn node_ref_serde_shapes() {
        assert_eq!(
            serde_json::from_str::<NodeRef>(r#"{"id":"abc"}"#).unwrap(),
            NodeRef::Id(super::IdRef { id: "abc".into() })
        );
        assert_eq!(
            serde_json::from_str::<NodeRef>(r#"{"ref":"auth"}"#).unwrap(),
            NodeRef::Alias(super::AliasRef { r#ref: "auth".into() })
        );
        assert_eq!(
            serde_json::from_str::<NodeRef>(r#""plain""#).unwrap(),
            NodeRef::Bare("plain".into())
        );
    }

    #[test]
    fn create_plan_with_ref_chain_and_links() {
        let mut p = sample();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "create_folder", "name": "认证", "ref": "auth_folder"},
            {"op": "create_request", "parent": {"ref": "auth_folder"}, "ref": "login_api",
             "name": "登录", "method": "post", "url": "{{baseUrl}}/auth/login"},
            {"op": "create_doc", "name": "需求", "markdown": "# 需求", "ref": "req_doc",
             "links": [{"ref": "login_api"}, {"ref": "auth_folder"}]},
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        let folder = p.folders.iter().find(|f| f.name == "认证").unwrap();
        assert_eq!(folder.requests.len(), 1);
        assert_eq!(folder.requests[0].method, RequestMethod::Post);
        assert_eq!(folder.requests[0].created_by, "AI");
        let Some((_, doc)) = p
            .iter_all_requests()
            .into_iter()
            .find(|(_, r)| r.protocol == Protocol::Markdown)
        else {
            panic!("doc not created");
        };
        assert_eq!(doc.doc_links.len(), 2);
        // Doc links point at the real generated ids.
        assert!(doc.doc_links.contains(&folder.requests[0].id));
        assert!(doc.doc_links.contains(&folder.id));
        // Reverse lookup works.
        assert_eq!(p.docs_linking_to(&folder.requests[0].id).len(), 1);
    }

    #[test]
    fn summaries_are_readable_no_raw_ids() {
        let mut p = sample();
        let login_id = p.requests[0].id.clone();
        let folder_id = p.folders[0].id.clone();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "rename_node", "id": login_id, "new_name": "用户登录"},
            {"op": "move_node", "id": login_id, "folder": folder_id},
            {"op": "update_request", "id": login_id, "description": "d"},
            {"op": "delete_node", "id": folder_id}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        for (r, id) in results.iter().zip([&login_id, &folder_id, &login_id, &folder_id]) {
            let text = &r.summary;
            assert!(
                !text.contains(id.as_str()),
                "summary leaks raw id: {text}"
            );
            assert!(text.contains('「'), "summary should use readable names: {text}");
        }
        assert!(results[0].summary.contains("重命名"));
        assert!(results[1].summary.contains("模块A"));
        assert!(results[2].summary.contains("描述"));
        assert!(results[3].summary.contains("删除目录"));
    }

    #[test]
    fn move_and_rename_via_real_ids() {
        let mut p = sample();
        let login_id = p.requests[0].id.clone();
        let folder_id = p.folders[0].id.clone();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "move_node", "id": login_id, "folder": folder_id},
            {"op": "rename_node", "id": {"id": folder_id}, "new_name": "模块B"},
            {"op": "rename_node", "id": login_id, "new_name": "用户登录"}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        assert_eq!(p.requests.len(), 0);
        assert_eq!(p.folders[0].name, "模块B");
        assert_eq!(p.folders[0].requests[0].name, "用户登录");
    }

    #[test]
    fn unknown_id_fails_and_skips_rest() {
        let mut p = sample();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "rename_node", "id": "ghost", "new_name": "x"},
            {"op": "create_folder", "name": "不会执行"}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(!results[0].ok);
        assert!(results[0].error.as_deref().unwrap_or("").contains("不存在"));
        assert!(!results[1].ok);
        assert!(p.folders.iter().all(|f| f.name != "不会执行"));
    }

    #[test]
    fn validate_is_dry_and_mutation_is_real() {
        let mut p = sample();
        let login_id = p.requests[0].id.clone();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "create_folder", "name": "V", "ref": "vf"},
            {"op": "move_node", "id": login_id, "folder": {"ref": "vf"}}
        ]))
        .unwrap();
        let check = validate_ops(&p, &ops);
        assert!(check.iter().all(|r| r.ok), "{check:?}");
        assert_eq!(p.folders.len(), 1, "dry run must not mutate");
        let applied = apply_change_ops(&mut p, &ops);
        assert!(applied.iter().all(|r| r.ok));
        assert_eq!(p.folders.len(), 2);
        assert_eq!(p.requests.len(), 0);
    }

    #[test]
    fn strict_enum_rejects_garbage() {
        let bad = serde_json::from_value::<Vec<ChangeOp>>(json!([
            {"op": "create_request", "name": "x", "method": "FETCH", "url": "/"}
        ]));
        assert!(bad.is_err());
    }

    #[test]
    fn unlink_and_delete() {
        let mut p = sample();
        let login_id = p.requests[0].id.clone();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "create_doc", "name": "D", "markdown": "m", "links": [login_id.clone()], "ref": "d"},
            {"op": "unlink_doc", "doc": {"ref": "d"}, "targets": [login_id.clone()]}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(results.iter().all(|r| r.ok), "{results:?}");
        let doc = p
            .iter_all_requests()
            .into_iter()
            .find(|(_, r)| r.protocol == Protocol::Markdown)
            .map(|(_, r)| r)
            .unwrap();
        assert!(doc.doc_links.is_empty());
        // Delete the doc.
        let doc_id = doc.id.clone();
        let del: Vec<ChangeOp> =
            serde_json::from_value(json!([{"op": "delete_node", "id": doc_id}])).unwrap();
        let results = apply_change_ops(&mut p, &del);
        assert!(results[0].ok);
        assert!(p.iter_all_requests().into_iter().all(|(_, r)| r.id != doc_id));
    }

    #[test]
    fn self_link_rejected() {
        let mut p = sample();
        let ops: Vec<ChangeOp> = serde_json::from_value(json!([
            {"op": "create_doc", "name": "D", "markdown": "m", "ref": "d"},
            {"op": "link_doc", "doc": {"ref": "d"}, "targets": [{"ref": "d"}]}
        ]))
        .unwrap();
        let results = apply_change_ops(&mut p, &ops);
        assert!(results[0].ok);
        assert!(!results[1].ok);
    }
}
