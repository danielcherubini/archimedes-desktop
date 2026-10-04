//! App settings: `settings.json` in the config dir.
//!
//! `{ "theme": "system" | "dark" | "light", "paneLayout": {...}, ... }`
//! with sane defaults written on first run. All extended fields are
//! `#[serde(default)]` so a pre-feature file parses to the defaults (no
//! migration needed). A corrupt file yields the defaults + a logged warning
//! — a bad file must never block app startup.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::agent::harness::{tool_specs, Model};
use crate::agent::SessionManager;

/// A user-managed LLM provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// The provider's id: the slug of its name (the desktop's Settings page
    /// derives it from the name — a name commit re-identifies the provider,
    /// remapping the `default_model` / `default_thinking_levels` references
    /// old → new; a blank name keeps the current id).
    pub id: String,
    pub name: String,
    /// Normalized base URL (…/v1).
    pub base_url: String,
    /// Plaintext (empty = local gateway, no key).
    pub api_key: String,
    /// The wire API (ADR 0024): `"openai-completions"` (the default — a
    /// pre-feature file parses to it, no migration) / `"anthropic-messages"`
    /// / `"openai-responses"` / `"litellm"` (discovery via `GET /model/info`,
    /// wire = openai-completions — ADR 0026).
    #[serde(default = "default_provider_api")]
    pub api: String,
    /// The key-management page URL (set by the known-providers picker — the
    /// row's "Get key" link; `None` for a hand-typed provider).
    #[serde(default)]
    pub key_url: Option<String>,
}

fn default_provider_api() -> String {
    "openai-completions".to_string()
}

/// Font settings for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontSettings {
    /// UI font size in px (default 14).
    #[serde(default = "default_font_size")]
    pub size_px: u32,
    /// `None` = the design system's pinned sans stack (the app default —
    /// the `index.css` stack resolves to `"Noto Sans"` first, with the
    /// system tail as the offline fallback).
    #[serde(default)]
    pub ui_family: Option<String>,
    /// `None` = the design system's pinned mono stack (the app default —
    /// the `index.css` stack resolves to `"Fira Code"` first, with the
    /// system tail as the offline fallback).
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
    /// Whether to trust new Spaces by default.
    #[serde(default)]
    pub default_trust_new_spaces: bool,
    /// `"provider/id"`; `None` = system default.
    #[serde(default)]
    pub default_model: Option<String>,
    /// The per-app thinking-level seed (the native session start's last
    /// rung: remembered > stored > this > `None`); `None` = the model's
    /// own default. `#[serde(default)]` — a pre-feature file parses to
    /// `None` (no migration needed).
    #[serde(default)]
    pub default_thinking_level: Option<String>,
    /// The harness-level tool filter: `[]` = all tools enabled (the native
    /// `AgentLoop` maps an empty list to `None` = all). `#[serde(default)]`
    /// — a pre-feature file parses to `[]` (no migration needed).
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    /// User-managed providers (default `[]`).
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// (ADR 0019) The user-managed MCP servers: `name → entry` in pi's
    /// `mcpServers` entry shape VERBATIM (an entry copy-pastes between the
    /// two files). Merged into the harness's effective set at global
    /// precedence (project > desktop > pi-global). `#[serde(default)]` —
    /// a pre-feature file parses to `{}` (no migration).
    #[serde(default)]
    pub mcp_servers: HashMap<String, Value>,
    /// Font settings.
    #[serde(default)]
    pub font: FontSettings,
    /// (ADR 0015) The per-model remembered thinking level: the composed
    /// model key (`"<provider>/<id>"`) → the last EXPLICITLY chosen level.
    /// `#[serde(default)]` — a pre-feature file parses to an empty map.
    #[serde(default)]
    pub default_thinking_levels: HashMap<String, String>,
    /// (ADR 0023) Per-agent subagent model overrides: agent name → model key
    /// (`"provider/id"`, or a hand-edited `"provider/id:<level>"` — the
    /// `:<level>` suffix is picked up by the dispatch's existing thinking
    /// candidate handling). `#[serde(default)]` — a pre-feature file parses
    /// to an empty map (no migration).
    #[serde(default)]
    pub subagent_models: HashMap<String, String>,
    /// The working-indicator spinner style (the `braille-loader` variant name,
    /// e.g. `"typing"` / `"pendulum"`): the animation the chat's top working
    /// indicator runs while the agent is busy. `None` = the frontend's
    /// `typing` default (a pre-feature file — `#[serde(default)]`, no
    /// migration).
    #[serde(default)]
    pub spinner_style: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            pane_layout: Value::Object(Default::default()),
            default_trust_new_spaces: false,
            default_model: None,
            default_thinking_level: None,
            enabled_tools: Vec::new(),
            providers: Vec::new(),
            mcp_servers: HashMap::new(),
            font: FontSettings::default(),
            default_thinking_levels: HashMap::new(),
            subagent_models: HashMap::new(),
            spinner_style: None,
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
            // The app default (a fresh settings file): Noto Sans for the
            // UI, Fira Code for code — both the `index.html` Google Fonts
            // families (the quoted CSS family names the frontend's
            // `applySettingsFont` splices into the `--font-sans` /
            // `--font-mono` stacks).
            ui_family: Some("\"Noto Sans\"".to_string()),
            code_family: Some("\"Fira Code\"".to_string()),
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
        Ok(mut settings) => {
            // A hand-edited file may carry a blank level (the frontend never
            // SAVES `""`): a blank level is "no level" — the same as `null`.
            if settings.default_thinking_level.as_deref() == Some("") {
                settings.default_thinking_level = None;
            }
            settings
        }
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

