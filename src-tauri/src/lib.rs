pub mod acp;
mod commands;
pub mod config;

use tauri::Manager;
use tokio::sync::Mutex;

use crate::acp::SessionManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::sessions::start_session,
            commands::sessions::send_prompt,
            commands::sessions::close_session,
            commands::sessions::respond_permission
        ])
        .setup(|app| {
            // The agent registry lives in the app's config directory.
            let config_dir = app
                .path()
                .config_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let manager = SessionManager::new(config_dir)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            app.manage(Mutex::new(manager));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
