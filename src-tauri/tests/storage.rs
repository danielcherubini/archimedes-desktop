//! Integration test for the SQLite persistence layer (Task 5).
//!
//! Covers: schema creation, session upsert, message upsert semantics (two
//! agent-text chunks sharing one message_key collapse to a single row with
//! the accumulated text), session listing, history fetch, and cascade
//! delete (deleting a session removes its messages).

use std::path::PathBuf;

use archimedes_desktop_lib::agent::SessionInfo;
use archimedes_desktop_lib::storage::Db;

fn temp_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("archimedes-storage-test-{}", uuid::Uuid::new_v4()))
}

fn sample_session() -> SessionInfo {
    SessionInfo {
        session_id: "sess-1".to_string(),
        agent_id: "fake".to_string(),
        cwd: PathBuf::from("/tmp/proj"),
        capabilities: serde_json::json!({
            "piSessionId": "sess-1",
            "loadSession": true,
            "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false },
        }),
        config_options: None,
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

#[test]
fn spaces_upsert_find_delete_and_order() {
    let path = temp_db_path();
    let db = Db::open(&path).expect("db should open");

    assert!(
        db.list_spaces().expect("list_spaces").is_empty(),
        "a fresh db has no spaces"
    );

    db.upsert_space("/tmp/pa").expect("upsert_space /tmp/pa");
    std::thread::sleep(std::time::Duration::from_millis(5));
    db.upsert_space("/tmp/pb").expect("upsert_space /tmp/pb");

    let spaces = db.list_spaces().expect("list_spaces");
    assert_eq!(spaces.len(), 2, "exactly two spaces should be stored");
    assert_eq!(spaces[0].path, "/tmp/pb", "most recently opened first");
    let pa = spaces
        .iter()
        .find(|s| s.path == "/tmp/pa")
        .expect("a /tmp/pa row should exist");
    assert!(pa.created_at <= pa.last_opened_at);

    // A re-touch wins the ordering.
    std::thread::sleep(std::time::Duration::from_millis(5));
    db.upsert_space("/tmp/pa")
        .expect("upsert_space /tmp/pa again");
    let spaces = db.list_spaces().expect("list_spaces");
    assert_eq!(
        spaces[0].path, "/tmp/pa",
        "a re-touch wins the recent-first ordering"
    );

    assert_eq!(
        db.find_space("/tmp/pa").expect("find_space /tmp/pa"),
        Some("/tmp/pa".to_string())
    );
    assert_eq!(db.find_space("/nope").expect("find_space /nope"), None);

    db.delete_space("/tmp/pb").expect("delete_space /tmp/pb");
    let spaces = db.list_spaces().expect("list_spaces");
    assert_eq!(spaces.len(), 1, "deleting a space leaves the other");
    assert_eq!(spaces[0].path, "/tmp/pa");
    let pa_before = spaces[0].clone();
    std::thread::sleep(std::time::Duration::from_millis(5));
    db.upsert_space("/tmp/pb")
        .expect("upsert_space /tmp/pb again");
    let spaces = db.list_spaces().expect("list_spaces");
    assert_eq!(
        spaces.len(),
        2,
        "a re-upsert of a deleted space brings it back"
    );
    let pb = spaces
        .iter()
        .find(|s| s.path == "/tmp/pb")
        .expect("a /tmp/pb row should exist");
    assert!(
        pb.created_at > pa_before.created_at,
        "a recreated space gets a fresh created_at"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_backfills_space_rows_from_existing_sessions() {
    let base = std::env::temp_dir().join(format!("archimedes-spaces-bk-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).expect("base dir");
    let cwd_a = base.join("cwd_a");
    let cwd_b = base.join("cwd_b");
    // These MUST exist for the backfill's canonicalization.
    std::fs::create_dir_all(&cwd_a).expect("cwd_a dir");
    std::fs::create_dir_all(&cwd_b).expect("cwd_b dir");
    // A folder that is NEVER created: the backfill must skip it without failing.
    let gone = base.join(format!("gone-{}", uuid::Uuid::new_v4()));
    let db_path = base.join("archimedes.db");

    let mk_session = |id: &str, cwd: std::path::PathBuf| SessionInfo {
        session_id: id.to_string(),
        agent_id: "fake".to_string(),
        cwd,
        capabilities: serde_json::json!({
            "piSessionId": id,
            "loadSession": true,
            "promptCapabilities": { "image": true, "audio": false, "embeddedContext": false },
        }),
        config_options: None,
    };

    let db1 = Db::open(&db_path).expect("db should open");
    // Two sessions in cwd_a (the `SELECT DISTINCT` must collapse them), one in
    // cwd_b (an existing folder that got a row through a real session), and
    // one in the never-created `gone` folder.
    db1.record_session(&mk_session("sess-a1", cwd_a.clone()))
        .expect("record a1");
    db1.record_session(&mk_session("sess-a2", cwd_a.clone()))
        .expect("record a2");
    db1.record_session(&mk_session("sess-b1", cwd_b.clone()))
        .expect("record b1");
    db1.record_session(&mk_session("sess-g1", gone.clone()))
        .expect("record g1");
    // db1 stays open while db2 runs its backfill — that's fine: the backfill
    // only issues `INSERT ... DO NOTHING`.

    let db2 = Db::open(&db_path).expect("db should reopen");
    let spaces = db2.list_spaces().expect("list_spaces");
    assert_eq!(
        spaces.len(),
        2,
        "the backfill creates rows for the canonical existing cwds only; got {spaces:?}"
    );
    let canonical_a = std::fs::canonicalize(&cwd_a).expect("canonical cwd_a");
    let canonical_b = std::fs::canonicalize(&cwd_b).expect("canonical cwd_b");
    let paths: Vec<String> = spaces.iter().map(|s| s.path.clone()).collect();
    assert!(
        paths.contains(&canonical_a.display().to_string()),
        "a row for the canonical cwd_a"
    );
    assert!(
        paths.contains(&canonical_b.display().to_string()),
        "a row for the canonical cwd_b"
    );
    // The vanished folder: no row, and it did not fail `Db::open`.
    assert_eq!(
        db2.find_space(gone.display().to_string().as_str())
            .expect("find_space gone"),
        None
    );
    assert_eq!(
        db2.find_space(canonical_a.display().to_string().as_str())
            .expect("find_space a"),
        Some(canonical_a.display().to_string())
    );

    // A third open: the backfill is idempotent (DO NOTHING) — same rows,
    // timestamps unchanged (a backfill must never refresh recency).
    let before: Vec<(String, i64, i64)> = spaces
        .iter()
        .map(|s| (s.path.clone(), s.created_at, s.last_opened_at))
        .collect();
    let db3 = Db::open(&db_path).expect("db should reopen again");
    let spaces3 = db3.list_spaces().expect("list_spaces");
    let after: Vec<(String, i64, i64)> = spaces3
        .iter()
        .map(|s| (s.path.clone(), s.created_at, s.last_opened_at))
        .collect();
    assert_eq!(
        before, after,
        "the backfill must not refresh last_opened_at or created_at"
    );

    let _ = std::fs::remove_dir_all(&base);
}
