//! JSONL-over-stdio client for pi's RPC mode (`pi --mode rpc`).
//!
//! Wire format (verified against `@earendil-works/pi-coding-agent@0.87.1`
//! `dist/modes/rpc/rpc-types.d.ts` + `rpc-mode.js`): one JSON object per
//! line, bidirectional. Commands + `extension_ui_response` on stdin;
//! `response` + session events on stdout. Async correlation by the command's
//! optional `id`, echoed in the matching `response`
//! (`rpc-mode.js:293`: `const id = command.id`).
//!
//! - success: `{ id, type: "response", command, success: true, data }`
//! - error:   `{ id, type: "response", command, success: false, error: string }`
//!
//! Payload field names are **camelCase** (only the `type`/`method`
//! discriminators are snake_case; `set_editor_text` is a snake_case method —
//! pi's own casing is inconsistent, verified in `rpc-types.d.ts:385-453`).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent::errors::RpcError;

/// A parsed stdout line that is not a `response` (a session event).
///
/// One variant per event in the `agent_settled`/`assistantMessageEvent`
/// catalog (research §F1), with the exact camelCase wire field names
/// (serde `rename_all = "camelCase"` on the enum renames the variant field
/// names; the `set_editor_text` method keeps its snake_case wire name via an
/// explicit variant `rename`).
///
/// Deserialization is **permissive of unknown events**: the reader parses
/// each line as `Value` first, routes by `type`, and deserializes known types
/// into their variants; an unknown `type` becomes `Unknown { raw }` (never an
/// error — the RPC surface is unversioned and pi will add events).
// NOTE: the `type` discriminators are snake_case on the wire (only the
// PAYLOAD field names are camelCase) — so the variant names ARE the wire
// names, and `rename_all = "camelCase"` is applied PER-VARIANT (to the
// fields only). An enum-level `rename_all` would rename the tag values too
// (`AgentSettled` -> `agentSettled`) and every event would fall through to
// `unknown`.
// The variant names are intentionally the wire names (snake_case) — the
// `type` discriminators are snake_case on the wire and the variant name IS
// the tag value (an enum-level `rename_all` would corrupt it), so the
// casing lint is suppressed for this type.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "type")]
pub enum RpcEvent {
    agent_start,
    #[serde(rename_all = "camelCase")]
    agent_end {
        messages: Vec<Value>,
        will_retry: bool,
    },
    agent_settled,
    turn_start,
    #[serde(rename_all = "camelCase")]
    turn_end {
        message: Value,
        tool_results: Vec<Value>,
    },
    #[serde(rename_all = "camelCase")]
    message_start {
        message: Value,
    },
    #[serde(rename_all = "camelCase")]
    message_end {
        message: Value,
    },
    #[serde(rename_all = "camelCase")]
    message_update {
        usage: Value,
        assistant_message_event: Value,
    },
    #[serde(rename_all = "camelCase")]
    text_start {
        content_index: u64,
    },
    #[serde(rename_all = "camelCase")]
    text_delta {
        content_index: u64,
        delta: String,
    },
    #[serde(rename_all = "camelCase")]
    text_end {
        content_index: u64,
        content: String,
    },
    #[serde(rename_all = "camelCase")]
    thinking_start {
        content_index: u64,
    },
    #[serde(rename_all = "camelCase")]
    thinking_delta {
        content_index: u64,
        delta: String,
    },
    #[serde(rename_all = "camelCase")]
    thinking_end {
        content_index: u64,
        content: String,
    },
    #[serde(rename_all = "camelCase")]
    toolcall_start {
        content_index: u64,
        id: String,
        tool_name: String,
    },
    #[serde(rename_all = "camelCase")]
    toolcall_delta {
        content_index: u64,
        delta: String,
    },
    #[serde(rename_all = "camelCase")]
    toolcall_end {
        tool_call: Value,
    },
    #[serde(rename_all = "camelCase")]
    tool_execution_start {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    #[serde(rename_all = "camelCase")]
    tool_execution_update {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        partial_result: Value,
    },
    #[serde(rename_all = "camelCase")]
    tool_execution_end {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        is_error: bool,
    },
    #[serde(rename_all = "camelCase")]
    queue_update {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    entry_appended {
        entry: Value,
    },
    #[serde(rename_all = "camelCase")]
    session_info_changed {
        name: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    thinking_level_changed {
        level: String,
    },
    #[serde(rename_all = "camelCase")]
    compaction_start {
        reason: String,
    },
    #[serde(rename_all = "camelCase")]
    compaction_end {
        reason: String,
        result: Option<Value>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    auto_retry_start {
        attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error_message: String,
    },
    #[serde(rename_all = "camelCase")]
    auto_retry_end {
        success: bool,
        attempt: u64,
        final_error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    summarization_retry_scheduled {
        attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error_message: String,
    },
    #[serde(rename_all = "camelCase")]
    summarization_retry_attempt_start {
        source: String,
        reason: Option<String>,
    },
    summarization_retry_finished,
    #[serde(rename_all = "camelCase")]
    bash_execution_update {
        id: Option<String>,
        delta: String,
    },
    #[serde(rename_all = "camelCase")]
    extension_error {
        extension_path: String,
        event: String,
        error: String,
    },
    /// An event `type` this version of the client doesn't know (permissive
    /// of unknown events — the raw line is preserved for logging/inspection).
    #[serde(rename_all = "camelCase")]
    unknown {
        raw: Value,
    },
}

impl RpcEvent {
    /// The wire `type` discriminator (for tests + the normalizer's match).
    pub fn kind(&self) -> std::borrow::Cow<'static, str> {
        match self {
            RpcEvent::agent_start => std::borrow::Cow::Borrowed("agent_start"),
            RpcEvent::agent_end { .. } => std::borrow::Cow::Borrowed("agent_end"),
            RpcEvent::agent_settled => std::borrow::Cow::Borrowed("agent_settled"),
            RpcEvent::turn_start => std::borrow::Cow::Borrowed("turn_start"),
            RpcEvent::turn_end { .. } => std::borrow::Cow::Borrowed("turn_end"),
            RpcEvent::message_start { .. } => std::borrow::Cow::Borrowed("message_start"),
            RpcEvent::message_end { .. } => std::borrow::Cow::Borrowed("message_end"),
            RpcEvent::message_update { .. } => std::borrow::Cow::Borrowed("message_update"),
            RpcEvent::text_start { .. } => std::borrow::Cow::Borrowed("text_start"),
            RpcEvent::text_delta { .. } => std::borrow::Cow::Borrowed("text_delta"),
            RpcEvent::text_end { .. } => std::borrow::Cow::Borrowed("text_end"),
            RpcEvent::thinking_start { .. } => std::borrow::Cow::Borrowed("thinking_start"),
            RpcEvent::thinking_delta { .. } => std::borrow::Cow::Borrowed("thinking_delta"),
            RpcEvent::thinking_end { .. } => std::borrow::Cow::Borrowed("thinking_end"),
            RpcEvent::toolcall_start { .. } => std::borrow::Cow::Borrowed("toolcall_start"),
            RpcEvent::toolcall_delta { .. } => std::borrow::Cow::Borrowed("toolcall_delta"),
            RpcEvent::toolcall_end { .. } => std::borrow::Cow::Borrowed("toolcall_end"),
            RpcEvent::tool_execution_start { .. } => {
                std::borrow::Cow::Borrowed("tool_execution_start")
            }
            RpcEvent::tool_execution_update { .. } => {
                std::borrow::Cow::Borrowed("tool_execution_update")
            }
            RpcEvent::tool_execution_end { .. } => std::borrow::Cow::Borrowed("tool_execution_end"),
            RpcEvent::queue_update { .. } => std::borrow::Cow::Borrowed("queue_update"),
            RpcEvent::entry_appended { .. } => std::borrow::Cow::Borrowed("entry_appended"),
            RpcEvent::session_info_changed { .. } => {
                std::borrow::Cow::Borrowed("session_info_changed")
            }
            RpcEvent::thinking_level_changed { .. } => {
                std::borrow::Cow::Borrowed("thinking_level_changed")
            }
            RpcEvent::compaction_start { .. } => std::borrow::Cow::Borrowed("compaction_start"),
            RpcEvent::compaction_end { .. } => std::borrow::Cow::Borrowed("compaction_end"),
            RpcEvent::auto_retry_start { .. } => std::borrow::Cow::Borrowed("auto_retry_start"),
            RpcEvent::auto_retry_end { .. } => std::borrow::Cow::Borrowed("auto_retry_end"),
            RpcEvent::summarization_retry_scheduled { .. } => {
                std::borrow::Cow::Borrowed("summarization_retry_scheduled")
            }
            RpcEvent::summarization_retry_attempt_start { .. } => {
                std::borrow::Cow::Borrowed("summarization_retry_attempt_start")
            }
            RpcEvent::summarization_retry_finished => {
                std::borrow::Cow::Borrowed("summarization_retry_finished")
            }
            RpcEvent::bash_execution_update { .. } => {
                std::borrow::Cow::Borrowed("bash_execution_update")
            }
            RpcEvent::extension_error { .. } => std::borrow::Cow::Borrowed("extension_error"),
            RpcEvent::unknown { raw } => raw
                .get("type")
                .and_then(|t| t.as_str())
                // The type string is borrowed from `raw` (which borrows from
                // `&self`) — an owned `String` is needed for the 'static
                // return (the literal arms coerce to `Cow::Borrowed`).
                .map(|s| std::borrow::Cow::Owned(s.to_string()))
                .unwrap_or_else(|| std::borrow::Cow::Borrowed("unknown")),
        }
    }
}

/// An `extension_ui_request` line on stdout: the agent-side extension is
/// asking the client (the desktop) to render an interactive UI.
///
/// 4 blocking dialogs (`select`/`confirm`/`input`/`editor` — the agent awaits
/// the response; an optional `timeout` auto-resolves) + 5 fire-and-forget
/// notifications (`notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text`
/// — no response expected).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum ExtensionUiRequest {
    Select {
        id: String,
        title: String,
        options: Vec<String>,
        timeout: Option<f64>,
    },
    Confirm {
        id: String,
        title: String,
        message: String,
        timeout: Option<f64>,
    },
    Input {
        id: String,
        title: String,
        placeholder: Option<String>,
        timeout: Option<f64>,
    },
    Editor {
        id: String,
        title: String,
        prefill: Option<String>,
    },
    Notify {
        id: String,
        message: String,
        notify_type: Option<String>,
    },
    SetStatus {
        id: String,
        status_key: String,
        status_text: Option<String>,
    },
    SetWidget {
        id: String,
        widget_key: String,
        widget_lines: Option<Vec<String>>,
        widget_placement: Option<String>,
    },
    SetTitle {
        id: String,
        title: String,
    },
    /// Wire method is `set_editor_text` (snake_case — pi's own casing is
    /// inconsistent; the other methods are camelCase, verified in
    /// `rpc-types.d.ts:385-453`).
    #[serde(rename = "set_editor_text")]
    SetEditorText {
        id: String,
        text: String,
    },
}

/// The client's answer to an `extension_ui_request`. Three wire shapes:
/// `{type, id, value}` (select/input/editor — `value` is a REQUIRED string;
/// `undefined` is never sent), `{type, id, confirmed}` (confirm), and
/// `{type, id, cancelled: true}` (the user dismissed the dialog — pi treats
/// it as a negative/empty answer).
#[derive(Debug, Clone, PartialEq)]
pub enum ExtensionUiResponse {
    Value { id: String, value: String },
    Confirmed { id: String, confirmed: bool },
    Cancelled { id: String },
}

impl serde::Serialize for ExtensionUiResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // All three shapes carry the constant `type: "extension_ui_response"`
        // discriminator + `id`, differing only in the answer field — a
        // constant tag can't come from `#[serde(tag = "type")]` (which would
        // use the variant names), so the shape is built by hand.
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("type", "extension_ui_response")?;
        match self {
            ExtensionUiResponse::Value { id, value } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("value", value)?;
            }
            ExtensionUiResponse::Confirmed { id, confirmed } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("confirmed", confirmed)?;
            }
            ExtensionUiResponse::Cancelled { id } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("cancelled", &true)?;
            }
        }
        map.end()
    }
}

