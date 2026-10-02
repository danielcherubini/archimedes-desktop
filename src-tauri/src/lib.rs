pub mod agent;
pub mod agents;
pub mod commands;
pub mod config;
pub mod skills;
pub mod storage;
#[doc(hidden)]
pub mod test_support;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::Manager;

use crate::agent::{EventSink, SessionManager, SubagentSessionManager};
use crate::storage::Db;

/// The shared app setup: agent registry from `config_dir`, persistence
/// database in `app_data_dir/archimedes.db`.
///
/// Factored out of [`run`] so the real command surface can be exercised in
/// tests without a GUI (see `tests/ipc.rs`).
pub fn setup_dirs<R: tauri::Runtime>(
    app: &tauri::App<R>,
    config_dir: PathBuf,
    app_data_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    // The agent registry lives in the app's config directory.
    let mut manager = SessionManager::new(config_dir.clone())
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    // The persistence database lives in the app data directory.
    let db = Arc::new(
        Db::open(&app_data_dir.join("archimedes.db"))
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
    );
    manager.attach_db(db.clone());
    // The subagent manager (its own `SessionDriver` + a dedicated
    // `WorkerRuntime`, built in `new()` — two idle threads, negligible).
    // Injected into the main manager (the main session's bridge listener
    // services `dispatch_subagent` frames); the injection order breaks the
    // apparent cycle: the subagent manager needs nothing from the main
    // manager; only the main manager's `SessionDriver.subagent` field points
    // at it.
    let subagent_manager = Arc::new(
        // The trust db is threaded (ADR 0010 — subagent Sessions inherit
        // Space trust; the subagent's driver `db` stays `None` — ephemeral).
        SubagentSessionManager::new(config_dir, Some(db.clone()))
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
    );
    manager.set_subagent_manager(subagent_manager.clone());
    // The event sink (TauriSink) is managed state so commands — and the
    // headless IPC test — can obtain it without an `AppHandle` parameter.
    //
    // It must be managed under the TRAIT-OBJECT type `Arc<dyn EventSink>`:
    // the commands resolve it as `State<Arc<dyn EventSink>>` and Tauri's
    // state registry is keyed by TypeId — managing the concrete
    // `Arc<TauriSink<R>>` registers a different key, and `start_session`
    // fails at runtime with "state not managed for field `sink`".
    // (`Arc::<dyn EventSink>::new` doesn't compile because `dyn EventSink`
    // is unsized, so the coercion is spelled as an annotated binding.)
    let sink: Arc<dyn EventSink> = Arc::new(commands::sessions::TauriSink(app.handle().clone()));
    app.manage(sink);
    // The manager is `Sync` (its mutable state is `Arc<Mutex<…>>`
    // internally), so it is shared directly without an outer lock.
    app.manage(Arc::new(manager));
    // The subagent manager is `Send + Sync` (same reasoning — the
    // `WorkerRuntime` fields are `Send` + `Sync`), so it is managed
    // directly too (the `respond_*` commands resolve it as state).
    app.manage(subagent_manager);
    app.manage(db);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::sessions::start_session,
            commands::sessions::send_prompt,
            commands::sessions::close_session,
            commands::sessions::respond_permission,
            commands::sessions::respond_bridge_request,
            commands::sessions::resume_session,
            commands::sessions::set_session_config_option,
            commands::sessions::cancel_session,
            commands::history::list_sessions,
            commands::history::load_history,
            commands::history::delete_session,
            commands::history::set_session_archived,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::list_models,
            commands::settings::test_mcp_server,
            commands::settings::auth_mcp_server,
            commands::spaces::list_agents,
            commands::spaces::list_spaces,
            commands::spaces::delete_space,
            commands::spaces::space_for_path,
            commands::spaces::set_space_trusted,
            commands::skills::list_skills,
            commands::clipboard::read_clipboard_image
        ])
        .setup(|app| {
            // The on-disk dirs are named `archimedes` (NOT the bundle
            // identifier — Tauri's `config_dir()` / `app_data_dir()`
            // append the identifier, which would give
            // `…/codes.archimedes.desktop`): `~/.config/archimedes` +
            // `~/.local/share/archimedes` (Linux), `~/Library/Application
            // Support/archimedes` (macOS), `%APPDATA%/archimedes` +
            // `%LOCALAPPDATA%/archimedes` (Windows). `dirs` resolves the
            // platform base dirs the same way Tauri does; a `None` base
            // means the platform home is undeterminable — a hard error
            // (the app has nowhere to persist state).
            let config_dir = dirs::config_dir()
                .map(|p| p.join("archimedes"))
                .ok_or_else(|| -> Box<dyn std::error::Error> {
                    "could not resolve the platform config dir".into()
                })?;
            let app_data_dir = dirs::data_dir().map(|p| p.join("archimedes")).ok_or_else(
                || -> Box<dyn std::error::Error> {
                    "could not resolve the platform data dir".into()
                },
            )?;
            setup_dirs(app, config_dir, app_data_dir)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
