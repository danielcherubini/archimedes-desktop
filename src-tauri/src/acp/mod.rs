//! ACP session core: spawn an agent, speak ACP, drive the session lifecycle.

mod errors;
mod fs_backend;
mod permission;
mod session;
mod terminal;

pub use errors::AcpError;
pub use fs_backend::FsBackend;
pub use permission::{permission_key, PendingPermissions, PermissionOutcome};
pub use session::{ClosedReason, EventSink, SessionInfo, SessionManager};
pub use terminal::TerminalManager;
