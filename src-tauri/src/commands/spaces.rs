//! Tauri commands for the space and agent registries (read-only views plus
//! the dialog's folder check). See `docs/roadmap/spaces.md` Task 4.
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::agent::SessionManager;
use crate::storage::Db;

/// A registry entry (camelCase over IPC) — the agent dropdown's data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEntryDto {
    pub id: String,
    pub name: String,
}

/// `list_agents` — every configured agent (v1 default registry: single
/// `pi` entry). Powers the dialog dropdown; default selection is
/// `agents[0]` (a decision in the frontend: `pi` first, no `fake` default).
#[tauri::command]
pub async fn list_agents(
    state: State<'_, Arc<SessionManager>>,
) -> Result<Vec<AgentEntryDto>, String> {
    Ok(state
        .agents()
        .iter()
        .map(|e| AgentEntryDto {
            id: e.id.clone(),
            name: e.name.clone(),
        })
        .collect())
}

/// The folder check + canonicalizer for the new-space dialog.
///
/// Returns the CANONICAL path and whether a space row already exists for
/// it. A typed `~/x` or a missing folder is an ERROR ("no such folder"),
/// not a `None`: the dialog shows it inline instead of letting a broken
/// path travel to `start_session`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCheck {
    pub canonical_path: String,
    pub is_space: bool,
}

#[tauri::command]
pub async fn space_for_path(state: State<'_, Arc<Db>>, path: String) -> Result<SpaceCheck, String> {
    let canonical = std::fs::canonicalize(&path).map_err(|_| format!("no such folder: {path}"))?;
    if !canonical.is_dir() {
        return Err(format!("not a folder: {path}"));
    }
    let p = canonical.display().to_string();
    let is_space = state.find_space(&p).map_err(|e| e.to_string())?.is_some();
    Ok(SpaceCheck {
        canonical_path: p,
        is_space,
    })
}

/// All spaces, most recently opened first.
#[tauri::command]
pub async fn list_spaces(
    state: State<'_, Arc<Db>>,
) -> Result<Vec<crate::storage::SpaceRow>, String> {
    state.list_spaces().map_err(|e| e.to_string())
}

/// "Forget this space" — deletes the bookkeeping row only (conversations
/// stay stored; design decision). No-op if the row is already gone.
#[tauri::command]
pub async fn delete_space(state: State<'_, Arc<Db>>, path: String) -> Result<(), String> {
    state.delete_space(&path).map_err(|e| e.to_string())
}
