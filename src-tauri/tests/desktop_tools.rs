//! End-to-end integration test for the Phase 2 desktop tools (Task 3): the
//! override's bridge round-trips (`todo_update` / `sudo_exec` / `ask`) are
//! driven over the REAL `agent::bridge::start_listener` with a `fake_pi` in
//! the `FAKE_PI_TOOLS` mode (the wire simulation). This proves the
//! end-to-end contract (the plan's `done-when`): the override → the bridge →
//! the desktop → the result.
//!
//! The test wiring mirrors `bridge_integration.rs`: a real `start_listener`
//! (anchor = the test process's own pid) + a `fake_pi` spawned DIRECTLY (a
//! child of the test — a descendant, so the peer-verification passes) with
//! the bridge env set by the TEST (`PI_ARCHIMEDES_BRIDGE=1` + `_SOCKET` /
//! `_SESSION` / `_SERVER_PID`). The fake connects to the socket and sends the
//! `todo_update` / `sudo_exec` / `ask` frames the override would send, printing
//! each received response to stdout (one JSON line — the fake→test assertion
//! channel).
//!
//! **Correlation ids (CRITICAL for the `sudo_exec` sub-prompts):** the fake
//! uses FIXED frame ids (`todo-1`/`todo-2`/`sudo-1`/`sudo-2`/`ask-1`). The
//! `sudo_exec` sub-prompts are method-aware oneshots in `pending_sudo` keyed
//! `"{sid}/{id}:confirm"` / `"{sid}/{id}:password"` (the desktop emits a
//! `bridge-request` with the DERIVED `requestId`); the test reads the derived
//! `requestId`s from the capturing `EventSink`'s `bridge-request` events (the
//! `wait_for_request` path in `bridge_integration.rs`) and resolves the
//! oneshots via `respond` BEFORE any response exists. Without fixed/known ids
//! the test could not pre-empt the oneshots.
//!
//! The built-in-tools-untouched invariant is asserted in the `tools.rs` unit
//! tests (the authoritative check — `tools_spawn_args` appends only `-e <path>`
//! and never `--no-builtin-tools`/`--tools`), NOT here: the e2e spawns
//! `fake_pi` itself, so "the absence of any `--no-builtin-tools`/`--tools` arg
//! in the spawn" asserts nothing about the desktop's spawn composition, and
//! `fake_pi` exposes no `get_state` tool list.
//!
//! Linux-only: the bridge listener is fail-closed (not started) on
//! macOS/Windows.

