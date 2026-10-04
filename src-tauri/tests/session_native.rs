//! The native session e2e (ADR 0025 Task 4): `start_session` spawns a
//! WORKER (the `AgentLoop` runs in the Worker process — the
//! `set_provider_factory` in-process mock `Provider` CANNOT survive, the
//! provider lives in the Worker process; the rewrite drives via the
//! `WorkerFactory` seam with the `fake_worker` fixture — the canned
//! `SinkFrame` / store-frame stream) → `send_prompt` → the
//! `session-update` `SinkFrame`s flow (the frozen shapes, re-emitted
//! verbatim) + the store frames are persisted (the `TranscriptPersister`)
//! → `resume_session` (the native branch — the `native_messages` re-read)
//! → the resumed session operates normally.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use archimedes_lib::agent::harness::{CompactionConfig, Model, ModelCatalog};
use archimedes_lib::agent::worker::client::WorkerHandle;
use archimedes_lib::agent::worker::manager::{WorkerFactory, WorkerManager};
use archimedes_lib::agent::{EventSink, SessionInfo, SessionManager, StopReason};
use archimedes_lib::storage::Db;
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ── the `fake_worker` fixture factory (the `WorkerFactory` seam) ──────────

/// The `fake_worker` fixture (the `WorkerFactory` seam — `cargo test`
/// builds the bin targets; the fixture speaks the Worker protocol: the
/// canned `SinkFrame` / store-frame stream, the `ready` handshake).
struct FixtureFactory;

impl WorkerFactory for FixtureFactory {
    fn spawn(&self) -> Result<WorkerHandle, archimedes_lib::agent::worker::client::WorkerError> {
        WorkerHandle::spawn(&fixture_path())
    }
}

/// The `fake_worker` fixture path (the `CARGO_MANIFEST_DIR`/
/// `target/debug` convention — `cargo test` builds the bin targets).
fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/target/debug/fake_worker"
    ))
}

// ── a recording `EventSink` ──────────────────────────────────────────────

struct RecSink {
    tx: mpsc::UnboundedSender<(String, Value)>,
}

impl EventSink for RecSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}

// ── the fixtures ──────────────────────────────────────────────────────────

/// A temp config dir (the `settings.json` home — the seeded settings give
/// the session start a `defaultThinkingLevel` — the settings-driven rung).
fn temp_config_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("session-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        json!({ "theme": "dark", "defaultThinkingLevel": "high" }).to_string(),
    )
    .unwrap();
    dir
}

/// A full `Model` literal (the `base_url` EMPTY so
/// `refresh_model_metadata` is a no-op — no network;
/// `openai-completions` so the model is selectable).
fn test_model(id: &str) -> Model {
    Model {
        id: id.to_string(),
        provider: "fake".to_string(),
        base_url: String::new(),
        api_key: "k".to_string(),
        context_window: 128000,
        cost_per_mtok_in: 0.0,
        cost_per_mtok_out: 0.0,
        supports_tools: true,
        supports_thinking: true,
        thinking_levels: vec!["low".to_string(), "high".to_string()],
        api: Some("openai-completions".to_string()),
    }
}

/// A known catalog (the `set_catalog` seam — the base catalog is empty
/// in a test, so the effective catalog is this one): two
/// OpenAI-compatible models, the default `fake/m1`.
fn test_catalog() -> ModelCatalog {
    ModelCatalog {
        models: vec![test_model("m1"), test_model("m2")],
        default_model: Some("fake/m1".to_string()),
        compaction: CompactionConfig::default(),
    }
}

/// The e2e manager (ADR 0025 — the Worker-mediated path): `set_catalog`
/// (a known catalog) + a `WorkerManager` with the `fake_worker` factory
/// (the LATE-WIRE — the `SessionManager` ↔ `WorkerManager` construction
/// cycle: the `WorkerManager`'s `on_crash` / `on_event` callbacks capture
/// the `Arc<SessionManager>`; the `attach_*` methods are `&self`, so the
/// `OnceLock` `set` needs no exclusive access).
fn build_manager(
    dir: &Path,
    sink: &Arc<dyn EventSink>,
) -> (Arc<SessionManager>, Arc<WorkerManager>, Arc<Db>) {
    let db = Arc::new(Db::open(&dir.join("archimedes.db")).unwrap());
    let mut manager = SessionManager::new(dir.to_path_buf());
    manager.attach_db(db.clone());
    manager.set_catalog(test_catalog());
    manager.set_sink(sink.clone());
    let manager = Arc::new(manager);
    let wm = {
        let m = manager.clone();
        let m2 = manager.clone();
        Arc::new(WorkerManager::new(
            Arc::new(FixtureFactory),
            Arc::new(move |s, c| m.handle_crash(&s, c)),
            Arc::new(move |s, b, e| m2.route_event(&s, b, e)),
            Arc::new(|_s| None),
        ))
    };
    manager.attach_worker_manager(wm.clone());
    (manager, wm, db)
}

