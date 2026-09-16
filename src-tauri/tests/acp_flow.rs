//! Integration test: drives the fake ACP agent through the full session
//! lifecycle — initialize → session/new → prompt → streamed updates → close —
//! and verifies the connection is torn down and the child process reaped.
//!
//! A second test kills the fake agent mid-session and asserts the session is
//! removed and a `session-closed` event with `reason: "agent-exited"` is
//! emitted.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use agent_client_protocol::schema::v1::StopReason;
use archimedes_desktop_lib::acp::{AcpError, EventSink, PermissionOutcome, SessionManager};
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
fn find_fake_agent_pid(pattern: &Path) -> Option<i32> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", pattern.to_string_lossy().as_ref()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .split_whitespace()
        .next()
        .and_then(|p| p.parse().ok())
}

fn kill_pid(pid: i32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
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
fn wait_for_process_gone(pattern: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if find_fake_agent_pid(pattern).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    find_fake_agent_pid(pattern).is_none()
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
    let info = manager
        .start_session("fake", cwd, &sink)
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
    let info = manager
        .resume_session("fake", FAKE_SESSION_ID, cwd, &sink)
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
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        manager.start_session("fake", cwd, &sink),
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
    manager
        .start_session("fake", cwd.clone(), &sink)
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
    manager
        .resume_session("fake", FAKE_SESSION_ID, cwd, &sink)
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

    let err = manager
        .resume_session("fake", FAKE_SESSION_ID, cwd, &sink)
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
    let info = manager
        .start_session("fake", cwd, &sink)
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

    let info = manager
        .start_session("fake", cwd, &sink)
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

    let info = manager
        .start_session("fake", cwd, &sink)
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
