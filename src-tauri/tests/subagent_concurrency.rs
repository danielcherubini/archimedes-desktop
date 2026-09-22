//! Regression test (ADR 0004, the build gate for the subagent-sessions
//! feature): two concurrent ACP sessions — one on the app (main) runtime,
//! one on the dedicated `WorkerRuntime` — must BOTH answer `send_prompt`
//! promptly.
//!
//! On 2026-09-15, two concurrent sessions on ONE runtime hung
//! `send_prompt` 60+ seconds (root cause never confirmed — suspected
//! SDK/async-io level: two long-lived per-connection transport tasks
//! sharing one global async-io reactor). This test reproduces that shape
//! with the SDK transport stack (the `fake_agent` binary speaks ACP over
//! stdio — the same transport the real `pi-acp` uses): the main-runtime
//! session runs on the test's own runtime (which plays the "app runtime"
//! role — no Tauri app runtime required), and the worker-runtime session
//! is established on the `WorkerRuntime` via `spawn_task` (channel-based
//! handoff only; the manager's driver/transport tasks keep running on the
//! worker runtime while the test drives the manager from the main
//! runtime).
//!
//! The concurrent prompts are wrapped in a HARD 10 s timeout: a fired
//! timeout IS the repro, and it must be a clean red test (a panic
//! carrying the elapsed time), NOT a stalled `cargo test` with no output
//! (the 2026-09-15 hang may be an indefinite stall, and an unbounded
//! `join!` would hang the test binary, defeating the STOP gate).

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use agent_client_protocol::schema::v1::StopReason;
use archimedes_desktop_lib::acp::{EventSink, SessionManager, WorkerRuntime};

/// The session id the `main` fake agent reports (via `FAKE_SESSION_ID`).
const FAKE_SESSION_ID_MAIN: &str = "main-sess";
/// The session id the `worker` fake agent reports (via `FAKE_SESSION_ID`).
const FAKE_SESSION_ID_WORKER: &str = "worker-sess";

/// The full path to the compiled `fake_agent` binary.
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

// ---------------------------------------------------------------------------
// Test event sink (std mpsc-backed)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TestSink {
    tx: std::sync::mpsc::Sender<(String, Value)>,
}

