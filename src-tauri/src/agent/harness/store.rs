//! The native session store (native-agent-harness Task 6,
//! reviewer-corrected Major #13 + Minor #29): the provider transcript in
//! SQLite. `storage/db.rs` is a **single `rusqlite::Connection` behind a
//! `std::sync::Mutex`** (NOT a pool), so the `SessionStore` wraps the
//! manager's `Arc<Db>` (the production wiring constructs it from the
//! `SessionManager::attach_db` seam in the native start path).
//!
//! The `native_messages` table stores the **provider transcript** —
//! `content_json` is the serialized full `ChatMessage` (role + content,
//! `tool_calls`, and `tool_call_id`; the `role` column is a DENORMALIZED
//! index for cheap queries, but `load_messages` reconstructs from
//! `content_json`, NOT the `role` column alone, so tool-call arguments
//! and tool-result ids round-trip). The existing `messages` table is
//! deliberately NOT reused: its `payload_json` values are **display
//! shapes** (e.g. `{"text": "Hello"}`) — lossy for reconstructing
//! provider `ChatMessage`s — and it has no `seq` column (`record_message`
//! is an upsert keyed `(session_id, kind, message_key)`).
//!
//! `clear_messages` clears `native_messages` ONLY (the existing
//! `Db::clear_messages_for` is `DELETE FROM messages` and never touches
//! `native_messages` — and the native `resume_session` branch must NOT
//! clear before `load_messages` anyway: resume = `load_messages`).
//! Display rows keep flowing through the existing `record_message`
//! (the `AgentLoop` calls both).

use std::sync::Arc;

use crate::agent::harness::provider::ChatMessage;
use crate::storage::{Db, DbError};

/// One display `messages`-table upsert (the `Db::record_message` shape).
/// `Serialize` / `Deserialize` (ADR 0025 Task 2): the `DisplayUpsert` wire
/// frame carries these verbatim (`IpcStore` → the Supervisor's persister).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisplayRow {
    pub kind: String,
    pub message_key: String,
    pub payload_json: serde_json::Value,
    pub created_at: i64,
}

/// The transcript persistence seam (ADR 0025): `SqliteStore` (the current
/// `SessionStore`, unchanged behavior) / `NoopStore` (tests) / `IpcStore`
/// (Task 2 — the Worker: frames every call over the protocol; the
/// Supervisor applies the frames — the sole writer).
pub trait Store: Send + Sync {
    fn insert_message(
        &self,
        session_id: &str,
        seq: u64,
        role: &str,
        content_json: &str,
    ) -> Result<(), DbError>;
    fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, DbError>;
    fn clear_messages(&self, session_id: &str) -> Result<(), DbError>;
    fn replace_messages(
        &self,
        session_id: &str,
        rows: &[(u64, String, String)],
    ) -> Result<(), DbError>;
    /// The display persistence. Takes PRE-COMPUTED rows (the loop keeps
    /// the `TurnState` accumulators — `text_acc` / `tool_state` /
    /// `thought_state`): `SqliteStore` writes them via
    /// `db.record_message` (the write half of today's `persist_update`),
    /// `IpcStore` frames them (`DisplayUpsert`), `NoopStore` ignores them.
    ///
    /// Tries EVERY row (a failed row never stops the rest — the old
    /// `persist_update`'s `let _ =` semantics) and reports the first
    /// error (the caller's handling is unchanged: a failure is logged /
    /// ignored per the call site).
    fn persist_display(&self, session_id: &str, rows: &[DisplayRow]) -> Result<(), DbError>;
    /// The context-usage persistence (the `sessions.context_usage_json`
    /// write — today `db.record_session_context_usage(used, window)`;
    /// a missing `sessions` row is a no-op, not an error).
    fn record_context_usage(&self, session_id: &str, used: u64, window: u64)
        -> Result<(), DbError>;
}

/// The native session store (the provider transcript on the shared `Db`).
#[derive(Clone)]
pub struct SessionStore(Arc<Db>);

