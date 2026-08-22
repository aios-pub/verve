//! OpenAI-compatible chat client over the shared `HttpClient`.
//!
//! Talks to `{base_url}/chat/completions` with Bearer auth. Streaming uses
//! `stream: true` + `stream_options.include_usage` so the final chunk carries
//! real token usage; vendors that reject `stream_options` get one automatic
//! retry without it (usage then falls back to local estimation upstream).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::AsyncReadExt as _;
use futures::future::Either;
use http_client::{AsyncBody, Builder, HttpRequestExt as _, Method, RedirectPolicy};

use super::chat::{ToolArgDiag, ChatMessage, ChatRequest, ChatResponse, StreamAccumulator, StreamChunk, StreamEvent,
    StreamOptions, Usage,};
use super::config::AiProviderConfig;

/// Connect timeout for the initial POST.
pub const CONNECT_TIMEOUT_SECS: u64 = 60;
/// Abort a stream when no bytes arrive for this long.
const IDLE_TIMEOUT_SECS: u64 = 180;
/// Cap on error-body bytes read for display.
const ERROR_BODY_LIMIT: usize = 64 * 1024;
/// Fixed interval between HTTP 429 retries (user-facing predictable pacing).
pub const RATE_RETRY_INTERVAL_SECS: u64 = 10;

/// Result of a completed (or stopped) stream.
pub struct StreamOutcome {
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
    /// True when the caller's stop flag interrupted the stream.
    pub stopped: bool,
    /// Calls whose streamed arguments were invalid JSON (sanitized to `{}`);
    /// the agent turns these into actionable tool errors instead of a bare
    /// "missing parameter" the model can only blindly retry.
    pub bad_args: Vec<ToolArgDiag>,
}

pub struct AiClient {
    http: Arc<dyn http_client::HttpClient>,
    provider: AiProviderConfig,
    api_key: String,
}

impl AiClient {
    /// Build a client for `provider`, resolving its API key from the vault.
    /// Uses the shared global app http client.
    pub fn for_provider(provider: &AiProviderConfig, cx: &mut gpui::App) -> Result<Self> {
        let api_key = super::config::load_provider_api_key(&provider.id).unwrap_or_default();
        Ok(Self {
            http: cx.http_client(),
            provider: provider.clone(),
            api_key,
        })
    }

    /// `.../chat/completions` endpoint from a base URL.
    pub fn endpoint(base_url: &str) -> String {
        format!("{}/chat/completions", base_url.trim_end_matches('/'))
    }

    /// The configured model name (used by background calls like style
    /// learning that don't carry an explicit request).
    pub fn model(&self) -> &str {
        &self.provider.model
    }

