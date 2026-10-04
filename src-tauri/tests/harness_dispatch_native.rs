//! End-to-end tests for the `dispatch_native` re-plumb (ADR 0025 Task 5):
//! the `SubagentSessionManager` DELEGATES to the `WorkerManager`'s
//! `dispatch_subagent` flow — a `fake_worker` child process per dispatch
//! (the in-process driver + the throwaway-DB / `CapturingSink` /
//! `NativeDeps` machinery is DELETED). The child's transcript persists as
//! a hidden ephemeral row (the `TranscriptPersister`'s `is_subagent`
//! flag — the `ipc.rs` coverage).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use archimedes_lib::agent::harness::{Model, ModelCatalog};
use archimedes_lib::agent::subagent::{
    LaunchConfig, SubagentCancel, SubagentOutcome, SubagentSessionManager,
};
use archimedes_lib::agent::worker::client::{WorkerError, WorkerHandle, WorkerInboundEvent};
use archimedes_lib::agent::worker::manager::{WorkerFactory, WorkerManager};
use archimedes_lib::agent::worker::protocol::{StartEnv, StartMode};
use archimedes_lib::agent::EventSink;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

/// The `fake_worker` fixture factory (the `WorkerFactory` seam — the
/// `CARGO_MANIFEST_DIR`/`target/debug` convention; `cargo test` builds
/// the bin targets).
struct FakeWorkerFactory;

impl WorkerFactory for FakeWorkerFactory {
    fn spawn(&self) -> Result<WorkerHandle, WorkerError> {
        WorkerHandle::spawn(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/debug/fake_worker"
        )))
    }
}

/// A no-op `EventSink` (the `dispatch_native` signature's `sink`
/// parameter — the re-plumb's no-op; the lifecycle events ride the
/// `WorkerManager`'s `sink_for` lookup).
struct NoopSink;
impl EventSink for NoopSink {
    fn emit(&self, _event: &str, _payload: Value) {}
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

/// Build a `SubagentSessionManager` on a `WorkerManager` (the ADR 0025
/// Task 5 re-plumb — the native dispatch runs in a `fake_worker`
/// process): the parent `p1` is `attach`ed with the given `catalog`
/// (the `dispatch_subagent` flow resolves the child's `model` against
/// it) + the given `trusted` flag (the trust-inheritance test) + a
/// settle bound. The `sink_for` lookup delivers the lifecycle events on
/// the given sink; the `on_event` closure re-emits the CHILD's
/// `SinkFrame`s on the sink too (the `session_info` Start echo — the
/// `systemPrompt` / `trusted` envelope assertion).
async fn make_manager(
    catalog: ModelCatalog,
    settle_timeout: Duration,
    sink: Arc<dyn EventSink>,
    trusted: bool,
) -> Arc<SubagentSessionManager> {
    let factory: Arc<dyn WorkerFactory> = Arc::new(FakeWorkerFactory);
    let on_event_sink = sink.clone();
    let wm = WorkerManager::new(
        factory,
        Arc::new(|_id: String, _code: Option<i32>| {}),
        Arc::new(
            move |_id: String, is_subagent: bool, evt: WorkerInboundEvent| {
                if is_subagent {
                    if let WorkerInboundEvent::SinkFrame { event, payload } = &evt {
                        on_event_sink.emit(event, payload.clone());
                    }
                }
            },
        ),
        Arc::new(move |id: &str| (id == "p1").then(|| sink.clone())),
    )
    .with_settle_timeout(settle_timeout);
    let wm = Arc::new(wm);
    let env = StartEnv::from_parts(
        "p1".to_string(),
        "/tmp/space".to_string(),
        StartMode::Fresh,
        None,
        catalog.models[0].clone(),
        catalog,
        None,
        trusted,
        None,
        "/tmp".to_string(),
        true,
        None,
    );
    wm.attach("p1", &env)
        .await
        .expect("the parent attach completes");
    let manager = Arc::new(SubagentSessionManager::new());
    manager.set_worker_manager(wm);
    manager
}

/// A `dispatch_native` call (the `SubagentSessionManager` → the
/// `WorkerManager` flow — the subagent runs in a `fake_worker`
/// process). The `sink` parameter is the re-plumb's no-op.
fn dispatch(
    manager: &SubagentSessionManager,
    launch: LaunchConfig,
    task: &str,
    parent_model: &Model,
) -> (oneshot::Receiver<SubagentOutcome>, SubagentCancel) {
    let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
    manager.dispatch_native(
        "p1",
        Path::new("/tmp/space"),
        parent_model,
        Vec::new(),
        "tester".to_string(),
        launch,
        task.to_string(),
        &sink,
    )
}

/// Bounded-await the dispatch outcome (a missing resolution is a test
/// failure, not a hang).
async fn outcome(rx: oneshot::Receiver<SubagentOutcome>, timeout: Duration) -> SubagentOutcome {
    tokio::time::timeout(timeout, rx)
        .await
        .expect("the dispatch must resolve (the teardown cannot hang)")
        .expect("the oneshot must not be dropped")
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
        frontmatter_thinking: None,
        tools: None,
    }
}

