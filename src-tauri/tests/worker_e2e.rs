//! End-to-end Worker integration (ADR 0025 Task 7 — the architecture's
//! whole proof): a REAL `archimedes` binary (the
//! `env!("CARGO_BIN_EXE_archimedes")` — NOT the `fake_worker`) as a
//! Worker, driven by the REAL Supervisor code (the `WorkerManager`
//! production factory) against a CANNED provider (wiremock — the
//! `harness/provider.rs` tests' pattern: a mock HTTP server with canned
//! SSE streams).
//!
//! The full loop: Supervisor → spawn real Worker → real `AgentLoop` →
//! mock provider SSE → tool execution → events + store frames back →
//! the `TranscriptPersister` persists the transcript (COMPLETE — the
//! store-frame transport means the user / system / tool-role rows are
//! all there, so a resume of the e2e session would work).
//!
//! The e2e does NOT need the Tauri mock runtime: it drives the
//! `WorkerManager` directly (the `tests/session_native.rs` pattern) with
//! the production factory (`WorkerHandle::spawn(env!(…))` — the
//! `WorkerFactory` seam makes this direct).

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use archimedes_lib::agent::events::RpcEvent;
use archimedes_lib::agent::harness::catalog::{Model, ModelCatalog};
use archimedes_lib::agent::harness::prompt::build_child_system_message;
use archimedes_lib::agent::worker::client::{WorkerError, WorkerHandle, WorkerInboundEvent};
use archimedes_lib::agent::worker::manager::{WorkerFactory, WorkerManager};
use archimedes_lib::agent::worker::protocol::StartEnv;
use archimedes_lib::agent::{EventSink, PermissionOutcome, TranscriptPersister};
use archimedes_lib::storage::Db;
use serde_json::Value;
use tokio::sync::mpsc;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Shared helpers ───────────────────────────────────────────────────

/// The REAL-binary factory (the production factory shape — the `exe`
/// path is the built `archimedes` binary, NOT the `fake_worker`).
struct RealWorkerFactory;

impl WorkerFactory for RealWorkerFactory {
    fn spawn(&self) -> Result<WorkerHandle, WorkerError> {
        WorkerHandle::spawn(Path::new(env!("CARGO_BIN_EXE_archimedes")))
    }
}

/// A unique temp dir.
fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("worker-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A wiremock-backed OpenAI-compatible model (the `harness/provider.rs`
/// tests' pattern — the `Model` carries the provider config verbatim:
/// the `base_url` points at the mock server, a dummy `api_key`, the
/// `openai-completions` wire).
fn wire_model(server: &MockServer, id: &str) -> Model {
    Model {
        id: id.to_string(),
        provider: "fake".to_string(),
        base_url: server.uri(),
        api_key: "sk-test".to_string(),
        context_window: 128_000,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: false,
        thinking_levels: Vec::new(),
        api: Some("openai-completions".to_string()),
    }
}

/// Mount a canned `text/event-stream` response on the mock server (the
/// `tests/provider.rs` pattern).
async fn mount_sse(server: &MockServer, body: &str) {
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.to_string(), "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// Mount two canned streams: the `tool-result` turn's request (whose
/// body carries a `"role":"tool"` message) gets `second`, the FIRST
/// request (no tool message yet) gets `first`. Wiremock 0.6 routes an
/// overlapping match to the FIRST mounted mock, so the body-matched
/// mock is mounted first (the `expect(1)` constraint does NOT gate the
/// routing — a plain `POST` mock swallows every request).
async fn mount_two_request_streams(server: &MockServer, first: &str, second: &str) {
    // The tool-result turn (mounted FIRST — wins the overlapping match).
    Mock::given(method("POST"))
        .and(body_string_contains("\"role\":\"tool\""))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(second.to_string(), "text/event-stream"),
        )
        .mount(server)
        .await;
    // The first request (no tool message in the body yet).
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(first.to_string(), "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// A recording `EventSink` (the parent-scope sink test double — the
/// `subagent-session-started` / `subagent-closed` UI lifecycle events
/// land here).
#[derive(Clone)]
struct RecSink {
    tx: mpsc::UnboundedSender<(String, Value)>,
}

impl EventSink for RecSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

/// The e2e harness: a `WorkerManager` (the production factory — the
/// REAL binary) + a `TranscriptPersister` on a temp-dir `Db` + a
/// collecting `on_event` (an `mpsc` channel the test drains) + a
/// collecting `on_crash` + a parent-scope `RecSink`.
struct E2e {
    manager: WorkerManager,
    db: Arc<Db>,
    persister: Arc<TranscriptPersister>,
    events: mpsc::UnboundedReceiver<(String, bool, WorkerInboundEvent)>,
    crashes: mpsc::UnboundedReceiver<(String, Option<i32>)>,
    sink: mpsc::UnboundedReceiver<(String, Value)>,
    /// The CUMULATIVE collectors (every `wait` / `wait_sink` appends to
    /// these — a frame consumed by an earlier step's `wait` is still
    /// visible to a later step's assertions; a fresh local `Vec` would
    /// lose the pre-`permission-request` frames, e.g. the first model
    /// call's `tool_execution_start`).
    collected_events: Vec<(String, bool, WorkerInboundEvent)>,
    collected_sink: Vec<(String, Value)>,
}

impl E2e {
    async fn new() -> Self {
        let dir = temp_dir();
        let db: Arc<Db> = Arc::new(Db::open(&dir.join("e2e.db")).unwrap());
        let persister = Arc::new(TranscriptPersister::new(db.clone()));
        let persister_clone = Arc::clone(&persister);
        let (evt_tx, events) = mpsc::unbounded_channel();
        let (crash_tx, crashes) = mpsc::unbounded_channel();
        let (sink_tx, sink) = mpsc::unbounded_channel();
        let manager = WorkerManager::new(
            Arc::new(RealWorkerFactory),
            Arc::new(move |sid: String, code: Option<i32>| {
                let _ = crash_tx.send((sid, code));
            }),
            Arc::new(
                move |sid: String, is_subagent: bool, evt: WorkerInboundEvent| {
                    // The Supervisor's sole-writer persistence (the store
                    // frames applied to the temp-dir `Db` — the `TranscriptPersister`).
                    if let WorkerInboundEvent::Store(frame) = &evt {
                        persister_clone.apply(is_subagent, frame);
                    }
                    let _ = evt_tx.send((sid, is_subagent, evt));
                },
            ),
            Arc::new(move |_id: &str| {
                Some(Arc::new(RecSink {
                    tx: sink_tx.clone(),
                }) as Arc<dyn EventSink>)
            }),
        );
        Self {
            manager,
            db,
            persister,
            events,
            crashes,
            sink,
            collected_events: Vec::new(),
            collected_sink: Vec::new(),
        }
    }

    /// Attach a session (a REAL Worker) with the given `StartEnv` (the
    /// `TranscriptPersister`'s `set_cwd` — the `ensure_session_row`
    /// source).
    async fn attach(&self, env: StartEnv) {
        self.persister.set_cwd(&env.session_id, &env.cwd);
        self.manager
            .attach(&env.session_id, &env)
            .await
            .expect("the attach completes (ready handshake + start)");
    }

    /// Drain the `on_event` collector until `pred` matches (bounded — a
    /// stall fails the test with the partial list). Frames accumulate in
    /// `collected_events` (every `wait` sees the full history).
    async fn wait(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&[(String, bool, WorkerInboundEvent)]) -> bool,
    ) -> Result<Vec<(String, bool, WorkerInboundEvent)>, Vec<(String, bool, WorkerInboundEvent)>>
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if pred(&self.collected_events) {
                return Ok(self.collected_events.clone());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(self.collected_events.clone());
            }
            match tokio::time::timeout(remaining, self.events.recv()).await {
                Ok(Some(e)) => self.collected_events.push(e),
                Ok(None) | Err(_) => return Err(self.collected_events.clone()),
            }
        }
    }

    /// Drain the parent-scope sink collector until `pred` matches
    /// (bounded). Frames accumulate in `collected_sink` (every
    /// `wait_sink` sees the full history).
    async fn wait_sink(
        &mut self,
        timeout: Duration,
        pred: impl Fn(&[(String, Value)]) -> bool,
    ) -> Result<Vec<(String, Value)>, Vec<(String, Value)>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if pred(&self.collected_sink) {
                return Ok(self.collected_sink.clone());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(self.collected_sink.clone());
            }
            match tokio::time::timeout(remaining, self.sink.recv()).await {
                Ok(Some(e)) => self.collected_sink.push(e),
                Ok(None) | Err(_) => return Err(self.collected_sink.clone()),
            }
        }
    }
}

