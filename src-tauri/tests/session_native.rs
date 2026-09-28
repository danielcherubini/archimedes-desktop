//! The native session e2e (native-agent-harness Task 7): a `kind: native`
//! registry entry (the built-in `archimedes` merged into the loaded
//! registry) → `start_session` spawns an in-process `AgentLoop` (NOT a
//! subprocess — the provider comes from the `set_provider_factory` injection
//! seam, so no real model call is made) → `send_prompt` → the normalized
//! `session-update` events flow (the existing frozen shapes) → `resume_session`
//! (the native branch — BEFORE the `piSessionFile` check: a native session's
//! `capabilities_json` has no `piSessionFile`) → `load_messages` (resume)
//! works.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use archimedes_lib::agent::harness::{
    ChatMessage, ChatRole, CompactionConfig, FinishReason, MessageContent, Model, ModelCatalog,
    Provider, ProviderError, ProviderEvent,
};
use archimedes_lib::agent::{EventSink, SessionManager, StopReason};
use archimedes_lib::config::{AgentKind, Registry};
use archimedes_lib::storage::Db;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ── the mock `Provider` (canned responses + request recording) ──────────

enum MockResponse {
    Stream(Vec<ProviderEvent>),
    Error(ProviderError),
}

impl MockResponse {
    fn text_then_done(delta: &str) -> Self {
        Self::Stream(vec![
            ProviderEvent::TextDelta(delta.to_string()),
            ProviderEvent::Done(FinishReason::Stop),
        ])
    }
}

// The mock provider's shared state (type aliases — `clippy::type_complexity`).
type ResponseQueue = Arc<StdMutex<VecDeque<MockResponse>>>;
type RecordedTurns = Arc<StdMutex<Vec<Vec<(String, String)>>>>;

/// A `Provider` returning canned responses in order (a `complete()` beyond
/// the queue is a `Fatal` error — a test bug) that ALSO records each
/// request's messages (the resume assertion: the resumed loop's first model
/// request carries the `load_messages` transcript).
struct MockProvider {
    responses: ResponseQueue,
    recorded: RecordedTurns,
}

impl MockProvider {
    fn new(responses: ResponseQueue, recorded: RecordedTurns) -> Self {
        Self {
            responses,
            recorded,
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        req: &archimedes_lib::agent::harness::ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        // Record the request's messages (role + text) — the resume assertion.
        let recorded_msgs: Vec<(String, String)> = req
            .messages
            .iter()
            .map(|m| (role_str(m.role), text_of(m)))
            .collect();
        self.recorded.lock().unwrap().push(recorded_msgs);
        let next = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockResponse::Error(ProviderError::Fatal(
                "mock: no canned response left".into(),
            )));
        match next {
            MockResponse::Stream(events) => Ok(futures_util::stream::iter(events).boxed()),
            MockResponse::Error(e) => Err(e),
        }
    }
}

fn role_str(role: ChatRole) -> String {
    match role {
        ChatRole::System => "system".to_string(),
        ChatRole::User => "user".to_string(),
        ChatRole::Assistant => "assistant".to_string(),
        ChatRole::Tool => "tool".to_string(),
    }
}

fn text_of(m: &ChatMessage) -> String {
    match &m.content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(_) => String::new(),
    }
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

fn test_model(id: &str) -> Model {
    Model {
        id: id.to_string(),
        provider: "test".to_string(),
        base_url: "http://localhost/v1".to_string(),
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

/// A temp config dir with an `agents.json` containing `pi` ONLY (the
/// pre-Task-7 shape — the built-in `archimedes` native entry is merged in
/// by `Registry::load`).
fn temp_config_dir_pi_only() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("session-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agents.json"),
        json!({
            "agents": [{
                "id": "pi",
                "name": "Pi",
                "command": "pi",
                "args": ["--mode", "rpc"],
                "env": {},
                "bridge": true,
            }]
        })
        .to_string(),
    )
    .unwrap();
    dir
}

/// A known catalog (the `set_catalog` seam — `SessionManager::new` seeds
/// from the user's real pi config, which a test cannot control): two
/// OpenAI-compatible models, the default `test/m1`.
fn test_catalog() -> ModelCatalog {
    ModelCatalog {
        models: vec![test_model("m1"), test_model("m2")],
        default_model: Some("test/m1".to_string()),
        compaction: CompactionConfig::default(),
    }
}