#[cfg(target_os = "linux")]
mod desktop_tools {
    use std::collections::HashMap;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};
    use tokio::sync::{watch, Mutex};

    use archimedes_desktop_lib::agent::bridge::{self, BridgeHandle};
    use archimedes_desktop_lib::agent::{
        CachedPassword, EventSink, PendingBridge, PendingSudo, SudoRun, SudoRunner, TodoStore,
    };

    /// The full path to the compiled `fake_pi` binary.
    const FAKE_PI: &str = env!("CARGO_BIN_EXE_fake_pi");

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

    /// A `SudoRunner` that "runs" the command (returns `exit_code: 0` + a
    /// stdout marker) and counts its calls — the "no runner call" assertion
    /// for the denied `sudo_exec` (the runner is called exactly once, for the
    /// confirmed case; the denied case returns before the runner).
    struct FakeRunner {
        calls: AtomicUsize,
    }

    impl SudoRunner for FakeRunner {
        fn run(
            &self,
            _argv: Vec<String>,
            _password: String,
            _timeout: Duration,
        ) -> Pin<Box<dyn std::future::Future<Output = SudoRun> + Send + 'static>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                SudoRun {
                    exit_code: 0,
                    stdout: "ok\n".to_string(),
                    stderr: String::new(),
                    timed_out: false,
                    error: None,
                }
            })
        }
    }

    impl FakeRunner {
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    /// Resolve a pending oneshot (the `respond_bridge_request` mechanism) by
    /// the compound key `"{session_id}/{request_id}"`. Works for BOTH
    /// `pending_bridge` (the `ask`) and `pending_sudo` (the sudo sub-prompts)
    /// — they are the same type.
    async fn respond(pending: &PendingBridge, session_id: &str, request_id: &str, result: Value) {
        let key = bridge::bridge_key(session_id, request_id);
        if let Some(sender) = pending.lock().await.remove(&key) {
            let _ = sender.send(result);
        }
    }

    /// A unique temp path (the socket).
    fn unique_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "archimedes-desktop-tools-{tag}-{}.tmp",
            uuid::Uuid::new_v4()
        ))
    }

    /// Start the real listener (anchor = the test process's own pid) and
    /// return the handle + the (unused) close sender. The test supplies the
    /// `pending_bridge` / `pending_sudo` maps (to resolve the oneshots) + the
    /// `todo_store` (to assert the store) + the `runner` (to assert the call
    /// count); the per-session `sudo_password` cache is internal.
    async fn start(
        session_id: &str,
        path: &Path,
        sink: Arc<CapturingSink>,
        pending: PendingBridge,
        pending_sudo: PendingSudo,
        todo_store: Arc<TodoStore>,
        runner: Arc<FakeRunner>,
    ) -> BridgeHandle {
        let (close_tx, _close_rx) = watch::channel(false);
        let sudo_password: Arc<Mutex<HashMap<String, CachedPassword>>> =
            Arc::new(Mutex::new(HashMap::new()));
        bridge::start_listener(
            session_id.to_string(),
            path,
            std::process::id(),
            sink,
            pending,
            &close_tx,
            Duration::from_secs(30),
            None,
            None,
            todo_store,
            pending_sudo,
            sudo_password,
            runner,
        )
        .await
        .expect("start_listener should bind")
    }

    /// Poll the sink until a `bridge-request` with `request_id` appears (the
    /// `wait_for_request` path in `bridge_integration.rs`).
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

    /// Poll the sink until a `bridge-event` with `event` appears (the
    /// `todos_update` push).
    async fn wait_for_bridge_event(
        sink: &CapturingSink,
        event: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(p) = sink
                .events_named("bridge-event")
                .iter()
                .find(|p| p.get("event").and_then(Value::as_str) == Some(event))
            {
                return Ok(p.clone());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "bridge-event {event} not emitted in time; got: {:?}",
                    sink.events_named("bridge-event")
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Poll the fake's captured stdout until a line containing `needle`
    /// appears (the fake prints each received bridge response as one JSON
    /// line; the `needle` is the response's `"id":"<id>"`).
    async fn wait_for_stdout(
        lines: &Arc<StdMutex<Vec<String>>>,
        needle: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let guard = lines.lock().unwrap();
                if let Some(l) = guard.iter().find(|l| l.contains(needle)) {
                    return Ok(l.clone());
                }
            }
            if Instant::now() > deadline {
                let got: Vec<String> = lines.lock().unwrap().clone();
                return Err(format!("timeout waiting for {needle:?}; got: {got:?}"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Spawn `fake_pi` in `FAKE_PI_TOOLS` mode (the TEST sets the bridge env
    /// — the fake connects to the socket, so the desktop's peer-verification
    /// passes: the fake is a descendant of the test = the anchor). A
    /// background thread reads the fake's stdout line-by-line into `out_lines`
    /// (the assertion channel).
    fn spawn_fake(socket: &Path, out_lines: Arc<StdMutex<Vec<String>>>) -> std::process::Child {
        use std::io::BufRead;
        let mut child = std::process::Command::new(FAKE_PI)
            .env("FAKE_PI_TOOLS", "1")
            .env("PI_ARCHIMEDES_BRIDGE", "1")
            .env("PI_ARCHIMEDES_BRIDGE_SOCKET", socket)
            .env("PI_ARCHIMEDES_BRIDGE_SESSION", "sess-tools")
            .env(
                "PI_ARCHIMEDES_BRIDGE_SERVER_PID",
                std::process::id().to_string(),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn fake_pi");
        let stdout = child.stdout.take().expect("stdout is piped");
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        out_lines.lock().unwrap().push(line.trim_end().to_string());
                    }
                    Err(_) => break,
                }
            }
        });
        child
    }

    /// The full e2e: drive the three tools over the bridge (the override's
    /// `execute()` round-trips) and assert the desktop-side execution.
    #[tokio::test]
    async fn the_override_round_trips_execute_in_the_desktop() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let pending_sudo: PendingSudo = Arc::new(Mutex::new(HashMap::new()));
        let todo_store = Arc::new(TodoStore::new());
        let runner = Arc::new(FakeRunner {
            calls: AtomicUsize::new(0),
        });
        let out_lines: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let path = unique_path("tools");
        let handle = start(
            "sess-tools",
            &path,
            sink.clone(),
            pending.clone(),
            pending_sudo.clone(),
            todo_store.clone(),
            runner.clone(),
        )
        .await;

        let mut child = spawn_fake(&path, out_lines.clone());
        let sid = "sess-tools";

        // ── manage_todo_list: write → the TodoStore updates + a
        // `todos_update`-shaped push is emitted (the EXISTING `TodoBoardPanel`
        // renders it) + the response carries the stored todos.
        let resp = wait_for_stdout(&out_lines, "\"id\":\"todo-1\"", Duration::from_secs(15))
            .await
            .expect("the todo write response (a descendant is accepted)");
        let v: Value = serde_json::from_str(&resp).expect("the response is JSON");
        assert_eq!(v.get("type").and_then(Value::as_str), Some("response"));
        let details = &v["result"]["details"];
        assert_eq!(
            details.get("operation").and_then(Value::as_str),
            Some("write")
        );
        assert_eq!(
            details
                .get("todos")
                .and_then(Value::as_array)
                .map(|a| a.len()),
            Some(3),
            "the write response carries the stored todos"
        );
        assert_eq!(
            details["todos"][0].get("content").and_then(Value::as_str),
            Some("Fix the auth middleware")
        );
        assert_eq!(
            details["todos"][0].get("status").and_then(Value::as_str),
            Some("in_progress")
        );
        // The desktop's TodoStore is updated (the same 3 items).
        assert_eq!(
            todo_store.get(sid).len(),
            3,
            "the TodoStore is updated by the write"
        );
        assert_eq!(todo_store.get(sid)[0].content, "Fix the auth middleware");
        // A `todos_update`-shaped push is emitted (the existing board renders it).
        let push = wait_for_bridge_event(&sink, "todos_update", Duration::from_secs(5))
            .await
            .expect("a todos_update push is emitted");
        assert_eq!(
            push.get("payload")
                .and_then(|p| p.get("todos"))
                .and_then(Value::as_array)
                .map(|a| a.len()),
            Some(3),
            "the todos_update push carries the stored todos"
        );

        // ── manage_todo_list: read → the stored todos come back.
        let resp = wait_for_stdout(&out_lines, "\"id\":\"todo-2\"", Duration::from_secs(15))
            .await
            .expect("the todo read response");
        let v: Value = serde_json::from_str(&resp).expect("the response is JSON");
        let details = &v["result"]["details"];
        assert_eq!(
            details.get("operation").and_then(Value::as_str),
            Some("read")
        );
        assert_eq!(
            details
                .get("todos")
                .and_then(Value::as_array)
                .map(|a| a.len()),
            Some(3),
            "the read returns the stored todos"
        );
        assert_eq!(
            details["todos"][1].get("content").and_then(Value::as_str),
            Some("Write the tests")
        );

        // ── sudo_exec (confirmed): the test resolves `:confirm` (true) +
        // `:password` (a password) via `respond` → the desktop runs the
        // command via the (fake) runner (`exitCode: 0`).
        let _ = wait_for_request(&sink, "sudo-1:confirm", Duration::from_secs(15))
            .await
            .expect("the :confirm bridge-request (derived requestId)");
        respond(
            &pending_sudo,
            sid,
            "sudo-1:confirm",
            json!({ "confirmed": true }),
        )
        .await;
        let _ = wait_for_request(&sink, "sudo-1:password", Duration::from_secs(15))
            .await
            .expect("the :password bridge-request (derived requestId)");
        respond(
            &pending_sudo,
            sid,
            "sudo-1:password",
            json!({ "password": "s3cret" }),
        )
        .await;
        let resp = wait_for_stdout(&out_lines, "\"id\":\"sudo-1\"", Duration::from_secs(15))
            .await
            .expect("the sudo run response");
        let v: Value = serde_json::from_str(&resp).expect("the response is JSON");
        let details = &v["result"]["details"];
        assert_eq!(
            details.get("exitCode").and_then(Value::as_i64),
            Some(0),
            "the desktop ran the command via the runner (exitCode: 0)"
        );
        assert_eq!(
            runner.calls(),
            1,
            "the runner was called once (the confirmed case)"
        );
        assert!(
            v["result"].get("isError").is_none(),
            "a successful run has no isError"
        );

        // ── sudo_exec (denied): the test resolves `:confirm` (false) →
        // "not confirmed" + NO runner call.
        let _ = wait_for_request(&sink, "sudo-2:confirm", Duration::from_secs(15))
            .await
            .expect("the denied :confirm bridge-request");
        respond(
            &pending_sudo,
            sid,
            "sudo-2:confirm",
            json!({ "confirmed": false }),
        )
        .await;
        let resp = wait_for_stdout(&out_lines, "\"id\":\"sudo-2\"", Duration::from_secs(15))
            .await
            .expect("the sudo denied response");
        let v: Value = serde_json::from_str(&resp).expect("the response is JSON");
        let details = &v["result"]["details"];
        assert_eq!(
            details.get("error").and_then(Value::as_str),
            Some("command not confirmed"),
            "the denied case is 'not confirmed'"
        );
        assert_eq!(
            runner.calls(),
            1,
            "the runner was NOT called for the denied case (still 1)"
        );
        assert_eq!(
            v["result"].get("isError").and_then(Value::as_bool),
            Some(true),
            "the denied case has isError"
        );

        // ── ask: the test resolves it via `respond` with an
        // `AskResponsePayload` → the desktop responds with the raw
        // `AskResponsePayload` (the override shapes it — the desktop does NOT
        // shape `ask`).
        let _ = wait_for_request(&sink, "ask-1", Duration::from_secs(15))
            .await
            .expect("the ask bridge-request");
        let ask_result = json!({
            "cancelled": false,
            "results": [ { "id": "q1", "selectedOptions": ["A"] } ]
        });
        respond(&pending, sid, "ask-1", ask_result.clone()).await;
        let resp = wait_for_stdout(&out_lines, "\"id\":\"ask-1\"", Duration::from_secs(15))
            .await
            .expect("the ask response");
        let v: Value = serde_json::from_str(&resp).expect("the response is JSON");
        assert_eq!(
            v.get("result"),
            Some(&ask_result),
            "the desktop responds with the raw AskResponsePayload (verbatim)"
        );
        assert!(v.get("error").is_none(), "the ask response has no error");

        // The fake exits 0 after the responses (a descendant is accepted and
        // all the frames are answered).
        let _ = child.wait();
        bridge::teardown(Some(handle));
        let _ = std::fs::remove_file(&path);
    }
}
