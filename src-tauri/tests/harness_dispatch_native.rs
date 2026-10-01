//! End-to-end tests for `SubagentSessionManager::dispatch_native` (the
//! native-native subagent — Task 2): an in-process child `AgentLoop`
//! (NO pi process, NO `WorkerRuntime`) runs a task, streams the child's
//! frames to the parent's real sink EXACTLY ONCE (the `CapturingSink`
//! forwards — it never re-emits), and resolves a `SubagentOutcome`
//! (the unconditional teardown tears the child down on EVERY exit —
//! settle / timeout / cancel / preflight error).
//!
//! The oneshot is resolved by the driver task ONLY after the teardown
//! (`loop_handle.await` + the `events_rx` drain to `None` — the child
//! loop task has ended, its `events_tx` is dropped, `events_rx` is
//! closed), so a bounded oneshot resolution is the observable proof of
//! the unconditional teardown (a missing `abort()` would hang the
//! driver on a child stuck in a hanging provider read → the test
//! times out).

mod common;
use common::{MockProvider, MockResponse, RecSink};

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use archimedes_lib::agent::harness::{
    AgentLoop, ChatRole, FinishReason, Model, ModelCatalog, ModelRequest, Prompt, Provider,
    ProviderError, ProviderEvent, RetryPolicy, SessionStore, SudoDeps, ToolCall,
};
use archimedes_lib::agent::subagent::{
    LaunchConfig, NativeDeps, SubagentOutcome, SubagentSessionManager,
};
use archimedes_lib::agent::{EventSink, ProviderFactory, RpcEvent, SessionInfo, TodoStore};
use archimedes_lib::storage::Db;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::{watch, Mutex as TokioMutex};
use tokio_util::sync::CancellationToken;

const FAKE_PI: &str = env!("CARGO_BIN_EXE_fake_pi");

/// A config dir with ONE `fake` registry entry (the `dispatch_native`
/// driver never spawns it — the registry just has to load).
fn temp_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("native-dispatch-cfg-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "agents": [
                { "id": "fake", "name": "Fake", "command": FAKE_PI, "args": [] }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    dir
}

fn test_model(id: &str, provider: &str, thinking_levels: Vec<String>) -> Model {
    Model {
        id: id.to_string(),
        provider: provider.to_string(),
        base_url: "http://fake/v1".to_string(),
        api_key: "k".to_string(),
        context_window: 100_000,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: !thinking_levels.is_empty(),
        thinking_levels,
        api: Some("openai-completions".to_string()),
    }
}

fn test_catalog() -> (Model, ModelCatalog) {
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    (m1, catalog)
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

/// Build a `SubagentSessionManager` with `NativeDeps` set (a mock
/// `provider_factory` + the given catalog + a short-ish settle bound;
/// `trust_db: None` — fail-closed). The `trust_db`-threading variant is
/// [`make_manager_with_trust`].
fn make_manager(
    config_dir: &Path,
    catalog: ModelCatalog,
    provider: Arc<dyn Provider>,
    settle_timeout: Duration,
) -> Arc<SubagentSessionManager> {
    make_manager_with_trust(config_dir, catalog, provider, settle_timeout, None)
}

/// The `trust_db`-threading variant of [`make_manager`] (ADR 0010 — the
/// trust-inheritance test: the child's permission gate looks the Space up
/// in `trust_db`; `Some` = the parent's trust is inherited, `None` =
/// fail-closed).
fn make_manager_with_trust(
    config_dir: &Path,
    catalog: ModelCatalog,
    provider: Arc<dyn Provider>,
    settle_timeout: Duration,
    trust_db: Option<Arc<Db>>,
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
        trust_db,
    });
    manager
}

fn rec_sink() -> (Arc<dyn EventSink>, mpsc::UnboundedReceiver<(String, Value)>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(RecSink { tx }), rx)
}

/// Drain the sink until `pred` (bounded — the tests must not hang).
async fn collect_until(
    rx: &mut mpsc::UnboundedReceiver<(String, Value)>,
    timeout: Duration,
    mut pred: impl FnMut(&[(String, Value)]) -> bool,
) -> Vec<(String, Value)> {
    let mut events: Vec<(String, Value)> = Vec::new();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        if pred(&events) {
            return events;
        }
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(ev) => events.push(ev),
                None => return events,
            },
            _ = &mut deadline => return events,
        }
    }
}

/// The default `LaunchConfig` (all `None` — inherit the parent's).
fn default_launch() -> LaunchConfig {
    LaunchConfig {
        system_prompt: None,
        model: None,
        thinking: None,
        tools: None,
    }
}

/// A `Provider` whose stream emits the canned events, then HANGS (no
/// `Done` — `stream.next()` never resolves: the child cannot settle, so
/// the driver's `settle_timeout` + the unconditional teardown must win).
struct HangingStreamProvider {
    events: Vec<ProviderEvent>,
}

#[async_trait]
impl Provider for HangingStreamProvider {
    async fn complete(
        &self,
        _req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Ok(Box::pin(
            futures_util::stream::iter(self.events.clone())
                .chain(futures_util::stream::pending::<ProviderEvent>()),
        ))
    }
}

/// A `Provider` recording the `ModelRequest`s (the `system_prompt` test
/// asserts on `req.messages[0]`).
struct RecordingProvider {
    responses: StdMutex<VecDeque<MockResponse>>,
    requests: Arc<StdMutex<Vec<ModelRequest>>>,
}

impl RecordingProvider {
    fn new(responses: Vec<MockResponse>) -> (Self, Arc<StdMutex<Vec<ModelRequest>>>) {
        let requests = Arc::new(StdMutex::new(Vec::new()));
        (
            Self {
                responses: StdMutex::new(responses.into()),
                requests: requests.clone(),
            },
            requests,
        )
    }
}

#[async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        self.requests.lock().unwrap().push(req.clone());
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

