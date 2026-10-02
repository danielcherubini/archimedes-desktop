//! The `AgentLoop` e2e (native-agent-harness Task 6): a mock
//! `Provider` (canned `ProviderEvent` streams — the loop test
//! constructs it directly; the `SessionManager::set_provider_factory`
//! injection seam is Task 7) + a temp `SessionStore`. The loop emits
//! `RpcEvent`-shaped values and runs them through the existing
//! `normalize` + `persist_update` pipeline (the frozen `session-update`
//! frames are captured via a recording `EventSink`) and persists the
//! provider transcript to the `native_messages` table.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use archimedes_lib::agent::events::RpcEvent;
use archimedes_lib::agent::harness::{
    ChatMessage, ChatRole, CompactionConfig, FinishReason, MessageContent, Model, ModelCatalog,
    Prompt, Provider, ProviderError, ProviderEvent, RetryPolicy, SessionStore, SudoDeps, Usage,
};
use archimedes_lib::agent::interactive::PendingInteractive;
use archimedes_lib::agent::tools::ContentBlock;
use archimedes_lib::agent::{EventSink, PendingPermissions, PermissionOutcome, TodoStore};
use archimedes_lib::storage::Db;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Mutex as TokioMutex};
use tokio_util::sync::CancellationToken;

// ── the mock `Provider` ───────────────────────────────────────────────

/// One canned response: a `ProviderEvent` stream, or an error.
enum MockResponse {
    Stream(Vec<ProviderEvent>),
    Error(ProviderError),
}

impl MockResponse {
    fn text_then_done(delta: &str) -> Self {
        Self::Stream(vec![
            ProviderEvent::TextDelta(delta.to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ])
    }
}

/// A `Provider` returning canned responses in order (a `complete()`
/// beyond the queue is a `Fatal` error — a test bug).
struct MockProvider {
    responses: Mutex<VecDeque<MockResponse>>,
}

impl MockProvider {
    fn with(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _req: &archimedes_lib::agent::harness::ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let next = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockResponse::Error(ProviderError::Fatal(
                "mock: no canned response left".into(),
            )));
        match next {
            MockResponse::Stream(events) => Ok(futures_util::stream::iter(events).boxed()),
            MockResponse::Error(e) => Err(e),
        }
    }
}

// ── a recording `EventSink` ───────────────────────────────────────────

struct RecSink {
    tx: mpsc::UnboundedSender<(String, Value)>,
}

impl EventSink for RecSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

// ── the test harness ──────────────────────────────────────────────────

fn test_model(context_window: u32) -> Model {
    Model {
        id: "test-model".to_string(),
        provider: "test".to_string(),
        base_url: "http://localhost/v1".to_string(),
        api_key: "k".to_string(),
        context_window,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: false,
        thinking_levels: Vec::new(),
        api: Some("openai-completions".to_string()),
    }
}

fn temp_cwd() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("harness-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Harness {
    prompt_tx: mpsc::Sender<Prompt>,
    events_rx: mpsc::Receiver<RpcEvent>,
    sink_rx: mpsc::UnboundedReceiver<(String, Value)>,
    pending: PendingPermissions,
    store: SessionStore,
}