/// (1) **the core**: `dispatch_native` on a manager wired with a
/// `WorkerManager` delegates to the `dispatch_subagent` flow — a
/// `fake_worker` child runs the turn → `Completed` with the
/// `SubagentCapture`'s final text (the child's `SinkFrame`
/// `agent_message_chunk` stream — the `fake_worker`'s canned
/// `"canned answer"`), + the `subagent-session-started` /
/// `subagent-closed` lifecycle events on the PARENT's sink.
#[tokio::test]
async fn dispatch_native_runs_the_child_and_resolves_completed() {
    let (sink, mut sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(&manager, default_launch(), "do the thing", &m1);
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    match outcome {
        SubagentOutcome::Completed { output, metrics } => {
            assert_eq!(
                output, "canned answer",
                "the `SubagentCapture`'s final text (the child's `SinkFrame` `agent_message_chunk` stream)"
            );
            assert!(metrics.duration_ms >= 1, "the wall clock (ms)");
        }
        other => panic!("expected `Completed`, got {other:?}"),
    }
    // The `subagent-session-started` / `subagent-closed` lifecycle events
    // fired on the PARENT's sink (the `SubagentCapture`'s metrics ride
    // verbatim).
    let events = collect_until(&mut sink_rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-closed")
    })
    .await;
    let started = events
        .iter()
        .find(|(e, _)| e == "subagent-session-started")
        .map(|(_, p)| p.clone())
        .expect("the `subagent-session-started` fired on the parent's sink");
    assert_eq!(started["parentSessionId"], "p1");
    assert!(!started["sessionId"].as_str().unwrap_or("").is_empty());
    let closed = events
        .iter()
        .find(|(e, _)| e == "subagent-closed")
        .map(|(_, p)| p.clone())
        .expect("the `subagent-closed` fired on the parent's sink");
    assert_eq!(closed["status"], "completed");
    assert_eq!(closed["sessionId"], started["sessionId"], "the same child");
    assert_eq!(
        closed["metrics"]["output"], "canned answer",
        "the closed event's metrics carry the captured final text"
    );
}

