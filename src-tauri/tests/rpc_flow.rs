//! Integration test: drives `fake_pi` through the full session lifecycle —
//! `get_state` establish → `prompt` → streamed `session-update` frames →
//! close → `session-closed` — plus the establishment-timeout, resume-gate,
//! agent-death, and two-sessions-coexist behaviors.
//!
//! The `fake_pi` binary is spawned DIRECTLY (no copy): the ETXTBSY lesson
//! (a copy races the kernel's write-fd check) — and these tests never reap
//! processes by binary path (the driver kills via the child handle `PiRpc`
//! owns), so a shared path cannot false-positive.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use archimedes_lib::agent::{
    EventSink, PermissionOutcome, RpcError, SessionInfo, SessionManager, StopReason,
};
use archimedes_lib::storage::Db;

/// The session-id PREFIX `fake_pi` reports for a fresh session (the id
/// itself is UNIQUE per process — see `bin/fake_pi.rs`).
const FAKE_SESSION_PREFIX: &str = "fake-pi-";

/// The `fake_pi` binary path (integration tests get `CARGO_BIN_EXE_*`).
const FAKE_PI: &str = env!("CARGO_BIN_EXE_fake_pi");

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
    let dir = std::env::temp_dir().join(format!("rpc-flow-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an `agents.json` with a single `fake` entry pointing at `fake_pi`
/// with the given env (e.g. `FAKE_PI_HANG=1`). `marker` (when `Some`) is
/// an extra ARG the fake ignores but the process's cmdline carries — a
/// unique per-test marker a test can `pkill -f` (a shared binary path
/// cannot be matched without killing other tests' children).
fn write_agents_json_pi(dir: &std::path::Path, env: &[(&str, &str)], marker: Option<&str>) {
    let args: Vec<Value> = marker
        .map(|m| {
            vec![
                Value::String("--flow-test".to_string()),
                Value::String(m.to_string()),
            ]
        })
        .unwrap_or_default();
    // `env` is a JSON OBJECT (a `BTreeMap` on the wire) — the slice form
    // would serialize as an array of pairs and fail to deserialize.
    let env_map: serde_json::Map<String, Value> = env
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect();
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Pi",
                "command": FAKE_PI,
                "args": args,
                "env": env_map,
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Poll the event channel until `count` events arrive (or the timeout).
fn wait_for_events(
    rx: &Receiver<(String, Value)>,
    count: usize,
    timeout: Duration,
) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + timeout;
    while events.len() < count && std::time::Instant::now() < deadline {
        // `Timeout` = no event yet (KEEP waiting until the deadline —
        // breaking on the first 100 ms timeout would return an empty vec
        // the instant the first event is slightly late); `Disconnected`
        // = the sender is gone (stop).
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(e) => events.push(e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    events
}

/// Poll the channel until an event named `want` arrives (skipping the
/// unrelated events queued ahead of it — e.g. the `session-update` frames
/// of an earlier prompt), or the timeout.
fn wait_for_event_named(
    rx: &Receiver<(String, Value)>,
    want: &str,
    timeout: Duration,
) -> Option<(String, Value)> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(e) if e.0 == want => return Some(e),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Wrap an `update` object in the `session-update` payload shape
/// (`session_update_text` reads `payload["update"]`).
fn p_update(update: &Value) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        "update".to_string(),
        update.clone(),
    )]))
}