/// An event queue with a pluggable consumer: events received before the
/// consumer attaches (via `attach`) are BUFFERED; after attach, they are
/// forwarded live. A failed live send (the consumer dropped the receiver)
/// is the teardown signal (the session is going away).
///
/// This is load-bearing: the reader task starts before any consumer exists
/// (the first `send()` may complete — resolving its response — while the
/// consumer is still between "send returned" and "called `events()`"; an
/// event arriving in that window must not kill the reader).
struct QueuedEvents<T> {
    buffered: Vec<T>,
    consumer: Option<mpsc::UnboundedSender<T>>,
}

impl<T> QueuedEvents<T> {
    /// Deliver `event`: live if a consumer is attached (a failed send = the
    /// consumer dropped the receiver → `false` = stop reading), else buffered.
    fn deliver(&mut self, event: T) -> bool {
        match self.consumer.as_ref() {
            Some(tx) => tx.send(event).is_ok(),
            None => {
                self.buffered.push(event);
                true
            }
        }
    }

    /// Attach the consumer (once): buffer → live handoff, in order.
    fn attach(&mut self, tx: mpsc::UnboundedSender<T>) {
        assert!(self.consumer.is_none(), "attach called more than once");
        self.consumer = Some(tx.clone());
        for ev in self.buffered.drain(..) {
            let _ = tx.send(ev); // the receiver is alive — cannot fail
        }
    }
}

