//! The OpenAI-compatible model client (native-agent-harness Task 4, ADR
//! 0012: v1 is OpenAI-compatible only — the user's entire current model
//! set is). `OpenAiCompatibleProvider` speaks the OpenAI-compatible
//! chat-completions API: a `POST {base_url}/chat/completions` with
//! `stream: true`, parsing the SSE response (content / reasoning /
//! tool-call deltas, `finish_reason`, `usage`) into a `ProviderEvent`
//! stream.
//!
//! The [`Provider`] trait is the seam the `AgentLoop` (Task 6) calls —
//! it is provider-agnostic in shape (a `complete()` returning a boxed,
//! `'static`, `Send` stream) so a second provider (Anthropic) can be
//! added later without touching the loop.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;

use crate::agent::tools::ContentBlock;

/// A model request (the `AgentLoop`'s `complete()` argument, Task 6).
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// The model id (e.g. `tama` / an OpenRouter slug).
    pub model: String,
    /// The conversation so far.
    pub messages: Vec<ChatMessage>,
    /// The tool specs the model may call (empty = no tools).
    pub tools: Vec<ToolSpec>,
    /// Sampling / limits.
    pub options: ModelOptions,
}

/// A message's content: plain text, or a list of content blocks (mirrors
/// the Task 1 [`ContentBlock`] shape — text or a base64 image).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// `{ "content": "..." }` (the common case).
    Text(String),
    /// `{ "content": [{ "type": "text" | "image", ... }] }`.
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// The OpenAI-compatible WIRE form of the content: `Text` → a plain
    /// string (`{ "content": "..." }`); `Blocks` → an array of OpenAI
    /// content parts (a text part `{ "type": "text", "text" }` — the
    /// `ContentBlock::Text` wire shape verbatim — or an image part
    /// `{ "type": "image_url", "image_url": { "url": "data:<mime>;base64,…"
    /// } }`, the OpenAI data-URI shape).
    ///
    /// This is the ONLY serialization used on the request wire
    /// (`request_body`); the `ContentBlock`'s own (pi-shaped) serialization
    /// is the DB / transcript / tool-result shape, NOT the model request.
    pub fn to_wire(&self) -> Value {
        match self {
            MessageContent::Text(t) => Value::String(t.clone()),
            MessageContent::Blocks(blocks) => {
                let parts: Vec<Value> = blocks
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => json!({
                            "type": "text",
                            "text": text,
                        }),
                        // The OpenAI image wire shape: a `data:` URI
                        // (`data:<mime>;base64,<base64>`) in
                        // `image_url.url`. The `ContentBlock::Image` field
                        // is `data` (base64, NO `data:` prefix) + `mimeType`
                        // (renamed from `mime_type`), so compose the URI here.
                        ContentBlock::Image { image } => json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", image.mime_type, image.data),
                            },
                        }),
                    })
                    .collect();
                Value::Array(parts)
            }
        }
    }
}

/// A chat message's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    /// A tool result (paired with `tool_call_id`).
    Tool,
}

/// One chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: MessageContent,
    /// Set on a `tool` message: the `tool_call.id` this result answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Set on an `assistant` message that called tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// A tool the model may call (the OpenAI `tools[]` entry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// The JSON Schema of the tool's parameters.
    pub parameters: Value,
}

/// A complete tool call (accumulated from `ToolCallDelta`s).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The provider-assigned id (e.g. `call_1`).
    pub id: String,
    /// The tool name.
    pub name: String,
    /// The parsed arguments (an empty object when the model sent none).
    pub arguments: Value,
}

/// The REQUEST-wire shape of a tool call (the OpenAI contract: a nested
/// `function` object, a `type: "function"` tag, and `arguments` as a JSON
/// **string**). Used ONLY when building the `request_body` messages — the
/// display wire (`toolcall_end` / `tool_execution_start`) keeps the flat
/// [`ToolCall`] shape, and `ChatMessage`'s own serialization (the DB /
/// transcript) is untouched.
struct WireToolCall<'a> {
    id: &'a str,
    name: &'a str,
    arguments: &'a Value,
}

/// The `function` object of a [`WireToolCall`].
struct WireFunction<'a> {
    name: &'a str,
    arguments: &'a Value,
}

impl Serialize for WireToolCall<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(3))?;
        map.serialize_entry("id", self.id)?;
        map.serialize_entry("type", "function")?;
        map.serialize_entry(
            "function",
            &WireFunction {
                name: self.name,
                arguments: self.arguments,
            },
        )?;
        map.end()
    }
}

impl Serialize for WireFunction<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("name", self.name)?;
        // The arguments as a JSON **string** (the OpenAI contract) —
        // re-serialized from the parsed `Value`.
        let text = serde_json::to_string(self.arguments).map_err(serde::ser::Error::custom)?;
        map.serialize_entry("arguments", &text)?;
        map.end()
    }
}

