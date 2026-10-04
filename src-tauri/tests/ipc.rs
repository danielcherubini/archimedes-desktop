//! The headless command-surface test (ADR 0025 Task 4): the Tauri command
//! surface driven through the `tauri::test` mock runtime + the
//! `setup_dirs` seam (the injectable `WorkerFactory` — the `fake_worker`
//! fixture; the `test_support` pattern).
//!
//! The UI contract is the SINK frames: the `SinkFrame`s are re-emitted
//! VERBATIM on the Tauri sink (the frontend is unchanged); the raw
//! `RpcEvent` stream is bookkeeping-only (NOT forwarded — a forward would
//! DOUBLE-DELIVER every `session-update`); the store frames are persisted
//! to `messages` / `native_messages` (the `TranscriptPersister`).

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::Listener;
use tauri::Manager;

use archimedes_lib::agent::worker::client::WorkerHandle;
use archimedes_lib::agent::worker::manager::{WorkerFactory, WorkerManager};
use archimedes_lib::agent::SessionInfo;
use archimedes_lib::setup_dirs_with_factory;
use archimedes_lib::storage::Db;

/// The `fake_worker` fixture factory (the `WorkerFactory` seam —
/// `cargo test` builds the bin targets; the fixture speaks the Worker
/// protocol: the canned event + store-frame + `SinkFrame` stream, the
/// `__crash__` exit 137, the `__permission__` round-trip, the `__slow__`
/// 10 s settle, the `__interactive__` `interactive-event` frame, and the
/// `config` echo).
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

/// A temp config + data dir pair (the `settings.json` home + the
/// `archimedes.db` home — the `setup_dirs` seam's dirs).
#[derive(Clone)]
struct TestDirs {
    config_dir: std::path::PathBuf,
    app_data_dir: std::path::PathBuf,
}

impl TestDirs {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("ipc-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        // The `cwd` (the Space) — it must EXIST (the `spaces` table is
        // keyed by the canonical path).
        let cwd = base.join("space");
        std::fs::create_dir_all(&cwd).unwrap();
        Self {
            config_dir: base.clone(),
            app_data_dir: base,
        }
    }

    /// The `cwd` (the Space — the `start_session` / `set_space_trusted`
    /// target).
    fn cwd(&self) -> std::path::PathBuf {
        self.config_dir.join("space")
    }
}

/// The Tauri app (the `tauri::test` mock runtime + the `setup_dirs` seam
/// with the `fake_worker` factory) + the webview + the `run` thread.
///
/// The mock runtime's `run` BLOCKS (its `Ready` event triggers the
/// `setup` — the managed state; the `WebviewWindow`'s `close` ends it —
/// the single window + webview share id 0, so the `CloseWindow` message
/// empties the window map → the `ExitRequested` → the loop breaks). The
/// `run` runs on a background thread; the test invokes commands on the
/// test thread (the `get_ipc_response` — the command runs on Tauri's
/// thread pool, independent of the `run` loop). `Drop` closes the
/// webview + joins the thread (a non-daemon thread would block the test
/// binary at exit).
struct TestApp {
    handle: tauri::AppHandle<tauri::test::MockRuntime>,
    webview: tauri::WebviewWindow<tauri::test::MockRuntime>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn build_app(dirs: TestDirs) -> TestApp {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            archimedes_lib::commands::sessions::start_session,
            archimedes_lib::commands::sessions::send_prompt,
            archimedes_lib::commands::sessions::close_session,
            archimedes_lib::commands::sessions::respond_permission,
            archimedes_lib::commands::sessions::respond_interactive_request,
            archimedes_lib::commands::sessions::resume_session,
            archimedes_lib::commands::sessions::set_session_config_option,
            archimedes_lib::commands::sessions::cancel_session,
            archimedes_lib::commands::sessions::get_stalled_info,
            archimedes_lib::commands::spaces::set_space_trusted,
        ])
        .setup(move |app| {
            setup_dirs_with_factory(
                app,
                dirs.config_dir.clone(),
                dirs.app_data_dir.clone(),
                Some(Arc::new(FixtureFactory)),
            )
        })
        .build(tauri::generate_context!())
        .expect("the app should build");
    let handle = app.handle().clone();
    let thread = std::thread::spawn(move || {
        app.run(|_h, _e| {});
    });
    // Wait for the `setup` to complete (the managed state — bounded, so a
    // missing setup is a test failure, not a hang). Tauri's `setup` also
    // creates the config's default window (the `main` webview — the
    // `tauri.conf.json` window) BEFORE the setup closure runs, so by the
    // time the state is managed it exists.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if handle
            .try_state::<Arc<archimedes_lib::agent::SessionManager>>()
            .is_some()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        handle
            .try_state::<Arc<archimedes_lib::agent::SessionManager>>()
            .is_some(),
        "the setup completed (the SessionManager is managed state)"
    );
    let webview = handle
        .get_webview_window("main")
        .expect("the config window exists after the setup");
    TestApp {
        handle,
        webview,
        thread: Some(thread),
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        // Close the webview (the `CloseWindow` message empties the window
        // map → the `run` loop breaks) + join the thread.
        let _ = self.webview.close();
        let _ = self.thread.take().unwrap().join();
    }
}

