//! The `archimedes --worker` binary smoke test (ADR 0025 Task 2 —
//! INTEGRATION target: `env!("CARGO_BIN_EXE_archimedes")` is only
//! defined for `tests/` targets, NOT for in-file `#[cfg(test)]`
//! modules): the Worker mode starts WITHOUT the Tauri runtime (the
//! flag is checked before the Tauri init — the Tauri runtime never
//! starts in a Worker), emits a `ready` JSON line, and exits 0 on a
//! `close` line / stdin EOF.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::time::Duration;

/// Spawn the built binary in Worker mode (the Tauri runtime never
/// starts — the `--worker` flag is checked before the Tauri init).
fn spawn_worker() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_archimedes"));
    cmd.arg("--worker");
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd
}

/// Read the first stdout line (the `ready` frame — bounded, so a
/// missing frame is a test failure, not a hang).
async fn read_ready_line(child: &mut tokio::process::Child) -> serde_json::Value {
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(15), reader.read_line(&mut line))
        .await
        .expect("the ready line arrives within the bound")
        .expect("stdout is readable");
    assert!(n > 0, "the first stdout line is the ready frame");
    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("the ready line is JSON");
    assert_eq!(
        v["type"], "ready",
        "the first frame is the `ready` discriminator"
    );
    v
}

/// Spawn → `ready` → a `close` line → exit 0.
#[tokio::test]
async fn worker_ready_then_close_exits_zero() {
    let mut child = spawn_worker().spawn().expect("the worker starts");
    let ready = read_ready_line(&mut child).await;
    assert!(
        ready["version"].is_string(),
        "the ready frame carries the binary version: {ready}"
    );
    // The `close` line (the `Inbound::Close` wire form).
    let stdin = child.stdin.take().expect("stdin is piped");
    let mut stdin = tokio::io::BufWriter::new(stdin);
    stdin
        .write_all(b"{\"type\":\"close\"}\n")
        .await
        .expect("the close line is written");
    // `BufWriter` buffers — flush so the Worker actually sees the line
    // (the test holds the writer open until the end — the Worker exits
    // on the `close` line itself, NOT on the stdin EOF).
    stdin.flush().await.expect("the close line is flushed");
    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("the worker exits after the close line")
        .expect("the worker process ends");
    assert!(status.success(), "close → exit 0, got {status}");
}

/// Spawn → `ready` → NO `close`, just EOF (close stdin) → exit 0 (the
/// clean-EOF path).
#[tokio::test]
async fn worker_stdin_eof_exits_zero() {
    let mut child = spawn_worker().spawn().expect("the worker starts");
    let _ready = read_ready_line(&mut child).await;
    // No `close` — just EOF (drop the stdin handle).
    drop(child.stdin.take().expect("stdin is piped"));
    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("the worker exits on stdin EOF")
        .expect("the worker process ends");
    assert!(status.success(), "stdin EOF → exit 0, got {status}");
}