impl Store for SessionStore {
    // The 4 existing methods delegate verbatim (the explicit `SessionStore::`
    // path picks the INHERENT method — never the trait one).
    fn insert_message(
        &self,
        session_id: &str,
        seq: u64,
        role: &str,
        content_json: &str,
    ) -> Result<(), DbError> {
        SessionStore::insert_message(self, session_id, seq, role, content_json)
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, DbError> {
        SessionStore::load_messages(self, session_id)
    }

    fn clear_messages(&self, session_id: &str) -> Result<(), DbError> {
        SessionStore::clear_messages(self, session_id)
    }

    fn replace_messages(
        &self,
        session_id: &str,
        rows: &[(u64, String, String)],
    ) -> Result<(), DbError> {
        SessionStore::replace_messages(self, session_id, rows)
    }

    /// The display write half (today's `persist_update`'s `db.record_message`
    /// loop, moved verbatim): tries EVERY row (a failed row never stops
    /// the rest — the `let _ =` semantics), reports the first error.
    fn persist_display(&self, session_id: &str, rows: &[DisplayRow]) -> Result<(), DbError> {
        let mut first_err = None;
        for row in rows {
            if let Err(e) = self.0.record_message(
                session_id,
                &row.kind,
                Some(&row.message_key),
                &row.payload_json.to_string(),
            ) {
                first_err.get_or_insert(e);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn record_context_usage(
        &self,
        session_id: &str,
        used: u64,
        window: u64,
    ) -> Result<(), DbError> {
        self.0
            .record_session_context_usage(session_id, used, window)
    }
}

/// The no-op store (tests — never errors, never panics; `load_messages`
/// yields an empty transcript, the `IpcStore`'s write-only counterpart is
/// Task 2's).
pub struct NoopStore;

impl Store for NoopStore {
    fn insert_message(
        &self,
        _session_id: &str,
        _seq: u64,
        _role: &str,
        _content_json: &str,
    ) -> Result<(), DbError> {
        Ok(())
    }

    fn load_messages(&self, _session_id: &str) -> Result<Vec<ChatMessage>, DbError> {
        Ok(Vec::new())
    }

    fn clear_messages(&self, _session_id: &str) -> Result<(), DbError> {
        Ok(())
    }

    fn replace_messages(
        &self,
        _session_id: &str,
        _rows: &[(u64, String, String)],
    ) -> Result<(), DbError> {
        Ok(())
    }

    fn persist_display(&self, _session_id: &str, _rows: &[DisplayRow]) -> Result<(), DbError> {
        Ok(())
    }

    fn record_context_usage(
        &self,
        _session_id: &str,
        _used: u64,
        _window: u64,
    ) -> Result<(), DbError> {
        Ok(())
    }
}

impl SessionStore {
    pub fn new(db: Arc<Db>) -> Self {
        Self(db)
    }

    /// The underlying `Db` (the display `record_message` persistence).
    pub fn db(&self) -> &Arc<Db> {
        &self.0
    }

    /// Insert or refresh one transcript message (idempotent — the same
    /// `(session_id, seq)` always collapses to one row). `seq` is a
    /// message index (it cannot realistically exceed `i64::MAX`).
    pub fn insert_message(
        &self,
        session_id: &str,
        seq: u64,
        role: &str,
        content_json: &str,
    ) -> Result<(), DbError> {
        self.0
            .insert_native_message(session_id, seq as i64, role, content_json)
    }

    /// The session's transcript in `seq` order (reconstruct the
    /// `messages` vec for resume). A malformed row is a `Json` error —
    /// a corrupt transcript must not silently load as an empty one.
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, DbError> {
        let rows = self.0.load_native_messages(session_id)?;
        rows.into_iter()
            .map(|row| serde_json::from_str(&row).map_err(DbError::Json))
            .collect()
    }

    /// Clear the session's transcript (`native_messages` ONLY — the
    /// display `messages` table is NOT touched).
    pub fn clear_messages(&self, session_id: &str) -> Result<(), DbError> {
        self.0.clear_native_messages(session_id)
    }

    /// Replace the session's transcript ATOMICALLY (the `run_compaction`
    /// rewrite: a single `Db` transaction — a crash mid-rewrite must
    /// never leave an empty / partial transcript). `seq` is a message
    /// index (it cannot realistically exceed `i64::MAX`).
    pub fn replace_messages(
        &self,
        session_id: &str,
        rows: &[(u64, String, String)],
    ) -> Result<(), DbError> {
        let rows: Vec<(i64, String, String)> = rows
            .iter()
            .map(|(seq, role, content_json)| (*seq as i64, role.clone(), content_json.clone()))
            .collect();
        self.0.replace_native_messages(session_id, &rows)
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayRow, NoopStore, SessionStore, Store};
    use crate::agent::harness::provider::ChatMessage;
    use crate::agent::SessionInfo;
    use crate::storage::Db;
    use std::sync::Arc;

    /// A temp-dir `Db` with a recorded `sessions` row (the `native_messages`
    /// / `messages` rows FK to `sessions`).
    fn temp_db() -> (Arc<Db>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("harness-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Db::open(&dir.join("t.db")).expect("db should open"));
        db.record_session(&SessionInfo {
            session_id: "s1".to_string(),
            cwd: std::path::PathBuf::from("/tmp"),
            capabilities: serde_json::json!({}),
            config_options: None,
            archived: false,
            context_usage: None,
            is_subagent: false,
        })
        .expect("record_session");
        (db, dir)
    }

    /// `NoopStore` — all 6 seam methods return `Ok(())` (never errors,
    /// never panics; `load_messages` yields an empty transcript).
    #[test]
    fn noop_store_all_methods_ok() {
        let store: Arc<dyn Store> = Arc::new(NoopStore);
        assert!(store.insert_message("s1", 0, "user", "{}").is_ok());
        let loaded = store.load_messages("s1").expect("load never errors");
        assert!(
            loaded.is_empty(),
            "the noop store yields an empty transcript"
        );
        assert!(store.clear_messages("s1").is_ok());
        assert!(store
            .replace_messages("s1", &[(0, "user".to_string(), "{}".to_string())])
            .is_ok());
        assert!(store
            .persist_display(
                "s1",
                &[
                    DisplayRow {
                        kind: "agent-text".to_string(),
                        message_key: "m1".to_string(),
                        payload_json: serde_json::json!({ "text": "hi" }),
                        created_at: 1,
                    },
                    DisplayRow {
                        kind: "tool-call".to_string(),
                        message_key: "t1".to_string(),
                        payload_json: serde_json::json!({}),
                        created_at: 2,
                    },
                ]
            )
            .is_ok());
        assert!(store.record_context_usage("s1", 100, 10_000).is_ok());
    }

    /// `SessionStore` as a `Store`: `insert_message` / `load_messages`
    /// round-trip against a temp-dir `Db` (the idempotent upsert — the
    /// same `(session_id, seq)` collapses to one row).
    #[test]
    fn session_store_insert_load_round_trip() {
        let (db, dir) = temp_db();
        let store = SessionStore::new(db.clone());
        let msg = ChatMessage {
            role: crate::agent::harness::provider::ChatRole::User,
            content: crate::agent::harness::provider::MessageContent::Text("hello".to_string()),
            tool_call_id: None,
            tool_calls: None,
        };
        let content_json = serde_json::to_string(&msg).unwrap();
        store
            .insert_message("s1", 0, "user", &content_json)
            .unwrap();
        // An idempotent re-insert (the same seq) collapses to one row.
        store
            .insert_message("s1", 0, "user", &content_json)
            .unwrap();
        let loaded = store.load_messages("s1").unwrap();
        assert_eq!(loaded.len(), 1, "the upsert is idempotent (one row)");
        assert_eq!(loaded[0], msg, "the content round-trips verbatim");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `persist_display` — a `Vec<DisplayRow>` lands as `messages` rows via
    /// `db.record_message` (the upsert idempotency: a re-send with an
    /// updated payload refreshes the SAME row, never duplicates).
    #[test]
    fn session_store_persist_display_upserts_rows() {
        let (db, dir) = temp_db();
        let store = SessionStore::new(db.clone());
        store
            .persist_display(
                "s1",
                &[
                    DisplayRow {
                        kind: "agent-text".to_string(),
                        message_key: "m1".to_string(),
                        payload_json: serde_json::json!({ "text": "Hel" }),
                        created_at: 1,
                    },
                    DisplayRow {
                        kind: "tool-call".to_string(),
                        message_key: "t1".to_string(),
                        payload_json: serde_json::json!({ "title": "run" }),
                        created_at: 2,
                    },
                ],
            )
            .unwrap();
        let rows = db.messages_for("s1").unwrap();
        assert_eq!(rows.len(), 2, "two rows landed");
        let text = rows.iter().find(|r| r.kind == "agent-text").unwrap();
        assert_eq!(text.message_key.as_deref(), Some("m1"));
        assert_eq!(text.payload_json, r#"{"text":"Hel"}"#);
        // The accumulator grows: a re-send with the updated payload
        // refreshes the SAME row (the upsert idempotency).
        store
            .persist_display(
                "s1",
                &[
                    DisplayRow {
                        kind: "agent-text".to_string(),
                        message_key: "m1".to_string(),
                        payload_json: serde_json::json!({ "text": "Hello" }),
                        created_at: 3,
                    },
                    DisplayRow {
                        kind: "tool-call".to_string(),
                        message_key: "t1".to_string(),
                        payload_json: serde_json::json!({ "title": "run", "status": "done" }),
                        created_at: 4,
                    },
                ],
            )
            .unwrap();
        let rows = db.messages_for("s1").unwrap();
        assert_eq!(rows.len(), 2, "the upsert never duplicates");
        let text = rows.iter().find(|r| r.kind == "agent-text").unwrap();
        assert_eq!(
            text.payload_json, r#"{"text":"Hello"}"#,
            "the payload was refreshed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `record_context_usage` — lands in `sessions.context_usage_json`
    /// (the `Db::record_session_context_usage` semantics — a missing
    /// `sessions` row is a no-op, not an error).
    #[test]
    fn session_store_record_context_usage_lands_in_the_sessions_row() {
        let (db, dir) = temp_db();
        let store = SessionStore::new(db.clone());
        store.record_context_usage("s1", 53_760, 128_000).unwrap();
        let row = db.session("s1").unwrap().unwrap();
        let usage: serde_json::Value =
            serde_json::from_str(&row.context_usage_json.unwrap()).expect("the usage is JSON");
        assert_eq!(usage["used"], 53_760);
        assert_eq!(usage["window"], 128_000);
        // A missing `sessions` row is a no-op (the `UPDATE` matches nothing).
        assert!(store.record_context_usage("missing-session", 1, 10).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
