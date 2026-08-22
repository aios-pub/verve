//! Agent runtime: session loop, tools, skills, and context management.

pub mod context;
pub mod skills;
pub mod tools;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use gpui::Entity;

use super::chat::{
    ChatMessage, ChatRequest, ContentPart, ImageUrl, MessageContent, Role, StreamEvent, ToolArgDiag,
    ToolCall, Usage,
};
use super::client::AiClient;
use super::config::{AgentMode, AiProviderConfig, AiSettings};
use super::ops::{ChangeOp, OpResult};
use crate::state::AppState;

/// Which phase the running turn is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Streaming model output (status words rotate).
    Streaming,
    /// A plan card is pending user decision (plan mode only).
    AwaitingPlan,
}

/// User's verdict on a proposed plan. `Revise` carries the user's optional
/// adjustment note typed next to the button in the plan card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanDecision {
    Apply,
    Revise { note: String },
    Reject,
}

/// Codex-style working-plan step status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// One step of the agent-maintained working plan. The `detail` field is
/// Verve's addition over Codex's update_plan: specifics (node names, field
/// values, grouping rules) live here so context compaction can never lose
/// them — the whole plan is re-injected into the system prompt each request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// 一句话步骤标题（≤30 字）。
    pub step: String,
    /// 本步骤的关键细节：要创建/修改的节点、字段取值、归类规则等。
    #[serde(default)]
    pub detail: String,
    pub status: PlanStatus,
}

/// Enforce codex plan discipline: at most ONE `in_progress` step (the last
/// one wins, earlier ones fall back to pending). Pure helper (unit-tested).
pub fn normalize_plan_steps(mut steps: Vec<PlanStep>) -> Vec<PlanStep> {
    let mut seen_in_progress = false;
    for st in steps.iter_mut().rev() {
        if st.status == PlanStatus::InProgress {
            if seen_in_progress {
                st.status = PlanStatus::Pending;
            } else {
                seen_in_progress = true;
            }
        }
    }
    for st in steps.iter_mut() {
        if st.step.chars().count() > 60 {
            let clipped: String = st.step.chars().take(60).collect();
            st.step = clipped;
        }
        if st.detail.chars().count() > 500 {
            let clipped: String = st.detail.chars().take(500).collect();
            st.detail = clipped;
        }
    }
    steps
}

/// Render the working plan for the system prompt (rebuilt every request, so
/// it survives auto-compaction by construction).
pub fn work_plan_prompt(steps: &[PlanStep]) -> String {
    if steps.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n当前工作计划（由 update_plan 工具维护，每次请求都会原样带上、自动压缩不会丢失它；多步任务的细节必须写进各步骤的 detail，不要只留在对话里）：\n");
    for (i, st) in steps.iter().enumerate() {
        let mark = match st.status {
            PlanStatus::Pending => "待办",
            PlanStatus::InProgress => "进行中",
            PlanStatus::Completed => "已完成",
        };
        out.push_str(&format!("{}. [{}] {}", i + 1, mark, st.step));
        if !st.detail.trim().is_empty() {
            out.push_str(&format!(" —— {}", st.detail.trim()));
        }
        out.push('\n');
    }
    out
}

/// Live 429-retry progress surfaced to the panel status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryStatus {
    pub attempt: u32,
    pub max: u32,
    pub wait_secs: u64,
}

/// One attachment staged for the next message.
#[derive(Clone, Debug)]
pub enum Attachment {
    Image { name: String, data_url: String },
    TextFile { name: String, content: String },
}

impl Attachment {
    pub fn name(&self) -> &str {
        match self {
            Attachment::Image { name, .. } | Attachment::TextFile { name, .. } => name,
        }
    }
}

/// Token/cost line shown under a finished assistant message.
#[derive(Clone, Debug, Default)]
pub struct UsageLine {
    pub prompt: u64,
    pub completion: u64,
    pub estimated: bool,
    pub cost: Option<f64>,
    pub currency: Option<String>,
}

impl UsageLine {
    pub fn display(&self) -> String {
        let est = if self.estimated { "≈" } else { "" };
        let mut s = format!("输入 {est}{} tok · 输出 {est}{} tok", self.prompt, self.completion);
        if let Some(c) = self.cost {
            let cur = self.currency.clone().unwrap_or_default();
            s.push_str(&format!(" · {cur}{c:.4}"));
        }
        s
    }
}

/// One bubble/card in the transcript.
pub enum TranscriptItem {
    User {
        text: String,
        attachment_names: Vec<String>,
    },
    Assistant {
        text: String,
        streaming: bool,
        usage: Option<UsageLine>,
    },
    Tool {
        name: String,
        summary: String,
        ok: bool,
    },
    /// Codex-style working plan; updated in place (the card tracks the
    /// latest state instead of stacking one card per update).
    WorkPlan { steps: Vec<PlanStep> },
    Plan {
        ops: Vec<ChangeOp>,
        validation: Vec<OpResult>,
        applied: Option<Vec<OpResult>>,
        /// Sender for the in-flight decision (consumed on first use).
        decision_tx: Option<smol::channel::Sender<PlanDecision>>,
        decided: Option<PlanDecision>,
    },
    Error(String),
    /// Neutral note (loop-limit reached, stopped, …).
    Note(String),
}

/// Assemble the system prompt: base rules + mode rule + skill + style guide
/// + workspace snapshot + selection.
pub fn build_system_prompt(
    mode: AgentMode,
    skill_id: &str,
    overview: Option<&Value>,
    selection: Option<&str>,
    work_plan: &[PlanStep],
) -> String {
    let mut out = String::new();
    out.push_str(skills::BASE_RULES);
    out.push_str("\n\n");
    out.push_str(match mode {
        AgentMode::Plan => skills::PLAN_MODE_RULE,
        AgentMode::FullAccess => skills::FULL_ACCESS_RULE,
    });
    out.push_str("\n\n");
    out.push_str("\n\n");
    out.push_str(&skills::skill_index());
    out.push_str("\n\n当前技能：");
    out.push_str(&skills::skill_prompt(skill_id));
    if let Some(ov) = overview {
        out.push_str("\n\n当前工作区概览（渐进式加载：此处只有计数与顶层名称，不含 id/详情；需要结构时调用 list_workspace 逐层查看，需要单节点详情用 get_request / get_doc）：\n");
        out.push_str(&ov.to_string());
    }
    if let Some(sel) = selection.filter(|s| !s.is_empty()) {
        out.push_str("\n\n用户当前选中：");
        out.push_str(sel);
    }
    out.push_str(&work_plan_prompt(work_plan));
    out
}

/// AI file-write safety net (plan §"涉及文件修改"):
/// ① copy `workspace.json` → `.bak` **before** touching anything;
/// ② run `apply` (mutation only) then persist via `notify_workspace`;
/// ③ parse-check the persisted file (JSON round-trip);
/// ④ on corruption, restore the backup, reload from disk and return Err —
///    the user-visible plan card / tool card reports the rollback. In plan
///    mode step ② additionally only happens after the user approved.
fn apply_guarded<T>(
    state: &Entity<AppState>,
    cx: &mut gpui::AsyncApp,
    apply: impl FnOnce(&mut AppState, &mut gpui::Context<AppState>) -> T,
) -> Result<T, String> {
    let backup = crate::state::persistence::backup_workspace_file();
    if let Some(bak) = &backup {
        log::info!("AI 变更前已备份 workspace → {}", bak.display());
    }
    let out = state.update(cx, |s, cx| {
        let t = apply(s, cx);
        s.notify_workspace(cx);
        t
    });
    match crate::state::persistence::validate_workspace_file() {
        Ok(()) => Ok(out),
        Err(e) => {
            log::error!("AI 变更后 workspace.json 校验失败：{e}");
            if backup.is_some() {
                match crate::state::persistence::restore_workspace_backup() {
                    Ok(()) => {
                        let ws_id = state.update(cx, |s, _| s.active_workspace_id.clone());
                        state.update(cx, |s, _| s.reload_from_disk(ws_id));
                        log::warn!("已从备份恢复 workspace.json");
                    }
                    Err(re) => log::error!("恢复备份失败：{re}（备份文件保留在 .bak）"),
                }
            }
            Err(format!("已自动回滚（workspace.json 校验失败: {e}）"))
        }
    }
}

fn estimate_usage(req: &ChatRequest, completion: &ChatMessage) -> Usage {
    let mut prompt: u64 = 0;
    for m in &req.messages {
        prompt += context::estimate_tokens(&m.text());
        if let Some(calls) = m.tool_calls.as_deref() {
            for c in calls {
                prompt += context::estimate_tokens(&c.function.arguments);
            }
        }
    }
    let mut done = context::estimate_tokens(&completion.text());
    if let Some(calls) = completion.tool_calls.as_deref() {
        for c in calls {
            done += context::estimate_tokens(&c.function.arguments);
        }
    }
    Usage {
        prompt_tokens: prompt,
        completion_tokens: done,
        total_tokens: prompt + done,
    }
}

