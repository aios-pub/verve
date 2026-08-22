//! OpenAI-compatible chat wire types (`POST {base_url}/chat/completions`).
//!
//! All structs serde-mirror the vendor JSON exactly (snake_case field names,
//! optional fields omitted rather than nulled — several vendors reject
//! explicit nulls). Non-streaming responses and streaming chunks share the
//! `ChatMessage` shape; [`StreamAccumulator`] reassembles chunk fragments
//! into a final message, including OpenAI's index-keyed `tool_calls`
//! argument sharding.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Messages & tools
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    ImageUrl {
        image_url: ImageUrl,
    },
}

/// Message content: plain text or multimodal parts (text + image_url).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Default for MessageContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    /// Always `"function"` on this wire format.
    #[serde(rename = "type", default = "tool_type")]
    pub typ: String,
    pub function: FunctionCall,
}

impl Default for ToolCall {
    // Manual impl: `derive(Default)` would leave `typ` as an EMPTY string
    // (serde's `default = "tool_type"` only covers deserialization), and
    // streamed tool-call assembly pushes defaults — the empty type then
    // round-trips back to vendors as `tool_calls[].type: ""`, which strict
    // gateways (OpenAI/OpenRouter) reject with 400 "Input should be
    // 'function'".
    fn default() -> Self {
        Self {
            id: String::new(),
            typ: tool_type(),
            function: FunctionCall::default(),
        }
    }
}