/// Persist the settings (overwrites the file). In-process writers (the
/// `set_config_option` memory writes — ADR 0015) call this directly; the
/// command delegates to it.
pub fn write_settings(config_dir: &Path, settings: &Settings) -> Result<(), String> {
    let path = settings_path(config_dir);
    fs::write(
        &path,
        serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Persist the settings (overwrites the file).
#[tauri::command]
pub async fn save_settings(
    state: State<'_, Arc<SessionManager>>,
    settings: Settings,
) -> Result<(), String> {
    write_settings(state.config_dir(), &settings)
}

/// The effective catalog's models (the frontend's Default-model select +
/// the provider rows' discovery status).
/// `force_refresh` = a provider id whose discovery cache entry is bypassed
/// (the settings page's refresh affordance); `None` = cached.
#[tauri::command]
pub async fn list_models(
    state: State<'_, Arc<SessionManager>>,
    force_refresh: Option<String>,
) -> Result<Vec<ModelDto>, String> {
    Ok(state
        .effective_catalog(force_refresh.as_deref())
        .await
        .models
        .iter()
        .map(ModelDto::from)
        .collect())
}

/// The camelCase wire shape (the `Model` struct itself is NOT renamed —
/// `capabilities_json` embeds composed keys, not `Model`, so a DTO here
/// is the safe choice over renaming the struct). Wire-out-only by design
/// (no `Deserialize` — the command only returns it).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDto {
    pub id: String,
    pub provider: String,
    pub context_window: u32,
    pub supports_thinking: bool,
    pub thinking_levels: Vec<String>,
}

impl From<&Model> for ModelDto {
    fn from(m: &Model) -> Self {
        Self {
            id: m.id.clone(),
            provider: m.provider.clone(),
            context_window: m.context_window,
            supports_thinking: m.supports_thinking,
            thinking_levels: m.thinking_levels.clone(),
        }
    }
}

/// The native harness's tool names, sorted (the Settings page's
/// enabled-tools checkbox list). STATELESS (no `tauri::State` param — the
/// command needs no state: `tool_specs()` is a free function).
#[tauri::command]
pub async fn list_tools() -> Result<Vec<String>, String> {
    let mut names: Vec<String> = tool_specs().iter().map(|t| t.name.clone()).collect();
    names.sort(); // deterministic UI order
    Ok(names)
}

/// A known provider template (the built-in catalog — the Settings'
/// known-providers picker's data source; ADR 0024). Seeded from the
/// ZCode builtin provider catalog (`config/provider/zcode-builtin.json`
/// `templateRules` — base URLs / key URLs / wire APIs VERBATIM; ids +
/// display names ADAPTED (see the `KNOWN_PROVIDERS` comment).
pub struct KnownProvider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api: &'static str,
    pub key_url: &'static str,
}

