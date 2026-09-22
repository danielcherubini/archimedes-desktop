//! A REAL-pi-acp two-session concurrency diagnostic — the 2026-09-15 repro
//! rebuilt (ADR 0002) + the control (the ADR 0004 shipped topology).
//!
//! **What this is:** a diagnostic, NOT a CI gate. The 2026-09-15 incident was
//! "reproduced (real-app topology, live)" — two concurrent ACP sessions on
//! ONE tokio runtime hung `send_prompt` 60+ seconds, suspected at the
//! SDK/async-io level (two long-lived per-connection transport tasks sharing
//! one global async-io reactor); the diagnostic was deleted and the root
//! cause never confirmed. The shipped regression test
//! (`subagent_concurrency.rs`) passes — but with the FAKE agent. This
//! harness runs two concurrent REAL pi ACP sessions (the full `pi-acp`
//! stack: the SDK's transport tasks, model calls, the full event flow —
//! desktop ↔ pi-acp ↔ pi, the real-app topology) on ONE runtime (the repro)
//! and on TWO runtimes (the control, expected to pass).
//!
//! **Mechanism note (load-bearing):** the sessions are built with the
//! `agent-client-protocol` crate's OWN connection machinery — `AcpAgent`
//! spawns the child process ITSELF via `async_process::Child` (an
//! async-io-based transport — the exact ADR 0002 suspicion surface). A
//! hand-spawned `tokio::process::Command` + attached pipes would run the
//! transport on tokio's reactor instead — a hang-free result there would be
//! a FALSE "resolved → lift the cap" at the gate of the whole
//! investigation. The desktop's production wiring is mirrored:
//! `Client::builder()` + the minimal auto-responding handlers
//! (session.rs:375-476) + `connect_with` (session.rs:478-558) + the
//! bare-agent `AcpAgent` spawn (session.rs:830).
//!
//! **The establisher BLOCKS-UNTIL-CLOSE (load-bearing):** the desktop treats
//! `connect_with`'s return as connection DEATH and the crate tears down the
//! transport on that return (`ChildGuard::drop` SIGKILLs the process group).
//! The establisher (a) hands a CLONE of the connection + the session id out
//! via channels (the caller returns once they arrive), (b) establishes
//! (`initialize` + `session/new`), and (c) BLOCKS on a close signal until the
//! test drops it. A RETURNING establisher would close the connection and
//! CORRUPT THE GATE: the prompt would resolve instantly with connection
//! errors and the "any outcome is fine" liveness assertion would accept that
//! as "resolved" — a silent false "resolved → lift the cap".
//!
//! **Liveness, not semantics:** the assertion is that both `send_prompt`
//! FUTURES RESOLVE (any outcome — `end_turn`, a refusal, or an error) within
//! 10 s; a non-resolution is the hang. The hermetic run resolves with a
//! deterministic provider 401 that pi-acp maps to `end_turn` (see the
//! HERMETIC DEVIATION note below) — no real model call, no cost.
//!
//! **Skip rule (a diagnostic, not a CI gate):** if no `pi-acp` (or `pi`)
//! binary resolves, the tests `eprintln!` a SKIP and return — never fail.
//! The fake-agent `subagent_concurrency.rs` remains the always-on CI gate.
//!
//! ============================================================================
//! EXPERIMENT LOG (the spine of the deliverable's evidence — one line per
//! experiment: date, versions, topology, hang or not, per-session outcome,
//! wall time)
//! ============================================================================
//! 2026-09-22  probe   keyless hermetic (clean env, temp HOME)
//!             → initialize OK; session/new FAILED "Authentication required"
//!             (pi registers models only for providers with an env key —
//!             0 models → the session can never be created). The plan's
//!             keyless rule is empirically impossible.
//! 2026-09-22  probe   clean env + fake OPENAI_API_KEY (hermetic deviation)
//!             → initialize OK (pi-acp 0.0.33); session/new OK (39 models,
//!             openai/gpt-5.5); prompt resolved `end_turn` in ~0.5s
//!             (deterministic provider 401 — pi-acp maps provider errors to
//!             end_turn; no real model call).
//! 2026-09-22  repro   two_real_sessions_one_runtime_hang
//!             pi-acp 0.0.33 + pi 0.87.0 (two runs, --test-threads=1)
//!             → PASS (no hang). Run 1: session 1 -> EndTurn, session 2 ->
//!             EndTurn, prompt phase 511ms. Run 2: session 1 -> EndTurn,
//!             session 2 -> EndTurn, prompt phase 597ms. Both wrapper shells
//!             alive before the prompt phase; both process trees (sh + pi-acp
//!             + pi) reaped after close. Total wall time ~10.6s / ~11.1s.
//! 2026-09-22  control two_real_sessions_two_runtimes_pass
//!             pi-acp 0.0.33 + pi 0.87.0 (two runs, --test-threads=1)
//!             → PASS (expected — the shipped ADR 0004 topology). Run 1:
//!             session 1 -> EndTurn, session 2 -> EndTurn, prompt phase
//!             715ms. Run 2: session 1 -> EndTurn, session 2 -> EndTurn,
//!             prompt phase 1.04s. Reap asserts passed. (The `EndTurn`
//!             outcomes are the hermetic deviation's deterministic provider
//!             401 — pi-acp maps provider errors to `end_turn`; no real
//!             model call.)
//!
//! GATE VERDICT (the next-task gate): the repro PASSES — the 2026-09-15
//! hang does NOT reproduce on the current versions (pi-acp 0.0.33 + pi
//! 0.87.0, `agent-client-protocol` 2.1.0, tokio 1.53.1, async-io 2.6.0):
//! two concurrent real ACP sessions on ONE multi-threaded runtime (2
//! workers) both answer `send_prompt` in well under 1 s. The hang is
//! resolved (or was version/environment-specific and no longer present)
//! — the orchestrator goes straight to Task 4 (the "resolved" conclusion:
//! lift the cap). Tasks 2-3 are NOT run (conditional on the repro
//! hanging).
//!
//! HERMETIC DEVIATION (recorded, per the plan's escape-hatch rule "verify
//! empirically, don't assume"): the plan's keyless rule (unset PI_KEY /
//! PI_KEY_ENV, empty temp HOME) is EMPIRICALLY IMPOSSIBLE with pi-acp
//! 0.0.33 — `session/new` fails with "Authentication required" because pi
//! registers models only for providers with an env key (a keyless run has 0
//! models and the session can never be created; verified 2026-09-22, probe
//! line above). The harness therefore uses a FULLY CLEAN env (the wrapper's
//! `env -i`: the inherited env is NOT the vehicle — it leaks the real
//! provider keys on this box, e.g. OPENROUTER_API_KEY) + a temp
//! HOME/XDG_CONFIG_HOME (empty pi config surface: no settings.json, no
//! extensions) + a FAKE constant OPENAI_API_KEY (the session creates; the
//! prompt resolves with a deterministic provider 401 that pi-acp maps to
//! `end_turn` — no real model call, no cost, no real key).
//! ============================================================================

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    ClientCapabilities, ContentBlock, FileSystemCapabilities, InitializeRequest, NewSessionRequest,
    PromptRequest, ReadTextFileRequest, ReadTextFileResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionNotification,
    StopReason, TextContent, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{
    on_receive_notification, on_receive_request, AcpAgent, AcpAgentConfig, Agent, Client,
    ConnectionTo, Responder,
};