/// A partial tool call, accumulated across chunks (keyed by `index`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallDelta {
    /// The `tool_calls[].index` (stable across a call's fragments).
    pub index: u32,
    /// Present when the provider sends the id (usually the first fragment).
    pub id: Option<String>,
    /// Name fragments (the provider may split the name across chunks).
    pub name: Option<String>,
    /// Argument-string fragments (JSON text, accumulated and parsed at
    /// the end).
    pub arguments: Option<String>,
}

/// Sampling / limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Passed through if the provider supports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Always `true` for `complete()` (the stream is the response).
    pub stream: bool,
}

/// Token usage reported by the provider (usually a trailing chunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Why the model finished its turn.
///
/// Renamed from `StopReason` (reviewer-corrected): `session.rs` already
/// exports `pub enum StopReason` (the ACP wire values). The `AgentLoop`
/// (Task 6) maps this → the session `StopReason`
/// (`Stop` → `EndTurn`, `Length` → `MaxTokens`, `Error` → via the error
/// path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// The model finished its turn normally.
    Stop,
    /// The model called tools (the loop executes them and continues).
    ToolCalls,
    /// The model hit its output token limit.
    Length,
    /// The provider reported an error.
    Error,
}

/// One event of the `complete()` stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    /// A fragment of the assistant's text.
    TextDelta(String),
    /// A fragment of the assistant's reasoning (when the model emits it).
    ThinkingDelta(String),
    /// A partial tool call (accumulated across chunks, `index`-keyed).
    ToolCallDelta(ToolCallDelta),
    /// A complete tool call (accumulated `name` + `arguments`).
    ToolCall(ToolCall),
    /// The token usage (usually a trailing chunk).
    Usage(Usage),
    /// The turn is over.
    Done(FinishReason),
    /// A mid-stream error (the stream ends after this).
    Error(ProviderError),
}

/// A provider error. The `AgentLoop`'s `RetryPolicy` (Task 6) retries on
/// [`Retryable`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    /// Transient (HTTP 429 / 5xx, a network failure) — worth retrying.
    #[error("retryable: {0}")]
    Retryable(String),
    /// Permanent (other 4xx, a malformed response) — not worth retrying.
    #[error("fatal: {0}")]
    Fatal(String),
    /// Missing / bad credentials (HTTP 401 / 403).
    #[error("auth: {0}")]
    Auth(String),
}

/// A model provider. Object-safe (a `Box<dyn Provider>` compiles) so a
/// second provider (Anthropic) can be added later without touching the
/// `AgentLoop` (Task 6).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Run one model turn: a streaming chat-completions call. Returns a
    /// boxed stream of [`ProviderEvent`]s (the `AgentLoop` consumes it
    /// with `futures_util::StreamExt`).
    async fn complete(
        &self,
        req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError>;
}

/// A client for the OpenAI-compatible chat-completions API (OpenAI,
/// OpenRouter, `tama`, …).
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleProvider {
    /// The API base (e.g. `https://openrouter.ai/api/v1` — the client
    /// appends `/chat/completions`).
    pub base_url: String,
    /// The `Authorization: Bearer` key.
    pub api_key: String,
}

/// The accumulated per-`index` tool-call state (fragments arrive as
/// `id` / `name` / `arguments` pieces).
#[derive(Default)]
struct ToolAcc {
    id: Option<String>,
    name: Option<String>,
    /// The argument fragments concatenated (JSON text).
    arguments: String,
}

/// The SSE line/chunk parser: turns raw response bytes into
/// [`ProviderEvent`]s (content / reasoning / tool-call deltas,
/// `finish_reason` → `Done`, `usage` → `Usage`). The line buffer holds
/// RAW BYTES (a network chunk may split a multi-byte codepoint; a
/// per-chunk `from_utf8_lossy` would replace each half with U+FFFD
/// before line assembly — only COMPLETE lines are converted).
struct SseStream {
    /// The raw byte stream (`reqwest::bytes_stream`, mapped to `Vec<u8>`).
    raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    /// The not-yet-`\n`-terminated tail of the response (RAW bytes — an
    /// incomplete trailing multi-byte sequence is held back between
    /// chunks and only converted once its line is complete).
    buf: Vec<u8>,
    /// Events parsed but not yet delivered.
    pending: VecDeque<ProviderEvent>,
    /// Per-`index` tool-call accumulation (flushed as `ToolCall`s at
    /// `finish_reason` / stream end).
    tool_acc: BTreeMap<u32, ToolAcc>,
    /// A `finish_reason` chunk was seen (no synthetic `Done` at end).
    finished: bool,
    /// The raw stream is exhausted (the stream ends after `pending`).
    exhausted: bool,
}

impl SseStream {
    fn new(raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>) -> Self {
        Self {
            raw,
            buf: Vec::new(),
            pending: VecDeque::new(),
            tool_acc: BTreeMap::new(),
            finished: false,
            exhausted: false,
        }
    }