/// Build the loop (a mock `Provider` + a temp `Db`) and spawn its
/// `run()` task.
async fn build_harness(
    provider: MockProvider,
    retry: RetryPolicy,
    compaction: CompactionConfig,
    model: Model,
) -> (Harness, tokio::task::JoinHandle<()>) {
    let dir = std::env::temp_dir().join(format!("harness-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("db.sqlite")).unwrap());
    // The `native_messages` rows FK to `sessions`: record the test session
    // so transcript inserts (and the `run_compaction` rewrite) are valid.
    db.record_session(&archimedes_lib::agent::SessionInfo {
        session_id: "ns1".to_string(),
        cwd: std::path::PathBuf::from("/tmp"),
        capabilities: serde_json::json!({}),
        config_options: None,
        archived: false,
    })
    .unwrap();
    let store = SessionStore::new(db.clone());
    let (sink_tx, sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    let (events_tx, events_rx) = mpsc::channel(256);
    let (prompt_tx, prompt_rx) = mpsc::channel(8);
    let pending: PendingPermissions = Arc::new(TokioMutex::new(std::collections::HashMap::new()));
    let pending_bridge: PendingInteractive =
        Arc::new(TokioMutex::new(std::collections::HashMap::new()));
    let cancel = CancellationToken::new();
    // The turn cancel (finding 8c — shared with the handle in production;
    // the tests cancel it directly) + the settle watch (finding 3 — the
    // loop writes it on every `agent_settled`; the test's receiver is
    // dropped — the `events` mpsc is the test's observation channel).
    let turn_cancel: Arc<Mutex<CancellationToken>> = Arc::new(Mutex::new(CancellationToken::new()));
    let (settle_tx, _settle_rx) = watch::channel(0u64);
    let loop_ = archimedes_lib::agent::harness::AgentLoop::new(
        "ns1".to_string(),
        temp_cwd(),
        model,
        Box::new(provider),
        ModelCatalog {
            compaction,
            ..Default::default()
        },
        store.clone(),
        events_tx,
        cancel,
        turn_cancel,
        settle_tx,
        prompt_tx.clone(),
        prompt_rx,
        pending.clone(),
        pending_bridge,
        None, // trust_db (untrusted → the gate prompts)
        sink,
        Arc::new(TodoStore::new()),
        None, // subagent (not exercised)
        SudoDeps::default(),
        retry,
        None, // config_dir (no desktop MCP layer in the test)
    );
    let task = tokio::spawn(loop_.run());
    (
        Harness {
            prompt_tx,
            events_rx,
            sink_rx,
            pending,
            store,
        },
        task,
    )
}

/// Collect the loop's `RpcEvent`s until `agent_settled`.
async fn collect_until_settled(rx: &mut mpsc::Receiver<RpcEvent>) -> Vec<RpcEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        let settled = matches!(ev, RpcEvent::agent_settled);
        events.push(ev);
        if settled {
            break;
        }
    }
    events
}

/// Drain the sink (all `(event, payload)` pairs).
fn drain_sink(rx: &mut mpsc::UnboundedReceiver<(String, Value)>) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    while let Ok(m) = rx.try_recv() {
        out.push(m);
    }
    out
}

