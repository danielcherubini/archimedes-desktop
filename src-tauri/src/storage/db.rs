//! SQLite persistence for session history and message transcripts.
//!
//! The database lives at `app.path().app_data_dir()/archimedes.db`. The
//! client owns history: every session is recorded when it is established
//! (or resumed), and every message is upserted as it streams in, so a
//! restart restores the full conversation with no duplicate rows.
//!
//! Upsert semantics: `messages` is keyed by `(session_id, kind,
//! message_key)`. Agent text is stored once per message (one row per
//! `messageId`, holding the accumulated text — not one row per chunk);
//! tool calls are stored once per tool call id (the latest merged state).

use std::path::Path;
use std::sync::Mutex as StdMutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::acp::SessionInfo;

/// Errors surfaced by the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// A row from the `sessions` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    /// The ACP session id (primary key).
    pub id: String,
    pub agent_id: String,
    pub cwd: String,
    /// Unix milliseconds.
    pub created_at: i64,
    pub title: Option<String>,
    /// The negotiated `agent_capabilities`, serialized (camelCase).
    pub capabilities_json: String,
}

/// A row from the `messages` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: i64,
    pub session_id: String,
    /// `'user' | 'agent-text' | 'tool-call' | 'diff'`.
    pub kind: String,
    /// `ContentChunk.messageId` for agent-text; the tool call id for
    /// tool-call rows; `NULL` otherwise.
    pub message_key: Option<String>,
    /// The message body, serialized (camelCase).
    pub payload_json: String,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// The app's SQLite database.
///
/// The connection is wrapped in a `std::sync::Mutex` (a raw
/// `rusqlite::Connection` is `!Sync`), and the `Db` itself is shared via
/// `Arc` between Tauri commands and driver tasks.
pub struct Db {
    conn: StdMutex<Connection>,
}

impl Db {
    /// Open (creating if needed) the database at `path`, migrating the
    /// schema. Parent directories are created as needed.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        // Enforce the ON DELETE CASCADE on messages.
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: StdMutex::new(conn),
        })
    }

    /// Record (or refresh) a session.
    ///
    /// Upserts by session id: `created_at` and `title` are preserved on a
    /// re-record (e.g. a resume), while agent_id / cwd / capabilities are
    /// refreshed.
    pub fn record_session(&self, info: &SessionInfo) -> Result<(), DbError> {
        let capabilities = serde_json::to_string(&info.capabilities)?;
        self.conn.lock().expect("db mutex poisoned").execute(
            "INSERT INTO sessions (id, agent_id, cwd, created_at, title, capabilities_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               agent_id = excluded.agent_id,
               cwd = excluded.cwd,
               capabilities_json = excluded.capabilities_json",
            params![
                info.session_id.to_string(),
                info.agent_id,
                info.cwd.display().to_string(),
                now_ms(),
                Option::<String>::None,
                capabilities,
            ],
        )?;
        Ok(())
    }

    /// Insert or update a message.
    ///
    /// `INSERT ... ON CONFLICT(session_id, kind, message_key) DO UPDATE` —
    /// the same `(session_id, kind, message_key)` triple always collapses to
    /// one row, so re-sent or replayed updates never duplicate.
    pub fn record_message(
        &self,
        session_id: &str,
        kind: &str,
        message_key: Option<&str>,
        payload_json: &str,
    ) -> Result<(), DbError> {
        self.conn.lock().expect("db mutex poisoned").execute(
            "INSERT INTO messages (session_id, kind, message_key, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, kind, message_key) DO UPDATE SET
               payload_json = excluded.payload_json,
               created_at = excluded.created_at",
            params![session_id, kind, message_key, payload_json, now_ms()],
        )?;
        Ok(())
    }

    /// All stored sessions, newest first.
    pub fn list_sessions(&self) -> Result<Vec<SessionRow>, DbError> {
        let guard = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = guard.prepare(
            "SELECT id, agent_id, cwd, created_at, title, capabilities_json
             FROM sessions
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    cwd: row.get(2)?,
                    created_at: row.get(3)?,
                    title: row.get(4)?,
                    capabilities_json: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// A session's messages in insertion order (the transcript order).
    pub fn messages_for(&self, session_id: &str) -> Result<Vec<MessageRow>, DbError> {
        let guard = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = guard.prepare(
            "SELECT id, session_id, kind, message_key, payload_json, created_at
             FROM messages
             WHERE session_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![session_id], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    kind: row.get(2)?,
                    message_key: row.get(3)?,
                    payload_json: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete a session; its messages are removed by `ON DELETE CASCADE`.
    pub fn delete_session(&self, session_id: &str) -> Result<(), DbError> {
        self.conn
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
        Ok(())
    }
}

/// The current unix time in milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u128::from(d.as_millis()) as i64)
        .unwrap_or(0)
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    cwd TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    title TEXT,
    capabilities_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    message_key TEXT,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, kind, message_key)
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, id);
"#;