    /// Append a raw chunk; process every complete line it contains.
    ///
    /// The bytes are buffered RAW and only the `\n`-terminated lines are
    /// converted to UTF-8 (a multi-byte codepoint split across chunks is
    /// reassembled here, NOT lossy-converted per chunk — finding 9).
    ///
    /// (`drain(..=pos)`, NOT `split_off`: `split_off(at)` returns the
    /// portion AFTER `at` and keeps the before-portion in `self` — the
    /// exact opposite of what is needed here.)
    fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            self.handle_line(&line);
        }
    }

    /// Handle one SSE line (`data: <json>`; `:`-comments and other
    /// fields are ignored, as is `data: [DONE]`).
    fn handle_line(&mut self, line: &str) {
        let Some(payload) = line.trim().strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        self.handle_chunk(&chunk);
    }

    /// Handle one parsed chunk (a `choices[0].delta` + optional
    /// top-level `usage`).
    fn handle_chunk(&mut self, v: &Value) {
        // A trailing chunk may carry `usage` with an empty `choices`.
        // The wire `u64` token counts saturate to `u32::MAX` (a silent
        // `as u32` would TRUNCATE — a 4 GB token count would wrap to a
        // small one).
        if let Some(u) = v.get("usage").and_then(|u| {
            u.get("prompt_tokens")
                .and_then(|t| t.as_u64())
                .zip(u.get("completion_tokens").and_then(|t| t.as_u64()))
        }) {
            self.pending.push_back(ProviderEvent::Usage(Usage {
                input_tokens: u32::try_from(u.0).unwrap_or(u32::MAX),
                output_tokens: u32::try_from(u.1).unwrap_or(u32::MAX),
            }));
        }
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return;
        };
        let Some(delta) = choice.get("delta") else {
            return;
        };
        // A single chunk can carry BOTH a `reasoning` fragment and a
        // `content` fragment (some providers batch the thinking's tail
        // with the text's head — the transition chunk). The thinking
        // block PRECEDES the text block in the model's output, so the
        // `ThinkingDelta` must be emitted BEFORE the `TextDelta` — the
        // reverse order would make the frontend start the text block
        // first and file the thinking's tail under a SECOND thinking
        // block (the "missing last word" bug).
        if let Some(thinking) = delta
            .get("reasoning")
            .or_else(|| delta.get("reasoning_content"))
            .and_then(|c| c.as_str())
        {
            self.pending
                .push_back(ProviderEvent::ThinkingDelta(thinking.to_string()));
        }
        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            self.pending
                .push_back(ProviderEvent::TextDelta(text.to_string()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for call in calls {
                let Some(index) = call.get("index").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let index = index as u32;
                let acc = self.tool_acc.entry(index).or_default();
                let mut partial = ToolCallDelta {
                    index,
                    id: None,
                    name: None,
                    arguments: None,
                };
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    acc.id = Some(id.to_string());
                    partial.id = Some(id.to_string());
                }
                let function = call.get("function");
                if let Some(name) = function
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                {
                    // `name` fragments accumulate (the provider may split
                    // the name across chunks); `id` is set once.
                    match &mut acc.name {
                        Some(existing) => existing.push_str(name),
                        None => acc.name = Some(name.to_string()),
                    }
                    partial.name = Some(name.to_string());
                }
                if let Some(args) = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(|a| a.as_str())
                {
                    acc.arguments.push_str(args);
                    partial.arguments = Some(args.to_string());
                }
                self.pending
                    .push_back(ProviderEvent::ToolCallDelta(partial));
            }
        }
        // A `finish_reason` closes the turn: flush the accumulated tool
        // calls first (the `AgentLoop` needs them before it sees the
        // `Done`), then the `Done` itself.
        if let Some(reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            self.flush_tool_calls();
            self.pending
                .push_back(ProviderEvent::Done(map_finish_reason(reason)));
            self.finished = true;
        }
    }

    /// Emit the accumulated tool calls as complete `ToolCall` events
    /// (dropping any index that never arrived with a `name`).
    fn flush_tool_calls(&mut self) {
        for (_index, acc) in std::mem::take(&mut self.tool_acc).into_iter() {
            let Some(name) = acc.name else {
                continue;
            };
            self.pending.push_back(ProviderEvent::ToolCall(ToolCall {
                id: acc.id.unwrap_or_default(),
                name,
                arguments: parse_arguments(&acc.arguments),
            }));
        }
    }
}

/// Map a wire `finish_reason` string to a [`FinishReason`] (an unknown
/// value is a normal stop — some providers use `content_filter`, …;
/// it is logged so a new provider-side value is diagnosable, not
/// silent).
fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolCalls,
        "length" => FinishReason::Length,
        "error" => FinishReason::Error,
        _ => {
            eprintln!("harness: unknown finish_reason {reason:?} — treating as a stop");
            FinishReason::Stop
        }
    }
}

