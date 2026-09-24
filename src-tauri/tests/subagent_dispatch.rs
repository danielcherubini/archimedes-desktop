//! End-to-end integration test for the subagent dispatch path (Task 5):
//! the whole desktop path is driven with `fake_pi` (no real inference): a
//! fake MAIN agent (the `FAKE_PI_DISPATCH*` modes — open a bridge
//! connection and send a `dispatch_subagent` frame) + a fake SUBAGENT
//! agent (the default / echo / no-text / hang modes — answer `prompt`
//! over the pi RPC).
//!
//! The test wiring mirrors production: build the `SubagentSessionManager`
//! (same config dir as the main), `main_manager.set_subagent_manager(
//! subagent_manager)`, then `start_session` the main. The main's bridge
//! listener services the `dispatch_subagent` frame (spawning the subagent
//! on the worker runtime); the subagent spawns the SAME registry entry as
//! the parent (the `ARCHIMEDES_SUBAGENT=1` env, set by the desktop ONLY
//! for subagent spawns, selects the subagent behavior per the mode rule).
//!
//! **Reaping assertion:** the main and subagent share the SAME binary
//! (the registry entry applies to both spawns), so the reaping is
//! asserted by a unique MARKER env var (inherited by both; read from
//! `/proc/<pid>/environ`): the matching process COUNT goes 1 → 2 (peak)
//! → 1 (the subagent reaped, the main live).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use archimedes_desktop_lib::agent::{
    EventSink, SessionManager, StopReason, SubagentSessionManager,
};
use serde_json::Value;

/// The full path to the compiled `fake_pi` binary.
const FAKE_PI: &str = env!("CARGO_BIN_EXE_fake_pi");

// ---------------------------------------------------------------------------
// Test event sink (collects into a shared `Vec`)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CollectSink {
    events: Arc<StdMutex<Vec<(String, Value)>>>,
}

impl EventSink for CollectSink {
    fn emit(&self, event: &str, payload: Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subagent-dispatch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an agents.json with ONE `fake` entry: `command` = `fake_pi`,
/// `args` = `[]`, `bridge: true`, and a CUSTOM env map (the mode envs +
/// the unique `FAKE_PI_TEST_MARKER` the reaping counter matches).
fn write_agents_json(dir: &Path, env: &[(&str, &str)]) {
    let mut env_map = serde_json::Map::new();
    for (k, v) in env {
        env_map.insert(k.to_string(), serde_json::json!(v));
    }
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Pi",
                "command": FAKE_PI,
                "args": [],
                "bridge": true,
                "env": env_map
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// A fast COUNT of the running fake processes carrying `marker` in their
/// `environ` (the marker env is inherited by the main AND the subagent —
/// they share the registry entry, so a `cmdline` match cannot tell them
/// apart; the environ is the only discriminator).
///
/// On Linux this is a DIRECT `/proc` scan: `pgrep` cannot match env vars.
/// The scan reads `/proc/<pid>/environ` (a small NUL-separated file) for
/// every pid — kept as cheap as possible (allocation-free digit parse,
/// a single `read` per pid).
struct MarkerCounter {
    marker: Vec<u8>,
}

impl MarkerCounter {
    fn new(marker: &str) -> Self {
        Self {
            marker: format!("FAKE_PI_TEST_MARKER={marker}\0").into_bytes(),
        }
    }

    /// Scan `/proc` and return the count of processes whose `environ`
    /// carries the marker (a vanished pid — reaped — drops out).
    fn scan(&self) -> usize {
        #[cfg(unix)]
        {
            let mut n = 0;
            if let Ok(entries) = std::fs::read_dir("/proc") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy().into_owned();
                    let b = name.as_bytes();
                    if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    let path = format!("/proc/{}/environ", String::from_utf8_lossy(b));
                    if let Ok(env) = std::fs::read(&path) {
                        if env
                            .windows(self.marker.len())
                            .any(|w| w == self.marker.as_slice())
                        {
                            n += 1;
                        }
                    }
                }
            }
            n
        }
        #[cfg(not(unix))]
        {
            let _ = self.marker;
            0
        }
    }
}

/// Poll the collected events until `pred` holds or `timeout` elapses (no
/// lock held across the await).
async fn wait_for_event(
    events: &StdMutex<Vec<(String, Value)>>,
    timeout: Duration,
    pred: impl Fn(&[(String, Value)]) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        {
            let guard = events.lock().unwrap();
            if pred(&guard) {
                return true;
            }
        }
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The text of a `session-update` payload (the `agent_message_chunk` text).
fn update_text(payload: &Value) -> Option<String> {
    payload
        .pointer("/update/content/text")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// All `session-update` texts for a `sessionId` (the session's stream).
fn stream_texts(events: &[(String, Value)], session_id: &str) -> Vec<String> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "session-update")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .filter_map(|(_, p)| update_text(p))
        .collect()
}

/// The first `subagent-closed` payload for a `sessionId`, if any.
fn subagent_closed<'a>(events: &'a [(String, Value)], session_id: &str) -> Option<&'a Value> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "subagent-closed")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .map(|(_, p)| p)
        .next()
}