/// (1) **the core**: `dispatch_native` with `NativeDeps` set spawns an
/// in-process child `AgentLoop`, runs the task, streams the child's
/// frames to the parent's real sink EXACTLY ONCE (the `CapturingSink`
/// forwards — never re-emits), and resolves `Completed { output }`.
/// After `Completed`, the child's `events_rx` is closed (the oneshot is
/// resolved ONLY after the unconditional teardown — `loop_handle.await`
/// + the `events_rx` drain to `None` — so a bounded resolution proves
/// the loop task ended; a missing `abort()` would hang the driver).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_runs_the_child_and_streams_frames_exactly_once() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    let provider: Arc<dyn Provider> =
        Arc::new(MockProvider::new(vec![MockResponse::Stream(vec![
            ProviderEvent::TextDelta("hello ".to_string()),
            ProviderEvent::TextDelta("world".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ])]));
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(20));
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    // The oneshot resolves `Completed` WITHIN THE BOUND (the driver
    // resolves it only after the unconditional teardown).
    let outcome = tokio::time::timeout(Duration::from_secs(30), dispatch_rx)
        .await
        .expect("the dispatch must resolve (the teardown cannot hang)")
        .expect("the oneshot must not be dropped");
    let SubagentOutcome::Completed { output, metrics } = outcome else {
        panic!("expected Completed, got {outcome:?}");
    };
    assert_eq!(output, "hello world", "the last message's accumulated text");
    assert_eq!(metrics.output, "hello world");
    assert!(
        metrics.duration_ms > 0,
        "the wall-clock duration is recorded"
    );

    // The sink saw the child's frames EXACTLY ONCE (one
    // `agent_message_chunk` per delta — NOT twice).
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let started = events
        .iter()
        .find(|(e, _)| e == "subagent-session-started")
        .map(|(_, p)| p.clone())
        .expect("a subagent-session-started event must fire");
    // The RESOLVED config is observable (the composed `provider/id` form,
    // the resolved thinking level, the child's tool set).
    assert_eq!(started["model"], "fake/m1", "the inherited parent model");
    assert!(
        started["thinkingLevel"].is_null(),
        "no thinking level → null (got {:?})",
        started["thinkingLevel"]
    );
    let tools = started["enabledTools"]
        .as_array()
        .expect("enabledTools is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    // The recursion guard: the child's tools EXCLUDE `subagent` (an empty
    // parent set = all → the `tool_specs` names minus `subagent`).
    assert!(
        !tools.contains(&"subagent".to_string()),
        "the child's enabledTools must exclude `subagent` — got {tools:?}"
    );
    assert!(
        tools.contains(&"bash".to_string()) && tools.contains(&"read".to_string()),
        "an empty parent set = all — the child inherits the tool set — got {tools:?}"
    );

    let chunks: Vec<&Value> = events
        .iter()
        .filter(|(e, p)| {
            e == "session-update" && p["update"]["sessionUpdate"] == "agent_message_chunk"
        })
        .map(|(_, p)| &p["update"])
        .collect();
    assert_eq!(
        chunks.len(),
        2,
        "each delta frame is emitted EXACTLY ONCE (not twice) — got {} frames",
        chunks.len()
    );
    let deltas: Vec<String> = chunks
        .iter()
        .map(|u| u["content"]["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(deltas, vec!["hello ".to_string(), "world".to_string()]);

    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("a subagent-closed event must fire");
    assert_eq!(closed["status"], "completed");
    assert_eq!(closed["metrics"]["output"], "hello world");
}

/// (2) **not configured**: a manager that never got `set_native_deps`
/// resolves an ALREADY-RESOLVED oneshot with `Failed { "native subagent
/// dispatch is not configured" }` (NO pi fallback — a native parent
/// always has an attached `db` in production).
#[test]
fn dispatch_native_without_native_deps_fails_immediately() {
    let config_dir = temp_config_dir();
    let manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone(), None)
            .expect("subagent manager should build"),
    );
    let (parent_model, _) = test_catalog();
    let (sink, _rx) = rec_sink();
    let (mut dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );
    let outcome = dispatch_rx
        .try_recv()
        .expect("the oneshot must be already resolved");
    assert!(
        matches!(
            outcome,
            SubagentOutcome::Failed { ref error }
                if error == "native subagent dispatch is not configured"
        ),
        "the unconfigured manager fails immediately — got {outcome:?}"
    );
}

/// (3) **timeout + teardown**: a child whose model stream HANGS (no
/// `Done` — it cannot settle) is torn down by the driver's
/// `settle_timeout` + the unconditional teardown (`abort()` — the
/// belt-and-braces for a hung provider read): `Failed { "timed out" }`
/// + `subagent-closed { status: "failed" }`, WITHIN THE BOUND (a missing
/// teardown would hang the driver → the test times out).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_times_out_and_tears_down_a_hanging_child() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    let provider: Arc<dyn Provider> = Arc::new(HangingStreamProvider {
        events: vec![ProviderEvent::TextDelta("partial".to_string())],
    });
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(3));
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
        .await
        .expect("the dispatch must resolve (the timeout teardown cannot hang)")
        .expect("the oneshot must not be dropped");
    assert!(
        matches!(
            outcome,
            SubagentOutcome::Failed { ref error } if error == "timed out"
        ),
        "a hanging child settles Failed (timed out) — got {outcome:?}"
    );
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("a subagent-closed event must fire");
    assert_eq!(closed["status"], "failed");
    assert_eq!(closed["error"], "timed out");
}