/// (2) **crash**: a child that CRASHES (the `fake_worker`'s
/// `"__crash__"` prompt — `exit(137)`, NO `agent_settled`) → `Failed`
/// with the exit code in the error (the process exit is the crash
/// signal — NOT a `Completed` with empty output).
#[tokio::test]
async fn dispatch_native_a_crashed_child_is_failed_with_the_exit_code() {
    let (sink, _sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(&manager, default_launch(), "__crash__", &m1);
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    match outcome {
        SubagentOutcome::Failed { error } => {
            assert!(
                error.contains("137"),
                "the error carries the exit code — got: {error}"
            );
        }
        other => panic!("expected `Failed`, got {other:?}"),
    }
}

/// (3) **timeout**: a child that HANGS (the `fake_worker`'s
/// `"__slow__"` prompt — a 10 s sleep before settling) + a SHORT
/// `settle_timeout` → `Failed { error: "timed out" }` (the
/// `settle_timeout` bound wins — the child is reaped).
#[tokio::test]
async fn dispatch_native_times_out_and_reaps_a_hanging_child() {
    let (sink, _sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_millis(300), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(&manager, default_launch(), "__slow__", &m1);
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    match outcome {
        SubagentOutcome::Failed { error } => {
            assert_eq!(error, "timed out", "the current timeout outcome");
        }
        other => panic!("expected `Failed {{ timed out }}`, got {other:?}"),
    }
}

/// (4) **cancel**: a `SubagentCancel` (the `dispatch_native` return —
/// the re-plumb's `WorkerManager` `cancel_subagent` — the `send_abort`
/// + the drive's `Cancelled` outcome + the reap) on a hanging child →
/// `Failed { error: "cancelled" }` (the cancel wins the race).
#[tokio::test]
async fn dispatch_native_cancel_cancels_the_dispatch() {
    let (sink, _sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, cancel) = dispatch(&manager, default_launch(), "__slow__", &m1);
    // Give the child a moment to start (the `send_abort` needs a live
    // handle — the drive's preflight), then cancel.
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    match outcome {
        SubagentOutcome::Failed { error } => {
            assert_eq!(error, "cancelled", "the cancel outcome");
        }
        other => panic!("expected `Failed {{ cancelled }}`, got {other:?}"),
    }
}

/// (5) **not configured**: a manager that never got a `WorkerManager`
/// (`set_worker_manager` never called — a `SessionManager` that never
/// `attach_worker_manager`ed) → an ALREADY-RESOLVED `Failed` (the
/// dispatch is unavailable — NOT a hang).
#[test]
fn dispatch_native_without_a_worker_manager_fails_immediately() {
    let manager = SubagentSessionManager::new();
    let sink: Arc<dyn EventSink> = Arc::new(NoopSink);
    let m1 = test_model("m1", "fake", Vec::new());
    let (mut dispatch_rx, cancel) = manager.dispatch_native(
        "parent-1",
        Path::new("/tmp/space"),
        &m1,
        Vec::new(),
        "tester".to_string(),
        default_launch(),
        "do the thing".to_string(),
        &sink,
    );
    // The oneshot is ALREADY resolved (a `recv` on a resolved oneshot
    // is immediate — no `tokio` runtime needed).
    let outcome = dispatch_rx
        .try_recv()
        .expect("the oneshot is already resolved (not configured)");
    match outcome {
        SubagentOutcome::Failed { error } => {
            assert!(
                error.contains("not configured"),
                "the error says the dispatch is not configured — got: {error}"
            );
        }
        other => panic!("expected `Failed`, got {other:?}"),
    }
    // The `SubagentCancel` is a no-op (idempotent — never panics).
    cancel.cancel();
    cancel.cancel();
}

/// (6) **`model` / `thinking` overrides**: `launch.model` (a
/// `provider/id` key IN the parent's catalog + a `:<level>` suffix)
/// beats the parent's model, and the suffix's `thinking` level rides the
/// child's `StartEnv` (the `subagent-session-started` payload carries
/// the RESOLVED model + thinking).
#[tokio::test]
async fn dispatch_native_model_and_thinking_overrides_reach_the_child() {
    let (sink, mut sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let m2 = test_model("m2", "fake", vec!["low".to_string(), "high".to_string()]);
    let catalog = ModelCatalog {
        models: vec![m1.clone(), m2.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(
        &manager,
        LaunchConfig {
            model: Some("fake/m2:high".to_string()),
            ..Default::default()
        },
        "do the thing",
        &m1,
    );
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the override model is IN the catalog — the dispatch completes — got {outcome:?}"
    );
    // The `subagent-session-started` payload: the override model + the
    // suffix's thinking level.
    let events = collect_until(&mut sink_rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-session-started")
    })
    .await;
    let started = events
        .iter()
        .find(|(e, _)| e == "subagent-session-started")
        .map(|(_, p)| p.clone())
        .expect("the `subagent-session-started` fired");
    assert_eq!(
        started["model"], "fake/m2",
        "the override model (the `provider/id` key) rides the payload"
    );
    assert_eq!(
        started["thinkingLevel"], "high",
        "the `:<level>` suffix rides the payload"
    );
}

/// (7) **`system_prompt` override**: `launch.system_prompt = Some(…)` →
/// the child's `StartEnv.system_prompt` is the `build_child_system_message`
/// output (the prompt + the todo instructions — the child's `session_info`
/// Start echo carries it verbatim).
#[tokio::test]
async fn dispatch_native_system_prompt_reaches_the_child_start() {
    let (sink, mut sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(
        &manager,
        LaunchConfig {
            system_prompt: Some("be terse".to_string()),
            ..Default::default()
        },
        "do the thing",
        &m1,
    );
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the dispatch completes — got {outcome:?}"
    );
    // The child's `session_info` Start echo (re-emitted on the parent's
    // scope by the `on_event` router): the `systemPrompt` is the
    // `build_child_system_message` output — the prompt + the todo
    // instructions.
    let events = collect_until(&mut sink_rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, p)| {
            e == "session-update"
                && p["update"]["sessionUpdate"] == "session_info"
                && p["update"]["systemPrompt"].is_string()
        })
    })
    .await;
    let info = events
        .iter()
        .find(|(e, p)| {
            e == "session-update"
                && p["update"]["sessionUpdate"] == "session_info"
                && p["update"]["systemPrompt"].is_string()
        })
        .map(|(_, p)| p.clone())
        .expect("the child's `session_info` Start echo was re-emitted");
    let system_prompt = info["update"]["systemPrompt"].as_str().unwrap().to_string();
    assert!(
        system_prompt.starts_with("be terse"),
        "the override prompt is the system message's prefix — got: {system_prompt:?}"
    );
    assert!(
        system_prompt.contains("manage_todo_list"),
        "the `build_child_system_message` output carries the todo instructions — got: {system_prompt:?}"
    );
}

