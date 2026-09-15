pub mod history;
pub mod sessions;
pub mod settings;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub platform: String,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_crate_version_and_platform() {
        let info = app_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.platform.is_empty());
    }
}