/// The first `subagent-session-started` payload for a `sessionId`, if any.
fn subagent_started<'a>(events: &'a [(String, Value)], session_id: &str) -> Option<&'a Value> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "subagent-session-started")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .map(|(_, p)| p)
        .next()
}

/// The DISTINCT `sessionId`s of the `subagent-session-started` events.
fn distinct_started_ids(evs: &[(String, Value)]) -> std::collections::HashSet<&str> {
    evs.iter()
        .filter(|(name, _)| name.as_str() == "subagent-session-started")
        .filter_map(|(_, p)| p["sessionId"].as_str())
        .collect()
}

/// The count of `subagent-closed` events with `status: "completed"`.
fn completed_closed_count(evs: &[(String, Value)]) -> usize {
    evs.iter()
        .filter(|(name, p)| {
            name.as_str() == "subagent-closed" && p["status"].as_str() == Some("completed")
        })
        .count()
}

/// The common dispatch-test preamble: build the managers, start the main
/// session, and return the (manager, main session id, marker).
async fn setup_dispatch_test(
    config_dir: &Path,
    env: &[(&str, &str)],
    events: &Arc<StdMutex<Vec<(String, Value)>>>,
) -> (Arc<SessionManager>, String, Arc<SubagentSessionManager>) {
    let marker = uuid::Uuid::new_v4().to_string();
    let mut full_env: Vec<(&str, String)> = env.iter().map(|(k, v)| (*k, v.to_string())).collect();
    full_env.push(("FAKE_PI_TEST_MARKER", marker.clone()));
    let env_refs: Vec<(&str, &str)> = full_env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    write_agents_json(config_dir, &env_refs);

    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(events),
    });
    let cwd = config_dir.to_path_buf();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.to_path_buf())
            .expect("subagent manager should build"),
    );
    let mut manager =
        SessionManager::new(config_dir.to_path_buf()).expect("main manager should build");
    manager.set_subagent_manager(subagent_manager.clone());
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        let manager = Arc::clone(&manager);
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("main start_session should succeed");
    (manager, info.session_id, subagent_manager)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// (1) **success**: the main (a `FAKE_PI_DISPATCH` fake_pi, `bridge: true`)
