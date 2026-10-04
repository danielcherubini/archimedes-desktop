//! The Worker process (ADR 0025 Task 2): the desktop's own binary
//! re-entered with `--worker` — one session's `AgentLoop` over a
//! stdio JSONL protocol with the Supervisor.
//!
//! The Tauri runtime NEVER starts in a Worker (`main` checks the flag
//! before the Tauri init). The Worker: installs the crash-logging
//! panic hook (tag `"worker"`), builds the `WorkerCore`, emits
//! `Ready`, then observes three things concurrently — the `events_rx`
//! pump task (each `RpcEvent` → an `Outbound::Event` frame on stdout),
//! the crash-watch task (the loop `JoinHandle` — `Err` (a panic — the
//! hook already wrote the crash log) → a best-effort `WorkerError`
//! frame + `std::process::exit(1)`; a panic in a `tokio::spawn`'d task
//! is otherwise SWALLOWED by tokio, and without this watch a panicked
//! loop would leave the Worker running forever with a dead loop), and
//! the stdin line stream (each line → `decode_inbound` →
//! `core.handle` → the outbound frames to stdout). Exit 0 when the
//! core is closed or stdin hits EOF.

pub mod client;
pub mod core;
pub mod dispatch;
pub mod manager;
pub mod protocol;
pub mod sink;
pub mod store;

pub use client::{WorkerError, WorkerErrorKind, WorkerHandle, WorkerInboundEvent};
pub use core::{default_build_loop, LoopControl, LoopHandles, WorkerCore};
pub use dispatch::IpcDispatcher;
pub use manager::{SubagentCapture, WorkerFactory, WorkerManager, DEFAULT_SUBAGENT_SETTLE_TIMEOUT};
pub use protocol::{
    decode_inbound, decode_outbound, encode_inbound, encode_outbound, Inbound, Outbound,
    ProtoError, StartEnv, StartMode, StoreFrame, SubagentDispatchWire,
};
pub use sink::IpcEventSink;
pub use store::IpcStore;

use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;

