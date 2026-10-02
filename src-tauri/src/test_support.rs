//! Test support utilities for Archimedes Desktop.
//!
//! Provides helpers for common patterns in integration and unit testing.

use crate::agent::RpcError;
use std::time::Duration;

/// The shared lock serializing the HOME-mutating tests (the parallel test
/// harness runs all module tests concurrently; a `HOME` read by one test
/// mid-mutation by another would see the wrong value).
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire the env-mutation lock (poison-tolerant — a sibling test
/// panicking while holding it must not turn this test's failure into an
/// opaque `PoisonError` panic; see the `ENV_LOCK` docs).
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    std::sync::Mutex::lock(&ENV_LOCK).unwrap_or_else(|p| p.into_inner())
}

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

/// Serve a fixed `GET` response for as many requests as arrive (until
/// the `JoinHandle` is aborted); `counter` (when `Some`) is incremented
/// per request (the cache tests count fetches). `status` + `body` mirror
/// the `catalog.rs` `raw_json_server` (its raw-HTTP-response construction
/// — that private one accepts exactly ONCE and is unreachable from other
/// modules).
pub async fn raw_json_server(
    listener: tokio::net::TcpListener,
    status: u16,
    body: &str,
    counter: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
) -> tokio::task::JoinHandle<()> {
    // Owned (the spawned task needs `'static`; the `&str` borrow would
    // otherwise escape the function).
    let body = body.to_string();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let mut data = Vec::new();
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                data.extend_from_slice(&buf[..n]);
            }
            if let Some(counter) = counter.clone() {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let reason = if status == 200 { "OK" } else { "Unauthorized" };
            let resp = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    })
}
