//! Integration test: the `tool_exec` bridge method (native-agent-harness
//! Task 2) — the desktop executes delegated built-in tools and answers the
//! frame itself (method-aware, like `todo_update` / `sudo_exec`).
//!
//! A fake agent (the test process itself — a descendant of the anchor, the
//! test's own pid, so the peer verification accepts it) dials the REAL
//! `agent::bridge::start_listener`'s Unix-socket server, sends a `tool_exec`
//! request frame, and asserts the `response` frame the desktop writes back:
//!
//! - **read:** a `tool_exec` with `tool: "read"` on a known file returns the
//!   file content in `result.content[0].text` with **NO `error` key** (an
//!   `error: null` frame is a client-side failure — the frame keys must be
//!   byte-compatible with the existing handlers: `v`/`type`/`id`/`result`).
//! - **unknown tool:** a `tool_exec` with an unknown `tool` returns a
//!   **successful** response whose `result` is the executor's
//!   `isError: true` result (a clean tool failure the LLM sees — NOT a hard
//!   `error` frame).
//! - **abort:** a `tool_exec` with `tool: "bash"` + a client that closes the
//!   socket mid-run → the handler cancels the child (EOF →
//!   `cancel_token.cancel()`) and does not hang (the child is gone within a
//!   bounded time).
//! - **timeout:** a `bash` `timeout_ms` bounds the round-trip (the response
//!   arrives well before the 30 s fast-tool / 330 s request caps).
//!
//! Linux-only: the bridge listener is fail-closed (not started) on
//! macOS/Windows.

#[cfg(target_os = "linux")]
mod bridge_tool_exec {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::sync::{watch, Mutex};

    use archimedes_desktop_lib::agent::bridge::{self, BridgeHandle, SudoRun, SudoRunner};
    use archimedes_desktop_lib::agent::{EventSink, TodoStore};

    /// A mock `EventSink` (the test double for `TauriSink` — `tool_exec`
    /// emits no events, so the emissions are not captured).
    struct NoopSink;

    impl EventSink for NoopSink {
        fn emit(&self, _event: &str, _payload: Value) {}
    }

    /// A no-op `SudoRunner` (the listener in this test is never asked to run
    /// `sudo` — the frames it sees are `tool_exec`; the seam just has to be
    /// named for the `start_listener` signature).
    struct NoopRunner;