/// Invoke a command (the `get_ipc_response` — the command's Ok payload as
/// a `Value`; the Err payload is the `Err` arm).
///
/// ASYNC + `spawn_blocking`: the `get_ipc_response` BLOCKS the calling
/// thread, and the `#[tokio::test]` runtime is SINGLE-THREADED (the
/// `tokio` features lack `rt-multi-thread`) — a direct call would starve
/// the `raw_json_server` task (the model discovery's target — the
/// `start_session`'s `effective_catalog` `GET /v1/models` would time out
/// → "no models available"). A `spawn_blocking` thread leaves the
/// worker free to poll the server task.
async fn invoke_raw(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: Value,
) -> Result<Value, Value> {
    let webview = webview.clone();
    let cmd = cmd.to_string();
    tokio::task::spawn_blocking(move || {
        get_ipc_response(
            &webview,
            InvokeRequest {
                cmd,
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
    })
    .await
    .expect("the invoke task")
    .map(|b| b.deserialize::<Value>().expect("the payload is JSON"))
}

/// Invoke a command (the Ok arm — a `Value`; the Err arm is a test
/// failure with the error payload in the message).
async fn invoke_ok(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: Value,
) -> Value {
    invoke_raw(webview, cmd, body)
        .await
        .unwrap_or_else(|e| panic!("the command `{cmd}` must succeed, got error: {e}"))
}

/// Deserialize a command payload (the Ok arm's `Value` → the typed
/// shape).
fn value_to<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the payload deserializes")
}

/// The event collector (the `listen` on the mock runtime — the
/// `TauriSink`'s `AppHandle::emit` delivers to the global listeners).
struct EventCollector {
    events: StdMutex<Vec<(String, Value)>>,
}

impl EventCollector {
    /// Register listeners for the given event names (the `listen` — the
    /// `TauriSink`'s `AppHandle::emit` delivers to the global listeners).
    fn new(app: &TestApp, names: &[&str]) -> Arc<Self> {
        let collector = Arc::new(Self {
            events: StdMutex::new(Vec::new()),
        });
        for name in names {
            let c = collector.clone();
            // The `listen` handler is `'static` — the event name is
            // OWNED (the `names` slice's `&str`s don't outlive this
            // function).
            let name_owned = name.to_string();
            let _ = app.handle.listen(name_owned.clone(), move |event| {
                let payload = serde_json::from_str::<Value>(event.payload()).unwrap_or(Value::Null);
                c.events
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push((name_owned.clone(), payload));
            });
        }
        collector
    }

    /// All payloads for an event name (so far).
    fn find(&self, name: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(e, _)| e == name)
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// Wait for an event (bounded — a missing frame is a test failure,
    /// not a hang).
    async fn wait_for(&self, name: &str, timeout: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if let Some(v) = self.find(name).into_iter().next() {
                return Some(v);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        None
    }
}

/// The app's managed `WorkerManager` (the registry view — the
/// `test_support` accessor: a `start_session` spawns a Worker —
/// `test_is_attached` / `test_session_count`).
fn worker_manager(app: &TestApp) -> Arc<WorkerManager> {
    archimedes_lib::test_support::worker_manager(&app.handle)
        .expect("the WorkerManager is managed state")
}

/// The app's managed `Db` (the `test_support` accessor — the
/// `TranscriptPersister`'s rows are asserted through it).
fn db(app: &TestApp) -> Arc<Db> {
    archimedes_lib::test_support::db(&app.handle).expect("the Db is managed state")
}

/// Seed the config dir with a user provider (a raw-JSON `GET /v1/models`
/// discovery server on loopback) + `defaultModel` — `start_session`'s model
/// resolution (the settings `default_model` rung > the catalog default >
/// the selectable set) resolves the composed key `tama/m1` (the
/// `fake_worker` never calls the LLM — the model is resolved, not used).
/// Returns the server task (kept alive for the test's duration — the
/// discovery is a fresh `GET` per `effective_catalog`).
async fn seed_default_model(dirs: &TestDirs) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the discovery listener binds");
    let addr = listener.local_addr().unwrap();
    let settings = json!({
        "providers": [
            { "id": "tama", "name": "tama", "baseUrl": format!("http://{addr}/v1"), "apiKey": "k", "api": "openai-completions" }
        ],
        "defaultModel": "tama/m/1"
    });
    std::fs::write(
        dirs.config_dir.join("settings.json"),
        serde_json::to_string(&settings).unwrap(),
    )
    .expect("settings.json writes");
    archimedes_lib::test_support::raw_json_server(listener, 200, r#"{"data":[{"id":"m/1"}]}"#, None)
        .await
}

/// (a) `start_session` spawns the `fake_worker` (the `WorkerManager`'s
/// registry); a `send_prompt` yields the canned `SinkFrame`s on the Tauri
/// sink (the UI contract) AND writes the `messages` / `native_messages`
/// rows (the store frames applied by the `TranscriptPersister`) AND the
/// raw `RpcEvent` stream is NOT forwarded to the Tauri sink (no
/// `session-update` duplication — the sink saw the `session-update`
/// `SinkFrame`s exactly as the `fake_worker` emitted them).
#[tokio::test]
async fn start_session_spawns_the_worker_and_send_prompt_re_emits_the_sink_frames() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["session-update"]);

    // (a1) `start_session` → the Worker is attached (the `test_support`
    // accessor — the `WorkerManager`'s registry).
    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );
    let wm = worker_manager(&app);
    assert!(
        wm.test_is_attached(&info.session_id),
        "start_session spawned the session's Worker"
    );

    // (a2) `send_prompt` → the canned `SinkFrame`s on the Tauri sink +
    // the store frames persisted.
    let stop = invoke_ok(
        &webview,
        "send_prompt",
        json!({ "sessionId": info.session_id, "text": "hello" }),
    )
    .await;
    assert_eq!(
        stop,
        json!("end_turn"),
        "the turn settles (the agent_settled)"
    );
    // The raw `RpcEvent` stream is NOT forwarded: the `session-update`
    // `SinkFrame`s are re-emitted VERBATIM — the `fake_worker` emits
    // exactly ONE `session_info` frame (on `start`) + TWO
    // `agent_message_chunk` frames (on `prompt`); a raw-`RpcEvent`
    // forward would add `session-update` frames beyond these.
    let updates = collector.find("session-update");
    let chunks = updates
        .iter()
        .filter(|p| p["update"]["sessionUpdate"] == "agent_message_chunk")
        .count();
    assert_eq!(
        chunks, 2,
        "the two `agent_message_chunk` SinkFrames are re-emitted verbatim"
    );
    assert_eq!(
        updates.len(),
        3,
        "no `session-update` beyond the three SinkFrames (the raw RpcEvent is bookkeeping-only)"
    );
    // The store frames are persisted (the `TranscriptPersister` — the
    // `native_messages` / `messages` upserts). `load_native_messages`
    // returns the `content_json` strings (parsed for the `role` field).
    let d = db(&app);
    let native = d
        .load_native_messages(&info.session_id)
        .expect("the native_messages load");
    let roles: Vec<Value> = native
        .iter()
        .map(|c| serde_json::from_str(c).expect("the content_json parses"))
        .collect();
    assert!(
        roles.iter().any(|v| v["role"] == "user"),
        "the user transcript row is persisted"
    );
    assert!(
        roles.iter().any(|v| v["role"] == "assistant"),
        "the assistant transcript row is persisted"
    );
    let msgs = d.messages_for(&info.session_id).expect("the messages load");
    assert!(
        msgs.iter().any(|m| m.kind == "agent-text"),
        "the display row is persisted"
    );
}

