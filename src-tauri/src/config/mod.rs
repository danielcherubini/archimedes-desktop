//! Agent registry: the set of ACP agents the desktop can launch, loaded from
//! `<config_dir>/agents.json`.

mod registry;

pub use registry::{AgentEntry, ConfigError, Registry};