/// (4) **`model` / `thinking` overrides**: `launch.model` (a second
/// catalog model) → `subagent-session-started.model` = that model;
/// `launch.thinking` (a valid level) → `thinkingLevel` = that level;
/// `launch.model = Some("other/m2:high")` with `thinking: None` →
/// `thinkingLevel` = `"high"` (the `:<level>` suffix); an INVALID level
/// is DROPPED (never sent upstream as a bogus `reasoning_effort`); an
/// UNKNOWN model is `Failed { "unknown model: …" }`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_model_and_thinking_overrides_reach_the_child() {
    let config_dir = temp_config_dir();
    let m1 = test_model("m1", "fake", Vec::new());
    let m2 = test_model("m2", "other", vec!["high".to_string(), "low".to_string()]);
    let catalog = ModelCatalog {
        models: vec![m1.clone(), m2.clone()],
        ..Default::default()
    };
    // One canned response per dispatch below.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(vec![
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("a".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("b".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("c".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("d".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("e".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
    ]));
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(10));
    let (sink, mut rx) = rec_sink();

    /// Run one dispatch + read its `subagent-session-started` payload.
    async fn run(
        manager: &SubagentSessionManager,
        m1: &Model,
        launch: LaunchConfig,
        sink: &Arc<dyn EventSink>,
        rx: &mut mpsc::UnboundedReceiver<(String, Value)>,
    ) -> (SubagentOutcome, Value) {
        let (dispatch_rx, _cancel) = manager.dispatch_native(
            "parent-1",
            &std::env::temp_dir(),
            m1,
            Vec::new(),
            "tester".to_string(),
            launch,
            "do the thing".to_string(),
            sink,
        );
        let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
            .await
            .expect("the dispatch must resolve")
            .expect("the oneshot must not be dropped");
        let events = collect_until(rx, Duration::from_secs(5), |evs| {
            evs.iter().any(|(e, _)| e == "subagent-closed")
        })
        .await;
        let started = events
            .iter()
            .rev()
            .find(|(e, _)| e == "subagent-session-started")
            .map(|(_, p)| p.clone())
            .expect("a subagent-session-started event must fire");
        (outcome, started)
    }

    // (a) `launch.model` = a second catalog model.
    let (outcome, started) = run(
        &manager,
        &m1,
        LaunchConfig {
            model: Some("other/m2".to_string()),
            ..default_launch()
        },
        &sink,
        &mut rx,
    )
    .await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the `model` override dispatch completes — got {outcome:?}"
    );
    assert_eq!(
        started["model"], "other/m2",
        "the override model is observable"
    );

    // (b) `launch.thinking` = a valid level of the override model.
    let (outcome, started) = run(
        &manager,
        &m1,
        LaunchConfig {
            model: Some("other/m2".to_string()),
            thinking: Some("high".to_string()),
            ..default_launch()
        },
        &sink,
        &mut rx,
    )
    .await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the `thinking` override dispatch completes — got {outcome:?}"
    );
    assert_eq!(
        started["thinkingLevel"], "high",
        "the explicit thinking override is observable"
    );

    // (c) The `:<level>` suffix (`thinking: None` → the suffix is used).
    let (outcome, started) = run(
        &manager,
        &m1,
        LaunchConfig {
            model: Some("other/m2:high".to_string()),
            thinking: None,
            ..default_launch()
        },
        &sink,
        &mut rx,
    )
    .await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the `:<level>` suffix dispatch completes — got {outcome:?}"
    );
    assert_eq!(
        started["model"], "other/m2",
        "the `:<level>` suffix is stripped off the model key"
    );
    assert_eq!(
        started["thinkingLevel"], "high",
        "the `:<level>` suffix is the thinking level when `launch.thinking` is `None`"
    );

    // (d) An INVALID level is DROPPED (the model's `thinking_levels`
    // does not include it — not sent upstream as a bogus
    // `reasoning_effort`).
    let (outcome, started) = run(
        &manager,
        &m1,
        LaunchConfig {
            model: Some("other/m2".to_string()),
            thinking: Some("bogus".to_string()),
            ..default_launch()
        },
        &sink,
        &mut rx,
    )
    .await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "an invalid level is dropped, not fatal — got {outcome:?}"
    );
    assert!(
        started["thinkingLevel"].is_null(),
        "an invalid level is dropped (null) — got {:?}",
        started["thinkingLevel"]
    );

    // (e) An UNKNOWN model is `Failed { "unknown model: …" }` (the driver
    // returns BEFORE any lifecycle event — no `subagent-*` frames).
    {
        let (dispatch_rx, _cancel) = manager.dispatch_native(
            "parent-1",
            &std::env::temp_dir(),
            &m1,
            Vec::new(),
            "tester".to_string(),
            LaunchConfig {
                model: Some("nope/unknown".to_string()),
                ..default_launch()
            },
            "do the thing".to_string(),
            &sink,
        );
        let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
            .await
            .expect("the dispatch must resolve")
            .expect("the oneshot must not be dropped");
        assert!(
            matches!(
                outcome,
                SubagentOutcome::Failed { ref error }
                    if error == "unknown model: nope/unknown"
            ),
            "an unknown model is a `Failed` — got {outcome:?}"
        );
    }
    // `m2` is unused except via the composed keys above.
    let _ = m2;
}

/// (5) **`system_prompt` override**: `launch.system_prompt = Some(…)`
/// seeds the child's provider transcript — the child's FIRST model
/// request carries a LEADING `System` message (the task prompt is the
/// SECOND message). ADR 0017: the system message is the prompt + the
/// todo guidance line (the parent's tools = `Vec::new()` = ALL → the
/// child HAS `manage_todo_list`), so the leading message is the
/// prompt + the todo line, not the prompt alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_system_prompt_seeds_the_child_transcript() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    let (provider, requests) = RecordingProvider::new(vec![MockResponse::Stream(vec![
        ProviderEvent::TextDelta("ok".to_string()),
        ProviderEvent::Done(FinishReason::Stop),
    ])]);
    let provider: Arc<dyn Provider> = Arc::new(provider);
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(10));
    let (sink, _rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        LaunchConfig {
            system_prompt: Some("You are terse.".to_string()),
            ..default_launch()
        },
        "do the thing".to_string(),
        &sink,
    );
    let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
        .await
        .expect("the dispatch must resolve")
        .expect("the oneshot must not be dropped");
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the `system_prompt` dispatch completes — got {outcome:?}"
    );

    // The child's first model request: a LEADING `System` message with
    // the prompt + the todo line (ADR 0017 — the child's tools = the
    // parent's `Vec::new()` = ALL minus `subagent` → the child HAS
    // `manage_todo_list`), then the task prompt.
    let reqs = requests.lock().unwrap();
    assert!(!reqs.is_empty(), "the child made a model request");
    let first = &reqs[0];
    assert_eq!(
        first.messages.first().map(|m| m.role),
        Some(ChatRole::System),
        "the first message is the seeded `System` prompt — got {:?}",
        first.messages.first().map(|m| &m.content)
    );
    match &first.messages.first().unwrap().content {
        archimedes_lib::agent::harness::MessageContent::Text(t) => {
            assert_eq!(
                t,
                "You are terse.\nUse manage_todo_list to track multi-step work — write the plan before starting, mark items completed as you go"
            )
        }
        other => panic!("the seeded prompt is text — got {other:?}"),
    }
    assert_eq!(
        first.messages.get(1).map(|m| m.role),
        Some(ChatRole::User),
        "the task prompt is the second message"
    );
}