    fn build_post(&self, body_json: String, accept_sse: bool) -> Result<http::Request<AsyncBody>> {
        let mut builder = Builder::new()
            .method(Method::POST)
            .uri(Self::endpoint(&self.provider.base_url))
            .follow_redirects(RedirectPolicy::FollowAll)
            .header("Content-Type", "application/json");
        if !self.api_key.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", self.api_key));
        }
        if accept_sse {
            builder = builder.header("Accept", "text/event-stream");
        }
        builder
            .body(AsyncBody::from(body_json.into_bytes()))
            .context("build chat request")
    }

    async fn send_with_connect_timeout(
        &self,
        body_json: String,
        accept_sse: bool,
    ) -> Result<http_client::Response<AsyncBody>> {
        let req = self.build_post(body_json, accept_sse)?;
        let send_fut = self.http.send(req);
        let timer = smol::Timer::after(Duration::from_secs(CONNECT_TIMEOUT_SECS));
        match futures::future::select(Box::pin(send_fut), timer).await {
            Either::Left((resp, _)) => resp,
            Either::Right(_) => Err(anyhow::anyhow!(
                "连接超时（{CONNECT_TIMEOUT_SECS}s）— 检查网络或代理设置"
            )),
        }
    }

    /// Send, retrying HTTP 429 *and transport errors* (connect timeout,
    /// dropped connection — rate-limiting vendors often stall instead of
    /// answering) at a fixed {RATE_RETRY_INTERVAL_SECS}s interval, up to the
    /// user-set cap (`AiSettings.max_rate_limit_retries`, default 20).
    /// `on_retry(attempt, max, wait_secs)` fires before each wait so the UI
    /// can surface the retry progress. The wait is abortable via `stop`.
    async fn send_with_rate_limit_retry(
        &self,
        body_json: String,
        accept_sse: bool,
        stop: Option<&AtomicBool>,
        mut on_retry: impl FnMut(u32, u32, u64) + Send,
    ) -> Result<http_client::Response<AsyncBody>> {
        let max_retries = super::config::load_ai_settings().clamped_rate_retries() as u32;
        let mut attempt: u32 = 0;
        loop {
            // 1s slices so a user-initiated stop interrupts the wait quickly.
            let wait = async {
                for _ in 0..RATE_RETRY_INTERVAL_SECS {
                    smol::Timer::after(Duration::from_secs(1)).await;
                    if let Some(flag) = stop
                        && flag.load(Ordering::SeqCst)
                    {
                        return false;
                    }
                }
                true
            };
            let sent = self
                .send_with_connect_timeout(body_json.clone(), accept_sse)
                .await;
            // Classify: success returns immediately; both failure kinds share
            // one retry budget and one pacing. Transport errors (connect
            // timeout, dropped connection) are retried too — rate-limiting
            // vendors often stall instead of answering, and bailing out on
            // the first timeout made the earlier "no more retry logs" bug.
            let failure: Option<String> = match sent {
                Ok(resp) if resp.status().as_u16() != 429 => return Ok(resp),
                Ok(resp) => {
                    if attempt >= max_retries {
                        let status = resp.status();
                        let e = Self::error_from_response(status, resp.into_body()).await;
                        return Err(anyhow::anyhow!(
                            "{e}（已每 {RATE_RETRY_INTERVAL_SECS}s 重试 {max_retries} 次仍未通过）"
                        ));
                    }
                    Some("429 限流".to_string())
                }
                Err(e) => {
                    if attempt >= max_retries {
                        return Err(anyhow::anyhow!(
                            "{e}（已每 {RATE_RETRY_INTERVAL_SECS}s 重试 {max_retries} 次仍未通过）"
                        ));
                    }
                    Some(format!("请求失败：{e}"))
                }
            };
            if let Some(reason) = failure {
                attempt += 1;
                log::warn!(
                    "AI {reason}：第 {attempt}/{max_retries} 次重试，{RATE_RETRY_INTERVAL_SECS}s 后重发"
                );
                on_retry(attempt, max_retries, RATE_RETRY_INTERVAL_SECS);
                if !wait.await {
                    return Err(anyhow::anyhow!("已停止"));
                }
            }
        }
    }

    /// Read the error body (capped) and turn a failed status into an error
    /// that carries the vendor's own message.
    async fn error_from_response(
        status: http::StatusCode,
        mut body: AsyncBody,
    ) -> anyhow::Error {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        while buf.len() < ERROR_BODY_LIMIT {
            let n = body.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let text = String::from_utf8_lossy(&buf);
        anyhow::anyhow!("HTTP {status}: {}", head_chars(&text, 500))
    }

    /// Non-streaming completion.
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let wire = ChatRequest {
            stream: None,
            stream_options: None,
            ..req.clone()
        };
        let body_json = serde_json::to_string(&wire).context("serialize chat request")?;
        let resp = self
            .send_with_connect_timeout(body_json, false)
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(Self::error_from_response(status, resp.into_body()).await);
        }
        let mut body = resp.into_body();
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        loop {
            let n = body.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 32 * 1024 * 1024 {
                return Err(anyhow::anyhow!("响应体过大（>32MB）"));
            }
        }
        let text = String::from_utf8_lossy(&buf);
        serde_json::from_str(&text).with_context(|| {
            format!("解析响应失败: {}", head_chars(&text, 300))
        })
    }

    /// Streaming completion with auto-degrade: if the vendor rejects the
    /// request before any event was emitted (e.g. it dislikes
    /// `stream_options`), retry once without the field.
    pub async fn chat_stream(
        &self,
        req: &ChatRequest,
        stop: Arc<AtomicBool>,
        mut on_event: impl FnMut(StreamEvent) + Send,
        mut on_retry: impl FnMut(u32, u32, u64) + Send,
    ) -> Result<StreamOutcome> {
        let mut emitted = false;
        {
            let mut cb = |ev: StreamEvent| {
                emitted = true;
                on_event(ev);
            };
            let wire = ChatRequest {
                stream: Some(true),
                stream_options: Some(StreamOptions { include_usage: true }),
                ..req.clone()
            };
            if let Ok((outcome, _)) = self.stream_once(&wire, &stop, &mut cb, &mut on_retry).await {
                return Ok(outcome);
            }
        }
        if emitted {
            // Events already reached the caller — a retry would duplicate
            // them. Surface via the second attempt only when nothing was sent.
            return Err(anyhow::anyhow!("流式请求中断"));
        }
        let wire = ChatRequest {
            stream: Some(true),
            stream_options: None,
            ..req.clone()
        };
        let mut cb = |ev: StreamEvent| on_event(ev);
        let (outcome, _) = self.stream_once(&wire, &stop, &mut cb, &mut on_retry).await?;
        Ok(outcome)
    }

    /// One streaming attempt. Returns the outcome plus whether any event was
    /// emitted (used by the auto-degrade logic above).
    async fn stream_once(
        &self,
        wire: &ChatRequest,
        stop: &AtomicBool,
        on_event: &mut dyn FnMut(StreamEvent),
        on_retry: &mut (dyn FnMut(u32, u32, u64) + Send),
    ) -> Result<(StreamOutcome, bool)> {
        let body_json = serde_json::to_string(wire).context("serialize chat request")?;
        let resp = self
            .send_with_rate_limit_retry(
                body_json,
                true,
                Some(stop),
                |a, m, w| on_retry(a, m, w),
            )
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(Self::error_from_response(status, resp.into_body()).await);
        }

        let mut body = resp.into_body();
        let mut parser = crate::http::sse::SseParser::new();
        let mut acc = StreamAccumulator::new();
        let mut emitted = false;
        let mut stopped = false;
        let mut chunk = [0u8; 4096];

        loop {
            if stop.load(Ordering::SeqCst) {
                stopped = true;
                break;
            }
            let n = {
                let read_fut = body.read(&mut chunk);
                let timer = smol::Timer::after(Duration::from_secs(IDLE_TIMEOUT_SECS));
                match futures::future::select(Box::pin(read_fut), timer).await {
                    Either::Left((n, _)) => n.unwrap_or(0),
                    Either::Right(_) => {
                        return Err(anyhow::anyhow!("空闲超时（{IDLE_TIMEOUT_SECS}s 无数据）"))
                    }
                }
            };
            if n == 0 {
                break;
            }
            let mut done = false;
            for ev in parser.feed(&chunk[..n]) {
                if ev.data.trim() == "[DONE]" {
                    done = true;
                    break;
                }
                let Ok(parsed) = serde_json::from_str::<StreamChunk>(&ev.data) else {
                    // Keep-alives / non-JSON payloads are skipped.
                    continue;
                };
                for evt in acc.ingest(&parsed) {
                    emitted = true;
                    on_event(evt);
                }
            }
            if done {
                break;
            }
        }

        let (message, finish_reason, usage, bad_args) = acc.finish();
        Ok((
            StreamOutcome {
                message,
                finish_reason,
                usage,
                stopped,
                bad_args,
            },
            emitted,
        ))
    }

    /// Connectivity probe: one tiny completion returning real usage.
    pub async fn ping(&self) -> Result<Usage> {
        let req = ChatRequest {
            model: self.provider.model.clone(),
            messages: vec![ChatMessage::user_text("ping")],
            max_tokens: Some(8),
            ..Default::default()
        };
        let resp = self.chat(&req).await?;
        Ok(resp.usage.unwrap_or_default())
    }
}

/// First `n` chars of `s` (char-boundary safe) with an ellipsis marker.
fn head_chars(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if out.chars().count() < s.chars().count() {
        out.push('…');
    }
    out
}

/// Build a client for `provider` and probe it (settings "测试连接").
pub async fn test_connection(
    provider: &AiProviderConfig,
    cx: &mut gpui::App,
) -> Result<Usage> {
    let client = AiClient::for_provider(provider, cx)?;
    client.ping().await
}

#[cfg(test)]
mod tests {
    use super::AiClient;

    #[test]
    fn endpoint_joins_without_double_slash() {
        assert_eq!(
            AiClient::endpoint("https://api.x.com/v1/"),
            "https://api.x.com/v1/chat/completions"
        );
        assert_eq!(
            AiClient::endpoint("https://api.x.com/v1"),
            "https://api.x.com/v1/chat/completions"
        );
    }

    #[test]
    fn head_chars_respects_char_boundaries() {
        assert_eq!(super::head_chars("abcdef", 3), "abc…");
        // 5 CJK chars, take 2 → no panic, ellipsis appended.
        assert_eq!(super::head_chars("你好世界呀", 2), "你好…");
        assert_eq!(super::head_chars("ab", 5), "ab");
    }
}