/// The e2e manager: `set_provider_factory` (the injection seam — BEFORE
/// `start_session`) + `set_catalog` (a known catalog).
async fn build_manager(
    dir: &Path,
) -> (
    SessionManager,
    RecordedTurns,
    Arc<dyn EventSink>,
    mpsc::UnboundedReceiver<(String, Value)>,
) {
    let db = Arc::new(Db::open(&dir.join("archimedes.db")).unwrap());
    let mut manager = SessionManager::new(dir.to_path_buf()).unwrap();
    manager.attach_db(db);

    // The injection seams (BEFORE `start_session`): a mock `Provider`
    // factory (no real model call) + a known catalog.
    let responses = Arc::new(StdMutex::new(VecDeque::new()));
    let recorded: RecordedTurns = Arc::new(StdMutex::new(Vec::new()));
    let responses_factory = Arc::clone(&responses);
    let recorded_factory = Arc::clone(&recorded);
    manager.set_provider_factory(move |_m: &Model| {
        Box::new(MockProvider::new(
            Arc::clone(&responses_factory),
            Arc::clone(&recorded_factory),
        )) as Box<dyn Provider>
    });
    manager.set_catalog(test_catalog());

    // The mock's canned responses: session 1's prompt → "Hello"; session 2
    // (the resume)'s prompt → "Resumed".
    *responses.lock().unwrap() = VecDeque::from(vec![
        MockResponse::text_then_done("Hello"),
        MockResponse::text_then_done("Resumed"),
    ]);

    let (sink_tx, sink_rx) = mpsc::unbounded_channel();
    let sink: Arc<dyn EventSink> = Arc::new(RecSink { tx: sink_tx });
    (manager, recorded, sink, sink_rx)
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

/// (a) The full native-session e2e: start (in-process `AgentLoop`, no
/// subprocess) → the capability envelope has NO `piSessionFile` + the
/// synthesized `config_options` (model + thought_level) are present →
/// `send_prompt` → the normalized `session-update` frames flow → the
/// `native_messages` transcript is persisted → `resume_session` (the native
/// branch — NOT `NotResumable`) → `load_messages` (the resumed loop's first
/// model request carries the loaded transcript).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_session_end_to_end() {
    let dir = temp_config_dir_pi_only();

    // The registry merge: the built-in `archimedes` native entry is APPENDED
    // after the user's `pi` entry (the default remains `pi`).
    {
        let registry = Registry::load(&dir).unwrap();
        assert_eq!(registry.agents[0].id, "pi", "pi stays the default");
        let native = registry.get("archimedes").unwrap();
        assert_eq!(native.kind, AgentKind::Native);
        assert!(native.command.is_empty());
        assert!(native.harness.is_some());
    }

    let (manager, recorded, sink, mut sink_rx) = build_manager(&dir).await;

    // The built-in's `harness.default_model` is `None` → resolved from the
    // catalog's default (`test/m1`).
    let info = manager
        .start_session("archimedes", dir.clone(), &sink)
        .await
        .expect("the native session starts (in-process, no subprocess)");
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
    assert_eq!(info.capabilities["model"], "test/m1");

    // (2) The synthesized `config_options` (the existing shape — the
    // frontend is unchanged): a model selector (the `openai_compatible()`
    // ids `"<provider>/<id>"`) + a thought_level selector (the model's
    // `thinking_levels`; the built-in's default `high`).
    let options = info
        .config_options
        .expect("the native session synthesizes config options");
    let model_opt = options
        .iter()
        .find(|o| o["id"] == "model")
        .expect("a model selector");
    assert_eq!(model_opt["currentValue"], "test/m1");
    let model_values: Vec<String> = model_opt["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(model_values, vec!["test/m1", "test/m2"]);
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

    // (3) The turn: `send_prompt` → the normalized `session-update` frames
    // flow (the SAME frozen shapes an external session emits) + `EndTurn`.
    let reason = manager
        .send_prompt(&sid, "hi".to_string())
        .await
        .expect("the native turn resolves");
    assert_eq!(reason, StopReason::EndTurn);
    let chunk = wait_for_update(&mut sink_rx, "agent_message_chunk").await;
    assert_eq!(chunk["content"]["text"], "Hello");

    // (4) The provider transcript (the `native_messages` table): user +
    // assistant.
    let db = Db::open(&dir.join("archimedes.db")).unwrap();
    let rows = db.load_native_messages(&sid).unwrap();
    assert_eq!(rows.len(), 2, "user + assistant persisted");
    let user_msg: ChatMessage = serde_json::from_str(&rows[0]).unwrap();
    assert_eq!(
        user_msg,
        ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text("hi".to_string()),
            tool_call_id: None,
            tool_calls: None,
        }
    );

    // (5) Close + RESUME (the native branch — BEFORE the `piSessionFile`
    // check: the stored `capabilities_json` has no `piSessionFile`, so the
    // external path would be `NotResumable`).
    manager.close_session(&sid).await.expect("close works");
    let resumed = manager
        .resume_session("archimedes", &sid, dir.clone(), &sink)
        .await
        .expect("a native session resumes from the native_messages table (not NotResumable)");
    assert_eq!(resumed.session_id, sid);
    assert!(
        resumed.capabilities.get("piSessionFile").is_none(),
        "the resumed native session has no pi session file"
    );

    // (6) `load_messages` (resume) works: the resumed loop's FIRST model
    // request carries the loaded transcript (user "hi" + assistant "Hello")
    // BEFORE the new prompt's user message.
    let reason = manager
        .send_prompt(&sid, "again".to_string())
        .await
        .expect("the resumed turn resolves");
    assert_eq!(reason, StopReason::EndTurn);
    let resumed_chunk = wait_for_update(&mut sink_rx, "agent_message_chunk").await;
    assert_eq!(resumed_chunk["content"]["text"], "Resumed");

    // The resumed loop's model request (cloned — the `MutexGuard` is NOT
    // held across the assertions below).
    let req: Vec<(String, String)> = {
        let guard = recorded.lock().unwrap();
        guard
            .last()
            .cloned()
            .expect("the resumed loop made a model call")
    };
    let loaded: Vec<&(String, String)> = req.iter().collect();
    // The loaded transcript (2 messages) precedes the new prompt.
    assert!(
        loaded.len() >= 3,
        "the request carries the loaded transcript + the new prompt, got {loaded:?}"
    );
    let user_hi = loaded.iter().position(|(r, t)| r == "user" && t == "hi");
    let assistant_hello = loaded
        .iter()
        .position(|(r, t)| r == "assistant" && t == "Hello");
    assert!(user_hi.is_some(), "the loaded user message, got {loaded:?}");
    assert!(
        assistant_hello.is_some(),
        "the loaded assistant message, got {loaded:?}"
    );
    let user_hi = user_hi.unwrap();
    let assistant_hello = assistant_hello.unwrap();
    assert!(
        user_hi < assistant_hello,
        "the transcript order is preserved"
    );

    let _ = manager.close_session(&sid).await;
    let _ = std::fs::remove_dir_all(&dir);
}