/// (8) **`tools` override**: `launch.tools = Some([…])` → the child's
/// `StartEnv.enabled_tools` is the override VERBATIM (an empty override
/// = NO tools — NOT re-expanded; the `subagent-session-started` payload
/// carries the resolved tool list).
#[tokio::test]
async fn dispatch_native_tools_override_reaches_the_child() {
    let (sink, mut sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(
        &manager,
        LaunchConfig {
            tools: Some(vec!["read".to_string()]),
            ..Default::default()
        },
        "do the thing",
        &m1,
    );
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the dispatch completes — got {outcome:?}"
    );
    // The `subagent-session-started` payload: the override tool list.
    let events = collect_until(&mut sink_rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, _)| e == "subagent-session-started")
    })
    .await;
    let started = events
        .iter()
        .find(|(e, _)| e == "subagent-session-started")
        .map(|(_, p)| p.clone())
        .expect("the `subagent-session-started` fired");
    assert_eq!(
        started["enabledTools"],
        json!(["read"]),
        "the override tool list rides the payload"
    );
}

/// (9) **trust inheritance**: a parent `attach`ed with `trusted: true`
/// → the child's `StartEnv.trusted` is `true` (the child's
/// `session_info` Start echo carries it — the child's permission gate
/// auto-confirms the trusted Space's `confirm`, ADR 0010).
#[tokio::test]
async fn dispatch_native_trust_inheritance_reaches_the_child() {
    let (sink, mut sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    // The parent is `attach`ed with `trusted: true` (the inheritance
    // source).
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(&manager, default_launch(), "do the thing", &m1);
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    assert!(
        matches!(outcome, SubagentOutcome::Completed { .. }),
        "the dispatch completes — got {outcome:?}"
    );
    // The child's `session_info` Start echo: `trusted` is `true` (the
    // parent's trust is inherited).
    let events = collect_until(&mut sink_rx, Duration::from_secs(10), |evs| {
        evs.iter().any(|(e, p)| {
            e == "session-update"
                && p["update"]["sessionUpdate"] == "session_info"
                && p["sessionId"] != "p1"
        })
    })
    .await;
    let info = events
        .iter()
        .find(|(e, p)| {
            e == "session-update"
                && p["update"]["sessionUpdate"] == "session_info"
                && p["sessionId"] != "p1"
        })
        .map(|(_, p)| p.clone())
        .expect("the child's `session_info` Start echo was re-emitted");
    assert_eq!(
        info["update"]["trusted"], true,
        "the parent's trust is inherited (the child's `StartEnv.trusted`)"
    );
}

/// (10) **unknown model**: `launch.model` a key NOT in the parent's
/// catalog → `Failed { error: "unknown model: …" }` (NO fallback — the
/// dispatch fails fast, the parent's model is NOT used).
#[tokio::test]
async fn dispatch_native_an_unknown_model_is_failed() {
    let (sink, _sink_rx) = rec_sink();
    let m1 = test_model("m1", "fake", Vec::new());
    let catalog = ModelCatalog {
        models: vec![m1.clone()],
        ..Default::default()
    };
    let manager = make_manager(catalog, Duration::from_secs(30), sink, true).await;
    let (dispatch_rx, _cancel) = dispatch(
        &manager,
        LaunchConfig {
            model: Some("fake/nope".to_string()),
            ..Default::default()
        },
        "do the thing",
        &m1,
    );
    let outcome = outcome(dispatch_rx, Duration::from_secs(30)).await;
    match outcome {
        SubagentOutcome::Failed { error } => {
            assert!(
                error.contains("unknown model"),
                "the error says the model is unknown — got: {error}"
            );
        }
        other => panic!("expected `Failed`, got {other:?}"),
    }
}

/// A recording `EventSink` (pushes EVERY event's `(name, payload)` to a
/// channel — the `subagent-session-started` / `subagent-closed` / the
/// child's `session_info` Start echo observation point).
struct RecSink {
    tx: mpsc::UnboundedSender<(String, Value)>,
}

impl EventSink for RecSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

fn rec_sink() -> (Arc<dyn EventSink>, mpsc::UnboundedReceiver<(String, Value)>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(RecSink { tx }), rx)
}
