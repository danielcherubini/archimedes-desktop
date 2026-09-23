//! End-to-end integration test for the subagent dispatch path (Task 6): the
//! whole desktop path is driven with FAKE agents (no real inference): a fake
//! MAIN agent (the `dispatch` / `dispatch-cancel` `fake_agent` modes — open a
//! bridge connection and send a `dispatch_subagent` frame) + a fake SUBAGENT
//! agent (the `subagent` / `subagent-hang` modes — answer `session/prompt`).
//!
//! The test wiring mirrors production: build the `SubagentSessionManager`
//! (same config dir as the main), `main_manager.set_subagent_manager(
//! subagent_manager)`, then `start_session` the main. The main's bridge
//! listener services the `dispatch_subagent` frame (spawning the subagent on
//! the worker runtime); the subagent spawns the SAME registry entry as the
//! parent (the `PI_ACP_PI_COMMAND` env, set only for subagent spawns, selects
//! the subagent behavior per the mode rule).
//!
//! **Reaping assertion (reviewed):** the main and subagent fake agents share
//! the SAME binary AND the SAME `args` (the registry entry applies to both
//! spawns), so `pgrep -f` cannot tell them apart — we assert the process
//! COUNT of matching processes goes 2 → 1 (the subagent reaped), NOT
//! "absent" (the main stays live).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::StopReason;
use archimedes_desktop_lib::acp::{EventSink, SessionManager, SubagentSessionManager};
use serde_json::Value;

/// The session id the main fake agent reports (its default; the subagent
/// reports a DISTINCT id, `fake-subagent-1`, so the shared sink's
/// `session-update` / `session-closed` events are distinguishable).
const FAKE_SESSION_ID_MAIN: &str = "fake-session-1";
/// The session id the subagent fake agent reports (distinct from the main).
const FAKE_SESSION_ID_SUBAGENT: &str = "fake-subagent-1";

/// The full path to the compiled `fake_agent` binary.
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_fake_agent");

// ---------------------------------------------------------------------------
// Test event sink (collects into a shared `Vec`)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CollectSink {
    events: Arc<StdMutex<Vec<(String, Value)>>>,
}

