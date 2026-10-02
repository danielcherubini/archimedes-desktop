//! Shared test-only helpers (a canned-response `Provider`, a collecting
//! `EventSink`).

pub mod mock_provider;
pub mod rec_sink;

pub use mock_provider::{MockProvider, MockResponse};
pub use rec_sink::RecSink;