/// (6) **`tools` override — EXECUTION behavior (not just the payload)**:
/// `launch.tools = Some(…)` is used VERBATIM (a non-empty `launch.tools`
/// that empties out after the `subagent` strip is NOT re-expanded).
/// `Some([])` / `Some(["subagent"])` = NO tools — a child that attempts a
/// tool call gets a `tool not enabled` error (NOT executed) and the model
/// is NOT advertised the stripped tool. `Some(["read"])` = the model IS
/// advertised `read` but NOT `bash` / `subagent`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_tools_override_controls_execution_and_advertised_specs() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();

    /// Run one dispatch with the given `launch.tools`, returning the
    /// `subagent-session-started` payload + the advertised tool names
    /// (the last `ModelRequest` the child sent — every model call
    /// advertises the same set) + the `read` tool result text (the
    /// `tool_call_update` `rawOutput.content[0].text`). The child model
    /// ATTEMPTS a `read` call (a NON-mutating tool — it skips the
    /// permission gate, so no 300 s prompt) then settles.
    async fn run_case(
        config_dir: &Path,
        catalog: &ModelCatalog,
        parent_model: &Model,
        tools: Option<Vec<String>>,
    ) -> (Value, Vec<String>, Option<String>) {
        let (provider, requests) = RecordingProvider::new(vec![
            MockResponse::Stream(vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "t1".to_string(),
                    name: "read".to_string(),
                    arguments: serde_json::json!({ "path": "/nonexistent-file" }),
                }),
                ProviderEvent::Done(FinishReason::ToolCalls),
            ]),
            MockResponse::Stream(vec![
                ProviderEvent::TextDelta("done".to_string()),
                ProviderEvent::Done(FinishReason::Stop),
            ]),
        ]);
        let provider: Arc<dyn Provider> = Arc::new(provider);
        let manager = make_manager(
            config_dir,
            catalog.clone(),
            provider,
            Duration::from_secs(10),
        );
        let (sink, mut rx) = rec_sink();
        let (dispatch_rx, _cancel) = manager.dispatch_native(
            "parent-1",
            &std::env::temp_dir(),
            parent_model,
            Vec::new(),
            "tester".to_string(),
            LaunchConfig {
                tools,
                ..default_launch()
            },
            "do the thing".to_string(),
            &sink,
        );
        let _ = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
            .await
            .expect("the dispatch must resolve")
            .expect("the oneshot must not be dropped");
        let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
            evs.iter().any(|(e, _)| e == "subagent-closed")
        })
        .await;
        let started = events
            .iter()
            .rev()
            .find(|(e, _)| e == "subagent-session-started")
            .map(|(_, p)| p.clone())
            .expect("a subagent-session-started event must fire");
        // The advertised tool names (the LAST `ModelRequest` the child
        // sent — every model call advertises the same set).
        let reqs = requests.lock().unwrap().clone();
        let advertised: Vec<String> = reqs
            .last()
            .map(|r| r.tools.iter().map(|t| t.name.clone()).collect())
            .unwrap_or_default();
        // The `read` tool result text (the `tool_call_update`
        // `rawOutput.content[0].text`).
        let read_result = events
            .iter()
            .filter(|(e, p)| {
                e == "session-update" && p["update"]["sessionUpdate"] == "tool_call_update"
            })
            .find_map(|(_, p)| {
                p["update"]["rawOutput"]["content"][0]["text"]
                    .as_str()
                    .map(|s| s.to_string())
            });
        (started, advertised, read_result)
    }

    // `launch.tools = Some([])` → NO tools: the model is NOT advertised
    // `read` / `bash`, and a `read` attempt is a `tool not enabled`
    // error (NOT executed).
    let (started, advertised, read_result) =
        run_case(&config_dir, &catalog, &parent_model, Some(Vec::new())).await;
    let tools = started["enabledTools"]
        .as_array()
        .expect("enabledTools is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        tools,
        Vec::<String>::new(),
        "an empty `launch.tools` = no tools"
    );
    assert!(
        !advertised.iter().any(|n| n == "bash"),
        "the model is NOT advertised `bash` (no tools) — got {advertised:?}"
    );
    assert!(
        !advertised.iter().any(|n| n == "read"),
        "the model is NOT advertised `read` (no tools) — got {advertised:?}"
    );
    assert_eq!(
        read_result.as_deref(),
        Some("tool not enabled: read"),
        "the `read` attempt is a `tool not enabled` error (NOT executed)"
    );

    // `launch.tools = Some(["subagent"])` → the `subagent` is stripped →
    // `[]` (no tools — NOT re-expanded): the same behavior as `Some([])`.
    let (started, advertised, read_result) = run_case(
        &config_dir,
        &catalog,
        &parent_model,
        Some(vec!["subagent".to_string()]),
    )
    .await;
    let tools = started["enabledTools"]
        .as_array()
        .expect("enabledTools is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        tools,
        Vec::<String>::new(),
        "a `launch.tools` that empties out after the `subagent` strip = no tools"
    );
    assert!(
        !advertised.iter().any(|n| n == "read"),
        "the model is NOT advertised `read` (no tools) — got {advertised:?}"
    );
    assert_eq!(
        read_result.as_deref(),
        Some("tool not enabled: read"),
        "the `read` attempt is a `tool not enabled` error (NOT executed)"
    );

    // `launch.tools = Some(["read"])` → the model IS advertised `read`
    // but NOT `bash` / `subagent`, and the `read` attempt IS executed
    // (NOT a `tool not enabled` error).
    let (started, advertised, read_result) = run_case(
        &config_dir,
        &catalog,
        &parent_model,
        Some(vec!["read".to_string()]),
    )
    .await;
    let tools = started["enabledTools"]
        .as_array()
        .expect("enabledTools is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(tools, vec!["read".to_string()]);
    assert!(
        advertised.iter().any(|n| n == "read"),
        "the model IS advertised `read` — got {advertised:?}"
    );
    assert!(
        !advertised.iter().any(|n| n == "bash"),
        "the model is NOT advertised `bash` — got {advertised:?}"
    );
    assert!(
        !advertised.iter().any(|n| n == "subagent"),
        "the model is NOT advertised `subagent` — got {advertised:?}"
    );
    assert_ne!(
        read_result.as_deref(),
        Some("tool not enabled: read"),
        "an enabled `read` is executed (NOT a `tool not enabled` error)"
    );
}

/// (7) **`CapturingSink` unit**: the ENVELOPED `session-update` frame
/// (`{ "sessionId", "update" }`) — the `agent_message_chunk` deltas are
/// accumulated per `messageId` + the `last_message_id`'s text is the
/// final output (a tool-using turn does NOT concatenate every
/// intermediate message); EVERY frame is forwarded to the wrapped real
/// sink EXACTLY ONCE (the decorator never re-emits).
#[test]
fn capturing_sink_accumulates_the_last_message_and_forwards_every_frame_once() {
    use archimedes_lib::agent::subagent::CapturingSink;

    let (real_tx, mut real_rx) = mpsc::unbounded_channel();
    let real: Arc<dyn EventSink> = Arc::new(RecSink { tx: real_tx });
    let capturing = Arc::new(CapturingSink::new(real.clone()));

    // Two chunks of `m1` + one chunk of `m2` (the LAST message wins —
    // mirroring the driver's `captures()`).
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "type": "text", "text": "hello " } } }),
    );
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "type": "text", "text": "world" } } }),
    );
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "m2", "content": { "type": "text", "text": "final" } } }),
    );
    // A NON-chunk frame (forwarded, not captured).
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_thought_chunk", "messageId": "m1", "content": { "type": "text", "text": "thinking" } } }),
    );
    // A NON-`session-update` event (forwarded untouched).
    capturing.emit(
        "subagent-closed",
        serde_json::json!({ "sessionId": "s", "status": "completed" }),
    );

    // The `last_message_id`'s accumulated text is the output (NOT
    // `m1`'s + `m2`'s concatenated).
    assert_eq!(capturing.captured_text(), "final");
    let ids = capturing.captured_ids();
    assert_eq!(ids.len(), 2, "both `messageId`s were captured");
    assert!(ids.contains(&"m1".to_string()) && ids.contains(&"m2".to_string()));

    // EVERY frame was forwarded to the real sink EXACTLY ONCE (5 frames
    // — the decorator never re-emits).
    let mut forwarded = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        match real_rx.try_recv() {
            Ok(_) => forwarded += 1,
            Err(_) => break,
        }
    }
    assert_eq!(forwarded, 5, "every frame is forwarded exactly once");
}

