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

use crate::agent::SessionInfo;

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
    /// `'user' | 'agent-text' | 'agent-thought' | 'tool-call' | 'diff'`.
    pub kind: String,
    /// `ContentChunk.messageId` for agent-text; the tool call id for
    /// tool-call rows; `NULL` otherwise.
    pub message_key: Option<String>,
    /// The message body, serialized (camelCase).
    pub payload_json: String,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// A row from the `spaces` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    pub path: String,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds.
    pub last_opened_at: i64,
    /// Trusted Space flag (untrusted by default; fail-closed reads).
    pub trusted: bool,
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
        // One-time migration for pre-existing databases: add `trusted`
        // (fresh databases already have it from SCHEMA). Gated on a schema
        // pre-check — the ALTER runs only when the column is absent, so a
        // re-open never issues it and no error string is matched (the
        // backfill below, by contrast, is idempotent via DO NOTHING).
        let has_trusted: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('spaces') WHERE name = 'trusted'",
            [],
            |row| row.get(0),
        )?;
        if has_trusted == 0 {
            conn.execute(
                "ALTER TABLE spaces ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        // One-time backfill for pre-existing databases: give a space row to every
        // distinct stored session cwd (canonicalized). A vanished folder is skipped
        // silently — its sessions stay stored, just without a space.
        // DO NOTHING on conflict: the backfill must NOT refresh last_opened_at on
        // every open (that would collapse the sidebar's recent-first ordering to a
        // tie on every restart — recency is refreshed by `upsert_space`, which runs
        // on real starts/resumes, i.e. Task 3's `record_session` hook).
        //
        // Key regime: rows are keyed by CANONICAL path — every upsert source
        // canonicalizes (`upsert_space` is fed `info.cwd`, canonicalized at
        // `start_session`/`resume_session`; the backfill above canonicalizes too),
        // so this migration intentionally does NOT re-key pre-existing non-canonical
        // rows. A hypothetical legacy non-canonical row is fail-closed: the
        // canonical lookup never matches it, so it can only cause MORE prompts,
        // never fewer.
        let cwds: Vec<String> = {
            let mut stmt = conn.prepare("SELECT DISTINCT cwd FROM sessions")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for cwd in cwds {
            if let Ok(c) = std::fs::canonicalize(&cwd) {
                let p = c.display().to_string();
                let now = now_ms();
                conn.execute(
                    "INSERT INTO spaces (path, created_at, last_opened_at) VALUES (?1, ?2, ?2)
                     ON CONFLICT(path) DO NOTHING",
                    params![p, now],
                )?;
            }
        }
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

    /// One stored session by id (`None` when absent) — the resume path
    /// reads the stored `capabilities_json` (the `piSessionFile` is the
    /// `--session` argument of the resume spawn).
    pub fn session(&self, id: &str) -> Result<Option<SessionRow>, DbError> {
        let guard = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = guard
            .prepare("SELECT id, agent_id, cwd, created_at, title, capabilities_json FROM sessions WHERE id = ?1")?;
        let row = stmt
            .query_map(params![id], |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    cwd: row.get(2)?,
                    created_at: row.get(3)?,
                    title: row.get(4)?,
                    capabilities_json: row.get(5)?,
                })
            })?
            .next()
            .transpose()?;
        Ok(row)
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

    /// Delete a session's messages.
    ///
    /// `resume_session` calls this before `session/load`: the agent's
    /// restored replay is treated as the authoritative history, so a
    /// replay that reuses (or changes) a `messageId` replaces the stored
    /// transcript instead of corrupting or duplicating it.
    pub fn clear_messages_for(&self, session_id: &str) -> Result<(), DbError> {
        self.conn.lock().expect("db mutex poisoned").execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Delete a session; its messages are removed by `ON DELETE CASCADE`.
    pub fn delete_session(&self, session_id: &str) -> Result<(), DbError> {
        self.conn
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
        Ok(())
    }

    /// Insert or touch the bookkeeping row for a folder.
    ///
    /// Keyed by the canonical path (the single key form, ADR 0010) — the
    /// input is canonicalized here, falling back to the raw string when the
    /// folder no longer exists (a best-effort write; reads stay fail-closed).
    /// `created_at` is preserved on conflict; `last_opened_at` is always
    /// refreshed (an actual start/resume is a real "open"). No folder
    /// validation happens here — whether the folder exists is the caller's
    /// problem (the canonicalizing gate is in `start_session`/`space_for_path`).
    pub fn upsert_space(&self, path: &str) -> Result<(), DbError> {
        let key = space_key(path).unwrap_or_else(|| path.to_string());
        self.conn.lock().expect("db mutex poisoned").execute(
            "INSERT INTO spaces (path, created_at, last_opened_at) VALUES (?1, ?2, ?2)
                 ON CONFLICT(path) DO UPDATE SET last_opened_at = excluded.last_opened_at",
            params![key, now_ms()],
        )?;
        Ok(())
    }

    /// Set (or clear) a space's trust flag, keyed by the canonical path
    /// (raw fallback on canonicalize failure, as in `upsert_space`).
    ///
    /// Returns `true` when a row matched, `false` when nothing matched (a
    /// missing row is a no-op, not an error — but a diagnosable one, not a
    /// silent one).
    pub fn set_space_trusted(&self, path: &str, trusted: bool) -> Result<bool, DbError> {
        let key = space_key(path).unwrap_or_else(|| path.to_string());
        let n = self.conn.lock().expect("db mutex poisoned").execute(
            "UPDATE spaces SET trusted = ?2 WHERE path = ?1",
            params![key, trusted],
        )?;
        Ok(n > 0)
    }

    /// All spaces, most recently opened first.
    pub fn list_spaces(&self) -> Result<Vec<SpaceRow>, DbError> {
        let guard = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = guard.prepare(
            "SELECT path, created_at, last_opened_at, trusted
             FROM spaces
             ORDER BY last_opened_at DESC, path ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(SpaceRow {
                    path: row.get(0)?,
                    created_at: row.get(1)?,
                    last_opened_at: row.get(2)?,
                    trusted: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Look up one space row by exact stored path (`None` if absent).
    pub fn find_space(&self, path: &str) -> Result<Option<String>, DbError> {
        let guard = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = guard.prepare("SELECT path FROM spaces WHERE path = ?1")?;
        let found = match stmt.query_row(params![path], |row| row.get::<_, String>(0)) {
            Ok(path) => Some(path),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        Ok(found)
    }

    /// The trust flag for the space at `cwd`, keyed by the canonical path
    /// (the single key form, ADR 0010). Fail-closed: a canonicalize failure
    /// or a missing row is `Ok(false)` — untrusted.
    pub fn space_trusted(&self, cwd: &std::path::Path) -> Result<bool, DbError> {
        // Fail-closed on canonicalize failure (unlike the write path's raw
        // fallback): a folder that can't be canonicalized is untrusted.
        let Some(key) = space_key(cwd) else {
            return Ok(false);
        };
        let guard = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = guard.prepare("SELECT trusted FROM spaces WHERE path = ?1")?;
        let found = stmt
            .query_map(params![key], |row| row.get::<_, i64>(0))?
            .next()
            .transpose()?;
        Ok(found.map(|t| t != 0).unwrap_or(false))
    }

    /// Delete the bookkeeping row only (conversations/messages are NOT touched).
    pub fn delete_space(&self, path: &str) -> Result<(), DbError> {
        self.conn
            .lock()
            .expect("db mutex poisoned")
            .execute("DELETE FROM spaces WHERE path = ?1", params![path])?;
        Ok(())
    }
}

/// The current unix time in milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The single key form of a `spaces` row (ADR 0010): the canonical path.
/// `None` when the path can't be canonicalized (the folder is missing, or
/// the filesystem changed) — the write path falls back to the raw string
/// (best-effort), the read path fails closed.
fn space_key(path: impl AsRef<std::path::Path>) -> Option<String> {
    std::fs::canonicalize(path)
        .ok()
        .map(|c| c.display().to_string())
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
CREATE TABLE IF NOT EXISTS spaces (
    path TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    last_opened_at INTEGER NOT NULL,
    trusted INTEGER NOT NULL DEFAULT 0
);
"#;
