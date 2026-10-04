//! End-to-end test for the NATIVE `subagent` tool (the ADR 0025 Task 5
//! re-plumb): a NATIVE parent session (a mock `Provider` + the in-process
//! `AgentLoop`) issues a `subagent` tool call → `dispatch_subagent` →
//! `SubagentSessionManager::dispatch_native` DELEGATES to the
//! `WorkerManager`'s `dispatch_subagent` flow → a `fake_worker` child
//! process runs the turn → the `ToolResult` carries the child's captured
//! final text into the parent's transcript.
//!
//! This proves the native `subagent` tool EXECUTES end-to-end (not just
//! displays): the parent's `subagent` call spawns a WORKER child, runs a
//! turn, and captures the result — the whole flow through the
//! `WorkerManager` (NO in-process driver — the throwaway-DB machinery is
//! deleted).

mod common;
use common::mock_provider::{MockProvider, MockResponse};
use common::RecSink;

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use archimedes_lib::agent::events::RpcEvent;
use archimedes_lib::agent::harness::{
    FinishReason, Model, ModelCatalog, Prompt, ProviderEvent, RetryPolicy, SessionStore, SudoDeps,
    ToolCall,
};
use archimedes_lib::agent::interactive::PendingInteractive;
use archimedes_lib::agent::subagent::SubagentSessionManager;
use archimedes_lib::agent::worker::client::{WorkerError, WorkerHandle};
use archimedes_lib::agent::worker::manager::{WorkerFactory, WorkerManager};
use archimedes_lib::agent::worker::protocol::{StartEnv, StartMode};
use archimedes_lib::agent::{EventSink, PendingPermissions, TodoStore};
use archimedes_lib::storage::Db;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Mutex};
use tokio_util::sync::CancellationToken;

/// The parent's `Model` (the `build_harness` model — the `dispatch_native`
/// delegation's `WorkerManager` flow resolves the CHILD's model from the
/// parent's `StartEnv` catalog by this key, so the catalog must contain
/// it).
fn parent_model() -> Model {
    Model {
        id: "fake-model".to_string(),
        provider: "fake".to_string(),
        base_url: "http://localhost/v1".to_string(),
        api_key: "k".to_string(),
        context_window: 100_000,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: false,
        thinking_levels: Vec::new(),
        api: Some("openai-completions".to_string()),
    }
}

/// The `fake_worker` fixture factory (the `WorkerFactory` seam — the
/// `CARGO_MANIFEST_DIR`/`target/debug` convention; `cargo test` builds
/// the bin targets).
struct FakeWorkerFactory;

impl WorkerFactory for FakeWorkerFactory {
    fn spawn(&self) -> Result<WorkerHandle, WorkerError> {
        WorkerHandle::spawn(&std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/debug/fake_worker"
        )))
    }
}

/// Build a `SubagentSessionManager` on a `WorkerManager` (the ADR 0025
/// Task 5 re-plumb — the native dispatch runs in a `fake_worker`
/// process): the parent `ns1` is `attach`ed with the given `catalog`
/// (the `dispatch_subagent` flow resolves the child's `model` against
/// it) + a short settle bound. The `sink_for` lookup returns the given
/// sink for the parent (the `subagent-session-started` /
/// `subagent-closed` UI lifecycle events are delivered on the PARENT's
/// sink).
async fn make_manager(
    catalog: ModelCatalog,
    settle_timeout: Duration,
    sink: Arc<dyn EventSink>,
) -> Arc<SubagentSessionManager> {
    let factory: Arc<dyn WorkerFactory> = Arc::new(FakeWorkerFactory);
    let wm = WorkerManager::new(
        factory,
        Arc::new(|_id: String, _code: Option<i32>| {}),
        Arc::new(
            |_id: String,
             _is_subagent: bool,
             _evt: archimedes_lib::agent::worker::client::WorkerInboundEvent| {},
        ),
        Arc::new(move |id: &str| (id == "ns1").then(|| sink.clone())),
    )
    .with_settle_timeout(settle_timeout);
    let wm = Arc::new(wm);
    let env = StartEnv::from_parts(
        "ns1".to_string(),
        "/tmp".to_string(),
        StartMode::Fresh,
        None,
        catalog.models[0].clone(),
        catalog,
        None,
        true,
        None,
        "/tmp".to_string(),
        true,
        None,
    );
    wm.attach("ns1", &env)
        .await
        .expect("the parent attach completes");
    let manager = Arc::new(SubagentSessionManager::new());
    manager.set_worker_manager(wm);
    manager
}

