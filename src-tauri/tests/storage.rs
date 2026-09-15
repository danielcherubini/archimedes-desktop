//! Integration test for the SQLite persistence layer (Task 5).
//!
//! Covers: schema creation, session upsert, message upsert semantics (two
//! agent-text chunks sharing one message_key collapse to a single row with
//! the accumulated text), session listing, history fetch, and cascade
//! delete (deleting a session removes its messages).

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{AgentCapabilities, SessionId};
use archimedes_desktop_lib::acp::SessionInfo;
use archimedes_desktop_lib::storage::Db;

fn temp_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("archimedes-storage-test-{}", uuid::Uuid::new_v4()))
}

fn sample_session() -> SessionInfo {
    SessionInfo {
        session_id: SessionId::new("sess-1"),
        agent_id: "fake".to_string(),
        cwd: PathBuf::from("/tmp/proj"),
        capabilities: AgentCapabilities::default(),
    }
}

#[test]
fn records_sessions_and_messages_with_upsert_semantics() {
    let path = temp_db_path();
    let db = Db::open(&path).expect("db should open");

    let session = sample_session();
    db.record_session(&session)
        .expect("record_session should succeed");

    // Two agent-text chunks sharing one message_key: the second record
    // carries the ACCUMULATED text and must upsert the first row, not
    // insert a duplicate.
    db.record_message("sess-1", "agent-text", Some("m1"), r#"{"text":"hello"}"#)
        .expect("record_message should succeed");
    db.record_message(
        "sess-1",
        "agent-text",
        Some("m1"),
        r#"{"text":"hello world"}"#,
    )
    .expect("record_message should succeed");
    // One tool-call message, keyed by its tool call id.
    db.record_message(
        "sess-1",
        "tool-call",
        Some("tc1"),
        r#"{"toolCallId":"tc1","title":"run a tool","status":"completed"}"#,
    )
    .expect("record_message should succeed");

    // --- list_sessions ---
    let sessions = db.list_sessions().expect("list_sessions should succeed");
    assert_eq!(sessions.len(), 1, "exactly one session should be stored");
    assert_eq!(sessions[0].id, "sess-1");
    assert_eq!(sessions[0].agent_id, "fake");
    assert_eq!(sessions[0].cwd, "/tmp/proj");
    assert!(sessions[0].capabilities_json.starts_with("{"));

    // --- messages_for ---
    let messages = db
        .messages_for("sess-1")
        .expect("messages_for should succeed");
    assert_eq!(
        messages.len(),
        2,
        "exactly one agent-text row (upserted) plus one tool-call row; got {:?}",
        messages.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>()
    );

    let agent_text = messages
        .iter()
        .find(|m| m.kind == "agent-text")
        .expect("an agent-text row should exist");
    assert_eq!(agent_text.message_key.as_deref(), Some("m1"));
    let payload: serde_json::Value =
        serde_json::from_str(&agent_text.payload_json).expect("payload should be JSON");
    assert_eq!(
        payload["text"], "hello world",
        "the upserted row must hold the accumulated text"
    );

    let tool_call = messages
        .iter()
        .find(|m| m.kind == "tool-call")
        .expect("a tool-call row should exist");
    assert_eq!(tool_call.message_key.as_deref(), Some("tc1"));

    // --- cascade delete ---
    db.delete_session("sess-1")
        .expect("delete_session should succeed");
    assert!(
        db.list_sessions()
            .expect("list_sessions should succeed")
            .is_empty(),
        "the session should be gone after delete"
    );
    assert!(
        db.messages_for("sess-1")
            .expect("messages_for should succeed")
            .is_empty(),
        "messages must be removed with their session (ON DELETE CASCADE)"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn reopens_an_existing_database() {
    let path = temp_db_path();
    {
        let db = Db::open(&path).expect("db should open");
        let session = sample_session();
        db.record_session(&session)
            .expect("record_session should succeed");
        db.record_message("sess-1", "user", None, r#"{"text":"hi"}"#)
            .expect("record_message should succeed");
    }
    // A second open (simulating an app restart) must see the same data.
    let db = Db::open(&path).expect("db should reopen");
    assert_eq!(db.list_sessions().expect("list_sessions").len(), 1);
    assert_eq!(db.messages_for("sess-1").expect("messages_for").len(), 1);
    let _ = std::fs::remove_file(&path);
}
