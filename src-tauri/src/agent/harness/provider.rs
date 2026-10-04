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
    /// The session id for LiteLLM session / request tracking (if known).
    pub session_id: Option<String>,
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

/// Assembles raw response bytes into complete `\n`-terminated lines
/// (shared by ALL the SSE parsers — the OpenAI [`SseStream`] and the two
/// `event:`-tagged ones, [`AnthropicStream`] / [`ResponsesStream`]). The
/// buffer holds RAW BYTES (a network chunk may split a multi-byte
/// codepoint; a per-chunk `from_utf8_lossy` would replace each half with
/// U+FFFD before line assembly — only COMPLETE lines are converted;
/// finding 9).
struct LineAssembler {
    /// The not-yet-`\n`-terminated tail of the response (RAW bytes — an
    /// incomplete trailing multi-byte sequence is held back between
    /// chunks and only converted once its line is complete).
    buf: Vec<u8>,
}

impl LineAssembler {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append a raw chunk; return every complete line it contains.
    ///
    /// (`drain(..=pos)`, NOT `split_off`: `split_off(at)` returns the
    /// portion AFTER `at` and keeps the before-portion in `self` — the
    /// exact opposite of what is needed here.)
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }

    /// The not-yet-`\n`-terminated tail (a stream end — a tail that ends
    /// mid-codepoint is truncated on the wire; the lossy conversion is
    /// the only option there). `None` when nothing is buffered.
    fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let line: Vec<u8> = self.buf.drain(..).collect();
        Some(String::from_utf8_lossy(&line).into_owned())
    }
}