pub const KNOWN_PROVIDERS: &[KnownProvider] = &[
    // (the 20 entries — seeded from the ZCode catalog: base URLs / key
    // URLs / wire apis VERBATIM from `templateRules`; ids + display names
    // ADAPTED — ZCode `zai-standard-api` → `zai-api`, `moonshot-kimi` →
    // `kimi`, `qwen-alibaba-model-studio-cn`/`-intl` → `alibaba-cn`/`-intl`,
    // `opencode-*-messages` → `opencode-*-anthropic`, names simplified
    // (`Z.ai Coding Plan` → `Z.ai`, `BigModel Coding Plan` → `BigModel`);
    // the order is the ZCode catalog's order with the six OpenCode entries
    // regrouped: go-chat, go-anthropic, go-responses, zen-chat,
    // zen-anthropic, zen-responses)
    KnownProvider {
        id: "zai",
        name: "Z.ai",
        base_url: "https://api.z.ai/api/anthropic",
        api: "anthropic-messages",
        key_url: "https://z.ai/manage-apikey/apikey-list",
    },
    KnownProvider {
        id: "zai-api",
        name: "Z.ai API",
        base_url: "https://api.z.ai/api/paas/v4",
        api: "openai-completions",
        key_url: "https://z.ai/manage-apikey/apikey-list",
    },
    KnownProvider {
        id: "bigmodel",
        name: "BigModel",
        base_url: "https://open.bigmodel.cn/api/anthropic",
        api: "anthropic-messages",
        key_url: "https://bigmodel.cn/coding-plan/personal/overview",
    },
    KnownProvider {
        id: "bigmodel-api",
        name: "BigModel API",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        api: "openai-completions",
        key_url: "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
    },
    KnownProvider {
        id: "kimi",
        name: "Kimi",
        base_url: "https://api.moonshot.cn/anthropic",
        api: "anthropic-messages",
        key_url: "https://platform.kimi.com/console/api-keys",
    },
    KnownProvider {
        id: "minimax",
        name: "MiniMax",
        base_url: "https://api.minimaxi.com/anthropic",
        api: "anthropic-messages",
        key_url: "https://platform.minimaxi.com/console/access?tab=api-keys",
    },
    KnownProvider {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/anthropic",
        api: "anthropic-messages",
        key_url: "https://platform.deepseek.com/api_keys",
    },
    KnownProvider {
        id: "alibaba-cn",
        name: "Alibaba Cloud (China)",
        base_url: "https://dashscope.aliyuncs.com/apps/anthropic",
        api: "anthropic-messages",
        key_url: "https://bailian.console.aliyun.com/cn-beijing?tab=model",
    },
    KnownProvider {
        id: "alibaba-intl",
        name: "Alibaba Cloud (Global)",
        base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        api: "openai-completions",
        key_url: "https://modelstudio.console.aliyun.com/ap-southeast-1?tab=dashboard",
    },
    KnownProvider {
        id: "xiaomi-mimo",
        name: "Xiaomi MiMo",
        base_url: "https://api.xiaomimimo.com/anthropic",
        api: "anthropic-messages",
        key_url: "https://platform.xiaomimimo.com/",
    },
    KnownProvider {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        api: "openai-responses",
        key_url: "https://platform.openai.com/api-keys",
    },
    KnownProvider {
        id: "anthropic",
        name: "Anthropic",
        base_url: "https://api.anthropic.com/v1",
        api: "anthropic-messages",
        key_url: "https://console.anthropic.com/settings/keys",
    },
    KnownProvider {
        id: "xai",
        name: "xAI",
        base_url: "https://api.x.ai/v1",
        api: "openai-responses",
        key_url: "https://console.x.ai",
    },
    KnownProvider {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api",
        api: "anthropic-messages",
        key_url: "https://openrouter.ai/keys",
    },
    KnownProvider {
        id: "opencode-go-chat",
        name: "OpenCode Go (Chat)",
        base_url: "https://opencode.ai/zen/go/v1",
        api: "openai-completions",
        key_url: "https://opencode.ai/auth",
    },
    KnownProvider {
        id: "opencode-go-anthropic",
        name: "OpenCode Go (Anthropic)",
        base_url: "https://opencode.ai/zen/go/v1",
        api: "anthropic-messages",
        key_url: "https://opencode.ai/auth",
    },
    KnownProvider {
        id: "opencode-go-responses",
        name: "OpenCode Go (Responses)",
        base_url: "https://opencode.ai/zen/go/v1",
        api: "openai-responses",
        key_url: "https://opencode.ai/auth",
    },
    KnownProvider {
        id: "opencode-zen-chat",
        name: "OpenCode Zen (Chat)",
        base_url: "https://opencode.ai/zen/v1",
        api: "openai-completions",
        key_url: "https://opencode.ai/auth",
    },
    KnownProvider {
        id: "opencode-zen-anthropic",
        name: "OpenCode Zen (Anthropic)",
        base_url: "https://opencode.ai/zen/v1",
        api: "anthropic-messages",
        key_url: "https://opencode.ai/auth",
    },
    KnownProvider {
        id: "opencode-zen-responses",
        name: "OpenCode Zen (Responses)",
        base_url: "https://opencode.ai/zen/v1",
        api: "openai-responses",
        key_url: "https://opencode.ai/auth",
    },
];