/// (b) A `"__crash__"` prompt → the `session-stalled` event fires + the
/// in-flight `send_prompt` resolves (the composer-unlock assertion) + the
/// session resumes via `resume_session` (a fresh `fake_worker` attached).
#[tokio::test]
async fn a_crash_stalls_the_session_resolves_the_turn_and_resumes() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["session-stalled"]);

    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );

    // The crash: the `__crash__` prompt → the Worker exits 137 → the
    // `on_crash` path (the `session-stalled` event + the in-flight
    // `send_prompt` resolution — the composer-unlock assertion).
    let stop = invoke_raw(
        &webview,
        "send_prompt",
        json!({ "sessionId": info.session_id, "text": "__crash__" }),
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "the in-flight send_prompt must RESOLVE on a crash (the composer never stays locked), got error: {e}"
        )
    });
    assert_eq!(
        stop,
        json!("cancelled"),
        "the crash resolves the turn Cancelled"
    );
    // The `session-stalled` event fires (the frontend's banner).
    let stalled = collector
        .wait_for("session-stalled", Duration::from_secs(15))
        .await
        .expect("the session-stalled event fires");
    assert_eq!(stalled["sessionId"], info.session_id);
    assert!(
        stalled["at"].is_number(),
        "the stalled event carries the crash time"
    );
    // The `stalled_info` query (the `get_stalled_info` command).
    let stalled_info = invoke_ok(
        &webview,
        "get_stalled_info",
        json!({ "sessionId": info.session_id }),
    )
    .await;
    assert!(
        stalled_info["at"].is_number(),
        "the stalled info carries the crash time: {stalled_info}"
    );
    // The session is RESUMABLE (a fresh Worker + re-hydrate).
    let resumed: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "resume_session",
            json!({ "sessionId": info.session_id, "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );
    assert_eq!(resumed.session_id, info.session_id);
    let wm = worker_manager(&app);
    assert!(
        wm.test_is_attached(&info.session_id),
        "the resume attached a fresh Worker"
    );
}

/// (c) `respond_permission` relays to the `fake_worker` (the
/// `__permission__` flow completes — the in-flight `send_prompt` settles
/// on the post-response `agent_settled`) AND the `trust-space` outcome
/// writes `spaces.trusted = 1` (the Supervisor's persistence half — the
/// Worker's `StaticTrustSource` flip is covered by the Task-2 unit test,
/// so the assertion here is the `db` write only — minimal, non-redundant).
#[tokio::test]
async fn respond_permission_relays_to_the_worker_and_persists_the_trust_space() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["permission-request"]);

    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );

    // The `__permission__` prompt: the `fake_worker` emits a
    // `PermissionRequest` (the router re-emits it as the
    // `permission-request` Tauri event) and BLOCKS until a
    // `PermissionResponse` — so the `send_prompt` runs in a task (the
    // `respond_permission` must land while it is in flight).
    let prompt_webview = webview.clone();
    let prompt_session = info.session_id.clone();
    let prompt = tokio::spawn(async move {
        invoke_raw(
            &prompt_webview,
            "send_prompt",
            json!({ "sessionId": prompt_session, "text": "__permission__" }),
        )
        .await
    });
    // The `permission-request` event arrives (the payload UNCHANGED —
    // the `requestId` / `options` verbatim).
    let req = collector
        .wait_for("permission-request", Duration::from_secs(15))
        .await
        .expect("the permission-request event fires");
    assert_eq!(req["requestId"], "p1", "the requestId rides verbatim");
    assert!(
        req["request"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["optionId"] == "trust-space"),
        "the options ride verbatim"
    );
    // The `respond_permission` command (the `trust-space` outcome):
    // relays the `PermissionResponse` to the Worker + persists
    // `spaces.trusted = 1` (the Supervisor's persistence half).
    invoke_ok(
        &webview,
        "respond_permission",
        json!({
            "sessionId": info.session_id,
            "requestId": "p1",
            "outcome": { "selected": { "option_id": "trust-space" } }
        }),
    )
    .await;
    // The round-trip completes: the `fake_worker`'s post-response stream
    // (`agent_settled`) settles the in-flight turn (a missed relay would
    // hang the turn forever).
    let stop = prompt
        .await
        .expect("the send_prompt task")
        .unwrap_or_else(|e| panic!("the send_prompt must settle after the relay, got error: {e}"));
    assert_eq!(
        stop,
        json!("end_turn"),
        "the round-trip completes (the response was relayed)"
    );
    // The `trust-space` outcome persists `spaces.trusted = 1`.
    let d = db(&app);
    assert!(
        d.space_trusted(&dirs.cwd())
            .expect("the space_trusted lookup"),
        "the trust-space outcome persists the spaces write"
    );
}

