//! A collecting `EventSink` (moved from `harness_subagent_dispatch.rs` —
//! shared with `harness_dispatch_native`).

use archimedes_lib::agent::EventSink;
use serde_json::Value;
use tokio::sync::mpsc;

/// A collecting `EventSink` (records every `(event, payload)` pair).
#[derive(Clone)]
pub struct RecSink {
    pub tx: mpsc::UnboundedSender<(String, Value)>,
}

impl EventSink for RecSink {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send((event.to_string(), payload));
    }
}