/// History auto-compaction (see `compact_history`): when the conversation
/// exceeds this many chars (≈ a third of a 128k-token window), the older
/// part is summarized by the model itself and replaced with the summary.
const COMPACT_THRESHOLD_CHARS: usize = 60_000;
/// How many trailing messages always survive compaction verbatim.
const COMPACT_KEEP_TAIL: usize = 8;

/// Marker prefix on the rolling summary message injected into history.
/// Subsequent compactions feed it back into the summarizer so earlier
/// summaries are merged forward instead of silently dropped.
pub const COMPACT_SUMMARY_MARKER: &str = "（此前对话已自动压缩为摘要）";

/// Soft char budget for the verbatim tail (Cline-style
/// `preserveRecentTokens`): keep the largest recent suffix within this
/// budget instead of a fixed message count, so giant tool results near the
/// end still leave compaction worthwhile.
const COMPACT_TAIL_MAX_CHARS: usize = 28_000;

/// Split point for compaction: everything before the boundary gets
/// summarized; the tail is kept verbatim. Only user-message starts are
/// valid cuts — tool exchanges never span a user message, so cutting there
/// keeps every assistant `tool_calls` / tool-result pair intact (strict
/// gateways 400 on orphaned halves). Among valid cuts (≥ 4 messages to
/// summarize, ≥ `keep_tail` kept) prefer the one keeping the largest tail
/// within `COMPACT_TAIL_MAX_CHARS`; if none fits, summarize as much as
/// allowed. Pure helper (unit-tested).
fn split_for_compaction(history: &[ChatMessage], keep_tail: usize) -> Option<(usize, usize)> {
    if history.len() <= keep_tail + 4 {
        return None;
    }
    let total: usize = history.iter().map(|m| m.text().len()).sum();
    if total < COMPACT_THRESHOLD_CHARS {
        return None;
    }
    let max_cut = history.len() - keep_tail;
    let candidates: Vec<usize> = (4..=max_cut)
        .filter(|&i| history.get(i).map_or(false, |m| m.role == Role::User))
        .collect();
    if candidates.is_empty() {
        // No user-message cut point in range — refuse to split unsafely.
        return None;
    }
    // Largest tail within budget = earliest candidate whose tail fits.
    let suffix_chars = |cut: usize| -> usize {
        history.get(cut..).map_or(0, |rest| rest.iter().map(|m| m.text().len()).sum())
    };
    let boundary = candidates
        .iter()
        .copied()
        .find(|&c| suffix_chars(c) <= COMPACT_TAIL_MAX_CHARS)
        .or_else(|| candidates.last().copied())?;
    let head_chars: usize = history[..boundary].iter().map(|m| m.text().len()).sum();
    Some((boundary, head_chars))
}

/// Verbatim-text copy of a message with image parts stripped — used to keep
/// the original task (first user message) alive across compactions without
/// re-sending its base64 images on every request.
fn text_only_copy(msg: &ChatMessage) -> Option<ChatMessage> {
    match msg.content.as_ref()? {
        MessageContent::Text(_) => Some(msg.clone()),
        MessageContent::Parts(parts) => {
            let texts: Vec<String> = parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                return None;
            }
            Some(ChatMessage {
                role: msg.role,
                content: Some(MessageContent::Text(texts.join("\n"))),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            })
        }
    }
}

/// Compact prompt (condenser-style): preserve the goal and current status,
/// applied changes with readable names and the id mapping the model still
/// needs for tools, decisions, encountered problems, open items and user
/// preferences. A prior summary riding in the head is rendered as
/// 「此前摘要」 and merged forward (rolling summary).
fn compaction_prompt(head: &[ChatMessage]) -> String {
    let mut transcript = String::new();
    for m in head {
        let text = m.text();
        if text.is_empty() {
            continue;
        }
        let role = match m.role {
            Role::System => continue,
            Role::User => "用户",
            Role::Assistant => "助手",
            Role::Tool => "工具结果",
        };
        // Prior summaries are the model's own distilled output — keep them
        // whole instead of clipping at the per-message limit.
        let (label, limit) = if text.starts_with(COMPACT_SUMMARY_MARKER) {
            ("此前摘要", 4_000usize)
        } else {
            (role, 800)
        };
        let mut snippet: String = text.chars().take(limit).collect();
        if snippet.chars().count() < text.chars().count() {
            snippet.push('…');
        }
        transcript.push_str(&format!("{label}: {snippet}\n"));
    }
    format!(
        "请把以下 AI 助手与用户的对话历史压缩成一份要点摘要（中文，不超过 800 字），必须保留：\n\
1) 用户的原始目标与当前进度；\n\
2) 已执行/已确认的变更及涉及的节点名称（保留 name↔id 对应关系，id 原样保留）；\n\
3) 已做出的重要决定；\n\
4) 遇到的问题/报错及其处理结果；\n\
5) 尚未完成或等待用户决定的事项与下一步；\n\
6) 用户表达的偏好。\n\
若对话历史中含「此前摘要」，请将它与后续新对话合并为一份反映最新状态的摘要，不要分段重复旧摘要。\n\
只输出摘要正文，不要任何前言或解释。\n\n--- 对话历史 ---\n{transcript}"
    )
}

/// Actionable error for a tool call whose streamed arguments did not
/// assemble into valid JSON — the model must know the payload was mangled
/// (usually output-limit truncation on a huge plan) instead of getting a
/// bare "missing parameter" it can only blindly retry.
fn bad_args_error(diag: &ToolArgDiag, length_capped: bool) -> String {
    let trunc = if length_capped {
        "且本轮输出达到长度上限（finish=length），内容被截断"
    } else {
        "（疑似流式分片拼装损坏）"
    };
    format!(
        "{} 的参数未成功送达：流式拼装后的 JSON 非法{}。原始参数约 {} 字符，开头为：{}。\
请改用更小的批次重新提交（每次不超过 8 个 op），不要原样重发同一份大参数。",
        diag.name, trunc, diag.raw_len, diag.head
    )
}

/// Escalating guidance on consecutive identical tool failures; the fourth
/// identical failure aborts the turn (see run_turn).
fn repeated_failure_guidance(streak: usize) -> Option<&'static str> {
    match streak {
        3 => Some(
            "（这是连续第 3 次完全相同的失败。请改变做法：分批提交更小的 ops、或先用 list_workspace/search_workspace 核对 id。若第 4 次仍相同，本轮将自动终止。）",
        ),
        _ => None,
    }
}

