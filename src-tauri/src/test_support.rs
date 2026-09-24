//! Test support utilities for Archimedes Desktop.
//!
//! Provides helpers for common patterns in integration and unit testing.

use crate::agent::AcpError;
use std::time::Duration;

/// A helper to run an async attempt function with retries on `SpawnFailed`.
///
/// Executes up to 3 attempts with a 200ms delay between them (if the first two
/// fail with `AcpError::SpawnFailed`). Propagates other errors immediately.
/// Preserves the original `AcpError::SpawnFailed` error on the final attempt.
pub async fn run_with_retry<F, Fut, T>(mut attempt_fn: F) -> Result<T, AcpError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AcpError>>,
{
    let mut last_err = None;
    for i in 0..3 {
        match attempt_fn().await {
            Ok(result) => return Ok(result),
            Err(AcpError::SpawnFailed { hint }) => {
                last_err = Some(AcpError::SpawnFailed { hint });
                if i < 2 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| AcpError::Protocol {
        message: "unreachable".to_string(),
    }))
}
