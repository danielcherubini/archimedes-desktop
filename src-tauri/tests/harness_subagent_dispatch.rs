//! End-to-end test for the NATIVE `subagent` tool (the native-native
//! subagent): a NATIVE parent session (a mock `Provider` + the in-process
//! `AgentLoop`) issues a `subagent` tool call → `dispatch_subagent` → the
//! `SubagentSessionManager::dispatch_native` spawns an IN-PROCESS native
//! child `AgentLoop` (NO external `pi` process, NO `WorkerRuntime`) → the
//! child answers the task → the `ToolResult` carries the child's output
//! into the parent's transcript.
//!
//! This proves the native `subagent` tool EXECUTES end-to-end (not just
//! displays): the parent's `subagent` call spawns an in-process native
//! child, runs a turn, and captures the result — the whole flow through
//! the in-process harness.

mod common;
use common::{MockProvider, MockResponse, RecSink};

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use archimedes_desktop_lib::agent::bridge::PendingBridge;
use archimedes_desktop_lib::agent::harness::{
    FinishReason, Model, ModelCatalog, ModelRequest, Prompt, Provider, ProviderError,
    ProviderEvent, RetryPolicy, SessionStore, SudoDeps, ToolCall,
};
use archimedes_desktop_lib::agent::rpc::RpcEvent;
use archimedes_desktop_lib::agent::subagent::{NativeDeps, SubagentSessionManager};
use archimedes_desktop_lib::agent::{EventSink, PendingPermissions, ProviderFactory, TodoStore};
use archimedes_desktop_lib::storage::Db;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Mutex};
use tokio_util::sync::CancellationToken;