fn tool_type() -> String {
    "function".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// Present on assistant messages that requested tool execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Present on tool-role result messages, echoing the call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Function name attached to tool results (accepted by all major vendors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Default for ChatMessage {
    fn default() -> Self {
        Self::assistant(String::new())
    }
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(MessageContent::Text(text.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(MessageContent::Text(text.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            role: Role::User,
            content: Some(MessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// A tool-result message answering `call` with JSON-ish text.
    pub fn tool_result(call: &ToolCall, content: String) -> Self {
        Self {
            role: Role::Tool,
            content: Some(MessageContent::Text(content)),
            tool_calls: None,
            tool_call_id: Some(call.id.clone()),
            name: Some(call.function.name.clone()),
        }
    }

    /// Append a streamed text fragment (assistant accumulation).
    pub fn append_text(&mut self, fragment: &str) {
        let next = match self.content.take() {
            Some(MessageContent::Text(mut t)) => {
                t.push_str(fragment);
                MessageContent::Text(t)
            }
            Some(MessageContent::Parts(_)) | None => MessageContent::Text(fragment.to_string()),
        };
        self.content = Some(next);
    }

    /// Plain-text view of the content (parts contribute their text segments).
    pub fn text(&self) -> String {
        match &self.content {
            Some(MessageContent::Text(t)) => t.clone(),
            Some(MessageContent::Parts(parts)) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            None => String::new(),
        }
    }
}

/// A function tool. Serializes to the OpenAI wire shape:
/// `{"type":"function","function":{"name":…,"description":…,"parameters":…}}`.
/// The `type` discriminator is mandatory on several vendors (e.g. DashScope
/// rejects tools without it with `tools[0].type: type cannot be empty`).
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the function parameters.
    pub parameters: Value,
}

#[derive(Serialize, Deserialize)]
struct WireFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Serialize, Deserialize)]
struct WireTool {
    #[serde(rename = "type")]
    typ: String,
    function: WireFunction,
}

impl Serialize for ToolDef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        WireTool {
            typ: "function".to_string(),
            function: WireFunction {
                name: self.name.clone(),
                description: self.description.clone(),
                parameters: self.parameters.clone(),
            },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WireTool::deserialize(deserializer)?;
        if wire.typ != "function" {
            return Err(serde::de::Error::custom(format!(
                "unsupported tool type: {}",
                wire.typ
            )));
        }
        Ok(ToolDef {
            name: wire.function.name,
            description: wire.function.description,
            parameters: wire.function.parameters,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

// ---------------------------------------------------------------------------
// Usage & responses
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatChoice {
    pub message: ChatMessage,
    #[serde(default)]
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub choices: Vec<ChatChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

// ---------------------------------------------------------------------------
// Streaming chunks
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeltaFunction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeltaToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<DeltaFunction>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Delta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StreamChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: Delta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Incremental events surfaced to the UI while a stream plays out.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// A text-content fragment.
    Text(String),
    /// A tool-call fragment arrived (arguments sharded by index).
    ToolFragment {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    /// The stream finished with this reason (`stop`, `tool_calls`, …).
    Finish(String),
    /// Usage arrived (typically on the final chunk, `include_usage`).
    Usage(Usage),
}

/// Reassembles streaming chunks into a final assistant message.
///
/// Tool calls arrive sharded: each fragment carries the `index` it belongs
/// to, with `id`/`function.name` set only on the first fragment for that
/// Diagnosis for one tool call whose streamed arguments assembled into
/// invalid JSON (sanitized to `{}` before the message enters history).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolArgDiag {
    pub call_id: String,
    pub name: String,
    pub raw_len: usize,
    /// First chars of the raw (invalid) payload — enough for the model to
    /// recognize what it tried to send.
    pub head: String,
}

/// index and `function.arguments` appended across fragments.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    message: ChatMessage,
    tool_calls: Vec<ToolCall>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self {
            message: ChatMessage::assistant(String::new()),
            ..Default::default()
        }
    }

    /// Ingest one parsed chunk; returns the events it produced.
    pub fn ingest(&mut self, chunk: &StreamChunk) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        if let Some(u) = chunk.usage {
            events.push(StreamEvent::Usage(u));
            self.usage.get_or_insert_with(Usage::default).add(u);
        }
        for choice in &chunk.choices {
            let delta = &choice.delta;
            if let Some(text) = delta.content.as_deref().filter(|t| !t.is_empty()) {
                self.message.append_text(text);
                events.push(StreamEvent::Text(text.to_string()));
            }
            if let Some(frags) = delta.tool_calls.as_deref() {
                for frag in frags {
                    let index = frag.index.unwrap_or(0);
                    if let Some(evt) = self.ingest_tool_fragment(index, frag) {
                        events.push(evt);
                    }
                }
            }
            if let Some(reason) = choice.finish_reason.as_deref() {
                self.finish_reason = Some(reason.to_string());
                events.push(StreamEvent::Finish(reason.to_string()));
            }
        }
        events
    }

    fn ingest_tool_fragment(&mut self, index: u32, frag: &DeltaToolCall) -> Option<StreamEvent> {
        // Grow until `index` is addressable via get().
        while self.tool_calls.len() <= index as usize {
            self.tool_calls.push(ToolCall::default());
        }
        let Some(call) = self.tool_calls.get_mut(index as usize) else {
            return None;
        };
        let mut name = None;
        if let Some(id) = frag.id.as_deref() {
            call.id = id.to_string();
        }
        if let Some(f) = frag.function.as_ref() {
            if let Some(n) = f.name.as_deref().filter(|n| !n.is_empty()) {
                call.function.name = n.to_string();
                name = Some(n.to_string());
            }
            if let Some(args) = f.arguments.as_deref().filter(|a| !a.is_empty()) {
                call.function.arguments.push_str(args);
            }
        }
        let arguments = frag
            .function
            .as_ref()
            .and_then(|f| f.arguments.clone())
            .unwrap_or_default();
        Some(StreamEvent::ToolFragment {
            index,
            id: frag.id.clone(),
            name,
            arguments,
        })
    }

    /// Consume the accumulator, producing the final assistant message with
    /// assembled tool calls (empty-id calls are dropped: vendors that omit
    /// ids also omit names/arguments, and the agent cannot answer them).
    /// The fourth value diagnoses calls whose streamed arguments did not
    /// assemble into valid JSON (they are sanitized to `{}` for replay
    /// safety — without the diagnosis the executor only sees "missing
    /// parameter" and the model blindly retries the same huge payload).
    pub fn finish(
        mut self,
    ) -> (ChatMessage, Option<String>, Option<Usage>, Vec<ToolArgDiag>) {
        let mut diag = Vec::new();
        self.message.tool_calls = match self.tool_calls.is_empty() {
            true => None,
            false => Some(
                self.tool_calls
                    .into_iter()
                    .filter(|c| !c.id.is_empty() && !c.function.name.is_empty())
                    .map(|mut c| {
                        if c.typ.is_empty() {
                            c.typ = "function".to_string();
                        }
                        // Sharded streaming can assemble arguments that are
                        // not valid JSON; replaying them in history makes
                        // strict gateways 400 ("arguments must be valid
                        // JSON"). Sanitize here, but keep a diagnosis so the
                        // agent can tell the model what actually happened.
                        if serde_json::from_str::<Value>(&c.function.arguments).is_err() {
                            diag.push(ToolArgDiag {
                                call_id: c.id.clone(),
                                name: c.function.name.clone(),
                                raw_len: c.function.arguments.chars().count(),
                                head: c.function.arguments.chars().take(200).collect(),
                            });
                            c.function.arguments = "{}".to_string();
                        }
                        c
                    })
                    .collect(),
            ),
        };
        (self.message, self.finish_reason, self.usage, diag)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChatMessage, ChatRequest, ChatResponse, ContentPart, MessageContent, Role, StreamAccumulator,
        StreamChunk, ToolCall, Usage,
    };

    #[test]
    fn message_helpers_fill_wire_fields() {
        let m = ChatMessage::system("hello");
        assert_eq!(m.role, Role::System);
        assert_eq!(m.text(), "hello");
        assert!(m.tool_calls.is_none());

        let call = ToolCall {
            id: "call_1".into(),
            typ: "function".into(),
            function: super::FunctionCall {
                name: "list_workspace".into(),
                arguments: "{}".into(),
            },
        };
        let tool_msg = ChatMessage::tool_result(&call, r#"{"ok":true}"#.into());
        assert_eq!(tool_msg.role, Role::Tool);
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(tool_msg.name.as_deref(), Some("list_workspace"));

        let parts = ChatMessage::user_parts(vec![
            ContentPart::Text { text: "看图".into() },
            ContentPart::ImageUrl {
                image_url: super::ImageUrl {
                    url: "data:image/png;base64,AAA".into(),
                },
            },
        ]);
        assert_eq!(parts.text(), "看图");
    }

    #[test]
    fn tools_serialize_with_type_discriminator() {
        let req = super::ChatRequest {
            model: "m".into(),
            messages: vec![super::ChatMessage::user_text("hi")],
            tools: Some(vec![super::ToolDef {
                name: "list_workspace".into(),
                description: "d".into(),
                parameters: serde_json::json!({"type": "object"}),
            }]),
            ..Default::default()
        };
        let json = serde_json::to_value(&req).unwrap();
        let tool = &json["tools"][0];
        assert_eq!(tool["type"], "function", "wire: {json}");
        assert_eq!(tool["function"]["name"], "list_workspace");
        assert_eq!(tool["function"]["parameters"]["type"], "object");
        // Round-trip.
        let back: super::ChatRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.tools.as_deref().unwrap_or(&[])[0].name, "list_workspace");
    }

    #[test]
    fn request_omits_optional_fields() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![ChatMessage::user_text("hi")],
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: Some(true),
            stream_options: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "m");
        assert_eq!(json["stream"], true);
        assert!(json.get("tools").is_none());
        assert!(json.get("temperature").is_none());
        assert!(json["messages"][0].get("tool_calls").is_none());
    }

    #[test]
    fn parse_non_stream_response_with_tools() {
        // Shape mirrors real vendor payloads (trimmed).
        let raw = r#"{
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": {"name": "get_request", "arguments": "{\"id\":\"r1\"}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 5, "total_tokens": 105}
        }"#;
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        let choice = resp.choices.first().unwrap();
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        let calls = choice.message.tool_calls.as_deref().unwrap_or(&[]);
        let Some(c0) = calls.first() else { return };
        assert_eq!(c0.id, "call_9");
        assert_eq!(c0.function.name, "get_request");
        assert_eq!(c0.function.arguments, r#"{"id":"r1"}"#);
        let usage = resp.usage.unwrap();
        assert_eq!(usage.total_tokens, 105);
    }

    #[test]
    fn stream_accumulator_reassembles_text_and_sharded_tool_calls() {
        let mut acc = StreamAccumulator::new();
        let chunks: Vec<StreamChunk> = serde_json::from_str(
            r#"[
              {"choices":[{"index":0,"delta":{"role":"assistant","content":"你"}}]},
              {"choices":[{"index":0,"delta":{"content":"好"}}]},
              {"choices":[{"index":0,"delta":{"tool_calls":[
                 {"index":0,"id":"call_a","function":{"name":"create_folder","arguments":"{\"na"}}]}}]},
              {"choices":[{"index":0,"delta":{"tool_calls":[
                 {"index":0,"function":{"arguments":"me\":\"x\"}"}},{"index":1,"id":"call_b","function":{"name":"rename_node","arguments":"{}"}}]}}]},
              {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]},
              {"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":7,"total_tokens":49}}
            ]"#,
        )
        .unwrap();
        for c in &chunks {
            acc.ingest(c);
        }
        let (msg, reason, usage, _) = acc.finish();
        assert_eq!(msg.text(), "你好");
        assert_eq!(reason.as_deref(), Some("tool_calls"));
        assert_eq!(usage.map(|u| u.total_tokens), Some(49));
        let calls = msg.tool_calls.as_deref().unwrap_or(&[]);
        assert_eq!(calls.len(), 2);
        let (a, b) = (calls.first().unwrap(), calls.get(1).unwrap());
        assert_eq!(a.id, "call_a");
        assert_eq!(a.function.name, "create_folder");
        assert_eq!(a.function.arguments, r#"{"name":"x"}"#);
        assert_eq!(b.function.name, "rename_node");
    }

    #[test]
    fn accumulator_drops_empty_tool_calls() {
        let mut acc = StreamAccumulator::new();
        let chunk: StreamChunk =
            serde_json::from_str(r#"{"choices":[{"index":0,"delta":{"content":"hi"}}]}"#).unwrap();
        acc.ingest(&chunk);
        let (msg, _, _, _) = acc.finish();
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn accumulator_sanitizes_invalid_tool_arguments() {
        let mut acc = StreamAccumulator::new();
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[
               {"index":0,"id":"call_bad","function":{"name":"propose_changes","arguments":"{\"ops\":[{oops"}}]}}]}"#,
        )
        .unwrap();
        acc.ingest(&chunk);
        let (msg, _, _, diag) = acc.finish();
        let calls = msg.tool_calls.iter().flatten().collect::<Vec<_>>();
        assert_eq!(calls[0].function.arguments, "{}");
        assert_eq!(diag.len(), 1);
        assert_eq!(diag[0].call_id, "call_bad");
        assert_eq!(diag[0].name, "propose_changes");
        assert!(diag[0].raw_len > 0);
        assert!(diag[0].head.contains("ops"));
    }

    #[test]
    fn tool_call_default_has_function_type() {
        // Regression: derive(Default) produced type="" which strict vendors
        // rejected with HTTP 400 "Input should be 'function'".
        let c = ToolCall::default();
        assert_eq!(c.typ, "function");
        let mut acc = StreamAccumulator::new();
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[
               {"index":0,"id":"call_x","function":{"name":"t","arguments":"{}"}}]}}]}"#,
        )
        .unwrap();
        acc.ingest(&chunk);
        let (msg, _, _, _) = acc.finish();
        for call in msg.tool_calls.iter().flatten() {
            assert_eq!(call.typ, "function");
        }
    }

    #[test]
    fn usage_add_accumulates() {
        let mut u = Usage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
        };
        u.add(Usage {
            prompt_tokens: 5,
            completion_tokens: 1,
            total_tokens: 6,
        });
        assert_eq!(u.total_tokens, 18);
    }

    #[test]
    fn content_parts_untagged_roundtrip() {
        let m = ChatMessage::user_parts(vec![ContentPart::Text { text: "a".into() }]);
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["content"][0]["type"], "text");
        let back: ChatMessage = serde_json::from_value(json).unwrap();
        assert_eq!(back.text(), "a");
    }

    #[test]
    fn tool_message_serializes_role_lowercase() {
        let json = serde_json::to_value(ChatMessage::assistant("x")).unwrap();
        assert_eq!(json["role"], "assistant");
    }
}