/// Parse accumulated argument text as JSON (an empty payload becomes
/// `{}` — a tool with no arguments; a MALFORMED payload also becomes
/// `{}`, but is logged — a silent `{}` would make the tool run with
/// no arguments and hide the provider-side corruption).
fn parse_arguments(args: &str) -> Value {
    if args.trim().is_empty() {
        return Value::Object(Default::default());
    }
    match serde_json::from_str(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("harness: malformed tool-call arguments — using {{}}: {e}");
            Value::Object(Default::default())
        }
    }
}

impl Stream for SseStream {
    type Item = ProviderEvent;

    fn poll_next(self: Pin<&mut SseStream>, cx: &mut Context<'_>) -> Poll<Option<ProviderEvent>> {
        let this = self.get_mut();
        loop {
            if let Some(event) = this.pending.pop_front() {
                return Poll::Ready(Some(event));
            }
            if this.exhausted {
                return Poll::Ready(None);
            }
            match this.raw.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => this.push(&chunk),
                Poll::Ready(Some(Err(e))) => {
                    // A mid-stream transport failure is retryable (the
                    // `AgentLoop`'s `RetryPolicy` re-issues `complete()`).
                    this.exhausted = true;
                    this.pending
                        .push_back(ProviderEvent::Error(ProviderError::Retryable(format!(
                            "stream error: {e}"
                        ))));
                }
                Poll::Ready(None) => {
                    this.exhausted = true;
                    // The stream ended: process the last (un-`\n`ed) line,
                    // flush any tool calls, and synthesize a `Done` if the
                    // provider never sent a `finish_reason`. (A tail that
                    // ends mid-codepoint is truncated on the wire — the
                    // lossy conversion is the only option there.)
                    let tail = String::from_utf8_lossy(&this.buf).into_owned();
                    if !tail.is_empty() {
                        this.handle_line(&tail);
                    }
                    if !this.finished {
                        this.flush_tool_calls();
                        this.pending
                            .push_back(ProviderEvent::Done(FinishReason::Error));
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    async fn complete(
        &self,
        req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        // The request-level bounds (finding 4 — an IDLE-based bound, NOT a
        // TOTAL request bound): a `reqwest` `.timeout()` applies "from when
        // the request starts connecting until the response body has
        // finished" — it kills HEALTHY slow streams too (a legitimately
        // long streaming call — e.g. a 4k-token compaction summary at
        // 5-10 tok/s on a slow local OpenAI-compatible endpoint, the
        // harness's target deployment per ADR 0012 ≈ 7-14 min — is cut
        // mid-stream → `Retryable` → re-issued from scratch up to 5× →
        // compaction/turn permanently fails). Instead:
        // - `connect_timeout`: a generous CONNECT bound (the TCP connect +
        //   TLS handshake — a stalled connect ERRORS `Retryable`, it does
        //   not hang the turn until the user Stops).
        // - `read_timeout`: the time BETWEEN BYTES on the body (5 min of
        //   SILENCE = a stalled provider ERRORS `Retryable` — the
        //   `RetryPolicy`'s backoff; a healthy slow stream that dribbles
        //   bytes keeps flowing, no matter how long it runs).
        let client = build_client(
            Duration::from_secs(30),     // a generous CONNECT bound
            Duration::from_secs(5 * 60), // 5 min of SILENCE = a stalled provider
        );
        let resp = client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request_body(req))
            .send()
            .await
            .map_err(|e| ProviderError::Retryable(format!("request failed: {e}")))?;

        // Map the HTTP status to a `ProviderError` BEFORE reading the body
        // (a non-2xx body is an error payload, not an SSE stream):
        // 401/403 → `Auth`, 429/5xx → `Retryable`, other 4xx → `Fatal`.
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Auth(format!("status {status}: {body}")));
        }
        if status.as_u16() == 429 || status.is_server_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Retryable(format!("status {status}: {body}")));
        }
        if status.is_client_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Fatal(format!("status {status}: {body}")));
        }
        if !status.is_success() {
            return Err(ProviderError::Fatal(format!("status {status}")));
        }

        // `bytes_stream` yields `bytes::Bytes`; map to `Vec<u8>` so the
        // stream type does not name the `bytes` crate.
        let raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>> =
            resp.bytes_stream().map(|r| r.map(|b| b.to_vec())).boxed();
        Ok(Box::pin(SseStream::new(raw)))
    }
}

/// Build the `reqwest` client for a model call (finding 4 — the IDLE-based
/// bounds: a generous `connect_timeout` + a `read_timeout` (the time BETWEEN
/// BYTES on the body — a stalled provider errors `Retryable`, a healthy slow
/// stream that dribbles bytes keeps flowing). Extracted so the tests can
/// inject a short `read_timeout` (the production 5 min is too slow to test).
fn build_client(connect_timeout: Duration, read_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .build()
        .expect("the reqwest client builds (fixed connect + read timeouts)")
}