impl EventSink for CollectSink {
    fn emit(&self, event: &str, payload: Value) {
        let n = self.events.lock().unwrap().len();
        eprintln!("[SINK] emit {event} (vec len before={n})");
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subagent-dispatch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copy the fake agent binary to a unique path. The main AND subagent use the
/// SAME copy (the single registry entry applies to both spawns), so a
/// `pgrep` on this path matches BOTH — the reaping assertion counts them.
fn unique_fake_agent(dir: &Path) -> PathBuf {
    let path = dir.join(format!("fake_agent-{}", uuid::Uuid::new_v4()));
    std::fs::copy(FAKE_AGENT, &path).unwrap();
    path
}

/// Write an agents.json with ONE `fake` entry: `command` = `bin`, `args` =
/// `[mode]` (the main's positional-arg mode), `bridge: true`, and `env`
/// carrying ONLY `FAKE_DISPATCH_TASK` + `FAKE_SUBAGENT_MODE` (the subagent
/// variant; the same entry is used for the subagent's spawn).
fn write_agents_json(
    dir: &Path,
    bin: &Path,
    mode: &str,
    subagent_mode: &str,
    subagent_delay_ms: Option<&str>,
) {
    let mut env = serde_json::Map::new();
    env.insert(
        "FAKE_DISPATCH_TASK".to_string(),
        serde_json::json!("do the task"),
    );
    env.insert(
        "FAKE_SUBAGENT_MODE".to_string(),
        serde_json::json!(subagent_mode),
    );
    // An optional `FAKE_SUBAGENT_DELAY_MS` (the `subagent` variant's
    // "thinking" delay — extends the subagent's process lifetime so the
    // E2E can observe it as a separate process; `None` = immediate response).
    if let Some(d) = subagent_delay_ms {
        env.insert("FAKE_SUBAGENT_DELAY_MS".to_string(), serde_json::json!(d));
    }
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Agent",
                "command": bin.to_string_lossy(),
                "args": [mode],
                "bridge": true,
                "env": env
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// Write an agents.json with ONE `fake` entry with a CUSTOM env map (the
/// concurrent / stale-carry-over tests — the standard helper's env set is
/// fixed). `command` = `bin`, `args` = `[mode]`, `bridge: true`.
fn write_agents_json_custom(dir: &Path, bin: &Path, mode: &str, env: &[(&str, &str)]) {
    let mut env_map = serde_json::Map::new();
    for (k, v) in env {
        env_map.insert(k.to_string(), serde_json::json!(v));
    }
    let json = serde_json::json!({
        "agents": [
            {
                "id": "fake",
                "name": "Fake Agent",
                "command": bin.to_string_lossy(),
                "args": [mode],
                "bridge": true,
                "env": env_map
            }
        ]
    });
    std::fs::write(
        dir.join("agents.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

/// The DISTINCT `sessionId`s of the `subagent-session-started` events.
fn distinct_started_ids(evs: &[(String, Value)]) -> std::collections::HashSet<&str> {
    evs.iter()
        .filter(|(name, _)| name.as_str() == "subagent-session-started")
        .filter_map(|(_, p)| p["sessionId"].as_str())
        .collect()
}

/// The count of `subagent-closed` events with `status: "completed"`.
fn completed_closed_count(evs: &[(String, Value)]) -> usize {
    evs.iter()
        .filter(|(name, p)| {
            name.as_str() == "subagent-closed" && p["status"].as_str() == Some("completed")
        })
        .count()
}

/// A fast COUNT of the running fake-agent processes matching `pattern`
/// (processes whose cmdline contains the unique binary path — the main +
/// subagent share the binary + args, so this is the only way to tell them
/// apart).
///
/// On Linux this is a DIRECT `/proc` scan: `pgrep -f` is a fork+exec over
/// every process (~45 ms here) — too SLOW to sample a short-lived process
/// (the subagent's whole lifecycle is ~10 ms; its process lives a few ms),
/// so a `pgrep`-based count can never observe the 2-process peak. The scan
/// is kept FAST (~0.2 ms): `read_dir(/proc)` + a `comm` / `cmdline` read
/// ONLY for pids not seen before (the subagent is a NEW process — the
/// baseline scan, taken before the prompt, covers everything else).
///
/// A pid's `cmdline` is read ONCE (first sight): a zombie's cmdline is
/// empty, so a pid first seen as a zombie can never match (its process was
/// already dead by then). A matched pid stays counted until it VANISHES
/// from `/proc` (reaped) — a zombie of a matched pid is still counted until
/// the parent reaps it (the count returns to 1 either way).
struct ProcessCounter {
    seen: std::collections::HashSet<u32>,
    matching: std::collections::HashSet<u32>,
    needle: Vec<u8>,
    comm_prefix: String,
}

impl ProcessCounter {
    /// Build the counter and take the BASELINE (everything running now —
    /// the main + any other test's processes — is `seen` before the
    /// subagent exists).
    fn new(pattern: &Path) -> Self {
        let needle = pattern.to_string_lossy().as_bytes().to_vec();
        // `comm` is the executable's file name, truncated to 15 bytes — a
        // cheap gate before the (slower) `cmdline` read.
        let comm_full = pattern
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let comm_prefix = comm_full[..comm_full.len().min(15)].to_string();
        let mut counter = Self {
            seen: std::collections::HashSet::new(),
            matching: std::collections::HashSet::new(),
            needle,
            comm_prefix,
        };
        counter.scan();
        counter
    }

    /// Scan `/proc` and return the count of matching processes still present
    /// (a vanished pid — reaped — drops out of the count).
    ///
    /// Kept FAST (the subagent's process lives only a few ms, so the scan
    /// must complete in well under a ms to sample it): allocation-free per
    /// pid (`as_bytes` + a manual digit parse, no `to_string_lossy` / no
    /// `parse`), a `comm` read ONLY for pids not seen before (the subagent
    /// is a NEW process — the baseline covers everything else), and a
    /// `cmdline` read only when the `comm` gate passes. A vanished matched
    /// pid is detected against the (cheap) full pid list, not a per-pid
    /// `present` set.
    fn scan(&mut self) -> usize {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut all_pids: Vec<u32> = Vec::with_capacity(700);
            if let Ok(entries) = std::fs::read_dir("/proc") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let b = name.as_bytes();
                    if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    let pid: u32 = b.iter().fold(0, |a, c| a * 10 + u32::from(c - b'0'));
                    all_pids.push(pid);
                    if self.seen.contains(&pid) {
                        continue;
                    }
                    self.seen.insert(pid);
                    // A NEW pid: read its `comm` (the executable's file
                    // name, truncated to 15 bytes — a cheap gate before the
                    // slower `cmdline` read). Allocation-free path (fixed
                    // buffer).
                    let mut cbuf = [0u8; 64];
                    let cpath = proc_path(pid, b"comm", &mut cbuf);
                    let Ok(comm) = std::fs::read(cpath) else {
                        continue;
                    };
                    if !comm.starts_with(self.comm_prefix.as_bytes()) {
                        continue;
                    }
                    // NUL-separated argv; a zombie's cmdline is empty (a
                    // pid first seen as a zombie can never match — the
                    // process was already dead). The full path is the
                    // authoritative check (`comm` is truncated — another
                    // test's copy could share the 15-byte prefix).
                    let mut kbuf = [0u8; 64];
                    let kpath = proc_path(pid, b"cmdline", &mut kbuf);
                    let Ok(cmdline) = std::fs::read(kpath) else {
                        continue;
                    };
                    if cmdline
                        .windows(self.needle.len())
                        .any(|w| w == self.needle.as_slice())
                    {
                        self.matching.insert(pid);
                    }
                }
            }
            // A matched pid that is no longer in `/proc` (reaped) drops out.
            self.matching
                .iter()
                .filter(|p| all_pids.contains(p))
                .count()
        }
        #[cfg(not(unix))]
        {
            // The bridge is fail-closed on non-Unix (macOS / Windows), so
            // these tests cannot run; report 0 (no fake-agent processes).
            0
        }
    }
}

/// Build a `/proc/<pid>/<file>` path into `buf` (allocation-free; the fixed
/// buffer is reused by the caller). `pid` is written in decimal.
fn proc_path<'a>(pid: u32, file: &[u8], buf: &'a mut [u8; 64]) -> &'a str {
    let mut i = 0;
    for c in b"/proc/" {
        buf[i] = *c;
        i += 1;
    }
    let mut tmp = [0u8; 11];
    let mut n = 0;
    let mut v = pid;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
        }
        tmp[..n].reverse();
    }
    for c in &tmp[..n] {
        buf[i] = *c;
        i += 1;
    }
    buf[i] = b'/';
    i += 1;
    for c in file {
        buf[i] = *c;
        i += 1;
    }
    std::str::from_utf8(&buf[..i]).unwrap()
}