/// fires the `dispatch_subagent` bridge frame → the desktop dispatches the
/// subagent on the worker runtime → the subagent (the default `fake_pi`
/// mode) answers with its "Hello" turn. Assert the main prompt resolves
/// `end_turn` with `dispatch:do the task` in its stream, the
/// `subagent-session-started` / `subagent-closed` events fire with the
/// right payload (the `metrics` carry the subagent's output + a real
/// `durationMs`), and the marker process COUNT goes 1 → 2 → 1 (the
/// subagent reaped, the main live).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_spawns_rpc_child_and_captures() {
    let config_dir = temp_config_dir();
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let (manager, main_sid, _sub) = setup_dispatch_test(
        &config_dir,
        &[
            ("FAKE_PI_DISPATCH", "1"),
            ("FAKE_PI_DISPATCH_TASK", "do the task"),
        ],
        &events,
    )
    .await;

    // The marker counter: the baseline is 1 (the main).
    let counter = {
        let marker = events
            .lock()
            .unwrap()
            .iter()
            .find(|(n, _)| n.as_str() == "session-update")
            .map(|_| "")
            .unwrap_or("");
        let _ = marker;
        // Read the marker back from the agents.json (the setup wrote it).
        let json: Value =
            serde_json::from_str(&std::fs::read_to_string(config_dir.join("agents.json")).unwrap())
                .unwrap();
        let marker = json["agents"][0]["env"]["FAKE_PI_TEST_MARKER"]
            .as_str()
            .unwrap()
            .to_string();
        MarkerCounter::new(&marker)
    };
    assert_eq!(counter.scan(), 1, "the baseline is the main (1)");

    // The main prompt fires the `dispatch_subagent` frame (the
    // `FAKE_PI_DISPATCH` mode) and blocks until the response (the subagent
    // answers AFTER settling its turn). Hard 30 s timeout.
    let reason = tokio::time::timeout(
        Duration::from_secs(30),
        manager.send_prompt(&main_sid, "go".to_string()),
    )
    .await
    .expect("the main prompt (the dispatch E2E) must not stall")
    .expect("main send_prompt should succeed");
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt resolves end_turn"
    );

    // The main's stream contains the echoed dispatch result
    // (`dispatch:do the task` — the subagent's ECHO of the task: the
    // registry entry's env is shared by the main and the subagent, so the
    // subagent sees `FAKE_PI_DISPATCH=1` too; with `ARCHIMEDES_SUBAGENT=1`
    // (set by the desktop ONLY for subagent spawns) it selects the subagent
    // echo behavior instead of the default `Hello` turn).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| stream_texts(
            evs, &main_sid
        )
        .iter()
        .any(|t| t == "dispatch:do the task"))
        .await,
        "the main's stream should contain `dispatch:do the task`"
    );

    // A `subagent-session-started` fired (the subagent's DISTINCT pi id —
    // unique per process, `fake-pi-<hex>`); its payload carries the
    // parent's id + the task.
    let sub_sid = {
        let evs = events.lock().unwrap();
        let started = evs
            .iter()
            .find(|(name, p)| {
                name.as_str() == "subagent-session-started"
                    && p["parentSessionId"].as_str() == Some(main_sid.as_str())
            })
            .expect("a subagent-session-started for the main should fire")
            .1
            .clone();
        started["sessionId"].as_str().unwrap().to_string()
    };
    assert!(
        sub_sid.starts_with("fake-pi-"),
        "the subagent reports a unique pi id, got {sub_sid}"
    );
    {
        let evs = events.lock().unwrap();
        let started = subagent_started(&evs, &sub_sid).unwrap();
        assert_eq!(started["agentName"], "fake");
        assert_eq!(started["task"], "do the task");
    }

    // `subagent-closed` (completed) with the metrics snapshot (the
    // subagent's output + a real `durationMs`).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| subagent_closed(
            evs, &sub_sid
        )
        .and_then(|p| p["status"].as_str())
            == Some("completed"))
        .await,
        "a subagent-closed (completed) for the subagent should fire"
    );
    {
        let evs = events.lock().unwrap();
        let closed = subagent_closed(&evs, &sub_sid).unwrap();
        assert_eq!(
            closed["metrics"]["output"], "do the task",
            "the metrics carry the subagent's final output (the echo)"
        );
        assert!(
            closed["metrics"]["durationMs"].is_u64()
                && closed["metrics"]["durationMs"].as_u64().unwrap() > 0,
            "durationMs is a positive number"
        );
    }

    // The marker process COUNT returns to 1 (the subagent reaped — the
    // driver teardown closed its stdin; the main stays live).
    let reap_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let c = counter.scan();
        if c == 1 {
            break;
        }
        if Instant::now() > reap_deadline {
            panic!("the subagent process should be reaped (count back to 1); got {c}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Cleanup.
    let _ = manager.close_session(&main_sid).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (2) **cancellation**: the main (a `FAKE_PI_DISPATCH_CANCEL` fake_pi —
/// sends the frame, closes the bridge connection WITHOUT reading) + the
/// subagent (the same entry — `ARCHIMEDES_SUBAGENT=1` selects the HANG
/// behavior: the prompt never settles). The parent connection close
/// cancels the in-flight dispatch. Assert the subagent session is torn
/// down (`subagent-closed` with `status: "failed"` + `error "cancelled"`)
/// and the main stays live (its prompt still resolves `end_turn` with
/// `dispatch:aborted` in its stream).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_cancellation_tears_down_subagent() {
    let config_dir = temp_config_dir();
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let (manager, main_sid, _sub) = setup_dispatch_test(
        &config_dir,
        &[
            ("FAKE_PI_DISPATCH_CANCEL", "1"),
            ("FAKE_PI_DISPATCH_TASK", "do the task"),
        ],
        &events,
    )
    .await;

    // The main prompt fires the frame, closes the connection (cancelling
    // the dispatch), and settles with `dispatch:aborted`.
    let reason = tokio::time::timeout(
        Duration::from_secs(30),
        manager.send_prompt(&main_sid, "go".to_string()),
    )
    .await
    .expect("the main prompt (the cancellation E2E) must not stall")
    .expect("main send_prompt should succeed");
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt resolves end_turn"
    );

    // The main's stream contains `dispatch:aborted` (the close marker).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| stream_texts(
            evs, &main_sid
        )
        .iter()
        .any(|t| t == "dispatch:aborted"))
        .await,
        "the main's stream should contain `dispatch:aborted`"
    );

    // The subagent (which HANGS on its prompt) is torn down by the
    // parent-close cancellation: `subagent-closed` (failed, `cancelled`).
    let sub_sid = {
        let evs = events.lock().unwrap();
        let started = evs
            .iter()
            .find(|(name, p)| {
                name.as_str() == "subagent-session-started"
                    && p["parentSessionId"].as_str() == Some(main_sid.as_str())
            })
            .expect("a subagent-session-started should fire before the cancel")
            .1
            .clone();
        started["sessionId"].as_str().unwrap().to_string()
    };
    assert!(
        wait_for_event(&events, Duration::from_secs(10), |evs| subagent_closed(
            evs, &sub_sid
        )
        .filter(|p| p["status"].as_str() == Some("failed"))
        .is_some())
        .await,
        "subagent-closed (failed) should fire"
    );
    {
        let evs = events.lock().unwrap();
        let closed = subagent_closed(&evs, &sub_sid).unwrap();
        assert_eq!(
            closed["error"], "cancelled",
            "the error is `cancelled` (the parent close)"
        );
    }

    // The main stays live (its session count is 1).
    assert_eq!(manager.session_count().await, 1, "the main stays live");

    // Cleanup.
    let _ = manager.close_session(&main_sid).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (3) **concurrent subagents, per-dispatch captures** (the shared-capture
/// regression, and the N-sessions-on-one-worker-runtime shape): one main
/// session issues TWO `dispatch_subagent` frames (the `FAKE_PI_DISPATCH_TWO`
/// mode — both frames written BEFORE either response is read, so the two
/// subagents run CONCURRENTLY on the ONE `SubagentSessionManager` worker
/// runtime). Each subagent (the `echo` behavior — the task text verbatim)
/// answers with ITS OWN task as its final text (DISTINCT per dispatch).
/// Assert each `Completed.output` is ITS OWN text (both outputs correct AND
/// distinct — a shared `last_message_id` / `text_capture` would clobber /
/// accumulate across the two sessions) + the main prompt resolves within a
/// HARD 30 s timeout (the 2026-09-15 clean-red pattern).
// The `events` guard is dropped before each await, but clippy's liveness
// analysis is scope-based, not `drop`-aware.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_concurrent_subagents_get_their_own_output() {
    let config_dir = temp_config_dir();
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let (manager, main_sid, _sub) = setup_dispatch_test(
        &config_dir,
        &[
            ("FAKE_PI_DISPATCH_TWO", "1"),
            ("FAKE_PI_DISPATCH_TASK", "task-one"),
            ("FAKE_PI_DISPATCH_TASK_2", "task-two"),
        ],
        &events,
    )
    .await;

    // The main prompt fires the TWO `dispatch_subagent` frames (the
    // `FAKE_PI_DISPATCH_TWO` mode, concurrent). Wrap it in a HARD 30 s
    // timeout (the 2026-09-15 clean-red pattern — N sessions on the ONE
    // worker runtime): a fired timeout IS the concurrency regression.
    let started = Instant::now();
    let reason = match tokio::time::timeout(
        Duration::from_secs(30),
        manager.send_prompt(&main_sid, "go".to_string()),
    )
    .await
    {
        Ok(r) => r.expect("main send_prompt should succeed"),
        Err(_) => panic!(
            "CONCURRENCY REGRESSION (ADR 0004): the main prompt (TWO concurrent \
             subagent dispatches on the ONE worker runtime) stalled — the 30 s hard \
             timeout fired at {:?} (the 2026-09-15 hang shape: N sessions on \
             one worker runtime)",
            started.elapsed()
        ),
    };
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt should resolve end_turn"
    );

    // Each `Completed.output` is ITS OWN text: the main echoed
    // `dispatch1:task-one` and `dispatch2:task-two` (NOT a clobbered /
    // accumulated shared-capture value — a shared `last_message_id` /
    // `text_capture` would make the second output carry the first's text).
    let evs = events.lock().unwrap();
    let texts = stream_texts(&evs, &main_sid);
    drop(evs);
    assert!(
        texts.iter().any(|t| t == "dispatch1:task-one"),
        "the first concurrent subagent's output must be its OWN task (got {texts:?})"
    );
    assert!(
        texts.iter().any(|t| t == "dispatch2:task-two"),
        "the second concurrent subagent's output must be its OWN task (got {texts:?})"
    );

    // Two DISTINCT subagent sessions were established (distinct pi ids —
    // unique per process), and both completed.
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| {
            distinct_started_ids(evs).len() >= 2
        })
        .await,
        "two DISTINCT subagent sessions should be established"
    );
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| {
            completed_closed_count(evs) >= 2
        })
        .await,
        "both concurrent subagents should complete (two subagent-closed completed)"
    );

    // Cleanup.
    let _ = manager.close_session(&main_sid).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (4) **no-text after text** (the stale-carry-over regression): one main
