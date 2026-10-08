//! The `#`-mention picker's MCP surface: the effective three-layer set
//! (ADR 0019) as a read-only name → (kind, summary) list. NO live
//! connect (the per-session `McpManager` rule, ADR 0018/0019, is
//! untouched) — the settings read is best-effort (a missing / corrupt
//! `settings.json` = the `load_settings` defaults = an empty desktop
//! layer).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::State;

use crate::agent::mcp::config::{load_servers, server_infos, McpServerInfo};
use crate::agent::SessionManager;
use crate::config::load_settings;

/// The EFFECTIVE MCP server set for `cwd` (the `#`-mention picker —
/// ADR 0019's three-layer merge, project > desktop > pi-global).
/// `cwd: None` = app-scope: the project layer degrades via `read_layer`'s
/// best-effort read of `/`/`.pi/mcp.json` (absent in practice on every
/// platform — the mechanism is the best-effort degradation, not a
/// guaranteed skip).
#[tauri::command]
pub async fn list_mcp_servers_effective(
    state: State<'_, Arc<SessionManager>>,
    cwd: Option<String>,
) -> Result<Vec<McpServerInfo>, String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let desktop = load_settings(state.config_dir()).mcp_servers;
    let project = cwd
        .as_deref()
        .map(Path::new)
        .unwrap_or_else(|| Path::new("/"));
    Ok(server_infos(&load_servers(&home, project, Some(&desktop))))
}