/// (8) **`CapturingSink` unit**: an empty delta is SKIPPED (no capture,
/// but the frame is still forwarded).
#[test]
fn capturing_sink_skips_empty_deltas_but_forwards_the_frame() {
    use archimedes_lib::agent::subagent::CapturingSink;

    let (real_tx, mut real_rx) = mpsc::unbounded_channel();
    let real: Arc<dyn EventSink> = Arc::new(RecSink { tx: real_tx });
    let capturing = Arc::new(CapturingSink::new(real));

    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "type": "text", "text": "" } } }),
    );
    assert_eq!(
        capturing.captured_text(),
        "",
        "an empty delta is not captured"
    );
    assert!(capturing.captured_ids().is_empty());
    assert!(
        real_rx.try_recv().is_ok(),
        "the empty-delta frame was still forwarded"
    );
}

/// (8b) **`CapturingSink` unit**: the normalizer's bookkeeping chunks
/// (the dedicated `messageId: "system"` — "Retry failed", "Compacting
/// context…") are NOT captured (a failed child turn settles with the
/// LAST captured chunk as its output — a bookkeeping one-liner must
/// not hijack the subagent's answer), but the frame is still forwarded
/// to the real sink EXACTLY ONCE (the user sees the one-liner).
#[test]
fn capturing_sink_skips_system_chunks_but_forwards_them() {
    use archimedes_lib::agent::subagent::CapturingSink;

    let (real_tx, mut real_rx) = mpsc::unbounded_channel();
    let real: Arc<dyn EventSink> = Arc::new(RecSink { tx: real_tx });
    let capturing = Arc::new(CapturingSink::new(real));

    // A real message, then a `system` bookkeeping chunk, then another
    // real message.
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": { "type": "text", "text": "hello " } } }),
    );
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "system", "content": { "type": "text", "text": "Retry failed" } } }),
    );
    // The `system` chunk is NOT captured (the last real message still
    // wins — NOT the bookkeeping one-liner).
    assert_eq!(
        capturing.captured_text(),
        "hello ",
        "a `system` chunk must not hijack the capture"
    );
    assert!(
        !capturing.captured_ids().contains(&"system".to_string()),
        "the `system` id is not captured"
    );
    // A subsequent real message IS captured (the last real message
    // wins — the skip is per-chunk, not sticky).
    capturing.emit(
        "session-update",
        serde_json::json!({ "sessionId": "s", "update": { "sessionUpdate": "agent_message_chunk", "messageId": "m2", "content": { "type": "text", "text": "final" } } }),
    );
    assert_eq!(capturing.captured_text(), "final");

    // EVERY frame was forwarded to the real sink EXACTLY ONCE (the
    // `system` chunk included — the user sees the one-liner).
    for _ in 0..3 {
        assert!(real_rx.try_recv().is_ok(), "a frame was forwarded");
    }
    assert!(
        real_rx.try_recv().is_err(),
        "exactly 3 frames were forwarded (no re-emit)"
    );
}

/// (9) **`AgentLoop` getters + `prepend_system`**: the new accessors read
/// the private fields; `prepend_system` pushes a `System` message to the
/// FRONT of the provider transcript (the `model_request` sends
/// `messages: self.messages.clone()`).
#[test]
fn agent_loop_getters_and_prepend_system() {
    use archimedes_lib::agent::harness::AgentLoop;
    use archimedes_lib::agent::harness::{ModelCatalog, RetryPolicy, SessionStore};
    use archimedes_lib::agent::{EventSink, TodoStore};
    use archimedes_lib::storage::Db;
    use tokio::sync::{mpsc, watch, Mutex};
    use tokio_util::sync::CancellationToken;

    let dir =
        std::env::temp_dir().join(format!("native-dispatch-getters-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.db")).expect("db should open"));
    db.record_session(&archimedes_lib::agent::SessionInfo {
        session_id: "s1".to_string(),
        agent_id: "native".to_string(),
        cwd: std::path::PathBuf::from("/tmp"),
        capabilities: serde_json::json!({}),
        config_options: None,
        archived: false,
    })
    .expect("record_session");
    let model = test_model("m1", "fake", vec!["high".to_string()]);
    let (prompt_tx, prompt_rx) = mpsc::channel(8);
    let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
    let mut loop_ = AgentLoop::new(
        "s1".to_string(),
        dir.clone(),
        model,
        Box::new(NoopProvider),
        ModelCatalog::default(),
        SessionStore::new(db),
        mpsc::channel(8).0,
        CancellationToken::new(),
        Arc::new(StdMutex::new(CancellationToken::new())),
        watch::channel(0u64).0,
        prompt_tx,
        prompt_rx,
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        None,
        sink,
        Arc::new(TodoStore::new()),
        None,
        SudoDeps::default(),
        RetryPolicy::new(),
    );
    // The getters (before configuration — the defaults).
    assert!(loop_.enabled_tools().is_none(), "None = all (the default)");
    assert!(loop_.thinking_level().is_none());
    loop_.set_enabled_tools(Some(vec!["read".to_string()]));
    loop_.set_thinking_level(Some("high".to_string()));
    assert_eq!(loop_.enabled_tools(), Some(&["read".to_string()][..]));
    assert_eq!(loop_.thinking_level(), Some("high"));
    // `prepend_system` compiles + runs (a `&mut` transcript seed — the
    // END-TO-END assertion is the `system_prompt` test above: the child's
    // first model request carries the leading `System` message).
    loop_.prepend_system("You are terse.".to_string());
}

