//! Integration test for the ACP `terminal/*` client backend.
//!
//! The fake agent (in `terminal` mode) drives a full terminal lifecycle:
//! `terminal/create` (running `echo hello`) → `terminal/output` →
//! `terminal/wait_for_exit`. The client spawns a real PTY, buffers the output,
//! emits a `terminal-output` Tauri event (base64), and reports the exit status.
//!
//! The test asserts:
//!   1. a `terminal-output` event carries the bytes for `hello`, and
//!   2. the `terminal/wait_for_exit` response carries exit code 0 (echoed back
//!      by the agent as a `exit:0` message chunk).

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::prelude::BASE64_STANDARD;
use base64::Engine as _;
use serde_json::Value;

use agent_client_protocol::schema::v1::StopReason;
use archimedes_desktop_lib::acp::{EventSink, SessionManager};

/// The fixed session id reported by the fake agent (see `bin/fake_agent.rs`).
const FAKE_SESSION_ID: &str = "fake-session-1";

/// The full path to the compiled `fake_agent` binary.
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

#[derive(Clone)]
struct TestSink {
    tx: std::sync::mpsc::Sender<(String, Value)>,
}

impl EventSink for TestSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

fn temp_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("acp-terminal-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an agents.json whose `fake` agent runs the fake agent in `terminal`
/// mode.
fn write_agents_json(dir: &Path) {
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Agent",
                "command": FAKE_AGENT,
                "args": ["terminal"],
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

/// Collect every event that arrives within `duration` (regardless of count).
fn drain_events(rx: &Receiver<(String, Value)>, duration: Duration) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(e) => events.push(e),
            Err(_) => break,
        }
    }
    events
}

/// Find the pid of a running fake agent, if any.
fn find_fake_agent_pid() -> Option<i32> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", FAKE_AGENT])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .and_then(|p| p.parse().ok())
}

/// Poll until no fake agent process remains, or `timeout` elapses.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_create_output_and_wait_for_exit() {
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

    // The agent's prompt handler runs the terminal sequence end-to-end; the
    // client handles every terminal request autonomously, so the prompt
    // completes on its own.
    let reason = manager
        .send_prompt(FAKE_SESSION_ID, "run it".to_string())
        .await
        .expect("send_prompt should succeed");
    assert_eq!(reason, StopReason::EndTurn);

    // Give the event sink a moment to drain the terminal-output events. All
    // of them are emitted before the prompt completes, so a short drain is
    // enough.
    let events = drain_events(&rx, Duration::from_millis(1500));

    // 1. A terminal-output event carries "hello".
    let mut saw_hello = false;
    for (event, payload) in &events {
        if event != "terminal-output" {
            continue;
        }
        let Some(data) = payload.get("data").and_then(|d| d.as_str()) else {
            continue;
        };
        let bytes = BASE64_STANDARD.decode(data).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        if text.contains("hello") {
            saw_hello = true;
        }
    }
    assert!(
        saw_hello,
        "a terminal-output event should carry the 'hello' bytes; events: {events:?}"
    );

    // 2. The wait_for_exit response carried exit code 0 (echoed by the agent
    //    as an `exit:0` message chunk).
    let exit_chunks: Vec<String> = events
        .iter()
        .filter(|(event, _)| event == "session-update")
        .filter_map(|(_, p)| p.pointer("/update/content/text").and_then(|t| t.as_str()))
        .filter(|t| t.starts_with("exit:"))
        .map(|t| t.to_string())
        .collect();
    assert!(
        exit_chunks.contains(&"exit:0".to_string()),
        "agent should echo exit:0 from the wait_for_exit response; events: {events:?}"
    );

    manager
        .close_session(FAKE_SESSION_ID)
        .await
        .expect("close_session should succeed");

    // Wait for the agent process to be reaped so it cannot interfere with
    // other tests that locate the fake agent by name.
    assert!(
        wait_for_process_gone(Duration::from_secs(10)),
        "fake_agent process should have been reaped after close"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}
