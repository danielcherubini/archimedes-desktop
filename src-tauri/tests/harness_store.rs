//! The native transcript store (native-agent-harness Task 6): the
//! `native_messages` table — round-trip, clear/display independence,
//! and the tool-transcript fidelity (the lossy-transcript guard: a
//! `ToolCall` message's arguments + a `tool` result's `tool_call_id`
//! must round-trip through `content_json`).

use std::sync::Arc;

use archimedes_lib::agent::harness::{
    ChatMessage, ChatRole, MessageContent, SessionStore, ToolCall,
};
use archimedes_lib::agent::SessionInfo;
use archimedes_lib::storage::Db;
use serde_json::json;

/// A temp `Db` (a fresh file per test — `Db` is a single
/// `rusqlite::Connection` behind a `std::sync::Mutex`).
fn temp_db() -> (Arc<Db>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("harness-store-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite");
    let db = Arc::new(Db::open(&path).unwrap());
    (db, dir)
}

/// A `sessions` row (the display `messages` table AND `native_messages`
/// are FK-referenced to it — a transcript row requires the session row).
fn record_session(db: &Db, id: &str) {
    db.record_session(&SessionInfo {
        session_id: id.to_string(),
        agent_id: "native".to_string(),
        cwd: std::path::PathBuf::from("/tmp"),
        capabilities: json!({}),
        config_options: None,
        archived: false,
    })
    .unwrap();
}

fn user(text: &str) -> ChatMessage {
    ChatMessage {
        role: ChatRole::User,
        content: MessageContent::Text(text.to_string()),
        tool_call_id: None,
        tool_calls: None,
    }
}

fn assistant(text: &str) -> ChatMessage {
    ChatMessage {
        role: ChatRole::Assistant,
        content: MessageContent::Text(text.to_string()),
        tool_call_id: None,
        tool_calls: None,
    }
}

#[test]
fn insert_and_load_round_trip() {
    let (db, _dir) = temp_db();
    record_session(&db, "s1");
    let store = SessionStore::new(db.clone());

    store
        .insert_message(
            "s1",
            0,
            "user",
            &serde_json::to_string(&user("hi")).unwrap(),
        )
        .unwrap();
    store
        .insert_message(
            "s1",
            1,
            "assistant",
            &serde_json::to_string(&assistant("hello")).unwrap(),
        )
        .unwrap();

    let msgs = store.load_messages("s1").unwrap();
    assert_eq!(msgs.len(), 2, "both rows load");
    assert_eq!(msgs[0], user("hi"), "the user message round-trips");
    assert_eq!(
        msgs[1],
        assistant("hello"),
        "the assistant message round-trips"
    );
    // A different session is isolated.
    assert!(store.load_messages("s2").unwrap().is_empty());
}

#[test]
fn insert_is_idempotent_on_conflict() {
    let (db, _dir) = temp_db();
    record_session(&db, "s1");
    let store = SessionStore::new(db.clone());

    let first = serde_json::to_string(&user("v1")).unwrap();
    store.insert_message("s1", 0, "user", &first).unwrap();
    let second = serde_json::to_string(&user("v2")).unwrap();
    store.insert_message("s1", 0, "user", &second).unwrap();

    let msgs = store.load_messages("s1").unwrap();
    assert_eq!(
        msgs.len(),
        1,
        "the same (session_id, seq) collapses to one row"
    );
    assert_eq!(msgs[0], user("v2"), "the upsert refreshes the content");
}

#[test]
fn clear_messages_clears_native_but_not_display() {
    let (db, _dir) = temp_db();
    let store = SessionStore::new(db.clone());
    record_session(&db, "s1");
    // A display row (the `record_message` upsert — the display
    // `messages` table).
    db.record_message("s1", "user", None, r#"{"text":"display"}"#)
        .unwrap();
    store
        .insert_message(
            "s1",
            0,
            "user",
            &serde_json::to_string(&user("native")).unwrap(),
        )
        .unwrap();

    store.clear_messages("s1").unwrap();

    assert!(
        store.load_messages("s1").unwrap().is_empty(),
        "clear_messages empties the native_messages rows"
    );
    let rows = db.messages_for("s1").unwrap();
    assert_eq!(rows.len(), 1, "the display `messages` row survives");
    assert_eq!(rows[0].kind, "user");
    assert_eq!(rows[0].payload_json, r#"{"text":"display"}"#);
}

#[test]
fn tool_transcript_round_trip_preserves_tool_calls_and_ids() {
    // The lossy-transcript guard: a `ToolCall` message (with arguments)
    // + a `tool` result message (with `tool_call_id`) must round-trip
    // through `content_json` with the tool-call arguments and the
    // result's `tool_call_id` intact.
    let (db, _dir) = temp_db();
    record_session(&db, "s1");
    let store = SessionStore::new(db.clone());

    let assistant_with_call = ChatMessage {
        role: ChatRole::Assistant,
        content: MessageContent::Text(String::new()),
        tool_call_id: None,
        tool_calls: Some(vec![ToolCall {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            arguments: json!({ "command": "ls -la", "timeout_ms": 5000 }),
        }]),
    };
    let tool_result = ChatMessage {
        role: ChatRole::Tool,
        content: MessageContent::Text("file-list".to_string()),
        tool_call_id: Some("call_1".to_string()),
        tool_calls: None,
    };
    store
        .insert_message(
            "s1",
            0,
            "assistant",
            &serde_json::to_string(&assistant_with_call).unwrap(),
        )
        .unwrap();
    store
        .insert_message(
            "s1",
            1,
            "tool",
            &serde_json::to_string(&tool_result).unwrap(),
        )
        .unwrap();

    let msgs = store.load_messages("s1").unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(
        msgs[0].tool_calls,
        Some(vec![ToolCall {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            arguments: json!({ "command": "ls -la", "timeout_ms": 5000 }),
        }]),
        "the tool-call arguments round-trip"
    );
    assert_eq!(
        msgs[1].tool_call_id.as_deref(),
        Some("call_1"),
        "the tool result's tool_call_id round-trips"
    );
    assert_eq!(msgs[1], tool_result);
}

#[test]
fn display_record_is_independent_of_native_messages() {
    let (db, _dir) = temp_db();
    let store = SessionStore::new(db.clone());
    record_session(&db, "s1");

    // A display row ONLY (the existing `record_message` persistence —
    // the display `payload_json` is a display shape, lossy for the
    // provider transcript).
    db.record_message("s1", "agent-text", Some("m1"), r#"{"text":"x"}"#)
        .unwrap();

    assert!(
        store.load_messages("s1").unwrap().is_empty(),
        "a display row never lands in native_messages"
    );
}