/// (10) **cancel + teardown**: a parent turn-cancel (the
/// `SubagentCancel` handle's `cancel()` — `dispatch_subagent`'s
/// `turn.cancelled()` arm calls it) tears a HANGING child down (no
/// `Done` — the child cannot settle on its own): `Failed
/// { "cancelled" }` + `subagent-closed { status: "failed", error:
/// "cancelled" }`, WITHIN THE BOUND (the oneshot is resolved by the
/// driver ONLY after the unconditional teardown — `loop_handle.await`
/// + the `events_rx` drain to `None` — so a bounded resolution proves
/// the child loop task ended, its `events_tx` is dropped, `events_rx`
/// is closed: the teardown, NOT just a detached `JoinHandle`; a
/// missing `abort()` would hang the driver → the test times out).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_cancel_tears_down_a_hanging_child() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    let provider: Arc<dyn Provider> = Arc::new(HangingStreamProvider {
        events: vec![ProviderEvent::TextDelta("partial".to_string())],
    });
    // A LONG `settle_timeout` — the CANCEL arm must win (NOT the
    // timeout): a bounded resolution far short of the settle bound
    // proves the `cancel_rx.changed()` arm fired the teardown.
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(30));
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    // Simulate a parent turn-cancel (`dispatch_subagent`'s
    // `turn.cancelled()` arm calls `cancel.cancel()`).
    cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(10), dispatch_rx)
        .await
        .expect("the dispatch must resolve (the cancel teardown cannot hang)")
        .expect("the oneshot must not be dropped");
    assert!(
        matches!(
            outcome,
            SubagentOutcome::Failed { ref error } if error.contains("cancelled")
        ),
        "a cancelled child settles Failed (cancelled) — got {outcome:?}"
    );
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("a subagent-closed event must fire");
    assert_eq!(closed["status"], "failed");
    assert_eq!(closed["error"], "cancelled");
}

/// A `Provider` whose `complete()` ALWAYS fails with a `Retryable`
/// error (retry exhaustion: the turn makes `max_attempts` model calls,
/// emitting the `auto_retry_start` / `auto_retry_end` bookkeeping
/// one-liners — `system`-id `agent_message_chunk`s — then settles via
/// `settle_error` → `agent_settled`).
struct AlwaysFailingProvider;

#[async_trait]
impl Provider for AlwaysFailingProvider {
    async fn complete(
        &self,
        _req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        Err(ProviderError::Retryable(
            "mock: the model call always fails".to_string(),
        ))
    }
}

/// (10b) **a failed child turn**: the model call exhausts the retry
/// budget (the `auto_retry_start` / `auto_retry_end` bookkeeping
/// one-liners — `system`-id `agent_message_chunk`s — are emitted) and
/// the turn settles via `settle_error` → `agent_settled` (the driver's
/// `Race::Settled`): `Completed { output: "" }` (the documented behavior
/// — the bookkeeping text is NOT the subagent's answer), the
/// `subagent-closed` `metrics.output` = "", and the `system` frames
/// ARE still forwarded to the parent's real sink (the user sees
/// "Retry failed" — only the CAPTURE excludes them).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_failed_turn_reports_empty_output_not_bookkeeping() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    let provider: Arc<dyn Provider> = Arc::new(AlwaysFailingProvider);
    // The default `RetryPolicy` (5 attempts, 1 s base — 1s, 2s, 4s, 8s
    // = 15 s of backoff) — a `settle_timeout` far beyond it.
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(60));
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    let outcome = tokio::time::timeout(Duration::from_secs(45), dispatch_rx)
        .await
        .expect("the dispatch must resolve (the backoff is bounded, 15 s)")
        .expect("the oneshot must not be dropped");
    let SubagentOutcome::Completed { output, metrics } = outcome else {
        panic!("a failed child turn settles `Completed` — got {outcome:?}");
    };
    // The bookkeeping one-liners ("Retry failed", "Retrying (…)") are
    // NOT the subagent's answer — the output is empty.
    assert_eq!(
        output, "",
        "a failed child turn reports `Completed {{ output: \"\" }}` — got {output:?}"
    );
    assert_eq!(metrics.output, "");

    // The `system` frames ARE still forwarded to the parent's real
    // sink (the user sees the one-liner — only the capture excludes
    // them).
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let system_chunks: Vec<String> = events
        .iter()
        .filter(|(e, p)| {
            e == "session-update"
                && p["update"]["sessionUpdate"] == "agent_message_chunk"
                && p["update"]["messageId"] == "system"
        })
        .map(|(_, p)| p["update"]["content"]["text"].as_str().unwrap().to_string())
        .collect();
    assert!(
        system_chunks.iter().any(|t| t == "Retry failed"),
        "the `system` bookkeeping frame is forwarded to the UI — got {system_chunks:?}"
    );
    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("a subagent-closed event must fire");
    assert_eq!(closed["status"], "completed");
    assert_eq!(
        closed["metrics"]["output"], "",
        "the `subagent-closed` metrics carry the empty output"
    );
}

/// A no-op `EventSink` (the getter test).
struct NoopSink;

impl EventSink for NoopSink {
    fn emit(&self, _event: &str, _payload: Value) {}
}

/// A `Provider` that never resolves (the getter test never calls it).
struct NoopProvider;

#[async_trait]
impl Provider for NoopProvider {
    async fn complete(
        &self,
        _req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        futures_util::future::pending().await
    }
}

/// A `Provider` whose `complete()` PANICS (the child loop task dies — the
/// `settle_tx` it owns is dropped — the driver's `settle_rx` `changed()`
/// errors → `Race::Died`).
struct PanickingProvider;

#[async_trait]
impl Provider for PanickingProvider {
    async fn complete(
        &self,
        _req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        panic!("mock: the provider panics (the child loop task dies)");
    }
}

/// (11) **`Race::Died`**: a child whose loop task DIES (a panicking
/// `Provider` — the `settle_tx` it owns is dropped) is a `Failed {
/// "child loop task died" }` + `subagent-closed { status: "failed" }`,
/// NOT a `Completed` (the `settle_rx` `changed()` `Err` — the sender was
/// dropped — is a death signal, mirroring `drive_native_session`). A long
/// `settle_timeout` proves the `Died` arm (the dropped `settle_tx`), NOT
/// the timeout, wins; a bounded resolution proves the driver tears the
/// dead child down (the `events_rx` drain to `None` — the child task has
/// ended, its `events_tx` is dropped, `events_rx` is closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_a_dying_child_loop_is_a_failed_not_completed() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    let provider: Arc<dyn Provider> = Arc::new(PanickingProvider);
    // A LONG `settle_timeout` — the `Died` arm (the dropped `settle_tx`)
    // must win (NOT the timeout).
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(30));
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
        .await
        .expect("the dispatch must resolve (a dead child cannot hang the driver)")
        .expect("the oneshot must not be dropped");
    assert!(
        matches!(
            outcome,
            SubagentOutcome::Failed { ref error } if error == "child loop task died"
        ),
        "a dying child loop task is a `Failed` (child loop task died) — got {outcome:?}"
    );
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("a subagent-closed event must fire");
    assert_eq!(closed["status"], "failed");
    assert_eq!(closed["error"], "child loop task died");
}