/// Non-Linux fallback (the bridge is fail-closed there, so these tests
/// cannot run): `pgrep -f`.
#[cfg(not(target_os = "linux"))]
fn count_fake_agent_processes(pattern: &Path) -> usize {
    let out = match std::process::Command::new("pgrep")
        .args(["-f", pattern.to_string_lossy().as_ref()])
        .output()
    {
        Ok(o) => o,
        Err(_) => return 0,
    };
    if !out.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .count()
}

/// Poll the collected events until `pred` holds or `timeout` elapses (no lock
/// held across the await).
async fn wait_for_event(
    events: &StdMutex<Vec<(String, Value)>>,
    timeout: Duration,
    pred: impl Fn(&[(String, Value)]) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        {
            let guard = events.lock().unwrap();
            if pred(&guard) {
                return true;
            }
        }
        if Instant::now() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The text of a `session-update` payload (the `agent_message_chunk` text).
fn update_text(payload: &Value) -> Option<String> {
    payload
        .pointer("/update/content/text")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// All `session-update` texts for a `sessionId` (the session's stream).
fn stream_texts(events: &[(String, Value)], session_id: &str) -> Vec<String> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "session-update")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .filter_map(|(_, p)| update_text(p))
        .collect()
}

/// The first `subagent-closed` payload for a `sessionId`, if any.
fn subagent_closed<'a>(events: &'a [(String, Value)], session_id: &str) -> Option<&'a Value> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "subagent-closed")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .map(|(_, p)| p)
        .next()
}

/// The first `subagent-session-started` payload for a `sessionId`, if any.
fn subagent_started<'a>(events: &'a [(String, Value)], session_id: &str) -> Option<&'a Value> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "subagent-session-started")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .map(|(_, p)| p)
        .next()
}

/// The first `session-closed` payload for a `sessionId`, if any.
fn session_closed<'a>(events: &'a [(String, Value)], session_id: &str) -> Option<&'a Value> {
    events
        .iter()
        .filter(|(name, _)| name.as_str() == "session-closed")
        .filter(|(_, p)| p["sessionId"].as_str() == Some(session_id))
        .map(|(_, p)| p)
        .next()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// (1) **success**: the main (a `dispatch` fake agent, `bridge: true`) fires
/// the `dispatch_subagent` bridge frame → the desktop dispatches the subagent
/// on the worker runtime → the subagent answers `subagent-done`. Assert the
/// main prompt resolves `end_turn` with `dispatch:subagent-done` in its
/// stream, the `subagent-session-started` / `subagent-closed` events fire with
/// the right payload, the subagent's `session-closed` fires, and the
/// fake-agent process COUNT goes 2 → 1 (the subagent reaped, the main live).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_success_full_round_trip() {
    let mut last_err = None;
    for attempt in 0..3 {
        match run_dispatch_success_scenario().await {
            Ok(()) => return,
            Err(e) => {
                eprintln!("Attempt {attempt} failed: {e}");
                last_err = Some(e);
            }
        }
    }
    panic!("3 attempts failed: {last_err:?}");
}

