//! Agent-definition listing for the Settings page (the Subagents
//! section — ADR 0023).

use serde::Serialize;

use crate::agents::AgentDefinition;

/// The camelCase wire shape. NOT the `AgentDefinition` struct itself —
/// a DTO keeps the command's contract stable and excludes the fields
/// the UI doesn't need (`path` / `system_prompt` / `tools` /
/// `thinking`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinitionDto {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    /// `"space" | "user"` (the `AgentScope` Display form).
    pub scope: String,
}

/// The mapping lives in a `From` impl (testable in isolation — the
/// command body is a thin `discover_agents(None).map(...)` wrapper).
impl From<AgentDefinition> for AgentDefinitionDto {
    fn from(d: AgentDefinition) -> Self {
        Self {
            name: d.name,
            description: d.description,
            model: d.model,
            scope: d.scope.to_string(),
        }
    }
}

/// The USER-level Agent definitions (the Settings page's Subagents
/// section — the page is app-global: `discover_agents(None)` =
/// user-level only; a space-level definition is NOT listed, but an
/// override for it still applies by name at dispatch time and can be
/// added by hand-editing `settings.json`).
#[tauri::command]
pub async fn list_agent_definitions() -> Result<Vec<AgentDefinitionDto>, String> {
    Ok(crate::agents::discover_agents(None)
        .into_iter()
        .map(AgentDefinitionDto::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentScope;
    use std::fs;

    fn full_definition() -> AgentDefinition {
        AgentDefinition {
            name: "scout".to_string(),
            description: "Fast recon.".to_string(),
            model: Some("tama/m-1".to_string()),
            thinking: Some("high".to_string()),
            tools: Some(vec!["read".to_string()]),
            system_prompt: "You are a scout.".to_string(),
            scope: AgentScope::User,
            path: "/home/u/.agents/agents/scout.md".to_string(),
        }
    }

    #[test]
    fn agent_definition_dto_mapping() {
        let dto = AgentDefinitionDto::from(full_definition());
        assert_eq!(dto.name, "scout");
        assert_eq!(dto.description, "Fast recon.");
        assert_eq!(dto.model.as_deref(), Some("tama/m-1"));
        assert_eq!(dto.scope, "user");

        // The DTO excludes the fields the UI doesn't need.
        let json = serde_json::to_string(&dto).unwrap();
        for key in ["systemPrompt", "path", "tools", "thinking"] {
            assert!(
                !json.contains(key),
                "wire JSON must not contain `{key}`: {json}"
            );
        }
    }

    #[test]
    fn agent_definition_dto_a_none_model_is_null_on_the_wire() {
        let mut d = full_definition();
        d.model = None;
        d.scope = AgentScope::Space;
        let dto = AgentDefinitionDto::from(d);
        let json = serde_json::to_string(&dto).unwrap();
        assert!(
            json.contains("\"model\":null"),
            "expected `null` model in: {json}"
        );
        assert!(
            json.contains("\"scope\":\"space\""),
            "expected `\"space\"` scope in: {json}"
        );
    }

    /// Restores `HOME` on scope exit (even when an assertion panics
    /// mid-test): `Some(v)` → set, `None` → remove. Mirrors the
    /// `agents.rs` test helper (private to that module).
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match self.0.take() {
                // SAFETY: the `ENV_LOCK` is still held at drop time (this
                // guard is declared after the `_lock` guard and outlives
                // it in reverse); no other thread mutates HOME concurrently.
                Some(v) => unsafe {
                    std::env::set_var("HOME", v);
                },
                // SAFETY: same as above.
                None => unsafe {
                    std::env::remove_var("HOME");
                },
            }
        }
    }

    // The command's central documented contract: USER-level definitions
    // ONLY (`discover_agents(None)`). `HOME` is isolated the same way
    // `agents.rs`' tests do (`env_lock` + `RestoreHome`): a space-level
    // definition in a separate temp dir must NOT appear, while the
    // user-level one is found via the isolated `HOME`.
    /// `#[allow(clippy::await_holding_lock)]` is INTENTIONAL: the command reads
    /// `HOME` (via `user_roots`), so the guard must stay held ACROSS the
    /// `.await` to serialize against the other HOME-reading tests — dropping
    /// it before the await would open a window where a sibling test could
    /// flip `HOME` mid-discovery.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn list_agent_definitions_returns_user_level_definitions_only() {
        // Hold the shared lock for the WHOLE set→assert→restore span (a
        // `cargo test --lib` runs all module tests concurrently).
        let _lock = crate::test_support::env_lock();
        let original = std::env::var_os("HOME");
        let _restore = RestoreHome(original.clone());
        let home = scratch();
        // SAFETY: the `ENV_LOCK` is held for the whole set→assert→restore
        // span (the `env_lock` guard); no other thread mutates HOME
        // concurrently.
        unsafe {
            std::env::set_var("HOME", &home);
        }
        fs::create_dir_all(home.join(".agents/agents")).unwrap();
        fs::write(
            home.join(".agents/agents/user-agent.md"),
            "---\nname: user-agent\ndescription: A user-level agent.\n---\nBody.\n",
        )
        .unwrap();

        // A SEPARATE temp dir stands in for a Space (the command never
        // scans it — `discover_agents(None)` skips space-level roots).
        let space = scratch();
        fs::create_dir_all(space.join(".agents/agents")).unwrap();
        fs::write(
            space.join(".agents/agents/space-agent.md"),
            "---\nname: space-agent\ndescription: A space-level agent.\n---\nBody.\n",
        )
        .unwrap();

        let out = list_agent_definitions().await.unwrap();
        assert_eq!(
            out.len(),
            1,
            "expected exactly the user-level definition: {out:?}"
        );
        assert_eq!(out[0].name, "user-agent");
        assert_eq!(out[0].scope, "user");
        assert!(!out.iter().any(|d| d.name == "space-agent"));
    }

    fn scratch() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("archimedes-cmd-agents-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
