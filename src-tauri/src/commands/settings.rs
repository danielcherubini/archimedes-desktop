//! App settings: `settings.json` in the config dir.
//!
//! `{ "theme": "system" | "dark" | "light", "paneLayout": {...}, ... }`
//! with sane defaults written on first run. All extended fields are
//! `#[serde(default)]` so a pre-feature file parses to the defaults (no
//! migration needed). A corrupt file yields the defaults + a logged warning
//! — a bad file must never block app startup.

use std::fs;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::agent::SessionManager;

/// A user-managed LLM provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Stable slug (internal — generated ONCE at add time, never changes
    /// afterwards).
    pub id: String,
    pub name: String,
    /// Normalized base URL (…/v1).
    pub base_url: String,
    /// Plaintext (empty = local gateway, no key).
    pub api_key: String,
}

/// Font settings for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontSettings {
    /// UI font size in px (default 14).
    #[serde(default = "default_font_size")]
    pub size_px: u32,
    /// `None` = the design system's pinned sans stack.
    #[serde(default)]
    pub ui_family: Option<String>,
    /// `None` = the design system's pinned mono stack.
    #[serde(default)]
    pub code_family: Option<String>,
}

fn default_font_size() -> u32 {
    14
}

fn default_theme() -> String {
    "dark".to_string()
}

/// The persisted app settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// `"system" | "dark" | "light"` (default `"dark"`).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Free-form pane layout state (owned by the frontend).
    #[serde(default)]
    pub pane_layout: Value,
    /// Registry id of the default agent; `None` = agents[0].
    #[serde(default)]
    pub default_agent: Option<String>,
    /// Whether to trust new Spaces by default.
    #[serde(default)]
    pub default_trust_new_spaces: bool,
    /// `"provider/id"`; `None` = system default.
    #[serde(default)]
    pub default_model: Option<String>,
    /// User-managed providers (default `[]`).
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Font settings.
    #[serde(default)]
    pub font: FontSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            pane_layout: Value::Object(Default::default()),
            default_agent: None,
            default_trust_new_spaces: false,
            default_model: None,
            providers: Vec::new(),
            font: FontSettings::default(),
        }
    }
}

/// HAND-WRITTEN (do NOT `#[derive(Default)]` — a derived `Default` would
/// give `size_px: 0`, a second "default" that diverges from the serde
/// missing-field default of 14):
impl Default for FontSettings {
    fn default() -> Self {
        Self {
            size_px: default_font_size(),
            ui_family: None,
            code_family: None,
        }
    }
}

fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Read the settings from the config dir. A MISSING file → the defaults
/// (written to disk, as today — a write failure is LOGGED via `eprintln!`
/// and the defaults are still returned: the write is best-effort, the app
/// must not fail to load). A CORRUPT file (unreadable OR unparseable) → the
/// defaults + a logged warning (`eprintln!`), NOT an `Err` — a bad file must
/// never block app startup; the file is left untouched until the next
/// `save_settings`.
pub fn load_settings(config_dir: &Path) -> Settings {
    let path = settings_path(config_dir);
    if !path.exists() {
        let settings = Settings::default();
        let json = match serde_json::to_string_pretty(&settings) {
            Ok(json) => json,
            Err(e) => {
                eprintln!("settings: failed to serialize defaults: {e}");
                return settings;
            }
        };
        if let Err(e) = fs::write(&path, json) {
            eprintln!("settings: failed to write defaults to {path:?}: {e}");
        }
        return settings;
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("settings: unreadable {path:?}: {e}; using defaults");
            return Settings::default();
        }
    };
    match serde_json::from_str::<Settings>(&raw) {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("settings: corrupt {path:?}: {e}; using defaults");
            Settings::default()
        }
    }
}

/// Read the settings. Always succeeds (see `load_settings`); the `Result`
/// is kept for command-shape stability but is never `Err`.
#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<SessionManager>>) -> Result<Settings, String> {
    Ok(load_settings(state.config_dir()))
}

/// Persist the settings (overwrites the file).
#[tauri::command]
pub async fn save_settings(
    state: State<'_, Arc<SessionManager>>,
    settings: Settings,
) -> Result<(), String> {
    let config_dir = state.config_dir().clone();
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
        assert_eq!(settings.default_agent, None);
        assert!(!settings.default_trust_new_spaces);
        assert_eq!(settings.default_model, None);
        assert!(settings.providers.is_empty());
        assert_eq!(settings.font, FontSettings::default());
    }

    #[test]
    fn a_pre_feature_settings_json_parses_to_the_new_defaults() {
        // A file written before the extended fields existed must parse (all
        // new fields `#[serde(default)]`) rather than be treated as corrupt.
        let settings: Settings =
            serde_json::from_str(r#"{ "theme": "dark", "paneLayout": {} }"#).unwrap();
        assert_eq!(settings.default_agent, None);
        assert!(!settings.default_trust_new_spaces);
        assert_eq!(settings.default_model, None);
        assert!(settings.providers.is_empty());
        assert_eq!(settings.font, FontSettings::default());
    }

    #[test]
    fn font_settings_default_is_size_14_not_zero() {
        // Guards against a regression to a `#[derive(Default)]` (which would
        // give `size_px: 0`, a second "default" diverging from the serde
        // missing-field default of 14).
        assert_eq!(FontSettings::default().size_px, 14);
    }

    #[test]
    fn settings_round_trip_with_the_new_fields() {
        let settings = Settings {
            theme: "system".to_string(),
            pane_layout: serde_json::json!({ "chatWidth": 480 }),
            default_agent: Some("claude-code".to_string()),
            default_trust_new_spaces: true,
            default_model: Some("anthropic/claude-opus-4".to_string()),
            providers: vec![ProviderConfig {
                id: "my-gateway".to_string(),
                name: "My Gateway".to_string(),
                base_url: "http://localhost:8080/v1".to_string(),
                api_key: String::new(),
            }],
            font: FontSettings {
                size_px: 18,
                ui_family: Some("Inter".to_string()),
                code_family: Some("JetBrains Mono".to_string()),
            },
        };
        let json = serde_json::to_string(&settings).unwrap();
        for key in [
            "\"defaultAgent\"",
            "\"defaultTrustNewSpaces\"",
            "\"defaultModel\"",
            "\"providers\"",
            "\"font\"",
            "\"sizePx\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn load_settings_on_a_corrupt_file_returns_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        let settings = load_settings(dir.path());
        assert_eq!(settings, Settings::default());
        // The file is left untouched until the next `save_settings`.
        let raw = fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert_eq!(raw, "{ not json");
    }

    #[test]
    fn load_settings_on_a_missing_file_writes_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_settings(dir.path());
        assert_eq!(settings, Settings::default());
        // First-run behavior: the defaults are written to disk.
        assert!(dir.path().join("settings.json").exists());
    }

    #[test]
    fn get_settings_never_errors() {
        // A corrupt file must never surface as an `Err` from the command
        // (it is covered end-to-end in `tests/ipc.rs`); the logic lives in
        // `load_settings`, which always returns the defaults in that case.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        let settings = load_settings(dir.path());
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            theme: "light".to_string(),
            pane_layout: serde_json::json!({ "chatWidth": 480 }),
            ..Settings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"theme\":\"light\""));
        assert!(json.contains("\"paneLayout\""));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme, "light");
        assert_eq!(back.pane_layout["chatWidth"], 480);
    }
}