async fn run_dispatch_success_scenario() -> Result<(), String> {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    write_agents_json(&config_dir, &bin, "dispatch", "subagent", Some("200"));

    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(&events),
    });
    let cwd = config_dir.clone();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone())
            .map_err(|e| format!("subagent manager build failed: {e}"))?,
    );
    let mut manager = SessionManager::new(config_dir.clone())
        .map_err(|e| format!("main manager build failed: {e}"))?;
    manager.set_subagent_manager(subagent_manager);
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .map_err(|e| format!("main start_session failed: {e}"))?;
    if info.session_id.to_string() != FAKE_SESSION_ID_MAIN {
        return Err(format!("unexpected session id: {}", info.session_id));
    }

    let (prompt_tx, mut prompt_rx) = tokio::sync::oneshot::channel();
    {
        let manager = Arc::clone(&manager);
        let sid = info.session_id.to_string();
        tokio::spawn(async move {
            let r = manager.send_prompt(&sid, "go".to_string()).await;
            let _ = prompt_tx.send(r);
        });
    }

    let mut counter = ProcessCounter::new(&bin);
    let peak = Arc::new(AtomicUsize::new(counter.scan()));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let peak = Arc::clone(&peak);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut c = counter;
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let n = c.scan();
                if n > peak.load(Ordering::Relaxed) {
                    peak.store(n, Ordering::Relaxed);
                }
            }
        })
    };
    let prompt_deadline = Instant::now() + Duration::from_secs(30);
    let reason = loop {
        match prompt_rx.try_recv() {
            Ok(r) => break r.map_err(|e| format!("main send_prompt failed: {e}"))?,
            Err(_) => {
                if prompt_rx.is_terminated() {
                    stop.store(true, Ordering::Relaxed);
                    sampler.join().ok();
                    return Err("the prompt task vanished without resolving".to_string());
                }
            }
        }
        if Instant::now() > prompt_deadline {
            stop.store(true, Ordering::Relaxed);
            sampler.join().ok();
            return Err("timeout waiting for the main prompt (the dispatch E2E)".to_string());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    stop.store(true, Ordering::Relaxed);
    sampler
        .join()
        .map_err(|_| "the sampler thread should finish")?;
    let peak = peak.load(Ordering::Relaxed);
    if reason != StopReason::EndTurn {
        return Err(format!("expected EndTurn, got {:?}", reason));
    }

    if peak != 2 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let dump = events.lock().unwrap();
        eprintln!("[DEBUG] peak={peak}; events:");
        for (name, p) in dump.iter() {
            eprintln!("  {name}: {}", p);
        }
        drop(dump);
        return Err(format!("expected peak 2, got {}", peak));
    }

    if !wait_for_event(&events, Duration::from_secs(5), |evs| {
        stream_texts(evs, FAKE_SESSION_ID_MAIN)
            .iter()
            .any(|t| t.contains("dispatch:subagent-done"))
    })
    .await
    {
        return Err("the main's stream should contain `dispatch:subagent-done`".to_string());
    }

    if !wait_for_event(&events, Duration::from_secs(5), |evs| {
        subagent_started(evs, FAKE_SESSION_ID_SUBAGENT).is_some()
    })
    .await
    {
        return Err("a subagent-session-started for the subagent id should fire".to_string());
    }
    {
        let evs = events.lock().unwrap();
        let started = subagent_started(&evs, FAKE_SESSION_ID_SUBAGENT).unwrap();
        if started["parentSessionId"].as_str() != Some(FAKE_SESSION_ID_MAIN) {
            return Err("unexpected parent session id".to_string());
        }
        if started["agentName"].as_str() != Some("fake") {
            return Err("unexpected agent name".to_string());
        }
        if started["task"].as_str() != Some("do the task") {
            return Err("unexpected task".to_string());
        }
    }

    if !wait_for_event(&events, Duration::from_secs(5), |evs| {
        subagent_closed(evs, FAKE_SESSION_ID_SUBAGENT).and_then(|p| p["status"].as_str())
            == Some("completed")
    })
    .await
    {
        return Err("a subagent-closed (completed) for the subagent id should fire".to_string());
    }
    {
        let evs = events.lock().unwrap();
        let closed = subagent_closed(&evs, FAKE_SESSION_ID_SUBAGENT).unwrap();
        if !closed["metrics"]["durationMs"].is_number() {
            return Err("durationMs not a number".to_string());
        }
    }

    if !wait_for_event(&events, Duration::from_secs(5), |evs| {
        session_closed(evs, FAKE_SESSION_ID_SUBAGENT).is_some()
    })
    .await
    {
        return Err("a session-closed for the subagent id should fire".to_string());
    }

    let mut reap_counter = ProcessCounter::new(&bin);
    let reap_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let c = reap_counter.scan();
        if c == 1 {
            break;
        }
        if Instant::now() > reap_deadline {
            return Err(format!(
                "the subagent process should be reaped (count back to 1); got {c}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let _ = manager.close_session(FAKE_SESSION_ID_MAIN).await;
    let _ = std::fs::remove_dir_all(&config_dir);
    Ok(())
}

/// (2) **cancellation**: the main (a `dispatch-cancel` fake agent — sends the
/// frame, closes the bridge connection WITHOUT reading) + the subagent (a
/// `subagent-hang` fake agent — the prompt never settles). The parent
/// connection close cancels the in-flight dispatch. Assert the subagent
/// session is torn down (`subagent-closed` with `status: "failed"` + `error
/// "cancelled"`), the process COUNT goes 2 → 1, and the main stays live
/// (its prompt still resolves `end_turn`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_cancellation_tears_down_subagent() {
    let mut last_err = None;
    for attempt in 0..3 {
        match run_dispatch_cancellation_scenario().await {
            Ok(()) => return,
            Err(e) => {
                eprintln!("Attempt {attempt} failed: {e}");
                last_err = Some(e);
            }
        }
    }
    panic!("3 attempts failed: {last_err:?}");
}

async fn run_dispatch_cancellation_scenario() -> Result<(), String> {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    write_agents_json(&config_dir, &bin, "dispatch-cancel", "hang", None);

    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(&events),
    });
    let cwd = config_dir.clone();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone())
            .map_err(|e| format!("subagent manager build failed: {e}"))?,
    );
    let mut manager = SessionManager::new(config_dir.clone())
        .map_err(|e| format!("main manager build failed: {e}"))?;
    manager.set_subagent_manager(subagent_manager);
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .map_err(|e| format!("main start_session failed: {e}"))?;
    if info.session_id.to_string() != FAKE_SESSION_ID_MAIN {
        return Err(format!("unexpected session id: {}", info.session_id));
    }

    let (prompt_tx, mut prompt_rx) = tokio::sync::oneshot::channel();
    {
        let manager = Arc::clone(&manager);
        let sid = info.session_id.to_string();
        tokio::spawn(async move {
            let r = manager.send_prompt(&sid, "go".to_string()).await;
            let _ = prompt_tx.send(r);
        });
    }

    let mut counter = ProcessCounter::new(&bin);
    let peak = Arc::new(AtomicUsize::new(counter.scan()));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let peak = Arc::clone(&peak);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut c = counter;
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let n = c.scan();
                if n > peak.load(Ordering::Relaxed) {
                    peak.store(n, Ordering::Relaxed);
                }
            }
        })
    };
    let prompt_deadline = Instant::now() + Duration::from_secs(30);
    let reason = loop {
        match prompt_rx.try_recv() {
            Ok(r) => break r.map_err(|e| format!("main send_prompt failed: {e}"))?,
            Err(_) => {
                if prompt_rx.is_terminated() {
                    stop.store(true, Ordering::Relaxed);
                    sampler.join().ok();
                    return Err("the prompt task vanished without resolving".to_string());
                }
            }
        }
        if Instant::now() > prompt_deadline {
            stop.store(true, Ordering::Relaxed);
            sampler.join().ok();
            return Err("timeout waiting for the main prompt (the cancellation E2E)".to_string());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    stop.store(true, Ordering::Relaxed);
    sampler
        .join()
        .map_err(|_| "the sampler thread should finish")?;
    let peak = peak.load(Ordering::Relaxed);

    if reason != StopReason::EndTurn {
        return Err(format!("expected EndTurn, got {:?}", reason));
    }

    if peak != 2 {
        return Err(format!("expected peak 2, got {}", peak));
    }

    if !wait_for_event(&events, Duration::from_secs(5), |evs| {
        subagent_closed(evs, FAKE_SESSION_ID_SUBAGENT)
            .filter(|p| p["status"].as_str() == Some("failed"))
            .is_some()
    })
    .await
    {
        return Err("subagent-closed (failed) should fire".to_string());
    }
    {
        let evs = events.lock().unwrap();
        let closed = subagent_closed(&evs, FAKE_SESSION_ID_SUBAGENT).unwrap();
        if closed["error"].as_str() != Some("cancelled") {
            return Err("expected error `cancelled`".to_string());
        }
    }

    let mut reap_counter = ProcessCounter::new(&bin);
    let reap_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let c = reap_counter.scan();
        if c == 1 {
            break;
        }
        if Instant::now() > reap_deadline {
            return Err(format!("subagent not reaped; got {c}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    if manager.session_count().await != 1 {
        return Err("expected 1 session count".to_string());
    }

    let _ = manager.close_session(FAKE_SESSION_ID_MAIN).await;
    let _ = std::fs::remove_dir_all(&config_dir);
    Ok(())
}

/// (3) **the subagent's own bridge round-trip** (the `ask` path, no relay):
/// the `subagent` fake agent mode, after `session/new`, connects to its OWN
/// `PI_ARCHIMEDES_BRIDGE_SOCKET` (the subagent's own listener, NOT the
/// main's) and sends a `request` frame (`method: "ask"`); the test answers it
/// via `SubagentSessionManager::respond_bridge_request` (the subagent manager's
/// respond path — a plain tokio test can't reach the Tauri command's managed
/// state); the fake agent reads the response and echoes it as a chunk before
/// `end_turn`. Assert the subagent's stream (keyed `fake-subagent-1`) contains
/// the echoed answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_subagent_own_bridge_ask_round_trip() {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    // The main is in `dispatch` mode (it spawns the subagent); the subagent is
    // in the `ask` variant (it connects to its OWN bridge and sends an `ask`).
    write_agents_json(&config_dir, &bin, "dispatch", "ask", None);

    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(&events),
    });
    let cwd = config_dir.clone();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone()).expect("subagent manager should build"),
    );
    let mut manager = SessionManager::new(config_dir.clone()).expect("main manager should build");
    manager.set_subagent_manager(subagent_manager.clone());
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("main start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID_MAIN);

    // The main prompt spawns the subagent (the `dispatch` mode). The subagent
    // (the `ask` variant) connects to its OWN bridge and sends an `ask`
    // (`id: "fake-subagent-ask-1"`); the test answers it via the subagent
    // manager's `respond_bridge_request` (keyed by the subagent's ACP id + the
    // request id the fake agent chose).
    let (prompt_tx, mut prompt_rx) = tokio::sync::oneshot::channel();
    {
        let manager = Arc::clone(&manager);
        let sid = info.session_id.to_string();
        tokio::spawn(async move {
            let r = manager.send_prompt(&sid, "go".to_string()).await;
            let _ = prompt_tx.send(r);
        });
    }
    let prompt_deadline = Instant::now() + Duration::from_secs(30);
    let mut answered = false;
    let reason = loop {
        // The oneshot is set only AFTER the subagent's `ask` is answered (the
        // subagent blocks on it), so this can't resolve before the ask is seen.
        match prompt_rx.try_recv() {
            Ok(r) => break r.expect("main send_prompt should succeed"),
            Err(_) => {
                if prompt_rx.is_terminated() {
                    panic!("the prompt task vanished without resolving");
                }
            }
        }
        // Wait for the subagent's `bridge-request` (`method: "ask"`, on the
        // subagent's own listener), then answer it ONCE (the `ask` path, no
        // relay). The `answered` guard prevents a double `respond_bridge_request`
        // (a second call would miss the (already-removed) entry and return `false`).
        // The owned `(request_id, session_id)` is extracted INSIDE the guard's
        // scope (the `MutexGuard` is dropped at the block's end — no borrow
        // outlives it).
        if !answered {
            let ask_info = {
                let evs = events.lock().unwrap();
                evs.iter()
                    .find(|(name, p)| {
                        name.as_str() == "bridge-request"
                            && p["method"].as_str() == Some("ask")
                            && p["sessionId"].as_str() == Some(FAKE_SESSION_ID_SUBAGENT)
                    })
                    .map(|(_, p)| {
                        (
                            p["requestId"].as_str().unwrap_or_default().to_string(),
                            p["sessionId"].as_str().unwrap_or_default().to_string(),
                        )
                    })
            };
            if let Some((request_id, session_id)) = ask_info {
                // Answer the `ask` (the subagent manager's respond path).
                let ok = subagent_manager
                    .respond_bridge_request(
                        &session_id,
                        &request_id,
                        Value::String("the-answer".to_string()),
                    )
                    .await;
                assert!(
                    ok,
                    "respond_bridge_request should find the subagent's pending ask"
                );
                answered = true;
            }
        }
        if Instant::now() > prompt_deadline {
            panic!("timeout waiting for the main prompt (the subagent ask E2E)");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(reason, StopReason::EndTurn);

    // The subagent's stream (keyed `fake-subagent-1`) contains the echoed
    // answer (the fake agent read the response line and echoed it as a chunk).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| stream_texts(
            evs,
            FAKE_SESSION_ID_SUBAGENT
        )
        .iter()
        .any(|t| t.contains("the-answer")))
        .await,
        "the subagent's stream should contain the echoed answer"
    );

    // The subagent completed (its `ask` round-trip settled the prompt).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| subagent_closed(
            evs,
            FAKE_SESSION_ID_SUBAGENT
        )
        .and_then(|p| p["status"].as_str())
            == Some("completed"))
        .await,
        "a subagent-closed (completed) should fire after the ask round-trip"
    );

    // Cleanup.
    let _ = manager.close_session(FAKE_SESSION_ID_MAIN).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (4) **concurrent subagents, per-dispatch captures** (the shared-capture
/// regression, and the N-sessions-on-one-worker-runtime shape): one main
/// session issues TWO `dispatch_subagent` frames (the `dispatch-two` mode —
/// both frames written BEFORE either response is read, so the two subagents
/// run CONCURRENTLY on the ONE `SubagentSessionManager` worker runtime). Each
/// subagent (`echo` variant) answers with its OWN task as its final text
/// (DISTINCT per dispatch). Assert each `Completed.output` is ITS OWN text
/// (both outputs correct AND distinct — a shared `last_message_id` /
/// `text_capture` would clobber / accumulate across the two sessions; the
/// prompt is wrapped in a HARD 10 s timeout, the 2026-09-15 clean-red
/// pattern).
// The `events` guard is dropped before each await, but clippy's liveness
// analysis is scope-based, not `drop`-aware (the same pattern the
// `text_capture` unit test in `subagent.rs` documents).
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_concurrent_subagents_get_their_own_output() {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    // The main is in `dispatch-two` mode (two frames, CONCURRENT — the
    // default); the subagent is in the `echo` variant (each echoes its OWN
    // task); the subagent session id is DISTINCT per process (two subagents
    // from the SAME registry entry); a 500 ms "thinking" delay widens the
    // concurrent window (both subagents write their captures at ~the same
    // time).
    write_agents_json_custom(
        &config_dir,
        &bin,
        "dispatch-two",
        &[
            ("FAKE_DISPATCH_TASK", "task-one"),
            ("FAKE_DISPATCH_TASK_2", "task-two"),
            ("FAKE_SUBAGENT_MODE", "echo"),
            ("FAKE_SUBAGENT_DELAY_MS", "500"),
            ("FAKE_SUBAGENT_SESSION_ID_PER_PID", "1"),
        ],
    );

    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(&events),
    });
    let cwd = config_dir.clone();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone()).expect("subagent manager should build"),
    );
    let mut manager = SessionManager::new(config_dir.clone()).expect("main manager should build");
    manager.set_subagent_manager(subagent_manager);
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("main start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID_MAIN);

    // The main prompt fires the TWO `dispatch_subagent` frames (the
    // `dispatch-two` mode, concurrent). Wrap it in a HARD 10 s timeout (the
    // 2026-09-15 clean-red pattern — N ACP sessions on the ONE worker
    // runtime): a fired timeout IS the concurrency regression.
    let started = Instant::now();
    let sid = info.session_id.to_string();
    let reason = match tokio::time::timeout(
        Duration::from_secs(10),
        manager.send_prompt(&sid, "go".to_string()),
    )
    .await
    {
        Ok(r) => r.expect("main send_prompt should succeed"),
        Err(_) => panic!(
            "CONCURRENCY REGRESSION (ADR 0004): the main prompt (TWO concurrent \
             subagent dispatches on the ONE worker runtime) stalled — the 10 s hard \
             timeout fired at {:?} (the 2026-09-15 hang shape: N ACP sessions on \
             one worker runtime)",
            started.elapsed()
        ),
    };
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt should resolve end_turn"
    );

    // Each `Completed.output` is ITS OWN text: the main echoed
    // `dispatch1:task-one` and `dispatch2:task-two` (NOT a clobbered /
    // accumulated shared-capture value — a shared `last_message_id` /
    // `text_capture` would make the second output carry the first's text,
    // and a `messageId` collision (`m1` per process) would garble the
    // accumulated entry).
    let evs = events.lock().unwrap();
    let texts = stream_texts(&evs, FAKE_SESSION_ID_MAIN);
    drop(evs);
    assert!(
        texts.iter().any(|t| t == "dispatch1:task-one"),
        "the first concurrent subagent's output must be its OWN task (got {texts:?})"
    );
    assert!(
        texts.iter().any(|t| t == "dispatch2:task-two"),
        "the second concurrent subagent's output must be its OWN task (got {texts:?})"
    );

    // Two DISTINCT subagent sessions were established (distinct ACP ids —
    // `FAKE_SUBAGENT_SESSION_ID_PER_PID`), and both completed.
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| {
            distinct_started_ids(evs).len() >= 2
        })
        .await,
        "two DISTINCT subagent sessions (distinct ACP ids) should be established"
    );
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| {
            completed_closed_count(evs) >= 2
        })
        .await,
        "both concurrent subagents should complete (two subagent-closed completed)"
    );

    // Cleanup: close the main (reaps its process).
    let _ = manager.close_session(FAKE_SESSION_ID_MAIN).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (5) **no-text after text** (the stale-carry-over regression): one main
