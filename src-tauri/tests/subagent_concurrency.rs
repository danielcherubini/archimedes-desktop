//! Regression test (ADR 0004, the build gate for the subagent-sessions
//! feature): two concurrent sessions — one on the app (main) runtime, one
//! on the dedicated `WorkerRuntime` — must BOTH answer `send_prompt`
//! promptly.
//!
//! On 2026-09-15, two concurrent sessions on ONE runtime hung
//! `send_prompt` 60+ seconds (root cause never confirmed — suspected
//! SDK/async-io level: two long-lived per-connection transport tasks
//! sharing one global async-io reactor). This test reproduces that shape
//! with the `fake_pi` binary (it speaks the pi RPC over stdio — the same
//! transport the real `pi` uses): the main-runtime session runs on the
//! test's own runtime (which plays the "app runtime" role — no Tauri app
//! runtime required), and the worker-runtime session is established on
//! the `WorkerRuntime` via `spawn_task` (channel-based handoff only; the
//! manager's driver/transport tasks keep running on the worker runtime
//! while the test drives the manager from the main runtime).
//!
//! The concurrent prompts are wrapped in a HARD 10 s timeout: a fired
//! timeout IS the repro, and it must be a clean red test (a panic
//! carrying the elapsed time), NOT a stalled `cargo test` with no output
//! (the 2026-09-15 hang may be an indefinite stall, and an unbounded
//! `join!` would hang the test binary, defeating the STOP gate).
//!
//! The session ids are DYNAMIC (`fake_pi` reports `fake-pi-<pid><nanos>`
//! per process, or the `--session` file's stem on resume) — the test
//! uses the ids `start_session` returns (each process's id is unique).

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use archimedes_desktop_lib::agent::{EventSink, SessionManager, StopReason, WorkerRuntime};

