//! The native agent harness (native-agent-harness Phase 2): the in-process
//! Rust replacement for the pi agent process — a `Provider` (the model
//! client, Task 4) + the `AgentLoop` (Task 6: the model → tool →
//! retry/compaction loop, the native `ToolRegistry`, the SQLite
//! `SessionStore`, the `Compactor`, the `RetryPolicy`) + the model
//! catalog (Task 5). Grows across Tasks 5–7; the pieces land one per
//! task.

pub mod catalog;
pub mod compact;
pub mod dispatch;
pub mod r#loop;
pub mod prompt;
pub mod provider;
pub mod retry;
pub mod store;
pub mod trust;

pub use catalog::{
    discover_models, merge_catalog, CompactionConfig, DiscoveredMeta, Model, ModelCatalog,
    ProviderDiscovery, DEFAULT_CONTEXT_WINDOW,
};
pub use compact::{split_for_compaction, Compactor};
pub use dispatch::{InProcessDispatcher, MockDispatcher, SubagentDispatcher};
pub use prompt::{
    build_child_system_message, build_main_prompt, load_project_context, PromptContext,
};
pub use provider::{
    build_provider, AnthropicProvider, ChatMessage, ChatRole, FinishReason, MessageContent,
    ModelOptions, ModelRequest, OpenAiCompatibleProvider, OpenAiResponsesProvider, Provider,
    ProviderError, ProviderEvent, ToolCall, ToolCallDelta, ToolSpec, Usage,
};
pub(crate) use r#loop::tool_specs;
pub use r#loop::{AgentLoop, ControlCmd, Prompt, SudoDeps};
pub use retry::RetryPolicy;
pub use store::{DisplayRow, NoopStore, SessionStore, Store};
pub use trust::{SqliteTrustSource, StaticTrustSource, TrustSource};
