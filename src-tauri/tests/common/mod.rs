//! Shared test-only helpers (a canned-response `Provider`, a collecting
//! `EventSink`, process-scan utilities).

pub mod mock_provider;
// `proc_scan` is only used by test binaries that include it — the others
// (e.g. `harness_dispatch_native`) pull in `common` for the mock helpers
// only, so it is dead code there (allow it — the lint would otherwise
// fire in every test binary that includes `common`).
#[allow(dead_code)]
pub mod proc_scan;
pub mod rec_sink;

pub use mock_provider::{MockProvider, MockResponse};
pub use rec_sink::RecSink;
