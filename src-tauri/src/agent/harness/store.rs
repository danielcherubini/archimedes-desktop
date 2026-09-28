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

/// The native session store (the provider transcript on the shared `Db`).
#[derive(Clone)]
pub struct SessionStore(Arc<Db>);

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
