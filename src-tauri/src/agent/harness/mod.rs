//! The native agent harness (native-agent-harness Phase 2): the in-process
//! Rust replacement for the pi agent process — a `Provider` (the model
//! client, Task 4) + the `AgentLoop` (Task 6: the model → tool →
//! retry/compaction loop, the native `ToolRegistry`, the SQLite
//! `SessionStore`, the `Compactor`, the `RetryPolicy`) + the model
//! catalog (Task 5). Grows across Tasks 5–7; the pieces land one per
//! task.

pub mod catalog;
pub mod compact;
pub mod r#loop;
pub mod prompt;
pub mod provider;
pub mod retry;
pub mod store;

pub use catalog::{
    discover_models, seed_from_pi_config, CompactionConfig, DiscoveredMeta, Model, ModelCatalog,
    ProviderDiscovery,
};
pub use compact::{split_for_compaction, Compactor};
pub use prompt::{
    build_child_system_message, build_main_prompt, load_project_context, PromptContext,
};
pub use provider::{
    ChatMessage, ChatRole, FinishReason, MessageContent, ModelOptions, ModelRequest,
    OpenAiCompatibleProvider, Provider, ProviderError, ProviderEvent, ToolCall, ToolCallDelta,
    ToolSpec, Usage,
};
pub(crate) use r#loop::tool_specs;
pub use r#loop::{AgentLoop, ControlCmd, Prompt, SudoDeps};
pub use retry::RetryPolicy;
pub use store::SessionStore;
