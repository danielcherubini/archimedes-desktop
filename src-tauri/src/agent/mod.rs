//! Pi-RPC session core: spawn a pi agent, speak its JSONL RPC protocol,
//! drive the session lifecycle.

pub mod bridge;
mod errors;
mod fs_backend;
pub mod gate;
mod permission;
pub mod rpc;
mod session;
pub mod subagent;
pub mod worker_runtime;

pub use bridge::{bridge_key, PendingBridge};
pub use errors::RpcError;
pub use fs_backend::{FsBackend, FsError};
pub use gate::{gate_env, gate_spawn_args, install_gate_extension};
pub use permission::{permission_key, PendingPermissions, PermissionOutcome};
pub use rpc::{ExtensionUiRequest, ExtensionUiResponse, PiRpc, PiRpcHandle, RpcEvent};
pub use session::{
    normalize_capabilities, user_message_payload, ClosedReason, EventSink, ImagePayload,
    SessionInfo, SessionManager, StopReason, SubagentSpawn, MAX_IMAGE_BYTES,
};
pub use subagent::{SubagentMetrics, SubagentOutcome, SubagentSessionManager};
pub use worker_runtime::WorkerRuntime;
