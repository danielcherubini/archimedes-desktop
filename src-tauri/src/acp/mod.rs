//! ACP session core: spawn an agent, speak ACP, drive the session lifecycle.

pub mod bridge;
mod errors;
mod fs_backend;
pub mod launch_wrapper;
mod permission;
mod session;
pub mod subagent;
pub mod worker_runtime;

pub use bridge::{bridge_key, PendingBridge};
pub use errors::AcpError;
pub use fs_backend::FsBackend;
pub use permission::{permission_key, PendingPermissions, PermissionOutcome};
pub use session::{ClosedReason, EventSink, SessionInfo, SessionManager, SubagentSpawn};
pub use subagent::{SubagentMetrics, SubagentOutcome, SubagentSessionManager};
pub use worker_runtime::WorkerRuntime;
