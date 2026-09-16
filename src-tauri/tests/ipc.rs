//! End-to-end IPC test for the Task 5 command surface (history, settings,
//! resume) — runs the REAL Tauri command handlers, the real SQLite file, and
//! the real fake agent, without a GUI (Tauri's mock runtime).
//!
//! Flow: settings defaults + round-trip → start a session with the fake
//! agent → prompt → the transcript is persisted → `list_sessions` /
//! `load_history` see it → `delete_session` removes it (cascade).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

use archimedes_desktop_lib::acp::EventSink;
use archimedes_desktop_lib::storage::Db;

/// The fixed session id reported by the fake agent (see `bin/fake_agent.rs`).
const FAKE_SESSION_ID: &str = "fake-session-1";
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("archimedes-ipc-{}-{}", tag, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_agents_json(dir: &std::path::Path) {
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

/// A test `EventSink` that records events over an mpsc channel.
#[derive(Clone)]
struct TestSink(std::sync::mpsc::Sender<(String, serde_json::Value)>);

impl EventSink for TestSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.send((event.to_string(), payload));
    }
}

fn build_app(
    config_dir: PathBuf,
    app_data_dir: PathBuf,
    events: std::sync::mpsc::Sender<(String, serde_json::Value)>,
) -> tauri::App<MockRuntime> {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            archimedes_desktop_lib::commands::app_info,
            archimedes_desktop_lib::commands::sessions::start_session,
            archimedes_desktop_lib::commands::sessions::send_prompt,
            archimedes_desktop_lib::commands::sessions::close_session,
            archimedes_desktop_lib::commands::sessions::respond_permission,
            archimedes_desktop_lib::commands::sessions::resume_session,
            archimedes_desktop_lib::commands::history::list_sessions,
            archimedes_desktop_lib::commands::history::load_history,
            archimedes_desktop_lib::commands::history::delete_session,
            archimedes_desktop_lib::commands::settings::get_settings,
            archimedes_desktop_lib::commands::settings::save_settings
        ])
        .build(tauri::generate_context!())
        .expect("app should build");

    // Exercise the REAL setup path: `run()`'s setup closure calls exactly
    // this function (the mock runtime never runs the event loop's `Ready`
    // phase, where the setup hook would run, so the test calls it directly
    // on the built app). This registers the SessionManager, the Db, and
    // the EventSink exactly as the real app does.
    archimedes_desktop_lib::setup_dirs(&app, config_dir, app_data_dir)
        .expect("setup_dirs should succeed");

    // Regression check for the TypeId lesson: the commands resolve the
    // sink as `State<Arc<dyn EventSink>>`, and Tauri's state registry is
    // keyed by TypeId. If `setup_dirs` ever manages the CONCRETE
    // `Arc<TauriSink<R>>` again (the original runtime bug: "state not
    // managed for field `sink` on command `start_session`), the
    // trait-object slot is empty and this assertion fails.
    assert!(
        app.try_state::<std::sync::Arc<dyn EventSink>>().is_some(),
        "setup_dirs must manage the sink as Arc<dyn EventSink> (TypeId-keyed)"
    );

    // Swap the managed sink for the observable test sink (same TypeId
    // slot, same annotation style as `setup_dirs`). A second `manage()`
    // of an already-managed type is a no-op in Tauri, so the slot has to
    // be cleared out first.
    #[allow(deprecated)] // `unmanage` is the only way to clear the slot in a test
    let _ = app.unmanage::<std::sync::Arc<dyn EventSink>>();
    let sink: std::sync::Arc<dyn EventSink> = std::sync::Arc::new(TestSink(events));
    let swapped = app.manage(sink);
    assert!(
        swapped,
        "test sink should take the same slot the real sink leaves"
    );

    app
}

fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    cmd: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .expect("IPC call should succeed")
    .deserialize::<serde_json::Value>()
    .expect("response should deserialize")
}