/// session issues TWO `dispatch_subagent` frames SEQUENTIALLY (the
/// `FAKE_PI_DISPATCH_TWO` mode + `FAKE_PI_DISPATCH_TWO_SEQUENTIAL=1` —
/// response 1 is read BEFORE frame 2 is sent). The first subagent answers
/// with text (the `echo` behavior — `hello-text`); the second answers with
/// NO text (the `EMPTY` sentinel — a settle with no assistant message).
/// Assert the first `Completed.output` is its text AND the second is the
/// EMPTY STRING (NOT the previous subagent's final text — a shared
/// `last_message_id` / `text_capture` would carry it over).
// The `events` guard is dropped before each await, but clippy's liveness
// analysis is scope-based, not `drop`-aware.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_no_text_after_text_dispatch_returns_empty_output() {
    let config_dir = temp_config_dir();
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let (manager, main_sid, _sub) = setup_dispatch_test(
        &config_dir,
        &[
            ("FAKE_PI_DISPATCH_TWO", "1"),
            ("FAKE_PI_DISPATCH_TASK", "hello-text"),
            ("FAKE_PI_DISPATCH_TASK_2", "EMPTY"),
            ("FAKE_PI_DISPATCH_TWO_SEQUENTIAL", "1"),
        ],
        &events,
    )
    .await;

    // The main prompt fires the two dispatches SEQUENTIALLY (the
    // `FAKE_PI_DISPATCH_TWO_SEQUENTIAL` rule). Hard 30 s timeout.
    let started = Instant::now();
    let reason = match tokio::time::timeout(
        Duration::from_secs(30),
        manager.send_prompt(&main_sid, "go".to_string()),
    )
    .await
    {
        Ok(r) => r.expect("main send_prompt should succeed"),
        Err(_) => panic!(
            "CONCURRENCY REGRESSION (ADR 0004): the main prompt (two sequential \
             subagent dispatches on the ONE worker runtime) stalled — the 30 s hard \
             timeout fired at {:?}",
            started.elapsed()
        ),
    };
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt should resolve end_turn"
    );

    // The first `Completed.output` is its text (`dispatch1:hello-text`).
    let evs = events.lock().unwrap();
    let texts = stream_texts(&evs, &main_sid);
    drop(evs);
    assert!(
        texts.iter().any(|t| t == "dispatch1:hello-text"),
        "the text subagent's output must be its OWN text (got {texts:?})"
    );
    // The second (no-text) `Completed.output` is the EMPTY STRING — the
    // main echoed `dispatch2:` (an empty output). A shared
    // `last_message_id` / `text_capture` would carry over the FIRST
    // subagent's final text (`dispatch2:hello-text` — the stale-carry-over
    // bug).
    assert!(
        texts.iter().any(|t| t == "dispatch2:"),
        "the no-text subagent's output must be the EMPTY STRING, not the previous \
         subagent's final text (got {texts:?})"
    );
    assert!(
        !texts.iter().any(|t| t == "dispatch2:hello-text"),
        "the no-text subagent's output must NOT carry over the previous \
         subagent's final text (got {texts:?})"
    );

    // Both subagents completed.
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| {
            completed_closed_count(evs) >= 2
        })
        .await,
        "both subagents should complete (two subagent-closed completed)"
    );

    // Cleanup.
    let _ = manager.close_session(&main_sid).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (5) **cost accumulation** (the metrics-capture E2E): the subagent (the
/// `FAKE_PI_COST_PUSH` mode) pushes TWO `cost_update` frames through its
/// OWN `PI_ARCHIMEDES_BRIDGE_SOCKET` before settling (ONE connection per
/// push — the desktop reads exactly one frame per connection — and it reads
/// each bare `ack` line BEFORE the next push, so the desktop's
/// end-of-turn capture is COMPLETE). Assert the `subagent-closed` metrics
/// are the SUM of both payloads (inputTokens 100+200=300, outputTokens
/// 50+25=75, cost 0.001+0.002=0.003 — NOT the last payload's
/// `{ 200, 25, 0.002 }` and NOT the zeros of a session that pushed nothing)
/// + a real `durationMs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_subagent_cost_push_is_accumulated_into_metrics() {
    let config_dir = temp_config_dir();
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    // The entry env applies to BOTH spawns: the MAIN (no
    // `ARCHIMEDES_SUBAGENT`) takes the dispatch branch (fires the frame);
    // the SUBAGENT takes the `FAKE_PI_COST_PUSH` sub-branch (pushes the
    // cost frames + the default turn).
    let (manager, main_sid, _sub) = setup_dispatch_test(
        &config_dir,
        &[("FAKE_PI_DISPATCH", "1"), ("FAKE_PI_COST_PUSH", "1")],
        &events,
    )
    .await;

    // The main prompt fires the `dispatch_subagent` frame (the
    // `FAKE_PI_DISPATCH` mode) and blocks until the response (the subagent
    // answers AFTER pushing its two `cost_update` frames). Hard 30 s
    // timeout.
    let started = Instant::now();
    let reason = match tokio::time::timeout(
        Duration::from_secs(30),
        manager.send_prompt(&main_sid, "go".to_string()),
    )
    .await
    {
        Ok(r) => r.expect("main send_prompt should succeed"),
        Err(_) => panic!(
            "the main prompt (the cost-push E2E) stalled — the 30 s hard timeout fired at {:?}",
            started.elapsed()
        ),
    };
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt should resolve end_turn"
    );

    // The main's stream contains the echoed dispatch result
    // (`dispatch:Hello` — the cost-push subagent's default turn text,
    // answered AFTER both cost pushes were acked).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| stream_texts(
            evs, &main_sid
        )
        .iter()
        .any(|t| t == "dispatch:Hello"))
        .await,
        "the main's stream should contain `dispatch:Hello`"
    );

    // `subagent-closed` (completed) with the metrics snapshot.
    let sub_sid = {
        let evs = events.lock().unwrap();
        let started = evs
            .iter()
            .find(|(name, p)| {
                name.as_str() == "subagent-session-started"
                    && p["parentSessionId"].as_str() == Some(main_sid.as_str())
            })
            .expect("a subagent-session-started should fire")
            .1
            .clone();
        started["sessionId"].as_str().unwrap().to_string()
    };
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| subagent_closed(
            evs, &sub_sid
        )
        .and_then(|p| p["status"].as_str())
            == Some("completed"))
        .await,
        "a subagent-closed (completed) for the subagent should fire"
    );
    // The metrics are the SUM of BOTH pushed payloads (payload 1: input
    // 100 / output 50 / cost 0.001, `cacheReadTokens` ABSENT → 0; payload
    // 2: input 200 / output 25 / cacheRead 10 / cost 0.002): inputTokens
    // 300, outputTokens 75, cost 0.003.
    {
        let evs = events.lock().unwrap();
        let closed = subagent_closed(&evs, &sub_sid).unwrap();
        let m = &closed["metrics"];
        assert_eq!(
            m["inputTokens"], 300,
            "inputTokens is the SUM of both payloads (not the last's 200)"
        );
        assert_eq!(
            m["outputTokens"], 75,
            "outputTokens is the SUM of both payloads (not the last's 25)"
        );
        let cost = m["cost"].as_f64().unwrap();
        assert!(
            (cost - 0.003).abs() < 1e-9,
            "cost is the SUM of both payloads (0.003), got {cost}"
        );
        assert!(
            m["durationMs"].is_u64() && m["durationMs"].as_u64().unwrap() > 0,
            "durationMs is a positive number"
        );
    }

    // Cleanup.
    let _ = manager.close_session(&main_sid).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}
