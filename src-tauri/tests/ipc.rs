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

use archimedes_desktop_lib::agent::EventSink;
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
            archimedes_desktop_lib::commands::settings::save_settings,
            archimedes_desktop_lib::commands::spaces::list_agents,
            archimedes_desktop_lib::commands::spaces::list_spaces,
            archimedes_desktop_lib::commands::spaces::delete_space,
            archimedes_desktop_lib::commands::spaces::space_for_path
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

#[test]
fn spaces_and_agents_commands_round_trip() {
    let config_dir = temp_dir("config");
    let app_data_dir = temp_dir("data");
    write_agents_json(&config_dir);

    let (events_tx, events_rx) = std::sync::mpsc::channel();
    let app = build_app(config_dir.clone(), app_data_dir.clone(), events_tx);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock webview should build");

    // --- the folder the new-space dialog would pick ---
    let newproj = config_dir.join("newproj");
    std::fs::create_dir_all(&newproj).unwrap();

    // space_for_path: a fresh, not-yet-a-space folder is canonicalized and
    // reports isSpace=false (on a symlink-free temp dir canonical == the path;
    // if the platform symlinked it instead, this still asserts the returned
    // path is the canonical one — the behaviour we want to observe).
    let check = invoke(
        &webview,
        "space_for_path",
        serde_json::json!({ "path": newproj.to_string_lossy() }),
    );
    assert_eq!(
        check["canonicalPath"],
        std::fs::canonicalize(&newproj)
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(check["isSpace"], false);

    // list_agents: exactly the registry entries (camelCase keys); default
    // selection is agents[0] on the frontend.
    let agents = invoke(&webview, "list_agents", serde_json::json!({}));
    assert_eq!(
        agents,
        serde_json::json!([{ "id": "fake", "name": "Fake Agent" }])
    );

    // --- start a session in that folder (record_session → upsert_space hook) ---
    let info = invoke(
        &webview,
        "start_session",
        serde_json::json!({ "agentId": "fake", "cwd": newproj.to_string_lossy() }),
    );
    assert_eq!(info["sessionId"], FAKE_SESSION_ID);

    // list_spaces: exactly one row, the canonicalized path.
    let canonical = std::fs::canonicalize(&newproj)
        .unwrap()
        .display()
        .to_string();
    let spaces = invoke(&webview, "list_spaces", serde_json::json!({}));
    let arr = spaces.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["path"], canonical);

    // space_for_path again: the folder is a space now.
    let check = invoke(
        &webview,
        "space_for_path",
        serde_json::json!({ "path": newproj.to_string_lossy() }),
    );
    assert_eq!(check["isSpace"], true);

    // delete_space: drops the bookkeeping row only.
    invoke(
        &webview,
        "delete_space",
        serde_json::json!({ "path": canonical }),
    );
    let spaces = invoke(&webview, "list_spaces", serde_json::json!({}));
    assert!(spaces.as_array().unwrap().is_empty());

    // The stored session is untouched by deleting the space.
    let sessions = invoke(&webview, "list_sessions", serde_json::json!({}));
    let arr = sessions.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["sessionId"], FAKE_SESSION_ID);

    // space_for_path on a nonexistent folder: an ERROR, not a None (the
    // dialog shows the reason inline).
    let missing = config_dir.join(format!("nope-{}", uuid::Uuid::new_v4()));
    let err = get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "space_for_path".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(serde_json::json!({
                "path": missing.to_string_lossy()
            })),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .expect_err("a missing folder must be an error, not a None");
    let err_str = err.to_string();
    assert!(
        err_str.contains("no such folder"),
        "expected a 'no such folder' error, got: {err_str}"
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

    // Clean up (best effort).
    drop(app);
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&app_data_dir);
}

/// The `invoke` helper `.expect`s success — this variant does NOT, and
/// returns the error payload (`get_ipc_response`'s error is the payload
/// `Value` itself).
fn invoke_err(
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
    .expect_err("the invalid image should be rejected")
}

