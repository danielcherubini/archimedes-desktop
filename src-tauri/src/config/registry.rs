use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The agent's backend kind: an EXTERNAL agent (the desktop spawns its
/// process — the existing `pi --mode rpc` path) or a NATIVE agent (the
/// desktop runs the in-process `AgentLoop` harness — the entry carries a
/// harness config, NOT a spawn spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    External,
    Native,
}

/// The native harness config (a `kind: native` entry carries this INSTEAD
/// of a spawn spec — `command` is empty / absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// The provider family (v1: `"openai-compatible"` — the
    /// `OpenAiCompatibleProvider` wire).
    #[serde(default = "default_provider")]
    pub provider: String,
    /// The default model's composed key (`"<provider>/<id>"`); `None` =
    /// resolve from the `ModelCatalog` at session start (the built-in is
    /// `None` — the pure-serde `Registry::load` is NOT coupled to
    /// `seed_from_pi_config()`).
    #[serde(default)]
    pub default_model: Option<String>,
    /// The enabled tool names (`[]` = all).
    #[serde(default)]
    pub enabled_tools: Vec<String>,
    /// The default thinking level (`None` = the model's default).
    #[serde(default)]
    pub default_thinking_level: Option<String>,
}

fn default_provider() -> String {
    "openai-compatible".to_string()
}

/// A single agent the desktop can launch.
///
/// `command` is the executable (e.g. `pi`) for an EXTERNAL agent; a
/// `kind: native` entry carries a harness config (NOT a spawn spec), so
/// `command` defaults to `""` and must never be spawned. `args` and `env`
/// are optional extras passed through to the spawned process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    /// Stable identifier, e.g. `"pi"`.
    pub id: String,
    /// Display name shown in the UI.
    pub name: String,
    /// Executable to spawn (EXTERNAL agents only — the desktop spawns it
    /// directly in its RPC mode — `pi --mode rpc`; see ADR 0009). A
    /// `kind: native` entry has no spawn spec (the default `""`).
    #[serde(default)]
    pub command: String,
    /// Optional extra command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables for the child process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Whether this agent runs the archimedes suite with the bridge enabled.
    /// When set, the desktop spawns it with the `PI_ARCHIMEDES_BRIDGE_*` env
    /// vars and listens on a per-spawn peer-verified socket (the bridge,
    /// ADR 0003). Default `false`; the built-in `pi` entry is `true`.
    #[serde(default)]
    pub bridge: bool,
    /// The backend kind (`external` default — the existing entries are
    /// unchanged).
    #[serde(default)]
    pub kind: AgentKind,
    /// The native harness config (`kind: native` only; `None` otherwise).
    #[serde(default)]
    pub harness: Option<HarnessConfig>,
}

/// The full set of configured agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub agents: Vec<AgentEntry>,
}