    impl SudoRunner for NoopRunner {
        fn run(
            &self,
            _argv: Vec<String>,
            _password: String,
            _timeout: Duration,
        ) -> Pin<Box<dyn std::future::Future<Output = SudoRun> + Send + 'static>> {
            Box::pin(async {
                SudoRun {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: "not run".to_string(),
                    timed_out: false,
                    error: Some("noop runner".to_string()),
                }
            })
        }
    }

    /// A unique temp path (the socket or a `cwd` dir).
    fn unique_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "archimedes-tool-exec-{}-{}-{}.tmp",
            std::process::id(),
            tag,
            n
        ))
    }

    /// Start the real listener (anchor = the test process's own pid — the
    /// test IS a descendant of itself, so a direct client is accepted) and
    /// return the handle + a fresh session `cwd` (the `tool_exec` sandbox
    /// root the test writes its fixtures into).
    async fn start(session_id: &str, path: &Path) -> (BridgeHandle, watch::Sender<bool>, PathBuf) {
        let (close_tx, _close_rx) = watch::channel(false);
        let cwd = unique_path("cwd");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let handle = bridge::start_listener(
            session_id.to_string(),
            path,
            std::process::id(),
            &cwd,
            Arc::new(NoopSink),
            Arc::new(Mutex::new(HashMap::new())),
            &close_tx,
            Duration::from_secs(30),
            None,
            None,
            Arc::new(TodoStore::new()),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(NoopRunner),
        )
        .await
        .expect("start_listener should bind");
        (handle, close_tx, cwd)
    }

    /// Dial the listener, send one `tool_exec` request frame (the desktop
    /// answers it itself — method-aware), and return the response frame.
    async fn tool_exec_round_trip(path: &Path, id: &str, tool: &str, params: Value) -> Value {
        // `BufReader` (like the `connect_client` in `bridge.rs`): `read_line`
        // needs `AsyncBufRead`, which the bare `UnixStream` does not
        // implement.
        let inner = tokio::net::UnixStream::connect(path)
            .await
            .expect("connect");
        let mut client = tokio::io::BufReader::new(inner);
        let frame = json!({
            "v": 1, "type": "request", "id": id, "method": "tool_exec",
            "source": "main", "params": { "tool": tool, "params": params }
        });
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        client.flush().await.expect("flush");
        let mut line = String::new();
        client.read_line(&mut line).await.expect("read response");
        serde_json::from_str(line.trim()).expect("a valid response frame")
    }

    /// Tear the listener down + remove the socket.
    fn cleanup(handle: BridgeHandle, path: &Path) {
        bridge::teardown(Some(handle));
        let _ = std::fs::remove_file(path);
    }

    /// A direct `/proc` scan (no fork+exec — the same posture as
    /// `tests/common/proc_scan`): is a LIVE (non-zombie) process's `cmdline`
    /// containing `pattern` right now.
    fn cmdline_contains(pattern: &str) -> bool {
        let needle = pattern.as_bytes();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let b = name.as_bytes();
            if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) {
                continue;
            }
            // A zombie's `cmdline` is empty → GONE (the reap assertions want
            // "the process is dead", not "the parent has reaped it").
            let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            if !cmdline.is_empty() && cmdline.windows(needle.len()).any(|w| w == needle) {
                return true;
            }
        }
        false
    }

    // (a) `tool_exec` with `tool: "read"` on a known file returns the file
    // content with **NO `error` key** (byte-compatible frame keys).
    #[tokio::test]
    async fn tool_exec_read_returns_the_file_content_without_an_error_key() {
        let path = unique_path("read");
        let (handle, _close_tx, cwd) = start("sess-read", &path).await;
        std::fs::write(cwd.join("f.txt"), "hello file\n").expect("write fixture");
        let response =
            tool_exec_round_trip(&path, "t-read", "read", json!({ "path": "f.txt" })).await;
        cleanup(handle, &path);
        let _ = std::fs::remove_dir_all(&cwd);

        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response"),
            "the response frame is a `response`"
        );
        assert_eq!(
            response.get("id").and_then(Value::as_str),
            Some("t-read"),
            "the response echoes the request id"
        );
        assert_eq!(
            response["result"]["content"][0]["text"], "hello file",
            "the file content round-trips verbatim"
        );
        assert_eq!(
            response["result"]["isError"], false,
            "a successful read is not an error"
        );
        assert!(
            response.get("error").is_none(),
            "NO `error` key on success (an `error: null` frame is a client-side failure)"
        );
        assert_eq!(
            response.as_object().map(|o| o.len()),
            Some(4),
            "the frame keys are exactly {{v, type, id, result}}: {response}"
        );
    }

    // (b) `tool_exec` with an unknown `tool` returns a **successful**
    // response whose `result` is the executor's `isError: true` result
    // (a clean tool failure the LLM sees — NOT a hard `error` frame).
    #[tokio::test]
    async fn tool_exec_unknown_tool_is_a_clean_tool_failure_not_a_hard_error() {
        let path = unique_path("unknown");
        let (handle, _close_tx, cwd) = start("sess-unknown", &path).await;
        let response = tool_exec_round_trip(&path, "t-nope", "nope", json!({})).await;
        cleanup(handle, &path);
        let _ = std::fs::remove_dir_all(&cwd);

        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response"),
            "a tool failure is a SUCCESSFUL response frame"
        );
        assert_eq!(
            response.get("id").and_then(Value::as_str),
            Some("t-nope"),
            "the response echoes the request id"
        );
        assert!(
            response.get("error").is_none(),
            "a tool failure is NOT a hard `error` frame (the LLM sees the isError result)"
        );
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["content"][0]["text"], "unknown tool: nope",
            "the executor's failure text round-trips"
        );
    }

    // (c) `tool_exec` with `tool: "bash"` + a client that closes the socket
    // mid-run → the handler cancels the child (EOF → `cancel_token.cancel()`)
    // and does not hang (the child is gone within a bounded time).
    #[tokio::test]
    async fn tool_exec_bash_cancels_the_child_when_the_peer_closes() {
        let path = unique_path("eof");
        let (handle, _close_tx, cwd) = start("sess-eof", &path).await;
        // A unique marker (the `sh` process's cmdline carries it — killing
        // the `sh` removes the marker from the process table).
        let marker = format!("archimedes-tool-exec-eof-{}", uuid::Uuid::new_v4());
        // A 2-command list (so `sh` forks + waits — the top-level `sh`'s
        // cmdline keeps the marker for the duration of the run).
        let frame = json!({
            "v": 1, "type": "request", "id": "t-eof", "method": "tool_exec",
            "source": "main", "params": { "tool": "bash", "params": { "command": format!("sleep 30; echo {marker}") } }
        });
        let inner = tokio::net::UnixStream::connect(&path)
            .await
            .expect("connect");
        let mut client = tokio::io::BufReader::new(inner);
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        client.flush().await.expect("flush");
        // Let the tool start, then hang up (the peer's close → the
        // handler's EOF).
        tokio::time::sleep(Duration::from_secs(1)).await;
        drop(client);

        // The `sh` process (cmdline carries the marker) must be GONE within
        // a bounded time (the EOF arm cancelled the token → the child was
        // killed). Bounded = the handler did not hang.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if !cmdline_contains(&marker) {
                break;
            }
            if Instant::now() > deadline {
                cleanup(handle, &path);
                let _ = std::fs::remove_dir_all(&cwd);
                panic!("the bash child survived the peer's close — the handler did not cancel it");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        cleanup(handle, &path);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    // (d) `tool_exec` with `tool: "bash"` + a `timeout_ms` bounds the
    // round-trip (the response arrives well before the 30 s fast-tool /
    // 330 s request caps — the run is bounded by the caller's `timeout_ms`).
    #[tokio::test]
    async fn tool_exec_bash_timeout_ms_bounds_the_round_trip() {
        let path = unique_path("timeout");
        let (handle, _close_tx, cwd) = start("sess-timeout", &path).await;
        let started = Instant::now();
        let response = tool_exec_round_trip(
            &path,
            "t-timeout",
            "bash",
            json!({ "command": "sleep 30", "timeout_ms": 300 }),
        )
        .await;
        let elapsed = started.elapsed();
        cleanup(handle, &path);
        let _ = std::fs::remove_dir_all(&cwd);

        assert!(
            elapsed < Duration::from_secs(5),
            "the `timeout_ms` bounded the round-trip (took {elapsed:?})"
        );
        assert_eq!(
            response.get("id").and_then(Value::as_str),
            Some("t-timeout"),
            "the response echoes the request id"
        );
        // Either the executor's own deadline won the race (a `result` frame
        // with `isError: true`) or the outer timeout arm did
        // (`error: "timeout"`) — both are bounded outcomes.
        let bounded = response.get("error").and_then(Value::as_str) == Some("timeout")
            || response["result"]["isError"] == true;
        assert!(
            bounded,
            "the timed-out run is a failed result or an error:\"timeout\" frame: {response}"
        );
    }
}
