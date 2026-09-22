//! Integration test: drives the fake ACP agent through the full session
//! lifecycle — initialize → session/new → prompt → streamed updates → close —
//! and verifies the connection is torn down and the child process reaped.
//!
//! A second test kills the fake agent mid-session and asserts the session is
//! removed and a `session-closed` event with `reason: "agent-exited"` is
//! emitted.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use agent_client_protocol::schema::v1::StopReason;
use archimedes_desktop_lib::acp::{
    AcpError, EventSink, PermissionOutcome, SessionInfo, SessionManager,
};
use archimedes_desktop_lib::storage::Db;

/// The fixed session id reported by the fake agent (see `bin/fake_agent.rs`).
const FAKE_SESSION_ID: &str = "fake-session-1";

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
    let dir = std::env::temp_dir().join(format!("acp-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Collect up to `count` events from the sink, waiting up to `timeout`.
fn wait_for_events(
    rx: &Receiver<(String, Value)>,
    count: usize,
    timeout: Duration,
) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    let deadline = Instant::now() + timeout;
    while events.len() < count {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(e) => events.push(e),
            Err(_) => break,
        }
    }
    events
}

fn session_update_text(payload: &Value) -> Option<String> {
    payload
        .pointer("/update/content/text")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

/// Find the pid of a running fake agent matching `pattern`, if any.
///
/// A fast DIRECT `/proc` scan (no `pgrep` fork+exec, which is ~45 ms and whose
/// fork can transiently fail under load) — see `common::proc_scan`.
fn find_fake_agent_pid(pattern: &Path) -> Option<u32> {
    common::proc_scan::ProcScan::new(pattern).find_pid()
}

/// SIGKILL `pid` (a direct `kill(2)` syscall — NO fork+exec of the `kill`
/// binary, whose fork can transiently fail (EAGAIN) under load).
fn kill_pid(pid: u32) {
    common::proc_scan::kill_pid(pid);
}

/// Write an agents.json pointing at a specific (per-test) agent binary path.
fn write_agents_json_cmd(cmd: &Path, dir: &Path, mode: Option<&str>) {
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Agent",
                "command": cmd.to_string_lossy(),
                "args": mode.map(|m| vec![m.to_string()]).unwrap_or_default(),
                "env": {}
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Copy the fake agent binary to a unique path.
///
/// Tests in this binary run in parallel and all launch the same fake
/// agent binary; unless each test uses its own copy, a `pgrep` on the
/// shared path cannot tell whose agent is whose (the `agent_death` test
/// would kill another test's agent while waiting on its own, which is
/// gone). The copy lives in `dir`, which each test removes at end.
fn unique_fake_agent(dir: &Path) -> PathBuf {
    let path = dir.join(format!("fake_agent-{}", uuid::Uuid::new_v4()));
    std::fs::copy(FAKE_AGENT, &path).unwrap();
    path
}

/// Poll until the fake agent process is gone, or `timeout` elapses.
/// Returns true if the process was reaped in time.
///
/// Uses a fast DIRECT `/proc` scan (no `pgrep` fork+exec) — the process under
/// test lives only a few ms, so the observation must be fast. We POLL for the
/// reap with a deadline (we WAIT for it, we don't sleep and hope). A zombie
/// (killed but not yet reaped) reads as GONE (its `cmdline` is empty), the same
/// as `pgrep`'s default skip-zombies behavior.
fn wait_for_process_gone(pattern: &Path, timeout: Duration) -> bool {
    let scan = common::proc_scan::ProcScan::new(pattern);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !scan.alive() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !scan.alive()
}

/// Call `op` (a `start_session` / `resume_session` future), RETRYING on a
/// transient `SpawnFailed` until a non-`SpawnFailed` result or the deadline.
/// Returns the first non-`SpawnFailed` result (an `Ok`, or a different error
/// the caller is asserting on, e.g. `InitializeFailed` / `NotResumable`).
///
/// A `SpawnFailed` is retried because the agent binary was just copied to a
/// known-good path, so the only realistic cause is a TRANSIENT OS refusal (the
/// test process's `fork()` refusing under load — the machine is overcommitted).
/// The spawn is idempotent (a fresh process each attempt; a failed spawn
/// registers no session), so retrying is safe. A SUSTAINED failure (the
/// deadline) returns the last error, so the test still fails loudly if the
/// spawn genuinely can't succeed. This makes the test DETERMINISTIC: it waits
/// for a successful spawn instead of hoping the first attempt succeeds.
async fn retry_on_spawn_failed<T, F, Fut>(deadline: Duration, mut op: F) -> Result<T, AcpError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AcpError>>,
{
    let end = Instant::now() + deadline;
    loop {
        match op().await {
            Err(AcpError::SpawnFailed { .. }) if Instant::now() < end => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

/// `start_session`, retrying a transient `SpawnFailed` (see
/// `retry_on_spawn_failed`).
async fn start_retrying(
    manager: &SessionManager,
    agent_id: &str,
    cwd: PathBuf,
    sink: &Arc<dyn EventSink>,
) -> Result<SessionInfo, AcpError> {
    retry_on_spawn_failed(Duration::from_secs(20), || {
        manager.start_session(agent_id, cwd.clone(), sink)
    })
    .await
}

/// `resume_session`, retrying a transient `SpawnFailed` (see
/// `retry_on_spawn_failed`).
async fn resume_retrying(
    manager: &SessionManager,
    agent_id: &str,
    session_id: &str,
    cwd: PathBuf,
    sink: &Arc<dyn EventSink>,
) -> Result<SessionInfo, AcpError> {
    retry_on_spawn_failed(Duration::from_secs(20), || {
        manager.resume_session(agent_id, session_id, cwd.clone(), sink)
    })
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_session_flow_streams_and_cleans_up() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    // 1. Start the session (initialize + session/new).
    let info = start_retrying(&manager, "fake", cwd, &sink)
        .await
        .expect("start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID);

    // 2. Send a prompt; the fake agent streams two chunks then end_turn.
    let reason = manager
        .send_prompt(FAKE_SESSION_ID, "hi".to_string())
        .await
        .expect("send_prompt should succeed");
    assert_eq!(reason, StopReason::EndTurn);

    // 3. Both chunks arrive, in order.
    let events = wait_for_events(&rx, 2, Duration::from_secs(5));
    let texts: Vec<String> = events
        .iter()
        .filter(|(event, _)| event == "session-update")
        .filter_map(|(_, p)| session_update_text(p))
        .collect();
    assert_eq!(texts, vec!["hello".to_string(), " world".to_string()]);

    // 4. Close the session.
    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");

    // 5. session-closed event arrives; the shared map is empty.
    let events = wait_for_events(&rx, 1, Duration::from_secs(10));
    let closed = events
        .iter()
        .find(|(event, _)| event == "session-closed")
        .expect("session-closed event should arrive");
    assert_eq!(closed.1["reason"], "user");
    assert_eq!(manager.session_count().await, 0);

    // 6. The child process is reaped (the transport task kills the process
    //    group; this happens just after the session-closed emit, so poll).
    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "fake_agent process should have been reaped after close"
    );

    // Clean up the temp dir (best effort).
    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_session_round_trips_the_session_id() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, Some("resume"));

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    // Resume the stored session id. The fake agent in `resume` mode
    // advertises `loadSession: true` and answers `session/load`.
    let info = resume_retrying(&manager, "fake", FAKE_SESSION_ID, cwd, &sink)
        .await
        .expect("resume_session should succeed");
    assert_eq!(
        info.session_id.to_string(),
        FAKE_SESSION_ID,
        "the resumed session id must round-trip"
    );
    assert!(
        info.capabilities.load_session,
        "the fake agent in resume mode advertises load_session"
    );

    // The agent replays a chunk on load; it must arrive as a session-update.
    let events = wait_for_events(&rx, 1, Duration::from_secs(5));
    let text = events
        .iter()
        .find(|(event, _)| event == "session-update")
        .and_then(|(_, p)| session_update_text(p));
    assert_eq!(
        text.as_deref(),
        Some("resumed"),
        "the load replay chunk should be delivered to the client"
    );

    // The resumed session lives in the same map and closes the same way.
    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");

    let events = wait_for_events(&rx, 1, Duration::from_secs(10));
    let closed = events
        .iter()
        .find(|(event, _)| event == "session-closed")
        .expect("session-closed event should arrive");
    assert_eq!(closed.1["reason"], "user");
    assert_eq!(manager.session_count().await, 0);

    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "fake_agent process should have been reaped after close"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn establishment_times_out_when_the_agent_hangs() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, Some("hang"));

    let (tx, _rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    // Shrink the (default 30 s) establishment timeout so the test stays
    // fast; the semantics under test are timing out, not the duration.
    manager.set_establish_timeout(Duration::from_millis(500));
    let cwd = config_dir.clone();

    // The agent answers `initialize` but never answers `session/new` —
    // establishment must time out instead of hanging forever.
    // (The inner `start_session` retries a transient `SpawnFailed`; its 20 s
    // retry deadline is inside the 30 s outer timeout, so the outer guard
    // still bounds the whole thing.)
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        start_retrying(&manager, "fake", cwd, &sink),
    )
    .await
    .expect("start_session must not hang past the establishment timeout");
    let err = result.expect_err("start_session must fail against a hanging agent");
    assert!(
        matches!(err, AcpError::InitializeFailed { .. }),
        "expected InitializeFailed on establishment timeout, got {err:?}"
    );
    assert_eq!(manager.session_count().await, 0);

    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "the hanging agent should be torn down after the timeout"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_replaces_stored_transcript() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, Some("resume"));

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    manager.attach_db(db.clone());
    let cwd = config_dir.clone();

    // 1. Start the session and populate the stored transcript.
    start_retrying(&manager, "fake", cwd.clone(), &sink)
        .await
        .expect("start_session should succeed");
    manager
        .send_prompt(FAKE_SESSION_ID, "hi".to_string())
        .await
        .expect("send_prompt should succeed");
    wait_for_events(&rx, 2, Duration::from_secs(5));
    let rows = db.messages_for(FAKE_SESSION_ID).expect("messages_for");
    assert_eq!(
        rows.len(),
        2,
        "user + agent-text rows must exist before the resume"
    );

    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");
    assert_closed_event(&rx).await;
    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "first agent should be reaped before the resume spawn"
    );

    // 2. Resume. The fake agent replays one chunk (`m1`: "resumed") on
    //    load — the stored transcript must be REPLACED by it, not
    //    duplicated (no stale `hello world` row, no duplicate `m1` row).
    resume_retrying(&manager, "fake", FAKE_SESSION_ID, cwd, &sink)
        .await
        .expect("resume_session should succeed");

    // Poll the DB until the replayed row is persisted (the restore builder
    // delivers the pre-response chunk right after the load response).
    let deadline = Instant::now() + Duration::from_secs(10);
    let rows = loop {
        let rows = db.messages_for(FAKE_SESSION_ID).expect("messages_for");
        let done = rows.iter().any(|r| {
            r.kind == "agent-text"
                && r.message_key.as_deref() == Some("m1")
                && r.payload_json.contains("resumed")
        });
        if done {
            break rows;
        }
        if Instant::now() > deadline {
            panic!("the resumed replay row was not persisted; got {rows:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    assert_eq!(
        rows.len(),
        1,
        "exactly one row must remain: the replayed chunk (no duplicate, no stale rows); got {rows:?}"
    );
    assert_eq!(rows[0].kind, "agent-text");
    let payload: serde_json::Value = serde_json::from_str(&rows[0].payload_json).unwrap();
    assert_eq!(
        payload["text"], "resumed",
        "the stored row must hold the replayed text, not the old "
    );

    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");
    assert_closed_event(&rx).await;
    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "the resumed agent should be reaped after close"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

/// Poll a `session-closed` event for `(session_id, reason)` out of a possibly
/// busy event queue: scan until the exact pair is seen, budget 5 s.
async fn wait_for_closed(rx: &Receiver<(String, Value)>, session_id: &str, reason: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while !seen && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok((event, payload)) if event == "session-closed" => {
                seen = payload["sessionId"] == session_id && payload["reason"] == reason;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(
        seen,
        "a session-closed event for `{session_id}` with reason `{reason}` should arrive within 5s"
    );
}

/// Drive the one-live policy (ADR 0002) end to end: a second `start_session`
/// supersedes the first with the `replaced` close reason, a superseded session
/// re-resumable, and the superseded processes are all reaped (no leak).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_live_supersede_policy() {
    let config_dir = temp_config_dir();
    // Two DISTINCT copies: the teardown-reap assertions discriminate the two
    // agents by path, which is what makes step 11 a real leak check rather
    // than a single shared-binary assertion.
    let bin_a = unique_fake_agent(&config_dir);
    let bin_b = unique_fake_agent(&config_dir);
    {
        let json = serde_json::json!({
            "agents": [
                {
                    "id": "l1",
                    "name": "L1",
                    "command": bin_a.to_string_lossy(),
                    "args": ["resume"],
                    "env": { "FAKE_SESSION_ID": "s1" }
                },
                {
                    "id": "l2",
                    "name": "L2",
                    "command": bin_b.to_string_lossy(),
                    "args": ["resume"],
                    "env": { "FAKE_SESSION_ID": "s2" }
                }
            ]
        });
        std::fs::write(
            config_dir.join("agents.json"),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    // `args: ["resume"]` is deliberate: one mode answers `session/new`,
    // advertises `loadSession: true`, answers `session/load`, and answers
    // `session/prompt` — covering both `start_session` and `resume_session`.

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    manager.attach_db(Arc::new(
        Db::open(&config_dir.join("archimedes.db")).expect("db should open"),
    ));
    manager.set_establish_timeout(Duration::from_secs(2));

    let cwd_a = config_dir.join("cwda");
    let cwd_b = config_dir.join("cwdb");
    std::fs::create_dir_all(&cwd_a).unwrap();
    std::fs::create_dir_all(&cwd_b).unwrap();

    // 1. A nonexistent folder is rejected before anything is spawned or
    //    superseded (early canonicalize in `start_session`).
    let missing = config_dir.join(format!("nope-{}", uuid::Uuid::new_v4()));
    let err = manager
        .start_session("l1", missing, &sink)
        .await
        .expect_err("start_session should fail for a missing folder");
    assert!(
        matches!(err, AcpError::FolderMissing { .. }),
        "expected FolderMissing, got {err:?}"
    );
    assert_eq!(
        manager.session_count().await,
        0,
        "nothing should be spawned for a missing folder"
    );

    // 2. The first session starts and is the only live session.
    start_retrying(&manager, "l1", cwd_a.clone(), &sink)
        .await
        .expect("start_session l1 should succeed");
    assert_eq!(manager.session_count().await, 1);

    // 3. A second live session starts: it supersedes s1 (an async flag send
    //    — the teardown runs concurrently in s1's driver task).
    start_retrying(&manager, "l2", cwd_b.clone(), &sink)
        .await
        .expect("start_session l2 should succeed");
    // Do NOT assert the count right here: s2 is already in the map and s1's
    // driver cleanup (the map removal) races s2's insertion, so the count
    // may still read 2 for a short window.

    // 4. s1's close event (`replaced`) arrives. It is emitted AFTER s1 has
    //    been removed from the sessions map, so observing it guarantees s1
    //    is gone.
    wait_for_closed(&rx, "s1", "replaced").await;

    // 5. Now (after the close event, not right after step 3) the map holds
    //    exactly the one live session.
    assert_eq!(
        manager.session_count().await,
        1,
        "s1 should be gone and s2 present"
    );

    // 6. The new live session is fully functional after the supersede
    //    (the default-mode prompt handler streams two chunks before
    //    end_turn; the chunk events may arrive interleaved — that is fine).
    manager
        .send_prompt("s2", "hi".to_string())
        .await
        .expect("send_prompt should succeed on the superseding session");

    // 7. Close s2; wait for its close event BEFORE asserting on counts
    //    (teardown is an async flag send).
    manager
        .close_session("s2")
        .await
        .expect("close_session s2 should succeed");
    wait_for_closed(&rx, "s2", "user").await;
    assert_eq!(
        manager.session_count().await,
        0,
        "everything should be closed"
    );

    // 8. A superseded session is re-resumable (loadSession is true in the
    //    `resume` mode); step 7 closed everything, so this takes the policy's
    //    no-op branch and s1 establishes cleanly.
    resume_retrying(&manager, "l1", "s1", cwd_a, &sink)
        .await
        .expect("resume_session s1 should succeed after a supersede");
    assert_eq!(manager.session_count().await, 1);

    // 9. `start_session` supersedes a *resumed* session too.
    start_retrying(&manager, "l2", cwd_b.clone(), &sink)
        .await
        .expect("start_session l2 should succeed (again)");
    wait_for_closed(&rx, "s1", "replaced").await;
    assert_eq!(
        manager.session_count().await,
        1,
        "only the newest session should be live"
    );

    // 10. Close everything again.
    manager
        .close_session("s2")
        .await
        .expect("close_session s2 should succeed (again)");
    wait_for_closed(&rx, "s2", "user").await;
    assert_eq!(
        manager.session_count().await,
        0,
        "everything should be closed again"
    );

    // 11. Process teardown proof: poll until BOTH agents are gone (deadline
    //     10 s; same `pgrep` idiom as the existing reap checks) — the
    //     superseded/closed processes were actually reaped, none leaked.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline
        && (find_fake_agent_pid(&bin_a).is_some() || find_fake_agent_pid(&bin_b).is_some())
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        find_fake_agent_pid(&bin_a).is_none(),
        "agent a should have been reaped (no leak)"
    );
    assert!(
        find_fake_agent_pid(&bin_b).is_none(),
        "agent b should have been reaped (no leak)"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

/// Drain events until a `session-closed` event arrives (other events that
/// are still in the queue do not mask it).
async fn assert_closed_event(rx: &std::sync::mpsc::Receiver<(String, serde_json::Value)>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while !seen && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok((event, _)) if event == "session-closed" => seen = true,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(seen, "a session-closed event should arrive after close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_without_load_session_is_not_resumable() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, None); // default mode: loadSession: false

    let (tx, _rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    let err = resume_retrying(&manager, "fake", FAKE_SESSION_ID, cwd, &sink)
        .await
        .expect_err("resume should fail: the agent does not advertise load_session");
    assert!(
        matches!(err, AcpError::NotResumable { .. }),
        "expected NotResumable, got {err:?}"
    );
    assert_eq!(
        manager.session_count().await,
        0,
        "no session should be registered when resume is refused"
    );

    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "the spawned agent should be torn down after a refused resume"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transcript_is_persisted_with_upsert_semantics() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    manager.attach_db(db.clone());

    let cwd = config_dir.clone();
    let info = start_retrying(&manager, "fake", cwd, &sink)
        .await
        .expect("start_session should succeed");
    manager
        .send_prompt(FAKE_SESSION_ID, "hi".to_string())
        .await
        .expect("send_prompt should succeed");

    // Both chunks have been delivered (persistence runs in the same
    // notification handler, right after the emit, so the rows exist by now).
    wait_for_events(&rx, 2, Duration::from_secs(5));

    let messages = db
        .messages_for(FAKE_SESSION_ID)
        .expect("messages_for should succeed");
    let kinds: Vec<&str> = messages.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["user", "agent-text"],
        "one user row and exactly one agent-text row (two chunks upserted); got {kinds:?}"
    );

    let user_row = &messages[0];
    let payload: serde_json::Value = serde_json::from_str(&user_row.payload_json).unwrap();
    assert_eq!(payload["text"], "hi");

    let agent_row = &messages[1];
    assert_eq!(agent_row.message_key.as_deref(), Some("m1"));
    let payload: serde_json::Value = serde_json::from_str(&agent_row.payload_json).unwrap();
    assert_eq!(
        payload["text"], "hello world",
        "the agent-text row must hold the accumulated text"
    );

    // The session row exists with the negotiated capabilities.
    let sessions = db.list_sessions().expect("list_sessions should succeed");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, info.session_id.to_string());
    assert!(sessions[0].capabilities_json.contains("loadSession"));

    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");
    let events = wait_for_events(&rx, 1, Duration::from_secs(10));
    assert!(events.iter().any(|(event, _)| event == "session-closed"));

    assert!(
        wait_for_process_gone(&agent_bin, Duration::from_secs(10)),
        "fake_agent process should have been reaped after close"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_death_produces_session_closed() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    let info = start_retrying(&manager, "fake", cwd, &sink)
        .await
        .expect("start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID);

    // Find and kill the fake agent.
    let pid = find_fake_agent_pid(&agent_bin).expect("fake agent should be running");
    kill_pid(pid);

    // The driver task should detect the exit, remove the session, and emit
    // session-closed with reason "agent-exited".
    let events = wait_for_events(&rx, 1, Duration::from_secs(10));
    let closed = events
        .iter()
        .find(|(event, _)| event == "session-closed")
        .expect("session-closed event should arrive on agent death");
    assert_eq!(closed.1["reason"], "agent-exited");
    assert_eq!(manager.session_count().await, 0);

    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permission_round_trip_is_answerable_and_nonblocking() {
    let config_dir = temp_config_dir();
    let agent_bin = unique_fake_agent(&config_dir);
    write_agents_json_cmd(&agent_bin, &config_dir, Some("permission"));

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = Arc::new(SessionManager::new(config_dir.clone()).unwrap());
    let cwd = config_dir.clone();

    let info = start_retrying(&manager, "fake", cwd, &sink)
        .await
        .expect("start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID);

    // Spawn the prompt; it blocks until the agent finishes, which requires the
    // user to answer the permission request.
    let manager2 = Arc::clone(&manager);
    let prompt_task = tokio::spawn(async move {
        manager2
            .send_prompt(FAKE_SESSION_ID, "hi".to_string())
            .await
    });

    // Drain events until the agent's permission request reaches the client.
    // The agent emits a `pre` chunk BEFORE the request; if the permission
    // handler blocked the event loop, that chunk would never be delivered while
    // the prompt is open. So its arrival proves the loop stayed responsive.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_permission_request = false;
    let mut saw_pre_chunk = false;
    while !saw_permission_request && Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok((event, payload)) => {
                if event == "permission-request" {
                    saw_permission_request = true;
                }
                if event == "session-update" {
                    if let Some(text) = session_update_text(&payload) {
                        if text == "pre" {
                            saw_pre_chunk = true;
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    assert!(
        saw_permission_request,
        "a permission-request event should be emitted for the agent's request"
    );
    assert!(
        saw_pre_chunk,
        "the pre-request chunk must arrive while the prompt is pending - the event loop was not blocked by the permission handler"
    );

    // Answer the permission prompt with the first option.
    manager
        .respond_permission(
            FAKE_SESSION_ID,
            "100",
            PermissionOutcome::Selected {
                option_id: "opt-1".to_string(),
            },
        )
        .await
        .expect("respond_permission should succeed");

    // The prompt should now complete with end_turn.
    let reason = prompt_task
        .await
        .unwrap()
        .expect("send_prompt should succeed");
    assert_eq!(reason, StopReason::EndTurn);

    // Collect the remaining chunks and assert the outcome + streaming order.
    let events = wait_for_events(&rx, 3, Duration::from_secs(5));
    let texts: Vec<String> = events
        .iter()
        .filter(|(event, _)| event == "session-update")
        .filter_map(|(_, p)| session_update_text(p))
        .collect();
    // The outcome chunk must reflect the selected option, and the streaming
    // chunks must follow it (they were emitted after the agent got its answer).
    assert!(
        texts.contains(&"outcome:selected:opt-1".to_string()),
        "agent should report the selected option; got {texts:?}"
    );
    assert!(
        texts.contains(&"hello".to_string()) && texts.contains(&" world".to_string()),
        "streaming chunks should arrive after the permission answer; got {texts:?}"
    );

    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");

    let _ = std::fs::remove_dir_all(&config_dir);
}
