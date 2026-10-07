//! The Settings IPC surface: the `#[tauri::command]` wrappers over the
//! `crate::config` service (the `settings.json` model + I/O live there —
//! this module is the IPC layer only) + the two wire DTOs (`ModelDto`,
//! `KnownProviderDto`).

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::agent::harness::{tool_specs, Model};
use crate::agent::SessionManager;
use crate::config::{load_settings, write_settings, KnownProvider, Settings, KNOWN_PROVIDERS};

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
            api: k.api.as_str().into(),
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

/// (ADR 0030 Task 5) Whether a `Sandboxed` `bash` can actually be confined
/// here: Linux + a kernel that enforces Landlock. Everywhere else `false`
/// (the sandbox module is not even compiled), and the tier FAILS CLOSED — so
/// Settings must grey the option out and say why rather than offer a choice
/// that only produces errors. STATELESS (the `list_tools` pattern).
#[tauri::command]
pub async fn shell_sandbox_available() -> Result<bool, String> {
    Ok(crate::agent::tools::shell_sandbox_available())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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

    /// (ADR 0030 Task 5) The Settings grey-out's source: on Linux it is the
    /// kernel probe's answer (the SAME signal `exec_bash` fails closed on —
    /// a UI that offered a tier the executor refuses would be a lie, and
    /// hiding a working one equally wrong); off Linux it is `false`
    /// unconditionally, because the sandbox is not compiled in there.
    #[tokio::test]
    async fn shell_sandbox_available_mirrors_the_kernel_probe() {
        let available = shell_sandbox_available().await.expect("stateless");
        #[cfg(target_os = "linux")]
        assert_eq!(
            available,
            crate::agent::tools::sandbox::landlock_available(),
            "on Linux the command reports the kernel's Landlock support"
        );
        #[cfg(not(target_os = "linux"))]
        assert!(!available, "off Linux there is no sandbox to offer");
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
