//! Integration test: drive the REAL `acp::bridge::start_listener` with a real
//! external (Python) stub agent speaking the bridge protocol — the
//! end-to-end validation gate (Task 6).
//!
//! The unit tests in `bridge.rs` cover the pieces; this test exercises the
//! full listener (the accept loop + peer verification + `handle_connection`)
//! against a real, out-of-process peer:
//!
//! - **Happy path:** a descendant stub (a child of the test process) connects,
//!   sends a `request` frame, and receives the `response` frame the test
//!   resolves via the `pending_bridge` oneshot (the `respond_bridge_request`
//!   mechanism). The `EventSink` sees a `bridge-request` with the right
//!   `method`/`params`. (This also proves a descendant is ACCEPTED.)
//! - **Peer-verification rejection (I8):** a DOUBLE-FORKED ORPHAN (a process
//!   that forks a grandchild then exits, so the grandchild is reparented to
//!   init — its parent chain never reaches the test process, the anchor)
//!   connects → the connection is rejected (closed, no `bridge-request`
//!   emitted). A plain spawned stub is NOT a valid foreign process here (it's
//!   a descendant → accepted); the double-fork makes it a true foreign one.
//! - **Push `seq` drop:** two `push` frames with the same `seq` → one
//!   `bridge-event`.
//! - **Terminal frame on timeout (M4):** a request that is not answered within
//!   the injectable (shrunk to 50 ms) timeout → the stub receives the
//!   `error:"cancelled"` terminal frame.
//!
//! The **anchor is the test process's own pid** (`std::process::id()` — the
//! "desktop" in the test). Peer verification uses the REAL `ProcfsReader`
//! (`SO_PEERCRED` + `/proc/<pid>/status`), so a spawned descendant is accepted
//! and a double-forked orphan is rejected.
//!
//! Linux-only: the bridge listener is fail-closed (not started) on
//! macOS/Windows, and the double-fork orphan relies on Unix reparenting.

#[cfg(target_os = "linux")]
mod bridge_integration {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};
    use tokio::sync::{watch, Mutex};

    use archimedes_desktop_lib::acp::bridge::{self, BridgeHandle};
    use archimedes_desktop_lib::acp::{EventSink, PendingBridge};

    /// The Python stub. It writes its result to the file given as arg 3.
    ///
    /// Modes:
    /// - `request <id>`: connect, send a `request` frame (method `ask`), read
    ///   the `response` frame, write it (or `CLOSED` if the connection is
    ///   dropped before a frame arrives).
    /// - `push <seq>`: open TWO connections, each sending a `push` frame with
    ///   the same `seq` (one frame per connection), write `ack` per connection.
    /// - `orphan`: double-fork — the forker (a child of the test) forks the
    ///   orphan and exits, so the orphan is reparented to init; the orphan
    ///   connects, sends a `request` frame, and writes `CONNECTED` then
    ///   `REJECTED` (closed) or `ACCEPTED`.
    const STUB: &str = r#"
import sys, socket, os, time, json

def main():
    sock_path = sys.argv[1]
    mode = sys.argv[2]
    out_file = sys.argv[3]
    arg = sys.argv[4] if len(sys.argv) > 4 else ""

    def write(msg):
        with open(out_file, "a") as f:
            f.write(msg + "\n")

    def connect():
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(sock_path)
        return s

    def read_line(s):
        buf = b""
        while b"\n" not in buf:
            chunk = s.recv(4096)
            if not chunk:
                return None
            buf += chunk
        return buf.split(b"\n")[0].decode("utf-8", "replace")

    if mode == "request":
        rid = arg
        s = connect()
        frame = {"v": 1, "type": "request", "id": rid, "method": "ask",
                 "source": "main", "toolCallId": "tc-" + rid,
                 "params": {"questions": [{"id": "q1", "question": "Pick one",
                                           "options": [{"label": "A"}, {"label": "B"}]}]}}
        s.sendall((json.dumps(frame) + "\n").encode())
        resp = read_line(s)
        write(resp if resp is not None else "CLOSED")
        s.close()
    elif mode == "push":
        seq = int(arg)
        for _ in range(2):
            s = connect()
            frame = {"v": 1, "type": "push", "seq": seq, "event": "state",
                     "payload": {"state": "working"}}
            s.sendall((json.dumps(frame) + "\n").encode())
            ack = read_line(s)
            write("ack" if ack == "ack" else (ack if ack is not None else "CLOSED"))
            s.close()
    elif mode == "orphan":
        pid = os.fork()
        if pid == 0:
            # orphan: wait for the forker to exit (reparent to init), then connect
            time.sleep(0.5)
            try:
                s = connect()
                write("CONNECTED")
                frame = {"v": 1, "type": "request", "id": "orphan-1",
                         "method": "ask", "source": "main", "params": {}}
                s.sendall((json.dumps(frame) + "\n").encode())
                resp = read_line(s)
                write("REJECTED" if resp is None else "ACCEPTED")
                s.close()
            except Exception:
                write("REJECTED")
            os._exit(0)
        else:
            # forker: exit immediately so the orphan is reparented to init
            os._exit(0)