/// (d) A `SinkFrame` (an `interactive-event` todo payload) re-emits on the
/// Tauri sink with the same event name + payload VERBATIM.
#[tokio::test]
async fn an_interactive_event_sink_frame_re_emits_verbatim() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["interactive-event"]);

    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );

    // The `__interactive__` prompt: the `fake_worker` emits an
    // `interactive-event` `SinkFrame` (a `todos_update` payload) +
    // `agent_settled`.
    let stop = invoke_ok(
        &webview,
        "send_prompt",
        json!({ "sessionId": info.session_id, "text": "__interactive__" }),
    )
    .await;
    assert_eq!(stop, json!("end_turn"), "the turn settles");
    // The `interactive-event` re-emits with the same event name + payload
    // VERBATIM (the `todos_update` shape — `store/interactive.ts`
    // consumes it unchanged).
    let events = collector
        .wait_for("interactive-event", Duration::from_secs(15))
        .await
        .map(|first| {
            let all = collector.find("interactive-event");
            (first, all.len())
        })
        .expect("the interactive-event SinkFrame re-emits");
    let (payload, count) = events;
    assert_eq!(count, 1, "the interactive-event re-emits exactly once");
    assert_eq!(
        payload["event"], "todos_update",
        "the payload rides verbatim (the event name)"
    );
    assert_eq!(
        payload["payload"]["todos"][0]["content"], "do the thing",
        "the payload rides verbatim (the todos)"
    );
}