/// The text of a `session-update` `agent_message_chunk` frame (if any).
fn session_update_text(payload: &Value) -> Option<String> {
    let update = payload.get("update")?;
    if update.get("sessionUpdate")?.as_str()? != "agent_message_chunk" {
        return None;
    }
    update
        .get("content")?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

/// (1) The full lifecycle: `start_session` (the `get_state` establish) →
/// `send_prompt` (the turn streams 2 `agent_message_chunk` frames keyed
/// `m1` then settles `EndTurn`) → `close_session` (the `session-closed`
/// event with `reason: "user"` + an empty session map).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_session_flow_streams_and_cleans_up() {
    let config_dir = temp_config_dir();
    write_agents_json_pi(&config_dir, &[], None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    // 1. Start the session (the `get_state` establish).
    let info = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session should succeed");
    assert!(
        info.session_id.starts_with(FAKE_SESSION_PREFIX),
        "a fresh session gets a unique pi id, got {}",
        info.session_id
    );
    // The capability envelope (the load-bearing keys).
    assert_eq!(
        info.capabilities["loadSession"], true,
        "fake_pi reports a session file"
    );

    // 2. Send a prompt; the fake streams two chunks then settles.
    let reason = manager
        .send_prompt(&info.session_id, "hi".to_string())
        .await
        .expect("send_prompt should succeed");
    assert_eq!(reason, StopReason::EndTurn);

    // 3. Both chunks arrive, in order (the `messageId` role rule keys them
    //    `m1` — the user `message_start` did not advance the counter).
    let events = wait_for_events(&rx, 2, Duration::from_secs(5));
    let mut chunks: Vec<&Value> = Vec::new();
    for (event, p) in &events {
        if event != "session-update" {
            continue;
        }
        let update = &p["update"];
        if update.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk") {
            chunks.push(update);
        }
    }
    assert_eq!(
        chunks.len(),
        2,
        "two text deltas (Hel + lo), got {events:?}"
    );
    let texts: Vec<String> = chunks
        .iter()
        .map(|u| session_update_text(&p_update(u)).expect("a text delta has text"))
        .collect();
    assert_eq!(texts, vec!["Hel".to_string(), "lo".to_string()]);
    assert_eq!(chunks[0]["messageId"], "m1");
    assert_eq!(chunks[1]["messageId"], "m1");

    // 4. Close the session.
    manager
        .close_session(&info.session_id)
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

    // Clean up the temp dir (best effort).
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (2) The establishment timeout: `fake_pi` in `FAKE_PI_HANG` mode never
/// answers `get_state` — establishment must time out (the
/// `EstablishTimeout` error) instead of hanging forever, and the session
/// must NOT be registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn establishment_times_out_when_the_agent_hangs() {
    let config_dir = temp_config_dir();
    write_agents_json_pi(&config_dir, &[("FAKE_PI_HANG", "1")], None);

    let (tx, _rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    // Shrink the (default 30 s) establishment timeout so the test stays
    // fast; the semantics under test are timing out, not the duration.
    manager.set_establish_timeout(Duration::from_millis(500));
    let cwd = config_dir.clone();

    let result = tokio::time::timeout(
        Duration::from_secs(30),
        archimedes_lib::test_support::run_with_retry(|| async {
            manager.start_session("fake", cwd.clone(), &sink).await
        }),
    )
    .await
    .expect("start_session must not hang past the establishment timeout");
    let err = result.expect_err("start_session must fail against a hanging agent");
    assert!(
        matches!(err, RpcError::EstablishTimeout { .. }),
        "expected EstablishTimeout on establishment timeout, got {err:?}"
    );
    assert_eq!(manager.session_count().await, 0);

    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (3) The resume gate: a stored session whose capabilities lack
/// `loadSession` (a legacy / `--no-session` row) is `NotResumable` — and
/// the refusal happens BEFORE spawning (no session is registered).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_without_load_session_is_not_resumable() {
    let config_dir = temp_config_dir();
    write_agents_json_pi(&config_dir, &[], None);

    let (tx, _rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    manager.attach_db(db.clone());
    // A stored row WITHOUT `loadSession` (a pre-swap legacy row — the
    // `list_sessions` normalizer maps it to `loadSession: false`).
    db.record_session(&SessionInfo {
        session_id: "legacy-1".to_string(),
        agent_id: "fake".to_string(),
        cwd: config_dir.clone(),
        capabilities: serde_json::json!({ "promptCapabilities": { "image": true } }),
        config_options: None,
        archived: false,
    })
    .expect("record_session should succeed");
    let cwd = config_dir.clone();

    let err = manager
        .resume_session("fake", "legacy-1", cwd, &sink)
        .await
        .expect_err("resume should fail: the stored session is not resumable");
    assert!(
        matches!(err, RpcError::NotResumable { .. }),
        "expected NotResumable, got {err:?}"
    );
    assert_eq!(
        manager.session_count().await,
        0,
        "no session should be registered when resume is refused"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (4) Agent death: killing the `fake_pi` process mid-session tears the
/// session down — a `session-closed` event with `reason: "agent-exited"`
/// and an empty session map.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_death_produces_session_closed() {
    let config_dir = temp_config_dir();
    // A unique marker arg (the fake ignores it; the process's cmdline
    // carries it — `pkill -f` matches it without touching other tests'
    // children, which share the same binary path).
    let marker = uuid::Uuid::new_v4().to_string();
    write_agents_json_pi(&config_dir, &[], Some(&marker));

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    let info = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session should succeed");

    // Kill the agent process (the marker arg is unique to this test — a
    // shared binary path cannot be matched without killing other tests'
    // children). The driver observes the exit (the exit-watcher task
    // reaps the child) and tears the session down.
    let mut matched = false;
    for _ in 0..30 {
        if let Ok(s) = std::process::Command::new("pkill")
            .args(["-f", "--", &format!("--flow-test {marker}")])
            .status()
        {
            // `pkill` exits 0 when it matched a process, 1 when none.
            matched = s.code() == Some(0);
            if matched {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        matched,
        "the agent process should be found (marker {marker})"
    );

    // The `session-closed` event arrives with the `agent-exited` reason
    // (and the session's id).
    let events = wait_for_events(&rx, 5, Duration::from_secs(15));
    let closed = events
        .iter()
        .find(|(event, _)| event == "session-closed")
        .expect("session-closed event should arrive after the agent dies");
    assert_eq!(closed.1["sessionId"], info.session_id);
    assert_eq!(closed.1["reason"], "agent-exited");
    assert_eq!(manager.session_count().await, 0);

    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (5) Two live sessions coexist (ADR 0002 — the one-live cap is lifted):
/// starting a second session does NOT close the first; both stream
/// independently; closing one leaves the other live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_live_sessions_coexist() {
    let config_dir = temp_config_dir();
    write_agents_json_pi(&config_dir, &[], None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let manager = SessionManager::new(config_dir.clone()).unwrap();
    let cwd = config_dir.clone();

    let s1 = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session #1 should succeed");
    assert!(
        s1.session_id.starts_with(FAKE_SESSION_PREFIX),
        "a fresh session gets a unique pi id, got {}",
        s1.session_id
    );

    // A second session (the same agent, a fresh spawn) — a DISTINCT pi
    // session id (the fake's per-process id; the driver keys the sessions
    // map by the pi `session_id`, so two live sessions are two entries).
    let s2 = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session #2 should succeed");
    assert_ne!(
        s1.session_id, s2.session_id,
        "two fresh sessions of the same agent have distinct pi ids"
    );
    assert_eq!(manager.session_count().await, 2, "both sessions are live");

    // Both stream independently.
    let r1 = manager
        .send_prompt(&s1.session_id, "a".to_string())
        .await
        .expect("prompt #1 should succeed");
    assert_eq!(r1, StopReason::EndTurn);
    let r2 = manager
        .send_prompt(&s2.session_id, "b".to_string())
        .await
        .expect("prompt #2 should succeed");
    assert_eq!(r2, StopReason::EndTurn);

    // Closing one leaves the other live.
    manager
        .close_session(&s1.session_id)
        .await
        .expect("close #1 should succeed");
    // Wait for the `session-closed` event (the first session — its id).
    // The earlier prompts' `session-update` frames are queued ahead of it
    // on the shared channel (skipped by the poll).
    let closed = wait_for_event_named(&rx, "session-closed", Duration::from_secs(10))
        .expect("session-closed event should arrive");
    assert_eq!(closed.1["sessionId"], s1.session_id);
    // The second session is still live (its handle is not closed).
    let r2b = manager
        .send_prompt(&s2.session_id, "c".to_string())
        .await
        .expect("prompt #2b should succeed (the second session is still live)");
    assert_eq!(r2b, StopReason::EndTurn);

    let _ = manager.close_session(&s2.session_id).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// A fresh, distinct tempdir for a session's `cwd` (returned with its
/// canonicalized display string — the `spaces` join key is canonicalized).
fn temp_cwd() -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("rpc-flow-cwd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let canonical = dir.canonicalize().unwrap();
    (dir, canonical.display().to_string())
}

/// Poll the channel until a `permission-request` event arrives (or the
/// timeout) — ASYNC on purpose: a blocking `recv_timeout` on a runtime
/// worker starves the driver task this test needs to run CONCURRENTLY
/// (the `join!` gate test's lesson — the driver is a spawned task on the
/// same runtime; a blocked worker cannot make progress for it).
async fn wait_for_permission_request(
    rx: &std::sync::mpsc::Receiver<(String, Value)>,
    timeout: Duration,
) -> Option<(String, Value)> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(e) = rx.try_recv() {
            if e.0 == "permission-request" {
                return Some(e);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

/// (6) Trusted Space (ADR 0010): an UNTRUSTED Space's gate `confirm`
/// prompts with the third option (`trust-space`); picking it answers
/// `confirmed: true` (the turn settles) AND sets the trust flag
/// (the row existed, so the best-effort write succeeds).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn untrusted_space_prompts_with_third_option_and_trust_space_outcome_trusts() {
    let config_dir = temp_config_dir();
    let cwd = config_dir.clone();
    let cwd_str = cwd.canonicalize().unwrap().display().to_string();
    write_agents_json_pi(&config_dir, &[("FAKE_PI_GATE", "1")], None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    manager.attach_db(db.clone());
    // The space row exists (an UNTRUSTED row — `set_space_trusted` is an
    // UPDATE, so the `trust-space` outcome's write needs it). `false`
    // (the pre-change behavior — rows born untrusted via the schema
    // default; the settings flag is threaded only by `record_session`).
    db.upsert_space(&cwd_str, false)
        .expect("upsert_space should succeed");

    let info = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session should succeed");

    // The prompt + the gate answer, CONCURRENTLY (`join!` — the fake
    // settles only after the client's response; the `permission-request`
    // event arrives mid-turn, and the pending entry is registered BEFORE
    // the event is emitted, so `respond_permission` finds it by the time
    // the event is seen).
    let sid = info.session_id.clone();
    let (reason, answer) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(15),
            manager.send_prompt(&sid, "hi".to_string()),
        ),
        async {
            // The `permission-request` event (ASYNC polling — a blocking
            // `recv_timeout` on this worker would starve the driver task
            // that emits it; see `wait_for_permission_request`).
            let (event, payload) = wait_for_permission_request(&rx, Duration::from_secs(10))
                .await
                .expect("an untrusted Space's gate must prompt");
            assert_eq!(event, "permission-request");
            let request = &payload["request"];
            assert_eq!(request["sessionId"], sid);
            let options: Vec<String> = request["options"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["optionId"].as_str().unwrap().to_string())
                .collect();
            assert!(
                options.iter().any(|o| o == "trust-space"),
                "the untrusted prompt offers the trust-space option, got {options:?}"
            );
            let request_id = payload["requestId"].as_str().unwrap().to_string();
            // The user picks `trust-space` → `confirmed: true` AND the
            // flag is set.
            let hit = manager
                .respond_permission(
                    &sid,
                    &request_id,
                    PermissionOutcome::Selected {
                        option_id: "trust-space".to_string(),
                    },
                )
                .await
                .expect("respond_permission should succeed");
            assert!(hit, "the pending entry must be resolved");
            Ok::<(), ()>(())
        },
    );
    assert!(answer.is_ok());
    let reason = reason
        .expect("the turn must settle after the trust-space answer")
        .expect("send_prompt should succeed");
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the turn settles after the confirmed gate"
    );

    // The flag is set (the row existed, so the best-effort write after the
    // answer succeeded).
    assert!(
        db.space_trusted(&cwd).expect("space_trusted should work"),
        "the trust-space outcome sets the trust flag"
    );

    let _ = manager.close_session(&info.session_id).await;
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

/// (7) Trusted Space (ADR 0010): a TRUSTED Space's gate `confirm` is
/// answered `confirmed: true` WITHOUT a `permission-request` event (no
/// prompt, no oneshot) — the turn settles on its own (the fake settles
/// on the client's response, which the handler sends immediately).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trusted_space_skips_the_permission_prompt() {
    let config_dir = temp_config_dir();
    let (cwd, cwd_str) = temp_cwd();
    write_agents_json_pi(&config_dir, &[("FAKE_PI_GATE", "1")], None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    manager.attach_db(db.clone());
    // The space row exists and is TRUSTED (BEFORE the session starts).
    db.upsert_space(&cwd_str, false)
        .expect("upsert_space should succeed");
    db.set_space_trusted(&cwd_str, true)
        .expect("set_space_trusted should succeed");
    assert!(
        db.space_trusted(&cwd).expect("space_trusted should work"),
        "precondition: the space is trusted"
    );

    let info = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session should succeed");

    // The gate is auto-confirmed (no prompt) — the fake settles the turn
    // on the client's `confirmed: true`, so the prompt resolves on its
    // own (NO `respond_permission` call).
    let reason = tokio::time::timeout(
        Duration::from_secs(15),
        manager.send_prompt(&info.session_id, "hi".to_string()),
    )
    .await
    .expect("the turn must settle without a user answer")
    .expect("send_prompt should succeed");
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the turn settles after the auto-confirmed gate"
    );

    // NO `permission-request` event was emitted (drain the turn's
    // `session-update` frames — the gate was answered without a prompt;
    // ASYNC polling — see `wait_for_permission_request`).
    let mut prompted = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if let Ok((event, _)) = rx.try_recv() {
            if event == "permission-request" {
                prompted = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!prompted, "a trusted Space's gate must not prompt");

    let _ = manager.close_session(&info.session_id).await;
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

/// (8) Trusted Space (ADR 0010, guard): a TRUSTED Space's `select`
/// request (the `ask` tool's dialog) is NEVER auto-answered — the trust
/// short-circuit exists only in the `confirm` arm, so the
/// `permission-request` event STILL arrives and the turn settles only on
/// the user's answer (a `value` response). The prompt's options are the
/// request's option labels and do NOT carry the `trust-space` third
/// option (only `confirm` prompts get it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trusted_space_never_auto_answers_select_requests() {
    let config_dir = temp_config_dir();
    let (cwd, cwd_str) = temp_cwd();
    write_agents_json_pi(&config_dir, &[("FAKE_PI_SELECT", "1")], None);

    let (tx, rx) = std::sync::mpsc::channel();
    let sink: Arc<dyn EventSink> = Arc::new(TestSink { tx });

    let mut manager = SessionManager::new(config_dir.clone()).unwrap();
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    manager.attach_db(db.clone());
    // The space row exists and is TRUSTED (BEFORE the session starts —
    // the same precondition the auto-answer test sets, which is exactly
    // the state a regression would abuse to suppress this prompt).
    db.upsert_space(&cwd_str, false)
        .expect("upsert_space should succeed");
    db.set_space_trusted(&cwd_str, true)
        .expect("set_space_trusted should succeed");
    assert!(
        db.space_trusted(&cwd).expect("space_trusted should work"),
        "precondition: the space is trusted"
    );

    let info = archimedes_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("start_session should succeed");

    // The select prompt + the answer, CONCURRENTLY (`join!` — the fake
    // settles only after the client's response; see (6)).
    let sid = info.session_id.clone();
    let (reason, answer) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(15),
            manager.send_prompt(&sid, "hi".to_string()),
        ),
        async {
            // The `permission-request` event MUST arrive even though the
            // Space is trusted (a `select` request is never auto-answered
            // — ASYNC polling — see `wait_for_permission_request`).
            let (event, payload) = wait_for_permission_request(&rx, Duration::from_secs(10))
                .await
                .expect("a select request must prompt, trusted or not");
            assert_eq!(event, "permission-request");
            let request = &payload["request"];
            assert_eq!(request["sessionId"], sid);
            let options: Vec<String> = request["options"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["optionId"].as_str().unwrap().to_string())
                .collect();
            // The options are the request's option labels ("A" / "B") —
            // NOT the confirm prompt's allow/reject/trust-space set.
            assert_eq!(options, vec!["A".to_string(), "B".to_string()]);
            assert!(
                !options.iter().any(|o| o == "trust-space"),
                "a select prompt must not offer the trust-space option, got {options:?}"
            );
            let request_id = payload["requestId"].as_str().unwrap().to_string();
            // The user picks an option → a `value` response settles the
            // turn (the `Select` arm maps `Selected` to `Value`).
            let hit = manager
                .respond_permission(
                    &sid,
                    &request_id,
                    PermissionOutcome::Selected {
                        option_id: "A".to_string(),
                    },
                )
                .await
                .expect("respond_permission should succeed");
            assert!(hit, "the pending entry must be resolved");
            Ok::<(), ()>(())
        },
    );
    assert!(answer.is_ok());
    let reason = reason
        .expect("the turn must settle after the select answer")
        .expect("send_prompt should succeed");
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the turn settles after the select answer"
    );

    // The trust flag is untouched (answering a question is not a trust
    // decision — it was trusted before and stays trusted).
    assert!(
        db.space_trusted(&cwd).expect("space_trusted should work"),
        "a select answer must not alter the trust flag"
    );

    let _ = manager.close_session(&info.session_id).await;
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}