/// Shared inner state of a spawned pi RPC child (behind an `Arc`, so
/// `PiRpcHandle` is a cheap clone).
struct Inner {
    /// stdin writer (closing it = the clean-shutdown signal: pi's
    /// `onInputEnd` → `shutdown()` → exit).
    stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    /// In-flight commands awaiting their `response`, keyed by `id`.
    pending: tokio::sync::Mutex<
        std::collections::HashMap<String, oneshot::Sender<Result<Value, RpcError>>>,
    >,
    /// The session-event queue (buffered until `events()` attaches the
    /// consumer — the driver task is the sole consumer; dropping its
    /// receiver tears the reader down).
    events: std::sync::Mutex<QueuedEvents<RpcEvent>>,
    /// The extension-UI request queue (same attach pattern).
    extension_ui: std::sync::Mutex<QueuedEvents<ExtensionUiRequest>>,
    /// Exit watch: `None` while the child runs, `Some(code)` after exit
    /// (published by the exit-watcher task — the only one with the real code).
    exited: watch::Sender<Option<i32>>,
    /// The child process (drained by the exit-watcher task).
    child: tokio::sync::Mutex<Option<tokio::process::Child>>,
}

/// Owns the child process. Created at the spawn site; dropped when the spawn
/// site returns — ALL lifecycle operations go through the handle.
pub struct PiRpc {
    inner: Arc<Inner>,
}