main()
"#;

    /// A mock `EventSink` that captures emissions (the test double for
    /// `TauriSink`).
    #[derive(Default)]
    struct CapturingSink(StdMutex<Vec<(String, Value)>>);

    impl EventSink for CapturingSink {
        fn emit(&self, event: &str, payload: Value) {
            self.0.lock().unwrap().push((event.to_string(), payload));
        }
    }

    impl CapturingSink {
        fn events_named(&self, name: &str) -> Vec<Value> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, p)| p.clone())
                .collect()
        }
    }

    /// Mirror of `SessionManager::respond_bridge_request`: resolve the pending
    /// oneshot by the compound key `"{session_id}/{request_id}"` and send the
    /// `result` verbatim. The test drives `start_listener` with a standalone
    /// `pending_bridge` map, so it resolves the oneshot the same way the
    /// command does.
    async fn respond_bridge_request(
        pending: &PendingBridge,
        session_id: &str,
        request_id: &str,
        result: Value,
    ) {
        let key = bridge::bridge_key(session_id, request_id);
        if let Some(sender) = pending.lock().await.remove(&key) {
            let _ = sender.send(result);
        }
    }

    /// A unique temp path (socket or output file).
    fn unique_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "archimedes-bridge-int-{}-{}-{}.tmp",
            std::process::id(),
            tag,
            n
        ))
    }

    /// Write the Python stub to a unique temp file and return its path.
    fn write_stub() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "archimedes-bridge-stub-{}-{}.py",
            std::process::id(),
            n
        ));
        std::fs::write(&path, STUB).expect("write stub");
        path
    }

    /// Start the real listener (anchor = the test process's own pid) and
    /// return the handle + the (unused) close sender.
    async fn start(
        session_id: &str,
        path: &Path,
        sink: Arc<CapturingSink>,
        pending: PendingBridge,
        timeout: Duration,
    ) -> (BridgeHandle, watch::Sender<bool>) {
        let (close_tx, _close_rx) = watch::channel(false);
        let handle = bridge::start_listener(
            session_id.to_string(),
            path,
            std::process::id(),
            sink,
            pending,
            &close_tx,
            timeout,
            None,
            None,
        )
        .await
        .expect("start_listener should bind");
        (handle, close_tx)
    }

    /// Poll the sink until a `bridge-request` with `request_id` appears.
    async fn wait_for_request(
        sink: &CapturingSink,
        request_id: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(p) = sink
                .events_named("bridge-request")
                .iter()
                .find(|p| p.get("requestId").and_then(Value::as_str) == Some(request_id))
            {
                return Ok(p.clone());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "bridge-request {request_id} not emitted in time; got: {:?}",
                    sink.events_named("bridge-request")
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Poll a file until a line containing `needle` appears (or `timeout`).
    async fn wait_for_file_line(
        path: &Path,
        needle: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(line) = content.lines().find(|l| l.contains(needle)) {
                    return Ok(line.to_string());
                }
            }
            if Instant::now() > deadline {
                let got = std::fs::read_to_string(path).unwrap_or_default();
                return Err(format!(
                    "timeout waiting for {needle:?} in {path:?}; got: {got}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Detect whether `python3` is usable (the suite is the end-to-end
    /// gate on dev machines, but a CI runner without python3 must not
    /// hard-fail — the tests skip instead).
    fn python3_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
            .and_then(|mut c| c.wait().ok())
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Spawn the Python stub (`python3 <stub> <socket> <mode> <arg>`) and
    /// return the child; `None` when `python3` is absent (the caller
    /// skips the test rather than panicking).
    fn spawn_stub(
        stub: &Path,
        socket: &Path,
        mode: &str,
        out_file: &Path,
        arg: &str,
    ) -> Option<std::process::Child> {
        if !python3_available() {
            eprintln!("bridge integration test: python3 not found on PATH — skipping");
            return None;
        }
        Some(
            std::process::Command::new("python3")
                .arg(stub)
                .arg(socket)
                .arg(mode)
                .arg(out_file)
                .arg(arg)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn python stub"),
        )
    }

    /// **Happy path:** a descendant stub connects, sends a `request` frame,
    /// and receives the `response` frame the test resolves via the
    /// `pending_bridge` oneshot. Also proves a descendant is ACCEPTED (a
    /// rejected connection would yield `CLOSED`, not the response).
    #[tokio::test]
    async fn happy_path_request_response_round_trip() {
        // python3 gate FIRST (before `start`): `start` binds the socket, so
        // an early skip must not leak the bound socket file (the teardown
        // below is the cleanup that a top-of-test skip would otherwise
        // skip).
        if !python3_available() {
            eprintln!("bridge integration test: python3 not found on PATH — skipping");
            return;
        }
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let path = unique_path("happy");
        let (handle, _close_tx) = start(
            "sess-happy",
            &path,
            sink.clone(),
            pending.clone(),
            Duration::from_secs(30),
        )
        .await;

        let stub = write_stub();
        let out_file = unique_path("happy-out");
        let Some(mut child) = spawn_stub(&stub, &path, "request", &out_file, "r-1") else {
            // python3 absent — the suite is skipped (not a failure). Route
            // through the same cleanup so a narrow-race skip after `start`
            // doesn't leak the bound socket file.
            bridge::teardown(Some(handle));
            let _ = std::fs::remove_file(&path);
            return;
        };

        // The stub (a DESCENDANT of the test process) connects → accepted →
        // the request is emitted with the right method/params.
        let req = wait_for_request(&sink, "r-1", Duration::from_secs(5))
            .await
            .expect("bridge-request emitted");
        assert_eq!(req.get("method").and_then(Value::as_str), Some("ask"));
        assert_eq!(
            req.get("sessionId").and_then(Value::as_str),
            Some("sess-happy")
        );
        assert!(
            req.get("params").and_then(|p| p.get("questions")).is_some(),
            "params carry the ask questions"
        );

        // The user answers — resolve the oneshot with a canned
        // `AskResponsePayload` (the `respond_bridge_request` mechanism).
        let result = json!({
            "cancelled": false,
            "results": [ { "id": "q1", "selectedOptions": ["A"] } ]
        });
        respond_bridge_request(&pending, "sess-happy", "r-1", result.clone()).await;

        // The stub reads the response frame and writes it to the out-file.
        let line = wait_for_file_line(&out_file, "response", Duration::from_secs(5))
            .await
            .expect("the stub got a response frame (a descendant is accepted)");
        let response: Value = serde_json::from_str(&line).expect("response is JSON");
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response")
        );
        assert_eq!(response.get("id").and_then(Value::as_str), Some("r-1"));
        assert_eq!(
            response.get("result"),
            Some(&result),
            "the result is written verbatim (no wrapper)"
        );
        assert!(response.get("error").is_none());

        let _ = child.wait();
        bridge::teardown(Some(handle));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out_file);
        let _ = std::fs::remove_file(&stub);
    }

    /// **Peer-verification rejection (I8):** a double-forked orphan (reparented
    /// to init — its parent chain never reaches the test process, the anchor)
    /// connects → the connection is rejected (closed, no `bridge-request`
    /// emitted). This is the security-critical path.
    #[tokio::test]
    async fn peer_verification_rejects_a_double_forked_orphan() {
        // python3 gate FIRST (before `start`) — see the happy-path test.
        if !python3_available() {
            eprintln!("bridge integration test: python3 not found on PATH — skipping");
            return;
        }
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let path = unique_path("orphan");
        let (handle, _close_tx) = start(
            "sess-orphan",
            &path,
            sink.clone(),
            pending.clone(),
            Duration::from_secs(30),
        )
        .await;

        let stub = write_stub();
        let out_file = unique_path("orphan-out");
        // Double-fork: the forker (a child of the test) forks the orphan and
        // exits, so the orphan is reparented to init — a TRUE foreign process.
        let Some(mut child) = spawn_stub(&stub, &path, "orphan", &out_file, "") else {
            // python3 absent — the suite is skipped (not a failure). Route
            // through the same cleanup so a narrow-race skip after `start`
            // doesn't leak the bound socket file.
            bridge::teardown(Some(handle));
            let _ = std::fs::remove_file(&path);
            return;
        };

        // Wait for the orphan to connect (it sleeps 0.5 s for reparenting,
        // then connects and writes "CONNECTED").
        let _ = wait_for_file_line(&out_file, "CONNECTED", Duration::from_secs(5))
            .await
            .expect("the orphan connected");
        // Give the accept loop a moment to reject + drop the stream.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // The foreign process is REJECTED: no `bridge-request` is emitted.
        assert!(
            sink.events_named("bridge-request").is_empty(),
            "a double-forked orphan must be rejected (no bridge-request emitted)"
        );

        // (Secondary) the orphan's read returns EOF → it writes "REJECTED"
        // (the connection was closed, not answered).
        let outcome = wait_for_file_line(&out_file, "REJECTED", Duration::from_secs(5))
            .await
            .expect("the orphan's connection was closed");
        assert!(
            outcome.contains("REJECTED"),
            "the orphan's connection is closed (rejected), not answered"
        );

        let _ = child.wait();
        bridge::teardown(Some(handle));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out_file);
        let _ = std::fs::remove_file(&stub);
    }

    /// **Push `seq` drop:** two `push` frames with the same `seq` (two
    /// connections, one frame each) → only one `bridge-event` is emitted.
    #[tokio::test]
    async fn push_frames_are_seq_deduped() {
        // python3 gate FIRST (before `start`) — see the happy-path test.
        if !python3_available() {
            eprintln!("bridge integration test: python3 not found on PATH — skipping");
            return;
        }
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let path = unique_path("seqdrop");
        let (handle, _close_tx) = start(
            "sess-seq",
            &path,
            sink.clone(),
            pending.clone(),
            Duration::from_secs(30),
        )
        .await;

        let stub = write_stub();
        let out_file = unique_path("seqdrop-out");
        // The stub opens TWO connections, each sending a `push` frame with the
        // same `seq`. The first is delivered, the second (seq <= last_seq) is
        // dropped (but still acked).
        let Some(mut child) = spawn_stub(&stub, &path, "push", &out_file, "1") else {
            // python3 absent — the suite is skipped (not a failure). Route
            // through the same cleanup so a narrow-race skip after `start`
            // doesn't leak the bound socket file.
            bridge::teardown(Some(handle));
            let _ = std::fs::remove_file(&path);
            return;
        };

        // Wait for both connections to be acked (the stub writes "ack" per
        // connection → two "ack" lines).
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(content) = std::fs::read_to_string(&out_file) {
                let acks = content.lines().filter(|l| l.contains("ack")).count();
                if acks >= 2 {
                    break;
                }
            }
            if Instant::now() > deadline {
                panic!(
                    "timeout waiting for 2 acks; got: {:?}",
                    std::fs::read_to_string(&out_file).unwrap_or_default()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Give any (erroneous) second `bridge-event` a moment to appear.
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(
            sink.events_named("bridge-event").len(),
            1,
            "a duplicate seq emits no second bridge-event"
        );

        let _ = child.wait();
        bridge::teardown(Some(handle));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out_file);
        let _ = std::fs::remove_file(&stub);
    }

    /// **Terminal frame on timeout (M4):** a request that is not answered
    /// within the injectable (shrunk to 50 ms) timeout → the stub receives the
    /// `error:"cancelled"` terminal frame.
    #[tokio::test]
    async fn a_timeout_writes_the_terminal_cancelled_frame() {
        // python3 gate FIRST (before `start`) — see the happy-path test.
        if !python3_available() {
            eprintln!("bridge integration test: python3 not found on PATH — skipping");
            return;
        }
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let path = unique_path("timeout");
        // The injectable timeout (the `Duration` parameter) shrunk to 50 ms.
        let (handle, _close_tx) = start(
            "sess-timeout",
            &path,
            sink.clone(),
            pending.clone(),
            Duration::from_millis(50),
        )
        .await;

        let stub = write_stub();
        let out_file = unique_path("timeout-out");
        let Some(mut child) = spawn_stub(&stub, &path, "request", &out_file, "r-2") else {
            // python3 absent — the suite is skipped (not a failure). Route
            // through the same cleanup so a narrow-race skip after `start`
            // doesn't leak the bound socket file.
            bridge::teardown(Some(handle));
            let _ = std::fs::remove_file(&path);
            return;
        };

        // Wait for the request to be emitted (the stub sent it).
        let _ = wait_for_request(&sink, "r-2", Duration::from_secs(5))
            .await
            .expect("bridge-request emitted");
        // Do NOT answer — the waiter times out (50 ms) and writes the
        // explicit terminal frame, then closes.
        let line = wait_for_file_line(&out_file, "cancelled", Duration::from_secs(5))
            .await
            .expect("the stub got the terminal frame");
        let response: Value = serde_json::from_str(&line).expect("response is JSON");
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response")
        );
        assert_eq!(response.get("id").and_then(Value::as_str), Some("r-2"));
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("cancelled"),
            "a timeout writes the terminal error:cancelled frame"
        );
        assert!(response.get("result").is_none());

        let _ = child.wait();
        bridge::teardown(Some(handle));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out_file);
        let _ = std::fs::remove_file(&stub);
    }
}
