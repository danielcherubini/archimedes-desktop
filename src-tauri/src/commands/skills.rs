//! The skill catalog over IPC (read-only — the desktop is a READER of skill
//! directories, never a writer; ADR 0013).
use std::path::Path;

use crate::skills::SkillInfo;

/// The skill catalog for a Space (its roots + the user-level roots).
/// `space_path: None` → user-level skills only. Never fails: discovery is
/// total (a missing root / unreadable file is skipped, Task 1) — the `Err`
/// arm is for the unexpected.
#[tauri::command]
pub async fn list_skills(space_path: Option<String>) -> Result<Vec<SkillInfo>, String> {
    Ok(crate::skills::discover_skills(
        space_path.as_deref().map(Path::new),
    ))
}