/// The HARD timeout for the PROMPT phase (the investigation rule: a hang
/// must be a CLEAN RED test, never a stalled `cargo test` — the
/// `subagent_concurrency.rs` pattern).
const PROMPT_TIMEOUT: Duration = Duration::from_secs(10);
/// The bound for the ESTABLISH phase (initialize + session/new; ~2 s on
/// this box — bounded so a broken setup is a clean red, not a stall).
const ESTABLISH_TIMEOUT: Duration = Duration::from_secs(15);
/// The reap-poll budget (the `wait_for_process_gone` pattern, acp_flow.rs:138).
const REAP_BUDGET: Duration = Duration::from_secs(5);
/// The fake constant provider key (the HERMETIC DEVIATION note in the
/// experiment log — a keyless run cannot create a session at all).
const FAKE_KEY: &str = "sk-fake-archimedes-diagnostic";

// ---------------------------------------------------------------------------
// Resolution + harness setup (the SKIP RULE lives here)
// ---------------------------------------------------------------------------

/// Resolve a binary: an env override first (for local runs), then `which`
/// (PATH lookup — on this box: `/usr/local/bin/pi-acp`, `pi` at
/// `~/.local/bin/pi`).
fn resolve_binary(override_var: Option<&str>, name: &str) -> Option<PathBuf> {
    if let Some(var) = override_var {
        if let Ok(p) = std::env::var(var) {
            let pb = PathBuf::from(p);
            if pb.is_file() {
                return Some(pb);
            }
        }
    }
    let out = std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!line.is_empty()).then_some(PathBuf::from(line))
}

