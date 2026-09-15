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
use archimedes_desktop_lib::acp::{EventSink, SessionManager};

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

fn write_agents_json(dir: &Path) {
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Agent",
                "command": FAKE_AGENT,
                "args": [],
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

/// Find the pid of the running fake agent, if any.
fn find_fake_agent_pid() -> Option<i32> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", FAKE_AGENT])
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

/// Poll until the fake agent process is gone, or `timeout` elapses.
/// Returns true if the process was reaped in time.
fn wait_for_process_gone(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if find_fake_agent_pid().is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    find_fake_agent_pid().is_none()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_session_flow_streams_and_cleans_up() {
    let config_dir = temp_config_dir();
    write_agents_json(&config_dir);

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
        wait_for_process_gone(Duration::from_secs(10)),
        "fake_agent process should have been reaped after close"
    );

    // Clean up the temp dir (best effort).
    let _ = std::fs::remove_dir_all(&config_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_death_produces_session_closed() {
    let config_dir = temp_config_dir();
    write_agents_json(&config_dir);

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
    let pid = find_fake_agent_pid().expect("fake agent should be running");
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