#[test]
fn send_prompt_images_ipc() {
    let config_dir = temp_dir("config");
    let app_data_dir = temp_dir("data");
    write_agents_json(&config_dir);

    let (events_tx, events_rx) = std::sync::mpsc::channel();
    let app = build_app(config_dir.clone(), app_data_dir.clone(), events_tx);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock webview should build");

    // --- start a session with the fake agent ---
    let info = invoke(
        &webview,
        "start_session",
        serde_json::json!({ "agentId": "fake", "cwd": config_dir.to_string_lossy() }),
    );
    assert_eq!(info["sessionId"], FAKE_SESSION_ID);

    // --- invalid image FIRST (validation-before-persistence proof) ---
    // An SVG from hand-rolled IPC must be rejected BEFORE anything is
    // written to SQLite: no `user` row may exist afterwards.
    let err = invoke_err(
        &webview,
        "send_prompt",
        serde_json::json!({
            "sessionId": FAKE_SESSION_ID,
            "text": "look at this",
            "images": [{ "mimeType": "image/svg+xml", "data": "AQID", "name": "a.svg", "sizeBytes": 3 }]
        }),
    );
    let err_str = err.to_string();
    assert!(
        err_str.contains("unsupported image type"),
        "expected an invalid-payload error, got: {err_str}"
    );
    let db = Db::open(&app_data_dir.join("archimedes.db")).unwrap();
    let rows = db.messages_for(FAKE_SESSION_ID).unwrap();
    assert!(
        rows.iter().all(|r| r.kind != "user"),
        "a rejected image payload must not be persisted, got: {:?}",
        rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>()
    );

    // --- valid image (nested camelCase deserialization proof) ---
    // The nested `mimeType` / `sizeBytes` keys reach the `ImagePayload`
    // `#[serde(rename_all = "camelCase")]` over the REAL Tauri IPC wire —
    // the unit tests never exercise Tauri's argument deserialization.
    let stop = invoke(
        &webview,
        "send_prompt",
        serde_json::json!({
            "sessionId": FAKE_SESSION_ID,
            "text": "look",
            "images": [{ "mimeType": "image/png", "data": "AQID", "name": "a.png", "sizeBytes": 3 }]
        }),
    );
    assert_eq!(stop, "end_turn");
    // Poll until the user row lands (the persistence hook may still be
    // draining — same deadline pattern as the existing test).
    let deadline = Instant::now() + Duration::from_secs(10);
    let user = loop {
        let rows = db.messages_for(FAKE_SESSION_ID).unwrap();
        if let Some(row) = rows.iter().find(|r| r.kind == "user") {
            break row.clone();
        }
        if Instant::now() > deadline {
            panic!(
                "user row did not appear; got {:?}",
                rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let payload: serde_json::Value = serde_json::from_str(&user.payload_json).unwrap();
    assert_eq!(payload["text"], "look");
    assert_eq!(payload["images"][0]["mimeType"], "image/png");
    assert_eq!(payload["images"][0]["data"], "AQID");

    // --- too many images (the count-cap proof over the REAL IPC wire) ---
    // 9 valid images exceed the `MAX_IMAGE_COUNT` (8) cap: rejected BEFORE
    // anything is persisted (same validation-before-persistence proof as the
    // SVG case above) — exactly one `user` row (the valid one above) exists.
    let nine: Vec<serde_json::Value> = (0..9)
        .map(|i| {
            serde_json::json!({
                "mimeType": "image/png",
                "data": "AQID",
                "name": format!("a{i}.png"),
                "sizeBytes": 3
            })
        })
        .collect();
    let err = invoke_err(
        &webview,
        "send_prompt",
        serde_json::json!({
            "sessionId": FAKE_SESSION_ID,
            "text": "look",
            "images": nine
        }),
    );
    let err_str = err.to_string();
    assert!(
        err_str.contains("at most 8"),
        "expected the image-count cap error, got: {err_str}"
    );
    let rows = db.messages_for(FAKE_SESSION_ID).unwrap();
    let user_rows: Vec<_> = rows.iter().filter(|r| r.kind == "user").collect();
    assert_eq!(
        user_rows.len(),
        1,
        "the 9-image payload must not add a user row, got {:?}",
        rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>()
    );

    // --- image-only prompt (empty text + 1 valid image) ---
    // The frontend supports image-only sends; the persisted row is
    // `{"text": "", "images": [...]}` (the client must not ship an empty
    // text block to the provider — the unit test proves the block shape).
    let stop = invoke(
        &webview,
        "send_prompt",
        serde_json::json!({
            "sessionId": FAKE_SESSION_ID,
            "text": "",
            "images": [{ "mimeType": "image/png", "data": "AQID", "name": "only.png", "sizeBytes": 3 }]
        }),
    );
    assert_eq!(stop, "end_turn");
    // Poll for the image-only user row (distinct from the one above by name).
    let deadline = Instant::now() + Duration::from_secs(10);
    let user = loop {
        let rows = db.messages_for(FAKE_SESSION_ID).unwrap();
        if let Some(row) = rows
            .iter()
            .find(|r| r.kind == "user" && r.payload_json.contains("only.png"))
        {
            break row.clone();
        }
        if Instant::now() > deadline {
            panic!(
                "image-only user row did not appear; got {:?}",
                rows.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let payload: serde_json::Value = serde_json::from_str(&user.payload_json).unwrap();
    assert_eq!(payload["text"], "");
    assert_eq!(payload["images"][0]["name"], "only.png");
    assert_eq!(payload["images"][0]["data"], "AQID");

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

    // Clean up (best effort).
    drop(app);
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&app_data_dir);
}