/// The camelCase wire shape (the `KnownProvider` itself is NOT renamed —
/// a DTO, the `ModelDto` pattern). Wire-out-only by design (no
/// `Deserialize` — the command only returns it).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownProviderDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: String,
    pub key_url: String,
}

impl From<&KnownProvider> for KnownProviderDto {
    fn from(k: &KnownProvider) -> Self {
        Self {
            id: k.id.into(),
            name: k.name.into(),
            base_url: k.base_url.into(),
            api: k.api.into(),
            key_url: k.key_url.into(),
        }
    }
}

/// The known-providers catalog (the Settings' picker's data source —
/// ADR 0024). STATELESS (no `tauri::State` param — the `list_tools`
/// pattern: the command needs no state).
#[tauri::command]
pub async fn list_known_providers() -> Result<Vec<KnownProviderDto>, String> {
    Ok(KNOWN_PROVIDERS.iter().map(KnownProviderDto::from).collect())
}

/// Test ONE MCP server definition (the Settings page's Test action, ADR
/// 0019): a one-shot bounded connect + `tools/list`. `Ok` = the tool
/// count; `Err` = the error text (surfaced verbatim in the row — a
/// `needs-auth` / a network failure). The entry is the `settings.json`
/// `mcpServers` entry VERBATIM (pi's shape — classified, not re-shaped).
#[tauri::command]
pub async fn test_mcp_server(name: Option<String>, entry: Value) -> Result<u32, String> {
    let def = crate::agent::mcp::types::classify_server(&entry)
        .ok_or_else(|| "invalid MCP server entry (need a `url` or a `command`)".to_string())?;
    let timeout = std::time::Duration::from_secs(10);
    crate::agent::mcp::manager::test_server(name.as_deref(), &def, Path::new("."), timeout)
        .await
        .map(|n| n as u32)
}