impl PiRpc {
    /// Spawn `program` with `args`/`env`/`cwd`. `env` is ADDITIVE (the
    /// child inherits the desktop's environment — the real `pi` needs the
    /// user's API keys / `PATH` from it — then the caller's map is layered
    /// on top: the registry entry's env + the bridge + the gate) and is
    /// typed `&BTreeMap<String, String>` to match the existing
    /// `AgentEntry.env` / `bridge_spawn_setup` flow.
    pub fn spawn(
        program: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, RpcError> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(cwd);
        // ADDITIVE: start from the desktop's own environment, then layer
        // the caller's map on top. (`Command::envs` alone would REPLACE the
        // inherited environment — the child would lose the user's API
        // keys and `PATH`, and the real `pi` could not authenticate.)
        let mut child_env: BTreeMap<String, String> = std::env::vars().collect();
        for (k, v) in env.iter() {
            child_env.insert(k.clone(), v.clone());
        }
        cmd.envs(&child_env);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        // stderr is inherited (diagnostics go to the desktop's console; the
        // RPC protocol is stdout-only).
        let mut child = cmd.spawn().map_err(|e| RpcError::Spawn(e.to_string()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RpcError::Io("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RpcError::Io("no stdout".into()))?;

        let (exited_tx, _) = watch::channel(None);

        let inner = Arc::new(Inner {
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            pending: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            events: std::sync::Mutex::new(QueuedEvents {
                buffered: Vec::new(),
                consumer: None,
            }),
            extension_ui: std::sync::Mutex::new(QueuedEvents {
                buffered: Vec::new(),
                consumer: None,
            }),
            exited: exited_tx.clone(),
            child: tokio::sync::Mutex::new(Some(child)),
        });

        // Reader task: one JSON object per line. `response` lines resolve the
        // matching pending command (by `id`); `extension_ui_request` lines go
        // to the extension-UI channel; everything else is a session event. A
        // malformed JSON line is a `Parse` error: the reader fails ALL
        // in-flight sends and stops (the protocol is strict JSONL — a broken
        // line means the stream is untrustworthy).
        let reader_inner = Arc::clone(&inner);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let v: Value = match serde_json::from_str(&line) {
                            Ok(v) => v,
                            Err(e) => {
                                fail_pending(&reader_inner, RpcError::Parse(e.to_string())).await;
                                return;
                            }
                        };
                        let Some(t) = v.get("type").and_then(|t| t.as_str()) else {
                            continue;
                        };
                        if t == "response" {
                            let Some(id) = v.get("id").and_then(|i| i.as_str()) else {
                                // A `response` without an `id` is not
                                // correlatable (pi only echoes `id` when the
                                // command carried one) — nothing to resolve.
                                continue;
                            };
                            let Some(slot) = reader_inner.pending.lock().await.remove(id) else {
                                continue;
                            };
                            let result = if v.get("success").and_then(|s| s.as_bool()) == Some(true)
                            {
                                Ok(v.get("data").cloned().unwrap_or(Value::Null))
                            } else {
                                let msg = v
                                    .get("error")
                                    .and_then(|e| e.as_str())
                                    .unwrap_or("unknown error")
                                    .to_string();
                                Err(RpcError::Command { error: msg })
                            };
                            let _ = slot.send(result);
                        } else if t == "extension_ui_request" {
                            if let Ok(req) = serde_json::from_value::<ExtensionUiRequest>(v) {
                                // A failed live send = the consumer dropped the
                                // receiver (teardown); a pre-attach event is
                                // buffered (the reader keeps reading).
                                if !reader_inner.extension_ui.lock().unwrap().deliver(req) {
                                    return;
                                }
                            }
                        } else {
                            // Known event types deserialize into their variant;
                            // an unknown `type` becomes `Unknown { raw }`
                            // (permissive — pi will add events).
                            let event = match serde_json::from_value::<RpcEvent>(v.clone()) {
                                Ok(ev) => ev,
                                Err(_) => RpcEvent::unknown { raw: v },
                            };
                            // A failed live send = the consumer dropped the
                            // receiver (teardown); a pre-attach event is
                            // buffered (the reader keeps reading).
                            let delivered = reader_inner.events.lock().unwrap().deliver(event);
                            if !delivered {
                                return;
                            }
                        }
                    }
                    Ok(None) => break, // EOF (child exited)
                    Err(_) => break,   // read error (child exited)
                }
            }
            // Stream ended (child exited or stdout closed): fail everything
            // still in flight. (The exit CODE is published by the
            // exit-watcher task, which is the only one with it.)
            fail_pending(&reader_inner, RpcError::ProcessExited(None)).await;
        });

        // Exit-watcher task: reaps the child, publishes the real exit code,
        // and fails any in-flight commands the reader didn't already fail
        // (e.g. the child died before its stdout drained).
        let watcher_inner = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut child = watcher_inner
                .child
                .lock()
                .await
                .take()
                .expect("child moved exactly once");
            let status = child.wait().await;
            // A signal-killed child has NO exit code (`code()` is
            // `None`); publish the shell convention (128 + signal) so a
            // `Some` value always means "exited" (`None` = still running
            // — the `send()` error mapping relies on the distinction).
            let code = match status {
                Ok(s) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        s.code().or_else(|| s.signal().map(|sig| 128 + sig))
                    }
                    #[cfg(not(unix))]
                    {
                        s.code()
                    }
                }
                Err(_) => None,
            };
            fail_pending(&watcher_inner, RpcError::ProcessExited(code)).await;
            let _ = watcher_inner.exited.send(code);
        });

        Ok(PiRpc { inner })
    }

    /// A cheap-clonable handle over the shared inner state. The driver task,
    /// the `LiveSession` entry, and command paths all hold clones — mirroring
    /// how `cx: ConnectionTo<Agent>` was a cheap clone passed around today.
    pub fn handle(&self) -> PiRpcHandle {
        PiRpcHandle {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Fail every in-flight command with `err` (idempotent — only the first
/// failure per `id` lands).
async fn fail_pending(inner: &Arc<Inner>, err: RpcError) {
    let mut pending = inner.pending.lock().await;
    for (_, slot) in pending.drain() {
        let _ = slot.send(Err(err.clone()));
    }
}

/// A cheap-clonable handle over a spawned pi RPC child (see `PiRpc::handle`).
#[derive(Clone)]
pub struct PiRpcHandle {
    inner: Arc<Inner>,
}

impl PiRpcHandle {
    /// Send a command (a JSON object carrying a `type` discriminator; e.g.
    /// `{"type": "get_state"}`). An `id` is attached when absent and the
    /// result is awaited via the `response` that echoes it.
    ///
    /// Resolves with the `data` payload on `success: true`, or
    /// `RpcError::Command { error }` on `success: false`, or
    /// `RpcError::ProcessExited` / `RpcError::Io` when the child is gone /
    /// stdin is broken.
    pub async fn send(&self, command: Value) -> Result<Value, RpcError> {
        let mut cmd = command;
        let id = uuid::Uuid::new_v4().to_string();
        cmd["id"] = Value::String(id.clone());
        let line = serde_json::to_string(&cmd).map_err(|e| RpcError::Parse(e.to_string()))?;

        // A send after the child exited is `ProcessExited` (not `Io` — the
        // stdin write would also fail, but the exit is the meaningful cause).
        {
            let exited = self.inner.exited.borrow();
            if exited.is_some() {
                return Err(RpcError::ProcessExited(*exited));
            }
        }
        let (tx, rx) = oneshot::channel();
        // Register the pending slot BEFORE writing — the reader could
        // deliver the response between write and insert otherwise (and drop
        // it, hanging this await). A failed write removes the slot again.
        self.inner.pending.lock().await.insert(id.clone(), tx);
        let written: Result<(), RpcError> = {
            let mut stdin = self.inner.stdin.lock().await;
            match stdin.as_mut() {
                Some(s) => {
                    use tokio::io::AsyncWriteExt;
                    let payload = format!("{line}\n").into_bytes();
                    s.write_all(&payload)
                        .await
                        .map_err(|e| RpcError::Io(e.to_string()))
                }
                None => Err(RpcError::ProcessExited(None)),
            }
        };
        if let Err(e) = written {
            self.inner.pending.lock().await.remove(&id);
            // Prefer the exit over a raw write error when the child is gone.
            if matches!(e, RpcError::Io(_)) && self.inner.exited.borrow().is_some() {
                return Err(RpcError::ProcessExited(*self.inner.exited.borrow()));
            }
            return Err(e);
        }
        rx.await.unwrap_or(Err(RpcError::ProcessExited(None)))
    }

    /// Answer an `extension_ui_request` (written to stdin; the agent's
    /// `pendingExtensionRequests` map correlates it by `id`,
    /// `rpc-mode.js:620-626`).
    pub async fn respond_extension_ui(
        &self,
        response: ExtensionUiResponse,
    ) -> Result<(), RpcError> {
        let line = serde_json::to_string(&response).map_err(|e| RpcError::Parse(e.to_string()))?;
        {
            let exited = self.inner.exited.borrow();
            if exited.is_some() {
                return Err(RpcError::ProcessExited(*exited));
            }
        }
        let mut stdin = self.inner.stdin.lock().await;
        let stdin = stdin.as_mut().ok_or(RpcError::ProcessExited(None))?;
        use tokio::io::AsyncWriteExt;
        let payload = format!("{line}\n").into_bytes();
        let written = stdin
            .write_all(&payload)
            .await
            .map_err(|e| RpcError::Io(e.to_string()));
        if written.is_err() && self.inner.exited.borrow().is_some() {
            return Err(RpcError::ProcessExited(*self.inner.exited.borrow()));
        }
        written
    }

    /// The session-event channel. The consumer ATTACHES once — the driver
    /// task is the sole consumer; events that arrived before the attach are
    /// buffered and flushed in order; dropping the receiver tears the reader
    /// task down (a second attach is a bug).
    pub fn events(&self) -> mpsc::UnboundedReceiver<RpcEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.events.lock().unwrap().attach(tx);
        rx
    }

    /// The extension-UI request channel (same attach pattern).
    pub fn extension_ui(&self) -> mpsc::UnboundedReceiver<ExtensionUiRequest> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.extension_ui.lock().unwrap().attach(tx);
        rx
    }

    /// Clonable exit watch (resolves when the child exits, any cause):
    /// `None` while running, `Some(code)` after exit.
    pub fn exited(&self) -> watch::Receiver<Option<i32>> {
        self.inner.exited.subscribe()
    }

    /// Close stdin (pi: `onInputEnd` → `shutdown()` → clean exit), wait up to
    /// 5 s for the exit, then kill. IDEMPOTENT — callable from `close_session`
    /// AND the driver teardown (the `PiRpc` owner is dropped when the spawn
    /// site returns, so the handle is the only reachable close path).
    pub async fn close(&self) {
        // Dropping the `ChildStdin` closes the pipe's write side → pi sees
        // EOF on stdin (`onInputEnd` → `shutdown()` → clean exit). (tokio's
        // `AsyncWriteExt` has no `close()` in 1.51.)
        self.inner.stdin.lock().await.take();
        // Wait (version-based — the exit-watcher's single `send` bumps the
        // version even when the value stays `None`, which a VALUE-based
        // wait on `is_none()` would miss) for the exit, capped at 5 s.
        // A child that ALREADY exited before the `subscribe` (the value is
        // `Some` — including a signal kill's `Some(128+sig)`) skips the
        // wait entirely (its version already bumped before we subscribed,
        // so a bare `changed()` would block until the cap).
        let mut exited = self.inner.exited.subscribe();
        if exited.borrow().is_none() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), exited.changed()).await;
        }
        let mut child = self.inner.child.lock().await;
        if let Some(c) = child.as_mut() {
            // `start_kill` is sync (sends SIGKILL); `wait` reaps.
            let _ = c.start_kill();
            let _ = c.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    /// Unit tests CANNOT use `env!("CARGO_BIN_EXE_fake_pi")` (it is only set
    /// for integration tests — see the note at `subagent.rs:625-628`):
    /// construct the binary path and spawn it directly. No copy: these tests
    /// never reap processes by binary path (they kill via the child handle
    /// `PiRpc` owns), so a shared path cannot false-positive — and a copy
    /// races the kernel's ETXTBSY check (the copy's write-fd can still be
    /// in flight when the forked child execs, and the kernel rejects the
    /// exec while a write fd is open on the target inode).
    fn fake_pi_bin() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/fake_pi"))
    }

    fn spawn_fake_pi(dir: &std::path::Path, env: &[(&str, &str)]) -> (PiRpc, PiRpcHandle) {
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let rpc = PiRpc::spawn(fake_pi_bin().to_str().unwrap(), &[], &env, dir).unwrap();
        let handle = rpc.handle();
        (rpc, handle)
    }

    #[tokio::test]
    async fn spawn_get_state_roundtrip() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[]);
        let data = handle
            .send(serde_json::json!({ "type": "get_state" }))
            .await
            .unwrap();
        // The session id is UNIQUE per process (mirroring real pi); the
        // prefix is the stable part.
        assert!(
            data["sessionId"].as_str().unwrap().starts_with("fake-pi-"),
            "a fresh session gets a unique pi id, got {}",
            data["sessionId"]
        );
        assert_eq!(data["sessionFile"], "/tmp/fake-pi-session.jsonl");
        handle.close().await;
    }

    #[tokio::test]
    async fn failed_command() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[]);
        let err = handle
            .send(serde_json::json!({ "type": "__fail" }))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            RpcError::Command {
                error: "boom".into()
            }
        );
        handle.close().await;
    }

    #[tokio::test]
    async fn process_exit_fails_inflight() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[]);
        // Close stdin (clean exit) and WAIT for the exit before sending, so
        // the send is unambiguously post-exit.
        handle.close().await;
        let err = handle
            .send(serde_json::json!({ "type": "get_state" }))
            .await
            .unwrap_err();
        assert!(
            matches!(err, RpcError::ProcessExited(_)),
            "expected ProcessExited, got {err:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_commands() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[]);
        // Two in-flight sends resolve independently (correlation by `id`,
        // not order).
        let (state, models) = tokio::join!(
            handle.send(serde_json::json!({ "type": "get_state" })),
            handle.send(serde_json::json!({ "type": "get_available_models" })),
        );
        assert!(
            state.unwrap()["sessionId"]
                .as_str()
                .unwrap()
                .starts_with("fake-pi-"),
            "a fresh session gets a unique pi id"
        );
        assert_eq!(models.unwrap()["models"].as_array().unwrap().len(), 2);
        handle.close().await;
    }

    #[tokio::test]
    async fn prompt_event_stream() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[("FAKE_PI_PROMPT", "1")]);
        // The `prompt` response arrives after preflight (start of turn); the
        // events stream after it.
        let _ = handle
            .send(serde_json::json!({ "type": "prompt", "message": "hi" }))
            .await
            .unwrap();
        eprintln!("[test] send(prompt) returned; calling events()");
        let mut events = handle.events();
        eprintln!("[test] events() attached");
        let mut kinds = Vec::new();
        let mut deltas = Vec::new();
        loop {
            eprintln!("[test] waiting for next event (recv #{});", kinds.len() + 1);
            let ev = events.recv().await.expect("event stream ended early");
            eprintln!("[test] got event: {}", ev.kind());
            kinds.push(ev.kind());
            if let RpcEvent::message_update {
                usage,
                assistant_message_event: ame,
            } = &ev
            {
                // `usage` is REQUIRED on the wire (not optional).
                assert!(usage.is_object(), "usage must be an object");
                if ame.get("type").is_some_and(|t| t == "text_delta") {
                    assert!(ame.get("contentIndex").is_some());
                    deltas.push(
                        ame.get("delta")
                            .and_then(|d| d.as_str())
                            .unwrap()
                            .to_string(),
                    );
                }
            }
            if matches!(ev, RpcEvent::agent_settled) {
                break;
            }
        }
        assert_eq!(
            kinds,
            vec![
                "message_start",
                "message_start",
                "message_update",
                "message_update",
                "message_end",
                "agent_end",
                "agent_settled"
            ]
            .into_iter()
            .map(std::borrow::Cow::Borrowed)
            .collect::<Vec<_>>(),
        );
        assert_eq!(deltas, vec!["Hel", "lo"]);
        handle.close().await;
    }

    #[tokio::test]
    async fn extension_ui_roundtrip() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[("FAKE_PI_GATE", "1")]);
        let _ = handle
            .send(serde_json::json!({ "type": "prompt", "message": "hi" }))
            .await
            .unwrap();
        let req = handle
            .extension_ui()
            .recv()
            .await
            .expect("no extension_ui_request");
        assert_eq!(
            req,
            ExtensionUiRequest::Confirm {
                id: "gate-1".into(),
                title: "Allow bash?".into(),
                message: "ls -la".into(),
                timeout: None,
            }
        );
        handle
            .respond_extension_ui(ExtensionUiResponse::Confirmed {
                id: "gate-1".into(),
                confirmed: true,
            })
            .await
            .unwrap();
        // After the answer, the turn's event sequence follows and settles.
        let mut events = handle.events();
        let mut settled = false;
        while let Some(ev) = events.recv().await {
            if matches!(ev, RpcEvent::agent_settled) {
                settled = true;
                break;
            }
        }
        assert!(settled, "agent_settled never arrived after the gate answer");
        handle.close().await;
    }

    #[tokio::test]
    async fn unknown_event_is_not_an_error() {
        let dir = temp_dir();
        let (_rpc, handle) = spawn_fake_pi(&dir, &[("FAKE_PI_UNKNOWN", "1")]);
        // fake_pi (FAKE_PI_UNKNOWN=1) emits `{"type":"brand_new_event","foo":1}`
        // with the turn's events before settling — so drive a turn first.
        let _ = handle
            .send(serde_json::json!({ "type": "prompt", "message": "hi" }))
            .await
            .unwrap();
        let mut events = handle.events();
        let mut saw_unknown = false;
        loop {
            let ev = events.recv().await.expect("event stream ended early");
            if let RpcEvent::unknown { raw } = &ev {
                assert_eq!(raw["type"], "brand_new_event");
                assert_eq!(raw["foo"], 1);
                saw_unknown = true;
            }
            if matches!(ev, RpcEvent::agent_settled) {
                break;
            }
        }
        assert!(saw_unknown, "the unknown event was not received as Unknown");
        handle.close().await;
    }
}