/// The SSE line/chunk parser: turns raw response bytes into
/// [`ProviderEvent`]s (content / reasoning / tool-call deltas,
/// `finish_reason` → `Done`, `usage` → `Usage`).
struct SseStream {
    /// The raw byte stream (`reqwest::bytes_stream`, mapped to `Vec<u8>`).
    raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    /// The raw-bytes line assembler (the multi-byte-codepoint safety —
    /// finding 9).
    lines: LineAssembler,
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
            lines: LineAssembler::new(),
            pending: VecDeque::new(),
            tool_acc: BTreeMap::new(),
            finished: false,
            exhausted: false,
        }
    }

    /// Append a raw chunk; process every complete line it contains.
    fn push(&mut self, chunk: &[u8]) {
        for line in self.lines.push(chunk) {
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

/// Map an Anthropic `stop_reason` to a [`FinishReason`].
fn map_anthropic_stop_reason(reason: &str) -> FinishReason {
    match reason {
        "end_turn" => FinishReason::Stop,
        "tool_use" => FinishReason::ToolCalls,
        "max_tokens" => FinishReason::Length,
        // `stop_sequence` (a custom stop sequence hit) is a normal stop.
        "stop_sequence" => FinishReason::Stop,
        _ => {
            eprintln!("harness: unknown anthropic stop_reason {reason:?} — treating as a stop");
            FinishReason::Stop
        }
    }
}

/// Normalize an Anthropic-wire base URL to include the `/v1` API prefix
/// (ADR 0024 — mirroring ZCode's `normalizeAnthropicBaseURL`, applied at
/// the adapter boundary). The Anthropic client appends `/messages` (and
/// discovery appends `/models`) to the base, so a GATEWAY ROOT that does
/// not already end in `/v1` gets it appended: `https://openrouter.ai/api`
/// → `…/api/v1`. Idempotent (a base whose path already ends in `/v1` is
/// left as-is, trailing slash dropped).
///
/// Operates on the URL PATH (not the full string, so a query string is
/// not corrupted — ZCode's exact behavior); an unparseable base falls
/// back to a plain suffix check (ZCode's `catch` arm).
pub fn normalize_anthropic_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(mut url) = url::Url::parse(trimmed) {
        // An OWNED `String` (NOT a `&str` borrowing from `url` — the
        // `set_path` below mutates `url`, so the borrow must not outlive it).
        let pathname = url.path().trim_end_matches('/').to_string();
        if pathname.to_lowercase().ends_with("/v1") {
            url.set_path(&pathname); // drop any trailing slash
            return url.to_string();
        }
        let with_v1 = format!("{pathname}/v1");
        url.set_path(&with_v1);
        return url.to_string();
    }
    // Fallback (unparseable): a plain suffix check on the trimmed string.
    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.to_lowercase().ends_with("/v1") {
        without_trailing.to_string()
    } else {
        format!("{without_trailing}/v1")
    }
}

/// Map an Anthropic `error` event's `type` to a [`ProviderError`] (the
/// wire `type` is the classification: `overloaded_error` / `api_error`
/// are transient, `authentication_error` / `permission_error` are auth,
/// the rest are permanent; an UNKNOWN type is conservative — retryable,
/// a new error type is probably transient, and the `RetryPolicy`'s
/// budget bounds the damage).
fn map_anthropic_error(v: &Value) -> ProviderError {
    let message = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("(no message)");
    let kind = v
        .get("error")
        .and_then(|e| e.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    match kind {
        "overloaded_error" | "api_error" => ProviderError::Retryable(format!("{kind}: {message}")),
        "authentication_error" | "permission_error" => {
            ProviderError::Auth(format!("{kind}: {message}"))
        }
        "invalid_request_error" | "not_found_error" => {
            ProviderError::Fatal(format!("{kind}: {message}"))
        }
        _ => {
            let label = if kind.is_empty() { "unknown" } else { kind };
            ProviderError::Retryable(format!("{label}: {message}"))
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
                    if let Some(tail) = this.lines.finish() {
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

/// The accumulated per-block tool-call state for the Anthropic wire (a
/// `tool_use` content block: the `id` + `name` arrive at
/// `content_block_start`, the `input` JSON text accumulates as
/// `input_json_delta` fragments).
#[derive(Default, Clone)]
struct AnthropicToolAcc {
    id: Option<String>,
    name: Option<String>,
    /// The argument fragments concatenated (JSON text).
    arguments: String,
}

/// The Anthropic Messages SSE parser: turns raw response bytes into
/// [`ProviderEvent`]s. The wire is `event:`-TAGGED (every `data:` line
/// is preceded by an `event: <name>` line — the OpenAI wire carries no
/// `event:` tags, so this parser tracks the current event name and
/// dispatches on it): `message_start` (the `usage.input_tokens`),
/// `content_block_start` (a `text` / `thinking` / `tool_use` block —
/// the `tool_use` block carries the `id` + `name`), `content_block_delta`
/// (`text_delta` / `thinking_delta` / `input_json_delta` fragments),
/// `content_block_stop`, `message_delta` (the `stop_reason` + the
/// `usage.output_tokens`), `message_stop`, `error`.
struct AnthropicStream {
    /// The raw byte stream (`reqwest::bytes_stream`, mapped to `Vec<u8>`).
    raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    /// The raw-bytes line assembler (the multi-byte-codepoint safety —
    /// finding 9).
    lines: LineAssembler,
    /// Events parsed but not yet delivered.
    pending: VecDeque<ProviderEvent>,
    /// The current SSE `event:` name (a `data:` line without one is
    /// dispatched on its `type` field instead — the Anthropic wire
    /// always tags its events, but a proxy may strip the tags; every
    /// Anthropic event carries a `type` matching its event name).
    event: Option<String>,
    /// Per-block tool-call accumulation (the `content_block.index` of a
    /// `tool_use` block; flushed as `ToolCall`s at the `message_delta`
    /// `stop_reason` / stream end).
    tool_acc: BTreeMap<u32, AnthropicToolAcc>,
    /// The `message_start` input tokens (the `usage` there carries the
    /// INPUT count only; the output comes from `message_delta`).
    input_tokens: Option<u32>,
    /// The last `message_delta` `usage.output_tokens` seen (cumulative on
    /// the wire — the FINAL `message_delta` carries the full count).
    last_output_tokens: Option<u32>,
    /// A `message_delta` with a `stop_reason` was seen (no synthetic
    /// `Done` at end).
    finished: bool,
    /// A combined `Usage` was emitted (exactly once — the `message_delta`
    /// `stop_reason` event, or the stream end when it never arrived).
    usage_emitted: bool,
    /// The raw stream is exhausted (the stream ends after `pending`).
    exhausted: bool,
}

impl AnthropicStream {
    fn new(raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>) -> Self {
        Self {
            raw,
            lines: LineAssembler::new(),
            pending: VecDeque::new(),
            event: None,
            tool_acc: BTreeMap::new(),
            input_tokens: None,
            last_output_tokens: None,
            finished: false,
            usage_emitted: false,
            exhausted: false,
        }
    }

    /// Append a raw chunk; process every complete line it contains.
    fn push(&mut self, chunk: &[u8]) {
        for line in self.lines.push(chunk) {
            self.handle_line(&line);
        }
    }

    /// Handle one line (an `event:` line sets the current event name —
    /// it PRECEDES its `data:` line; a `data:` line dispatches on it.
    /// `:`-comment lines and other fields are ignored.)
    fn handle_line(&mut self, line: &str) {
        let line = line.trim();
        if let Some(event) = line.strip_prefix("event:") {
            let event = event.trim();
            if !event.is_empty() {
                self.event = Some(event.to_string());
            }
            return;
        }
        let Some(payload) = line.strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        self.handle_chunk(&v);
    }

    /// Handle one parsed event (the `event:` tag is the discriminator;
    /// the `type` field is the fallback when the tag is absent — every
    /// Anthropic event carries a `type` matching its event name).
    fn handle_chunk(&mut self, v: &Value) {
        let kind = self
            .event
            .as_deref()
            .or_else(|| v.get("type").and_then(|t| t.as_str()));
        match kind {
            Some("message_start") => {
                if let Some(t) = v
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|t| t.as_u64())
                {
                    self.input_tokens = Some(u32::try_from(t).unwrap_or(u32::MAX));
                }
            }
            Some("content_block_start") => {
                let Some(index) = v.get("index").and_then(|i| i.as_u64()).map(|i| i as u32) else {
                    return;
                };
                let block = v.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(|t| t.as_str()) == Some("tool_use") {
                    let mut acc = AnthropicToolAcc::default();
                    if let Some(id) = block.and_then(|b| b.get("id")).and_then(|i| i.as_str()) {
                        acc.id = Some(id.to_string());
                    }
                    if let Some(name) = block.and_then(|b| b.get("name")).and_then(|n| n.as_str()) {
                        acc.name = Some(name.to_string());
                    }
                    self.tool_acc.insert(index, acc.clone());
                    // The `AgentLoop` renders the tool call as soon as its
                    // `name` is known (the `input` JSON still streams) —
                    // the `ToolCallDelta` carries the `id` + `name`.
                    self.pending
                        .push_back(ProviderEvent::ToolCallDelta(ToolCallDelta {
                            index,
                            id: acc.id.clone(),
                            name: acc.name.clone(),
                            arguments: None,
                        }));
                }
                // `text` / `thinking` blocks: their deltas carry the
                // fragments — nothing to record at `start`.
            }
            Some("content_block_delta") => {
                let Some(index) = v.get("index").and_then(|i| i.as_u64()).map(|i| i as u32) else {
                    return;
                };
                let delta = v.get("delta");
                match delta.and_then(|d| d.get("type")).and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(text) =
                            delta.and_then(|d| d.get("text")).and_then(|t| t.as_str())
                        {
                            self.pending
                                .push_back(ProviderEvent::TextDelta(text.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(thinking) = delta
                            .and_then(|d| d.get("thinking"))
                            .and_then(|t| t.as_str())
                        {
                            self.pending
                                .push_back(ProviderEvent::ThinkingDelta(thinking.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(fragment) = delta
                            .and_then(|d| d.get("partial_json"))
                            .and_then(|t| t.as_str())
                        {
                            let acc = self.tool_acc.entry(index).or_default();
                            acc.arguments.push_str(fragment);
                            self.pending
                                .push_back(ProviderEvent::ToolCallDelta(ToolCallDelta {
                                    index,
                                    id: None,
                                    name: None,
                                    arguments: Some(fragment.to_string()),
                                }));
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                // The `usage.output_tokens` (cumulative ��� the FINAL
                // `message_delta` carries the full count; an intermediate
                // one's value is a lower bound, so the combined `Usage`
                // is emitted ONCE, at the `stop_reason` event / stream
                // end, from the LAST value seen).
                if let Some(t) = v
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|t| t.as_u64())
                {
                    self.last_output_tokens = Some(u32::try_from(t).unwrap_or(u32::MAX));
                }
                if let Some(reason) = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|r| r.as_str())
                {
                    self.emit_usage();
                    self.flush_tool_calls();
                    self.pending
                        .push_back(ProviderEvent::Done(map_anthropic_stop_reason(reason)));
                    self.finished = true;
                }
            }
            Some("error") => {
                // A mid-stream `error` event (the HTTP status was 200 —
                // the error is IN the stream). The `AgentLoop` records
                // it (a `Done` may still follow on a healthy stream end;
                // the error takes precedence in the loop's accounting).
                self.exhausted = true;
                self.pending
                    .push_back(ProviderEvent::Error(map_anthropic_error(v)));
            }
            _ => {
                // `message_stop` / `content_block_stop` / unknown: no
                // event to deliver (a `message_stop` after a
                // `message_delta` is the healthy end — the `Done` was
                // already emitted; a `message_stop` WITHOUT a
                // `message_delta` hits the stream-end synthetic `Done`).
            }
        }
        // Consume the `event:` tag (a `data:` line belongs to the
        // PRECEDING `event:` line; the next `event:` line starts a new
        // event — the tags and the `data:` lines alternate on the wire).
        self.event = None;
    }

    /// Emit the combined `Usage` ONCE (the input from `message_start` +
    /// the last `output_tokens` seen; a `None` side becomes `0` — an
    /// absent count is reported as zero, NOT dropped: a `Usage` with one
    /// real side is more useful than none).
    fn emit_usage(&mut self) {
        if self.usage_emitted {
            return;
        }
        if self.input_tokens.is_none() && self.last_output_tokens.is_none() {
            return;
        }
        self.usage_emitted = true;
        self.pending.push_back(ProviderEvent::Usage(Usage {
            input_tokens: self.input_tokens.unwrap_or(0),
            output_tokens: self.last_output_tokens.unwrap_or(0),
        }));
    }

    /// Emit the accumulated tool calls as complete `ToolCall` events
    /// (dropping any block that never arrived with a `name` — a
    /// `tool_use` block without a `name` cannot be dispatched).
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

impl Stream for AnthropicStream {
    type Item = ProviderEvent;

    fn poll_next(
        self: Pin<&mut AnthropicStream>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<ProviderEvent>> {
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
                    // emit any pending `Usage`, flush the tool calls, and
                    // synthesize a `Done` if the provider never sent a
                    // `stop_reason`. (A tail that ends mid-codepoint is
                    // truncated on the wire — the lossy conversion is the
                    // only option there.)
                    if let Some(tail) = this.lines.finish() {
                        this.handle_line(&tail);
                    }
                    this.emit_usage();
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

/// The accumulated per-item tool-call state for the OpenAI Responses
/// wire (a `function_call` output item: the `id` + `name` arrive at
/// `output_item.added`, the `arguments` JSON text accumulates as
/// `function_call_arguments.delta` fragments, and the
/// `output_item.done` item carries the COMPLETE `arguments` —
/// authoritative, overwriting the accumulated text).
#[derive(Default, Clone)]
struct ResponsesToolAcc {
    id: Option<String>,
    name: Option<String>,
    /// The argument fragments concatenated (JSON text — the
    /// `output_item.done` item OVERWRITES it with the complete value).
    arguments: String,
    /// The `output_item.done` item's `ToolCall` was emitted already (the
    /// stream-end flush skips `done` accs — no duplicate `ToolCall` — and
    /// a `done` acc suppresses the stream-end synthetic `Done(Error)`:
    /// the call is complete and actionable, so the `AgentLoop` continues
    /// the turn with it instead of retrying the model call).
    done: bool,
}

/// The OpenAI Responses SSE parser (ADR 0024): turns raw response bytes
/// into [`ProviderEvent`]s. The wire is `event:`-TAGGED like Anthropic
/// (every `data:` line is preceded by an `event: <name>` line), but the
/// event names + payload shapes are the Responses API's:
/// `response.output_text.delta` (the text fragments),
/// `response.reasoning_summary_text.delta` (the reasoning fragments),
/// `response.output_item.added` (a `function_call` item — the `id` +
/// `name`), `response.function_call_arguments.delta` (the `arguments`
/// fragments), `response.output_item.done` (the complete `function_call`
/// item — authoritative `arguments`), `response.completed` (the
/// `usage` and `status: "completed"`), `response.incomplete` (the `usage`
/// and `incomplete_details.reason`), `response.failed` (the `usage`), and
/// an in-stream `error` event (terminal).
struct ResponsesStream {
    /// The raw byte stream (`reqwest::bytes_stream`, mapped to `Vec<u8>`).
    raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    /// The raw-bytes line assembler (the multi-byte-codepoint safety —
    /// finding 9).
    lines: LineAssembler,
    /// Events parsed but not yet delivered.
    pending: VecDeque<ProviderEvent>,
    /// The current SSE `event:` name (a `data:` line without one is
    /// dispatched on its `type` field instead — the Responses events
    /// carry a `type` matching the event name).
    event: Option<String>,
    /// Per-item tool-call accumulation (the `output_index` of a
    /// `function_call` item; the `output_item.done` emits the `ToolCall`
    /// directly, the stream end flushes the not-`done` accs).
    tool_acc: BTreeMap<u32, ResponsesToolAcc>,
    /// A terminal event was seen (`response.completed` /
    /// `response.incomplete` / `response.failed` / `error` — no synthetic
    /// `Done` at stream end).
    terminated: bool,
    /// The raw stream is exhausted (the stream ends after `pending`).
    exhausted: bool,
}

impl ResponsesStream {
    fn new(raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>>) -> Self {
        Self {
            raw,
            lines: LineAssembler::new(),
            pending: VecDeque::new(),
            event: None,
            tool_acc: BTreeMap::new(),
            terminated: false,
            exhausted: false,
        }
    }

    /// Append a raw chunk; process every complete line it contains.
    fn push(&mut self, chunk: &[u8]) {
        for line in self.lines.push(chunk) {
            self.handle_line(&line);
        }
    }

    /// Handle one line (an `event:` line sets the current event name —
    /// it PRECEDES its `data:` line; a `data:` line dispatches on it.
    /// `:`-comment lines and other fields are ignored.)
    fn handle_line(&mut self, line: &str) {
        let line = line.trim();
        if let Some(event) = line.strip_prefix("event:") {
            let event = event.trim();
            if !event.is_empty() {
                self.event = Some(event.to_string());
            }
            return;
        }
        let Some(payload) = line.strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        self.handle_chunk(&v);
    }

    /// Handle one parsed event (the `event:` tag is the discriminator;
    /// the `type` field is the fallback when the tag is absent — every
    /// Responses event carries a `type` matching its event name).
    fn handle_chunk(&mut self, v: &Value) {
        let kind = self
            .event
            .as_deref()
            .or_else(|| v.get("type").and_then(|t| t.as_str()));
        match kind {
            Some("response.output_text.delta") => {
                if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                    self.pending
                        .push_back(ProviderEvent::TextDelta(delta.to_string()));
                }
            }
            Some("response.reasoning_summary_text.delta") => {
                if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                    self.pending
                        .push_back(ProviderEvent::ThinkingDelta(delta.to_string()));
                }
            }
            Some("response.output_item.added") => {
                let Some(item) = v.get("item") else {
                    return;
                };
                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    // The `output_index` keys the acc (`0` when absent —
                    // a well-formed stream always carries it).
                    let index = v
                        .get("output_index")
                        .and_then(|i| i.as_u64())
                        .map(|i| i as u32)
                        .unwrap_or(0);
                    let mut acc = ResponsesToolAcc::default();
                    if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                        acc.id = Some(id.to_string());
                    }
                    if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                        acc.name = Some(name.to_string());
                    }
                    // The `arguments` (a JSON string, usually `""`) seed
                    // the accumulator — the `output_item.done` item
                    // overwrites it with the complete value.
                    if let Some(args) = item.get("arguments").and_then(|a| a.as_str()) {
                        acc.arguments = args.to_string();
                    }
                    self.tool_acc.insert(index, acc.clone());
                    // The `AgentLoop` renders the tool call as soon as
                    // its `name` is known (the `arguments` JSON still
                    // streams) — the `ToolCallDelta` carries the `id` +
                    // `name` (+ the `arguments` when non-empty).
                    self.pending
                        .push_back(ProviderEvent::ToolCallDelta(ToolCallDelta {
                            index,
                            id: acc.id.clone(),
                            name: acc.name.clone(),
                            arguments: if acc.arguments.is_empty() {
                                None
                            } else {
                                Some(acc.arguments.clone())
                            },
                        }));
                }
                // `message` / `reasoning` items: their content streams via
                // the `*_delta` events — nothing to record at `added`.
            }
            Some("response.function_call_arguments.delta") => {
                let Some(index) = v
                    .get("output_index")
                    .and_then(|i| i.as_u64())
                    .map(|i| i as u32)
                else {
                    return;
                };
                let Some(fragment) = v.get("delta").and_then(|d| d.as_str()) else {
                    return;
                };
                self.tool_acc
                    .entry(index)
                    .or_default()
                    .arguments
                    .push_str(fragment);
                self.pending
                    .push_back(ProviderEvent::ToolCallDelta(ToolCallDelta {
                        index,
                        id: None,
                        name: None,
                        arguments: Some(fragment.to_string()),
                    }));
            }
            Some("response.output_item.done") => {
                let Some(item) = v.get("item") else {
                    return;
                };
                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    let index = v
                        .get("output_index")
                        .and_then(|i| i.as_u64())
                        .map(|i| i as u32)
                        .unwrap_or(0);
                    let acc = self.tool_acc.entry(index).or_default();
                    // The done item carries the COMPLETE `arguments` (a
                    // JSON string) — AUTHORITATIVE: overwrite the
                    // accumulated text (a proxy that sent no
                    // `arguments.delta` events still yields a complete
                    // call), and mark the acc `done` (the stream-end
                    // flush skips it — no duplicate `ToolCall`; and a
                    // `done` acc suppresses the stream-end synthetic
                    // `Done(Error)` — the call is complete and actionable,
                    // so the `AgentLoop` continues the turn with it
                    // instead of retrying the model call).
                    if let Some(args) = item.get("arguments").and_then(|a| a.as_str()) {
                        acc.arguments = args.to_string();
                    }
                    // `id` / `name` fall back to the acc's (a proxy may
                    // omit them on the done item); a call with NO `name`
                    // cannot be dispatched — skip the item entirely (the
                    // acc stays not-`done`: the item never completed a
                    // dispatchable call, so a stream end without a
                    // terminal event still gets the synthetic
                    // `Done(Error)`).
                    let id = item
                        .get("id")
                        .and_then(|i| i.as_str())
                        .map(String::from)
                        .or_else(|| acc.id.clone());
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(String::from)
                        .or_else(|| acc.name.clone());
                    if let Some(name) = name {
                        acc.done = true;
                        self.pending.push_back(ProviderEvent::ToolCall(ToolCall {
                            id: id.unwrap_or_default(),
                            name,
                            arguments: parse_arguments(&acc.arguments),
                        }));
                    }
                }
                // `message` / `reasoning` items: nothing (their content
                // already streamed via the `*_delta` events).
            }
            Some("response.completed") => {
                // The terminal event: the `usage` (when at least one side
                // is present) PRECEDES the `Done`.
                if let Some(usage) = self.response_usage(v) {
                    self.pending.push_back(ProviderEvent::Usage(usage));
                }
                self.terminated = true;
                if v.get("response")
                    .and_then(|r| r.get("status"))
                    .and_then(|s| s.as_str())
                    == Some("completed")
                {
                    self.pending
                        .push_back(ProviderEvent::Done(FinishReason::Stop));
                }
            }
            Some("response.incomplete") => {
                if let Some(usage) = self.response_usage(v) {
                    self.pending.push_back(ProviderEvent::Usage(usage));
                }
                self.terminated = true;
                // `max_output_tokens` → `Length`; any other reason (or an
                // absent one) → `Error`.
                let reason = v
                    .get("response")
                    .and_then(|r| r.get("incomplete_details"))
                    .and_then(|d| d.get("reason"))
                    .and_then(|r| r.as_str());
                let finish = if reason == Some("max_output_tokens") {
                    FinishReason::Length
                } else {
                    FinishReason::Error
                };
                self.pending.push_back(ProviderEvent::Done(finish));
            }
            Some("response.failed") => {
                if let Some(usage) = self.response_usage(v) {
                    self.pending.push_back(ProviderEvent::Usage(usage));
                }
                self.terminated = true;
                self.pending
                    .push_back(ProviderEvent::Done(FinishReason::Error));
            }
            Some("error") => {
                // An in-stream `error` event (the HTTP status was 200 —
                // the error is IN the stream). Terminal: the turn cannot
                // continue (deliberately simpler than the Anthropic
                // `error` mapping — the Responses `error` object has no
                // stable `type` taxonomy, and a mid-stream error after a
                // partial response must NOT be blindly retried: the
                // `AgentLoop` has already consumed the partial deltas,
                // so the `RetryPolicy` sees a non-retryable end).
                self.exhausted = true;
                self.terminated = true;
                self.pending
                    .push_back(ProviderEvent::Done(FinishReason::Error));
            }
            _ => {
                // `response.created` / `response.output_item.added`
                // (non-`function_call`) / unknown: no event to deliver.
            }
        }
        // Consume the `event:` tag (a `data:` line belongs to the
        // PRECEDING `event:` line; the next `event:` line starts a new
        // event — the tags and the `data:` lines alternate on the wire).
        self.event = None;
    }

    /// The `usage` of a terminal event's `response` object (the wire
    /// `u64` counts saturate to `u32::MAX — a silent `as u32` would
    /// TRUNCATE); a `None` side becomes `0`; `None` when NEITHER side is
    /// present (no `Usage` event — a zero-filled `Usage` would
    /// misreport).
    fn response_usage(&self, v: &Value) -> Option<Usage> {
        let usage = v.get("response")?.get("usage")?;
        let input = usage
            .get("input_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| u32::try_from(t).unwrap_or(u32::MAX));
        let output = usage
            .get("output_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| u32::try_from(t).unwrap_or(u32::MAX));
        match (input, output) {
            (Some(input), Some(output)) => Some(Usage {
                input_tokens: input,
                output_tokens: output,
            }),
            (Some(input), None) => Some(Usage {
                input_tokens: input,
                output_tokens: 0,
            }),
            (None, Some(output)) => Some(Usage {
                input_tokens: 0,
                output_tokens: output,
            }),
            (None, None) => None,
        }
    }

    /// Emit the accumulated tool calls as complete `ToolCall` events
    /// (dropping any acc that never arrived with a `name` — a
    /// `function_call` item without a `name` cannot be dispatched — and
    /// any `done` acc: its `output_item.done` already emitted the
    /// `ToolCall`, flushing it again would duplicate it).
    fn flush_tool_calls(&mut self) {
        for (_index, acc) in std::mem::take(&mut self.tool_acc).into_iter() {
            if acc.done {
                continue;
            }
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

impl Stream for ResponsesStream {
    type Item = ProviderEvent;

    fn poll_next(
        self: Pin<&mut ResponsesStream>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<ProviderEvent>> {
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
                    // flush any not-`done` tool calls, and synthesize a
                    // `Done` when no terminal event was seen. (A tail
                    // that ends mid-codepoint is truncated on the wire —
                    // the lossy conversion is the only option there.)
                    //
                    // NO `Usage` is synthesized here (unlike Anthropic —
                    // a Responses `usage` arrives only on the terminal
                    // events; a missing terminal event means missing
                    // usage, and a zero-filled `Usage` would misreport).
                    if let Some(tail) = this.lines.finish() {
                        this.handle_line(&tail);
                    }
                    if !this.terminated {
                        // A `done` acc (its `output_item.done` emitted a
                        // complete, actionable `ToolCall`) SUPPRESSES the
                        // synthetic `Done(Error)`: the `AgentLoop` sees a
                        // stream end without a `Done` and continues the
                        // turn with the tool call instead of retrying
                        // the model call (a `Done(Error)` would be
                        // retryable — the partial text would be dropped
                        // and the turn re-issued from scratch). A not-
                        // `done` acc (or none at all) means the response
                        // was cut off — the synthetic `Done(Error)`
                        // applies.
                        let completed = this.tool_acc.values().any(|acc| acc.done);
                        this.flush_tool_calls();
                        if !completed {
                            this.pending
                                .push_back(ProviderEvent::Done(FinishReason::Error));
                        }
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
        // harness's target deployment per ADR 0014 ≈ 7-14 min — is cut
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
        let mut builder = client.post(&url).bearer_auth(&self.api_key);
        if let Some(ref sid) = req.session_id {
            builder = builder
                .header("x-litellm-session-id", sid)
                .header("x-request-id", sid);
        }
        let resp = builder
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
///
/// The client also identifies itself: a `User-Agent` header ([`USER_AGENT`])
/// so the provider's logs can attribute the traffic to the app.
/// The `User-Agent` the model calls identify with (`archimedes/<version>` —
/// the harness is the desktop's own OpenAI-compatible client; `reqwest` sends
/// NO `User-Agent` by default, so without this the traffic is unattributable
/// in the provider's logs).
pub const USER_AGENT: &str = concat!("archimedes/", env!("CARGO_PKG_VERSION"));

fn build_client(connect_timeout: Duration, read_timeout: Duration) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(USER_AGENT),
    );
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .default_headers(headers)
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

/// The default `max_tokens` for the Anthropic wire (Anthropic REQUIRES
/// `max_tokens` on every request — a generous default when the caller did
/// not set one).
pub const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 32768;

/// A client for the Anthropic Messages API (`POST {base_url}/messages`,
/// the `event:`-tagged SSE response parsed by [`AnthropicStream`]; ADR
/// 0024). The auth header is `x-api-key` (NOT `Authorization: Bearer` —
/// the Anthropic API's header), plus the required `anthropic-version`.
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    /// The API base (e.g. `https://api.anthropic.com/v1` — the client
    /// appends `/messages`).
    pub base_url: String,
    /// The `x-api-key` header value.
    pub api_key: String,
}

/// The Anthropic WIRE shape of a content block: `Text` →
/// `{ "type": "text", "text" }`; `Image` →
/// `{ "type": "image", "source": { "type": "base64", "media_type",
/// "data" } }` (the pi-shaped `data` is base64 WITHOUT a `data:` prefix —
/// sent VERBATIM, unlike the OpenAI data-URI composition in
/// [`MessageContent::to_wire`]).
fn anthropic_block_wire(b: &ContentBlock) -> Value {
    match b {
        ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
        ContentBlock::Image { image } => json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image.mime_type,
                "data": image.data,
            },
        }),
    }
}

/// The `content` of a `tool_result` block: the `Text` string verbatim;
/// `Blocks` → an array of the Anthropic block shapes (a `Text` block →
/// `{ type: "text", text }`, an `Image` block → the image shape above).
fn anthropic_tool_result_content(c: &MessageContent) -> Value {
    match c {
        MessageContent::Text(t) => Value::String(t.clone()),
        MessageContent::Blocks(blocks) => {
            Value::Array(blocks.iter().map(anthropic_block_wire).collect())
        }
    }
}

/// Merge `next` into `existing` (the consecutive same-role merge —
/// Anthropic requires strict user/assistant alternation): string+string →
/// joined with `"\n"`; array+array → concatenated; string+array → the
/// string as a `text` block prepended; array+string → the `text` block
/// appended.
fn merge_content(existing: &mut Value, next: &Value) {
    if let (Value::String(a), Value::String(b)) = (&mut *existing, next) {
        a.push('\n');
        a.push_str(b);
        return;
    }
    if existing.is_array() && next.is_array() {
        existing
            .as_array_mut()
            .unwrap()
            .extend(next.as_array().unwrap().iter().cloned());
        return;
    }
    // The mixed `Text`+`Blocks` shapes: the text becomes a `text` block
    // (PREPENDED when it is the first message, APPENDED when the second).
    if existing.is_string() && next.is_array() {
        let text = std::mem::take(existing);
        let mut merged = vec![json!({ "type": "text", "text": text })];
        merged.extend(next.as_array().unwrap().iter().cloned());
        *existing = Value::Array(merged);
        return;
    }
    if existing.is_array() && next.is_string() {
        existing
            .as_array_mut()
            .unwrap()
            .push(json!({ "type": "text", "text": next.as_str().unwrap() }));
    }
}

/// The Anthropic `messages` array: the non-system messages with
/// CONSECATIVE-SAME-ROLE MERGING (Anthropic requires strict
/// user/assistant alternation — adjacent same-role messages merge per
/// [`merge_content`]), a `tool` message as a `user` `tool_result` block
/// (a run of `tool` messages merges into ONE `user` message), and an
/// assistant `tool_calls` as `tool_use` blocks (the `input` is a JSON
/// **object** — the OpenAI wire's string form does not apply; an assistant
/// message with `tool_calls` and EMPTY `Text` content → `tool_use` blocks
/// ONLY, no empty `text` block).
fn anthropic_messages(req: &ModelRequest) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in req.messages.iter().filter(|m| m.role != ChatRole::System) {
        if m.role == ChatRole::Tool {
            let block = json!({
                "type": "tool_result",
                "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": anthropic_tool_result_content(&m.content),
            });
            // A run of `tool` messages merges into the previous `user`
            // message (its content is a block array — a `tool_result` is
            // always a block).
            if let Some(last) = out.last_mut() {
                if last.get("role").and_then(|r| r.as_str()) == Some("user")
                    && last.get("content").is_some_and(|c| c.is_array())
                {
                    last["content"].as_array_mut().unwrap().push(block);
                    continue;
                }
            }
            out.push(json!({ "role": "user", "content": Value::Array(vec![block]) }));
            continue;
        }
        let role = if m.role == ChatRole::Assistant {
            "assistant"
        } else {
            "user"
        };
        let mut content = match &m.content {
            MessageContent::Text(t) => Value::String(t.clone()),
            MessageContent::Blocks(blocks) => {
                Value::Array(blocks.iter().map(anthropic_block_wire).collect())
            }
        };
        if let Some(calls) = &m.tool_calls {
            let tool_blocks: Vec<Value> = calls
                .iter()
                .map(|tc| {
                    json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": tc.arguments,
                    })
                })
                .collect();
            if content.is_string() {
                // An EMPTY `Text` → `tool_use` blocks ONLY (no empty `text`
                // block); a non-empty `Text` → a `text` block followed by
                // the `tool_use` blocks.
                let text = std::mem::take(&mut content);
                let mut blocks = Vec::new();
                if text.as_str().is_some_and(|s| !s.is_empty()) {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                blocks.extend(tool_blocks);
                content = Value::Array(blocks);
            } else if let Some(arr) = content.as_array_mut() {
                arr.extend(tool_blocks);
            }
        }
        // The consecutive same-role merge (the previous message's role
        // must differ — `tool` runs already merged into `user`).
        if let Some(last) = out.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                merge_content(&mut last["content"], &content);
                continue;
            }
        }
        out.push(json!({ "role": role, "content": content }));
    }
    out
}

/// The Anthropic `thinking` mode for a `reasoning_effort` level (the
/// desktop's level vocabulary → a `budget_tokens` value; an unrecognized
/// non-empty level → the `adaptive` mode — no budget, the provider decides).
fn anthropic_thinking(level: &str) -> Value {
    let budget: u64 = match level {
        "low" => 4096,
        "medium" => 10000,
        "high" => 20000,
        "xhigh" => 40000,
        "max" => 128000,
        _ => return json!({ "type": "adaptive" }),
    };
    json!({ "type": "enabled", "budget_tokens": budget })
}

/// Build the Anthropic Messages request body from a [`ModelRequest`]
/// (`max_tokens` is ALWAYS present — Anthropic requires it; `system` is
/// present only when non-empty; `thinking` is present only when a
/// `reasoning_effort` is set). The message `content` uses the Anthropic
/// WIRE form (a plain string for `Text`, an array of Anthropic content
/// parts for `Blocks` — [`anthropic_messages`]).
fn anthropic_request_body(req: &ModelRequest) -> Value {
    // `system`: the `System` messages' text (`Text` verbatim; `Blocks` →
    // the `Text` block texts joined with `"\n\n"`), the messages joined
    // with `"\n\n"`.
    let system = req
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::System)
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    ContentBlock::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut body = json!({
        "model": req.model,
        "messages": Value::Array(anthropic_messages(req)),
        "stream": req.options.stream,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !req.tools.is_empty() {
        // Flat `{ name, description, input_schema }` — the OpenAI
        // `function` nesting + `parameters` key do NOT apply.
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect::<Vec<_>>()
            .into();
    }
    if let Some(t) = req.options.temperature {
        body["temperature"] = json!(t);
    }
    // `max_tokens` is ALWAYS present (Anthropic requires it): the caller's
    // value or the default, clamped UP over the `thinking` budget (the
    // API requires `max_tokens > budget_tokens`).
    let mut max_tokens = req
        .options
        .max_tokens
        .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS);
    if let Some(level) = &req.options.reasoning_effort {
        let thinking = anthropic_thinking(level);
        // The API requires `max_tokens > budget_tokens` — clamp UP over
        // the budget (the `adaptive` mode has no budget — no clamp).
        if let Some(budget) = thinking.get("budget_tokens").and_then(|b| b.as_u64()) {
            max_tokens = max_tokens.max(budget as u32 + 1024);
        }
        body["thinking"] = thinking;
    }
    body["max_tokens"] = json!(max_tokens);
    body
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(
        &self,
        req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        // Normalize the base to include the `/v1` API prefix (ADR 0024 —
        // mirroring ZCode's adapter-boundary normalization: a gateway root
        // like `https://openrouter.ai/api` → `…/api/v1` before `/messages`
        // is appended; idempotent for a base that already ends in `/v1`).
        let url = format!("{}/messages", normalize_anthropic_base_url(&self.base_url));
        // The shared idle-based bounds (the `OpenAiCompatibleProvider`
        // values — `build_client`'s docs: a generous CONNECT bound + a
        // `read_timeout` (the time BETWEEN BYTES on the body)).
        let client = build_client(
            Duration::from_secs(30),     // a generous CONNECT bound
            Duration::from_secs(5 * 60), // 5 min of SILENCE = a stalled provider
        );
        // `x-api-key` is ALWAYS sent (a keyless Anthropic endpoint is a
        // misconfiguration, but sending an empty value is harmless and
        // keeps one code path) + the required `anthropic-version` (the
        // `User-Agent` comes from `build_client`).
        let mut builder = client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01");
        if let Some(ref sid) = req.session_id {
            builder = builder
                .header("x-litellm-session-id", sid)
                .header("x-request-id", sid);
        }
        let resp = builder
            .json(&anthropic_request_body(req))
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
        Ok(Box::pin(AnthropicStream::new(raw)))
    }
}

/// The OpenAI Responses WIRE shape of a content block: `Text` →
/// `{ "type": "input_text", "text" }`; `Image` →
/// `{ "type": "input_image", "image_url": "data:<mime>;base64,<data>"
/// }` (the OpenAI data-URI composition — the `ContentBlock::Image`
/// `data` field is base64 WITHOUT a `data:` prefix, so the URI is
/// composed here; the part `type` is `input_image`, NOT `image_url` —
/// the OpenAI chat-completions part type from
/// [`MessageContent::to_wire`]).
fn responses_block_wire(b: &ContentBlock) -> Value {
    match b {
        ContentBlock::Text { text } => json!({ "type": "input_text", "text": text }),
        ContentBlock::Image { image } => json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        }),
    }
}

/// The `content` of a Responses wire item: `Text` → a plain string;
/// `Blocks` → an array of Responses parts ([`responses_block_wire`]).
fn responses_content_wire(c: &MessageContent) -> Value {
    match c {
        MessageContent::Text(t) => Value::String(t.clone()),
        MessageContent::Blocks(blocks) => {
            Value::Array(blocks.iter().map(responses_block_wire).collect())
        }
    }
}

/// The Responses `input` array: the non-system messages as items (NO
/// same-role merging — the Responses API accepts consecutive same-role
/// items): `user` / `assistant` → `{ role, content }` (the content in
/// the Responses wire form); a `tool` message → a `function_call_output`
/// item (`{ type, call_id, output }` — NO `role` field, the `item` type
/// IS the role); and an `assistant` message with `tool_calls` → ONE
/// `function_call` item PER CALL (in call order — the API validates that
/// each `function_call_output`'s `call_id` references a `function_call`
/// item in `input`, so the `function_call` items are REQUIRED: dropping
/// them makes the second turn of every tool-using session 400), preceded
/// by a `message` item for the text content (an EMPTY `Text` →
/// `function_call` items ONLY, no `message` item).
fn responses_input(req: &ModelRequest) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in req.messages.iter().filter(|m| m.role != ChatRole::System) {
        if m.role == ChatRole::Tool {
            out.push(json!({
                "type": "function_call_output",
                "call_id": m.tool_call_id.clone().unwrap_or_default(),
                "output": responses_content_wire(&m.content),
            }));
            continue;
        }
        let role = if m.role == ChatRole::Assistant {
            "assistant"
        } else {
            "user"
        };
        if let Some(calls) = &m.tool_calls {
            // The assistant's TEXT content (if any) is a SEPARATE
            // `message` item that PRECEDES the `function_call` items: an
            // EMPTY `Text` / `Blocks` → `function_call` items ONLY.
            let message_item = match &m.content {
                MessageContent::Text(t) if !t.is_empty() => Some(json!({
                    "role": role,
                    "content": Value::String(t.clone()),
                })),
                MessageContent::Text(_) => None,
                MessageContent::Blocks(blocks) if !blocks.is_empty() => Some(json!({
                    "role": role,
                    "content": Value::Array(blocks.iter().map(responses_block_wire).collect()),
                })),
                MessageContent::Blocks(_) => None,
            };
            if let Some(item) = message_item {
                out.push(item);
            }
            // ONE `function_call` item PER CALL, in call order. The
            // `arguments` is a JSON **string** (the same wire form as
            // the OpenAI `function` object's `arguments` — re-serialized
            // from the parsed `Value`; an empty-arguments call →
            // `"{}"`).
            for tc in calls {
                let args =
                    serde_json::to_string(&tc.arguments).expect("JSON serialization cannot fail");
                out.push(json!({
                    "type": "function_call",
                    "call_id": tc.id,
                    "name": tc.name,
                    "arguments": args,
                }));
            }
            continue;
        }
        out.push(json!({ "role": role, "content": responses_content_wire(&m.content) }));
    }
    out
}

/// A client for the OpenAI Responses API (`POST {base_url}/responses`,
/// the `event:`-tagged SSE response parsed by [`ResponsesStream`]; ADR
/// 0024). The auth header is `Authorization: Bearer` (the
/// `OpenAiCompatibleProvider` pattern — the Responses API is OpenAI's,
/// unlike Anthropic's `x-api-key`).
#[derive(Debug, Clone)]
pub struct OpenAiResponsesProvider {
    /// The API base (e.g. `https://api.openai.com/v1` — the client
    /// appends `/responses`).
    pub base_url: String,
    /// The `Authorization: Bearer` key.
    pub api_key: String,
}

/// Build the OpenAI Responses request body from a [`ModelRequest`]
/// (`max_output_tokens` is included ONLY when `Some` — unlike Anthropic,
/// NOT required; `instructions` is present only when non-empty; the
/// `reasoning` `effort` is passed through VERBATIM — the desktop's level
/// vocabulary for an OpenAI model comes from the provider's own
/// discovery, so it is already in the provider's vocabulary). The
/// message `content` uses the Responses WIRE form (a plain string for
/// `Text`, an array of Responses content parts for `Blocks` —
/// [`responses_input`]).
fn responses_request_body(req: &ModelRequest) -> Value {
    // `instructions`: the `System` messages' text (the same collection as
    // Anthropic's `system` — `Text` verbatim; `Blocks` → the `Text`
    // block texts joined with `"\n\n"`; the messages joined with
    // `"\n\n"`).
    let instructions = req
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::System)
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    ContentBlock::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut body = json!({
        "model": req.model,
        "input": Value::Array(responses_input(req)),
        "stream": req.options.stream,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if !req.tools.is_empty() {
        // Flat `{ type: "function", name, description, parameters }` —
        // the `function` nesting does NOT apply (the `name` /
        // `description` / `parameters` are at the TOP level plus the
        // `type` tag).
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect::<Vec<_>>()
            .into();
    }
    if let Some(t) = req.options.temperature {
        body["temperature"] = json!(t);
    }
    // `max_output_tokens` (the Responses name) — included ONLY when
    // `Some` (unlike Anthropic's `max_tokens`, NOT required).
    if let Some(m) = req.options.max_tokens {
        body["max_output_tokens"] = json!(m);
    }
    if let Some(level) = &req.options.reasoning_effort {
        if !level.is_empty() {
            body["reasoning"] = json!({ "effort": level });
        }
    }
    body
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    async fn complete(
        &self,
        req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        // The shared idle-based bounds (the `OpenAiCompatibleProvider`
        // values — `build_client`'s docs: a generous CONNECT bound + a
        // `read_timeout` (the time BETWEEN BYTES on the body)).
        let client = build_client(
            Duration::from_secs(30),     // a generous CONNECT bound
            Duration::from_secs(5 * 60), // 5 min of SILENCE = a stalled provider
        );
        let mut builder = client.post(&url).bearer_auth(&self.api_key);
        if let Some(ref sid) = req.session_id {
            builder = builder
                .header("x-litellm-session-id", sid)
                .header("x-request-id", sid);
        }
        let resp = builder
            .json(&responses_request_body(req))
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
        Ok(Box::pin(ResponsesStream::new(raw)))
    }
}

/// Build the `Provider` for a `Model` (the `api` discriminator — ADR
/// 0024; the `SessionManager`'s default factory + the tests).
pub fn build_provider(m: &crate::agent::harness::catalog::Model) -> Box<dyn Provider> {
    match m.api.as_deref() {
        Some("anthropic-messages") => Box::new(AnthropicProvider {
            base_url: m.base_url.clone(),
            api_key: m.api_key.clone(),
        }),
        Some("openai-responses") => Box::new(OpenAiResponsesProvider {
            base_url: m.base_url.clone(),
            api_key: m.api_key.clone(),
        }),
        // `openai-completions` / `litellm` / `None` / unknown — the
        // current default (an unknown API is unselectable in practice,
        // but a hand-built `Model` with a weird `api` still gets a
        // working client). `litellm` is a DISCOVERY mode, not a wire —
        // its completion wire IS `openai-completions` (ADR 0026).
        _ => Box::new(OpenAiCompatibleProvider {
            base_url: m.base_url.clone(),
            api_key: m.api_key.clone(),
        }),
    }
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

    /// Build an `AnthropicStream` over RAW byte chunks (the `&str`
    /// helper below cannot split a multi-byte codepoint mid-byte —
    /// `&str` is valid UTF-8; this one can).
    fn anthropic_stream_bytes(chunks: &[Vec<u8>]) -> AnthropicStream {
        let data: Vec<Vec<u8>> = chunks.to_vec();
        let raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>> =
            futures_util::stream::iter(data.into_iter().map(Ok)).boxed();
        AnthropicStream::new(raw)
    }

    /// Build an `AnthropicStream` over canned raw chunks (no HTTP).
    fn anthropic_stream(chunks: &[&str]) -> AnthropicStream {
        // Owned data so the stream is `'static` (independent of `chunks`).
        let data: Vec<Vec<u8>> = chunks.iter().map(|c| c.as_bytes().to_vec()).collect();
        anthropic_stream_bytes(&data)
    }

    /// Collect the whole stream (generic over the parser — the `SseStream`
    /// and the `AnthropicStream` suites share it).
    async fn collect<S: Stream<Item = ProviderEvent> + Unpin>(s: S) -> Vec<ProviderEvent> {
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
    fn map_anthropic_stop_reason_mapping() {
        assert_eq!(map_anthropic_stop_reason("end_turn"), FinishReason::Stop);
        assert_eq!(
            map_anthropic_stop_reason("tool_use"),
            FinishReason::ToolCalls
        );
        assert_eq!(
            map_anthropic_stop_reason("max_tokens"),
            FinishReason::Length
        );
        // `stop_sequence` (a custom stop sequence hit) is a normal stop.
        assert_eq!(
            map_anthropic_stop_reason("stop_sequence"),
            FinishReason::Stop
        );
        // An unknown value is a normal stop (logged).
        assert_eq!(
            map_anthropic_stop_reason("something_new"),
            FinishReason::Stop
        );
    }

    #[test]
    fn map_anthropic_error_mapping() {
        fn err(kind: Option<&str>) -> ProviderError {
            let mut error = serde_json::Map::new();
            error.insert("message".to_string(), json!("boom"));
            if let Some(k) = kind {
                error.insert("type".to_string(), json!(k));
            }
            map_anthropic_error(&json!({ "type": "error", "error": error }))
        }
        // `overloaded_error` / `api_error` are transient.
        for kind in ["overloaded_error", "api_error"] {
            match err(Some(kind)) {
                ProviderError::Retryable(_) => {}
                e => panic!("{kind} is retryable, got {e:?}"),
            }
        }
        // `authentication_error` / `permission_error` are auth.
        for kind in ["authentication_error", "permission_error"] {
            match err(Some(kind)) {
                ProviderError::Auth(_) => {}
                e => panic!("{kind} is auth, got {e:?}"),
            }
        }
        // `invalid_request_error` / `not_found_error` are permanent.
        for kind in ["invalid_request_error", "not_found_error"] {
            match err(Some(kind)) {
                ProviderError::Fatal(_) => {}
                e => panic!("{kind} is fatal, got {e:?}"),
            }
        }
        // An UNKNOWN type is conservative — retryable.
        match err(Some("mystery_error")) {
            ProviderError::Retryable(_) => {}
            e => panic!("an unknown type is retryable, got {e:?}"),
        }
        // An absent / empty `type` is retryable, labeled `unknown`.
        for kind in [None, Some("")] {
            match err(kind) {
                ProviderError::Retryable(msg) => {
                    assert!(
                        msg.contains("unknown"),
                        "an absent/empty type is labeled `unknown` (got {msg:?})"
                    );
                }
                other => panic!("an absent/empty type is retryable, got {other:?}"),
            }
        }
    }

    #[test]
    fn normalize_anthropic_base_url_appends_v1_at_the_adapter_boundary() {
        // (ADR 0024 — mirroring ZCode's `normalizeAnthropicBaseURL`.)
        // A base that ALREADY ends in `/v1` is left as-is (idempotent).
        assert_eq!(
            normalize_anthropic_base_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1"
        );
        // A trailing slash is dropped (ZCode sets the stripped pathname).
        assert_eq!(
            normalize_anthropic_base_url("https://api.anthropic.com/v1/"),
            "https://api.anthropic.com/v1"
        );
        // A GATEWAY ROOT that does not end in `/v1` gets it appended —
        // the OpenRouter case (the user's finding: `…/api` → `…/api/v1`).
        assert_eq!(
            normalize_anthropic_base_url("https://openrouter.ai/api"),
            "https://openrouter.ai/api/v1"
        );
        // A trailing slash on a gateway root is dropped + `/v1` appended.
        assert_eq!(
            normalize_anthropic_base_url("https://openrouter.ai/api/"),
            "https://openrouter.ai/api/v1"
        );
        // A BARE domain (no path) gets `/v1` (the official-URL discovery
        // win: `https://api.anthropic.com` → `…/v1`).
        assert_eq!(
            normalize_anthropic_base_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1"
        );
        // A deeper gateway root (Z.ai / BigModel / DeepSeek) — `/v1`
        // appended to the path (ZCode's exact behavior).
        assert_eq!(
            normalize_anthropic_base_url("https://api.z.ai/api/anthropic"),
            "https://api.z.ai/api/anthropic/v1"
        );
        assert_eq!(
            normalize_anthropic_base_url("https://open.bigmodel.cn/api/anthropic"),
            "https://open.bigmodel.cn/api/anthropic/v1"
        );
        assert_eq!(
            normalize_anthropic_base_url("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/anthropic/v1"
        );
        // Case-insensitive: an uppercase `/V1` is recognized (left as-is).
        assert_eq!(
            normalize_anthropic_base_url("https://api.anthropic.com/V1"),
            "https://api.anthropic.com/V1"
        );
        // A query string is NOT corrupted: `/v1` is appended to the PATH,
        // not after the query (ZCode operates on `url.pathname`).
        assert_eq!(
            normalize_anthropic_base_url("https://openrouter.ai/api?x=1"),
            "https://openrouter.ai/api/v1?x=1"
        );
        // An unparseable base falls back to a plain suffix check (ZCode's
        // `catch` arm) — still appends `/v1`.
        assert_eq!(normalize_anthropic_base_url("not-a-url"), "not-a-url/v1");
        // Empty / whitespace → empty (no `/v1` appended to nothing).
        assert_eq!(normalize_anthropic_base_url(""), "");
        assert_eq!(normalize_anthropic_base_url("   "), "");
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

    // ── the Anthropic Messages wire (`event:`-tagged SSE) ──────────────

    #[tokio::test]
    async fn anthropic_sse_text_and_finish_reason() {
        // The full happy path: one `event:` line preceding each `data:` line.
        let events = collect(anthropic_stream(&[
            "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"He\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"llo\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n\
             event: message_stop\n\
             data: {\"type\":\"message_stop\"}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("He".to_string()),
                ProviderEvent::TextDelta("llo".to_string()),
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                }),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[tokio::test]
    async fn anthropic_sse_thinking_deltas() {
        // A `thinking` block, then a `text` block, then the `message_delta`.
        let events = collect(anthropic_stream(&[
            "event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"think\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
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

    #[tokio::test]
    async fn anthropic_sse_tool_use_block() {
        let events = collect(anthropic_stream(&[
            "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"read\",\"input\":{}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pat\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"h\\\":\\\"/x\\\"}\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n",
        ]))
        .await;
        // The `message_delta` handler runs `emit_usage()` BEFORE
        // `flush_tool_calls()` — the `Usage` PRECEDES the `ToolCall`
        // (matching the OpenAI wire's trailing-usage semantics).
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("hi".to_string()),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 1,
                    id: Some("tu_1".to_string()),
                    name: Some("read".to_string()),
                    arguments: None,
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 1,
                    id: None,
                    name: None,
                    arguments: Some("{\"pat".to_string()),
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 1,
                    id: None,
                    name: None,
                    arguments: Some("h\":\"/x\"}".to_string()),
                }),
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 7,
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "tu_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "/x" }),
                }),
                ProviderEvent::Done(FinishReason::ToolCalls),
            ]
        );
    }

    #[tokio::test]
    async fn anthropic_sse_multiple_tool_use_blocks() {
        let events = collect(anthropic_stream(&[
            "event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_0\",\"name\":\"read\",\"input\":{}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"p\\\":\\\"x\\\"}\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"bash\",\"input\":{}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"c\\\":\\\"y\\\"}\"}}\n\n\
             event: content_block_stop\n\
             data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
        ]))
        .await;
        // Two `tool_use` blocks → two `ToolCall`s in index order (`read`
        // then `bash`); NO `Usage` (no `message_start` / `usage` in the
        // fixture).
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("hi".to_string()),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    id: Some("tu_0".to_string()),
                    name: Some("read".to_string()),
                    arguments: None,
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments: Some("{\"p\":\"x\"}".to_string()),
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 1,
                    id: Some("tu_1".to_string()),
                    name: Some("bash".to_string()),
                    arguments: None,
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 1,
                    id: None,
                    name: None,
                    arguments: Some("{\"c\":\"y\"}".to_string()),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "tu_0".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "p": "x" }),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "tu_1".to_string(),
                    name: "bash".to_string(),
                    arguments: json!({ "c": "y" }),
                }),
                ProviderEvent::Done(FinishReason::ToolCalls),
            ]
        );
    }

    #[tokio::test]
    async fn anthropic_sse_error_event() {
        let events = collect(anthropic_stream(&[
            "event: error\n\
             data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        ]))
        .await;
        assert_eq!(
            events.len(),
            1,
            "an `error` event is terminal (the stream ends)"
        );
        match &events[0] {
            ProviderEvent::Error(ProviderError::Retryable(msg)) => {
                assert!(
                    msg.contains("Overloaded"),
                    "the error carries the wire message (got {msg:?})"
                );
            }
            other => panic!("expected a Retryable error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn anthropic_sse_auth_error_event() {
        let events = collect(anthropic_stream(&[
            "event: error\n\
             data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"Bad key\"}}\n\n",
        ]))
        .await;
        match &events[0] {
            ProviderEvent::Error(ProviderError::Auth(msg)) => {
                assert!(
                    msg.contains("Bad key"),
                    "the error carries the wire message (got {msg:?})"
                );
            }
            other => panic!("expected an Auth error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn anthropic_sse_stream_end_without_message_delta() {
        // `message_start` + one `text_delta` only (no `message_delta`).
        let events = collect(anthropic_stream(&[
            "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\n",
        ]))
        .await;
        // The `Usage` is emitted at stream end (the input is known — the
        // `0` output is the documented absent-side rule) and a synthetic
        // `Done(Error)` closes the turn.
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("x".to_string()),
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 0,
                }),
                ProviderEvent::Done(FinishReason::Error),
            ]
        );
    }

    #[tokio::test]
    async fn anthropic_sse_usage_emitted_once_from_the_final_message_delta() {
        // An intermediate `message_delta` (NO `stop_reason`) does NOT emit
        // the `Usage` — exactly ONE, from the LAST value seen.
        let events = collect(anthropic_stream(&[
            "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":3}}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 7,
                }),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    /// (finding 9, the Anthropic wire) The raw bytes may split a
    /// MULTI-BYTE codepoint between chunks (CJK is 3 bytes, emoji is 4).
    /// The buffer must hold RAW bytes and convert only COMPLETE lines: the
    /// reassembled text is byte-identical at every possible split point.
    #[tokio::test]
    async fn anthropic_multibyte_chars_survive_chunk_boundaries() {
        // CJK (3-byte) + emoji (4-byte) content in an Anthropic
        // `content_block_delta` `text_delta` line.
        let text = "你好🌍世界🚀";
        let payload = format!(
            "event: content_block_delta\n\
             data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n"
        );
        let bytes = payload.as_bytes();
        // Split the raw bytes at EVERY offset — including the middle of a
        // 3-byte and a 4-byte codepoint. Each reassembly must be
        // byte-identical (no U+FFFD).
        for split_at in 1..bytes.len() {
            let events = collect(anthropic_stream_bytes(&[
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
        let events = collect(anthropic_stream_bytes(&[
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
            session_id: None,
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
            session_id: None,
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
            session_id: None,
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

    /// The client identifies itself: the request carries a `User-Agent`
    /// header (`archimedes/<version>`) so the provider's logs can
    /// attribute the traffic to the app (the harness is the desktop's own
    /// OpenAI-compatible client — `reqwest` sends NO `User-Agent` by
    /// default, so without this the traffic is unattributable).
    #[tokio::test]
    async fn the_request_sends_a_user_agent_header() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // A raw server that CAPTURES the request headers (the `reqwest`
        // response is complete — `data: [DONE]` — so the client closes
        // the connection; the captured bytes go to the test via the
        // channel).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the listener binds");
        let addr = listener.local_addr().expect("the listener has an address");
        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let mut data = Vec::new();
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let _ = tx.send(data).await;
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: [DONE]\r\n\r\n",
                )
                .await;
            // Hold the socket open until the test is done (the response
            // body is complete — the client closes the connection).
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = build_client(Duration::from_secs(30), Duration::from_millis(200));
        let _resp = client
            .post(format!("http://{addr}/chat/completions"))
            .send()
            .await
            .expect("the request sends");
        let raw = rx.recv().await.expect("the server captured the request");
        let request = String::from_utf8(raw).expect("the request is valid UTF-8");
        let header = request
            .lines()
            .find(|l| l.to_lowercase().starts_with("user-agent:"))
            .expect("the request carries a User-Agent header")
            .to_lowercase();
        assert_eq!(
            header,
            format!("user-agent: archimedes/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    // ── the Anthropic provider (ADR 0024) ──────────────────────────────

    /// A `ChatMessage` test-literal helper (the `None` fields default to
    /// `None`).
    fn chat_message(
        role: ChatRole,
        content: MessageContent,
        tool_call_id: Option<&str>,
        tool_calls: Option<Vec<ToolCall>>,
    ) -> ChatMessage {
        ChatMessage {
            role,
            content,
            tool_call_id: tool_call_id.map(|s| s.to_string()),
            tool_calls,
        }
    }

    /// The `AnthropicProvider` is object-safe (a `Box<dyn Provider>`
    /// compiles — the `SessionManager`'s factory dispatches on it).
    #[test]
    fn anthropic_provider_is_object_safe() {
        let provider: Box<dyn Provider> = Box::new(AnthropicProvider {
            base_url: "https://api.anthropic.com/v1".to_string(),
            api_key: "k".to_string(),
        });
        let _ = provider;
    }

    #[test]
    fn anthropic_request_body_maps_the_request() {
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![
                chat_message(
                    ChatRole::System,
                    MessageContent::Text("sys".to_string()),
                    None,
                    None,
                ),
                chat_message(
                    ChatRole::User,
                    MessageContent::Text("hi".to_string()),
                    None,
                    None,
                ),
                chat_message(
                    ChatRole::Assistant,
                    MessageContent::Text(String::new()),
                    None,
                    Some(vec![ToolCall {
                        id: "tu_1".to_string(),
                        name: "read".to_string(),
                        arguments: json!({ "path": "/x" }),
                    }]),
                ),
                chat_message(
                    ChatRole::Tool,
                    MessageContent::Text("out".to_string()),
                    Some("tu_1"),
                    None,
                ),
            ],
            tools: vec![ToolSpec {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({ "type": "object" }),
            }],
            options: ModelOptions {
                temperature: Some(0.2),
                max_tokens: Some(64),
                reasoning_effort: None,
                stream: true,
            },
            session_id: None,
        };
        let body = anthropic_request_body(&req);
        assert_eq!(body["model"], "m");
        assert_eq!(body["system"], "sys");
        // The non-system messages: `user`, `assistant`, `user` (the `tool`
        // message became a `user` `tool_result`).
        let messages = body["messages"].as_array().expect("messages are present");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hi");
        assert_eq!(messages[1]["role"], "assistant");
        // The assistant's `tool_calls` → `tool_use` blocks: the `input` is
        // a JSON **object** (NOT the OpenAI wire's JSON string), and an
        // EMPTY `Text` content → `tool_use` blocks ONLY.
        let content = messages[1]["content"]
            .as_array()
            .expect("assistant content is an array");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "tu_1");
        assert_eq!(content[0]["name"], "read");
        assert_eq!(content[0]["input"], json!({ "path": "/x" }));
        assert!(
            content[0]["input"].is_object(),
            "the `input` is an OBJECT, not a string"
        );
        // The `tool` message → a `user` `tool_result` block.
        assert_eq!(messages[2]["role"], "user");
        let tool_result = messages[2]["content"]
            .as_array()
            .expect("tool_result content is an array")
            .iter()
            .next()
            .expect("one tool_result block");
        assert_eq!(tool_result["type"], "tool_result");
        assert_eq!(tool_result["tool_use_id"], "tu_1");
        assert_eq!(tool_result["content"], "out");
        // `tools`: flat `{ name, description, input_schema }` — NO
        // `function` nesting, NO `type` field (unlike the OpenAI wire).
        assert_eq!(
            body["tools"][0],
            json!({
                "name": "read",
                "description": "Read a file",
                "input_schema": { "type": "object" },
            })
        );
        assert!(body["tools"][0].get("function").is_none());
        assert!(body["tools"][0].get("type").is_none());
        assert_eq!(body["max_tokens"], 64);
        let temperature = body["temperature"]
            .as_f64()
            .expect("temperature is a number");
        assert!(
            (temperature - 0.2).abs() < 1e-6,
            "temperature is passed through (got {temperature})"
        );
        assert_eq!(body["stream"], true);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn anthropic_request_body_consecutive_same_role_merge() {
        fn req(messages: Vec<ChatMessage>) -> Value {
            let r = ModelRequest {
                model: "m".to_string(),
                messages,
                tools: vec![],
                options: ModelOptions {
                    temperature: None,
                    max_tokens: None,
                    reasoning_effort: None,
                    stream: true,
                },
                session_id: None,
            };
            anthropic_request_body(&r)
        }
        // Two consecutive `User` `Text` messages → ONE `user` message with
        // the texts joined.
        let body = req(vec![
            chat_message(
                ChatRole::User,
                MessageContent::Text("a".to_string()),
                None,
                None,
            ),
            chat_message(
                ChatRole::User,
                MessageContent::Text("b".to_string()),
                None,
                None,
            ),
        ]);
        let messages = body["messages"].as_array().expect("messages are present");
        assert_eq!(messages.len(), 1, "the same-role pair merges into one");
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "a\nb");

        // A `User` `Text` then a `User` `Blocks` → ONE message with a
        // 2-block array (the text as a `text` block prepended).
        let body = req(vec![
            chat_message(
                ChatRole::User,
                MessageContent::Text("a".to_string()),
                None,
                None,
            ),
            chat_message(
                ChatRole::User,
                MessageContent::Blocks(vec![ContentBlock::Text {
                    text: "b".to_string(),
                }]),
                None,
                None,
            ),
        ]);
        let messages = body["messages"].as_array().expect("messages are present");
        assert_eq!(messages.len(), 1, "the Text+Blocks pair merges into one");
        assert_eq!(
            messages[0]["content"],
            json!([
                { "type": "text", "text": "a" },
                { "type": "text", "text": "b" },
            ])
        );
    }

    #[test]
    fn anthropic_request_body_multiple_tool_results_merge() {
        // A run of `tool` messages merges into ONE `user` message with
        // multiple `tool_result` blocks.
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![
                chat_message(
                    ChatRole::Tool,
                    MessageContent::Text("r1".to_string()),
                    Some("t1"),
                    None,
                ),
                chat_message(
                    ChatRole::Tool,
                    MessageContent::Text("r2".to_string()),
                    Some("t2"),
                    None,
                ),
            ],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                stream: true,
            },
            session_id: None,
        };
        let body = anthropic_request_body(&req);
        let messages = body["messages"].as_array().expect("messages are present");
        assert_eq!(
            messages.len(),
            1,
            "the tool run merges into ONE user message"
        );
        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"]
            .as_array()
            .expect("the merged content is a block array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "t1");
        assert_eq!(content[0]["content"], "r1");
        assert_eq!(content[1]["type"], "tool_result");
        assert_eq!(content[1]["tool_use_id"], "t2");
        assert_eq!(content[1]["content"], "r2");
    }

    #[test]
    fn anthropic_request_body_image_block_wire_shape() {
        // The Anthropic image wire shape: `source.data` is the base64
        // VERBATIM (NO `data:` prefix — unlike the OpenAI data-URI
        // composition in `MessageContent::to_wire`).
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![chat_message(
                ChatRole::User,
                MessageContent::Blocks(vec![ContentBlock::Image {
                    image: ImageRef {
                        data: "BASE64DATA".to_string(),
                        mime_type: "image/png".to_string(),
                    },
                }]),
                None,
                None,
            )],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                stream: true,
            },
            session_id: None,
        };
        let body = anthropic_request_body(&req);
        let content = body["messages"][0]["content"]
            .as_array()
            .expect("the content is a block array");
        assert_eq!(
            content[0],
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "BASE64DATA",
                },
            })
        );
    }

    #[test]
    fn anthropic_request_body_max_tokens_default_and_clamp() {
        fn req(max_tokens: Option<u32>, reasoning_effort: Option<&str>) -> Value {
            let r = ModelRequest {
                model: "m".to_string(),
                messages: vec![],
                tools: vec![],
                options: ModelOptions {
                    temperature: None,
                    max_tokens,
                    reasoning_effort: reasoning_effort.map(|s| s.to_string()),
                    stream: true,
                },
                session_id: None,
            };
            anthropic_request_body(&r)
        }
        // `max_tokens: None` + a `max` budget (128000) → clamped to
        // `budget + 1024` (Anthropic requires `max_tokens > budget_tokens`).
        assert_eq!(req(None, Some("max"))["max_tokens"], 129024);
        // `max_tokens: Some(64)` + a `low` budget (4096) → clamped UP to
        // `4096 + 1024`.
        assert_eq!(req(Some(64), Some("low"))["max_tokens"], 5120);
        // NO `reasoning_effort` + NO `max_tokens` → the default, NO
        // `thinking` field.
        let body = req(None, None);
        assert_eq!(body["max_tokens"], ANTHROPIC_DEFAULT_MAX_TOKENS);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn anthropic_request_body_thinking_map() {
        fn req(level: Option<&str>) -> Value {
            let r = ModelRequest {
                model: "m".to_string(),
                messages: vec![],
                tools: vec![],
                options: ModelOptions {
                    temperature: None,
                    max_tokens: None,
                    reasoning_effort: level.map(|s| s.to_string()),
                    stream: true,
                },
                session_id: None,
            };
            anthropic_request_body(&r)
        }
        // The desktop's level vocabulary → the `budget_tokens` values.
        assert_eq!(
            req(Some("low"))["thinking"],
            json!({ "type": "enabled", "budget_tokens": 4096 })
        );
        assert_eq!(
            req(Some("medium"))["thinking"],
            json!({ "type": "enabled", "budget_tokens": 10000 })
        );
        assert_eq!(
            req(Some("high"))["thinking"],
            json!({ "type": "enabled", "budget_tokens": 20000 })
        );
        assert_eq!(
            req(Some("xhigh"))["thinking"],
            json!({ "type": "enabled", "budget_tokens": 40000 })
        );
        assert_eq!(
            req(Some("max"))["thinking"],
            json!({ "type": "enabled", "budget_tokens": 128000 })
        );
        // An unrecognized non-empty level → the `adaptive` mode (no budget).
        assert_eq!(
            req(Some("weird"))["thinking"],
            json!({ "type": "adaptive" })
        );
        // `None` → the field is ABSENT.
        assert!(req(None).get("thinking").is_none());
    }

    // ── the OpenAI Responses wire (`event:`-tagged SSE, ADR 0024) ──────

    /// Build a `ResponsesStream` over RAW byte chunks (the `&str`
    /// helper below cannot split a multi-byte codepoint mid-byte —
    /// `&str` is valid UTF-8; this one can).
    fn responses_stream_bytes(chunks: &[Vec<u8>]) -> ResponsesStream {
        let data: Vec<Vec<u8>> = chunks.to_vec();
        let raw: BoxStream<'static, Result<Vec<u8>, reqwest::Error>> =
            futures_util::stream::iter(data.into_iter().map(Ok)).boxed();
        ResponsesStream::new(raw)
    }

    /// Build a `ResponsesStream` over canned raw chunks (no HTTP).
    fn responses_stream(chunks: &[&str]) -> ResponsesStream {
        // Owned data so the stream is `'static` (independent of `chunks`).
        let data: Vec<Vec<u8>> = chunks.iter().map(|c| c.as_bytes().to_vec()).collect();
        responses_stream_bytes(&data)
    }

    #[tokio::test]
    async fn responses_sse_text_and_completed() {
        // The full happy path: one `event:` line preceding each `data:`
        // line. A `message` `output_item.done` carries no content of its
        // own (the text streamed via the `*_delta` events) → nothing.
        let events = collect(responses_stream(&[
            "event: response.created\n\
             data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n\
             event: response.output_text.delta\n\
             data: {\"type\":\"response.output_text.delta\",\"delta\":\"He\"}\n\n\
             event: response.output_text.delta\n\
             data: {\"type\":\"response.output_text.delta\",\"delta\":\"llo\"}\n\n\
             event: response.output_item.done\n\
             data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"content\":[]}}\n\n\
             event: response.completed\n\
             data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n",
        ]))
        .await;
        // The `Usage` PRECEDES the `Done` (the `response.completed`
        // handler emits it first).
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("He".to_string()),
                ProviderEvent::TextDelta("llo".to_string()),
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                }),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[tokio::test]
    async fn responses_sse_reasoning_summary() {
        // The reasoning streams as `reasoning_summary_text.delta` fragments
        // (the `ThinkingDelta` events), then the text, then the terminal.
        let events = collect(responses_stream(&[
            "event: response.reasoning_summary_text.delta\n\
             data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"think\"}\n\n\
             event: response.reasoning_summary_text.delta\n\
             data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"ing\"}\n\n\
             event: response.output_text.delta\n\
             data: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\n\n\
             event: response.completed\n\
             data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":3}}}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::ThinkingDelta("think".to_string()),
                ProviderEvent::ThinkingDelta("ing".to_string()),
                ProviderEvent::TextDelta("done".to_string()),
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                }),
                ProviderEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[tokio::test]
    async fn responses_sse_function_call_full_lifecycle() {
        // `output_item.added` (the `id` + `name` + the empty `arguments`),
        // the `arguments` fragments, then the `output_item.done` with the
        // COMPLETE `arguments` (the done item is authoritative).
        let events = collect(responses_stream(&[
            "event: response.output_item.added\n\
             data: {\"type\":\"response.output_item.added\",\"output_index\":2,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"name\":\"read\",\"arguments\":\"\",\"status\":\"in_progress\"}}\n\n\
             event: response.function_call_arguments.delta\n\
             data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":2,\"delta\":\"{\"}\n\n\
             event: response.function_call_arguments.delta\n\
             data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":2,\"delta\":\"\\\"path\\\":\\\"/x\\\"}\"}\n\n\
             event: response.output_item.done\n\
             data: {\"type\":\"response.output_item.done\",\"output_index\":2,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"/x\\\"}\",\"status\":\"completed\"}}\n\n",
        ]))
        .await;
        // The `output_item.done` emits the `ToolCall`; the stream ends
        // WITHOUT a terminal event, but the (already-`done`) acc is NOT
        // flushed again — no duplicate `ToolCall`.
        assert_eq!(
            events,
            vec![
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 2,
                    id: Some("fc_1".to_string()),
                    name: Some("read".to_string()),
                    arguments: None,
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 2,
                    id: None,
                    name: None,
                    arguments: Some("{".to_string()),
                }),
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 2,
                    id: None,
                    name: None,
                    arguments: Some("\"path\":\"/x\"}".to_string()),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "fc_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "/x" }),
                }),
            ]
        );
    }

    #[tokio::test]
    async fn responses_sse_function_call_done_item_authoritative() {
        // NO `arguments.delta` events: the `output_item.done` carries the
        // COMPLETE `arguments` — the done item is authoritative (a proxy
        // that sent no `arguments.delta` events still yields a complete
        // call).
        let events = collect(responses_stream(&[
            "event: response.output_item.added\n\
             data: {\"type\":\"response.output_item.added\",\"output_index\":2,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"name\":\"read\",\"arguments\":\"\",\"status\":\"in_progress\"}}\n\n\
             event: response.output_item.done\n\
             data: {\"type\":\"response.output_item.done\",\"output_index\":2,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"/y\\\"}\",\"status\":\"completed\"}}\n\n",
        ]))
        .await;
        // The done item's `arguments` WIN (over the empty `added` one).
        assert_eq!(
            events,
            vec![
                ProviderEvent::ToolCallDelta(ToolCallDelta {
                    index: 2,
                    id: Some("fc_1".to_string()),
                    name: Some("read".to_string()),
                    arguments: None,
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "fc_1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({ "path": "/y" }),
                }),
            ]
        );
    }

    #[tokio::test]
    async fn responses_sse_incomplete_max_output_tokens() {
        // `incomplete_details.reason: "max_output_tokens"` → `Done(Length)`.
        let events = collect(responses_stream(&[
            "event: response.incomplete\n\
             data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":10,\"output_tokens\":32000}}}\n\n",
        ]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 32000,
                }),
                ProviderEvent::Done(FinishReason::Length),
            ]
        );
    }

    #[tokio::test]
    async fn responses_sse_failed() {
        // NO `usage` in the fixture → NO `Usage` event; just `Done(Error)`.
        let events = collect(responses_stream(&[
            "event: response.failed\n\
             data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"boom\"}}}\n\n",
        ]))
        .await;
        assert_eq!(events, vec![ProviderEvent::Done(FinishReason::Error)]);
    }

    #[tokio::test]
    async fn responses_sse_error_event() {
        // An in-stream `error` event → `Done(Error)` (terminal — the turn
        // cannot continue; deliberately simpler than the Anthropic
        // `error` mapping: the Responses `error` object has no stable
        // `type` taxonomy, and a mid-stream error after a partial
        // response must not be blindly retried).
        let events = collect(responses_stream(&[
            "event: error\n\
             data: {\"type\":\"error\",\"error\":{\"code\":\"server_error\",\"message\":\"boom\"}}\n\n",
        ]))
        .await;
        assert_eq!(
            events.len(),
            1,
            "an `error` event is terminal (the stream ends)"
        );
        assert_eq!(events[0], ProviderEvent::Done(FinishReason::Error));
    }

    #[tokio::test]
    async fn responses_sse_stream_end_without_terminal() {
        // One `output_text.delta` only (no terminal event): the stream end
        // synthesizes a `Done(Error)`. NO `Usage` — a Responses `usage`
        // arrives only on the terminal events, and a zero-filled `Usage`
        // would misreport.
        let events = collect(responses_stream(&["event: response.output_text.delta\n\
             data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n"]))
        .await;
        assert_eq!(
            events,
            vec![
                ProviderEvent::TextDelta("x".to_string()),
                ProviderEvent::Done(FinishReason::Error),
            ]
        );
    }

    /// (finding 9, the Responses wire) The raw bytes may split a
    /// MULTI-BYTE codepoint between chunks (CJK is 3 bytes, emoji is 4).
    /// The buffer must hold RAW bytes and convert only COMPLETE lines: the
    /// reassembled text is byte-identical at every possible split point.
    #[tokio::test]
    async fn responses_multibyte_chars_survive_chunk_boundaries() {
        // CJK (3-byte) + emoji (4-byte) content in a Responses
        // `output_text.delta` line.
        let text = "你好🌍世界🚀";
        let payload = format!(
            "event: response.output_text.delta\n\
             data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n"
        );
        let bytes = payload.as_bytes();
        // Split the raw bytes at EVERY offset — including the middle of a
        // 3-byte and a 4-byte codepoint. Each reassembly must be
        // byte-identical (no U+FFFD).
        for split_at in 1..bytes.len() {
            let events = collect(responses_stream_bytes(&[
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
        let events = collect(responses_stream_bytes(&[
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

    // ── the OpenAI Responses provider (ADR 0024) ──────────────────────

    /// The `OpenAiResponsesProvider` is object-safe (a `Box<dyn
    /// Provider>` compiles — the `SessionManager`'s factory dispatches on
    /// it).
    #[test]
    fn openai_responses_provider_is_object_safe() {
        let provider: Box<dyn Provider> = Box::new(OpenAiResponsesProvider {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: "k".to_string(),
        });
        let _ = provider;
    }

    #[test]
    fn responses_request_body_maps_the_request() {
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![
                chat_message(
                    ChatRole::System,
                    MessageContent::Text("sys".to_string()),
                    None,
                    None,
                ),
                chat_message(
                    ChatRole::User,
                    MessageContent::Text("hi".to_string()),
                    None,
                    None,
                ),
                chat_message(
                    ChatRole::Assistant,
                    MessageContent::Text("out".to_string()),
                    None,
                    None,
                ),
                chat_message(
                    ChatRole::Tool,
                    MessageContent::Text("result".to_string()),
                    Some("fc_1"),
                    None,
                ),
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
            session_id: None,
        };
        let body = responses_request_body(&req);
        assert_eq!(body["model"], "m");
        assert_eq!(body["instructions"], "sys");
        // The non-system messages → `input` items (NO same-role merging —
        // the Responses API accepts consecutive same-role items): the
        // `user` string content, the `assistant` string content, and the
        // `function_call_output` item (the `tool` message — the `item`
        // type IS the role: NO `role` field).
        let input = body["input"].as_array().expect("input is present");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "hi");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"], "out");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "fc_1");
        assert_eq!(input[2]["output"], "result");
        assert!(
            input[2].get("role").is_none(),
            "a `function_call_output` item has NO `role` field"
        );
        // `tools`: flat `{ type: "function", name, description,
        // parameters }` — NO `function` nesting.
        assert_eq!(
            body["tools"][0],
            json!({
                "type": "function",
                "name": "read",
                "description": "Read a file",
                "parameters": { "type": "object" },
            })
        );
        assert!(
            body["tools"][0].get("function").is_none(),
            "the Responses `tools` entry has NO `function` nesting"
        );
        // `max_output_tokens` (the Responses name — unlike Anthropic's
        // required `max_tokens`, included only when `Some`).
        assert_eq!(body["max_output_tokens"], 64);
        let temperature = body["temperature"]
            .as_f64()
            .expect("temperature is a number");
        assert!(
            (temperature - 0.2).abs() < 1e-6,
            "temperature is passed through (got {temperature})"
        );
        assert_eq!(body["stream"], true);
        // `reasoning`: the level passed through VERBATIM.
        assert_eq!(body["reasoning"], json!({ "effort": "high" }));
    }

    #[test]
    fn responses_request_body_blocks_and_image() {
        // `Blocks` content → an array of Responses parts: a `Text` block →
        // `{ type: "input_text", text }`, an `Image` block →
        // `{ type: "input_image", image_url: "data:<mime>;base64,<data>"
        // }` (the OpenAI data-URI — WITH the `data:` prefix, unlike the
        // Anthropic verbatim-base64 shape; the part `type` is
        // `input_image`, NOT `image_url`).
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![chat_message(
                ChatRole::User,
                MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "a".to_string(),
                    },
                    ContentBlock::Image {
                        image: ImageRef {
                            data: "B64".to_string(),
                            mime_type: "image/png".to_string(),
                        },
                    },
                ]),
                None,
                None,
            )],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                stream: true,
            },
            session_id: None,
        };
        let body = responses_request_body(&req);
        let content = body["input"][0]["content"]
            .as_array()
            .expect("the content is a parts array");
        assert_eq!(content[0], json!({ "type": "input_text", "text": "a" }));
        assert_eq!(
            content[1],
            json!({
                "type": "input_image",
                "image_url": "data:image/png;base64,B64",
            })
        );
    }

    #[test]
    fn responses_request_body_omits_none_options() {
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
            session_id: None,
        };
        let body = responses_request_body(&req);
        assert!(body.get("temperature").is_none());
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("reasoning").is_none());
        assert!(body.get("tools").is_none());
        // `messages: []` → an empty `input` array (the chosen shape).
        let input = body["input"].as_array().expect("input is present");
        assert!(input.is_empty(), "no messages → an empty `input` array");
        // `instructions` MUST be absent when there are no system messages.
        assert!(
            body.get("instructions").is_none(),
            "no system messages → NO `instructions` field"
        );
    }

    /// An `Assistant` message with `tool_calls` emits ONE `function_call`
    /// item PER CALL (in call order — `arguments` as a JSON **string**,
    /// the Responses wire form), preceded by a `message` item for the
    /// text content (an EMPTY `Text` → `function_call` items ONLY). This
    /// is REQUIRED: the Responses API validates that each
    /// `function_call_output`'s `call_id` references a `function_call`
    /// item in `input` — dropping the `function_call` items makes the
    /// second turn of every tool-using `openai-responses` session 400.
    #[test]
    fn responses_request_body_serializes_tool_calls_as_function_call_items() {
        let req = ModelRequest {
            model: "m".to_string(),
            messages: vec![
                chat_message(
                    ChatRole::Assistant,
                    MessageContent::Text(String::new()),
                    None,
                    Some(vec![
                        ToolCall {
                            id: "fc_1".to_string(),
                            name: "read".to_string(),
                            arguments: json!({ "path": "/x" }),
                        },
                        ToolCall {
                            id: "fc_2".to_string(),
                            name: "bash".to_string(),
                            arguments: Value::Object(Default::default()),
                        },
                    ]),
                ),
                chat_message(
                    ChatRole::Tool,
                    MessageContent::Text("out".to_string()),
                    Some("fc_1"),
                    None,
                ),
            ],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                stream: true,
            },
            session_id: None,
        };
        let body = responses_request_body(&req);
        let input = body["input"].as_array().expect("input is present");
        // In order: the `function_call` items (per call, call order),
        // then the `function_call_output` item. NO `message` item for the
        // assistant (EMPTY `Text`).
        assert_eq!(
            input.len(),
            3,
            "two `function_call` items + one `function_call_output` (no `message` item for the empty `Text`), got {input:?}"
        );
        // `fc_1`: `arguments` as a JSON **string** (not an object).
        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "fc_1");
        assert_eq!(input[0]["name"], "read");
        assert_eq!(input[0]["arguments"], "{\"path\":\"/x\"}");
        assert!(
            input[0]["arguments"].is_string(),
            "the `arguments` is a JSON **string**, not an object"
        );
        // `fc_2`: an empty-arguments call serializes to `"{}"`.
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "fc_2");
        assert_eq!(input[1]["name"], "bash");
        assert_eq!(input[1]["arguments"], "{}");
        // The `function_call_output` item: the `call_id` references the
        // `function_call` items' `call_id`s (the API's validation
        // target); NO `role` field.
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "fc_1");
        assert_eq!(input[2]["output"], "out");
        assert!(input[2].get("role").is_none());
        // NO `message` item anywhere (the assistant's `Text` was empty).
        assert!(
            input.iter().all(|item| item.get("role").is_none()),
            "an EMPTY `Text` → `function_call` items ONLY, no `message` item: {input:?}"
        );
    }

    // ── `build_provider` (ADR 0024: the `api` discriminator) ────────

    /// A `Model` test-literal helper (everything else pinned; `api` is
    /// the parameter — the `catalog.rs` tests' `model` helper pattern,
    /// extended with the `api` parameter).
    fn model(base_url: &str, api: Option<&str>) -> crate::agent::harness::catalog::Model {
        crate::agent::harness::catalog::Model {
            id: "m/1".to_string(),
            provider: "p".to_string(),
            base_url: base_url.to_string(),
            api_key: "k".to_string(),
            context_window: 128000,
            cost_per_mtok_in: 0.0,
            cost_per_mtok_out: 0.0,
            supports_tools: true,
            supports_thinking: false,
            thinking_levels: Vec::new(),
            api: api.map(str::to_string),
        }
    }

    /// A minimal `ModelRequest` (a single user message; `max_tokens`
    /// present for the Anthropic wire's required field).
    fn test_request() -> ModelRequest {
        ModelRequest {
            model: "m/1".to_string(),
            messages: vec![chat_message(
                ChatRole::User,
                MessageContent::Text("hi".to_string()),
                None,
                None,
            )],
            tools: vec![],
            options: ModelOptions {
                temperature: None,
                max_tokens: Some(64),
                reasoning_effort: None,
                stream: true,
            },
            session_id: None,
        }
    }

    /// A recorded mock HTTP server: LOOP-accepts (one request per
    /// connection — the `build_provider` dispatch test performs FOUR
    /// dispatches, so a single-accept server would hang the 2nd/3rd/4th),
    /// reads each request's headers until `\r\n\r\n` (+ the
    /// `Content-Length` body), records the `PATH` line + the
    /// `authorization` / `x-api-key` header presence, and replies the
    /// wire-appropriate minimal 200 SSE body (a `data: {"choices":[…]}`
    /// for OpenAI, a `message_start` + `message_delta` pair for
    /// Anthropic, a `response.completed` for Responses).
    async fn recorded_dispatch_server(
        listener: tokio::net::TcpListener,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::Receiver<(String, bool, bool)>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel::<(String, bool, bool)>(8);
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 65536];
                let mut data = Vec::new();
                // Read the headers (until `\r\n\r\n`).
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                let header_end = data
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .expect("the header terminator")
                    + 4;
                let headers = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
                let path = headers.lines().next().unwrap_or_default().to_string();
                let has_auth = headers.lines().any(|l| l.starts_with("authorization:"));
                let has_x_api_key = headers.lines().any(|l| l.starts_with("x-api-key:"));
                // Read the body (`Content-Length` bytes — a clean close
                // needs the request fully consumed; the client may send
                // the body in the same or a later segment).
                let content_length = headers
                    .lines()
                    .find(|l| l.starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1).unwrap().trim().parse().ok())
                    .unwrap_or(0) as usize;
                let mut have = data.len();
                while have < header_end + content_length {
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    have = data.len();
                }
                // The wire-appropriate minimal 200 SSE body (the `PATH`
                // decides the wire — one server, four dispatches).
                let body = if path.contains("/messages") {
                    // Anthropic: a `message_start` + `message_delta` pair.
                    "event: message_start\r\n\
                     data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\r\n\
                     \r\n\
                     event: message_delta\r\n\
                     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\r\n\
                     \r\n"
                } else if path.contains("/responses") {
                    // Responses: a `response.completed`.
                    "event: response.completed\r\n\
                     data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\r\n\
                     \r\n"
                } else {
                    // OpenAI: a `data: {"choices":[…]}` + `[DONE]`.
                    "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\r\n\
                     \r\n\
                     data: [DONE]\r\n\
                     \r\n"
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                if stream.write_all(resp.as_bytes()).await.is_err() {
                    break;
                }
                if tx.send((path, has_auth, has_x_api_key)).await.is_err() {
                    break;
                }
                // DROP the stream (the client sees EOF → the stream ends).
            }
        });
        (server, rx)
    }

    /// (ADR 0024) `build_provider` dispatches on `Model.api`: an
    /// `anthropic-messages` model runs on `AnthropicProvider` (`POST
    /// /messages`, an `x-api-key` header, NO `authorization` header), an
    /// `openai-responses` model on `OpenAiResponsesProvider` (`POST
    /// /responses`, an `authorization` header), and a `None`-api model
    /// on `OpenAiCompatibleProvider` (`POST /chat/completions`, an
    /// `authorization` header — the current default), and a `litellm`
    /// model on `OpenAiCompatibleProvider` too (`POST /chat/completions`,
    /// an `authorization` header — ADR 0026: `litellm` is a DISCOVERY
    /// mode, its wire is `openai-completions`). Each stream is collected
    /// to completion (the recorded mock server replies the
    /// wire-appropriate minimal 200 SSE body; the loop-accept server
    /// handles the FOUR dispatches — one request per connection).
    #[tokio::test]
    async fn build_provider_dispatches_on_the_api() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the listener binds");
        let addr = listener.local_addr().expect("the listener has an address");
        let base_url = format!("http://{addr}/v1");
        let (server, mut rx) = recorded_dispatch_server(listener).await;
        let req = test_request();

        // (1) `anthropic-messages` → `POST /messages` + `x-api-key` +
        // NO `authorization`.
        let provider = build_provider(&model(&base_url, Some("anthropic-messages")));
        let events: Vec<ProviderEvent> = provider
            .complete(&req)
            .await
            .expect("the request sends")
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Done(FinishReason::Stop))),
            "the anthropic stream completes: {events:?}"
        );
        let (path, has_auth, has_x_api_key) =
            rx.recv().await.expect("the server recorded the request");
        // (the method line is lowercase on the wire — `post`)
        assert!(
            path.starts_with("post ") && path.contains("/messages"),
            "got {path:?}"
        );
        assert!(
            has_x_api_key,
            "the anthropic wire sends `x-api-key` ({path:?})"
        );
        assert!(
            !has_auth,
            "the anthropic wire sends NO `authorization` ({path:?})"
        );

        // (2) `openai-responses` → `POST /responses` + `authorization`.
        let provider = build_provider(&model(&base_url, Some("openai-responses")));
        let events: Vec<ProviderEvent> = provider
            .complete(&req)
            .await
            .expect("the request sends")
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Done(FinishReason::Stop))),
            "the responses stream completes: {events:?}"
        );
        let (path, has_auth, has_x_api_key) =
            rx.recv().await.expect("the server recorded the request");
        assert!(
            path.starts_with("post ") && path.contains("/responses"),
            "got {path:?}"
        );
        assert!(
            has_auth,
            "the responses wire sends `authorization` ({path:?})"
        );
        assert!(
            !has_x_api_key,
            "the responses wire sends NO `x-api-key` ({path:?})"
        );

        // (3) `None` (the default) → `POST /chat/completions` +
        // `authorization`.
        let provider = build_provider(&model(&base_url, None));
        let events: Vec<ProviderEvent> = provider
            .complete(&req)
            .await
            .expect("the request sends")
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Done(FinishReason::Stop))),
            "the openai-compatible stream completes: {events:?}"
        );
        let (path, has_auth, _has_x_api_key) =
            rx.recv().await.expect("the server recorded the request");
        assert!(
            path.starts_with("post ") && path.contains("/chat/completions"),
            "got {path:?}"
        );
        assert!(
            has_auth,
            "the openai-compatible wire sends `authorization` ({path:?})"
        );

        // (4) (ADR 0026) `litellm` → the OpenAI arm: `POST /chat/completions` +
        // `authorization` (it is a discovery mode, NOT a fourth wire).
        let provider = build_provider(&model(&base_url, Some("litellm")));
        let events: Vec<ProviderEvent> = provider
            .complete(&req)
            .await
            .expect("the request sends")
            .collect()
            .await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Done(FinishReason::Stop))),
            "the litellm (openai-compatible) stream completes: {events:?}"
        );
        let (path, has_auth, has_x_api_key) =
            rx.recv().await.expect("the server recorded the request");
        assert!(
            path.starts_with("post ") && path.contains("/chat/completions"),
            "got {path:?}"
        );
        assert!(
            has_auth,
            "the litellm api uses the openai-compatible wire: `authorization` ({path:?})"
        );
        assert!(
            !has_x_api_key,
            "the litellm api uses the openai-compatible wire: NO `x-api-key` ({path:?})"
        );

        // The loop-accept server never exits on its own — abort it (an
        // `await` would hang on the `accept()` loop).
        server.abort();
    }
}