/// session issues TWO `dispatch_subagent` frames SEQUENTIALLY (the
/// `dispatch-two` mode + `FAKE_DISPATCH_TWO_SEQUENTIAL=1` — response 1 is
/// read BEFORE frame 2 is sent). The first subagent answers with text (the
/// `echo` variant echoes its task, `"hello-text"`); the second answers with
/// NO text (the `EMPTY` sentinel — `end_turn` only). Assert the first
/// `Completed.output` is its text AND the second is the EMPTY STRING (NOT the
/// previous subagent's final text — a shared `last_message_id` /
/// `text_capture` would carry it over; the plan specifies `""`).
// The `events` guard is dropped before each await, but clippy's liveness
// analysis is scope-based, not `drop`-aware (the same pattern the
// `text_capture` unit test in `subagent.rs` documents).
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_no_text_after_text_dispatch_returns_empty_output() {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    // The main is in `dispatch-two` mode SEQUENTIALLY (response 1 is read
    // before frame 2 is sent — the no-text dispatch runs AFTER the text
    // dispatch completes); the subagent is in the `echo` variant (`hello-
    // text` → echoed; the `EMPTY` sentinel → NO-TEXT turn).
    write_agents_json_custom(
        &config_dir,
        &bin,
        "dispatch-two",
        &[
            ("FAKE_DISPATCH_TASK", "hello-text"),
            ("FAKE_DISPATCH_TASK_2", "EMPTY"),
            ("FAKE_SUBAGENT_MODE", "echo"),
            ("FAKE_SUBAGENT_SESSION_ID_PER_PID", "1"),
            ("FAKE_DISPATCH_TWO_SEQUENTIAL", "1"),
        ],
    );

    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(&events),
    });
    let cwd = config_dir.clone();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone()).expect("subagent manager should build"),
    );
    let mut manager = SessionManager::new(config_dir.clone()).expect("main manager should build");
    manager.set_subagent_manager(subagent_manager);
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("main start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID_MAIN);

    // The main prompt fires the two dispatches SEQUENTIALLY (the
    // `FAKE_DISPATCH_TWO_SEQUENTIAL` rule). Hard 10 s timeout (the
    // clean-red pattern).
    let started = Instant::now();
    let sid = info.session_id.to_string();
    let reason = match tokio::time::timeout(
        Duration::from_secs(10),
        manager.send_prompt(&sid, "go".to_string()),
    )
    .await
    {
        Ok(r) => r.expect("main send_prompt should succeed"),
        Err(_) => panic!(
            "CONCURRENCY REGRESSION (ADR 0004): the main prompt (two sequential \
             subagent dispatches on the ONE worker runtime) stalled — the 10 s hard \
             timeout fired at {:?}",
            started.elapsed()
        ),
    };
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt should resolve end_turn"
    );

    // The first `Completed.output` is its text (`dispatch1:hello-text`).
    let evs = events.lock().unwrap();
    let texts = stream_texts(&evs, FAKE_SESSION_ID_MAIN);
    drop(evs);
    assert!(
        texts.iter().any(|t| t == "dispatch1:hello-text"),
        "the text subagent's output must be its OWN text (got {texts:?})"
    );
    // The second (no-text) `Completed.output` is the EMPTY STRING — the main
    // echoed `dispatch2:` (an empty output). A shared `last_message_id` /
    // `text_capture` would carry over the FIRST subagent's final text
    // (`dispatch2:hello-text` — the stale-carry-over bug).
    assert!(
        texts.iter().any(|t| t == "dispatch2:"),
        "the no-text subagent's output must be the EMPTY STRING, not the previous \
         subagent's final text (got {texts:?})"
    );
    assert!(
        !texts.iter().any(|t| t == "dispatch2:hello-text"),
        "the no-text subagent's output must NOT carry over the previous \
         subagent's final text (got {texts:?})"
    );

    // Both subagents completed.
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| {
            completed_closed_count(evs) >= 2
        })
        .await,
        "both subagents should complete (two subagent-closed completed)"
    );

    // Cleanup.
    let _ = manager.close_session(FAKE_SESSION_ID_MAIN).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// (6) **cost accumulation** (the `subagent-metrics-cost-push` E2E): the
