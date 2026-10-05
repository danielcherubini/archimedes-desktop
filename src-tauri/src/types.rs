//! Crate-root shared types (ADR-agnostic value types that more than one
//! layer needs — `storage` and `agent` both use `SessionInfo`, so it lives
//! at the crate root rather than under either, which is what keeps
//! `storage` from importing `agent`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A fully established session, ready to accept prompts.
///
/// `Serialize` so it can cross the IPC boundary as a command return value.
///
/// `capabilities` is the pi session's capability envelope (item 1 of the
/// swap plan): `piSessionId` / `piSessionFile`? / `model`? /
/// `thinkingLevel` / `loadSession` / `promptCapabilities` — the `model` key
/// is ABSENT when `get_state` reports no model, and `piSessionFile` is
/// A live session's identity + its current configuration (what the UI shows
/// in the header / the model selector). Serialized camelCase over IPC.
///
/// `capabilities` is the native session's capability envelope (item 1 of the
/// swap plan): `model`? / `thinkingLevel` / `loadSession` / `promptCapabilities`
/// — the `model` key is ABSENT when the session has no model, and the two
/// trailing keys are load-bearing: `loadSession` gates the frontend's Resume
/// button and `promptCapabilities.image` gates image sending (fail-closed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: PathBuf,
    pub capabilities: Value,
    /// The session's configuration options (model / thinking level
    /// selectors) synthesized from the `ModelCatalog` (the native session's
    /// model / thinking-level selectors); `None` when they can't be
    /// synthesized (no catalog / unresolvable stored model).
    pub config_options: Option<Vec<Value>>,
    /// The desktop's archived flag (ADR 0016). `false` for a newly
    /// started or ephemeral session; the resume paths and
    /// `list_sessions` read it from the stored row.
    pub archived: bool,
    /// The session's last known context usage (the `context_usage_update`
    /// frame's values — the provider's `input_tokens` vs the model's
    /// window), persisted on the `sessions` row on every frame so a
    /// CLOSED session's context survives (the frontend's store drops the
    /// entry on close — the row is the source of truth for the stored
    /// session's context bar). `None` until the first frame (a fresh
    /// session with no usage yet).
    pub context_usage: Option<ContextUsage>,
    /// The ephemeral-subagent flag (ADR 0025 §3): `true` for a subagent
    /// session (its transcript persists as a HIDDEN `sessions` row —
    /// `list_sessions` filters `is_subagent = 0`). `false` for a main
    /// session. `#[serde(default)]` so stored rows pre-dating the column
    /// (and the `Value::Null`-ish ephemeral envelope) deserialize `false`.
    #[serde(default)]
    pub is_subagent: bool,
}

/// A session's last known context usage (the `context_usage_update` frame's
/// values — the provider's `input_tokens` vs the model's `context_window`).
/// Persisted on the `sessions` row on every frame (the row's
/// `context_usage_json` column — `{"used": n, "window": n}`) so a CLOSED
/// session's context survives: the frontend's store drops the entry on
/// close, but the row keeps the last known value for the stored session's
/// context bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used: u64,
    pub window: u64,
}