/// `pi --version` (the pi-acp version is captured from the `initialize`
/// response — `pi-acp --version` prints nothing).
fn version_of(bin: &Path) -> String {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .to_string()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Write a PER-TEST UNIQUE wrapper (the `unique_fake_agent` lesson — tests
/// run in parallel and all launch the same binary; a `pgrep` on the shared
/// path cannot tell whose agent is whose) that CARRIES THE HERMETIC ENV
/// (`AcpAgentConfig` has no `env_remove`/`env_clear` — the wrapper is the
/// only vehicle).
///
/// **CRITICAL — NO `exec`:** `exec` replaces argv — before exec the cmdline
/// is `/bin/sh <this path>` (matches the `pgrep -f` needle), but AFTER exec
/// it is the pi-acp path (NO match) → the "2 alive" assert deterministically
/// reads 0 (spurious red) and "0 after" is VACUOUSLY true (leak detection
/// dead). Without `exec` the shell WAITS as the parent: its cmdline survives
/// for `pgrep -f`, and the crate's process-group kill reaps sh + pi-acp +
/// the pi grandchild TOGETHER. The count covers the 2 WRAPPER shells ONLY
/// (a child's argv never contains the parent's script path) — the shells
/// exiting PROVES their child tree exited.
fn write_wrapper(base: &Path, idx: u8, pi_acp: &Path, pi: &Path) -> PathBuf {
    let path = base.join(format!("wrapper-{idx}-{}.sh", uuid::Uuid::new_v4()));
    let script = format!(
        r#"#!/bin/sh
# Per-test unique wrapper — see the harness (NO exec; the shell waits as
# the parent so `pgrep -f <this path>` sees it). `env -i` gives the child
# a FULLY CLEAN env (the HERMETIC DEVIATION note in the experiment log):
# the inherited env is NOT the vehicle — it leaks the real provider keys
# on this box. Temp HOME/XDG_CONFIG_HOME = empty pi config surface; the
# fake OPENAI_API_KEY lets session/new succeed (a keyless run has 0
# models and fails it); PI_ACP_PI_COMMAND pins the pi grandchild (ADR
# 0005).
env -i \
  PATH="$PATH" \
  HOME="{home}" \
  XDG_CONFIG_HOME="{xdg}" \
  OPENAI_API_KEY="{key}" \
  PI_ACP_PI_COMMAND="{pi}" \
  {pi_acp}
"#,
        home = base.join("home").display(),
        xdg = base.join("config").display(),
        key = FAKE_KEY,
        pi = pi.display(),
        pi_acp = pi_acp.display(),
    );
    std::fs::write(&path, script).unwrap();
    path
}

/// The harness: resolved binaries + the per-test unique wrappers + the
/// hermetic temp dirs. `None` = SKIP (the skip rule: a diagnostic, not a
/// CI gate — the fake-agent `subagent_concurrency.rs` remains the gate).
struct Harness {
    base: PathBuf,
    wrapper1: PathBuf,
    wrapper2: PathBuf,
    cwd1: PathBuf,
    cwd2: PathBuf,
    pi_acp: PathBuf,
    pi_version: String,
}

fn setup_harness(tag: &str) -> Option<Harness> {
    let pi_acp = match resolve_binary(Some("PI_ACP_BINARY"), "pi-acp") {
        Some(p) => p,
        None => {
            eprintln!("SKIP: no `pi-acp` binary found (set PI_ACP_BINARY to run the diagnostic)");
            return None;
        }
    };
    // `pi` must ALSO be resolvable (pi-acp spawns `pi --mode rpc
    // --no-themes` as a GRANDCHILD — ADR 0005; pinned via
    // PI_ACP_PI_COMMAND in the wrapper).
    let pi = match resolve_binary(None, "pi") {
        Some(p) => p,
        None => {
            eprintln!("SKIP: no `pi` binary found (pi-acp spawns `pi` as a grandchild — ADR 0005)");
            return None;
        }
    };
    let base = std::env::temp_dir().join(format!(
        "acp-concurrency-real-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    let home = base.join("home");
    let xdg = base.join("config");
    let cwd1 = base.join("cwd1");
    let cwd2 = base.join("cwd2");
    for dir in [&home, &xdg, &cwd1, &cwd2] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let wrapper1 = write_wrapper(&base, 0, &pi_acp, &pi);
    let wrapper2 = write_wrapper(&base, 1, &pi_acp, &pi);
    Some(Harness {
        base,
        wrapper1,
        wrapper2,
        cwd1,
        cwd2,
        pi_acp,
        pi_version: version_of(&pi),
    })
}

// ---------------------------------------------------------------------------
// Process-count helpers (the reap assertions)
// ---------------------------------------------------------------------------

/// Count the running processes matching `pattern` (`pgrep -f` extended to a
/// COUNT — the wrapper path is the pattern; the count covers the wrapper
/// shells only).
fn count_processes(wrapper: &Path) -> usize {
    let out = match std::process::Command::new("pgrep")
        .args(["-f", wrapper.to_string_lossy().as_ref()])
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

/// Poll until the wrapper's process tree is gone (budget `REAP_BUDGET`).
fn wait_for_gone(wrapper: &Path) -> bool {
    let deadline = Instant::now() + REAP_BUDGET;
    loop {
        if count_processes(wrapper) == 0 {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// One ACP session (the desktop's production wiring, mirrored)
// ---------------------------------------------------------------------------

/// One real ACP session: the crate-spawned `AcpAgent` + the desktop's
/// `Client::builder()` handlers + the `connect_with` establisher (which
/// BLOCKS-UNTIL-CLOSE — see the module docs).
struct RealSession {
    /// A CLONE of the connection handle handed out by the establisher (the
    /// establisher keeps the original for the block-until-close).
    cx: ConnectionTo<Agent>,
    session_id: SessionId,
    /// The adapter's self-reported version (the `initialize`
    /// `agentInfo.version`).
    agent_version: String,
    /// The close signal: dropping it releases the establisher's
    /// block-until-close → `connect_with` returns → the crate tears down
    /// the transport (`ChildGuard::drop` SIGKILLs the process group).
    close_tx: tokio::sync::watch::Sender<()>,
}

/// Spawn ONE real ACP session on `runtime` and return its handle once the
/// session is established (the values arrive via channels — the caller
/// NEVER awaits `connect_with` inline: that would block until close — a
/// false hang).
async fn spawn_real_session(
    runtime: &tokio::runtime::Runtime,
    wrapper: &Path,
    cwd: &Path,
    idx: u8,
) -> Result<RealSession, String> {
    // THE CRATE SPAWNS THE CHILD (the mechanism note — load-bearing):
    // `AcpAgent` spawns the wrapper via `async_process::Child` (an
    // async-io-based transport — the exact ADR 0002 suspicion surface).
    // The wrapper (NOT `exec`'d) is the command: it carries the hermetic
    // env and survives for `pgrep -f`.
    let cwd_owned = cwd.to_path_buf();
    let agent = AcpAgent::new(
        AcpAgentConfig::new("/bin/sh").args(vec![wrapper.to_string_lossy().into_owned()]),
    );

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<ConnectionTo<Agent>>();
    let (est_tx, est_rx) = tokio::sync::oneshot::channel::<Result<(SessionId, String), String>>();
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(());

    // The DRIVER task (the desktop's `connect_with` driver, session.rs:357):
    // `connect_with` only resolves when the establisher returns (i.e. at
    // session close), so it must never be awaited inline here.
    runtime.spawn(async move {
        let builder = Client
            .builder()
            .name(format!("concurrency-real-{idx}"))
            // The desktop's notification sink (session.rs:379-432) — a
            // no-op capture here (minimal; the desktop persists + captures).
            .on_receive_notification(
                async move |_notif: SessionNotification, _cx: ConnectionTo<Agent>| Ok(()),
                on_receive_notification!(),
            )
            // The desktop's fs handlers (session.rs:435-458) — respond with
            // an internal error (the desktop's shape): an UNANSWERED fs
            // probe STALLS the prompt → a FALSE hang signal in the hot path.
            .on_receive_request(
                async move |_req: ReadTextFileRequest,
                            responder: Responder<ReadTextFileResponse>,
                            _cx: ConnectionTo<Agent>| {
                    responder.respond_with_internal_error("diagnostic harness: fs disabled")?;
                    Ok(())
                },
                on_receive_request!(),
            )
            .on_receive_request(
                async move |_req: WriteTextFileRequest,
                            responder: Responder<WriteTextFileResponse>,
                            _cx: ConnectionTo<Agent>| {
                    responder.respond_with_internal_error("diagnostic harness: fs disabled")?;
                    Ok(())
                },
                on_receive_request!(),
            )
            // The desktop's permission handler (permission.rs:126-127) —
            // CANCELLED (the cleanest stall-breaker; a "deny" would be
            // `Selected(…)`): an UNANSWERED `session/requestPermission`
            // STALLS the prompt → a FALSE hang signal in the hot path.
            .on_receive_request(
                async move |_req: RequestPermissionRequest,
                            responder: Responder<RequestPermissionResponse>,
                            _cx: ConnectionTo<Agent>| {
                    responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ))?;
                    Ok(())
                },
                on_receive_request!(),
            );

        let _ = builder
            .connect_with(agent, |cx: ConnectionTo<Agent>| async move {
                // (a) Hand a CLONE out; the establisher KEEPS the original
                //     `cx` for the block (the desktop's `cx2` pattern,
                //     session.rs:480-481).
                ready_tx.send(cx.clone()).ok();

                // (b) Establish: `initialize` + `session/new` (the
                //     desktop's establisher shape, session.rs:857-873),
                //     bounded + close-aware.
                let established = tokio::select! {
                    r = tokio::time::timeout(ESTABLISH_TIMEOUT, async {
                        let init = cx
                            .send_request(
                                InitializeRequest::new(ProtocolVersion::V1)
                                    .client_capabilities(
                                        ClientCapabilities::default()
                                            .fs(
                                                FileSystemCapabilities::default()
                                                    .read_text_file(true)
                                                    .write_text_file(true),
                                            )
                                            .terminal(false),
                                    ),
                            )
                            .block_task()
                            .await?;
                        let new_session = cx
                            .send_request(NewSessionRequest::new(cwd_owned.clone()))
                            .block_task()
                            .await?;
                        let version = init
                            .agent_info
                            .map(|info| info.version)
                            .unwrap_or_else(|| "unknown".to_string());
                        Ok::<_, agent_client_protocol::Error>((
                            new_session.session_id.clone(),
                            version,
                        ))
                    }) => r,
                    // A close during the establish window: report and tear
                    // down (the returning establisher closes the
                    // connection — correct here: the test asked to close).
                    _ = close_rx.changed() => {
                        est_tx.send(Err("closed during establish".to_string())).ok();
                        return Ok(());
                    }
                };
                let (session_id, agent_version) = match established {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => {
                        est_tx
                            .send(Err(format!("establish failed: {}", e.message)))
                            .ok();
                        return Ok(());
                    }
                    Err(_) => {
                        est_tx
                            .send(Err(format!(
                                "establish timed out after {ESTABLISH_TIMEOUT:?}"
                            )))
                            .ok();
                        return Ok(());
                    }
                };
                est_tx.send(Ok((session_id, agent_version))).ok();

                // (c) BLOCK until the test drops `close_tx` OR the agent
                //     process exits. A RETURNING establisher closes the
                //     connection and CORRUPTS THE GATE: the prompt would
                //     resolve instantly with connection errors and the
                //     "any outcome is fine" liveness assertion would accept
                //     that as "resolved" — a silent false
                //     "resolved → lift the cap".
                tokio::select! {
                    _ = close_rx.changed() => {}
                    _ = cx.incoming_closed() => {}
                }
                Ok(())
            })
            .await;
    });

    // The caller (run on the runtime via `block_on`) receives the values via
    // the channels and RETURNS once they arrive.
    let cx = ready_rx
        .await
        .map_err(|e| format!("ready channel closed: {e}"))?;
    let established = est_rx
        .await
        .map_err(|e| format!("establish channel closed: {e}"))?;
    let (session_id, agent_version) = established?;
    Ok(RealSession {
        cx,
        session_id,
        agent_version,
        close_tx,
    })
}

/// One prompt (the desktop's `send_prompt` shape, session.rs:1015-1048):
/// `send_request` + `block_task`. Any outcome (`end_turn`, a refusal, or an
/// error) is FINE — the assertion is transport LIVENESS (the future
/// resolves), not semantic completion.
async fn send_prompt(cx: &ConnectionTo<Agent>, sid: &SessionId) -> Result<StopReason, String> {
    let request = PromptRequest::new(
        sid.clone(),
        vec![ContentBlock::Text(TextContent::new(
            "Reply with the word ok",
        ))],
    );
    let response = cx
        .send_request(request)
        .block_task()
        .await
        .map_err(|e| e.message)?;
    Ok(response.stop_reason)
}

/// Format a prompt outcome for the experiment log (the outcome — `end_turn`
/// / auth-error / other — is DATA for the log, not an assertion).
fn outcome_str(r: &Result<StopReason, String>) -> String {
    match r {
        Ok(reason) => format!("{reason:?}"),
        Err(message) => format!("error: {message}"),
    }
}

/// Build the REFERENCE runtime shape — the `WorkerRuntime` (ADR 0004:
/// `new_multi_thread().worker_threads(2)`, worker_runtime.rs:81-85; a
/// stand-in for Tauri's main runtime).
fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime should build")
}

// ---------------------------------------------------------------------------
// The repro (the 2026-09-15 topology) + the control (the shipped topology)
// ---------------------------------------------------------------------------

/// **THE REPRO** (the 2026-09-15 topology — ADR 0002): two real pi-acp ACP
/// sessions on ONE tokio runtime. Both `send_prompt` futures must RESOLVE
/// (any outcome) within 10 s; a non-resolution is the hang (the timeout
/// firing IS the hang repro — a clean red test).
#[test]
fn two_real_sessions_one_runtime_hang() {
    let harness = match setup_harness("repro") {
        Some(h) => h,
        None => return,
    };
    eprintln!(
        "repro: pi-acp {} + pi {} — ONE runtime (the 2026-09-15 topology)",
        harness.pi_acp.display(),
        harness.pi_version
    );

    // ONE multi-threaded runtime (the `WorkerRuntime` shape, ADR 0004 — a
    // stand-in for Tauri's main runtime).
    let rt = build_runtime();

    let s1 = rt
        .block_on(spawn_real_session(&rt, &harness.wrapper1, &harness.cwd1, 0))
        .unwrap_or_else(|e| panic!("session 1 establish failed: {e}"));
    let s2 = rt
        .block_on(spawn_real_session(&rt, &harness.wrapper2, &harness.cwd2, 1))
        .unwrap_or_else(|e| panic!("session 2 establish failed: {e}"));
    eprintln!(
        "repro: session 1 = {} (pi-acp {}), session 2 = {}",
        s1.session_id, s1.agent_version, s2.session_id
    );

    // Both wrapper shells alive BEFORE the prompt phase (the "2 alive"
    // assert, subagent_concurrency.rs:342 — one wrapper shell per session).
    assert_eq!(
        count_processes(&harness.wrapper1),
        1,
        "session 1's wrapper shell should be alive before the prompt phase"
    );
    assert_eq!(
        count_processes(&harness.wrapper2),
        1,
        "session 2's wrapper shell should be alive before the prompt phase"
    );

    // THE REPRO: both `send_prompt` concurrently on ONE runtime, wrapped in
    // a HARD 10 s timeout. The timeout is CONSTRUCTED INSIDE the `async`
    // block (tokio 1.53: `timeout` requires the current runtime context at
    // construction — a plain test thread has none), and the `join!` is
    // inside the timeout's `async` block (in tokio 1.53 it expands to an
    // already-awaited expression).
    let (c1, c2) = (s1.cx.clone(), s2.cx.clone());
    let (sid1, sid2) = (s1.session_id.clone(), s2.session_id.clone());
    let started = Instant::now();
    let joined = rt.block_on(async {
        tokio::time::timeout(PROMPT_TIMEOUT, async {
            tokio::join!(send_prompt(&c1, &sid1), send_prompt(&c2, &sid2))
        })
        .await
    });
    let elapsed = started.elapsed();
    let (r1, r2) = match joined {
        Ok((r1, r2)) => (r1, r2),
        Err(_) => {
            eprintln!(
                "HANG REPRODUCED: at least one session's send_prompt did not resolve \
                 in 10s (elapsed {elapsed:?}) — the 2026-09-15 topology (two real \
                 sessions, one runtime) hangs"
            );
            panic!(
                "HANG REPRODUCED: a session's send_prompt did not resolve within \
                 10s (elapsed {elapsed:?}) — the 2026-09-15 topology (two real \
                 sessions, one runtime) hangs"
            );
        }
    };
    // Any outcome (`end_turn`, a refusal, or an error) is FINE — the
    // assertion is transport liveness. The outcomes are DATA for the
    // experiment log.
    let (o1, o2) = (outcome_str(&r1), outcome_str(&r2));
    eprintln!("repro: session 1 -> {o1}, session 2 -> {o2} (prompt phase {elapsed:?})");

    // CLOSE: drop the close signals (the drop IS the close — it releases the
    // establisher → `connect_with` returns → the crate SIGKILLs the process
    // group), BEFORE the reap poll (dropping them only after the reap
    // assertions would make "0 after" a spurious red — the shells cannot be
    // reaped before the drop).
    drop(s1.close_tx);
    drop(s2.close_tx);
    assert!(
        wait_for_gone(&harness.wrapper1),
        "session 1's process tree (sh + pi-acp + pi) should be reaped after close"
    );
    assert!(
        wait_for_gone(&harness.wrapper2),
        "session 2's process tree (sh + pi-acp + pi) should be reaped after close"
    );

    // Teardown: bounded shutdown (the `Runtime` is dropped on this plain
    // test thread — never from an async context).
    rt.shutdown_timeout(Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&harness.base);
}

/// **THE CONTROL** (the ADR 0004 shipped topology): the same two sessions,
/// each on its OWN multi-threaded runtime. Expected: PASS — if it FAILS,
/// the bug is not runtime-topology-specific (record and stop).
#[test]
fn two_real_sessions_two_runtimes_pass() {
    let harness = match setup_harness("control") {
        Some(h) => h,
        None => return,
    };
    eprintln!(
        "control: pi-acp {} + pi {} — TWO runtimes (the ADR 0004 shipped topology)",
        harness.pi_acp.display(),
        harness.pi_version
    );

    // TWO runtimes, each the shipped `WorkerRuntime` shape (NOT
    // `Runtime::new()` — the shipped shape is
    // `new_multi_thread().worker_threads(2)`).
    let rt1 = build_runtime();
    let rt2 = build_runtime();

    let s1 = rt1
        .block_on(spawn_real_session(
            &rt1,
            &harness.wrapper1,
            &harness.cwd1,
            0,
        ))
        .unwrap_or_else(|e| panic!("session 1 establish failed: {e}"));
    let s2 = rt2
        .block_on(spawn_real_session(
            &rt2,
            &harness.wrapper2,
            &harness.cwd2,
            1,
        ))
        .unwrap_or_else(|e| panic!("session 2 establish failed: {e}"));
    eprintln!(
        "control: session 1 = {} (pi-acp {}), session 2 = {}",
        s1.session_id, s1.agent_version, s2.session_id
    );

    // Both wrapper shells alive BEFORE the prompt phase (the "2 alive"
    // assert, subagent_concurrency.rs:342).
    assert_eq!(
        count_processes(&harness.wrapper1),
        1,
        "session 1's wrapper shell should be alive before the prompt phase"
    );
    assert_eq!(
        count_processes(&harness.wrapper2),
        1,
        "session 2's wrapper shell should be alive before the prompt phase"
    );

    // Both prompts concurrently: session 1 on rt1 (a dedicated OS thread —
    // a cloned `Handle` drives `block_on` there; `shutdown_timeout` needs
    // the `Runtime` BY VALUE, so the runtimes stay owned here), session 2
    // on rt2 (this thread). Each is 10 s-timeout-wrapped (the
    // investigation rule).
    let (c1, c2) = (s1.cx.clone(), s2.cx.clone());
    let (sid1, sid2) = (s1.session_id.clone(), s2.session_id.clone());
    let started = Instant::now();
    let rt1_handle = rt1.handle().clone();
    let prompt1 = std::thread::spawn(move || {
        // The timeout is CONSTRUCTED INSIDE the `async` block (tokio 1.53:
        // `timeout` requires the current runtime context at construction —
        // the fresh thread has none until `block_on` sets it).
        rt1_handle
            .block_on(async { tokio::time::timeout(PROMPT_TIMEOUT, send_prompt(&c1, &sid1)).await })
    });
    let r1: Result<StopReason, String> = prompt1
        .join()
        .expect("the session-1 prompt thread should not panic")
        .map_err(|_| "session 1's prompt did not resolve in 10s".to_string())
        .and_then(|r| r);
    let r2: Result<StopReason, String> = rt2
        .block_on(async { tokio::time::timeout(PROMPT_TIMEOUT, send_prompt(&c2, &sid2)).await })
        .map_err(|_| "session 2's prompt did not resolve in 10s".to_string())
        .and_then(|r| r);
    let elapsed = started.elapsed();
    match (&r1, &r2) {
        (Ok(_), Ok(_)) => {
            eprintln!(
                "control: session 1 -> {}, session 2 -> {} (prompt phase {elapsed:?})",
                outcome_str(&r1),
                outcome_str(&r2)
            );
        }
        (a, b) => {
            eprintln!(
                "CONTROL FAILED: session 1 -> {}, session 2 -> {} (prompt phase \
                 {elapsed:?}) — the shipped ADR 0004 topology did NOT pass: the \
                 bug is not runtime-topology-specific",
                outcome_str(a),
                outcome_str(b)
            );
            panic!(
                "CONTROL FAILED: the two-runtime topology (the shipped ADR 0004 \
                 shape) did not pass — the bug is not runtime-topology-specific"
            );
        }
    }

    // CLOSE + reap (the same block-until-close / drop-is-close / reap-poll
    // pattern as the repro).
    drop(s1.close_tx);
    drop(s2.close_tx);
    assert!(
        wait_for_gone(&harness.wrapper1),
        "session 1's process tree (sh + pi-acp + pi) should be reaped after close"
    );
    assert!(
        wait_for_gone(&harness.wrapper2),
        "session 2's process tree (sh + pi-acp + pi) should be reaped after close"
    );

    // Teardown: bounded shutdown of both runtimes (dropped on this plain
    // test thread — never from an async context).
    rt1.shutdown_timeout(Duration::from_secs(5));
    rt2.shutdown_timeout(Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&harness.base);
}
