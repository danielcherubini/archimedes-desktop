//! The native agent harness: the in-process `AgentLoop` session core —
//! drive the session lifecycle, the interactive channel, the subagent
//! manager.

pub mod crashlog;
pub mod debuglog;
mod errors;
pub mod events;
mod fs_backend;
pub mod harness;
pub mod interactive;
pub mod mcp;
mod permission;
pub mod persist;
mod session;
pub mod subagent;
pub mod todo;
pub mod tools;
pub mod worker;

pub use errors::SessionError;
pub use events::RpcEvent;
pub use fs_backend::{FsBackend, FsError};
pub use interactive::{
    interactive_key, CachedPassword, PendingInteractive, PendingSudo, RealSudoRunner, SudoRun,
    SudoRunner,
};
pub use permission::{permission_key, PendingPermissions, PermissionOutcome};
pub use persist::TranscriptPersister;
pub use session::{
    normalize_capabilities, user_message_payload, ClosedReason, EffectiveCatalog, EventSink,
    ImagePayload, ProviderFactory, SessionInfo, SessionManager, StalledInfo, StopReason,
    MAX_IMAGE_BYTES,
};
pub use subagent::{LaunchConfig, SubagentMetrics, SubagentOutcome, SubagentSessionManager};
pub use todo::{TodoItem, TodoStatus, TodoStore};
pub use tools::{ContentBlock, ImageRef, ToolCtx, ToolResult};