/// Build the native loop (a mock `Provider` + a temp `Db` + an OPTIONAL
/// `SubagentSessionManager` — the `subagent` tool dispatches through the
/// `WorkerManager`'s `dispatch_subagent` flow).
async fn build_harness(
    provider: MockProvider,
    subagent_manager: Option<Arc<SubagentSessionManager>>,
) -> (
    mpsc::Sender<Prompt>,
    mpsc::UnboundedReceiver<RpcEvent>,
    mpsc::UnboundedReceiver<(String, Value)>,
    SessionStore,
) {
    let dir = std::env::temp_dir().join(format!("harness-subagent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("db.sqlite")).unwrap());
    db.record_session(&archimedes_lib::agent::SessionInfo {
        session_id: "ns1".to_string(),
        cwd: PathBuf::from("/tmp"),
        capabilities: json!({}),
        config_options: None,
        archived: false,
        context_usage: None,
        is_subagent: false,
    })
    .unwrap();
    let store = SessionStore::new(db.clone());
    let (sink_tx, sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (prompt_tx, prompt_rx) = mpsc::channel(8);
    let pending: PendingPermissions = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let pending_bridge: PendingInteractive = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let cancel = CancellationToken::new();
    let turn_cancel: Arc<StdMutex<CancellationToken>> =
        Arc::new(StdMutex::new(CancellationToken::new()));
    let (settle_tx, _settle_rx) = watch::channel(0u64);
    let model = parent_model();
    let loop_ = archimedes_lib::agent::harness::AgentLoop::new(
        "ns1".to_string(),
        dir,
        model,
        Box::new(provider),
        Default::default(),
        Arc::new(store.clone()),
        events_tx,
        cancel,
        turn_cancel,
        settle_tx,
        prompt_tx.clone(),
        prompt_rx,
        pending,
        pending_bridge,
        None,
        sink,
        Arc::new(TodoStore::new()),
        subagent_manager.as_ref().map(|m| {
            Arc::new(archimedes_lib::agent::harness::InProcessDispatcher::new(
                m.clone(),
            )) as Arc<dyn archimedes_lib::agent::harness::SubagentDispatcher>
        }),
        SudoDeps::default(),
        RetryPolicy::default(),
        None, // config_dir (no desktop MCP layer in the test)
    );
    let task = tokio::spawn(loop_.run());
    // Reap the task on drop (the tests don't await it — the `events`
    // channel is the observation point).
    drop(task);
    (prompt_tx, events_rx, sink_rx, store)
}

/// Wait up to `timeout` for a predicate over the collected `RpcEvent`s.
async fn wait_for(
    events: &mut mpsc::UnboundedReceiver<RpcEvent>,
    timeout: Duration,
    mut pred: impl FnMut(&[RpcEvent]) -> bool,
) -> Result<Vec<RpcEvent>, ()> {
    let mut collected: Vec<RpcEvent> = Vec::new();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        if pred(&collected) {
            return Ok(collected);
        }
        tokio::select! {
            maybe = events.recv() => match maybe {
                Some(ev) => collected.push(ev),
                None => return Err(()),
            },
            _ = &mut deadline => return Err(()),
        }
    }
}

/// Extract the text from a `tool_execution_end` `result` (a serialized
/// `ToolResult`: `{ content: [...], ... }` — the text is in
/// `content[].text`).
fn extract_result_text(result: &Value) -> String {
    if let Some(text) = result.as_str() {
        return text.to_string();
    }
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let mut parts: Vec<String> = Vec::new();
        for block in content {
            if let Some(t) = block.get("text").and_then(Value::as_str) {
                parts.push(t.to_string());
            }
        }
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }
    result.to_string()
}

/// (1) **end-to-end**: the native parent issues a `subagent` tool call →
/// `dispatch_subagent` → `dispatch_native` DELEGATES to the
/// `WorkerManager`'s `dispatch_subagent` flow → a `fake_worker` child
/// process runs the turn (the `SubagentCapture` captures the child's
/// final text) → the `ToolResult` carries the child's captured output
/// into the parent's transcript (a `toolResult` message + the
/// `subagent-session-started` / `subagent-closed` sink events).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_subagent_tool_spawns_a_worker_child_and_captures_the_result() {
    // The subagent manager (a `WorkerManager` with a `fake_worker`
    // factory; the parent `ns1` is `attach`ed with a catalog containing
    // the parent's model — the flow resolves the child's `model` by that
    // key). The `sink_for` lookup delivers the lifecycle events on the
    // test's `RecSink`.
    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    let catalog = ModelCatalog {
        models: vec![parent_model()],
        ..Default::default()
    };
    let subagent_manager = make_manager(catalog, Duration::from_secs(30), sink.clone()).await;

    // The mock provider (the parent): a `subagent` tool call, then a final
    // message.
    let provider = MockProvider::new(vec![
        MockResponse::Stream(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "tc_subagent".to_string(),
                name: "subagent".to_string(),
                arguments: json!({ "task": "do the task", "agentName": "fake" }),
            }),
            ProviderEvent::Done(FinishReason::ToolCalls),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("done".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
    ]);

    let (prompt_tx, mut events, _sink, _store) =
        build_harness(provider, Some(subagent_manager)).await;

    let _ = prompt_tx
        .send(Prompt {
            text: "go".to_string(),
            images: Vec::new(),
        })
        .await;

    // The `subagent` tool EXECUTED (a `tool_execution_end` with the
    // child's captured final text — the `fake_worker`'s canned
    // `"canned answer"`: the `SubagentCapture` over the child's
    // `SinkFrame` `agent_message_chunk` stream).
    let evs = wait_for(&mut events, Duration::from_secs(30), |evs| {
        evs.iter().any(|ev| {
            matches!(
                ev,
                RpcEvent::tool_execution_end {
                    is_error: false,
                    ..
                }
            )
        })
    })
    .await
    .expect("the subagent tool should complete");

    // The tool result carried the WORKER CHILD's captured final text
    // (the `fake_worker`'s canned `"canned answer"` — NOT an error, NOT
    // a spawn failure).
    let tool_result = evs
        .iter()
        .find_map(|ev| match ev {
            RpcEvent::tool_execution_end { result, .. } => Some(extract_result_text(result)),
            _ => None,
        })
        .expect("the tool_execution_end carries the result");
    assert!(
        tool_result.contains("canned answer"),
        "the worker child's captured final text must be captured — got: {tool_result:?}"
    );

    // The subagent session lifecycle fired (a worker child spawned +
    // closed — on the PARENT's sink).
    let mut sink_events: Vec<(String, Value)> = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(deadline);
    loop {
        let has_lifecycle = sink_events
            .iter()
            .any(|(e, _)| e == "subagent-session-started")
            && sink_events.iter().any(|(e, _)| e == "subagent-closed");
        if has_lifecycle {
            break;
        }
        tokio::select! {
            maybe = sink_rx.recv() => match maybe {
                Some(ev) => sink_events.push(ev),
                None => break,
            },
            _ = &mut deadline => break,
        }
    }
    assert!(
        sink_events
            .iter()
            .any(|(e, _)| e == "subagent-session-started"),
        "a subagent-session-started event must fire (a worker child spawned)"
    );
    // The `subagent-session-started` event carries a real `sessionId` (a
    // spawned child — not a spurious event) + the parent's id.
    let started = sink_events
        .iter()
        .find(|(e, _)| e == "subagent-session-started")
        .map(|(_, p)| p.clone())
        .expect("the started event is present");
    let session_id = started
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !session_id.is_empty(),
        "the subagent-session-started event must carry a real sessionId — got: {started:?}"
    );
    assert_eq!(
        started["parentSessionId"], "ns1",
        "the started event carries the parent's id — got: {started:?}"
    );
    assert!(
        sink_events.iter().any(|(e, _)| e == "subagent-closed"),
        "a subagent-closed event must fire (the child reaped)"
    );
    // The `subagent-closed` event reports a COMPLETED child (the
    // `fake_worker`'s turn settled — not a spawn failure).
    let closed = sink_events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("the closed event is present");
    assert_eq!(
        closed["status"], "completed",
        "the child completed the task — got: {closed:?}"
    );
    assert_eq!(
        closed["metrics"]["output"], "canned answer",
        "the closed event's metrics carry the child's captured final text — got: {closed:?}"
    );
}

