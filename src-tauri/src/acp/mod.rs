//! ACP session core: spawn an agent, speak ACP, drive the session lifecycle.

mod errors;
mod session;

pub use errors::AcpError;
pub use session::{ClosedReason, EventSink, SessionInfo, SessionManager};
