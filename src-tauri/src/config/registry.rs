use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A single agent the desktop can launch.
///
/// `command` is the executable (e.g. `pi`); `args` and `env` are optional
/// extras passed through to the spawned process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    /// Stable identifier, e.g. `"pi"`.
    pub id: String,
    /// Display name shown in the UI.
    pub name: String,
    /// Executable to spawn (parsed by the ACP transport).
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
    /// install works out of the box.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join("agents.json");
        if !path.exists() {
            return Ok(Self::default_registry());
        }
        let raw = std::fs::read_to_string(path)?;
        let registry: Registry = serde_json::from_str(&raw)?;
        Ok(registry)
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
