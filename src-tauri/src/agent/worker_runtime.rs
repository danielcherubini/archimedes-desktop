//! A dedicated tokio runtime for subagent sessions (ADR 0004).
//!
//! Subagent sessions run on this worker runtime because two concurrent ACP
//! sessions on ONE runtime hung `send_prompt` 60+ seconds (2026-09-15
//! repro; root cause never confirmed — suspected SDK/async-io level: two
//! long-lived per-connection transport tasks sharing one global async-io
//! reactor). The regression test in `tests/subagent_concurrency.rs` is the
//! build gate for the whole feature.
//!
//! **Drop-safety constraint (reviewed):** tokio PANICS with "Cannot drop a
//! runtime in a context where blocking is not allowed" when a `Runtime` is
//! dropped from an async context — so the `Runtime` is owned by a dedicated
//! OS thread that blocks on a shutdown signal and drops the runtime on THAT
//! thread (never on the calling/async thread).
//!
//! Channel-based handoff only: tasks are handed to the worker runtime via
//! `spawn_task` (oneshot result delivery) — never `block_on` across
//! runtimes.
//!
//! **Panic semantics (profile-dependent):** under `panic = "unwind"`
//! (the dev/test profiles) a panicked task is contained (a panicked
//! spawned task does not kill the runtime) and logged by the
//! `spawn_task` supervisor. In RELEASE builds `[profile.release]` sets
//! `panic = "abort"`: a panicking task ABORTS THE WHOLE PROCESS by
//! design (the supervisor's `JoinError::is_panic()` branch is never
//! reached — the process is already gone).

use std::future::Future;
use std::sync::mpsc;
use std::thread;

use tokio::sync::oneshot;

/// Owns a dedicated tokio runtime on a dedicated OS thread.
///
/// The dedicated thread builds the `Runtime`, hands out a `Handle`, then
/// blocks on the shutdown signal — the `Runtime` is dropped on the dedicated
/// (non-async) thread, never from an async context. Dropping the
/// `WorkerRuntime` drops the shutdown `Sender`, which unblocks the dedicated
/// thread (it then drops the `Runtime` on itself and exits) — the app-exit
/// path needs no explicit `Drop` impl (a `Drop` impl would forbid
/// `shutdown_and_join` from taking `self` by value — E0509).
pub struct WorkerRuntime {
    /// Handle captured before the `Runtime` moves into the dedicated thread.
    handle: tokio::runtime::Handle,
    shutdown_tx: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}

impl WorkerRuntime {
    /// Build the worker runtime on a dedicated OS thread (a multi-thread
    /// runtime with worker_threads = 2 — its own thread pool, separate from
    /// the app runtime's). Built at app setup (`setup_dirs`); two idle
    /// threads are negligible. The dedicated thread owns the `Runtime`,
    /// hands out a `Handle`, then blocks on `shutdown_rx` — dropping the
    /// `Runtime` happens on this thread (tokio panics if a runtime is
    /// dropped from an async context).
    ///
    /// Failures are an `io::Error` (NOT a panic — the app degrades instead
    /// of crashing at startup): a thread-spawn refusal (EAGAIN under load —
    /// the exact failure `acp_flow` retries for `fork()`), a runtime-build
    /// failure, or a handle-handoff failure. A build / handoff failure inside
    /// the dedicated thread exits the thread (logged) — the caller's
    /// `handle_rx.recv()` sees the (dropped) sender and reports the failure
    /// (no misleading double panic).
    // `new_without_default` is allowed: a `WorkerRuntime` is a heavyweight
    // resource (a thread pool + OS thread), not a value with a natural
    // default, and the public surface is intentionally just
    // `new()` + `spawn_task()` + `shutdown_and_join()`.
    #[allow(clippy::new_without_default)]
    pub fn new() -> std::io::Result<Self> {
        let (handle_tx, handle_rx) = mpsc::channel::<tokio::runtime::Handle>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("archimedes-subagents".into())
            .spawn(move || {
                // A runtime-build failure (or a handoff failure) logs and
                // exits the thread: the caller's `handle_rx.recv()` sees
                // the (dropped) sender and reports the failure — no
                // misleading double panic.
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("archimedes-subagents-worker")
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("worker runtime: build failed: {e}");
                        return;
                    }
                };
                let handle = rt.handle().clone();
                if handle_tx.send(handle).is_err() {
                    eprintln!("worker runtime: handle handoff failed");
                    return;
                }
                let _ = shutdown_rx.recv(); // block until shutdown
                drop(rt); // dropped on this dedicated (non-async) thread
            })
            .map_err(|e| std::io::Error::other(format!("worker thread spawn: {e}")))?;
        let handle = handle_rx
            .recv()
            .map_err(|_| {
                std::io::Error::other(
                    "the dedicated thread exited before handing off its handle (a runtime build or handoff failure — see the worker thread's log)",
                )
            })?;
        Ok(Self {
            handle,
            shutdown_tx,
            thread,
        })
    }

    /// Run `f` on the worker runtime; the result is delivered through the
    /// returned oneshot. The caller never blocks on the worker runtime.
    ///
    /// A PANIC in the task is contained (a panicked spawned task does not
    /// kill the worker runtime) and reported through `JoinError::is_panic()`
    /// — LOGGED (a crash must not be silently reported as a cancel with no
    /// log line anywhere) — under `panic = "unwind"` (the dev/test
    /// profiles). In RELEASE builds `[profile.release]` sets
    /// `panic = "abort"`: a panicking task aborts the WHOLE PROCESS by
    /// design (the supervisor's `is_panic()` branch is never reached —
    /// the process is already gone). The oneshot is dropped on a panic
    /// (unwind): the caller sees a transport-level cancel (the log line
    /// is the crash signal).
    pub fn spawn_task<F>(&self, f: F) -> oneshot::Receiver<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        // The inner task runs `f` on the worker runtime.
        let inner = self.handle.spawn(async move {
            let out = f.await;
            let _ = tx.send(out);
        });
        // A supervisor (on the worker runtime too): observes a panicked
        // inner task (`JoinError::is_panic()`) — unwind profiles only
        // (release builds abort the process on a panic by design, so
        // this branch is never reached there). A CANCELLED inner task
        // (the worker runtime shut down while the task was in flight) is
        // NOT a panic — the oneshot is simply dropped (the caller sees a
        // transport-level cancel, as before).
        self.handle.spawn(async move {
            if let Err(e) = inner.await {
                if e.is_panic() {
                    eprintln!("worker runtime: a task panicked: {e}");
                }
            }
        });
        rx
    }

    /// Send the shutdown signal and join the dedicated thread (the thread
    /// drops the `Runtime` on itself and exits). Use from tests to await
    /// teardown; dropping the `WorkerRuntime` (the shutdown `Sender` is
    /// dropped, unblocking the thread — no join, since you cannot join
    /// self) covers the app-exit path.
    pub fn shutdown_and_join(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.thread.join();
    }
}