impl EventSink for TestSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("acp-concurrency-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copy the fake agent binary to a unique path (tests run in parallel and
/// all launch the same binary; a `pgrep` on the shared path cannot tell
/// whose agent is whose).
fn unique_fake_agent(dir: &Path) -> PathBuf {
    let path = dir.join(format!("fake_agent-{}", uuid::Uuid::new_v4()));
    std::fs::copy(FAKE_AGENT, &path).unwrap();
    path
}

/// Count the running fake-agent processes matching `pattern` (the `pgrep -f`
/// pattern extended to a COUNT — used to assert a process is reaped).
fn count_fake_agent_processes(pattern: &Path) -> usize {
    let out = match std::process::Command::new("pgrep")
        .args(["-f", pattern.to_string_lossy().as_ref()])
        .output()
    {
        Ok(o) => o,
        Err(_) => return 0,
    };
    if !out.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .count()
}

/// Write an agents.json with a SINGLE `worker` entry (the app-exit test —
/// one worker-runtime session).
fn write_single_agents_json(dir: &Path, worker_bin: &Path) {
    let json = serde_json::json!({
        "agents": [
            {
                "id": "worker",
                "name": "Worker Fake Agent",
                "command": worker_bin.to_string_lossy(),
                "args": [],
                "env": { "FAKE_SESSION_ID": FAKE_SESSION_ID_WORKER }
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Write an agents.json with TWO entries — `main` and `worker` — each
/// reporting a distinct session id, so the shared event sink's
/// `session-closed` events can be discriminated by `sessionId`.
fn write_agents_json(dir: &Path, main_bin: &Path, worker_bin: &Path) {
    let json = serde_json::json!({
        "agents": [
            {
                "id": "main",
                "name": "Main Fake Agent",
                "command": main_bin.to_string_lossy(),
                "args": [],
                "env": { "FAKE_SESSION_ID": FAKE_SESSION_ID_MAIN }
            },
            {
                "id": "worker",
                "name": "Worker Fake Agent",
                "command": worker_bin.to_string_lossy(),
                "args": [],
                "env": { "FAKE_SESSION_ID": FAKE_SESSION_ID_WORKER }
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Poll `session-closed` events for BOTH given session ids out of a possibly
/// busy event queue, in a SINGLE loop (budget 5 s).
///
/// The two closes are asserted in one loop (NOT two sequential discarding
/// scans) because the two sessions live on different runtimes and their
/// close events may interleave in either order — a sequential scan would
/// consume one session's close while hunting for the other's and then miss
/// it. The shared sink carries both, so we track each id independently.
fn wait_for_both_closed(rx: &Receiver<(String, Value)>, id_a: &str, id_b: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen_a = false;
    let mut seen_b = false;
    while (!seen_a || !seen_b) && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok((event, payload)) if event == "session-closed" => {
                let sid = payload["sessionId"].as_str().unwrap_or("");
                if sid == id_a {
                    seen_a = true;
                }
                if sid == id_b {
                    seen_b = true;
                }
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(
        seen_a,
        "a session-closed event for `{id_a}` should arrive within 5s"
    );
    assert!(
        seen_b,
        "a session-closed event for `{id_b}` should arrive within 5s"
    );
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sessions_on_two_runtimes_answer_concurrent_prompts() {
    let config_dir = temp_config_dir();
    let bin_main = unique_fake_agent(&config_dir);
    let bin_worker = unique_fake_agent(&config_dir);
    write_agents_json(&config_dir, &bin_main, &bin_worker);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
    let cwd = config_dir.clone();

    // 1. The MAIN-runtime session (the test's own runtime plays the "app
    //    runtime" role — the test does not require the Tauri app runtime).
    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let info = manager
        .start_session("main", cwd.clone(), &sink)
        .await
        .expect("main start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID_MAIN);

    // 2. The WORKER-runtime session: built on the dedicated runtime via
    //    `spawn_task` (channel-based handoff — never `block_on` across
    //    runtimes). The worker `SessionManager`'s driver/transport tasks
    //    keep running on the worker runtime (the manager is just a handle
    //    — `Arc<Mutex<…>>` internals, `Send + Sync`), so the ACP transport
    //    tasks live on the worker runtime while the test drives the manager
    //    from the main runtime.
    let worker = WorkerRuntime::new().expect("worker runtime should build");
    let worker_sink = Arc::clone(&sink);
    let worker_config_dir = config_dir.clone();
    let (worker_manager, worker_session) = worker
        .spawn_task(async move {
            let manager = Arc::new(
                SessionManager::new(worker_config_dir).expect("worker SessionManager should build"),
            );
            let info = manager
                .start_session("worker", cwd, &worker_sink)
                .await
                .expect("worker start_session should succeed");
            (manager, info.session_id.to_string())
        })
        .await
        .expect("worker spawn_task should complete");
    assert_eq!(worker_session, FAKE_SESSION_ID_WORKER);

    // 3. Send `send_prompt` to BOTH sessions concurrently, wrapped in a
    //    HARD 10 s timeout. An `Err(_)` (the timeout fired) IS the
    //    2026-09-15 repro: a clean red test (a panic carrying the elapsed
    //    time), NOT a stalled `cargo test` with no output.
    let started = Instant::now();
    // NOTE: `tokio::join!` in tokio 1.53 expands to an ALREADY-awaited
    // expression (it ends with `poll_fn(...).await`), so it is a value, not a
    // future. Wrap it in an `async` block to get a future that `timeout`
    // can bound. A fired timeout (the `Err` arm) IS the 2026-09-15 repro.
    let joined = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            manager.send_prompt(FAKE_SESSION_ID_MAIN, "hi".to_string()),
            worker_manager.send_prompt(&worker_session, "hi".to_string()),
        )
    })
    .await;
    let elapsed = started.elapsed();
    let (main_reason, worker_reason) = match joined {
        Ok((main, worker)) => (main, worker),
        Err(_) => panic!(
            "CONCURRENCY REGRESSION (ADR 0004): the two concurrent send_prompt \
             calls (one on the app runtime, one on the worker runtime) stalled \
             — the 10 s hard timeout fired at {elapsed:?} (the 2026-09-15 hang \
             was 60+ s). The worker runtime did NOT isolate the reactor; \
             switch to the ADR 0004 fallback (per-session OS thread + \
             single-threaded runtime) before building on top of it."
        ),
    };
    let main_reason = main_reason.expect("main send_prompt should succeed");
    let worker_reason = worker_reason.expect("worker send_prompt should succeed");
    assert_eq!(main_reason, StopReason::EndTurn);
    assert_eq!(worker_reason, StopReason::EndTurn);
    assert!(
        elapsed < Duration::from_secs(10),
        "both prompts should answer well below the hang threshold; took {elapsed:?} \
        (the fake agent answers immediately, so any stall is the transport)"
    );

    // 4. Close both; the `session-closed` events fire for both (teardown
    //    works on both runtimes). Both are awaited in a SINGLE loop because
    //    the two sessions live on different runtimes and their close events
    //    may interleave in either order (a sequential scan would consume one
    //    while hunting for the other and miss it).
    manager
        .close_session(FAKE_SESSION_ID_MAIN)
        .await
        .expect("main close_session should succeed");
    worker_manager
        .close_session(&worker_session)
        .await
        .expect("worker close_session should succeed");
    wait_for_both_closed(&rx, FAKE_SESSION_ID_MAIN, FAKE_SESSION_ID_WORKER);

    // 5. Teardown: send the shutdown signal and join the dedicated thread.
    //    The `Runtime` is dropped ON that thread (never from this async
    //    context — tokio panics if a runtime is dropped where blocking is
    //    not allowed), so a panic here would mean the drop-safety design
    //    is broken.
    worker.shutdown_and_join();

    // Clean up the temp dir (best effort).
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (4) **app-exit teardown**: start a worker-runtime session, `worker.
/// shutdown_and_join()` (the Task 1 method — signal + join; the `Runtime` is
/// dropped on the dedicated thread, so NO tokio "cannot drop a runtime in an
/// async context" panic), assert the fake-agent child is reaped (the `cx`
/// drop closes the stdio; the ACP SDK kills the process group on Unix) within
/// a few seconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_exit_shutdown_and_join_reaps_worker_agent() {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    // A single `worker` entry (the app-exit path: one worker-runtime session).
    write_single_agents_json(&config_dir, &bin);

    let (tx, _rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
    let cwd = config_dir.clone();

    // Start the worker-runtime session (the `Runtime` is owned by the
    // dedicated thread; the manager's driver/transport tasks run on it).
    let worker = WorkerRuntime::new().expect("worker runtime should build");
    let worker_manager =
        worker
            .spawn_task({
                let cd = config_dir.clone();
                async move {
                    Arc::new(SessionManager::new(cd).expect("worker SessionManager should build"))
                }
            })
            .await
            .expect("worker spawn_task should complete");
    let info = {
        let m = Arc::clone(&worker_manager);
        let s = Arc::clone(&sink);
        worker
            .spawn_task(async move {
                m.start_session("worker", cwd, &s)
                    .await
                    .expect("worker start_session should succeed")
            })
            .await
            .expect("worker spawn_task should complete")
    };
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID_WORKER);

    // The fake-agent child is alive (count 1).
    assert_eq!(
        count_fake_agent_processes(&bin),
        1,
        "the worker agent should be alive before shutdown"
    );

    // App-exit: `shutdown_and_join` (the `Runtime` is dropped on the dedicated
    // thread — NO tokio "cannot drop a runtime in an async context" panic).
    // The `cx` drop closes the stdio; the ACP SDK kills the process group on
    // Unix, so the child is reaped.
    worker.shutdown_and_join();

    // The child is reaped within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if count_fake_agent_processes(&bin) == 0 {
            break;
        }
        if Instant::now() > deadline {
            panic!("the worker agent process should be reaped after shutdown_and_join");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Clean up the temp dir (best effort).
    let _ = std::fs::remove_dir_all(&config_dir);
}