/// (12) **the recursion guard at EXECUTION time**: a child whose model
/// ATTEMPTS a `subagent` call (a nested dispatch) is REJECTED — the child's
/// `enabled_tools` exclude `subagent` (the driver strips it), so the call is
/// a `tool not enabled` tool-result error (NOT executed, NOT a nested
/// dispatch). The child then settles `Completed`. EXACTLY ONE
/// `subagent-session-started` fires (the child — a nested dispatch would
/// fire a second).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_a_child_subagent_call_is_rejected_not_nested() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    // The child model ATTEMPTS a `subagent` call (response 1 — a nested
    // dispatch), then settles (response 2 — text only).
    let (provider, _requests) = RecordingProvider::new(vec![
        MockResponse::Stream(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "t1".to_string(),
                name: "subagent".to_string(),
                arguments: serde_json::json!({ "task": "nested task" }),
            }),
            ProviderEvent::Done(FinishReason::ToolCalls),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("done".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
    ]);
    let provider: Arc<dyn Provider> = Arc::new(provider);
    let manager = make_manager(&config_dir, catalog, provider, Duration::from_secs(10));
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &config_dir,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
        .await
        .expect("the dispatch must resolve")
        .expect("the oneshot must not be dropped");
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the child settles (the `subagent` attempt is a tool-result error, not fatal) — got {outcome:?}"
    );
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    // EXACTLY ONE `subagent-session-started` (the child — a nested dispatch
    // would fire a second; the `subagent` attempt was rejected, not dispatched).
    let started_count = events
        .iter()
        .filter(|(e, _)| e == "subagent-session-started")
        .count();
    assert_eq!(
        started_count, 1,
        "a nested dispatch did NOT fire — got {started_count} started events"
    );
    // The `subagent` tool result is a `tool not enabled` error (NOT executed,
    // NOT a nested dispatch).
    let subagent_result = events
        .iter()
        .filter(|(e, p)| {
            e == "session-update" && p["update"]["sessionUpdate"] == "tool_call_update"
        })
        .find_map(|(_, p)| {
            p["update"]["rawOutput"]["content"][0]["text"]
                .as_str()
                .map(|s| s.to_string())
        });
    assert_eq!(
        subagent_result.as_deref(),
        Some("tool not enabled: subagent"),
        "the child's `subagent` attempt is a `tool not enabled` error — got {subagent_result:?}"
    );
}

/// (13) **a parent-turn cancel THROUGH `dispatch_subagent`** (the
/// end-to-end propagation path — test 10 cancels `SubagentCancel` DIRECTLY;
/// this one cancels the PARENT's `turn` token, which `dispatch_subagent`'s
/// `select!` arm turns into a `cancel.cancel()`): a native parent
/// `AgentLoop` (a real in-process loop with `subagent: Some(manager)`) emits
/// a `subagent` tool call → `dispatch_subagent` spawns a HANGING child and
/// blocks on its `select!` → the parent's `turn` token is cancelled →
/// `dispatch_subagent`'s `turn.cancelled()` arm fires `cancel.cancel()` →
/// the child driver's `cancel_rx` arm tears the child down: `subagent-closed
/// { status: "failed", error: "cancelled" }` (the proof the parent's `turn`
/// token reached the child) + the parent's `subagent` tool result is
/// `cancelled` (the `SubagentWait::Cancelled` mapping).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_subagent_a_parent_turn_cancel_propagates_to_the_hanging_child() {
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    // The child HANGS (no `Done` — it cannot settle on its own); a LONG
    // `settle_timeout` so only the CANCEL (not the timeout) tears it down.
    let child_provider: Arc<dyn Provider> = Arc::new(HangingStreamProvider {
        events: vec![ProviderEvent::TextDelta("partial".to_string())],
    });
    let manager = make_manager(
        &config_dir,
        catalog.clone(),
        child_provider,
        Duration::from_secs(30),
    );

    // The PARENT `AgentLoop` (a real in-process loop — `subagent: Some` so
    // the `subagent` tool call reaches `dispatch_subagent`; `enabled_tools`
    // `None` = all, so the `subagent` call passes the filter; `trust_db`
    // `None` — the parent emits a `subagent` call, NOT a mutating one, so
    // no gate). Its model emits a `subagent` tool call, then (unreached —
    // the turn is cancelled first) would settle.
    let parent_dir =
        std::env::temp_dir().join(format!("native-dispatch-parent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&parent_dir).unwrap();
    let db = Arc::new(Db::open(&parent_dir.join("t.db")).expect("db should open"));
    db.record_session(&SessionInfo {
        session_id: "parent-1".to_string(),
        agent_id: "native".to_string(),
        cwd: parent_dir.clone(),
        capabilities: serde_json::json!({}),
        config_options: None,
        archived: false,
    })
    .expect("record_session");
    let (parent_provider, _requests) = RecordingProvider::new(vec![MockResponse::Stream(vec![
        ProviderEvent::ToolCall(ToolCall {
            id: "t1".to_string(),
            name: "subagent".to_string(),
            arguments: serde_json::json!({ "task": "hang" }),
        }),
        ProviderEvent::Done(FinishReason::ToolCalls),
    ])]);
    let (events_tx, mut events_rx) = mpsc::channel(256);
    let (prompt_tx, prompt_rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let turn_cancel: Arc<StdMutex<CancellationToken>> =
        Arc::new(StdMutex::new(CancellationToken::new()));
    let (sink, mut rx) = rec_sink();
    let loop_ = AgentLoop::new(
        "parent-1".to_string(),
        parent_dir.clone(),
        parent_model.clone(),
        Box::new(BoxedProvider(Arc::new(parent_provider))),
        catalog.clone(),
        SessionStore::new(db),
        events_tx,
        cancel.clone(),
        turn_cancel.clone(),
        watch::channel(0u64).0,
        prompt_tx.clone(),
        prompt_rx,
        Arc::new(TokioMutex::new(std::collections::HashMap::new())),
        Arc::new(TokioMutex::new(std::collections::HashMap::new())),
        None,
        sink.clone(),
        Arc::new(TodoStore::new()),
        Some(manager),
        SudoDeps::default(),
        RetryPolicy::new(),
    );
    let _handle = tokio::spawn(loop_.run());

    // The parent's prompt → the model emits the `subagent` call →
    // `dispatch_subagent` spawns the (hanging) child.
    prompt_tx
        .send(Prompt {
            text: "dispatch a child".to_string(),
        })
        .await
        .expect("the prompt was queued");
    // Wait for the child to START (proof `dispatch_subagent` spawned it + is
    // now blocked on its `select!` — so the cancel below is honored).
    let _ = collect_until(&mut rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-session-started")
    })
    .await;

    // CANCEL THE PARENT'S `turn` TOKEN (NOT `SubagentCancel` directly — the
    // propagation path under test: `dispatch_subagent`'s `select!` arm turns
    // it into a `cancel.cancel()`).
    turn_cancel.lock().unwrap().cancel();

    // The child driver tears the child down: `subagent-closed { status:
    // "failed", error: "cancelled" }` — the proof the parent's `turn` token
    // reached the child (the end-to-end propagation).
    let events = collect_until(&mut rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("a subagent-closed event must fire");
    assert_eq!(
        closed["status"], "failed",
        "a parent-turn-cancelled child settles failed"
    );
    assert_eq!(
        closed["error"], "cancelled",
        "the `subagent-closed` error is `cancelled` (the parent's `turn` token propagated)"
    );
    // The parent's `subagent` tool result is `cancelled` (the
    // `SubagentWait::Cancelled` mapping — the `select!` arm returned it).
    let subagent_result = events
        .iter()
        .filter(|(e, p)| {
            e == "session-update" && p["update"]["sessionUpdate"] == "tool_call_update"
        })
        .find_map(|(_, p)| {
            p["update"]["rawOutput"]["content"][0]["text"]
                .as_str()
                .map(|s| s.to_string())
        });
    assert_eq!(
        subagent_result.as_deref(),
        Some("cancelled"),
        "the parent's `subagent` tool result is `cancelled` — got {subagent_result:?}"
    );
    // The parent's turn SETTLED (a cancelled turn settles `turn_end` +
    // `agent_settled` — `agent_settled` is on the `events` channel (a
    // control event, NOT a `session-update` sink frame) — the session
    // stays alive, matching the external `abort`).
    let mut settled = false;
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            ev = events_rx.recv() => match ev {
                Some(RpcEvent::agent_settled) => {
                    settled = true;
                    break;
                }
                Some(_) => {}
                None => break,
            },
            _ = &mut deadline => break,
        }
    }
    assert!(
        settled,
        "the parent's cancelled turn settled (`agent_settled` on the `events` channel)"
    );
}