/// The `agent_message_chunk` texts among the collected frames'
/// `SinkFrame` `session-update` payloads (the UI contract stream — step
/// (d) asserts on these, so the settle barrier waits on them too).
fn sink_chunk_texts(evs: &[(String, bool, WorkerInboundEvent)]) -> Vec<String> {
    evs.iter()
        .filter_map(|(_, _, e)| match e {
            WorkerInboundEvent::SinkFrame { event, payload } if event == "session-update" => {
                payload.get("update").cloned()
            }
            _ => None,
        })
        .filter(|u| u.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk"))
        .filter_map(|u| {
            u.get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Poll the transcript rows until `want` rows are persisted (bounded).
/// The `native_messages` rows are written by the `TranscriptPersister`
/// from the WORKER's store frames, which arrive on their own schedule —
/// the `agent_settled` RpcEvent does NOT imply the last message's row has
/// landed, so a read right after the settle races it.
async fn wait_for_rows(db: &Db, session_id: &str, want: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = db
            .load_native_messages(session_id)
            .expect("the transcript loads");
        if rows.len() >= want || tokio::time::Instant::now() >= deadline {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The `RpcEvent` kinds among the collected frames (in order).
fn event_kinds(evs: &[(String, bool, WorkerInboundEvent)]) -> Vec<Cow<'static, str>> {
    evs.iter()
        .filter_map(|(_, _, e)| match e {
            WorkerInboundEvent::Event(ev) => Some(ev.kind()),
            _ => None,
        })
        .collect()
}

/// The `tool_execution_end` (name-filtered) among the collected frames:
/// `(result, is_error)`.
fn tool_execution_end(
    evs: &[(String, bool, WorkerInboundEvent)],
    name: Option<&str>,
) -> Option<(Value, bool)> {
    evs.iter().find_map(|(_, _, e)| match e {
        WorkerInboundEvent::Event(RpcEvent::tool_execution_end {
            tool_name,
            result,
            is_error,
            ..
        }) if name.is_none_or(|n| tool_name == n) => Some((result.clone(), *is_error)),
        _ => None,
    })
}

/// The `StartEnv` builder (the e2e's common shape: a wiremock provider
/// model + a catalog of the given models + a temp `config_dir`; the
/// `enabled_tools` rides the HARNESS convention verbatim — `Some(v)` =
/// exactly `v`). NO `settings.json` is written, so the Worker runs on the
/// all-`Allow` file-access default (ADR 0030).
fn start_env(
    session_id: &str,
    cwd: &Path,
    model: Model,
    catalog: ModelCatalog,
    trusted: bool,
    enabled_tools: Option<Vec<String>>,
    subagent_enabled: bool,
) -> StartEnv {
    start_env_with_file_policy(
        session_id,
        cwd,
        model,
        catalog,
        trusted,
        enabled_tools,
        subagent_enabled,
        None,
    )
}

/// The `StartEnv` builder with a file-access policy: the `settings.json`
/// is written into the temp `config_dir` BEFORE the env is returned, so the
/// Worker's per-turn read (ADR 0030 Deviation 4) sees the pinned policy from
/// its first tool call. `None` = no file at all (the default).
// The Worker's start envelope IS 8 positional knobs; a struct would obscure
// which `StartEnv` field each test pins (and this mirrors `start_env`).
#[allow(clippy::too_many_arguments)]
fn start_env_with_file_policy(
    session_id: &str,
    cwd: &Path,
    model: Model,
    catalog: ModelCatalog,
    trusted: bool,
    enabled_tools: Option<Vec<String>>,
    subagent_enabled: bool,
    file_policy: Option<archimedes_lib::agent::policy::FilePolicy>,
) -> StartEnv {
    let config_dir = temp_dir();
    if let Some(policy) = file_policy {
        std::fs::write(
            config_dir.join("settings.json"),
            format!(
                r#"{{ "filePolicy": {} }}"#,
                serde_json::to_string(&policy).expect("a FilePolicy serializes")
            ),
        )
        .expect("the settings file is written");
    }
    StartEnv::from_parts(
        session_id.to_string(),
        cwd.display().to_string(),
        archimedes_lib::agent::worker::protocol::StartMode::Fresh,
        None,
        model,
        catalog,
        None,
        trusted,
        enabled_tools,
        config_dir.display().to_string(),
        subagent_enabled,
        None,
    )
}

// ── (1) The full turn ────────────────────────────────────────────────

/// A REAL `archimedes --worker` through a full turn (mock provider SSE
/// → tool call → final answer) with the events + the COMPLETE persisted
/// transcript asserted (system + user + assistant + tool-role rows — a
/// resume of the session would work) + the `SinkFrame` UI contract +
/// the clean-exit bookkeeping.
#[tokio::test]
async fn full_turn_real_worker_canned_provider() {
    let mut h = E2e::new().await;
    let server = MockServer::start().await;
    // The canned `chat.completions` stream: a few `text_delta`s + a
    // `toolcall` + `finish_reason: tool_calls` (request 1), then a
    // second request for the tool-result turn → a final `stop`.
    mount_two_request_streams(
        &server,
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"echo hi\\\"}\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
         data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;
    let cwd = temp_dir();
    let env = start_env(
        "e2e-full",
        &cwd,
        wire_model(&server, "e2e-model"),
        ModelCatalog {
            models: vec![wire_model(&server, "e2e-model")],
            ..Default::default()
        },
        true, // a trusted Space (no permission prompt)
        Some(vec!["bash".to_string()]),
        true,
    );
    h.attach(env).await;

    // (a) The `ready` handshake is completed by `attach` ITSELF (the
    // client's `wait_ready` — the `ready` watch; the pump subscribes
    // AFTER `Ready`, so the first `on_event` frame is a startup frame,
    // NOT `Ready` — the `context_usage_update` `SinkFrame`).
    let first = tokio::time::timeout(Duration::from_secs(15), h.events.recv())
        .await
        .expect("a startup frame arrives within the bound")
        .expect("the collector is live");
    let (sid, is_subagent, _) = &first;
    assert_eq!(sid, "e2e-full");
    assert!(!is_subagent);

    // (b) The prompt.
    h.manager
        .handle_for("e2e-full")
        .expect("the session is attached")
        .send_prompt("run it", &[])
        .expect("the prompt is sent");

    // (c) The `RpcEvent` bookkeeping stream (in order, up to
    // `agent_settled`) — and, in the SAME drain, the final message's
    // `agent_message_chunk` sink frames.
    //
    // Waiting on `agent_settled` ALONE is a race: the two frame kinds
    // reach stdout over SEPARATE paths — a `SinkFrame` / store frame rides
    // the main loop's `outbound_rx` arm (`run_worker_inner`), an
    // `RpcEvent` rides the spawned pump task — and although both take the
    // same stdout mutex, nothing orders one against the other. The settle
    // can therefore be written BEFORE the last message's chunks that the
    // loop emitted first. So the trigger waits for the data step (d)
    // asserts on, and `rows` below waits for what (e) asserts on.
    let evs = h
        .wait(Duration::from_secs(60), |evs| {
            let settled = evs
                .iter()
                .any(|(_, _, e)| matches!(e, WorkerInboundEvent::Event(ev) if ev.kind() == "agent_settled"));
            settled && sink_chunk_texts(evs).iter().any(|t| t == "done")
        })
        .await
        .expect("the turn settles within the bound");
    let kinds = event_kinds(&evs);
    let pos = |k: &str| kinds.iter().position(|x| x == k);
    assert!(pos("turn_start").is_some(), "a turn_start, got {kinds:?}");
    assert!(
        pos("message_start").is_some(),
        "a message_start, got {kinds:?}"
    );
    assert!(
        pos("message_update").is_some(),
        "a message_update, got {kinds:?}"
    );
    let t_start = pos("tool_execution_start").expect("a tool_execution_start, got {kinds:?}");
    let t_end = pos("tool_execution_end").expect("a tool_execution_end, got {kinds:?}");
    let settled = pos("agent_settled").expect("an agent_settled, got {kinds:?}");
    assert!(
        t_start < t_end,
        "tool_execution_start before tool_execution_end"
    );
    assert!(t_end < settled, "the tool execution ends before the settle");
    // The `tool_execution_end` (a real `exec_bash` on the temp cwd —
    // the trusted Space auto-approved the gate).
    let (result, is_error) =
        tool_execution_end(&evs, Some("bash")).expect("a bash tool_execution_end");
    assert!(!is_error, "the bash tool succeeded");
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("hi"), "the bash output, got {text:?}");

    // (d) The `SinkFrame` `session-update` frames on the UI side (the
    // frontend contract — verbatim): the `agent_message_chunk` frames
    // carry the streamed text ("Hel" + "lo" for the first message,
    // "done" for the final) + the `tool-call` display frames (the
    // `tool_call` / `tool_call_update` for `call_1`, the final update
    // `status: "completed"`).
    let chunks: Vec<Value> = evs
        .iter()
        .filter_map(|(_, _, e)| match e {
            WorkerInboundEvent::SinkFrame { event, payload } if event == "session-update" => {
                payload.get("update").cloned()
            }
            _ => None,
        })
        .collect();
    let texts: Vec<String> = chunks
        .iter()
        .filter(|u| u.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk"))
        .filter_map(|u| {
            u.get("content")
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        texts.iter().any(|t| t == "Hel") && texts.iter().any(|t| t == "lo"),
        "the first message's chunks, got {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "done"),
        "the final message's chunk, got {texts:?}"
    );
    let tool_frames: Vec<Value> = chunks
        .iter()
        .filter(|u| {
            matches!(
                u.get("sessionUpdate").and_then(Value::as_str),
                Some("tool_call") | Some("tool_call_update")
            )
        })
        .cloned()
        .collect();
    assert!(
        tool_frames
            .iter()
            .any(|u| u.get("toolCallId").and_then(Value::as_str) == Some("call_1")),
        "a tool-call frame for call_1, got {tool_frames:?}"
    );
    assert!(
        tool_frames
            .iter()
            .any(|u| u.get("status").and_then(Value::as_str) == Some("completed")),
        "the tool_call_update with status completed, got {tool_frames:?}"
    );

    // (e) The `native_messages` rows — the COMPLETE transcript (the
    // store frames applied by the `TranscriptPersister`): the system
    // prompt (seq 0 — the Worker's `build_main_prompt` +
    // `prepend_system`), the user message, the assistant message with
    // the `tool_calls`, the tool-role message with the result, the
    // final assistant text. A resume of the session would work.
    let rows = wait_for_rows(&h.db, "e2e-full", 5).await;
    assert_eq!(
        rows.len(),
        5,
        "system + user + assistant(tool_calls) + tool + final assistant, got {rows:?}"
    );
    let v: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::from_str(r).expect("the row is JSON"))
        .collect();
    assert_eq!(v[0]["role"], "system", "seq 0 is the system prompt");
    let system_text = v[0]["content"].as_str().unwrap_or_default();
    assert!(
        system_text.contains("<tools>"),
        "the main prompt (build_main_prompt) has a <tools> section, got {system_text:?}"
    );
    // The `<tools>` section matches the `tools[]` API param (the
    // `enabled_tools`-filtered advertised specs — the in-process start
    // set the enabled tools BEFORE building the main prompt): the
    // enabled `bash` is advertised, the OTHER tools are NOT.
    let tools_section = system_text
        .split_once("<tools>")
        .and_then(|(_, rest)| rest.split_once("</tools>"))
        .map(|(s, _)| s)
        .unwrap_or_default();
    assert!(
        tools_section.contains("bash"),
        "the enabled bash tool is advertised, got {tools_section:?}"
    );
    assert!(
        !tools_section.contains("read") && !tools_section.contains("subagent"),
        "the <tools> section is the enabled_tools-filtered set (NOT all the tools), got {tools_section:?}"
    );
    assert_eq!(v[1]["role"], "user");
    assert_eq!(v[1]["content"], "run it", "the user message verbatim");
    assert_eq!(v[2]["role"], "assistant");
    assert_eq!(
        v[2]["tool_calls"][0]["name"], "bash",
        "the assistant's tool_calls round-trip"
    );
    assert_eq!(v[3]["role"], "tool");
    assert_eq!(
        v[3]["tool_call_id"], "call_1",
        "the tool result pairs with the call"
    );
    let tool_text = v[3]["content"]
        .as_array()
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        tool_text.contains("hi"),
        "the tool result round-trips, got {tool_text:?}"
    );
    assert_eq!(v[4]["role"], "assistant");
    assert_eq!(v[4]["content"], "done", "the final assistant text");

    // (f) The `messages` display rows (the `agent-text` / `tool-call`
    // upserts — the tool result merged into the tool-call row via
    // `tool_call_update`).
    let display =
        h.db.messages_for("e2e-full")
            .expect("the display rows load");
    let kinds: Vec<&str> = display.iter().map(|r| r.kind.as_str()).collect();
    assert!(
        kinds.contains(&"agent-text"),
        "an agent-text row, got {kinds:?}"
    );
    let agent_rows: Vec<Value> = display
        .iter()
        .filter(|r| r.kind == "agent-text")
        .map(|r| serde_json::from_str(&r.payload_json).expect("the payload is JSON"))
        .collect();
    assert!(
        agent_rows
            .iter()
            .any(|p| p.get("text").and_then(Value::as_str) == Some("Hello")),
        "the accumulated first message (Hel + lo), got {agent_rows:?}"
    );
    let tool_row = display
        .iter()
        .find(|r| r.kind == "tool-call" && r.message_key.as_deref() == Some("call_1"))
        .expect("the tool-call row for call_1");
    let tool_payload: Value =
        serde_json::from_str(&tool_row.payload_json).expect("the payload is JSON");
    assert_eq!(
        tool_payload.get("status").and_then(Value::as_str),
        Some("completed"),
        "the tool result merged into the tool-call row, got {tool_payload:?}"
    );

    // (g) A clean `close` → exit 0 + the `session-closed` bookkeeping
    // (the `Exited` EXPECTED path — the `on_crash` callback did NOT
    // fire).
    let handle = h
        .manager
        .detach("e2e-full")
        .expect("the session is attached");
    let code = tokio::time::timeout(Duration::from_secs(15), handle.exited())
        .await
        .expect("the worker exits after the close");
    assert_eq!(code, Some(0), "a clean close exits 0, got {code:?}");
    assert!(
        h.crashes.try_recv().is_err(),
        "the on_crash callback did NOT fire on a clean close"
    );
}

// ── (2) The permission round-trip ────────────────────────────────────

/// The permission round-trip with the REAL Worker (the real `AgentLoop`
/// permission gate, the fail-closed `StaticTrustSource`): a non-trusted
/// Space + a canned `bash` tool call → the `PermissionRequest` outbound
/// (the FULL gate payload verbatim — the `request.options` list present)
/// → the test sends `PermissionResponse { outcome: Selected {
/// option_id: "allow" } }` → the tool-execution events follow.
#[tokio::test]
async fn permission_round_trip_real_worker() {
    let mut h = E2e::new().await;
    let server = MockServer::start().await;
    // The same wiremock stream as the full turn (a `bash` tool call,
    // then the tool-result turn's final `stop`).
    mount_two_request_streams(
        &server,
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"echo gated\\\"}\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
         data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;
    let cwd = temp_dir();
    let env = start_env_with_file_policy(
        "e2e-perm",
        &cwd,
        wire_model(&server, "e2e-model"),
        ModelCatalog {
            models: vec![wire_model(&server, "e2e-model")],
            ..Default::default()
        },
        false, // a NON-trusted Space (the gate prompts)
        Some(vec!["bash".to_string()]),
        true,
        // (ADR 0030) `ask` in every direction: the gate's decision is the
        // SETTINGS' now, and the default is all-`Allow` (no prompt at all),
        // so a permission round-trip test must pin `ask` explicitly —
        // otherwise the Worker would run the `bash` ungated and the
        // `PermissionRequest` below would never arrive.
        Some(archimedes_lib::agent::policy::FilePolicy {
            reads: archimedes_lib::agent::policy::AccessPolicy::Ask,
            writes: archimedes_lib::agent::policy::AccessPolicy::Ask,
            shell: archimedes_lib::agent::policy::AccessPolicy::Ask,
        }),
    );
    h.attach(env).await;
    // The `ready` handshake is completed by `attach` ITSELF (the
    // client's `wait_ready` — the `ready` watch; the pump subscribes
    // AFTER `Ready`, so it never arrives via `on_event`).
    h.manager
        .handle_for("e2e-perm")
        .expect("the session is attached")
        .send_prompt("run it", &[])
        .expect("the prompt is sent");

    // (a) The `PermissionRequest` outbound (the FULL gate payload
    // verbatim — the `request.options` list present; the `id` is the
    // payload's `requestId` — the tool call id).
    let collected = h
        .wait(Duration::from_secs(30), |evs| {
            evs.iter()
                .any(|(_, _, e)| matches!(e, WorkerInboundEvent::PermissionRequest { .. }))
        })
        .await
        .expect("the permission request arrives within the bound");
    let (sid, is_subagent, evt) = collected
        .into_iter()
        .find(|(_, _, e)| matches!(e, WorkerInboundEvent::PermissionRequest { .. }))
        .expect("a PermissionRequest frame");
    assert_eq!(sid, "e2e-perm");
    assert!(!is_subagent);
    let WorkerInboundEvent::PermissionRequest { id, payload } = evt else {
        panic!("unreachable")
    };
    assert_eq!(
        id, "call_1",
        "the id is the payload's requestId (the tool call id)"
    );
    assert_eq!(payload["sessionId"], "e2e-perm", "the payload's sessionId");
    assert_eq!(payload["requestId"], "call_1");
    assert_eq!(
        payload["request"]["toolCall"]["title"], "bash echo gated",
        "the title names the command (ADR 0030: a prompt names the thing being approved), got {payload:?}"
    );
    let options = payload["request"]["options"]
        .as_array()
        .expect("the request.options list is present (the full gate payload verbatim)");
    let option_ids: Vec<&str> = options
        .iter()
        .filter_map(|o| o.get("optionId").and_then(Value::as_str))
        .collect();
    assert_eq!(
        option_ids,
        vec!["allow", "reject", "trust-space"],
        "the full 3-option gate, got {option_ids:?}"
    );

    // (b) The test's response (`Selected { option_id: "allow" }` — the
    // real `PermissionOutcome`, NOT a bool).
    h.manager
        .handle_for("e2e-perm")
        .expect("the session is attached")
        .send_permission_response(
            &id,
            &PermissionOutcome::Selected {
                option_id: "allow".to_string(),
            },
        )
        .expect("the permission response is sent");

    // (c) The tool-execution events follow (the gate resolved `allow` —
    // the real `exec_bash` on the temp cwd) + the turn settles.
    let evs = h
        .wait(Duration::from_secs(60), |evs| {
            evs.iter().any(|(_, _, e)| {
                    matches!(e, WorkerInboundEvent::Event(ev) if ev.kind() == "agent_settled")
                })
        })
        .await
        .expect("the turn settles after the allow within the bound");
    let kinds = event_kinds(&evs);
    let t_start = kinds
        .iter()
        .position(|k| k == "tool_execution_start")
        .expect("a tool_execution_start after the allow, got {kinds:?}");
    let t_end = kinds
        .iter()
        .position(|k| k == "tool_execution_end")
        .expect("a tool_execution_end, got {kinds:?}");
    assert!(t_start < t_end);
    let (result, is_error) =
        tool_execution_end(&evs, Some("bash")).expect("a bash tool_execution_end");
    assert!(!is_error, "the bash tool succeeded after the allow");
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("gated"), "the bash output, got {text:?}");
    // The transcript: the user + assistant(tool_calls) + tool + final
    // assistant rows (the system prompt at seq 0) — polled (the rows are
    // written by the persister from the worker's store frames, which are
    // not ordered against the settle — see `full_turn`'s (c)).
    let rows = wait_for_rows(&h.db, "e2e-perm", 5).await;
    assert_eq!(rows.len(), 5, "the complete transcript, got {rows:?}");
    // A clean close (the `on_crash` callback did NOT fire).
    let handle = h
        .manager
        .detach("e2e-perm")
        .expect("the session is attached");
    let code = tokio::time::timeout(Duration::from_secs(15), handle.exited())
        .await
        .expect("the worker exits after the close");
    assert_eq!(code, Some(0), "a clean close exits 0, got {code:?}");
    assert!(
        h.crashes.try_recv().is_err(),
        "the on_crash callback did NOT fire on a clean close"
    );
}

// ── (3) The crash path ───────────────────────────────────────────────

/// The crash path degrades cleanly: a `StartEnv` whose provider
/// `base_url` is a dead port (connection-refused on every model call —
/// the harness's retry policy exhausts; the turn never settles) — the
/// crash is forced FAST + DETERMINISTIC by SIGKILLing the spawned
/// Worker from the test (the `WorkerHandle`'s `kill` — `build_provider`
/// (`harness/provider.rs`) does NOT panic and does NOT return `Err`:
/// it unconditionally constructs the provider struct from the `Model`
/// fields, so a dead port alone only exhausts the retry policy and ends
/// the turn cleanly — the SIGKILL is the task's fallback branch).
///
/// Assert: `on_crash` fires (the `WorkerManager`'s raw callback) + the
/// exit is non-zero (a kill) + the `TranscriptPersister`'s rows are
/// intact (everything before the crash persisted — the pre-crash user
/// message row + the seq-0 system prompt row).
///
/// NOTE (crash log): a SIGKILL never fires the panic hook (the process
/// is killed, not panicked), so no `crash-*.log` is written on THIS
/// path — the crash log's content contract (the non-empty backtrace,
/// the `force_capture` behavior) is covered by the `crashlog` unit
/// test.
#[tokio::test]
async fn crash_path_on_crash_fires_and_rows_stay_intact() {
    let mut h = E2e::new().await;
    let cwd = temp_dir();
    // A dead port (connection-refused is immediate — no 30 s connect
    // wait): the model call fails `Retryable` on every attempt.
    let dead_model = Model {
        id: "e2e-dead".to_string(),
        provider: "fake".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        api_key: "sk-test".to_string(),
        context_window: 128_000,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: false,
        thinking_levels: Vec::new(),
        api: Some("openai-completions".to_string()),
    };
    let env = start_env(
        "e2e-crash",
        &cwd,
        dead_model.clone(),
        ModelCatalog {
            models: vec![dead_model],
            ..Default::default()
        },
        true,
        Some(vec!["bash".to_string()]),
        true,
    );
    h.attach(env).await;
    // The `ready` handshake is completed by `attach` ITSELF (the
    // client's `wait_ready` — the `ready` watch; the pump subscribes
    // AFTER `Ready`, so it never arrives via `on_event`).
    h.manager
        .handle_for("e2e-crash")
        .expect("the session is attached")
        .send_prompt("run it", &[])
        .expect("the prompt is sent");

    // The pre-crash state: the user message row is persisted (the
    // `TranscriptInsert` applied by the `TranscriptPersister` — the
    // turn is now in its first model call / retry backoff).
    let mut rows = Vec::new();
    for _ in 0..100 {
        rows =
            h.db.load_native_messages("e2e-crash")
                .expect("the transcript loads");
        if rows.iter().any(|r| {
            serde_json::from_str::<Value>(r)
                .map(|v| v["role"] == "user")
                .unwrap_or(false)
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let v: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::from_str(r).expect("the row is JSON"))
        .collect();
    assert!(
        v.iter()
            .any(|m| m["role"] == "user" && m["content"] == "run it"),
        "the pre-crash user message row is persisted, got {v:?}"
    );
    assert_eq!(
        v.first()
            .and_then(|m| m.get("role").and_then(Value::as_str)),
        Some("system"),
        "the seq-0 system prompt row is persisted pre-crash"
    );

    // The FAST deterministic crash: SIGKILL the spawned Worker (the
    // `WorkerHandle`'s `kill` — a dead-port provider alone only
    // exhausts the retry policy and ends the turn cleanly; `build_provider`
    // cannot panic or return `Err`).
    let handle = h
        .manager
        .handle_for("e2e-crash")
        .expect("the session is attached");
    handle.kill().await.expect("the kill succeeds");

    // (a) `on_crash` fires (the `WorkerManager`'s raw callback) with
    // the session id + a non-zero exit code (the kill — a signal
    // exit's `code` is `None`; a normal exit's code is non-zero).
    let (sid, code) = tokio::time::timeout(Duration::from_secs(15), h.crashes.recv())
        .await
        .expect("on_crash fires within the bound")
        .expect("the collector is live");
    assert_eq!(sid, "e2e-crash", "the crash is attributed to the session");
    assert!(
        code != Some(0),
        "the exit is non-zero (a kill), got {code:?}"
    );

    // (b) The `TranscriptPersister`'s rows are intact (everything
    // before the crash persisted — the pre-crash user message row +
    // the seq-0 system prompt row).
    let rows =
        h.db.load_native_messages("e2e-crash")
            .expect("the rows are still intact");
    let v: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::from_str(r).expect("the row is JSON"))
        .collect();
    assert_eq!(
        v.first()
            .and_then(|m| m.get("role").and_then(Value::as_str)),
        Some("system"),
        "the system prompt row survived the crash"
    );
    assert!(
        v.iter()
            .any(|m| m["role"] == "user" && m["content"] == "run it"),
        "the pre-crash user message row survived the crash, got {v:?}"
    );
}

// ── (4) The subagent dispatch (closes the round-2 gap) ───────────────

/// The subagent dispatch end-to-end with REAL Workers (no prior test
/// covered the subagent path end-to-end): a main-session Worker
/// (`subagent_enabled: true`, a canned main SSE stream containing a
/// `subagent` tool call — `task` + an `agentName` resolving against a
/// canned agent definition, `HOME` isolated per the `test_support`
/// `ENV_LOCK` pattern) → the main Worker's `IpcDispatcher` emits
/// `SubagentDispatch` (the resolved `model_key` — the pre-flight
/// against the Worker's catalog) → the Supervisor's
/// `dispatch_subagent` resolves the key → spawns a SUBAGENT Worker
/// (a second wiremock — the subagent's canned stream: a short text
/// answer) with the child `StartEnv` (`system_prompt` = the
/// `build_child_system_message` output — assert the child's
/// `native_messages` seq 0 = that system message; `enabled_tools` =
/// `Some(child_tools)` verbatim — the three-way rule applied, the
/// wire's harness convention; `subagent_enabled: false`) → assert the
/// `SubagentResult` (`Completed` with the `SubagentCapture`'s final
/// text) delivered to the main Worker (the main's `subagent` tool
/// resolves) + the `subagent-session-started` / `subagent-closed`
/// lifecycle events + the ephemeral `is_subagent = 1` row + a clean
/// subagent Worker exit (no `on_crash`).
// The `ENV_LOCK` (`std::sync::Mutex`) is held across the test's `await`
// points INTENTIONALLY — a sibling test's `HOME` read mid-mutation would
// see the wrong value (the `test_support` `ENV_LOCK` pattern).
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn subagent_dispatch_end_to_end_real_workers() {
    // The `HOME` isolation (the `test_support` `ENV_LOCK` pattern): the
    // canned agent definition lives in the fixture root's
    // `~/.pi/agent/agents` (the `resolve_launch` pre-flight reads
    // discovered agent definitions from the `~/.pi/agent` / `~/.agents`
    // roots — the Worker process inherits the env).
    let _lock = archimedes_lib::test_support::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let original_home = std::env::var_os("HOME");
    // Restores `HOME` on scope exit (even when an assertion panics).
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => unsafe {
                    std::env::set_var("HOME", v);
                },
                None => unsafe {
                    std::env::remove_var("HOME");
                },
            }
        }
    }
    let _restore = RestoreHome(original_home.clone());
    let home = temp_dir();
    std::fs::create_dir_all(home.join(".pi/agent/agents")).unwrap();
    // The canned agent definition (the frontmatter `model` override
    // resolves against the main Worker's catalog → the subagent's
    // wiremock; the `tools` frontmatter → the child's tool set; the
    // body → the child's system message).
    std::fs::write(
        home.join(".pi/agent/agents/scout.md"),
        "---\nname: scout\nmodel: fake/sub-model\ntools: [bash]\n---\nYou are a scout. Answer concisely.\n",
    )
    .unwrap();
    // SAFETY: the `ENV_LOCK` is held for the whole set→assert→restore
    // span (the `_lock` guard); no other thread mutates HOME
    // concurrently.
    unsafe {
        std::env::set_var("HOME", &home);
    }

    let mut h = E2e::new().await;
    // The main session's wiremock (request 1: a `subagent` tool call;
    // request 2: the final `stop`).
    let main_server = MockServer::start().await;
    mount_two_request_streams(
        &main_server,
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"tc_sub\",\"function\":{\"name\":\"subagent\",\"arguments\":\"{\\\"task\\\":\\\"find the answer\\\",\\\"agentName\\\":\\\"scout\\\"}\"}}]}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
         data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"all done\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;
    // The subagent's wiremock (a short text answer).
    let sub_server = MockServer::start().await;
    mount_sse(
        &sub_server,
        "data: {\"choices\":[{\"delta\":{\"content\":\"the answer is 42\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    )
    .await;
    let cwd = temp_dir();
    let main_model = wire_model(&main_server, "main-model");
    let sub_model = wire_model(&sub_server, "sub-model");
    // The effective catalog (base + the wiremock providers): the
    // subagent's `model_key` (the parent's `provider/id`) + the
    // frontmatter's `model` override both resolve against it.
    let catalog = ModelCatalog {
        models: vec![main_model.clone(), sub_model],
        ..Default::default()
    };
    let env = start_env(
        "e2e-sub",
        &cwd,
        main_model.clone(),
        catalog,
        true,
        // The main session needs the `subagent` tool (the harness
        // convention — `Some(v)` = exactly `v`).
        Some(vec!["bash".to_string(), "subagent".to_string()]),
        true, // `subagent_enabled: true` (the recursion guard is OFF for the main)
    );
    h.attach(env).await;
    // The `ready` handshake is completed by `attach` ITSELF (the
    // client's `wait_ready` — the `ready` watch; the pump subscribes
    // AFTER `Ready`, so it never arrives via `on_event`).
    h.manager
        .handle_for("e2e-sub")
        .expect("the session is attached")
        .send_prompt("go", &[])
        .expect("the prompt is sent");

    // (a) The main Worker's `IpcDispatcher` emits `SubagentDispatch`
    // (the resolved `model_key` — the pre-flight against the Worker's
    // catalog: the parent's `provider/id`; the frontmatter's `model`
    // override rides in `launch.model`).
    let collected = h
        .wait(Duration::from_secs(30), |evs| {
            evs.iter()
                .any(|(_, _, e)| matches!(e, WorkerInboundEvent::SubagentDispatch(..)))
        })
        .await
        .expect("the SubagentDispatch arrives within the bound");
    let dispatch = collected
        .into_iter()
        .find_map(|(_, _, e)| match e {
            WorkerInboundEvent::SubagentDispatch(w) => Some(w),
            _ => None,
        })
        .expect("a SubagentDispatch frame");
    assert_eq!(dispatch.parent_session_id, "e2e-sub");
    assert_eq!(dispatch.agent_name, "scout", "the agentName rides verbatim");
    assert_eq!(dispatch.task, "find the answer", "the task rides verbatim");
    assert_eq!(
        dispatch.model_key, "fake/main-model",
        "the model_key is the parent's provider/id composition (the pre-flight)"
    );
    assert_eq!(
        dispatch.launch.model.as_deref(),
        Some("fake/sub-model"),
        "the frontmatter's model override resolved against the Worker's catalog"
    );

    // (b) The `subagent-session-started` UI lifecycle event (the
    // `subagent.rs:635` payload shape — on the parent's sink): the
    // child's resolved model (the frontmatter override) + the child's
    // resolved tool set (the three-way rule: `launch.tools` `Some` →
    // VERBATIM minus `subagent`/`list_agents`).
    let sink_events = h
        .wait_sink(Duration::from_secs(30), |evs| {
            evs.iter().any(|(e, _)| e == "subagent-session-started")
        })
        .await
        .expect("subagent-session-started arrives within the bound");
    let started = sink_events
        .iter()
        .find_map(|(e, p)| (e == "subagent-session-started").then_some(p.clone()))
        .expect("the subagent-session-started payload");
    let child_id = started["sessionId"]
        .as_str()
        .expect("the child session id")
        .to_string();
    assert_eq!(started["parentSessionId"], "e2e-sub");
    assert_eq!(started["agentName"], "scout");
    assert_eq!(started["task"], "find the answer");
    assert_eq!(
        started["model"], "fake/sub-model",
        "the child's resolved model (the frontmatter override)"
    );
    let enabled: Vec<String> = started["enabledTools"]
        .as_array()
        .expect("the enabledTools list")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        enabled,
        vec!["bash"],
        "the child's tool set (launch.tools verbatim minus the guard)"
    );

    // (c) The main's `agent_settled` (the `SubagentResult`
    // (`Completed` with the `SubagentCapture`'s final text) delivered
    // to the main Worker → the main's `subagent` tool resolves → the
    // final model call → the turn settles). The predicate matches the
    // MAIN's `agent_settled` (`is_subagent: false` — the child's
    // `agent_settled` is delivered under the parent's id with
    // `is_subagent: true` and would match a naive predicate early —
    // the `SubagentResult` delivery lands only after the child's reap
    // grace).
    let evs = h
        .wait(Duration::from_secs(60), |evs| {
            evs.iter().any(|(_, is_sub, e)| {
                !is_sub
                    && matches!(e, WorkerInboundEvent::Event(ev) if ev.kind() == "agent_settled")
            })
        })
        .await
        .expect("the main turn settles within the bound");
    // The `subagent` tool resolved with the child's captured final text
    // (`Completed` — NOT an error).
    let (result, is_error) =
        tool_execution_end(&evs, Some("subagent")).expect("the subagent tool_execution_end");
    assert!(
        !is_error,
        "the subagent dispatch Completed (not Failed), got {result:?}"
    );
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        text.contains("the answer is 42"),
        "the SubagentCapture's final text (the child's last message), got {text:?}"
    );
    // The main's final assistant text (the tool-result turn's `stop`) —
    // polled (the store frames are not ordered against the settle — see
    // `full_turn`'s (c)).
    let rows = wait_for_rows(&h.db, "e2e-sub", 5).await;
    let v: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::from_str(r).expect("the row is JSON"))
        .collect();
    assert_eq!(
        v.len(),
        5,
        "system + user + assistant(subagent tool_calls) + tool + final assistant, got {v:?}"
    );
    assert_eq!(
        v[4]["content"], "all done",
        "the main's final assistant text"
    );

    // (d) The `subagent-closed` UI lifecycle event (the
    // `subagent.rs:721/743` payload shape — on EVERY exit path):
    // `status: "completed"` + the `metrics` populated (the
    // `SubagentCapture`'s final text + the wall clock).
    let sink_events = h
        .wait_sink(Duration::from_secs(30), |evs| {
            evs.iter().any(|(e, _)| e == "subagent-closed")
        })
        .await
        .expect("subagent-closed arrives within the bound");
    let closed = sink_events
        .iter()
        .find_map(|(e, p)| (e == "subagent-closed").then_some(p.clone()))
        .expect("the subagent-closed payload");
    assert_eq!(closed["sessionId"], child_id);
    assert_eq!(
        closed["status"], "completed",
        "the dispatch Completed, got {closed:?}"
    );
    let metrics = &closed["metrics"];
    assert_eq!(
        metrics["output"], "the answer is 42",
        "the metrics' output is the SubagentCapture's final text"
    );
    assert!(
        metrics["durationMs"].as_u64().unwrap_or(0) > 0,
        "the metrics' wall clock is populated, got {metrics:?}"
    );

    // (e) The child's `native_messages`: seq 0 = the `system_prompt`
    // envelope field (the `build_child_system_message` output — the
    // frontmatter body, `has_todo_tool` = `false`), then the user task
    // + the child's final text.
    let child_rows =
        h.db.load_native_messages(&child_id)
            .expect("the child transcript loads");
    assert_eq!(
        child_rows.len(),
        3,
        "system + user + assistant, got {child_rows:?}"
    );
    let cv: Vec<Value> = child_rows
        .iter()
        .map(|r| serde_json::from_str(r).expect("the row is JSON"))
        .collect();
    assert_eq!(
        cv[0]["role"], "system",
        "the child's seq-0 row is the system message"
    );
    // The `build_child_system_message` output (the frontmatter body —
    // `has_todo_tool` = `false` → the body verbatim, no TODO guidance).
    let expected_system =
        build_child_system_message(Some("You are a scout. Answer concisely."), false)
            .expect("a non-empty body yields a system message");
    assert_eq!(
        cv[0]["content"], expected_system,
        "the child's seq-0 row is the build_child_system_message output"
    );
    assert_eq!(cv[1]["role"], "user");
    assert_eq!(
        cv[1]["content"], "find the answer",
        "the child's user row is the task"
    );
    assert_eq!(cv[2]["role"], "assistant");
    assert_eq!(
        cv[2]["content"], "the answer is 42",
        "the child's final text"
    );

    // (f) The ephemeral `is_subagent = 1` row (the `TranscriptPersister`'s
    // `ensure_session_row` — hidden from `list_sessions`, the transcript
    // loadable):
    let row =
        h.db.session(&child_id)
            .expect("the child session row loads")
            .expect("the child's sessions row exists");
    assert!(
        row.is_subagent,
        "the child's sessions row is flagged is_subagent = 1"
    );
    let all = h.db.list_sessions(true).expect("list_sessions loads");
    assert!(
        all.iter().all(|r| r.id != child_id),
        "the ephemeral subagent row is hidden from list_sessions"
    );

    // (g) A clean subagent Worker exit (no `on_crash` — the `Exited` is
    // EXPECTED via the drive teardown) + a clean main close.
    let main_handle = h.manager.detach("e2e-sub").expect("the main is attached");
    let code = tokio::time::timeout(Duration::from_secs(15), main_handle.exited())
        .await
        .expect("the main worker exits after the close");
    assert_eq!(code, Some(0), "the main worker exits 0, got {code:?}");
    assert!(
        h.crashes.try_recv().is_err(),
        "no on_crash (the subagent Worker exited cleanly via the drive teardown; the main via the close)"
    );
}
