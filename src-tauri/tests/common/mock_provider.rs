//! A canned-response `Provider` (moved from `harness_subagent_dispatch.rs`
//! — shared with `harness_dispatch_native`).

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;

use archimedes_lib::agent::harness::{ModelRequest, Provider, ProviderError, ProviderEvent};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;

/// One canned response: a `ProviderEvent` stream, or an error.
pub enum MockResponse {
    Stream(Vec<ProviderEvent>),
    Error(ProviderError),
}

/// A `Provider` returning canned responses in order (a `complete()` beyond
/// the queue is a `Fatal` error — a test bug).
pub struct MockProvider {
    responses: StdMutex<VecDeque<MockResponse>>,
}

impl MockProvider {
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: StdMutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _req: &ModelRequest,
    ) -> Result<BoxStream<'static, ProviderEvent>, ProviderError> {
        let next = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(MockResponse::Error(ProviderError::Fatal(
                "mock: no canned response left".into(),
            )));
        match next {
            MockResponse::Stream(events) => Ok(futures_util::stream::iter(events).boxed()),
            MockResponse::Error(e) => Err(e),
        }
    }
}