impl Registry {
    /// Load the registry from `<dir>/agents.json`.
    ///
    /// A missing file yields the default registry (a single `pi` entry
    /// pointing at the `pi` binary in its own RPC mode), so a fresh
    /// install works out of the box. The built-ins are ALWAYS merged in
    /// (a fresh install AND a user with an existing `agents.json` see the
    /// built-in `archimedes` native entry — see [`Self::with_builtins`]).
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join("agents.json");
        let registry = if !path.exists() {
            Self::default_registry()
        } else {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str(&raw)?
        };
        Ok(registry.with_builtins())
    }

    /// Append the built-ins AFTER the user entries (de-duped by `id` — a
    /// user override wins; the user entries are NEVER reordered:
    /// `NewSpaceDialog` uses `agents[0]` as the default, so a prepended
    /// built-in would silently flip the default to native — the registry's
    /// default remains `pi`).
    pub fn with_builtins(mut self) -> Self {
        for builtin in Self::builtins() {
            if !self.agents.iter().any(|agent| agent.id == builtin.id) {
                self.agents.push(builtin);
            }
        }
        self
    }

    /// The built-in agents (appended by [`Self::with_builtins`]).
    fn builtins() -> Vec<AgentEntry> {
        vec![Self::builtin_archimedes()]
    }

    /// The built-in 'Archimedes' native agent (OPT-IN — the registry's
    /// default remains `pi`): a harness config (provider + default model +
    /// enabled tools + default thinking level), NOT a spawn spec.
    /// `default_model` is `None` (resolved from the `ModelCatalog` at
    /// session start — `Registry::load` stays pure serde).
    fn builtin_archimedes() -> AgentEntry {
        AgentEntry {
            id: "archimedes".to_string(),
            name: "Archimedes".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            bridge: false,
            kind: AgentKind::Native,
            harness: Some(HarnessConfig {
                provider: "openai-compatible".to_string(),
                default_model: None,
                enabled_tools: Vec::new(), // all
                default_thinking_level: Some("high".to_string()),
            }),
        }
    }

    /// Look up an agent by its `id`.
    pub fn get(&self, id: &str) -> Option<&AgentEntry> {
        self.agents.iter().find(|agent| agent.id == id)
    }

    fn default_registry() -> Self {
        Self {
            agents: vec![AgentEntry {
                id: "pi".to_string(),
                name: "Pi".to_string(),
                command: "pi".to_string(),
                args: vec!["--mode".to_string(), "rpc".to_string()],
                env: BTreeMap::new(),
                bridge: true,
                kind: AgentKind::External,
                harness: None,
            }],
        }
    }
}

/// Errors that can occur while loading the agent registry.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read agents.json: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid agents.json: {0}")]
    Invalid(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("registry-unit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_native_entry_without_command_parses() {
        // A `kind: native` entry with "a harness config … not a spawn
        // spec" must deserialize with `command` omitted (it defaults to
        // `""`) and `harness` defaulting to `None`.
        let entry: AgentEntry =
            serde_json::from_str(r#"{ "id": "n", "name": "N", "kind": "native" }"#).unwrap();
        assert_eq!(entry.kind, AgentKind::Native);
        assert_eq!(entry.command, "");
        assert_eq!(entry.harness, None);
    }

    #[test]
    fn the_pre_task7_entry_shape_defaults_to_external() {
        // The existing entries (no `kind` key on the wire) are UNCHANGED:
        // `kind` defaults to `external`.
        let entry: AgentEntry =
            serde_json::from_str(r#"{ "id": "pi", "name": "Pi", "command": "pi" }"#).unwrap();
        assert_eq!(entry.kind, AgentKind::External);
        assert_eq!(entry.command, "pi");
        assert_eq!(entry.harness, None);
    }

    #[test]
    fn load_appends_the_builtin_after_the_user_entries() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("agents.json"),
            serde_json::to_string(&serde_json::json!({
                "agents": [{
                    "id": "pi",
                    "name": "Pi",
                    "command": "pi",
                    "args": ["--mode", "rpc"],
                    "bridge": true,
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let registry = Registry::load(&dir).unwrap();
        assert_eq!(registry.agents.len(), 2);
        assert_eq!(registry.agents[0].id, "pi", "the user entry stays first");
        assert_eq!(
            registry.agents[1].id, "archimedes",
            "the built-in is appended"
        );
        assert_eq!(registry.agents[1].kind, AgentKind::Native);
        assert_eq!(
            registry.agents[1]
                .harness
                .as_ref()
                .and_then(|h| h.default_model.as_deref()),
            None,
            "the built-in's default_model is None (catalog-resolved)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_user_override_of_the_builtin_wins() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("agents.json"),
            serde_json::to_string(&serde_json::json!({
                "agents": [
                    { "id": "pi", "name": "Pi", "command": "pi" },
                    {
                        "id": "archimedes",
                        "name": "Mine",
                        "kind": "native",
                        "harness": { "provider": "openai-compatible", "default_model": "tama/x" },
                    },
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let registry = Registry::load(&dir).unwrap();
        assert_eq!(
            registry.agents.len(),
            2,
            "exactly ONE archimedes (the user's)"
        );
        assert_eq!(registry.agents[1].name, "Mine");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