/// Run interactive OAuth authentication for an MCP server (Settings UI).
#[tauri::command]
pub async fn auth_mcp_server(name: String, entry: Value) -> Result<String, String> {
    let def = crate::agent::mcp::types::classify_server(&entry)
        .ok_or_else(|| "invalid MCP server entry (need a `url` or a `command`)".to_string())?;
    crate::agent::mcp::manager::authenticate_server(&name, &def).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_tools_returns_a_sorted_list_of_the_native_harness_tools() {
        // The command is stateless (no `tauri::State` param — it cannot be
        // constructed in a unit test, and it needs no state: `tool_specs()`
        // is a free function), so it is called directly.
        let names = list_tools().await.expect("the tool specs build");
        // Sorted (deterministic UI order).
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        // The built-in harness tools.
        for name in ["bash", "read", "edit", "write", "subagent"] {
            assert!(
                names.iter().any(|n| n == name),
                "missing {name} in {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn list_known_providers_returns_the_20_templates() {
        // The command is stateless (no `tauri::State` param — the `list_tools`
        // pattern), so it is called directly.
        let providers = list_known_providers()
            .await
            .expect("the catalog is built-in");
        assert_eq!(
            providers.len(),
            20,
            "the ZCode builtin catalog has 20 templates"
        );
        // Every `api` is a harness wire API (ADR 0024).
        for p in &providers {
            assert!(
                matches!(
                    p.api.as_str(),
                    "openai-completions" | "anthropic-messages" | "openai-responses"
                ),
                "unknown wire api {:?} for {:?}",
                p.api,
                p.id
            );
            assert!(
                p.base_url.starts_with("https://"),
                "a builtin template is https: {:?}",
                p.base_url
            );
            assert!(
                !p.key_url.is_empty(),
                "every template has a key URL: {:?}",
                p.id
            );
        }
        // The `id`s are unique.
        let mut ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate provider id in {ids:?}");
    }

    #[tokio::test]
    async fn list_known_providers_templates_match_the_zcode_catalog() {
        // Spot-checks: the ZCode catalog's values (base URLs / key URLs /
        // wire apis VERBATIM from `templateRules`).
        let providers = list_known_providers().await.unwrap();
        let by_id: HashMap<String, &KnownProviderDto> =
            providers.iter().map(|p| (p.id.clone(), p)).collect();
        let anthropic = &by_id["anthropic"];
        assert_eq!(anthropic.base_url, "https://api.anthropic.com/v1");
        assert_eq!(anthropic.api, "anthropic-messages");
        let openai = &by_id["openai"];
        assert_eq!(openai.api, "openai-responses");
        let zai = &by_id["zai"];
        assert_eq!(zai.base_url, "https://api.z.ai/api/anthropic");
        let deepseek = &by_id["deepseek"];
        assert_eq!(deepseek.base_url, "https://api.deepseek.com/anthropic");
        let alibaba_intl = &by_id["alibaba-intl"];
        assert_eq!(
            alibaba_intl.base_url,
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(alibaba_intl.api, "openai-completions");
    }

    #[test]
    fn default_settings_are_dark_with_empty_layout() {
        let settings = Settings::default();
        assert_eq!(settings.theme, "dark");
        assert!(settings.pane_layout.is_object());
        assert_eq!(settings.default_thinking_level, None);
        assert!(settings.enabled_tools.is_empty());
        assert!(!settings.default_trust_new_spaces);
        assert_eq!(settings.default_model, None);
        assert!(settings.providers.is_empty());
        assert_eq!(settings.font, FontSettings::default());
        assert!(settings.subagent_models.is_empty());
    }

    #[test]
    fn a_pre_feature_settings_json_parses_to_the_new_defaults() {
        // A file written before the extended fields existed must parse (all
        // new fields `#[serde(default)]`) rather than be treated as corrupt.
        let settings: Settings =
            serde_json::from_str(r#"{ "theme": "dark", "paneLayout": {} }"#).unwrap();
        assert_eq!(settings.default_thinking_level, None);
        assert!(settings.enabled_tools.is_empty());
        assert!(!settings.default_trust_new_spaces);
        assert_eq!(settings.default_model, None);
        assert!(settings.providers.is_empty());
        assert!(settings.mcp_servers.is_empty());
        assert_eq!(settings.font, FontSettings::default());
        assert!(settings.subagent_models.is_empty());
        assert_eq!(settings.spinner_style, None);
    }

    #[test]
    fn spinner_style_defaults_to_none_and_round_trips() {
        // A pre-feature file (no `spinnerStyle`) parses to `None` (the
        // frontend falls back to the `typing` default); an explicit choice
        // survives a round trip in camelCase.
        assert_eq!(Settings::default().spinner_style, None);
        let settings = Settings {
            spinner_style: Some("pendulum".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(
            json.contains("\"spinnerStyle\""),
            "missing spinnerStyle in {json}"
        );
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.spinner_style, Some("pendulum".to_string()));
    }

    #[test]
    fn test_mcp_server_an_invalid_entry_is_an_error() {
        // (ADR 0019) The Test action on a malformed entry (neither a `url`
        // nor a `command`) is an error, not a connect attempt.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = rt
            .block_on(test_mcp_server(
                None,
                serde_json::json!({ "headers": { "a": "b" } }),
            ))
            .expect_err("a malformed entry is an error");
        assert!(
            result.contains("invalid"),
            "the error names the problem: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_mcp_server_a_valid_stdio_entry_reports_the_tool_count() {
        // (ADR 0019) A valid stdio entry (the fake binary — three canned
        // tools) round-trips the count through the command.
        let bin = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/debug/fake_mcp_stdio"
        ));
        let count = test_mcp_server(
            None,
            serde_json::json!({ "command": bin.display().to_string() }),
        )
        .await
        .expect("the fake server answers");
        assert_eq!(count, 3);
    }

    #[test]
    fn mcp_servers_round_trips_in_camel_case() {
        // (ADR 0019) The `mcpServers` field round-trips the pi entry shape
        // VERBATIM (a `Value` — the shape is pi's, not a desktop struct).
        let file = r#"{ "theme": "dark", "mcpServers": { "tama": { "url": "https://tama/mcp", "headers": { "Authorization": "Bearer k" } }, "local": { "command": "npx", "args": ["-y", "x-mcp"] } } }"#;
        let settings: Settings = serde_json::from_str(file).unwrap();
        assert_eq!(settings.mcp_servers.len(), 2);
        assert_eq!(
            settings.mcp_servers["tama"],
            serde_json::json!({ "url": "https://tama/mcp", "headers": { "Authorization": "Bearer k" } })
        );
        assert_eq!(
            settings.mcp_servers["local"],
            serde_json::json!({ "command": "npx", "args": ["-y", "x-mcp"] })
        );
        // camelCase on the wire (a populated map serializes the field).
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"mcpServers\""), "camelCase key: {json}");
        assert!(
            json.contains("https://tama/mcp"),
            "the entry round-trips: {json}"
        );
    }

    #[test]
    fn default_thinking_levels_defaults_to_empty_and_round_trips() {
        // (ADR 0015) The per-model remembered thinking-level map defaults to
        // empty and round-trips in camelCase.
        assert!(Settings::default().default_thinking_levels.is_empty());
        // A pre-feature file (WITHOUT the field) parses to an empty map
        // (`#[serde(default)]` — no migration needed).
        let settings: Settings = serde_json::from_str(r#"{ "theme": "dark" }"#).unwrap();
        assert!(settings.default_thinking_levels.is_empty());
        // A file WITH the field parses to the entry.
        let with_map: Settings =
            serde_json::from_str(r#"{ "defaultThinkingLevels": { "a/b": "xhigh" } }"#).unwrap();
        assert_eq!(with_map.default_thinking_levels["a/b"], "xhigh");
        // camelCase on the wire (a populated map serializes the field).
        let settings = Settings {
            default_thinking_levels: std::collections::HashMap::from([(
                "a/b".to_string(),
                "xhigh".to_string(),
            )]),
            ..Settings::default()
        };
        let json = serde_json::to_string_pretty(&settings).unwrap();
        assert!(
            json.contains("\"defaultThinkingLevels\""),
            "missing key in {json}"
        );
    }

    #[test]
    fn subagent_models_round_trips_in_camel_case() {
        // (ADR 0023) The per-agent subagent model-override map defaults to
        // empty and round-trips in camelCase.
        assert!(Settings::default().subagent_models.is_empty());
        // A pre-feature file (WITHOUT the field) parses to an empty map
        // (`#[serde(default)]` — no migration needed).
        let settings: Settings = serde_json::from_str(r#"{ "theme": "dark" }"#).unwrap();
        assert!(settings.subagent_models.is_empty());
        // A file WITH the field parses to the entry.
        let with_map: Settings =
            serde_json::from_str(r#"{ "subagentModels": { "scout": "tama/m-1" } }"#).unwrap();
        assert_eq!(with_map.subagent_models["scout"], "tama/m-1");
        // camelCase on the wire (a populated map serializes the field).
        let settings = Settings {
            subagent_models: std::collections::HashMap::from([(
                "scout".to_string(),
                "tama/m-1".to_string(),
            )]),
            ..Settings::default()
        };
        let json = serde_json::to_string_pretty(&settings).unwrap();
        assert!(json.contains("\"subagentModels\""), "missing key in {json}");
    }

    #[test]
    fn font_settings_default_is_size_14_not_zero() {
        // Guards against a regression to a `#[derive(Default)]` (which would
        // give `size_px: 0`, a second "default" diverging from the serde
        // missing-field default of 14).
        assert_eq!(FontSettings::default().size_px, 14);
    }

    #[test]
    fn font_settings_default_is_noto_sans_ui_and_fira_code_code() {
        // The app default (a fresh settings file): Noto Sans for the UI and
        // Fira Code for code — both the `index.html` Google Fonts families
        // (the quoted CSS family names the frontend's `applySettingsFont`
        // splices into the `--font-sans` / `--font-mono` stacks).
        let font = FontSettings::default();
        assert_eq!(font.ui_family, Some("\"Noto Sans\"".to_string()));
        assert_eq!(font.code_family, Some("\"Fira Code\"".to_string()));
    }

    #[test]
    fn provider_config_api_defaults_to_openai_completions() {
        // (ADR 0024) A pre-feature `settings.json` (NO `api` field) parses
        // to `api == "openai-completions"` (the serde default — no
        // migration) + `key_url == None`.
        let config: ProviderConfig =
            serde_json::from_str(r#"{"id":"p","name":"P","baseUrl":"https://x/v1","apiKey":"k"}"#)
                .unwrap();
        assert_eq!(config.api, "openai-completions");
        assert_eq!(config.key_url, None);
    }

    #[test]
    fn provider_config_api_and_key_url_round_trip() {
        // (ADR 0024) A populated `api` + `keyUrl` round-trips camelCase.
        let config = ProviderConfig {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            api_key: "k".to_string(),
            api: "anthropic-messages".to_string(),
            key_url: Some("https://keys.example".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            json.contains("\"api\":\"anthropic-messages\""),
            "got {json}"
        );
        assert!(
            json.contains("\"keyUrl\":\"https://keys.example\""),
            "got {json}"
        );
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn settings_round_trip_with_the_new_fields() {
        let settings = Settings {
            theme: "system".to_string(),
            pane_layout: serde_json::json!({ "chatWidth": 480 }),
            default_thinking_level: Some("high".to_string()),
            enabled_tools: vec!["read".to_string()],
            default_trust_new_spaces: true,
            default_model: Some("anthropic/claude-opus-4".to_string()),
            providers: vec![ProviderConfig {
                id: "my-gateway".to_string(),
                name: "My Gateway".to_string(),
                base_url: "http://localhost:8080/v1".to_string(),
                api_key: String::new(),
                api: "openai-completions".into(),
                key_url: None,
            }],
            font: FontSettings {
                size_px: 18,
                ui_family: Some("Inter".to_string()),
                code_family: Some("JetBrains Mono".to_string()),
            },
            mcp_servers: std::collections::HashMap::from([
                (
                    "tama".to_string(),
                    serde_json::json!({ "url": "https://tama/mcp" }),
                ),
                (
                    "local".to_string(),
                    serde_json::json!({ "command": "npx", "args": ["-y", "x-mcp"] }),
                ),
            ]),
            default_thinking_levels: std::collections::HashMap::from([(
                "a/b".to_string(),
                "xhigh".to_string(),
            )]),
            subagent_models: std::collections::HashMap::from([(
                "scout".to_string(),
                "tama/m-1".to_string(),
            )]),
            spinner_style: Some("marquee".to_string()),
        };
        let json = serde_json::to_string(&settings).unwrap();
        for key in [
            "\"defaultThinkingLevel\"",
            "\"enabledTools\"",
            "\"defaultTrustNewSpaces\"",
            "\"defaultModel\"",
            "\"providers\"",
            "\"mcpServers\"",
            "\"font\"",
            "\"sizePx\"",
            "\"defaultThinkingLevels\"",
            "\"subagentModels\"",
            "\"spinnerStyle\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn load_settings_normalizes_a_blank_default_thinking_level_to_none() {
        // A hand-edited file may carry `"defaultThinkingLevel": ""` (the
        // frontend never SAVES `""`, but the `Option<String>` accepts it):
        // a blank level is "no level" — the same as a missing field (`null`).
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.json"),
            r#"{ "theme": "dark", "defaultThinkingLevel": "" }"#,
        )
        .unwrap();
        let settings = load_settings(dir.path());
        assert_eq!(settings.default_thinking_level, None);
        // The other fields still parse (the file is not corrupt).
        assert_eq!(settings.theme, "dark");
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

    #[test]
    fn list_models_dto_mapping() {
        // The `Model` struct itself is NOT `#[serde(rename_all)]` (it is
        // embedded in composed `capabilities_json` keys), so the
        // `list_models` command returns the explicit camelCase `ModelDto`
        // projection — the serialized output must carry `contextWindow`
        // (NOT `context_window`).
        let model = crate::agent::harness::Model {
            id: "m/1".into(),
            provider: "tama".into(),
            base_url: "https://tama.wizards.town/v1".into(),
            api_key: "k".into(),
            context_window: 99999,
            cost_per_mtok_in: 0.0,
            cost_per_mtok_out: 0.0,
            supports_tools: true,
            supports_thinking: true,
            thinking_levels: vec!["low".to_string()],
            api: Some("openai-completions".into()),
        };
        let dto = ModelDto::from(&model);
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["id"], "m/1");
        assert_eq!(v["provider"], "tama");
        assert_eq!(v["contextWindow"], 99999);
        assert_eq!(v["supportsThinking"], true);
        assert_eq!(v["thinkingLevels"], serde_json::json!(["low"]));
        // NOT snake_case on the wire.
        assert!(v.get("context_window").is_none());
    }
}