/// (2) **no manager**: when the `SubagentSessionManager` is absent
/// (`None`), the `subagent` tool returns an error result (the dispatch is
/// not available) — the tool does NOT panic or hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_subagent_tool_without_a_manager_returns_an_error_result() {
    let provider = MockProvider::new(vec![
        MockResponse::Stream(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "tc_subagent".to_string(),
                name: "subagent".to_string(),
                arguments: json!({ "task": "do the task", "agentName": "fake" }),
            }),
            ProviderEvent::Done(FinishReason::ToolCalls),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("done".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
    ]);

    let (prompt_tx, mut events, _sink, _store) = build_harness(provider, None).await;

    let _ = prompt_tx
        .send(Prompt {
            text: "go".to_string(),
            images: Vec::new(),
        })
        .await;

    // The `subagent` tool returned an ERROR result (no manager available).
    let evs = wait_for(&mut events, Duration::from_secs(10), |evs| {
        evs.iter()
            .any(|ev| matches!(ev, RpcEvent::tool_execution_end { is_error: true, .. }))
    })
    .await
    .expect("the subagent tool should complete (with an error)");

    let tool_result = evs
        .iter()
        .find_map(|ev| match ev {
            RpcEvent::tool_execution_end { result, .. } => Some(extract_result_text(result)),
            _ => None,
        })
        .expect("the tool_execution_end carries the result");
    assert!(
        tool_result.contains("not available"),
        "the error result must say the subagent dispatch is not available — got: {tool_result:?}"
    );
}