/// Wait for a `session-update` frame with the given `sessionUpdate` kind.
async fn wait_for_update(rx: &mut mpsc::UnboundedReceiver<(String, Value)>, kind: &str) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("timeout waiting for a {kind} frame");
        }
        let (event, payload) = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("the sink channel stayed open")
            .expect("a frame arrived");
        if event == "session-update" && payload["update"]["sessionUpdate"] == kind {
            return payload["update"].clone();
        }
    }
}

/// Drain the sink channel (clear pending frames — a prior turn's leftover
/// `agent_message_chunk` must not be confused with the next turn's frames).
/// The `send_prompt` resolved on `agent_settled` (the LAST frame the
/// `fake_worker` emits), so the turn's frames are all in the channel by
/// the time the drain runs — the short settle window covers the delivery
/// gap (the frames are emitted in order, `agent_settled` last).
async fn drain(rx: &mut mpsc::UnboundedReceiver<(String, Value)>) {
    tokio::time::sleep(Duration::from_millis(100)).await;
    while rx.try_recv().is_ok() {}
}

/// (a) The full native-session e2e (ADR 0025 — the Worker-mediated path):
/// start (the `AgentLoop` runs in the Worker — NO in-process loop) → the
/// capability envelope has NO `piSessionFile` + the synthesized
/// `config_options` (model + thought_level) are present → `send_prompt` →
/// the `session-update` `SinkFrame`s flow (re-emitted verbatim) + the
/// store frames are persisted (the `native_messages` transcript + the
/// `messages` display rows) → `resume_session` (the native branch — NOT
/// `NotResumable` — the `native_messages` re-read) → the resumed session
/// operates normally (a fresh `fake_worker` + re-hydrate).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_session_end_to_end() {
    let dir = temp_config_dir();
    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    let (manager, _wm, _db) = build_manager(&dir, &sink);

    // The base catalog is empty in a test → the model resolves from the
    // `set_catalog` catalog's default (`fake/m1`).
    let info = manager
        .start_session(dir.clone(), &sink)
        .await
        .expect("the native session starts (a Worker spawned — no in-process loop)");
    assert!(
        info.session_id.starts_with("arch_"),
        "a fresh native session ID starts with arch_, got {}",
        info.session_id
    );
    let sid = info.session_id.clone();

    // (1) The capability envelope: NO `piSessionFile` (resume is from the
    // `native_messages` table) + `loadSession: true` (the Resume button) +
    // the resolved model.
    assert!(
        info.capabilities.get("piSessionFile").is_none(),
        "a native session has no pi session file, got {:?}",
        info.capabilities
    );
    assert_eq!(info.capabilities["loadSession"], true);
    assert_eq!(info.capabilities["model"], "fake/m1");

    // (2) The synthesized `config_options` (the existing shape — the
    // frontend is unchanged): a model selector (the `selectable()`
    // ids `"<provider>/<id>"`) + a thought_level selector (the model's
    // `thinking_levels`; the built-in's default `high`).
    let options = info
        .config_options
        .expect("the native session synthesizes config options");
    let model_opt = options
        .iter()
        .find(|o| o["id"] == "model")
        .expect("a model selector");
    assert_eq!(model_opt["currentValue"], "fake/m1");
    let model_values: Vec<String> = model_opt["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(model_values, vec!["fake/m1", "fake/m2"]);
    let thought_opt = options
        .iter()
        .find(|o| o["id"] == "thought_level")
        .expect("a thought_level selector");
    assert_eq!(
        thought_opt["currentValue"], "high",
        "the built-in's default thinking level"
    );
    let thought_values: Vec<String> = thought_opt["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(thought_values, vec!["low", "high"]);

    // (3) The turn: `send_prompt` → the `session-update` `SinkFrame`s flow
    // (re-emitted VERBATIM — the SAME frozen shapes) + `EndTurn` (the
    // `agent_settled` settle).
    let reason = manager
        .send_prompt(&sid, "hi".to_string())
        .await
        .expect("the native turn resolves");
    assert_eq!(reason, StopReason::EndTurn);
    let chunk = wait_for_update(&mut sink_rx, "agent_message_chunk").await;
    assert_eq!(
        chunk["content"]["text"], "canned ",
        "the first canned chunk"
    );

    // (4) The transcript (the `native_messages` table — the store frames
    // applied by the `TranscriptPersister`): the user + assistant rows
    // (the `fake_worker`'s canned `TranscriptInsert` frames) + the display
    // row (the `DisplayUpsert` frame — the `messages` table).
    let db = Db::open(&dir.join("archimedes.db")).unwrap();
    let rows = db.load_native_messages(&sid).unwrap();
    let roles: Vec<Value> = rows
        .iter()
        .map(|c| serde_json::from_str(c).unwrap())
        .collect();
    assert!(
        roles.iter().any(|v| v["role"] == "user"),
        "the user transcript row is persisted, got {roles:?}"
    );
    assert!(
        roles.iter().any(|v| v["role"] == "assistant"),
        "the assistant transcript row is persisted, got {roles:?}"
    );
    let msgs = db.messages_for(&sid).unwrap();
    assert!(
        msgs.iter().any(|m| m.kind == "agent-text"),
        "the display row is persisted, got {msgs:?}"
    );

    // (5) Close + RESUME (the native branch — the `native_messages`
    // re-read: NOT `NotResumable`).
    manager.close_session(&sid).await.expect("close works");
    let resumed = manager
        .resume_session(&sid, dir.clone(), &sink)
        .await
        .expect("a native session resumes from the native_messages table (not NotResumable)");
    assert_eq!(resumed.session_id, sid);
    assert!(
        resumed.capabilities.get("piSessionFile").is_none(),
        "the resumed native session has no pi session file"
    );

    // (6) The resumed session operates normally (a fresh `fake_worker` +
    // re-hydrate — the `StartEnv` carries the re-read transcript): the
    // turn's `agent_message_chunk` frame flows. The channel is DRAINED
    // first (turn 1's leftover `answer` chunk — the `send_prompt` resolved
    // on `agent_settled`, the LAST frame, so the leftover is in the
    // channel by now) so the fresh `canned ` chunk is the first the wait
    // finds.
    drain(&mut sink_rx).await;
    let reason = manager
        .send_prompt(&sid, "again".to_string())
        .await
        .expect("the resumed turn resolves");
    assert_eq!(reason, StopReason::EndTurn);
    let resumed_chunk = wait_for_update(&mut sink_rx, "agent_message_chunk").await;
    assert_eq!(
        resumed_chunk["content"]["text"], "canned ",
        "the resumed turn's first canned chunk"
    );

    let _ = manager.close_session(&sid).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A legacy session whose stored ID is a bare UUID (no `arch_` prefix)
/// retains its exact ID when resumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resuming_legacy_bare_uuid_session_preserves_id() {
    let dir = temp_config_dir();
    let (sink_tx, mut sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    let (manager, _wm, _db) = build_manager(&dir, &sink);

    let legacy_id = uuid::Uuid::new_v4().to_string();
    assert!(
        !legacy_id.starts_with("arch_"),
        "legacy id has no arch_ prefix: {legacy_id}"
    );

    // Seed the DB with a legacy session record (the stored `model` —
    // `fake/m1` — resolves against the catalog) + a system prompt in
    // `native_messages` (the resume's re-read).
    let db = Db::open(&dir.join("archimedes.db")).unwrap();
    let info = SessionInfo {
        session_id: legacy_id.clone(),
        cwd: dir.clone(),
        capabilities: json!({
            "model": "fake/m1",
            "loadSession": true,
        }),
        config_options: None,
        archived: false,
        context_usage: None,
        is_subagent: false,
    };
    db.record_session(&info).unwrap();
    db.insert_native_message(
        &legacy_id,
        0,
        "system",
        r#"{"role":"system","content":"legacy system prompt"}"#,
    )
    .unwrap();

    let resumed = manager
        .resume_session(&legacy_id, dir.clone(), &sink)
        .await
        .expect("legacy session resumes");
    assert_eq!(
        resumed.session_id, legacy_id,
        "the resumed session preserves its exact legacy bare-UUID ID"
    );

    // The resumed session operates normally under the legacy ID (a fresh
    // `fake_worker` + re-hydrate — the `native_messages` re-read).
    let reason = manager
        .send_prompt(&legacy_id, "hello legacy".to_string())
        .await
        .expect("turn completes");
    assert_eq!(reason, StopReason::EndTurn);
    let chunk = wait_for_update(&mut sink_rx, "agent_message_chunk").await;
    assert_eq!(chunk["content"]["text"], "canned ");

    let _ = manager.close_session(&legacy_id).await;
    let _ = std::fs::remove_dir_all(&dir);
}