/// Extract a verified name↔id table from the tool-result messages that are
/// about to be compacted away. The table is code-extracted (never
/// model-recalled) and rides along with the summary, so after compaction
/// the model still has REAL ids to reference instead of reconstructing
/// (and garbling) them from prose memory.
fn extract_id_table(middle: &[ChatMessage]) -> Vec<(String, String)> {
    fn walk(v: &Value, out: &mut Vec<(String, String)>) {
        match v {
            Value::Object(m) => {
                if let (Some(id), Some(name)) = (
                    m.get("id").and_then(Value::as_str),
                    m.get("name").and_then(Value::as_str),
                ) && !id.is_empty()
                    && !name.is_empty()
                {
                    out.push((name.to_string(), id.to_string()));
                }
                for (_, x) in m {
                    walk(x, out);
                }
            }
            Value::Array(a) => {
                for x in a {
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for m in middle {
        if m.role != Role::Tool {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&m.text()) {
            walk(&v, &mut out);
        }
    }
    out.dedup_by(|a, b| a == b);
    out
}

/// Cap a tool result before it enters history (OpenHands caps summarizer
/// input at 10k chars/event, Cline at 2k): giant `list_workspace` payloads
/// otherwise bloat every subsequent request until compaction folds them.
const TOOL_RESULT_MAX_CHARS: usize = 8_000;

fn clamp_tool_result(result: String) -> String {
    if result.chars().count() <= TOOL_RESULT_MAX_CHARS {
        return result;
    }
    let clipped: String = result.chars().take(TOOL_RESULT_MAX_CHARS).collect();
    format!("{clipped}…（工具输出超长已截断，原文 {} 字符）", result.chars().count())
}

/// Guarantee every assistant `tool_calls` in history is fully answered by
/// tool-result messages before any later non-tool message: when a turn is
/// interrupted mid-exchange (stop button, error), the unanswered calls get a
/// stub result — strict gateways reject orphaned halves with a 400.
/// Returns how many stubs were inserted.
pub fn pad_missing_tool_results(history: &mut Vec<ChatMessage>) -> usize {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(history.len() + 4);
    let mut pending: Vec<ToolCall> = Vec::new();
    let mut padded = 0usize;
    for msg in history.drain(..) {
        if msg.role == Role::Tool {
            if let Some(id) = msg.tool_call_id.as_deref() {
                pending.retain(|c| c.id != id);
            }
        } else if !pending.is_empty() {
            for call in pending.drain(..) {
                padded += 1;
                out.push(ChatMessage::tool_result(
                    &call,
                    r#"{"error":"该工具调用未执行（会话被中断）"}"#.to_string(),
                ));
            }
        }
        if let Some(calls) = msg.tool_calls.as_ref() {
            pending.extend(calls.iter().cloned());
        }
        out.push(msg);
    }
    for call in pending.drain(..) {
        padded += 1;
        out.push(ChatMessage::tool_result(
            &call,
            r#"{"error":"该工具调用未执行（会话被中断）"}"#.to_string(),
        ));
    }
    *history = out;
    padded
}

/// The agent session entity: conversation history, transcript, turn state.
pub struct AgentSession {
    pub state: Entity<AppState>,
    pub history: Vec<ChatMessage>,
    pub items: Vec<TranscriptItem>,
    pub attachments: Vec<Attachment>,
    pub running: bool,
    pub phase: Phase,
    pub iteration: usize,
    pub mode: AgentMode,
    pub skill_id: String,
    pub status_idx: usize,
    /// Status-word pool flag for the current turn (true = CJK slang pool),
    /// decided by whether the user's message contains CJK.
    pub status_zh: bool,
    /// Non-zero while the client is waiting out a 429 retry.
    pub retry: Option<RetryStatus>,
    pub stop: Arc<AtomicBool>,
    /// Cumulative session usage + per-turn lines for the detail popover.
    pub usage_total: Usage,
    pub turn_lines: Vec<UsageLine>,
    /// Codex-style working plan (see [`PlanStep`]); injected into every
    /// system prompt so compaction cannot lose it.
    pub work_plan: Vec<PlanStep>,
    cost_currency: Option<String>,
    input_price: Option<f64>,
    output_price: Option<f64>,
    price_unit_div: f64,
    pub last_error: Option<String>,
}

impl AgentSession {
    pub fn new(state: Entity<AppState>, _window: &mut gpui::Window, _cx: &mut gpui::Context<Self>) -> Self {
        let settings = super::config::load_ai_settings();
        Self {
            state,
            history: Vec::new(),
            items: Vec::new(),
            attachments: Vec::new(),
            running: false,
            phase: Phase::Streaming,
            iteration: 0,
            mode: settings.agent_mode,
            skill_id: "general".to_string(),
            status_idx: 0,
            status_zh: true,
            retry: None,
            stop: Arc::new(AtomicBool::new(false)),
            usage_total: Usage::default(),
            turn_lines: Vec::new(),
            work_plan: Vec::new(),
            cost_currency: None,
            input_price: None,
            output_price: None,
            price_unit_div: 1_000_000.0,
            last_error: None,
        }
    }

    /// Re-read behaviour settings (called after settings edits / mode flips).
    pub fn refresh_settings(&mut self, cx: &mut gpui::Context<Self>) {
        let settings = super::config::load_ai_settings();
        self.mode = settings.agent_mode;
        cx.notify();
    }

    pub fn set_mode(&mut self, mode: AgentMode, cx: &mut gpui::Context<Self>) {
        self.mode = mode;
        let mut s = super::config::load_ai_settings();
        s.agent_mode = mode;
        let _ = super::config::save_ai_settings(&s);
        cx.notify();
    }

    pub fn set_skill(&mut self, skill_id: &str, cx: &mut gpui::Context<Self>) {
        self.skill_id = skill_id.to_string();
        cx.notify();
    }

    pub fn add_attachment(&mut self, att: Attachment, cx: &mut gpui::Context<Self>) {
        self.attachments.push(att);
        cx.notify();
    }

    pub fn remove_attachment(&mut self, ix: usize, cx: &mut gpui::Context<Self>) {
        if ix < self.attachments.len() {
            self.attachments.remove(ix);
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut gpui::Context<Self>) {
        self.history.clear();
        self.items.clear();
        self.attachments.clear();
        self.usage_total = Usage::default();
        self.turn_lines.clear();
        self.work_plan.clear();
        self.last_error = None;
        cx.notify();
    }

    /// Ask the running turn to stop (rejects any pending plan).
    pub fn stop(&mut self, cx: &mut gpui::Context<Self>) {
        self.stop.store(true, Ordering::SeqCst);
        for item in self.items.iter_mut().rev() {
            if let TranscriptItem::Plan { decision_tx, decided, .. } = item
                && decided.is_none()
            {
                if let Some(tx) = decision_tx.take() {
                    let _ = tx.try_send(PlanDecision::Reject);
                }
            }
        }
        cx.notify();
    }

    /// Register the user's plan decision from the panel buttons.
    pub fn decide_plan(&mut self, item_ix: usize, decision: PlanDecision, cx: &mut gpui::Context<Self>) {
        let Some(TranscriptItem::Plan {
            decision_tx, decided, ..
        }) = self.items.get_mut(item_ix)
        else {
            return;
        };
        if decided.is_some() {
            return;
        }
        *decided = Some(decision.clone());
        if let Some(tx) = decision_tx.take() {
            let _ = tx.try_send(decision);
        }
        cx.notify();
    }

    /// Send a user message and start a turn.
    pub fn send(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        if self.running {
            return;
        }
        let text = text.trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let settings = super::config::load_ai_settings();
        let Some(provider) = settings.active_provider().cloned() else {
            self.items.push(TranscriptItem::Error(
                "尚未配置可用的 AI 供应商，请在 设置 → AI 智能体 中添加。".into(),
            ));
            cx.notify();
            return;
        };
        if !self.attachments.iter().all(|a| match a {
            Attachment::Image { .. } => provider.supports_vision,
            _ => true,
        }) {
            self.items
                .push(TranscriptItem::Error("当前供应商未开启视觉输入，无法发送图片附件。".into()));
            cx.notify();
            return;
        }

        // Build the user message (text + attachment parts / embedded file text).
        let attachment_names: Vec<String> = self
            .attachments
            .iter()
            .map(|a| a.name().to_string())
            .collect();
        let images: Vec<String> = self
            .attachments
            .iter()
            .filter_map(|a| match a {
                Attachment::Image { data_url, .. } => Some(data_url.clone()),
                _ => None,
            })
            .collect();
        let mut text_buf = text.clone();
        for att in &self.attachments {
            if let Attachment::TextFile { name, content } = att {
                text_buf.push_str(&format!("\n\n--- 附件 {name} ---\n{content}"));
            }
        }
        // octos pattern: the status-word pool follows the user's language.
        self.status_zh = super::style::prompt_has_cjk(&text_buf);
        // Per-turn starting offset from the wall clock so back-to-back turns
        // don't open on the same word.
        self.status_idx = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as usize)
            .unwrap_or(0);
        let msg = if images.is_empty() {
            ChatMessage::user_text(text_buf)
        } else {
            let mut parts = vec![ContentPart::Text { text: text_buf }];
            parts.extend(
                images
                    .into_iter()
                    .map(|url| ContentPart::ImageUrl { image_url: ImageUrl { url } }),
            );
            ChatMessage::user_parts(parts)
        };
        self.history.push(msg);
        self.items.push(TranscriptItem::User {
            text,
            attachment_names,
        });
        self.attachments.clear();

        self.start_turn(provider, settings, cx);
    }

    fn start_turn(
        &mut self,
        provider: AiProviderConfig,
        settings: AiSettings,
        cx: &mut gpui::Context<Self>,
    ) {
        let Ok(client) = AiClient::for_provider(&provider, cx) else {
            self.items
                .push(TranscriptItem::Error("构造 AI 客户端失败（检查代理设置）。".into()));
            cx.notify();
            return;
        };
        self.running = true;
        self.phase = Phase::Streaming;
        self.iteration = 1;
        self.last_error = None;
        // In-flight bubble appears immediately (thinking indicator + later
        // streamed text) instead of waiting for the first network delta.
        self.items.push(TranscriptItem::Assistant {
            text: String::new(),
            streaming: true,
            usage: None,
        });
        self.stop = Arc::new(AtomicBool::new(false));
        self.cost_currency = provider.currency.clone();
        self.input_price = provider.input_price;
        self.output_price = provider.output_price;
        self.price_unit_div = match provider.price_unit {
            super::config::PriceUnit::Per1K => 1_000.0,
            super::config::PriceUnit::Per1M => 1_000_000.0,
        };
        cx.notify();

        let state = self.state.clone();
        let stop = self.stop.clone();
        let mode = self.mode;
        let skill_id = self.skill_id.clone();
        let max_iterations = settings.clamped_iterations();
        // Tool calling is assumed for every provider (user decision:
        // no per-provider toggle anymore).

        // Status-word rotator (octos persona-service pattern): one word
        // every 8s for the WHOLE in-flight turn — including tool waits and
        // plan-approval pauses — not only while text is streaming.
        cx.spawn(async move |this, cx| {
            loop {
                smol::Timer::after(super::style::STATUS_WORD_INTERVAL).await;
                let still_running = this
                    .update(cx, |s, cx| {
                        if s.running {
                            s.status_idx = s.status_idx.wrapping_add(1);
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !still_running {
                    return;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            run_turn(
                this, cx, state, client, mode, skill_id, max_iterations, stop,
            )
            .await;
        })
        .detach();
    }

    fn cost_of(&self, prompt: u64, completion: u64) -> Option<f64> {
        let input = self.input_price?;
        let output = self.output_price?;
        Some(prompt as f64 * input / self.price_unit_div + completion as f64 * output / self.price_unit_div)
    }

    fn usage_line(&self, usage: &Usage, estimated: bool) -> UsageLine {
        UsageLine {
            prompt: usage.prompt_tokens,
            completion: usage.completion_tokens,
            estimated,
            cost: self.cost_of(usage.prompt_tokens, usage.completion_tokens),
            currency: self.cost_currency.clone(),
        }
    }

    /// Repair malformed tool-call arguments already stored in history
    /// (returns how many were fixed). Used when a vendor 400s on
    /// "arguments must be valid JSON" — after repair the turn retries once.
    pub fn sanitize_history_tool_args(&mut self) -> usize {
        sanitize_tool_args(&mut self.history)
    }

    /// Fold old tool results when over budget, writing back to history.
    fn fold_history(&mut self) {
        let budget = 120_000usize;
        let mut history = std::mem::take(&mut self.history);
        context::enforce_budget(&mut history, budget);
        self.history = history;
    }
}

/// The full turn loop (see plan §2.2). Runs inside `cx.spawn`.
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    this: gpui::WeakEntity<AgentSession>,
    cx: &mut gpui::AsyncApp,
    state: Entity<AppState>,
    client: AiClient,
    mode: AgentMode,
    skill_id: String,
    max_iterations: usize,
    stop: Arc<AtomicBool>,
) {
    let mut turn_usage = Usage::default();
    let mut stopped = false;
    // Repair budget for malformed tool-argument JSON 400s ("...arguments
    // must be valid JSON"): a vendor can reject these repeatedly within one
    // turn, so allow a few sanitize-and-retry cycles before giving up.
    let mut arg_repairs: u32 = 0;
    const MAX_ARG_REPAIRS: u32 = 3;
    // Circuit breaker: consecutive identical tool failures (e.g. the model
    // resending a giant plan that gets mangled every time) burn iterations
    // and tokens; after the 4th identical failure the turn aborts.
    let mut fail_streak: usize = 0;
    let mut last_fail: Option<(String, String)> = None;
    // Read-only stall guard: consecutive iterations that only ran read
    // tools without proposing/applying anything — nudge the model to act.
    let mut read_only_streak: usize = 0;

    'turn: loop {
        if stop.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }

        // ---- Build request (system prompt + history w/ auto-compaction) ----
        let req = {
            // Auto-compaction: when over budget, summarize the older part via
            // the model itself and keep only the summary + recent tail.
            {
                let snapshot = this.update(cx, |s, _| s.history.clone()).unwrap_or_default();
                if let Some((boundary, _)) = split_for_compaction(&snapshot, COMPACT_KEEP_TAIL) {
                    // Condenser-style layout: the original task (first user
                    // message) survives verbatim, everything after it up to
                    // the tail is summarized (merging any prior summary).
                    let keep_upto = usize::from(
                        snapshot.first().is_some_and(|m| m.role == Role::User),
                    );
                    let (raw_head, tail) = snapshot.split_at(boundary);
                    let middle = raw_head.get(keep_upto..).unwrap_or(&[]);
                    let prompt = compaction_prompt(middle);
                    log::info!(
                        "[AI 上下文压缩] 历史 {} 条 / {} 字符 → 压缩 {} 条（含旧摘要合并），保留尾部 {} 条",
                        snapshot.len(),
                        snapshot.iter().map(|m| m.text().len()).sum::<usize>(),
                        middle.len(),
                        tail.len(),
                    );
                    let summary_req = ChatRequest {
                        model: client.model().to_string(),
                        messages: vec![
                            ChatMessage::system("你是对话历史压缩器。"),
                            ChatMessage::user_text(prompt),
                        ],
                        max_tokens: Some(1500),
                        ..Default::default()
                    };
                    match client.chat(&summary_req).await {
                        Ok(resp) => {
                            let summary = resp
                                .choices
                                .first()
                                .map(|c| c.message.text())
                                .unwrap_or_default();
                            if !summary.trim().is_empty() {
                                // Deterministic name↔id table from the very
                                // tool results being folded away — ids in it
                                // are exact, unlike ids the model might
                                // recall from the summary prose.
                                let id_table = extract_id_table(middle);
                                let table_block = if id_table.is_empty() {
                                    String::new()
                                } else {
                                    let lines: Vec<String> = id_table
                                        .iter()
                                        .take(80)
                                        .map(|(n, i)| format!("「{n}」= {i}"))
                                        .collect();
                                    format!(
                                        "\n\n〔系统自动提取的节点 id 对照表（代码从工具结果中提取，可直接引用）〕\n{}",
                                        lines.join("\n")
                                    )
                                };
                                let compacted = {
                                    let mut v: Vec<ChatMessage> = Vec::new();
                                    if let Some(first) = raw_head.first().and_then(text_only_copy) {
                                        v.push(first);
                                    }
                                    v.push(ChatMessage::user_text(format!(
                                        "{COMPACT_SUMMARY_MARKER}\n{summary}{table_block}"
                                    )));
                                    v.extend(tail.to_vec());
                                    v
                                };
                                let _ = this.update(cx, |s, cx| {
                                    s.history = compacted;
                                    s.items.push(TranscriptItem::Note(
                                        "上下文过长，已自动压缩较早的对话。".into(),
                                    ));
                                    cx.notify();
                                });
                            }
                        }
                        Err(e) => {
                            // Compaction is best-effort; the hard folder in
                            // fold_history remains as the safety net.
                            log::warn!("上下文压缩失败（回退为省略折叠）：{e}");
                        }
                    }
                }
            }
            let history = match this.update(cx, |s, cx| {
                s.fold_history();
                // Last line of defense: whatever path polluted the stored
                // history (legacy sessions, interrupted exchanges), the
                // outgoing request must never carry invalid tool arguments
                // — strict gateways 400 on them. Idempotent + cheap.
                let fixed = sanitize_tool_args(&mut s.history);
                if fixed > 0 {
                    log::warn!("发送前净化了 {fixed} 处非法工具参数。");
                }
                cx.notify();
                s.history.clone()
            }) {
                Ok(h) => h,
                Err(_) => return,
            };
            let (overview, selection) = state.update(cx, |s, _| {
                    let overview = s
                        .active_project()
                        .map(context::workspace_overview);
                    let selection = match (
                        s.selected_request.as_deref(),
                        s.selected_folder.as_deref(),
                    ) {
                        (Some(id), _) => s
                            .active_project()
                            .and_then(|p| p.find_request(id))
                            .map(|(_, r)| format!("接口「{}」({})", r.name, r.protocol.as_str())),
                        (None, Some(fid)) => s
                            .active_project()
                            .and_then(|p| p.find_folder(fid))
                            .map(|(_, f)| format!("目录「{}」", f.name)),
                        _ => None,
                    };
                (overview, selection)
            });
            let work_plan = this.update(cx, |s, _| s.work_plan.clone()).unwrap_or_default();
            let sys = build_system_prompt(
                mode,
                &skill_id,
                overview.as_ref(),
                selection.as_deref(),
                &work_plan,
            );
            // Full system prompt once per turn (first iteration); later
            // iterations rebuild nearly identical text, keep those at debug.
            let iter_now = this
                .update(cx, |s, _| s.iteration)
                .unwrap_or(1);
            if iter_now <= 1 {
                log::info!("[AI 系统提示词 · 第 {iter_now} 轮]
{sys}");
            } else {
                log::debug!("[AI 系统提示词 · 第 {iter_now} 轮]
{sys}");
            }
            let mut messages = vec![ChatMessage::system(sys)];
            messages.extend(history);
            ChatRequest {
                model: client.model().to_string(),
                messages,
                tools: Some(tools::tool_defs(mode)),
                stream: None,
                stream_options: None,
                ..Default::default()
            }
        };

        // ---- Stream with live transcript updates via a channel ----
        // Both deltas and 429-retry progress ride one Send-safe channel; the
        // drain loop applies them to the session on the main thread.
        enum Wire {
            Ev(StreamEvent),
            Retry(RetryStatus),
        }
        let (tx, rx) = smol::channel::unbounded::<Wire>();
        let tx_ev = tx.clone();
        let stream_fut = client.chat_stream(
            &req,
            stop.clone(),
            move |ev| {
                let _ = tx_ev.try_send(Wire::Ev(ev));
            },
            move |attempt, max, wait_secs| {
                let _ = tx.try_send(Wire::Retry(RetryStatus { attempt, max, wait_secs }));
            },
        );
        let this_c = this.clone();
        let drain_fut = async {
            while let Ok(wire) = rx.recv().await {
                let _ = this_c.update(cx, |s, cx| {
                    match wire {
                        Wire::Ev(ev) => apply_stream_event(s, &ev),
                        Wire::Retry(r) => s.retry = Some(r),
                    }
                    cx.notify();
                });
            }
        };
        let (outcome, _) = futures::future::join(stream_fut, drain_fut).await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                // Replaying assistant tool_calls whose sharded arguments
                // assembled into invalid JSON makes strict gateways 400
                // ("...arguments must be valid JSON"). Repair the stored
                // history and retry the same iteration once; other errors
                // fail the turn as before.
                let low = e.to_string().to_lowercase();
                let repairable = arg_repairs < MAX_ARG_REPAIRS
                    && (low.contains("must be valid json")
                        || (low.contains("tool call") && low.contains("arguments")));
                if !repairable {
                    let _ = this.update(cx, |s, cx| {
                        s.running = false;
                        s.retry = None;
                        s.last_error = Some(format!("{e}"));
                        s.items.push(TranscriptItem::Error(format!("{e}")));
                        cx.notify();
                    });
                    return;
                }
                arg_repairs += 1;
                log::warn!("上游拒绝工具参数 JSON，修复历史后重试本轮：{e}");
                let _ = this.update(cx, |s, cx| {
                    if matches!(
                        s.items.last(),
                        Some(TranscriptItem::Assistant { streaming: true, .. })
                    ) {
                        s.items.pop();
                    }
                    let fixed = s.sanitize_history_tool_args();
                    if fixed > 0 {
                        s.items.push(TranscriptItem::Note(format!(
                            "模型生成的 {fixed} 处工具参数不是合法 JSON，已自动修复并重试本轮。"
                        )));
                    } else {
                        s.items.push(TranscriptItem::Note(
                            "供应商拒绝了工具参数格式，自动重试本轮。".into(),
                        ));
                    }
                    cx.notify();
                });
                continue 'turn;
            }
        };
        let _ = this.update(cx, |s, cx| {
            s.retry = None;
            cx.notify();
        });
        if outcome.stopped {
            stopped = true;
        }

        // Usage (real or estimated).
        let (usage, estimated) = match outcome.usage {
            Some(u) => (u, false),
            None => (estimate_usage(&req, &outcome.message), true),
        };
        turn_usage.add(usage);
        {
            let iter_for_log = this.update(cx, |s, _| s.iteration).unwrap_or(0);
            let est_mark = if estimated { "≈" } else { "" };
            log::info!(
                "[AI 第 {iter_for_log} 轮流式输出完成] finish={:?} 输出 {} 字符 · 工具调用 {} 个 · tokens {est}{}+{est}{}",
                outcome.finish_reason,
                outcome.message.text().chars().count(),
                outcome.message.tool_calls.as_ref().map_or(0, Vec::len),
                usage.prompt_tokens,
                usage.completion_tokens,
                est = est_mark,
            );
        }

        // ---- Commit assistant message + usage line ----
        let tool_calls = outcome.message.tool_calls.clone();
        let finish = outcome.finish_reason.clone();
        let bad_args: Vec<ToolArgDiag> = outcome.bad_args.clone();
        let _ = this.update(cx, |s, cx| {
            s.history.push(outcome.message.clone());
            let line = s.usage_line(&usage, estimated);
            s.usage_total.add(usage);
            mark_assistant_done(&mut s.items, &line);
            cx.notify();
        });

        // ---- Tool calls? ----
        let Some(calls) = tool_calls.filter(|c| !c.is_empty()) else {
            break 'turn;
        };
        if finish.as_deref() != Some("tool_calls") && stop.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }

        for call in calls.iter() {
            if stop.load(Ordering::SeqCst) {
                stopped = true;
                break 'turn;
            }
            let args: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| json!({}));
            let mut result: String = if let Some(diag) =
                bad_args.iter().find(|d| d.call_id == call.id)
            {
                // Arguments were mangled in transit (sanitized to `{}`) —
                // answer with a diagnosis instead of executing an empty
                // call the model would only retry blindly.
                bad_args_error(diag, finish.as_deref() == Some("length"))
            } else {
                clamp_tool_result(
                    exec_tool_call(&this, cx, &state, mode, &call, &args, &stop).await,
                )
            };
            // Track consecutive identical failures for the circuit breaker.
            if result.contains("\"error\"") {
                let sig = (call.function.name.clone(), result.clone());
                if last_fail.as_ref() == Some(&sig) {
                    fail_streak += 1;
                } else {
                    fail_streak = 1;
                    last_fail = Some(sig);
                }
            } else {
                fail_streak = 0;
                last_fail = None;
            }
            if let Some(tip) = repeated_failure_guidance(fail_streak) {
                result.push_str(tip);
            }
            let abort = fail_streak >= 4;
            let _ = this.update(cx, |s, cx| {
                s.history.push(ChatMessage::tool_result(call, result));
                if abort {
                    s.items.push(TranscriptItem::Note(format!(
                        "工具 {} 连续 {} 次相同失败，已终止本轮以避免空转。建议精简指令或让它分批执行。",
                        call.function.name, fail_streak
                    )));
                    cx.notify();
                }
            });
            if abort {
                break 'turn;
            }
        }
        // Read-only stall guard: plans never appear if the model keeps
        // listing/reading and narrating instead of proposing changes.
        {
            let wrote = calls.iter().any(|c| {
                !tools::is_read_tool(&c.function.name)
                    && c.function.name != "update_plan"
                    && c.function.name != "use_skill"
            });
            if wrote {
                read_only_streak = 0;
            } else {
                read_only_streak += 1;
            }
            if read_only_streak >= 3 {
                let nudge = "\n【系统提示】你已经连续多轮只读取信息而没有提交任何变更计划。信息足够时请立即调用 propose_changes 提交第一批计划卡（计划模式下卡片本身就是确认环节）；若确实缺少关键信息，请直接向用户提出具体问题，不要继续读取。";
                let _ = this.update(cx, |s, cx| {
                    if let Some(last) = s.history.last_mut()
                        && last.role == Role::Tool
                        && let Some(MessageContent::Text(t)) = last.content.as_mut()
                    {
                        t.push_str(nudge);
                    }
                    s.items.push(TranscriptItem::Note(
                        "已提醒模型停止空转、提交计划卡。".into(),
                    ));
                    cx.notify();
                });
                read_only_streak = 0;
            }
        }

        // Loop guard.
        let Ok(next_iter) = this.update(cx, |s, cx| {
            s.iteration += 1;
            cx.notify();
            s.iteration
        }) else {
            return;
        };
        if next_iter > max_iterations {
            let _ = this.update(cx, |s, cx| {
                s.items.push(TranscriptItem::Note(format!(
                    "已达最大循环次数 {max_iterations}，可继续发送消息让它接着做。"
                )));
                cx.notify();
            });
            break;
        }
    }

    let _ = this.update(cx, |s, cx| {
        let padded = pad_missing_tool_results(&mut s.history);
        if padded > 0 {
            log::warn!("已补齐 {padded} 个因中断缺失的工具结果占位。");
        }
        s.running = false;
        s.retry = None;
        let line = s.usage_line(&turn_usage, false);
        s.turn_lines.push(line);
        if stopped {
            s.items.push(TranscriptItem::Note("已停止。".into()));
        }
        cx.notify();
    });
}

/// Execute one tool call, updating the transcript; returns the tool-result
/// content string for the model.
async fn exec_tool_call(
    this: &gpui::WeakEntity<AgentSession>,
    cx: &mut gpui::AsyncApp,
    state: &Entity<AppState>,
    mode: AgentMode,
    call: &ToolCall,
    args: &Value,
    stop: &Arc<AtomicBool>,
) -> String {
    let name = call.function.name.clone();

    // Progressive skill loading: the system prompt carries only a compact
    // index; this tool swaps the active skill and returns its full body.
    if name == "use_skill" {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let known = skills::builtin_skills().iter().find(|k| k.id == id);
        return match known {
            None => {
                let msg = format!("未知技能 id：{id}（可选：{}）", skills::skill_index());
                let _ = this.update(cx, |s, cx| {
                    s.items.push(TranscriptItem::Tool {
                        name: name.clone(),
                        summary: msg.clone(),
                        ok: false,
                    });
                    cx.notify();
                });
                format!("{{\"error\":\"{msg}\"}}")
            }
            Some(skill) => {
                let full = skill.prompt.to_string();
                let label = skill.name.to_string();
                let _ = this.update(cx, |s, cx| {
                    s.set_skill(&id, cx);
                    s.items.push(TranscriptItem::Tool {
                        name: name.clone(),
                        summary: format!("已切换技能：{label}"),
                        ok: true,
                    });
                    cx.notify();
                });
                // The full skill prompt rides back as the tool result AND
                // stays injected in every subsequent system prompt.
                format!("已切换到技能「{label}」。以下是该技能的完整指引，必须遵循：\n\n{full}")
            }
        };
    }

    // Codex-style working-plan update: maintained in session state and
    // re-injected into every system prompt (compaction-proof details).
    if name == "update_plan" {
        let outcome: Result<Vec<PlanStep>, String> = (|| {
            #[derive(serde::Deserialize)]
            struct RawStep {
                step: String,
                #[serde(default)]
                detail: String,
                status: PlanStatus,
            }
            #[derive(serde::Deserialize)]
            struct RawPlan {
                steps: Vec<RawStep>,
                #[serde(default)]
                note: Option<String>,
            }
            let raw: RawPlan = serde_json::from_value(args.clone())
                .map_err(|e| format!("update_plan 参数解析失败: {e}"))?;
            if raw.steps.is_empty() {
                return Err("steps 不能为空（要清空计划请提交一个只含收尾事项的新计划）".into());
            }
            if raw.steps.len() > 12 {
                return Err("计划步骤过多（≤12），请合并或分阶段".into());
            }
            Ok(normalize_plan_steps(
                raw.steps
                    .into_iter()
                    .map(|r| PlanStep {
                        step: r.step,
                        detail: r.detail,
                        status: r.status,
                    })
                    .collect(),
            ))
        })();
        return match outcome {
            Err(e) => {
                let _ = this.update(cx, |s, cx| {
                    s.items.push(TranscriptItem::Tool {
                        name: name.clone(),
                        summary: e.clone(),
                        ok: false,
                    });
                    cx.notify();
                });
                format!("{{\"error\":\"{e}\"}}")
            }
            Ok(steps) => {
                let summary = format!(
                    "计划已更新：{} 步（已完成 {} · 进行中 {} · 待办 {}）",
                    steps.len(),
                    steps.iter().filter(|s| s.status == PlanStatus::Completed).count(),
                    steps.iter().filter(|s| s.status == PlanStatus::InProgress).count(),
                    steps.iter().filter(|s| s.status == PlanStatus::Pending).count(),
                );
                let _ = this.update(cx, |s, cx| {
                    s.work_plan = steps.clone();
                    // Update the existing plan card in place instead of
                    // stacking a new one per update.
                    let card_ix = s
                        .items
                        .iter()
                        .rposition(|it| matches!(it, TranscriptItem::WorkPlan { .. }));
                    match card_ix {
                        Some(ix) => {
                            if let Some(item) = s.items.get_mut(ix) {
                                *item = TranscriptItem::WorkPlan { steps: steps.clone() };
                            }
                        }
                        None => s.items.push(TranscriptItem::WorkPlan { steps: steps.clone() }),
                    }
                    s.items.push(TranscriptItem::Tool {
                        name: name.clone(),
                        summary: summary.clone(),
                        ok: true,
                    });
                    cx.notify();
                });
                serde_json::json!({ "ok": true, "summary": summary }).to_string()
            }
        };
    }

    // Read tools: answer straight from the tree.
    if tools::is_read_tool(&name) {
        let result = state.update(cx, |s, _| {
            s.active_project()
                .ok_or_else(|| "没有打开的项目".to_string())
                .and_then(|p| tools::dispatch_read(&name, args, p))
        });
        let _ = this.update(cx, |s, cx| {
            match &result {
                Ok(v) => s.items.push(TranscriptItem::Tool {
                    name: name.clone(),
                    summary: summarize_result(v),
                    ok: true,
                }),
                Err(e) => s.items.push(TranscriptItem::Tool {
                    name: name.clone(),
                    summary: e.clone(),
                    ok: false,
                }),
            }
            cx.notify();
        });
        return match result {
            Ok(v) => v.to_string(),
            Err(e) => format!("{{\"error\":\"{e}\"}}"),
        };
    }

    // Plan mode: every write goes through propose_changes → plan card.
    if mode == AgentMode::Plan && name == "propose_changes" {
        let proposed = state.update(cx, |s, _| {
            s.active_project()
                .ok_or_else(|| "没有打开的项目".to_string())
                .and_then(|p| tools::propose_changes(args, p))
        });
        return match proposed {
            Err(e) => {
                let _ = this.update(cx, |s, cx| {
                    s.items.push(TranscriptItem::Tool {
                        name: name.clone(),
                        summary: e.clone(),
                        ok: false,
                    });
                    cx.notify();
                });
                format!("{{\"error\":\"{e}\"}}")
            }
            Ok((plan, validation)) => {
                let (tx, rx) = smol::channel::bounded::<PlanDecision>(1);
                let item_ix = this
                    .update(cx, |s, cx| {
                        s.items.push(TranscriptItem::Plan {
                            ops: plan.clone(),
                            validation: validation.clone(),
                            applied: None,
                            decision_tx: Some(tx),
                            decided: None,
                        });
                        s.phase = Phase::AwaitingPlan;
                        cx.notify();
                        s.items.len().saturating_sub(1)
                    })
                    .unwrap_or(usize::MAX);
                let decision = match rx.recv().await {
                    Ok(d) => d,
                    Err(_) => PlanDecision::Reject,
                };
                let refs_missing = validation
                    .iter()
                    .filter(|r| !r.ok)
                    .any(|r| r.error.as_deref().is_some_and(|e| e.contains("不存在")));
                let mut result = match decision {
                    PlanDecision::Apply => {
                        // Backup → apply → persist → validate, with automatic
                        // rollback on a corrupt file.
                        let applied = apply_guarded(state, cx, |s, _| {
                            let Some(p) = s.active_project_mut() else {
                                return Vec::new();
                            };
                            super::ops::apply_change_ops(p, &plan)
                        });
                        match applied {
                            Ok(results) => {
                                let results = this
                                    .update(cx, |s, cx| {
                                        if let Some(item) = s.items.get_mut(item_ix)
                                            && let TranscriptItem::Plan { applied, .. } = item
                                        {
                                            *applied = Some(results.clone());
                                        }
                                        s.phase = Phase::Streaming;
                                        cx.notify();
                                        results.clone()
                                    })
                                    .unwrap_or_default();
                                serde_json::to_string(&results).unwrap_or_else(|_| "{}".into())
                            }
                            Err(e) => {
                                let _ = this.update(cx, |s, cx| {
                                    s.phase = Phase::Streaming;
                                    s.items.push(TranscriptItem::Error(e.clone()));
                                    cx.notify();
                                });
                                format!("{{\"error\":\"{e}\"}}")
                            }
                        }
                    }
                    PlanDecision::Revise { note } => {
                        let _ = this.update(cx, |s, cx| {
                            s.phase = Phase::Streaming;
                            cx.notify();
                        });
                        if note.trim().is_empty() {
                            "用户要求调整该计划但未说明具体意见；请先向用户询问需要调整什么，等待回复后再修改。".to_string()
                        } else {
                            format!("用户要求按以下意见调整计划：{note}。请修改后重新提交变更计划。")
                        }
                    }
                    PlanDecision::Reject => {
                        let _ = this.update(cx, |s, cx| {
                            s.phase = Phase::Streaming;
                            cx.notify();
                        });
                        "用户已拒绝该计划。请询问用户希望如何调整。".to_string()
                    }
                };
                if refs_missing {
                    result.push_str(
                        "（校验提示：计划中有引用的节点在工作区不存在。id 只能来自本次对话中工具返回的真实结果——不要凭记忆复述；也可以用全局唯一的节点名称代替 id。请先重新调用 list_workspace(recursive=true) 获取最新 id，再分批（每批 ≤ 8 个 op）重新提交。）",
                    );
                }
                result
            }
        };
    }

    // Full-access mode: direct write tool applied immediately.
    if mode == AgentMode::FullAccess {
        // Flatten: outer Err = validation rollback, inner = tool outcome.
        let result = match apply_guarded(state, cx, |s, _| {
            let Some(p) = s.active_project_mut() else {
                return Err("没有打开的项目".to_string());
            };
            tools::dispatch_write(&name, args, p)
        }) {
            Ok(inner) => inner,
            Err(rollback) => Err(rollback),
        };
        let _ = this.update(cx, |s, cx| {
            match &result {
                Ok(v) => s.items.push(TranscriptItem::Tool {
                    name: name.clone(),
                    summary: summarize_results_json(v),
                    ok: true,
                }),
                Err(e) => s.items.push(TranscriptItem::Tool {
                    name: name.clone(),
                    summary: e.clone(),
                    ok: false,
                }),
            }
            cx.notify();
        });
        return match result {
            Ok(v) => v.to_string(),
            Err(e) => format!("{{\"error\":\"{e}\"}}"),
        };
    }

    let msg = format!("未知工具 {name}（当前为计划模式，写操作请用 propose_changes）");
    let _ = this.update(cx, |s, cx| {
        s.items.push(TranscriptItem::Tool {
            name: name.clone(),
            summary: msg.clone(),
            ok: false,
        });
        cx.notify();
    });
    format!("{{\"error\":\"{msg}\"}}")
}

fn summarize_result(v: &Value) -> String {
    if let Some(listing) = v.get("listing").and_then(Value::as_array) {
        let mut folders = 0usize;
        let mut requests = 0usize;
        let mut sample_names: Vec<&str> = Vec::new();
        for node in listing {
            match node.get("kind").and_then(Value::as_str) {
                Some("folder") => {
                    folders += 1;
                    if sample_names.len() < 3 {
                        if let Some(name) = node.get("name").and_then(Value::as_str) {
                            sample_names.push(name);
                        }
                    }
                }
                Some("request") => {
                    requests += 1;
                    if sample_names.len() < 3 {
                        if let Some(name) = node.get("name").and_then(Value::as_str) {
                            sample_names.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
        let total = folders + requests;
        if total == 0 {
            return "空目录".to_string();
        }
        let parts: Vec<String> = {
            let mut p = Vec::new();
            if folders > 0 {
                p.push(format!("{folders} 个目录"));
            }
            if requests > 0 {
                p.push(format!("{requests} 个接口"));
            }
            p
        };
        let tail = if sample_names.is_empty() {
            String::new()
        } else {
            format! (" · {}", sample_names.join("、"))
        };
        format!("{} ({}){}", parts.join("，"), total, tail)
    } else if let Some(total) = v.get("total").and_then(Value::as_u64) {
        let mut names: Vec<String> = Vec::new();
        if let Some(results) = v.get("results").and_then(Value::as_array) {
            for r in results.iter().take(2) {
                if let Some(name) = r.get("name").and_then(Value::as_str) {
                    names.push(name.to_string());
                }
            }
        }
        if names.is_empty() {
            format!("找到 {total} 条结果")
        } else {
            format!("找到 {total} 条结果 · {}", names.join("、"))
        }
    } else if let Some(err) = v.get("error").and_then(Value::as_str) {
        err.to_string()
    } else {
        let s = v.to_string();
        let mut head: String = s.chars().take(80).collect();
        if head.chars().count() < s.chars().count() {
            head.push('…');
        }
        head
    }
}

fn summarize_results_json(v: &Value) -> String {
    let first_ok = v
        .as_array()
        .and_then(|a| a.first())
        .map(|r| {
            r.get("summary")
                .and_then(Value::as_str)
                .unwrap_or("已执行")
                .to_string()
        })
        .unwrap_or_else(|| "已执行".to_string());
    first_ok
}

/// Append streaming deltas to the trailing assistant bubble (create if the
/// last one isn't a streaming assistant).
/// Repair malformed tool-call arguments already stored in a message history
/// (returns how many were fixed). Used when a strict gateway 400s on
/// "arguments must be valid JSON" — after repair the turn retries once.
fn sanitize_tool_args(history: &mut [ChatMessage]) -> usize {
    let mut fixed = 0usize;
    for msg in history.iter_mut() {
        let Some(calls) = msg.tool_calls.as_mut() else { continue };
        for call in calls.iter_mut() {
            if serde_json::from_str::<Value>(&call.function.arguments).is_err() {
                call.function.arguments = "{}".to_string();
                fixed += 1;
            }
        }
    }
    fixed
}

fn apply_stream_event(s: &mut AgentSession, ev: &StreamEvent) {
    match ev {
        StreamEvent::Text(t) => {
            match s.items.last_mut() {
                Some(TranscriptItem::Assistant { text, streaming, .. }) if *streaming => {
                    text.push_str(t);
                }
                _ => {
                    s.items.push(TranscriptItem::Assistant {
                        text: t.clone(),
                        streaming: true,
                        usage: None,
                    });
                }
            }
        }
        StreamEvent::ToolFragment { .. } => {
            // Show a placeholder bubble while tool args stream in.
            if !matches!(s.items.last(), Some(TranscriptItem::Assistant { streaming: true, .. })) {
                s.items.push(TranscriptItem::Assistant {
                    text: String::new(),
                    streaming: true,
                    usage: None,
                });
            }
        }
        StreamEvent::Finish(_) | StreamEvent::Usage(_) => {}
    }
}

fn mark_assistant_done(items: &mut Vec<TranscriptItem>, line: &UsageLine) {
    for item in items.iter_mut().rev() {
        if let TranscriptItem::Assistant { streaming, usage, .. } = item {
            if *streaming {
                *streaming = false;
                *usage = Some(line.clone());
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChatMessage, Role, COMPACT_KEEP_TAIL, COMPACT_SUMMARY_MARKER, bad_args_error,
        clamp_tool_result, compaction_prompt, pad_missing_tool_results, repeated_failure_guidance,
        extract_id_table, sanitize_tool_args, split_for_compaction, text_only_copy,
    };
    use super::super::chat::ToolArgDiag;
    use super::super::chat::{ContentPart, FunctionCall, ImageUrl, MessageContent, ToolCall};

    #[test]
    fn sanitize_tool_args_repairs_only_invalid_json() {
        let mut history = vec![
            ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![
                    ToolCall {
                        id: "call_ok".into(),
                        typ: "function".into(),
                        function: FunctionCall {
                            name: "list_workspace".into(),
                            arguments: r#"{"parent":"root"}"#.into(),
                        },
                    },
                    ToolCall {
                        id: "call_bad".into(),
                        typ: "function".into(),
                        function: FunctionCall {
                            name: "propose_changes".into(),
                            arguments: r#"{"ops":[{oops"#.into(),
                        },
                    },
                ]),
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: Role::User,
                content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        assert_eq!(sanitize_tool_args(&mut history), 1);
        let Some(calls) = history.first_mut().and_then(|m| m.tool_calls.as_mut()) else {
            panic!("no calls");
        };
        assert_eq!(calls[0].function.arguments, r#"{"parent":"root"}"#);
        assert_eq!(calls[1].function.arguments, "{}");
        // Second pass is a no-op.
        assert_eq!(sanitize_tool_args(&mut history), 0);
    }


    #[test]
    fn compaction_split_guards() {
        let mk = |t: &str| ChatMessage::user_text(t);
        // Short history → no compaction.
        let short: Vec<ChatMessage> = (0..6).map(|_| mk("hi")).collect();
        assert!(split_for_compaction(&short, COMPACT_KEEP_TAIL).is_none());

        // Long enough history but under char budget → none.
        let mid: Vec<ChatMessage> = (0..30).map(|_| mk("短")).collect();
        assert!(split_for_compaction(&mid, COMPACT_KEEP_TAIL).is_none());

        // Over budget → earliest user-message cut whose tail fits the
        // 28k-char budget: (40 - c) * 2000 <= 28000 → c >= 26.
        let long: Vec<ChatMessage> = (0..40).map(|_| mk(&"a".repeat(2000))).collect();
        let (boundary, head_chars) = split_for_compaction(&long, 8).unwrap();
        assert_eq!(boundary, 26);
        assert_eq!(head_chars, 26 * 2000);
        let tail_chars: usize = long.get(boundary..).unwrap().iter().map(|m| m.text().len()).sum();
        assert!(tail_chars <= 28_000);
    }

    #[test]
    fn compaction_split_prefers_max_summarization_when_tail_huge() {
        let mk = |t: &str| ChatMessage::user_text(t);
        // Every message 10k chars: no candidate tail fits 28k within the
        // 8-message minimum, so the cut maximizes the summarized head.
        let long: Vec<ChatMessage> = (0..40).map(|_| mk(&"a".repeat(10_000))).collect();
        let (boundary, _) = split_for_compaction(&long, 8).unwrap();
        assert_eq!(boundary, 32);
    }

    #[test]
    fn compaction_split_cuts_only_at_user_boundaries() {
        let mk = |t: &str| ChatMessage::user_text(t);
        // A tool exchange at 31/32 must land entirely in the kept tail: only
        // user-message starts are valid cuts.
        let history: Vec<ChatMessage> = (0..40)
            .map(|i| match i {
                31 => ChatMessage {
                    role: Role::Assistant,
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "c1".into(),
                        typ: "function".into(),
                        function: FunctionCall {
                            name: "list_workspace".into(),
                            arguments: "{}".into(),
                        },
                    }]),
                    tool_call_id: None,
                    name: None,
                },
                32 => ChatMessage::tool_result(
                    &ToolCall {
                        id: "c1".into(),
                        typ: "function".into(),
                        function: FunctionCall {
                            name: "list_workspace".into(),
                            arguments: "{}".into(),
                        },
                    },
                    r#"{"items":[]}"#.into(),
                ),
                _ => mk(&"a".repeat(2000)),
            })
            .collect();
        let (boundary, _) = split_for_compaction(&history, 8).unwrap();
        assert_eq!(history.get(boundary).map(|m| m.role), Some(Role::User));
        // The exchange (31/32) stays inside the verbatim tail.
        assert!(boundary < 31);
        let tail_chars: usize = history
            .get(boundary..)
            .unwrap()
            .iter()
            .map(|m| m.text().len())
            .sum();
        assert!(tail_chars <= 28_000);

        // No user message in the compactable region at all → refuse to cut.
        let all_bot: Vec<ChatMessage> = (0..20)
            .map(|i| {
                if i == 0 {
                    mk("start")
                } else {
                    ChatMessage::assistant("x".repeat(3200))
                }
            })
            .collect();
        // Over the 60k threshold, but the only user message sits at index 0,
        // outside the valid cut range → refuse to split unsafely.
        assert!(all_bot.iter().map(|m| m.text().len()).sum::<usize>() >= 60_000);
        assert!(split_for_compaction(&all_bot, 8).is_none());
    }

    #[test]
    fn clamp_tool_result_caps_and_counts_chars() {
        // Short results pass through untouched.
        assert_eq!(clamp_tool_result("ok".into()), "ok");
        // Long results clip by chars (multibyte-safe) and report the
        // original char count.
        let big: String = "汉".repeat(20_000);
        let clamped = clamp_tool_result(big.clone());
        assert!(clamped.starts_with(&"汉".repeat(8_000)));
        assert!(clamped.contains("20"));
        assert!(clamped.contains("截断"));
        assert!(clamped.chars().count() < big.chars().count());
    }

    #[test]
    fn compaction_prompt_keeps_ids_and_roles() {
        let head = vec![
            ChatMessage::system("sys"),
            ChatMessage::user_text("目标：整理教师模块"),
            ChatMessage::assistant("已创建「获取老师列表」(id=abc123)"),
        ];
        let p = compaction_prompt(&head);
        assert!(p.contains("用户: 目标"));
        assert!(p.contains("abc123"));
        assert!(!p.contains("System"));
        assert!(p.contains("压缩"));
    }

    #[test]
    fn compaction_prompt_merges_prior_summary() {
        let head = vec![
            ChatMessage::user_text("第一轮目标：设计教师接口"),
            ChatMessage::user_text(format!(
                "{COMPACT_SUMMARY_MARKER}\n此前结论：目录「教师」(id=f01) 已建立。"
            )),
            ChatMessage::user_text("继续补充学生模块"),
        ];
        let p = compaction_prompt(&head);
        // Prior summary is labeled distinctly and kept whole…
        assert!(p.contains("此前摘要: "));
        assert!(p.contains("id=f01"));
        // …and the instructions ask for a merge, not a fresh summary.
        assert!(p.contains("合并"));
    }

    #[test]
    fn text_only_copy_strips_images() {
        let msg = ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "看这张架构图".into(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,AAAA".into(),
                    },
                },
            ])),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };
        let copy = text_only_copy(&msg).expect("text present");
        assert_eq!(copy.text(), "看这张架构图");

        let image_only = ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(vec![ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AAAA".into(),
                },
            }])),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        };
        assert!(text_only_copy(&image_only).is_none());
    }

    #[test]
    fn pad_missing_tool_results_fills_interrupted_exchange() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            typ: "function".into(),
            function: FunctionCall {
                name: "list_workspace".into(),
                arguments: "{}".into(),
            },
        };
        let answered = ChatMessage::tool_result(&call("c1"), r#"{"ok":true}"#.into());
        let mut history = vec![
            ChatMessage::user_text("任务"),
            ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![call("c1"), call("c2"), call("c3")]),
                tool_call_id: None,
                name: None,
            },
            answered,
            ChatMessage::user_text("中断后新消息"),
            ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![call("c4")]),
                tool_call_id: None,
                name: None,
            },
        ];
        let padded = pad_missing_tool_results(&mut history);
        assert_eq!(padded, 3);
        // Stubs for c2/c3 land before the new user message; c4's stub at the end.
        let roles: Vec<Role> = history.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                Role::User,
                Role::Assistant,
                Role::Tool, // c1
                Role::Tool, // c2 stub
                Role::Tool, // c3 stub
                Role::User,
                Role::Assistant,
                Role::Tool, // c4 stub
            ]
        );
        let stub = history.get(3).unwrap();
        assert_eq!(stub.tool_call_id.as_deref(), Some("c2"));
        assert!(stub.text().contains("未执行"));
        // Idempotent.
        assert_eq!(pad_missing_tool_results(&mut history), 0);
    }

    #[test]
    fn plan_steps_enforce_single_in_progress_and_caps() {
        use super::{PlanStatus, PlanStep, normalize_plan_steps, work_plan_prompt};
        let steps = vec![
            PlanStep {
                step: "读取工作区".into(),
                detail: String::new(),
                status: PlanStatus::Completed,
            },
            PlanStep {
                step: "整理教师模块".into(),
                detail: "移动 edu-link-test01..24 → 目录「账号中心」".into(),
                status: PlanStatus::InProgress,
            },
            PlanStep {
                step: "整理学生模块".into(),
                detail: "学".repeat(600),
                status: PlanStatus::InProgress,
            },
        ];
        let out = normalize_plan_steps(steps);
        // 只有最后一个 in_progress 保留,前一个降级 pending。
        assert_eq!(out[1].status, PlanStatus::Pending);
        assert_eq!(out[2].status, PlanStatus::InProgress);
        // detail 截断到 500 字符。
        assert_eq!(out[2].detail.chars().count(), 500);
        // 提示词块包含状态标记与细节。
        let prompt = work_plan_prompt(&out);
        assert!(prompt.contains("[已完成] 读取工作区"));
        assert!(prompt.contains("[待办] 整理教师模块"));
        assert!(prompt.contains("[进行中] 整理学生模块"));
        assert!(prompt.contains("账号中心"));
        assert!(work_plan_prompt(&[]).is_empty());
    }

    #[test]
    fn bad_args_error_is_actionable() {
        let diag = ToolArgDiag {
            call_id: "c1".into(),
            name: "propose_changes".into(),
            raw_len: 38_412,
            head: r#"{"ops":[{"op":"move_node","id":"req_1"#.into(),
        };
        let msg = bad_args_error(&diag, true);
        assert!(msg.contains("propose_changes"));
        assert!(msg.contains("38412"));
        assert!(msg.contains("截断"));
        assert!(msg.contains("8 个 op"));
        let msg2 = bad_args_error(&diag, false);
        assert!(msg2.contains("拼装损坏"));
    }

    #[test]
    fn repeated_failure_guidance_escalates() {
        assert!(repeated_failure_guidance(1).is_none());
        assert!(repeated_failure_guidance(2).is_none());
        assert!(repeated_failure_guidance(3).is_some());
        assert!(repeated_failure_guidance(4).is_none());
    }

    #[test]
    fn extract_id_table_pulls_pairs_from_tool_results() {
        let middle = vec![
            ChatMessage::user_text("问题"),
            ChatMessage {
                role: Role::Tool,
                content: Some(MessageContent::Text(
                    r#"{"items":[{"id":"f1","name":"账号中心","kind":"folder"},{"id":"r1","name":"登录","kind":"request"}],"total":2}"#.into(),
                )),
                tool_calls: None,
                tool_call_id: Some("t1".into()),
                name: None,
            },
            ChatMessage {
                role: Role::Tool,
                content: Some(MessageContent::Text("not json".into())),
                tool_calls: None,
                tool_call_id: Some("t2".into()),
                name: None,
            },
        ];
        let table = extract_id_table(&middle);
        assert_eq!(table.len(), 2);
        assert!(table.contains(&("账号中心".to_string(), "f1".to_string())));
        assert!(table.contains(&("登录".to_string(), "r1".to_string())));
    }

    #[test]
    fn role_enum_reexported_for_tests() {
        // ChatMessage helper sanity inside this module.
        assert_eq!(ChatMessage::user_text("x").role, Role::User);
    }
}