/// Wait for the loop's pending-permission entry (registered before the
/// `permission-request` event) and resolve it with `outcome` — the mock
/// handle-free permission waiter.
async fn resolve_pending_permission(pending: &PendingPermissions, outcome: PermissionOutcome) {
    loop {
        let mut map = pending.lock().await;
        if let Some(key) = map.keys().find(|k| k.starts_with("ns1/")).cloned() {
            let sender = map.remove(&key).expect("the entry is in the map");
            let _ = sender.send(outcome);
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn kinds(events: &[RpcEvent]) -> Vec<std::borrow::Cow<'static, str>> {
    events.iter().map(|e| e.kind()).collect()
}

// ── (a) a text response ───────────────────────────────────────────────

#[tokio::test]
async fn text_response_emits_normalized_events_and_persists_transcript() {
    let (mut h, task) = build_harness(
        MockProvider::with(vec![MockResponse::text_then_done("Hello")]),
        RetryPolicy::new(),
        CompactionConfig::default(),
        test_model(128000),
    )
    .await;
    h.prompt_tx
        .send(Prompt {
            text: "hi".to_string(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let events = collect_until_settled(&mut h.events_rx).await;

    // (1) The raw `RpcEvent` vocabulary (the same shapes an external
    // session emits).
    let k = kinds(&events);
    assert!(
        k.contains(&"message_start".into()),
        "message_start, got {k:?}"
    );
    assert!(
        k.contains(&"message_update".into()),
        "message_update, got {k:?}"
    );
    assert!(k.contains(&"message_end".into()), "message_end, got {k:?}");
    assert!(
        k.contains(&"agent_settled".into()),
        "agent_settled, got {k:?}"
    );
    // The `message_update` carries `assistantMessageEvent
    // { type: "text_delta", delta }` (the precise wire shape).
    let delta = events
        .iter()
        .find_map(|e| match e {
            RpcEvent::message_update {
                assistant_message_event: ame,
                ..
            } => ame
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t == "text_delta")
                .then_some(ame.clone()),
            _ => None,
        })
        .expect("a text_delta assistantMessageEvent");
    assert_eq!(delta.get("delta"), Some(&json!("Hello")));

    // (2) The FROZEN `session-update` frames (the normalizer's output)
    // via the sink — the frontend is unchanged.
    let frames = drain_sink(&mut h.sink_rx);
    let chunk = frames
        .iter()
        .find(|(_, p)| p["update"]["sessionUpdate"] == "agent_message_chunk")
        .map(|(_, p)| p.clone())
        .expect("an agent_message_chunk frame");
    assert_eq!(chunk["update"]["content"]["text"], "Hello");
    assert_eq!(chunk["update"]["messageId"], "m1");

    // (3) The provider transcript (the `native_messages` table): the
    // user + assistant messages.
    let msgs = h.store.load_messages("ns1").unwrap();
    assert_eq!(msgs.len(), 2, "user + assistant persisted");
    assert_eq!(
        msgs[0],
        ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text("hi".to_string()),
            tool_call_id: None,
            tool_calls: None,
        }
    );
    assert_eq!(
        msgs[1],
        ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Text("Hello".to_string()),
            tool_call_id: None,
            tool_calls: None,
        }
    );
    task.abort();
}

// ── (b) a tool call: gate → execute → loop → settle ───────────────────

#[tokio::test]
async fn tool_call_gates_executes_loops_and_settles() {
    let (mut h, task) = build_harness(
        MockProvider::with(vec![
            MockResponse::Stream(vec![
                ProviderEvent::ToolCall(archimedes_lib::agent::harness::ToolCall {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    arguments: json!({ "command": "echo hi" }),
                }),
                ProviderEvent::Done(FinishReason::ToolCalls),
            ]),
            MockResponse::text_then_done("done"),
        ]),
        RetryPolicy::new(),
        CompactionConfig::default(),
        test_model(128000),
    )
    .await;
    h.prompt_tx
        .send(Prompt {
            text: "run it".to_string(),
            images: Vec::new(),
        })
        .await
        .unwrap();

    // The permission gate (an untrusted Space): the loop emits a
    // `permission-request` and registers a `PendingPermissions` oneshot
    // — resolve it with `allow` (the mock handle-free waiter).
    let pending = h.pending.clone();
    let gate = tokio::spawn(async move {
        resolve_pending_permission(
            &pending,
            PermissionOutcome::Selected {
                option_id: "allow".to_string(),
            },
        )
        .await;
    });
    let events = collect_until_settled(&mut h.events_rx).await;
    gate.await.unwrap();

    let k = kinds(&events);
    assert!(
        k.contains(&"tool_execution_start".into()),
        "tool_execution_start, got {k:?}"
    );
    assert!(
        k.contains(&"tool_execution_end".into()),
        "tool_execution_end, got {k:?}"
    );
    assert!(
        k.contains(&"agent_settled".into()),
        "agent_settled, got {k:?}"
    );
    // The tool result (a real `exec_bash` on the temp cwd).
    let (result, is_error) = events
        .iter()
        .find_map(|e| match e {
            RpcEvent::tool_execution_end {
                result, is_error, ..
            } => Some((result.clone(), *is_error)),
            _ => None,
        })
        .expect("a tool_execution_end");
    assert!(!is_error, "the bash tool succeeded");
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("hi"), "the bash output, got {text:?}");

    // The `permission-request` frame (the gate's prompt) via the sink.
    let frames = drain_sink(&mut h.sink_rx);
    let perm = frames
        .iter()
        .find(|(event, _)| event == "permission-request")
        .map(|(_, p)| p.clone())
        .expect("a permission-request frame");
    assert_eq!(perm["requestId"], "call_1");
    assert!(perm["request"]["toolCall"]["title"].is_string());

    // The provider transcript: user, assistant (tool_calls), tool
    // (the result, `tool_call_id` intact), and the FINAL assistant
    // (the second model call's response — the turn only settles after
    // it, so it is the fourth transcript message; a 3-row transcript
    // would be lossy — a resume would drop the final response).
    let msgs = h.store.load_messages("ns1").unwrap();
    assert_eq!(
        msgs.len(),
        4,
        "user + assistant + tool + final assistant persisted"
    );
    assert_eq!(
        msgs[1]
            .tool_calls
            .as_ref()
            .and_then(|c| c.first())
            .map(|t| t.name.as_str()),
        Some("bash"),
        "the assistant's tool_calls round-trip"
    );
    assert_eq!(msgs[2].role, ChatRole::Tool);
    assert_eq!(msgs[2].tool_call_id.as_deref(), Some("call_1"));
    // The `tool` message keeps the result's FULL `Blocks` (the pi shape —
    // the provider's `to_wire` is the request-wire shape; a `bash` `hi`
    // result is a single text block, but an image-only result would be
    // an image block, NOT flattened text).
    let tool_blocks = match &msgs[2].content {
        MessageContent::Blocks(b) => b,
        other => panic!("the tool result is the full blocks, got {other:?}"),
    };
    assert!(
        tool_blocks.iter().any(|b| match b {
            ContentBlock::Text { text } => text.contains("hi"),
            _ => false,
        }),
        "the tool result's text block round-trips, got {tool_blocks:?}"
    );
    task.abort();
}

// ── (c) a `Retryable` error is retried ────────────────────────────────

#[tokio::test]
async fn retryable_error_retries_then_succeeds() {
    let (mut h, task) = build_harness(
        MockProvider::with(vec![
            MockResponse::Error(ProviderError::Retryable(
                "429 too many requests".to_string(),
            )),
            MockResponse::text_then_done("ok"),
        ]),
        // A 1 ms base delay (the production default is 1 s).
        RetryPolicy::new_with(5, Duration::from_millis(1)),
        CompactionConfig::default(),
        test_model(128000),
    )
    .await;
    h.prompt_tx
        .send(Prompt {
            text: "hi".to_string(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let events = collect_until_settled(&mut h.events_rx).await;

    let k = kinds(&events);
    assert!(
        k.contains(&"auto_retry_start".into()),
        "auto_retry_start, got {k:?}"
    );
    assert!(
        k.contains(&"auto_retry_end".into()),
        "auto_retry_end, got {k:?}"
    );
    assert!(
        k.contains(&"agent_settled".into()),
        "the retry eventually settles, got {k:?}"
    );
    // The `auto_retry_end` reports a success.
    let success = events
        .iter()
        .find_map(|e| match e {
            RpcEvent::auto_retry_end { success, .. } => Some(*success),
            _ => None,
        })
        .expect("an auto_retry_end");
    assert!(success, "the retry succeeded");
    task.abort();
}

// ── (d) resume: `load_messages` reconstructs the transcript ───────────

#[tokio::test]
async fn load_messages_reconstructs_the_transcript_for_resume() {
    let (mut h, task) = build_harness(
        MockProvider::with(vec![MockResponse::text_then_done("Hello")]),
        RetryPolicy::new(),
        CompactionConfig::default(),
        test_model(128000),
    )
    .await;
    h.prompt_tx
        .send(Prompt {
            text: "hi".to_string(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let _ = collect_until_settled(&mut h.events_rx).await;

    // The resume path: reconstruct the `messages` vec from the
    // `native_messages` table (a native `resume_session` does NOT clear
    // before `load_messages`).
    let msgs = h.store.load_messages("ns1").unwrap();
    assert_eq!(msgs.len(), 2, "the full transcript reconstructs");
    assert_eq!(
        msgs[0],
        ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text("hi".to_string()),
            tool_call_id: None,
            tool_calls: None,
        }
    );
    assert_eq!(
        msgs[1],
        ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Text("Hello".to_string()),
            tool_call_id: None,
            tool_calls: None,
        }
    );
    task.abort();
}

// ── (e) the `Compactor`: a context-threshold compaction ───────────────

#[tokio::test]
async fn context_threshold_triggers_compaction() {
    // A small window (50) + reserve (10) + keep_recent (1): the first
    // turn's usage (150) exceeds the threshold (40), so the SECOND
    // prompt runs a summary model call (canned "SUMMARY") over the
    // older messages and rewrites the transcript.
    let (mut h, task) = build_harness(
        MockProvider::with(vec![
            MockResponse::Stream(vec![
                ProviderEvent::TextDelta("one".to_string()),
                ProviderEvent::Usage(Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                }),
                ProviderEvent::Done(FinishReason::Stop),
            ]),
            // The summary call (the older messages).
            MockResponse::text_then_done("SUMMARY"),
            // The second turn.
            MockResponse::text_then_done("two"),
        ]),
        RetryPolicy::new(),
        CompactionConfig {
            enabled: true,
            reserve_tokens: 10,
            keep_recent_tokens: 1,
        },
        test_model(50),
    )
    .await;
    h.prompt_tx
        .send(Prompt {
            text: "first".to_string(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let _ = collect_until_settled(&mut h.events_rx).await;
    h.prompt_tx
        .send(Prompt {
            text: "second".to_string(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let events = collect_until_settled(&mut h.events_rx).await;

    let k = kinds(&events);
    assert!(
        k.contains(&"compaction_start".into()),
        "compaction_start, got {k:?}"
    );
    assert!(
        k.contains(&"compaction_end".into()),
        "compaction_end, got {k:?}"
    );
    assert!(
        k.contains(&"agent_settled".into()),
        "the turn still settles, got {k:?}"
    );
    // The transcript is rewritten: the summary (a `system` message)
    // replaces the older messages.
    let msgs = h.store.load_messages("ns1").unwrap();
    let first = &msgs[0];
    assert_eq!(
        first.role,
        ChatRole::System,
        "the summary is a system message"
    );
    let text = match &first.content {
        MessageContent::Text(t) => t.clone(),
        other => panic!("the summary is text, got {other:?}"),
    };
    assert!(text.contains("SUMMARY"), "the summary, got {text:?}");
    task.abort();
}