/// The Worker main (the `run_worker` entry's async half — the stdio
/// JSONL loop over the `WorkerCore`).
pub async fn run_worker_inner() {
    crate::agent::crashlog::install_panic_hook("worker");
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();
    let (mut core, _inbound_tx) = WorkerCore::new(outbound_tx);
    // The stdout writer (the two producers — the pump task and the main
    // loop — serialized; the TOKIO `Stdout` (the async type — `io-std`
    // feature) behind the `Mutex`).
    let stdout = std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    // The pump task's completion signal (the `events_rx` channel
    // closed — the loop task ended; a no-op: the `closed` / stdin-EOF
    // conditions decide exit). An `mpsc` (NOT a oneshot — the `select!`
    // arms must be re-pollable across loop iterations) + a guard flag
    // (a closed + empty channel would otherwise fire the arm forever —
    // the guard disables it after the first fire).
    let (pump_done_tx, mut pump_done_rx) = tokio::sync::mpsc::channel(1);
    let mut pump_done_tx = Some(pump_done_tx);
    // The crash-watch's normal-end signal (the `JoinHandle` resolved
    // `Ok(())` — the normal loop end, e.g. after `Close`; a no-op: the
    // `closed` / stdin-EOF conditions decide exit — do NOT exit on
    // `Ok`). The `Err` (panic) path exits directly (the frame +
    // `std::process::exit(1)`).
    let (crash_ok_tx, mut crash_ok_rx) = tokio::sync::mpsc::channel(1);
    let mut crash_ok_tx = Some(crash_ok_tx);
    let mut pump_done = false;
    let mut crash_ok = false;

    // The `Ready` frame (the binary version — emitted at startup,
    // BEFORE `Start`).
    write_frame(
        &stdout,
        &Outbound::Ready {
            version: env!("CARGO_PKG_VERSION").to_string(),
            session_id: None,
        },
    )
    .await;

    let mut line_buf = String::new();
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        tokio::select! {
            n = stdin.read_line(&mut line_buf) => {
                match n {
                    Ok(0) => break, // stdin EOF (the clean-EOF path — exit 0).
                    Ok(_) => {
                        let line = line_buf
                            .trim_end_matches(['\n', '\r'])
                            .to_string();
                        line_buf.clear();
                        if line.is_empty() {
                            continue;
                        }
                        match decode_inbound(&line) {
                            Ok(msg) => {
                                for frame in core.handle(msg).await {
                                    write_frame(&stdout, &frame).await;
                                }
                                // The `Close` — the Worker is done
                                // (exit 0; the `closed` condition decides
                                // exit — the Supervisor does NOT close
                                // the stdin pipe on `Close`):
                                if core.is_closed() {
                                    break;
                                }
                                // The loop just came up (the `Start`):
                                // spawn the pump task (the `events_rx`
                                // moved out of the core) + the
                                // crash-watch task (the `JoinHandle` —
                                // a panic in a `tokio::spawn`'d task is
                                // otherwise SWALLOWED by tokio —
                                // surfaces only as a `JoinError`;
                                // without this watch a panicked loop
                                // would leave the Worker running forever
                                // with a dead loop — no exit code, no
                                // `on_crash`, the session hangs instead
                                // of stalling).
                                // One loop per Worker — the `Option`
                                // `take`s make the spawn-once provable
                                // (the compiler can't track the core's
                                // state across loop iterations).
                                if let Some((events_rx, task)) = core.take_pump() {
                                    let pump_stdout = stdout.clone();
                                    let pump_done_tx = pump_done_tx.take().expect(
                                        "the pump sender is taken once",
                                    );
                                    tokio::spawn(async move {
                                        let mut rx = events_rx;
                                        while let Some(event) = rx.recv().await {
                                            let frame = Outbound::Event { event };
                                            let line = encode_outbound(&frame);
                                            let mut w = pump_stdout.lock().await;
                                            if w.write_all(line.as_bytes()).await.is_err()
                                                || w.flush().await.is_err()
                                            {
                                                break; // stdout gone — the Supervisor is gone.
                                            }
                                        }
                                        // The task's end drops the
                                        // `pump_done_tx` sender (the
                                        // `mpsc` channel closes — the
                                        // `pump_done` arm fires once;
                                        // `drop` of the send future is
                                        // the clippy-clean no-op — the
                                        // arm is a no-op anyway).
                                        drop(pump_done_tx.send(()));
                                    });
                                    let crash_stdout = stdout.clone();
                                    let crash_ok_tx = crash_ok_tx.take().expect(
                                        "the crash-watch sender is taken once",
                                    );
                                    tokio::spawn(async move {
                                        match task.await {
                                            // `Ok(())` — the normal loop
                                            // end (e.g. after `Close`):
                                            // a no-op signal (the
                                            // `closed` / stdin-EOF
                                            // conditions decide exit —
                                            // do NOT exit on `Ok`).
                                            Ok(()) => {
                                                // The task's end drops
                                                // the `crash_ok_tx`
                                                // sender (the channel
                                                // closes — the arm
                                                // fires once; `drop` of
                                                // the send future is the
                                                // clippy-clean no-op —
                                                // the arm is a no-op
                                                // anyway).
                                                drop(crash_ok_tx.send(()));
                                            }
                                            // `Err` (a panic — the panic
                                            // hook already wrote the
                                            // crash log): best-effort
                                            // `WorkerError` frame + a
                                            // NON-ZERO exit (the
                                            // Supervisor sees the crash
                                            // via the exit code — the
                                            // session stalls, the app
                                            // lives).
                                            Err(e) => {
                                                let frame = Outbound::WorkerError {
                                                    code: "worker_panic".to_string(),
                                                    message: e.to_string(),
                                                    backtrace: None,
                                                };
                                                let line = encode_outbound(&frame);
                                                let mut w = crash_stdout.lock().await;
                                                let _ = w.write_all(line.as_bytes()).await;
                                                let _ = w.flush().await;
                                                std::process::exit(1);
                                            }
                                        }
                                    });
                                }
                                if core.is_closed() {
                                    break; // the `Close` — exit 0.
                                }
                            }
                            Err(e) => {
                                eprintln!("worker: bad inbound line: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("worker: stdin read failed: {e}");
                        break;
                    }
                }
            }
            frame = outbound_rx.recv() => {
                // The store / sink frames (the `IpcStore` /
                // `IpcEventSink` / `IpcDispatcher` write them as the
                // loop runs — NOT only after a `handle` returns).
                match frame {
                    Some(f) => {
                        write_frame(&stdout, &f).await;
                    }
                    None => break, // the core's sender is gone.
                }
            }
            _ = pump_done_rx.recv(), if !pump_done => {
                // The loop's event stream ended (the loop task ended —
                // its `events` sender dropped). A no-op: the `closed`
                // / stdin-EOF conditions decide exit. The guard
                // disables the arm after the first fire (a closed +
                // empty channel would otherwise fire it forever).
                pump_done = true;
            }
            _ = crash_ok_rx.recv(), if !crash_ok => {
                // The loop task ended NORMALLY (`Ok(())` — e.g. after
                // the `Close` cancel). A no-op.
                crash_ok = true;
            }
        }
    }
}

/// Write one outbound frame to stdout (the shared writer — the pump
/// task and the main loop serialize on the `Mutex`).
async fn write_frame(
    stdout: &std::sync::Arc<tokio::sync::Mutex<tokio::io::Stdout>>,
    frame: &Outbound,
) {
    let line = encode_outbound(frame);
    let mut w = stdout.lock().await;
    if w.write_all(line.as_bytes()).await.is_err() || w.flush().await.is_err() {
        eprintln!("worker: stdout write failed (the Supervisor is gone?)");
    }
}