/// Build the OpenAI-compatible request body from a [`ModelRequest`]
/// (`stream: true` is always sent; `None` options are omitted). The
/// message `content` goes through the OpenAI WIRE form
/// ([`MessageContent::to_wire`]) — a plain string for `Text`, an array
/// of OpenAI content parts (`text` / `image_url` data-URIs) for
/// `Blocks` — and an assistant `tool_calls` is serialized in the
/// OpenAI shape (`{ id, type: "function", function: { name, arguments:
/// <string> } }`).
fn request_body(req: &ModelRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            // The content is the OpenAI WIRE form (`MessageContent::to_wire`
            // — `Text` → a string, `Blocks` → an array of OpenAI content
            // parts with `image_url` data-URIs), NOT the `ContentBlock`'s
            // own pi-shaped serialization (the DB / transcript / tool-result
            // shape). The remaining fields keep their wire shape.
            let mut obj = serde_json::Map::new();
            obj.insert(
                "role".to_string(),
                serde_json::to_value(m.role).expect("role serialization cannot fail"),
            );
            obj.insert("content".to_string(), m.content.to_wire());
            if let Some(id) = &m.tool_call_id {
                obj.insert("tool_call_id".to_string(), Value::String(id.clone()));
            }
            if let Some(calls) = &m.tool_calls {
                obj.insert(
                    "tool_calls".to_string(),
                    Value::Array(
                        calls
                            .iter()
                            .map(|tc| {
                                serde_json::to_value(WireToolCall {
                                    id: &tc.id,
                                    name: &tc.name,
                                    arguments: &tc.arguments,
                                })
                                .expect("JSON serialization cannot fail")
                            })
                            .collect(),
                    ),
                );
            }
            Value::Object(obj)
        })
        .collect();
    let mut body = json!({
        "model": req.model,
        "messages": Value::Array(messages),
        "stream": req.options.stream,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                })
            })
            .collect::<Vec<_>>()
            .into();
    }
    if let Some(t) = req.options.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(m) = req.options.max_tokens {
        body["max_tokens"] = json!(m);
    }
    if let Some(r) = &req.options.reasoning_effort {
        body["reasoning_effort"] = json!(r);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::ImageRef;

    /// Build an `SseStream` over RAW byte chunks (the `&str` helper above
    /// cannot split a multi-byte codepoint mid-byte — `&str` is valid
    /// UTF-8; this one can).
    fn sse_stream_bytes(chunks: &[Vec<u8>]) -> SseStream {
        let data: Vec<Vec<u8>> = chunks.to_vec();
        let raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>> =
            futures_util::stream::iter(data.into_iter().map(Ok)).boxed();
        SseStream::new(raw)
    }

    /// Build an `SseStream` over canned raw chunks (no HTTP).
    fn sse_stream(chunks: &[&str]) -> SseStream {
        // Owned data so the stream is `'static` (independent of `chunks`).
        let data: Vec<Vec<u8>> = chunks.iter().map(|c| c.as_bytes().to_vec()).collect();
        sse_stream_bytes(&data)
    }

    /// Collect the whole stream.
    async fn collect(s: SseStream) -> Vec<ProviderEvent> {
        s.collect().await
    }

    // ── object safety / the seam ───────────────────────────────────────

    /// The `Provider` trait is object-safe (a `Box<dyn Provider>` compiles)
    /// so a second provider (Anthropic) can be added later.
    #[test]
    fn provider_trait_is_object_safe() {
        let provider: Box<dyn Provider> = Box::new(OpenAiCompatibleProvider {
            base_url: "https://example.com/v1".to_string(),
            api_key: "k".to_string(),
        });
        let _ = provider;
    }

    // ── SSE parsing ────────────────────────────────────────────────────

    #[tokio::test]
    async fn sse_content_and_finish_reason() {
        let events = collect(sse_stream(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
             data: [DONE]\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("He".to_string()),
                ProviderEvent::TextDelta("llo".to_string()),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[tokio::test]
    async fn sse_reasoning_deltas() {
        let events = collect(sse_stream(&[
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::ThinkingDelta("think".to_string()),
                ProviderEvent::TextDelta("done".to_string()),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    /// A single chunk can carry BOTH a `reasoning` fragment and a
    /// `content` fragment (some providers batch the thinking's tail with
    /// the text's head — the transition chunk). The thinking block
    /// PRECEDES the text block in the model's output, so the
    /// `ThinkingDelta` must be emitted BEFORE the `TextDelta` — the
    /// reverse order makes the frontend start the text block first and
    /// file the thinking's tail under a SECOND thinking block (the
    /// "missing last word" bug).
    #[tokio::test]
    async fn sse_chunk_with_reasoning_and_content_orders_thinking_first() {
        let events = collect(sse_stream(&[
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Fedora.\",\"content\":\"This project\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\" builds\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::ThinkingDelta("Fedora.".to_string()),
                ProviderEvent::TextDelta("This project".to_string()),
                ProviderEvent::TextDelta(" builds".to_string()),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[tokio::test]
    async fn sse_tool_call_fragments_accumulate() {
        // `name` and `arguments` split across chunks (as the real API
        // streams them); the `id` arrives only in the first fragment.
        let events = collect(sse_stream(&[
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"re\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"ad\",\"arguments\":\"{\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"path\\\": \\\"/x\\\"}\"}}]}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        ]))
        .await;
        // Three `ToolCallDelta`s, then the accumulated `ToolCall`, then
        // `Done(ToolCalls)` — the `ToolCall` precedes the `Done`.
        assert_eq!(
            events,
            vec![
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    name: Some("re".to_string()),
                    arguments: None,
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    id: None,
                    name: Some("ad".to_string()),
                    arguments: Some("{".to_string()),
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: Some("\"path\": \"/x\"}".to_string()),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "/x" }),
                }),
                ProviderEvent::Done(FinishReason::ToolCalls),
            ]
        );
    }

    #[tokio::test]
    async fn sse_usage_chunk() {
        let events = collect(sse_stream(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n\
             data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n\
             data: [DONE]\n\n",
        ]))
        .await;
        // No `finish_reason` on the wire: a synthetic `Done(Error)` closes
        // the stream (the `Usage` still arrives).
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("x".to_string()),
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                }),
                ProviderEvent::Done(FinishReason::Error),
            ]
        );
    }

    #[tokio::test]
    async fn sse_chunks_split_mid_line() {
        // The raw bytes may split a `data:` line anywhere — the line
        // buffer must reassemble it.
        let events = collect(sse_stream(&[
            "data: {\"choices\":[{\"de",
            "lta\":{\"content\":\"ab\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"fin",
            "ish_reason\":\"length\"}]}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("ab".to_string()),
                ProviderEvent::Done(FinishReason::Length),
            ]
        );
    }

    /// (finding 9) The raw bytes may split a MULTI-BYTE codepoint between
    /// chunks (CJK is 3 bytes, emoji is 4). A per-chunk `from_utf8_lossy`
    /// would replace each half with U+FFFD BEFORE line assembly, corrupting
    /// the reassembled text. The buffer must hold RAW bytes and convert
    /// only COMPLETE lines: the reassembled text is byte-identical at
    /// every possible split point.
    #[tokio::test]
    async fn sse_multibyte_chars_survive_chunk_boundaries() {
        // CJK (3-byte) + emoji (4-byte) content.
        let text = "你好🌍世界🚀";
        let payload =
            format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n");
        let bytes = payload.as_bytes();
        // Split the raw bytes at EVERY offset — including the middle of a
        // 3-byte and a 4-byte codepoint. Each reassembly must be
        // byte-identical (no U+FFFD).
        for split_at in 1..bytes.len() {
            let events = collect(sse_stream_bytes(&[
                bytes[..split_at].to_vec(),
                bytes[split_at..].to_vec(),
            ]))
            .await;
            let reassembled: String = events
                .iter()
                .filter_map(|e| match e {
                    ProviderEvent::TextDelta(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                reassembled, text,
                "split at byte {split_at} corrupts the text (got {reassembled:?})"
            );
            assert!(
                !reassembled.contains('\u{fffd}'),
                "split at byte {split_at} yields a replacement character"
            );
        }
        // A 4-byte emoji split across THREE chunks (1+1+2 bytes) — the
        // worst case: two codepoint boundaries inside one codepoint.
        let eb = bytes;
        let start = eb
            .iter()
            .position(|&b| b == 0xF0)
            .expect("the emoji's lead byte is present");
        let events = collect(sse_stream_bytes(&[
            eb[..start + 1].to_vec(),
            eb[start + 1..start + 2].to_vec(),
            eb[start + 2..].to_vec(),
        ]))
        .await;
        let reassembled: String = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reassembled, text, "the 1+1+2 split reassembles identically");
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_finish_reason("stop"), FinishReason::Stop);
        assert_eq!(map_finish_reason("tool_calls"), FinishReason::ToolCalls);
        assert_eq!(map_finish_reason("length"), FinishReason::Length);
        assert_eq!(map_finish_reason("error"), FinishReason::Error);
        // An unknown value is a normal stop.
        assert_eq!(map_finish_reason("content_filter"), FinishReason::Stop);
    }

    #[test]
    fn parse_arguments_handles_empty_and_malformed() {
        assert_eq!(parse_arguments(""), Value::Object(Default::default()));
        assert_eq!(parse_arguments("  "), Value::Object(Default::default()));
        assert_eq!(
            parse_arguments("{not json"),
            Value::Object(Default::default())
        );
        assert_eq!(parse_arguments(r#"{"a":1}"#), json!({ "a": 1 }));
    }

    /// A wire `usage` token count that exceeds `u32::MAX` SATURATES
    /// (a silent `as u32` would truncate — 4 GB of tokens would wrap to
    /// a small count).
    #[tokio::test]
    async fn sse_usage_saturates_on_u32_overflow() {
        let events = collect(sse_stream(&[
            &format!(
                "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{},\"completion_tokens\":5}}}}\n\n",
                u32::MAX as u64 + 1
            ),
        ]))
        .await;
        match &events[0] {
            ProviderEvent::Usage(u) => {
                assert_eq!(u.input_tokens, u32::MAX, "saturated, not truncated");
                assert_eq!(u.output_tokens, 5);
            }
            other => panic!("expected a Usage event, got {other:?}"),
        }
    }

    // ── request-body mapping ───────────────────────────────────────────

    #[test]
    fn request_body_maps_the_request() {
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![
                ChatMessage {
                    role: ChatRole::System,
                    content: MessageContent::Text("sys".to_string()),
                    tool_call_id: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: MessageContent::Text(String::new()),
                    tool_call_id: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "read".to_string(),
                        arguments: json!({ "path": "/x" }),
                    }]),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    content: MessageContent::Blocks(vec![ContentBlock::Text {
                        text: "file".to_string(),
                    }]),
                    tool_call_id: Some("call_1".to_string()),
                    tool_calls: None,
                },
            ],
            tools: vec![ToolSpec {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({ "type": "object" }),
            }],
            options: ModelOptions {
                temperature: Some(0.2),
                max_tokens: Some(64),
                reasoning_effort: Some("high".to_string()),
                stream: true,
            },
        };
        let body = request_body(&req);
        assert_eq!(body["model"], "m");
        assert_eq!(body["stream"], true);
        let temperature = body["temperature"]
            .as_f64()
            .expect("temperature is a number");
        assert!(
            (temperature - 0.2).abs() < 1e-6,
            "temperature is passed through (got {temperature})"
        );
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        // The message roles + the tool-result `tool_call_id`.
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
        // `Blocks` content serializes as an array of OpenAI parts (the
        // `to_wire` shape — `ContentBlock::Text` verbatim).
        assert_eq!(body["messages"][2]["content"][0]["type"], "text");
    }

    /// The model-request wire form is the OpenAI shape: `Text` → a plain
    /// string; `Blocks` → an array of content parts — a `Text` block
    /// verbatim, an `Image` block as an `image_url` **data-URI** (the
    /// pi-shaped `data` (base64, no prefix) + `mimeType` fields composed
    /// into `data:<mime>;base64,<data>` — the model never sees the
    /// pi-shaped `image` block).
    #[test]
    fn to_wire_is_the_openai_request_shape() {
        // `Text` → a plain string.
        assert_eq!(
            MessageContent::Text("hello".to_string()).to_wire(),
            json!("hello")
        );
        // `Blocks` → an array of OpenAI content parts.
        let blocks = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "before".to_string(),
            },
            ContentBlock::Image {
                image: ImageRef {
                    data: "BASE64DATA".to_string(),
                    mime_type: "image/png".to_string(),
                },
            },
        ]);
        assert_eq!(
            blocks.to_wire(),
            json!([
                { "type": "text", "text": "before" },
                {
                    "type": "image_url",
                    "image_url": { "url": "data:image/png;base64,BASE64DATA" }
                }
            ])
        );
    }

    /// The assistant `tool_calls` go on the wire in the OpenAI shape:
    /// `{ id, type: "function", function: { name, arguments: <JSON string> } }`
    /// — nested `function`, a `type` tag, and `arguments` as a STRING (a
    /// non-conformant flat shape is a 400 at the second model call of
    /// every tool-using turn).
    #[test]
    fn request_body_serializes_tool_calls_in_the_openai_wire_shape() {
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: MessageContent::Text(String::new()),
                    tool_call_id: None,
                    tool_calls: Some(vec![
                        ToolCall {
                            id: "call_1".to_string(),
                            name: "read".to_string(),
                            arguments: json!({ "path": "/x", "limit": 10 }),
                        },
                        ToolCall {
                            id: "call_2".to_string(),
                            name: "bash".to_string(),
                            arguments: Value::Object(Default::default()),
                        },
                    ]),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    content: MessageContent::Text("out".to_string()),
                    tool_call_id: Some("call_1".to_string()),
                    tool_calls: None,
                },
            ],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                stream: true,
            },
        };
        let body = request_body(&req);
        let calls = body["messages"][0]["tool_calls"]
            .as_array()
            .expect("assistant tool_calls are present");
        // `call_1`: nested `function`, a `type` tag, `arguments` as a
        // JSON **string** (not an object).
        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "read");
        assert_eq!(
            calls[0]["function"]["arguments"],
            "{\"limit\":10,\"path\":\"/x\"}"
        );
        // `call_2`: an empty-arguments call serializes to `"{}"`.
        assert_eq!(calls[1]["id"], "call_2");
        assert_eq!(calls[1]["type"], "function");
        assert_eq!(calls[1]["function"]["name"], "bash");
        assert_eq!(calls[1]["function"]["arguments"], "{}");
        // The `tool` message keeps its `tool_call_id` (no `tool_calls`).
        assert_eq!(body["messages"][1]["tool_call_id"], "call_1");
        assert!(body["messages"][1].get("tool_calls").is_none());
    }

    /// The display wire keeps the FLAT `ToolCall` shape (`{ id, name,
    /// arguments }` — the `toolcall_end` / `tool_execution_start` events):
    /// the OpenAI wire shape is request-side only.
    #[test]
    fn tool_call_display_wire_stays_flat() {
        let tc = ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: json!({ "path": "/x" }),
        };
        let v = serde_json::to_value(&tc).expect("serialization cannot fail");
        assert_eq!(
            v,
            json!({ "id": "call_1", "name": "read", "arguments": { "path": "/x" } })
        );
        assert!(
            v.get("function").is_none(),
            "no nested `function` on the display wire"
        );
        assert!(v.get("type").is_none(), "no `type` tag on the display wire");
    }

    #[test]
    fn request_body_omits_none_options_and_empty_tools() {
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                stream: true,
            },
        };
        let body = request_body(&req);
        assert!(body.get("tools").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    // ── the idle-based `read_timeout` (finding 4) ────────────────────

    /// A raw HTTP server: reads the request, writes the `200` headers, then
    /// either DRIBBLES a body byte every 50 ms (a healthy slow stream) or
    /// stays SILENT (a stalled one). The `read_timeout` applies to the time
    /// BETWEEN BYTES on the body, so a dribble under the timeout completes
    /// and a silence over it errors.
    async fn raw_http_server(
        listener: tokio::net::TcpListener,
        dribble: bool,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let mut data = Vec::new();
            // Read the request until the headers end (`\r\n\r\n`).
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                data.extend_from_slice(&buf[..n]);
            }
            if stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            if dribble {
                // Dribble a body byte every 50 ms for 2 s (a healthy slow
                // stream — each byte is well under the 200 ms `read_timeout`).
                for _ in 0..40 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if stream.write_all(b"x").await.is_err() {
                        return;
                    }
                }
            } else {
                // Silent: HOLD the connection open (no body bytes — the
                // `read_timeout` fires before this sleep ends; the test
                // aborts the task so it does not wait the full duration).
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            // Close (the stream is dropped when the function returns).
        })
    }

    /// (finding 4) A stream that DRIBBLES a byte every 50 ms (under the
    /// `read_timeout`) COMPLETES (a healthy slow stream is not cut — the
    /// pre-fix `.timeout()` was a TOTAL request bound that killed it), while
    /// a fully SILENT stream ERRORS (a stalled provider — the `read_timeout`
    /// fires). A short `read_timeout` (200 ms) is injected via `build_client`
    /// (the production 5 min is too slow to test).
    #[tokio::test]
    async fn a_slow_stream_completes_but_a_silent_stream_errors() {
        // Consume a `bytes_stream`, returning `(bytes, error)`: `error` is
        // `Some` if a chunk failed (the `read_timeout`), `None` if the stream
        // completed (EOF).
        async fn drain(resp: reqwest::Response) -> (Vec<u8>, Option<reqwest::Error>) {
            use futures_util::StreamExt;
            let mut stream = resp.bytes_stream();
            let mut body = Vec::new();
            let mut error: Option<reqwest::Error> = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(b) => body.extend_from_slice(&b),
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            (body, error)
        }

        // (1) Dribbling: a body byte every 50 ms for 2 s (under the 200 ms
        //     `read_timeout`). The stream COMPLETES (40 bytes — not cut).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the listener binds");
        let addr = listener.local_addr().expect("the listener has an address");
        let server = raw_http_server(listener, true).await;
        let client = build_client(Duration::from_secs(30), Duration::from_millis(200));
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .send()
            .await
            .expect("the request sends");
        let (body, error) = drain(resp).await;
        assert!(
            error.is_none(),
            "the slow (dribbling) stream completes — it is NOT cut, got {error:?}"
        );
        assert_eq!(body.len(), 40, "all 40 dribbled bytes were received");
        let _ = server.await;

        // (2) Silent: no body bytes (over the 200 ms `read_timeout`). The
        //     stream ERRORS (a stalled provider — the `read_timeout` fires).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the listener binds");
        let addr = listener.local_addr().expect("the listener has an address");
        let server = raw_http_server(listener, false).await;
        let client = build_client(Duration::from_secs(30), Duration::from_millis(200));
        let resp = client
            .post(format!("http://{addr}/chat/completions"))
            .send()
            .await
            .expect("the request sends");
        let (_body, error) = drain(resp).await;
        assert!(
            error.is_some(),
            "the silent stream errors — the `read_timeout` fired"
        );
        server.abort(); // the server holds the connection open (the sleep)
    }
}