/// (e) A clean `close_session` → `session-closed` (the `ClosedReason`
/// `User` payload) + the in-flight `send_prompt` resolves `Cancelled`
/// (the composer-unlock assertion).
#[tokio::test]
async fn a_clean_close_emits_session_closed_and_resolves_the_turn() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["session-closed"]);

    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );

    // An in-flight turn (the `__slow__` prompt — the `fake_worker`
    // sleeps 10 s before settling — so the close wins the race).
    let prompt_webview = webview.clone();
    let prompt_session = info.session_id.clone();
    let prompt = tokio::spawn(async move {
        invoke_raw(
            &prompt_webview,
            "send_prompt",
            json!({ "sessionId": prompt_session, "text": "__slow__" }),
        )
        .await
    });
    // Let the `send_prompt` ATTACH the in-flight turn BEFORE the close
    // (the `close_session` must not remove the session first — that
    // would make the `send_prompt` an `unknown_session` miss; the
    // `__slow__` turn is in flight for 10 s, so a short settle window
    // is enough for the attach to land).
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The clean close: `close_session` → the `detach` → the Worker's
    // exit (the `reap`'s `close` + grace + `kill` — the `fake_worker`
    // is mid-sleep, so the `kill` lands) → the router's EXPECTED-exit
    // handling.
    invoke_ok(
        &webview,
        "close_session",
        json!({ "sessionId": info.session_id }),
    )
    .await;
    // The `session-closed` event (the `ClosedReason` `User` payload — a
    // `close_session` is a `User` close).
    let closed = collector
        .wait_for("session-closed", Duration::from_secs(15))
        .await
        .expect("the session-closed event fires");
    assert_eq!(closed["reason"], "user", "a close_session is a User close");
    // The in-flight `send_prompt` resolves `Cancelled` (a close kills
    // the turn — the composer unlocks; it never stays locked forever).
    let stop = prompt
        .await
        .expect("the send_prompt task")
        .unwrap_or_else(|e| {
            panic!("the in-flight send_prompt must RESOLVE on a close, got error: {e}")
        });
    assert_eq!(
        stop,
        json!("cancelled"),
        "the close resolves the turn Cancelled"
    );
}

/// (f) `set_space_trusted` sends a `Config { trusted }` to the space's
/// running `fake_worker` (the mid-session trust toggle — the Worker's
/// `StaticTrustSource` flip) AND persists the `spaces` write.
#[tokio::test]
async fn set_space_trusted_pushes_a_config_to_the_running_workers() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["config-ack"]);

    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );
    let _ = info; // the session's Worker is running in the space

    // The `set_space_trusted` command: the `spaces` write + the
    // `Config { trusted }` push to the running Worker in the affected
    // Space.
    invoke_ok(
        &webview,
        "set_space_trusted",
        json!({ "path": dirs.cwd().display().to_string(), "trusted": true }),
    )
    .await;
    // The `fake_worker` echoes the `config` frame (a `config-ack`
    // `SinkFrame` — the delivery observation point).
    let ack = collector
        .wait_for("config-ack", Duration::from_secs(15))
        .await
        .expect("the worker received the Config frame");
    assert_eq!(
        ack["trusted"], true,
        "the Config frame carries the trusted flag"
    );
    // The `spaces` write.
    let d = db(&app);
    assert!(
        d.space_trusted(&dirs.cwd())
            .expect("the space_trusted lookup"),
        "the set_space_trusted command persists the spaces write"
    );
}

