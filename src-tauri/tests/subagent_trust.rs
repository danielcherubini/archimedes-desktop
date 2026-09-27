//! Subagent sessions inherit Space trust (ADR 0010): a subagent dispatch in
//! a TRUSTED Space gets NO `permission-request` event (the gate's
//! `confirm` is auto-confirmed and the turn settles on its own), and the
//! `trust-space` outcome from a subagent prompt PERSISTS the flag (the
//! user's trust decision is not silently discarded).
//!
//! Driven with `fake_pi`'s `FAKE_PI_GATE` mode (the fake fires the gate
//! extension's `confirm` dialog mid-turn and settles on the client's
//! answer) through the real `SubagentSessionManager::dispatch` — the same
//! mode `rpc_flow.rs` uses for the main-session trust tests
//! (`trusted_space_skips_the_permission_prompt`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use archimedes_desktop_lib::agent::subagent::{
    LaunchConfig, SubagentOutcome, SubagentSessionManager,
};
use archimedes_desktop_lib::agent::{EventSink, PermissionOutcome};
use archimedes_desktop_lib::storage::Db;
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

fn temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fresh directory + its CANONICAL display string (the `spaces` row is
/// keyed by the canonical path — `space_trusted` canonicalizes the lookup).
fn temp_cwd(prefix: &str) -> (PathBuf, String) {
    let dir = temp_dir(prefix);
    let canonical = dir.canonicalize().unwrap();
    (dir, canonical.display().to_string())
}

/// Write an `agents.json` with ONE `fake` entry: `command` = `fake_pi`,
/// env `FAKE_PI_GATE=1` (the fake fires the gate's `confirm` dialog on
/// `prompt` and settles the turn on the client's answer).
fn write_agents_gate(dir: &Path) {
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Pi",
                "command": FAKE_PI,
                "args": [],
                "env": { "FAKE_PI_GATE": "1" }
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Build the manager with the trust db threaded (the fix: `new` takes the
/// trust db — `None` keeps the pre-fix behavior: no trust lookup at all).
fn build_manager(config_dir: &Path, trust_db: Option<Arc<Db>>) -> SubagentSessionManager {
    SubagentSessionManager::new(config_dir.to_path_buf(), trust_db)
        .expect("subagent manager should build")
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

/// A config dir + a (canonicalized) cwd + a db with the space row present
/// and the trust flag in the given state.
fn trusted_fixture(trusted: bool) -> (PathBuf, PathBuf, String, Arc<Db>) {
    let config_dir = temp_dir("subagent-trust");
    write_agents_gate(&config_dir);
    let (cwd, cwd_str) = temp_cwd("subagent-trust-cwd");
    let db = Arc::new(Db::open(&config_dir.join("archimedes.db")).expect("db should open"));
    db.upsert_space(&cwd_str)
        .expect("upsert_space should succeed");
    db.set_space_trusted(&cwd_str, trusted)
        .expect("set_space_trusted should succeed");
    assert_eq!(
        db.space_trusted(&cwd).expect("space_trusted should work"),
        trusted,
        "precondition: the space is {trusted}"
    );
    (config_dir, cwd, cwd_str, db)
}

/// The no-config `LaunchConfig` (a config-less dispatch).
fn bare_launch() -> LaunchConfig {
    LaunchConfig {
        system_prompt: None,
        model: None,
        thinking: None,
        tools: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// (1) **trusted Space → no prompt**: a subagent dispatch in a TRUSTED
/// Space gets NO `permission-request` event (the gate's `confirm` is
/// auto-confirmed) — the fake settles the turn on the client's
/// `confirmed: true`, so the dispatch resolves on its own (NO
/// `respond_permission` call). Mirrors
/// `trusted_space_skips_the_permission_prompt` in `rpc_flow.rs`.
///
/// FAILS pre-fix: the subagent driver's db is `None`, so the permission
/// handler sees an untrusted Space and prompts — and the fake blocks on
/// its unanswered dialog, so the dispatch never settles (the timeout is
/// the signal).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_dispatch_in_trusted_space_does_not_prompt() {
    let (config_dir, cwd, _cwd_str, db) = trusted_fixture(true);

    let manager = build_manager(&config_dir, Some(db));
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: events.clone(),
    });

    let (outcome, _cancel) = manager.dispatch(
        "parent-sess",
        &cwd,
        "fake",
        "worker".to_string(),
        bare_launch(),
        "do the thing".to_string(),
        &sink,
    );

    // The gate is auto-confirmed (no prompt) — the fake settles the turn
    // on the client's `confirmed: true`, so the dispatch resolves on its
    // own (NO `respond_permission` call).
    let outcome = tokio::time::timeout(Duration::from_secs(15), outcome)
        .await
        .expect(
            "the subagent dispatch must settle WITHOUT a user answer — a trusted \
             Space's gate auto-confirms (a timeout means the subagent still \
             PROMPTED: its trust_db was not threaded)",
        )
        .expect("the dispatch oneshot should resolve");
    match outcome {
        SubagentOutcome::Completed { .. } => {}
        SubagentOutcome::Failed { error } => {
            panic!("the dispatch should complete in a trusted Space, got Failed: {error}")
        }
    }

    // NO `permission-request` event (ASYNC polling — the driver runs on
    // the worker runtime).
    assert!(
        !wait_for_event(&events, Duration::from_secs(2), |evs| evs
            .iter()
            .any(|(n, _)| n == "permission-request"),)
        .await,
        "a trusted Space's subagent gate must not prompt"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

/// (2) **`trust-space` outcome persists the flag**: a subagent dispatch in
/// an UNTRUSTED Space prompts; answering `trust-space` sets the trust flag
/// (the user's trust decision is NOT silently discarded) and the turn
/// settles on the answer (the fake settles on ANY response frame — the
/// `Completed` outcome does not discriminate `confirmed: true` from a
/// cancel).
///
/// FAILS pre-fix: the subagent driver's db is `None`, so the `trust-space`
/// arm's `if let Some(d) = &db` skips the write — the flag stays unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_trust_space_outcome_persists_the_flag() {
    let (config_dir, cwd, _cwd_str, db) = trusted_fixture(false);

    let manager = build_manager(&config_dir, Some(db.clone()));
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: events.clone(),
    });

    let (outcome, _cancel) = manager.dispatch(
        "parent-sess",
        &cwd,
        "fake",
        "worker".to_string(),
        bare_launch(),
        "do the thing".to_string(),
        &sink,
    );

    // The untrusted Space PROMPTS (the `permission-request` event carries
    // the subagent's session id + the request id).
    let (session_id, request_id) = {
        assert!(
            wait_for_event(&events, Duration::from_secs(15), |evs| evs
                .iter()
                .any(|(n, _)| n == "permission-request"),)
            .await,
            "an untrusted Space's subagent gate must prompt"
        );
        let guard = events.lock().unwrap();
        let (_, p) = guard
            .iter()
            .find(|(n, _)| n == "permission-request")
            .expect("the permission-request event");
        (
            p["sessionId"].as_str().expect("sessionId").to_string(),
            p["requestId"].as_str().expect("requestId").to_string(),
        )
    };

    // The user picks `trust-space` → the flag is set (the response frame
    // itself is not discriminated — the fake settles on any answer).
    assert!(
        manager
            .respond_permission(
                &session_id,
                &request_id,
                PermissionOutcome::Selected {
                    option_id: "trust-space".to_string(),
                },
            )
            .await,
        "respond_permission should resolve the subagent's prompt"
    );

    // The flag is PERSISTED (the write is best-effort in the waiter task —
    // poll until it lands).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if db.space_trusted(&cwd).expect("space_trusted should work") {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "the trust-space outcome must persist the flag for the subagent's \
                 Space (a missing write means the subagent's trust_db was not \
                 threaded — the decision was silently discarded)"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The answer settles the turn (the fake's `FAKE_PI_GATE` mode
    // emits its turn on ANY response frame).
    let outcome = tokio::time::timeout(Duration::from_secs(15), outcome)
        .await
        .expect("the subagent dispatch must settle after the trust-space answer")
        .expect("the dispatch oneshot should resolve");
    match outcome {
        SubagentOutcome::Completed { .. } => {}
        SubagentOutcome::Failed { error } => {
            panic!("the dispatch should complete after the trust-space answer, got Failed: {error}")
        }
    }

    // EPHEMERAL contract: the subagent's dispatch writes ZERO transcript
    // rows — the `messages` table has NO rows for the subagent's session
    // id (query the tempdir db directly — the `db`/`trust_db` field
    // separation keeps the subagent's driver from ever writing the
    // transcript, but this pins it).
    let rows = db
        .messages_for(&session_id)
        .expect("messages_for should work");
    assert!(
        rows.is_empty(),
        "the subagent's dispatch must write no transcript rows, got {} row(s)",
        rows.len()
    );

    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

/// (3) **`None` trust db keeps today's behavior** (fail-closed): with no
/// trust db threaded, a subagent dispatch prompts (the pre-fix flow) —
/// the `None` case is untrusted, and the `trust-space` outcome is a
/// silent no-op for the flag (the `if let Some(d)` skip) — pinned by
/// test (4) below.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_without_trust_db_prompts_as_before() {
    let (config_dir, cwd, _cwd_str, db) = trusted_fixture(true);

    let manager = build_manager(&config_dir, None);
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: events.clone(),
    });

    let (outcome, _cancel) = manager.dispatch(
        "parent-sess",
        &cwd,
        "fake",
        "worker".to_string(),
        bare_launch(),
        "do the thing".to_string(),
        &sink,
    );

    // NO trust db → the gate PROMPTS (fail-closed), even though the space
    // row is trusted in the (unthreaded) db.
    let (session_id, request_id) = {
        assert!(
            wait_for_event(&events, Duration::from_secs(15), |evs| evs
                .iter()
                .any(|(n, _)| n == "permission-request"),)
            .await,
            "a subagent without a trust db must prompt (fail-closed)"
        );
        let guard = events.lock().unwrap();
        let (_, p) = guard
            .iter()
            .find(|(n, _)| n == "permission-request")
            .expect("the permission-request event");
        (
            p["sessionId"].as_str().expect("sessionId").to_string(),
            p["requestId"].as_str().expect("requestId").to_string(),
        )
    };

    // The answer settles the turn (the fake settles on any response
    // frame); the flag is untouched (the `None` skip).
    assert!(
        manager
            .respond_permission(
                &session_id,
                &request_id,
                PermissionOutcome::Selected {
                    option_id: "allow".to_string(),
                },
            )
            .await
    );
    let outcome = tokio::time::timeout(Duration::from_secs(15), outcome)
        .await
        .expect("the subagent dispatch must settle after the allow answer")
        .expect("the dispatch oneshot should resolve");
    match outcome {
        SubagentOutcome::Completed { .. } => {}
        SubagentOutcome::Failed { error } => panic!("got Failed: {error}"),
    }
    assert!(
        db.space_trusted(&cwd).expect("space_trusted should work"),
        "the `allow` answer must not touch the trust flag (it was trusted before)"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

/// (4) **`None` trust db: `trust-space` is a silent no-op** (pins test 3's
/// doc claim): with no trust db threaded, answering `trust-space` sets NO
/// flag — the flag is UNCHANGED (still untrusted) and the turn still
/// settles on the answer (the fake settles on ANY response frame — the
/// `Completed` outcome does not discriminate `confirmed: true` from a
/// cancel; the `if let Some(d)` skip).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_trust_space_without_trust_db_is_a_noop() {
    let (config_dir, cwd, _cwd_str, db) = trusted_fixture(false);

    let manager = build_manager(&config_dir, None);
    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: events.clone(),
    });

    let (outcome, _cancel) = manager.dispatch(
        "parent-sess",
        &cwd,
        "fake",
        "worker".to_string(),
        bare_launch(),
        "do the thing".to_string(),
        &sink,
    );

    // NO trust db → the gate PROMPTS (fail-closed, even though the space
    // row is untrusted in the (unthreaded) db — same prompt either way).
    let (session_id, request_id) = {
        assert!(
            wait_for_event(&events, Duration::from_secs(15), |evs| evs
                .iter()
                .any(|(n, _)| n == "permission-request"),)
            .await,
            "a subagent without a trust db must prompt (fail-closed)"
        );
        let guard = events.lock().unwrap();
        let (_, p) = guard
            .iter()
            .find(|(n, _)| n == "permission-request")
            .expect("the permission-request event");
        (
            p["sessionId"].as_str().expect("sessionId").to_string(),
            p["requestId"].as_str().expect("requestId").to_string(),
        )
    };

    // `trust-space` with no trust db → the flag write is SKIPPED (silent
    // no-op); the turn settles (the fake settles on any response frame).
    assert!(
        manager
            .respond_permission(
                &session_id,
                &request_id,
                PermissionOutcome::Selected {
                    option_id: "trust-space".to_string(),
                },
            )
            .await,
        "respond_permission should resolve the subagent's prompt"
    );
    let outcome = tokio::time::timeout(Duration::from_secs(15), outcome)
        .await
        .expect("the subagent dispatch must settle after the trust-space answer")
        .expect("the dispatch oneshot should resolve");
    match outcome {
        SubagentOutcome::Completed { .. } => {}
        SubagentOutcome::Failed { error } => panic!("got Failed: {error}"),
    }

    // The flag is UNCHANGED (still untrusted — the `None` trust_db write
    // was a silent no-op).
    assert!(
        !db.space_trusted(&cwd).expect("space_trusted should work"),
        "a `trust-space` answer without a trust db must be a silent no-op for the flag"
    );

    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}