/// subagent (the default `subagent` variant + `FAKE_COST_PUSH=1`) pushes TWO
/// `cost_update` frames through its OWN `PI_ARCHIMEDES_BRIDGE_SOCKET` before
/// answering `session/prompt` (ONE connection per push — the desktop reads
/// exactly one frame per connection — and it reads each bare `ack` line
/// BEFORE the next write and before answering the prompt, so the desktop's
/// `end_turn`-time capture is COMPLETE). Assert the `subagent-closed`
/// metrics are the SUM of both payloads (inputTokens 100+200=300,
/// outputTokens 50+25=75, cost 0.001+0.002=0.003 — NOT the last payload's
/// `{ 200, 25, 0.002 }` and NOT the zeros of a session that pushed nothing)
/// + a real `durationMs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_subagent_cost_push_is_accumulated_into_metrics() {
    let config_dir = temp_config_dir();
    let bin = unique_fake_agent(&config_dir);
    // The main is in `dispatch` mode (it spawns the subagent); the subagent
    // is the default `subagent` variant + `FAKE_COST_PUSH=1` (it pushes two
    // `cost_update` frames through its OWN bridge before answering the
    // prompt — the desktop's accumulator must SUM them into the metrics).
    write_agents_json_custom(
        &config_dir,
        &bin,
        "dispatch",
        &[
            ("FAKE_DISPATCH_TASK", "do the task"),
            ("FAKE_SUBAGENT_MODE", "subagent"),
            ("FAKE_COST_PUSH", "1"),
        ],
    );

    let events: Arc<StdMutex<Vec<(String, Value)>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink: Arc<dyn EventSink> = Arc::new(CollectSink {
        events: Arc::clone(&events),
    });
    let cwd = config_dir.clone();

    let subagent_manager = Arc::new(
        SubagentSessionManager::new(config_dir.clone()).expect("subagent manager should build"),
    );
    let mut manager = SessionManager::new(config_dir.clone()).expect("main manager should build");
    manager.set_subagent_manager(subagent_manager);
    let manager = Arc::new(manager);

    let info = archimedes_desktop_lib::test_support::run_with_retry(|| async {
        manager.start_session("fake", cwd.clone(), &sink).await
    })
    .await
    .expect("main start_session should succeed");
    assert_eq!(info.session_id.to_string(), FAKE_SESSION_ID_MAIN);

    // The main prompt fires the `dispatch_subagent` frame (the `dispatch`
    // mode) and blocks until the response (the subagent answers
    // `subagent-done` AFTER pushing its two `cost_update` frames). Hard 10 s
    // timeout (the clean-red pattern).
    let started = Instant::now();
    let sid = info.session_id.to_string();
    let reason = match tokio::time::timeout(
        Duration::from_secs(10),
        manager.send_prompt(&sid, "go".to_string()),
    )
    .await
    {
        Ok(r) => r.expect("main send_prompt should succeed"),
        Err(_) => panic!(
            "the main prompt (the cost-push E2E) stalled — the 10 s hard timeout fired at {:?}",
            started.elapsed()
        ),
    };
    assert_eq!(
        reason,
        StopReason::EndTurn,
        "the main prompt should resolve end_turn"
    );

    // The main's stream contains the echoed dispatch result
    // (`dispatch:subagent-done` — the subagent answered its prompt AFTER both
    // cost pushes were acked).
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| stream_texts(
            evs,
            FAKE_SESSION_ID_MAIN
        )
        .iter()
        .any(|t| t.contains("dispatch:subagent-done")))
        .await,
        "the main's stream should contain `dispatch:subagent-done`"
    );

    // `subagent-closed` (completed) with the metrics snapshot.
    assert!(
        wait_for_event(&events, Duration::from_secs(5), |evs| subagent_closed(
            evs,
            FAKE_SESSION_ID_SUBAGENT
        )
        .and_then(|p| p["status"].as_str())
            == Some("completed"))
        .await,
        "a subagent-closed (completed) for the subagent id should fire"
    );
    // The metrics are the SUM of BOTH pushed payloads (payload 1: input 100 /
    // output 50 / cost 0.001, `cacheReadTokens` ABSENT → 0; payload 2: input
    // 200 / output 25 / cacheRead 10 / cost 0.002): inputTokens 300, outputTokens
    // 75, cost 0.003. The old last-payload semantics would have produced
    // `{ 200, 25, 0.002 }` — the exact assertions below prove the accumulator.
    {
        let evs = events.lock().unwrap();
        let closed = subagent_closed(&evs, FAKE_SESSION_ID_SUBAGENT).unwrap();
        let metrics = &closed["metrics"];
        assert_eq!(
            metrics["inputTokens"].as_u64(),
            Some(300),
            "inputTokens should be the SUM of both payloads (100 + 200 = 300)"
        );
        assert_eq!(
            metrics["outputTokens"].as_u64(),
            Some(75),
            "outputTokens should be the SUM of both payloads (50 + 25 = 75)"
        );
        let cost = metrics["cost"].as_f64().unwrap_or(f64::NAN);
        assert!(
            (cost - 0.003).abs() < 1e-9,
            "cost should be the SUM of both payloads (0.001 + 0.002 = 0.003), got {cost}"
        );
        let duration_ms = metrics["durationMs"].as_u64().unwrap_or(0);
        assert!(
            duration_ms > 0,
            "durationMs should be the real wall clock (> 0), got {duration_ms}"
        );
    }

    // Cleanup.
    let _ = manager.close_session(FAKE_SESSION_ID_MAIN).await;
    let _ = std::fs::remove_dir_all(&config_dir);
}
