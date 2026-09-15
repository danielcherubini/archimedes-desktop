pub mod acp;
pub mod commands;
pub mod config;
pub mod storage;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

use crate::acp::SessionManager;
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
    let mut manager =
        SessionManager::new(config_dir).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    // The persistence database lives in the app data directory.
    let db = Arc::new(
        Db::open(&app_data_dir.join("archimedes.db"))
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
    );
    manager.attach_db(db.clone());
    // The event sink (TauriSink) is managed state so commands — and the
    // headless IPC test — can obtain it without an `AppHandle` parameter.
    app.manage(Arc::new(commands::sessions::TauriSink(
        app.handle().clone(),
    )));
    app.manage(Mutex::new(manager));
    app.manage(db);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::sessions::start_session,
            commands::sessions::send_prompt,
            commands::sessions::close_session,
            commands::sessions::respond_permission,
            commands::sessions::resume_session,
            commands::history::list_sessions,
            commands::history::load_history,
            commands::history::delete_session,
            commands::settings::get_settings,
            commands::settings::save_settings
        ])
        .setup(|app| {
            let config_dir = app
                .path()
                .config_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            setup_dirs(app, config_dir, app_data_dir)?;
            // The "Check for updates" menu item asks the webview to run the
            // updater check (the updater plugin lives in the JS context).
            let check_item = MenuItem::with_id(
                app,
                "check-updates",
                "Check for updates",
                true,
                None::<&str>,
            )
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let menu = Menu::with_items(app, &[&check_item])
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            app.set_menu(menu)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "check-updates" {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("update-check-requested", ());
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