/// A `Db` with a TRUSTED Space (the trust-inheritance test's fixture): a
/// `sessions` row + a `spaces` row flagged `trusted` for `space` (ADR 0010 —
/// the permission gate's `space_trusted` lookup auto-approves a trusted
/// Space's mutating tools).
fn trusted_space_db() -> (Arc<Db>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("native-dispatch-trust-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let space = dir.join("space");
    std::fs::create_dir_all(&space).unwrap();
    let db = Arc::new(Db::open(&dir.join("t.db")).expect("db should open"));
    db.record_session(&SessionInfo {
        session_id: "s1".to_string(),
        agent_id: "native".to_string(),
        cwd: space.clone(),
        capabilities: serde_json::json!({}),
        config_options: None,
        archived: false,
    })
    .expect("record_session");
    db.upsert_space(&space.display().to_string())
        .expect("upsert_space");
    db.set_space_trusted(&space.display().to_string(), true)
        .expect("set_space_trusted");
    (db, space)
}

/// (14) **trust inheritance (ADR 0010)**: a native child in a TRUSTED Space
/// inherits the parent's trust — the permission gate AUTO-APPROVES a mutating
/// tool (`bash`) (no `permission-request` prompt, the tool runs). This is the
/// `NativeDeps.trust_db` fix: `dispatch_native` threads `deps.trust_db` onto
/// the child `AgentLoop` (parity with the external `dispatch`, which threads
/// `driver.trust_db`). Pre-fix the child ran with `trust_db: None` (fail-closed
/// — the gate prompted on every `bash`, the child hung, the dispatch
/// `Failed { "timed out" }`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_native_a_trusted_space_child_auto_approves_mutating_tools() {
    let (db, space) = trusted_space_db();
    let config_dir = temp_config_dir();
    let (parent_model, catalog) = test_catalog();
    // The child model ATTEMPTS a `bash` call (a MUTATING tool — the
    // permission gate). In a TRUSTED Space the gate auto-approves (no
    // prompt); the `bash` runs + the child settles.
    let (provider, _requests) = RecordingProvider::new(vec![
        MockResponse::Stream(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "t1".to_string(),
                name: "bash".to_string(),
                arguments: serde_json::json!({ "command": "echo hello" }),
            }),
            ProviderEvent::Done(FinishReason::ToolCalls),
        ]),
        MockResponse::Stream(vec![
            ProviderEvent::TextDelta("done".to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ]),
    ]);
    let provider: Arc<dyn Provider> = Arc::new(provider);
    // `trust_db: Some(db)` — the child inherits the parent's trust.
    let manager = make_manager_with_trust(
        &config_dir,
        catalog,
        provider,
        Duration::from_secs(10),
        Some(db),
    );
    let (sink, mut rx) = rec_sink();

    let (dispatch_rx, _cancel) = manager.dispatch_native(
        "parent-1",
        &space,
        &parent_model,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );

    let outcome = tokio::time::timeout(Duration::from_secs(20), dispatch_rx)
        .await
        .expect("the dispatch must resolve (a trusted Space auto-approves — no 300 s prompt)")
        .expect("the oneshot must not be dropped");
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "a trusted-Space child completes (the `bash` was auto-approved) — got {outcome:?}"
    );
    let events = collect_until(&mut rx, Duration::from_secs(5), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    // The `bash` was AUTO-APPROVED (no `permission-request` prompt — the
    // trusted short-circuit returns BEFORE the prompt is emitted).
    let prompted = events.iter().any(|(e, _)| e == "permission-request");
    assert!(
        !prompted,
        "a TRUSTED Space auto-approves — NO `permission-request` prompt"
    );
    // The `bash` actually RAN (its `tool_call_update` is the command output,
    // NOT a `permission denied` error).
    let bash_result = events
        .iter()
        .filter(|(e, p)| {
            e == "session-update" && p["update"]["sessionUpdate"] == "tool_call_update"
        })
        .find_map(|(_, p)| {
            p["update"]["rawOutput"]["content"][0]["text"]
                .as_str()
                .map(|s| s.to_string())
        });
    assert!(
        bash_result
            .as_deref()
            .map(|s| s.contains("hello"))
            .unwrap_or(false),
        "the `bash` RAN (auto-approved, its output is captured) — got {bash_result:?}"
    );
    assert!(
        bash_result
            .as_deref()
            .map(|s| !s.contains("permission denied"))
            .unwrap_or(false),
        "the `bash` was NOT denied (auto-approved) — got {bash_result:?}"
    );
}