/// (f) A `"__subagent__"` prompt → the Supervisor's `dispatch_subagent`
/// flow spawns a SECOND `fake_worker` (the subagent) → the child's
/// transcript persists as a HIDDEN `is_subagent = 1` `sessions` row
/// (the `TranscriptPersister`'s `ensure_session_row` — the child's
/// `sessionId` is the `subagent-session-started` payload's `sessionId`;
/// the `native_messages` FK holds on the child's id, NOT the parent's
/// delivery id — the `persist.rs` frame-`sid` fix).
#[tokio::test]
async fn a_subagent_transcript_persists_as_a_hidden_ephemeral_row() {
    let dirs = TestDirs::new();
    let _server = seed_default_model(&dirs).await;
    let app = build_app(dirs.clone());
    let webview = app.webview.clone();
    let collector = EventCollector::new(&app, &["subagent-session-started", "subagent-closed"]);

    // (f1) `start_session` (the parent — the `fake_worker`'s
    // `SubagentDispatch` frame re-targets the child's frames to the
    // parent's scope for the UI, but the child's transcript rows land
    // on the CHILD's id).
    let info: SessionInfo = value_to(
        invoke_ok(
            &webview,
            "start_session",
            json!({ "cwd": dirs.cwd().display().to_string() }),
        )
        .await,
    );

    // (f2) The `__subagent__` prompt: the `fake_worker` emits a
    // `SubagentDispatch` frame + `agent_settled` (the parent's turn
    // settles); the Supervisor's flow spawns the child `fake_worker`
    // (the child's task is the fixture's default — a canned turn +
    // store frames + `agent_settled`).
    let stop = invoke_ok(
        &webview,
        "send_prompt",
        json!({ "sessionId": info.session_id, "text": "__subagent__" }),
    )
    .await;
    assert_eq!(
        stop,
        json!("end_turn"),
        "the parent's turn settles (the `agent_settled` after the `SubagentDispatch`)"
    );

    // The child's `sessionId` (the `subagent-session-started` payload —
    // the pre-minted child id the flow `attach`ed).
    let started = collector
        .wait_for("subagent-session-started", Duration::from_secs(30))
        .await
        .expect("the `subagent-session-started` fired");
    let child_id = started["sessionId"]
        .as_str()
        .expect("the started event carries the child's sessionId")
        .to_string();
    assert!(!child_id.is_empty(), "the child's sessionId is a real id");
    assert_eq!(
        started["parentSessionId"], info.session_id,
        "the started event carries the parent's id"
    );

    // The child's turn completed (the `subagent-closed` — the drive's
    // settle + teardown). By this point the child's store frames have
    // been persisted (they land BEFORE the child's `agent_settled`).
    let closed = collector
        .wait_for("subagent-closed", Duration::from_secs(30))
        .await
        .expect("the `subagent-closed` fired");
    assert_eq!(
        closed["status"], "completed",
        "the child completed (the `fake_worker`'s canned turn) — got: {closed:?}"
    );
    assert_eq!(
        closed["sessionId"], child_id,
        "the closed event carries the same child id"
    );

    // The child's transcript persists as a HIDDEN `is_subagent = 1`
    // `sessions` row (the `ensure_session_row` — the `native_messages`
    // FK holds on the CHILD's id, not the parent's delivery id).
    let d = db(&app);
    let child_row = d
        .session(&child_id)
        .expect("the child's sessions row load")
        .expect("the child's ephemeral row exists (the `ensure_session_row`)");
    assert!(
        child_row.is_subagent,
        "the child's row is flagged `is_subagent`"
    );
    // The row is HIDDEN from `list_sessions` (the ephemeral row — the
    // user's session list is unchanged by the subagent's transcript).
    let all = d.list_sessions(true).expect("the sessions list");
    assert!(
        !all.iter().any(|r| r.id == child_id),
        "the child's row is hidden from list_sessions"
    );
    // The child's transcript rows (the `native_messages` — the
    // `ensure_session_row` created the FK row before the insert).
    let native = d
        .load_native_messages(&child_id)
        .expect("the child's native_messages load");
    assert!(
        !native.is_empty(),
        "the child's transcript rows are persisted (the `native_messages` FK held)"
    );
}