/// The full path to the compiled `fake_pi` binary.
const FAKE_PI: &str = env!("CARGO_BIN_EXE_fake_pi");

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
    let dir = std::env::temp_dir().join(format!("pi-concurrency-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Count the running `fake_pi` processes that carry `marker` in their
/// `environ` (a unique per-test marker — the shared binary path cannot
/// distinguish this test's children from other tests' when `cargo test`
/// runs the targets in parallel).
fn count_marker_processes(marker: &str) -> usize {
    let out = match std::process::Command::new("pgrep")
        .args(["-x", "fake_pi"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return 0,
    };
    if !out.status.success() {
        return 0;
    }
    let pids = String::from_utf8_lossy(&out.stdout);
    pids.split_ascii_whitespace()
        .filter(|pid| {
            let Ok(environ) = std::fs::read_to_string(format!("/proc/{}/environ", pid)) else {
                return false;
            };
            environ
                .split('\0')
                .any(|kv| kv == format!("FAKE_PI_TEST_MARKER={marker}").as_str())
        })
        .count()
}

/// Write an agents.json with the given entries (each a `fake_pi` spawn —
/// the same binary the main and worker sessions use; the `marker` env
/// distinguishes this test's children).
fn write_agents_json(dir: &std::path::Path, entries: &[(&str, &str, &str)]) {
    let agents: Vec<Value> = entries
        .iter()
        .map(|(id, name, marker)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "command": FAKE_PI,
                "args": [],
                "env": { "FAKE_PI_TEST_MARKER": marker }
            })
        })
        .collect();
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&serde_json::json!({ "agents": agents })).unwrap(),
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
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sessions_on_two_runtimes_answer_concurrent_prompts() {
    let config_dir = temp_config_dir();
    let marker_main = format!("main-{}", uuid::Uuid::new_v4());
    let marker_worker = format!("worker-{}", uuid::Uuid::new_v4());
    write_agents_json(
        &config_dir,
        &[
            ("main", "Main Fake Pi", &marker_main),
            ("worker", "Worker Fake Pi", &marker_worker),
        ],
    );

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });
    let cwd = config_dir.clone();

    // 1. The MAIN-runtime session (the test's own runtime plays the "app
    //    runtime" role — the test does not require the Tauri app runtime).
    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("main", cwd.clone(), &sink).await
    })
    .await
    .expect("main start_session should succeed");
    let main_sid = info.session_id.to_string();
    assert!(
        main_sid.starts_with("fake-pi-"),
        "the main session reports a dynamic pi id, got {main_sid}"
    );

    // 2. The WORKER-runtime session: built on the dedicated runtime via
    //    `spawn_task` (channel-based handoff — never `block_on` across
    //    runtimes). The worker `SessionManager`'s driver/transport tasks
    //    keep running on the worker runtime (the manager is just a handle
    //    — `Arc<Mutex<…>>` internals, `Send + Sync`), so the RPC transport
    //    tasks live on the worker runtime while the test drives the manager
    //    from the main runtime.
    let worker = WorkerRuntime::new().expect("worker runtime should build");
    let worker_sink = Arc::clone(&sink);
    let worker_config_dir = config_dir.clone();
    let (worker_manager, worker_sid) = worker
        .spawn_task(async move {
            let manager = Arc::new(
                SessionManager::new(worker_config_dir).expect("worker SessionManager should build"),
            );
            let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
                manager
                    .start_session("worker", cwd.clone(), &worker_sink)
                    .await
            })
            .await
            .expect("worker start_session should succeed");
            (manager, info.session_id.to_string())
        })
        .await
        .expect("worker spawn_task should complete");
    assert!(
        worker_sid.starts_with("fake-pi-"),
        "the worker session reports a dynamic pi id, got {worker_sid}"
    );
    assert_ne!(
        main_sid, worker_sid,
        "the two processes report distinct pi ids"
    );

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
            manager.send_prompt(&main_sid, "hi".to_string()),
            worker_manager.send_prompt(&worker_sid, "hi".to_string()),
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
        .close_session(&main_sid)
        .await
        .expect("main close_session should succeed");
    worker_manager
        .close_session(&worker_sid)
        .await
        .expect("worker close_session should succeed");
    wait_for_both_closed(&rx, &main_sid, &worker_sid);

    // 5. Teardown: send the shutdown signal and join the dedicated thread.
    //    The `Runtime` is dropped ON that thread (never from this async
    //    context — tokio panics if a runtime is dropped where blocking is
    //    not allowed), so a panic here would mean the drop-safety design
    //    is broken.
    worker.shutdown_and_join();
    // Drop the manager (the last `Arc<Inner>` holder — the runtime
    // shutdown cancelled the driver tasks but the manager's session
    // entries kept the handles alive; dropping it closes the stdin and
    // reaps the fakes).
    drop(worker_manager);

    // Clean up the temp dir (best effort).
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// **app-exit teardown**: start a worker-runtime session, `worker.
/// shutdown_and_join()` — signal + join; the `Runtime` is dropped on the
/// dedicated thread, so NO tokio "cannot drop a runtime in an async
/// context" panic — assert the `fake_pi` child is reaped (the runtime
/// shutdown drops the handles' `Arc<Inner>`, which drops the `ChildStdin`
/// → the agent's stdin sees EOF → the agent exits) within a few seconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_exit_shutdown_and_join_reaps_worker_agent() {
    let config_dir = temp_config_dir();
    let marker = format!("appexit-{}", uuid::Uuid::new_v4());
    // A single `worker` entry (the app-exit path: one worker-runtime
    // session).
    write_agents_json(&config_dir, &[("worker", "Worker Fake Pi", &marker)]);

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
                archimedes_desktop_lib::test_support::run_with_retry(|| async {
                    m.start_session("worker", cwd.clone(), &s).await
                })
                .await
                .expect("worker start_session should succeed")
            })
            .await
            .expect("worker spawn_task should complete")
    };
    let sid = info.session_id.to_string();
    assert!(
        sid.starts_with("fake-pi-"),
        "the worker session reports a dynamic pi id, got {sid}"
    );

    // The fake child is alive (count 1 — matched by the unique marker).
    assert_eq!(
        count_marker_processes(&marker),
        1,
        "the worker agent should be alive before shutdown"
    );

    // App-exit: `shutdown_and_join` (the `Runtime` is dropped on the
    // dedicated thread — NO tokio "cannot drop a runtime in an async
    // context" panic). The runtime shutdown CANCELS the driver tasks
    // (their `Arc<Inner>` clones drop); dropping the manager drops the
    // session entries (the remaining `Arc<Inner>` holders) — together the
    // `ChildStdin` closes, the agent's stdin sees EOF, and the agent
    // exits, so the child is reaped.
    worker.shutdown_and_join();
    drop(worker_manager);

    // The child is reaped within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if count_marker_processes(&marker) == 0 {
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
