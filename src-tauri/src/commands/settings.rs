//! App settings: `settings.json` in the config dir.
//!
//! `{ "theme": "dark" | "light", "paneLayout": {...} }` with sane
//! defaults written on first run.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;
use tokio::sync::Mutex;

use crate::acp::SessionManager;

/// The persisted app settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// `"dark"` or `"light"`.
    pub theme: String,
    /// Free-form pane layout state (owned by the frontend).
    #[serde(default)]
    pub pane_layout: Value,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            pane_layout: Value::Object(Default::default()),
        }
    }
}

fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Read the settings, writing the defaults if the file does not exist yet.
#[tauri::command]
pub async fn get_settings(state: State<'_, Mutex<SessionManager>>) -> Result<Settings, String> {
    let config_dir = state.inner().lock().await.config_dir().clone();
    let path = settings_path(&config_dir);
    if !path.exists() {
        let settings = Settings::default();
        fs::write(
            &path,
            serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        return Ok(settings);
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// Persist the settings (overwrites the file).
#[tauri::command]
pub async fn save_settings(
    state: State<'_, Mutex<SessionManager>>,
    settings: Settings,
) -> Result<(), String> {
    let config_dir = state.inner().lock().await.config_dir().clone();
    let path = settings_path(&config_dir);
    fs::write(
        &path,
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_dark_with_empty_layout() {
        let settings = Settings::default();
        assert_eq!(settings.theme, "dark");
        assert!(settings.pane_layout.is_object());
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            theme: "light".to_string(),
            pane_layout: serde_json::json!({ "chatWidth": 480 }),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"theme\":\"light\""));
        assert!(json.contains("\"paneLayout\""));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme, "light");
        assert_eq!(back.pane_layout["chatWidth"], 480);
    }
}