/// Write an `agents.json` with ONE `pi` entry: `command` = a NONEXISTENT
/// path (`/nonexistent/fake_pi`), `args` = `[]`, `bridge: true`. The
/// `pi` entry must exist (a MISSING `agents.json` makes `Registry::load`
/// fall back to `default_registry()` — a `pi` entry pointing at the REAL
/// `pi` binary, which the pre-fix `dispatch` (which looks up `"pi"`) would
/// then SPAWN); the nonexistent `command` makes the pre-fix spawn fail
/// deterministically (a red step) while staying inert (no real `pi`
/// process).
fn write_pi_agents_json(dir: &std::path::Path) {
    let json = json!({
        "agents": [
            {
                "id": "pi",
                "name": "Fake Pi",
                "command": "/nonexistent/fake_pi",
                "args": [],
                "bridge": true
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// The parent's `Model` (the `build_harness` model — the `dispatch_native`
/// driver resolves the CHILD's model from the catalog by this key, so the
/// catalog must contain it).
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

/// A `Provider` wrapper (the factory closure returns a CONCRETE type —
/// `Box<dyn Provider>` itself does not implement `Provider`).
struct BoxedProvider(Arc<dyn Provider>);

#[async_trait]
impl Provider for BoxedProvider {
    async fn complete(
        &self,
        req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.0.complete(req).await
    }
}

/// Build a `SubagentSessionManager` wired with `NativeDeps` (a mock
/// `provider_factory` — the child is an IN-PROCESS `AgentLoop`, NOT a
/// `fake_pi` process + a short settle bound).
fn make_manager(
    config_dir: &std::path::Path,
    catalog: ModelCatalog,
    provider: Arc<dyn Provider>,
    settle_timeout: Duration,
) -> Arc<SubagentSessionManager> {
    let manager = Arc::new(
        SubagentSessionManager::new(config_dir.to_path_buf(), None)
            .expect("subagent manager should build"),
    );
    let factory: ProviderFactory =
        Arc::new(move |_m: &Model| Box::new(BoxedProvider(provider.clone())));
    manager.set_native_deps(NativeDeps {
        provider_factory: factory,
        catalog,
        todo_store: Arc::new(TodoStore::new()),
        sudo: SudoDeps::default(),
        settle_timeout,
        trust_db: None,
    });
    manager
}

/// Build the native loop (a mock `Provider` + a temp `Db` + an OPTIONAL real
/// `SubagentSessionManager` — the `subagent` tool spawns a real sub-session).
async fn build_harness(
    provider: MockProvider,
    subagent_manager: Option<Arc<archimedes_desktop_lib::agent::SubagentSessionManager>>,
) -> (
    mpsc::Sender<archimedes_desktop_lib::agent::harness::Prompt>,
    mpsc::Receiver<RpcEvent>,
    mpsc::UnboundedReceiver<(String, Value)>,
    SessionStore,
) {
    let dir = std::env::temp_dir().join(format!("harness-subagent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("db.sqlite")).unwrap());
    db.record_session(&archimedes_desktop_lib::agent::SessionInfo {
        session_id: "ns1".to_string(),
        agent_id: "native".to_string(),
        cwd: PathBuf::from("/tmp"),
        capabilities: json!({}),
        config_options: None,
    })
    .unwrap();
    let store = SessionStore::new(db.clone());
    let (sink_tx, sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    let (events_tx, events_rx) = mpsc::channel(256);
    let (prompt_tx, prompt_rx) = mpsc::channel(8);
    let pending: PendingPermissions = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let pending_bridge: PendingBridge = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let cancel = CancellationToken::new();
    let turn_cancel: Arc<StdMutex<CancellationToken>> =
        Arc::new(StdMutex::new(CancellationToken::new()));
    let (settle_tx, _settle_rx) = watch::channel(0u64);
    let model = parent_model();
    let loop_ = archimedes_desktop_lib::agent::harness::AgentLoop::new(
        "ns1".to_string(),
        dir,
        model,
        Box::new(provider),
        Default::default(),
        store.clone(),
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
        subagent_manager,
        SudoDeps::default(),
        RetryPolicy::default(),
    );
    let task = tokio::spawn(loop_.run());
    // Reap the task on drop (the tests don't await it — the `events`
    // channel is the observation point).
    drop(task);
    (prompt_tx, events_rx, sink_rx, store)
}

/// Wait up to `timeout` for a predicate over the collected `RpcEvent`s.
async fn wait_for(
    events: &mut mpsc::Receiver<RpcEvent>,
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
/// `dispatch_subagent` → `dispatch_native` spawns an IN-PROCESS native
/// child `AgentLoop` (the manager is wired via `set_native_deps` with a
/// mock `provider_factory` — NOT a `fake_pi` process; the `pi` registry
/// entry's `command` points at a nonexistent path, so the pre-fix
/// `dispatch` (which looks up `pi`) would fail to spawn: deterministic red,
/// no real `pi` process) → the child answers the task (the mock provider's
/// turn) → the `ToolResult` carries the MOCK CHILD's output into the
/// parent's transcript (a `toolResult` message + the
/// `subagent-session-started` / `subagent-closed` sink events — NOT a
/// `fake_pi` "Hello", NOT an error).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_subagent_tool_spawns_an_in_process_native_child_and_captures_the_result() {
    // The config dir (a `pi` registry entry → a NONEXISTENT `command`
    // — the pre-fix `dispatch` looks up `pi` + fails to spawn it).
    let config_dir =
        std::env::temp_dir().join(format!("harness-subagent-cfg-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&config_dir).unwrap();
    write_pi_agents_json(&config_dir);

    // The subagent manager (wired with `NativeDeps` — a mock
    // `provider_factory`; the child is an IN-PROCESS `AgentLoop`). The
    // catalog contains the parent's model (`fake/fake-model` — the
    // `dispatch_native` driver resolves the child's model by that key).
    let parent_model = parent_model();
    let catalog = ModelCatalog {
        models: vec![parent_model.clone()],
        ..Default::default()
    };
    // The MOCK CHILD's provider: one turn answering the task.
    let child_provider: Arc<dyn Provider> =
        Arc::new(MockProvider::new(vec![MockResponse::Stream(vec![
            ProviderEvent::TextDelta("child output".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ])]));
    let subagent_manager = make_manager(
        &config_dir,
        catalog,
        child_provider,
        Duration::from_secs(15),
    );

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

    let (prompt_tx, mut events, mut sink, _store) =
        build_harness(provider, Some(subagent_manager)).await;

    let _ = prompt_tx
        .send(Prompt {
            text: "go".to_string(),
        })
        .await;

    // The `subagent` tool EXECUTED (a `tool_execution_end` with the mock
    // child's output — NOT a `fake_pi` "Hello", NOT an error: the child is
    // the in-process mock `Provider`, so the spawn of a nonexistent binary
    // can never have produced this result).
    let evs = wait_for(&mut events, Duration::from_secs(20), |evs| {
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

    // The tool result carried the MOCK CHILD's output (the in-process
    // `AgentLoop`'s turn — NOT a `fake_pi` "Hello": no external process
    // was spawned).
    let tool_result = evs
        .iter()
        .find_map(|ev| match ev {
            RpcEvent::tool_execution_end { result, .. } => Some(extract_result_text(result)),
            _ => None,
        })
        .expect("the tool_execution_end carries the result");
    assert!(
        tool_result.contains("child output"),
        "the mock child's output must be captured — got: {tool_result:?}"
    );
    assert!(
        !tool_result.contains("Hello"),
        "the child is in-process (the mock provider) — NOT a `fake_pi` process — got: {tool_result:?}"
    );

    // The subagent session lifecycle fired (an in-process child spawned +
    // closed).
    let mut sink_events: Vec<(String, Value)> = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(5));
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
            maybe = sink.recv() => match maybe {
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
        "a subagent-session-started event must fire (an in-process child spawned)"
    );
    // The `subagent-session-started` event carries a real `sessionId` (a
    // spawned child — not a spurious event).
    let started = sink_events
        .iter()
        .find(|(e, _)| e == "subagent-session-started")
        .map(|(_, p)| p.clone())
        .expect("the started event is present");
    let session_id = started
        .get("sessionId")
        .or_else(|| started.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !session_id.is_empty(),
        "the subagent-session-started event must carry a real sessionId — got: {started:?}"
    );
    assert!(
        sink_events.iter().any(|(e, _)| e == "subagent-closed"),
        "a subagent-closed event must fire (the child reaped)"
    );
    // The `subagent-closed` event reports a COMPLETED child (the mock
    // child's turn answered the task — not a spawn failure).
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
        closed["metrics"]["output"], "child output",
        "the closed event's metrics carry the mock child's output — got: {closed:?}"
    );
}

/// (2) **no manager**: when the `SubagentSessionManager` is absent (`None`),
/// the `subagent` tool returns an error result (the dispatch is not
/// available) — the tool does NOT panic or hang.
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
