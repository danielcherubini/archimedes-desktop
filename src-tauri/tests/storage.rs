//! Integration test for the SQLite persistence layer (Task 5).
//!
//! Covers: schema creation, session upsert, message upsert semantics (two
//! agent-text chunks sharing one message_key collapse to a single row with
//! the accumulated text), session listing, history fetch, and cascade
//! delete (deleting a session removes its messages).

use std::path::Path;
use std::path::PathBuf;

use archimedes_lib::agent::SessionInfo;
use archimedes_lib::storage::Db;

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

/// (finding 10) `native_messages` must NOT be orphaned on session delete:
/// deleting a session removes its native transcript (the `ON DELETE
/// CASCADE` foreign key — the `messages` cascade implies `PRAGMA
/// foreign_keys` is on), while an unrelated session's rows survive.
#[test]
fn delete_session_cascades_native_messages() {
    let path = temp_db_path();
    let db = Db::open(&path).expect("db should open");

    let session = sample_session(); // sess-1
    db.record_session(&session).expect("record_session sess-1");
    let other = SessionInfo {
        session_id: "sess-2".to_string(),
        ..session.clone()
    };
    db.record_session(&other).expect("record_session sess-2");

    // Transcript rows for BOTH sessions.
    db.insert_native_message("sess-1", 0, "user", r#"{"role":"user"}"#)
        .expect("insert native row 1");
    db.insert_native_message("sess-1", 1, "assistant", r#"{"role":"assistant"}"#)
        .expect("insert native row 2");
    db.insert_native_message("sess-2", 0, "user", r#"{"role":"user"}"#)
        .expect("insert native row 3");

    db.delete_session("sess-1")
        .expect("delete_session should succeed");
    assert!(
        db.load_native_messages("sess-1")
            .expect("load_native_messages sess-1")
            .is_empty(),
        "the deleted session's native_messages rows are gone (cascade)"
    );
    assert_eq!(
        db.load_native_messages("sess-2")
            .expect("load_native_messages sess-2")
            .len(),
        1,
        "an unrelated session's native_messages rows survive"
    );

    let _ = std::fs::remove_file(&path);
}

/// (finding 10) Pre-existing databases: a `native_messages` table that
/// predates the foreign key (no `REFERENCES`) must be migrated by
/// `Db::open` — the table is recreated WITH the `ON DELETE CASCADE` FK,
/// the pre-existing rows survive, and a session delete cascades.
#[test]
fn native_messages_fk_migrated_on_preexisting_databases() {
    let base = std::env::temp_dir().join(format!("archimedes-native-mig-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).expect("base dir");
    let path = base.join("archimedes.db");
    // A pre-migration database: the OLD `native_messages` (no FK) with a
    // row, plus a session row for it.
    {
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                cwd TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                title TEXT,
                capabilities_json TEXT NOT NULL
            );
            CREATE TABLE native_messages (
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                role TEXT NOT NULL,
                content_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                UNIQUE(session_id, seq)
            );",
        )
        .expect("old-schema tables");
        conn.execute(
            "INSERT INTO sessions (id, agent_id, cwd, created_at, capabilities_json)
             VALUES ('sess-1', 'native', '/tmp', 1, '{}');",
            [],
        )
        .expect("session row");
        conn.execute(
            "INSERT INTO native_messages (session_id, seq, role, content_json, created_at)
             VALUES ('sess-1', 0, 'user', '{}', 1);",
            [],
        )
        .expect("native row");
    }
    let db = Db::open(&path).expect("Db::open should migrate the native_messages table");
    assert_eq!(
        db.load_native_messages("sess-1")
            .expect("load_native_messages")
            .len(),
        1,
        "the pre-existing row survives the migration"
    );
    // The migrated table cascades.
    db.delete_session("sess-1").expect("delete_session");
    assert!(
        db.load_native_messages("sess-1")
            .expect("load_native_messages after delete")
            .is_empty(),
        "the migrated table cascades on session delete"
    );

    // A re-open: the migration is idempotent (the pre-check sees the FK
    // and skips the recreation) and the data is untouched.
    let db2 = Db::open(&path).expect("db should reopen (idempotent migration)");
    assert!(db2
        .load_native_messages("sess-1")
        .expect("load_native_messages reopen")
        .is_empty());

    let _ = std::fs::remove_dir_all(&base);
}

/// The `replace_native_messages` rewrite (the `run_compaction` seam):
/// the old rows are replaced by the new ones ATOMICALLY — a single
/// transaction (clear + reinsert), so a crash mid-rewrite never leaves
/// an empty / partial transcript.
#[test]
fn replace_native_messages_replaces_the_transcript() {
    let path = temp_db_path();
    let db = Db::open(&path).expect("db should open");
    db.record_session(&sample_session())
        .expect("record_session");
    // Three rows, then a 2-row replacement.
    db.insert_native_message("sess-1", 0, "user", r#"{"a":1}"#)
        .expect("insert 0");
    db.insert_native_message("sess-1", 1, "assistant", r#"{"a":2}"#)
        .expect("insert 1");
    db.insert_native_message("sess-1", 2, "tool", r#"{"a":3}"#)
        .expect("insert 2");
    db.replace_native_messages(
        "sess-1",
        &[
            (0, "system".to_string(), r#"{"a":"summary"}"#.to_string()),
            (1, "user".to_string(), r#"{"a":100}"#.to_string()),
        ],
    )
    .expect("replace_native_messages");
    let rows = db
        .load_native_messages("sess-1")
        .expect("load_native_messages");
    assert_eq!(
        rows.len(),
        2,
        "the old 3-row transcript is replaced by the new 2"
    );
    assert_eq!(rows[0], r#"{"a":"summary"}"#);
    assert_eq!(rows[1], r#"{"a":100}"#);

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

    db.upsert_space("/tmp/pa", false)
        .expect("upsert_space /tmp/pa");
    std::thread::sleep(std::time::Duration::from_millis(5));
    db.upsert_space("/tmp/pb", false)
        .expect("upsert_space /tmp/pb");

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
    db.upsert_space("/tmp/pa", false)
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
    db.upsert_space("/tmp/pb", false)
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

#[test]
fn spaces_trusted_column_fresh_and_migrated() {
    // (a) Fresh database: the column exists from SCHEMA. The dir is created
    // FIRST so `space_trusted`'s canonicalize succeeds and the assertion
    // actually exercises the fresh-DB default (a missing dir makes
    // canonicalize fail → fail-closed `Ok(false)` regardless of the default).
    let dir =
        std::env::temp_dir().join(format!("archimedes-trusted-fresh-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("fresh dir");
    let p = std::fs::canonicalize(&dir)
        .expect("canonical fresh dir")
        .display()
        .to_string();
    let fresh = temp_db_path();
    let db = Db::open(&fresh).expect("db should open");
    db.upsert_space(&p, false).expect("upsert_space");
    assert!(
        !db.space_trusted(Path::new(&p)).expect("space_trusted"),
        "a just-upserted space is untrusted by default"
    );
    let spaces = db.list_spaces().expect("list_spaces");
    assert_eq!(spaces.len(), 1);
    assert!(!spaces[0].trusted, "fresh row defaults to untrusted");
    let _ = std::fs::remove_file(&fresh);

    // (b) Pre-migration database: a `spaces` table with the OLD 3-column
    // schema, then `Db::open` must `ALTER TABLE` it into shape.
    let base = std::env::temp_dir().join(format!("archimedes-spaces-mig-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).expect("base dir");
    let cwd = base.join("proj");
    std::fs::create_dir_all(&cwd).expect("proj dir");
    let p = std::fs::canonicalize(&cwd)
        .expect("canonical proj")
        .display()
        .to_string();
    let db_path = base.join("archimedes.db");
    {
        let conn = rusqlite::Connection::open(&db_path).expect("raw open");
        conn.execute_batch(
            "CREATE TABLE spaces (
                path TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                last_opened_at INTEGER NOT NULL
            );",
        )
        .expect("old-schema table");
        conn.execute(
            "INSERT INTO spaces (path, created_at, last_opened_at) VALUES (?1, 1, 1);",
            rusqlite::params![p],
        )
        .expect("old-schema row");
    }
    let db2 = Db::open(&db_path).expect("Db::open should migrate the old table");
    let spaces = db2.list_spaces().expect("list_spaces");
    assert_eq!(
        spaces.len(),
        1,
        "the pre-existing row survives the migration"
    );
    assert!(
        !spaces[0].trusted,
        "the migrated row reads trusted = false (default 0)"
    );
    assert!(
        !db2.space_trusted(Path::new(&p)).expect("space_trusted"),
        "the migrated row is untrusted by default"
    );

    // (c) A third open: the migration is idempotent (the pre-check sees the
    // `trusted` column and skips the ALTER entirely) and the data is untouched.
    let db3 = Db::open(&db_path).expect("db should reopen (idempotent migration)");
    let spaces3 = db3.list_spaces().expect("list_spaces");
    assert_eq!(spaces3.len(), 1);
    assert!(!spaces3[0].trusted);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn set_space_trusted_updates_and_flag_reads() {
    let path = temp_db_path();
    let db = Db::open(&path).expect("db should open");
    let dir = std::env::temp_dir().join(format!("archimedes-trusted-t-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let p = std::fs::canonicalize(&dir)
        .expect("canonical dir")
        .display()
        .to_string();

    db.upsert_space(&p, false).expect("upsert_space");
    assert!(
        db.set_space_trusted(&p, true)
            .expect("set_space_trusted true"),
        "set reports true when a row matched"
    );
    assert!(
        db.space_trusted(Path::new(&p)).expect("space_trusted"),
        "the flag reads back true after set"
    );
    let spaces = db.list_spaces().expect("list_spaces");
    assert!(spaces[0].trusted, "list_spaces reflects the flag");

    assert!(
        db.set_space_trusted(&p, false)
            .expect("set_space_trusted false"),
        "clearing also matches the row"
    );
    assert!(!db.space_trusted(Path::new(&p)).expect("space_trusted"));
    assert!(!db.list_spaces().expect("list_spaces")[0].trusted);

    // No-op for a missing row: no error, but the bool says nothing matched
    // (a silent 0-row UPDATE is diagnosable, not invisible).
    assert!(
        !db.set_space_trusted("/no/such/row", true)
            .expect("set on a missing row is a no-op"),
        "a missing row updates nothing (false, not an error)"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn space_trusted_fail_closed() {
    let path = temp_db_path();
    let db = Db::open(&path).expect("db should open");
    // No row for the path: fail-closed, not an error.
    assert!(
        !db.space_trusted(Path::new("/tmp/never-upserted-anywhere"))
            .expect("space_trusted must not error on a missing row"),
        "a missing row is untrusted (fail-closed)"
    );
    // Canonicalize failure: fail-closed, not an error.
    let gone = temp_db_path().with_extension("does-not-exist");
    assert!(
        !db.space_trusted(&gone)
            .expect("space_trusted must not error on a canonicalize failure"),
        "a canonicalize failure is untrusted (fail-closed)"
    );
    let _ = std::fs::remove_file(&path);
}

/// The `spaces` table has ONE key form (ADR 0010: the canonical path), shared
/// by `upsert_space`, `set_space_trusted` and `space_trusted` — a write and a
/// read through a non-canonical form (here: a symlink) must agree on the row.
#[cfg(unix)]
#[test]
fn space_methods_agree_on_the_canonical_key() {
    let base = std::env::temp_dir().join(format!("archimedes-canonical-{}", uuid::Uuid::new_v4()));
    let real = base.join("real");
    std::fs::create_dir_all(&real).expect("real dir");
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let db_path = base.join("archimedes.db");
    let db = Db::open(&db_path).expect("db should open");

    // A write via the symlink path keys the row by the CANONICAL path —
    // never by the raw (non-canonical) string.
    db.upsert_space(&link.display().to_string(), false)
        .expect("upsert via symlink");
    let canonical = std::fs::canonicalize(&real).expect("canonical");
    let cp = canonical.display().to_string();
    assert!(
        db.find_space(&cp).expect("find canonical").is_some(),
        "the row is keyed by the canonical path"
    );
    assert!(
        db.find_space(&link.display().to_string())
            .expect("find symlink")
            .is_none(),
        "no row under the raw (non-canonical) key"
    );

    // A trust write via the symlink path hits the canonical row, and a read
    // via EITHER form sees the flag (they share the one key form).
    assert!(
        db.set_space_trusted(&link.display().to_string(), true)
            .expect("set via symlink"),
        "set matches the canonical row"
    );
    assert!(db.space_trusted(&link).expect("read via symlink"));
    assert!(db.space_trusted(&canonical).expect("read via canonical"));

    // A write for a path with no row: Ok(false) (diagnosable), not an error.
    let gone = base.join("never-existed");
    assert!(
        !db.set_space_trusted(&gone.display().to_string(), true)
            .expect("set on a missing row is not an error"),
        "a missing row updates nothing"
    );

    let _ = std::fs::remove_dir_all(&base);
}
