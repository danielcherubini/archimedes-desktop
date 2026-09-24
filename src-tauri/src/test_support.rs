//! Test support utilities for Archimedes Desktop.
//!
//! Provides helpers for common patterns in integration and unit testing.

use crate::agent::RpcError;
use std::time::Duration;

/// A helper to run an async attempt function with retries on spawn-class
/// errors.
///
/// Executes up to 3 attempts with a 200ms delay between them (if the first
/// two fail with a spawn-class error — `RpcError::Spawn` (a flake in
/// spawning the test agent), `RpcError::Parse` (a malformed line on the
/// establish round-trip), or `RpcError::EstablishTimeout` (the establish
/// command timed out)). Propagates other errors immediately.
pub async fn run_with_retry<F, Fut, T>(mut attempt_fn: F) -> Result<T, RpcError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, RpcError>>,
{
    let mut last_err = None;
    for i in 0..3 {
        match attempt_fn().await {
            Ok(result) => return Ok(result),
            Err(
                e @ (RpcError::Spawn { .. }
                | RpcError::Parse { .. }
                | RpcError::EstablishTimeout { .. }),
            ) => {
                last_err = Some(e);
                if i < 2 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| RpcError::Io("unreachable".to_string())))
}