#[test]
fn history_settings_and_resume_commands_round_trip() {
    let config_dir = temp_dir("config");
    let app_data_dir = temp_dir("data");
    write_agents_json(&config_dir);

    let (events_tx, events_rx) = std::sync::mpsc::channel();
    let app = build_app(config_dir.clone(), app_data_dir.clone(), events_tx);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock webview should build");

    // --- settings: sane defaults on first run ---
    let settings = invoke(&webview, "get_settings", serde_json::json!({}));
    assert_eq!(settings["theme"], "dark");
    assert!(settings["paneLayout"].is_object());
    // The defaults file must have been written.
    assert!(config_dir.join("settings.json").exists());

    // --- settings: save + read back ---
    let saved = invoke(
        &webview,
        "save_settings",
        serde_json::json!({
            "settings": { "theme": "light", "paneLayout": { "chatWidth": 480 } }
        }),
    );
    assert!(saved.is_null() || saved.is_object());
    let settings = invoke(&webview, "get_settings", serde_json::json!({}));
    assert_eq!(settings["theme"], "light");
    assert_eq!(settings["paneLayout"]["chatWidth"], 480);

    // --- history: empty before any session ---
    let sessions = invoke(&webview, "list_sessions", serde_json::json!({}));
    assert!(sessions.as_array().unwrap().is_empty());

    // --- start a session with the fake agent ---
    let info = invoke(
        &webview,
        "start_session",
        serde_json::json!({ "agentId": "fake", "cwd": config_dir.to_string_lossy() }),
    );
    assert_eq!(info["sessionId"], FAKE_SESSION_ID);
    assert!(info["capabilities"]["loadSession"].as_bool() == Some(false));

    // --- prompt → the fake agent streams two chunks ---
    let stop = invoke(
        &webview,
        "send_prompt",
        serde_json::json!({ "sessionId": FAKE_SESSION_ID, "text": "hi" }),
    );
    assert_eq!(stop, "end_turn");

    // The persistence hook runs in the notification handler; poll the DB
    // until both rows land (the IPC response returns when the turn ends,
    // but the notification handler may still be draining).
    let db = Db::open(&app_data_dir.join("archimedes.db")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let agent = loop {
        let rows = db.messages_for(FAKE_SESSION_ID).unwrap();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        if kinds == vec!["user", "agent-text"] {
            break rows
                .into_iter()
                .find(|r| r.kind == "agent-text")
                .expect("agent-text row");
        }
        if Instant::now() > deadline {
            panic!("transcript rows did not appear; got {kinds:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let payload: serde_json::Value = serde_json::from_str(&agent.payload_json).unwrap();
    assert_eq!(payload["text"], "hello world");

    // --- list_sessions + load_history see the session ---
    let sessions = invoke(&webview, "list_sessions", serde_json::json!({}));
    let arr = sessions.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["sessionId"], FAKE_SESSION_ID);
    assert_eq!(arr[0]["agentId"], "fake");

    let history = invoke(
        &webview,
        "load_history",
        serde_json::json!({ "sessionId": FAKE_SESSION_ID }),
    );
    let kinds: Vec<String> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, vec!["user".to_string(), "agent-text".to_string()]);

    // --- resume is refused: the fake agent does not advertise loadSession ---
    let err = get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "resume_session".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(serde_json::json!({
                "agentId": "fake",
                "sessionId": FAKE_SESSION_ID,
                "cwd": config_dir.to_string_lossy()
            })),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .expect_err("resume must fail for a non-loadSession agent");
    let err_str = err.to_string();
    assert!(
        err_str.contains("not") && err_str.to_lowercase().contains("resum"),
        "expected a NotResumable error, got: {err_str}"
    );

    // --- close the session and wait for the driver task's teardown ---
    invoke(
        &webview,
        "close_session",
        serde_json::json!({ "sessionId": FAKE_SESSION_ID }),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match events_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((event, _)) if event == "session-closed" => break,
            Ok(_) => continue,
            Err(_) => panic!("session-closed event should arrive after close"),
        }
    }

    invoke(
        &webview,
        "delete_session",
        serde_json::json!({ "sessionId": FAKE_SESSION_ID }),
    );
    let sessions = invoke(&webview, "list_sessions", serde_json::json!({}));
    assert!(sessions.as_array().unwrap().is_empty());
    let history = invoke(
        &webview,
        "load_history",
        serde_json::json!({ "sessionId": FAKE_SESSION_ID }),
    );
    assert!(history.as_array().unwrap().is_empty());

    // Clean up (best effort).
    drop(app);
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&app_data_dir);
}
