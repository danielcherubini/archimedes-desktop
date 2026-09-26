//! The bridge listener: a per-spawn, peer-verified local channel the agent's
//! archimedes suite uses to reach the desktop (the Client) — ADR 0003.
//!
//! The agent opens a **new connection per message** and destroys it on the
//! first data (one frame per connection — see the suite's `channel.ts`), so
//! [`handle_connection`] reads exactly ONE frame (a line) and dispatches:
//!
//! - **request frame** → register a oneshot in `pending_bridge` (keyed
//!   `"{session_id}/{request_id}"`, the `permission` convention) **before**
//!   emitting the `bridge-request` event (the UI's `respond_bridge_request`
//!   may race the emission and must find the entry), then spawn a **waiter
//!   that owns the stream** (330 s cap; the agent sends nothing while
//!   waiting, so EOF on the stream — the agent's close — is the
//!   immediate-cancel signal).
//! - **push frame** → `seq` drop (keep `last_seq` per listener; drop
//!   `seq <= last_seq`), emit a `bridge-event`, write an ack line, close.
//!
//! **Peer verification (fail-closed):** a connection is accepted only if the
//! connecting process is a **descendant of the desktop itself** — the anchor
//! is the desktop's own pid (`std::process::id()`, always alive). The
//! topology is desktop (D) → `pi` (A, a direct child of D — the desktop
//! spawns `pi --mode rpc` directly; the subagent dispatch spawns the same
//! binary with the `ARCHIMEDES_SUBAGENT=1` env); the peer connecting to the
//! desktop's socket is `pi` (A). On
//! Linux the peer's pid comes from `SO_PEERCRED` and the parent chain is
//! walked via `/proc/<pid>/status` (bounded ≤8 hops). On **macOS** (no
//! `SO_PEERCRED`/`ucred`) the listener is **not started** (fail-closed — the
//! bridge is unavailable, a documented v1 limitation).

use std::collections::HashMap;
use std::future::Future;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, watch, Mutex};

use crate::agent::session::{CostAccumulator, EventSink, SubagentSpawn};
use crate::agent::subagent::LaunchConfig;
use crate::agent::subagent::{SubagentMetrics, SubagentOutcome};
use crate::agent::todo::{TodoItem, TodoStatus, TodoStore};

/// The manager's map of pending bridge-request senders.
///
/// Keyed by `"{session_id}/{request_id}"` so the driver-task cleanup can
/// drain all entries for a closing session at once (dropping the senders
/// cancels the spawned waiters). The oneshot carries the response **`result`
/// `Value` verbatim** — no wrapper (for `password`, `{password}`; for
/// `confirm`, `{confirmed}`; for `ask`, the `AskResponsePayload`).
pub type PendingBridge = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// Build the compound key for a pending bridge request.
pub fn bridge_key(session_id: &str, request_id: &str) -> String {
    format!("{session_id}/{request_id}")
}

/// The key prefix of all pending-bridge keys that belong to `session_id`.
/// Keys are `"{session_id}/{request_id}"`, so the prefix carries the trailing
/// slash: a session id that is a plain prefix of another ("s1" vs "s10")
/// must not drain the other session's requests.
pub fn session_key_prefix(session_id: &str) -> String {
    format!("{session_id}/")
}

/// The manager's map of pending `sudo_exec` SUB-prompts (Phase 2, Task 1).
///
/// One entry per sub-prompt, keyed `"{session_id}/{request_id}:confirm"` /
/// `"{session_id}/{request_id}:password"` (the `requestId` the modals echo
/// VERBATIM into `respond_bridge_request`). Each oneshot carries the raw
/// `respond_bridge_request` `result` `Value` (for `:confirm`, `{confirmed}`;
/// for `:password`, `{password}` — an empty `""` is a cancel).
///
/// The driver-task cleanup drains all entries for a closing session at once
/// (dropping the senders cancels the in-flight sub-prompt waiters).
pub type PendingSudo = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

/// The per-session sudo credential (the suite's `CachedCredential`,
/// `packages/sudo/src/cache.ts` — in-memory only, never disk/keyring).
#[derive(Debug, Clone)]
pub struct CachedPassword {
    pub password: String,
    pub expires_at: std::time::Instant,
}

/// The sudo password-cache TTL (the suite's `DEFAULT_SUDO_CONFIG.ttlMs` —
/// 15 min, config key `archimedes.sudo.ttlMs`).
pub const SUDO_TTL: Duration = Duration::from_millis(900_000);

/// The `sudo_exec` default command timeout (the suite's
/// `DEFAULT_SUDO_CONFIG.defaultTimeoutMs` — 120 s): the RUN is bounded by
/// the caller's `timeoutMs` (absent = this), NOT by the 330 s sub-prompt
/// cap.
pub const SUDO_DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// The `SudoRunner`'s outcome: `timed_out: true` + `exit_code` (the timeout
/// sentinel, 124) = a timeout; `error: Some(…)` = a spawn failure (NOT an
/// auth failure — the cached credential is kept).
#[derive(Debug, Clone)]
pub struct SudoRun {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub error: Option<String>,
}

/// The `sudo_exec` execution seam (mirrors the suite's `SudoSpawner` seam,
/// `packages/sudo/src/tool.ts` `createSudoExecTool({spawner})`): the desktop
/// runs `sudo -S` on the host through this trait, so the tests use a fake
/// runner (no real `sudo` in `cargo test`) and the real runner enters
/// through `start_listener` → `ConnCtx` → the handler.
///
/// MUST be dyn-compatible (`Arc<dyn SudoRunner>` is the `start_listener`
/// parameter type) — a plain `async fn` in a trait is NOT dyn-compatible,
/// hence the boxed-future return (the boxed-future pattern).
pub trait SudoRunner: Send + Sync {
    /// Run `sudo -S <argv>` (the full argv from [`build_sudo_argv`] — no
    /// `-p`), writing `password` to stdin (never argv/env), bounded by
    /// `timeout` (on timeout the WHOLE process group is SIGKILLed — the
    /// suite's `killProcessTree`).
    fn run(
        &self,
        argv: Vec<String>,
        password: String,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = SudoRun> + Send + 'static>>;
}

/// The real `SudoRunner`: spawn `sudo -S <argv>` on the desktop host (its
/// OWN process group on Unix — the suite's `detached: true`), write the
/// password to stdin, and SIGKILL the WHOLE process group on timeout (a
/// bare timeout would leave the timed-out root children alive).
pub struct RealSudoRunner;

impl SudoRunner for RealSudoRunner {
    fn run(
        &self,
        argv: Vec<String>,
        password: String,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = SudoRun> + Send + 'static>> {
        Box::pin(run_sudo_real(argv, password, timeout))
    }
}

/// The real sudo run (see [`RealSudoRunner`]).
async fn run_sudo_real(argv: Vec<String>, password: String, timeout: Duration) -> SudoRun {
    let Some((cmd0, rest)) = argv.split_first() else {
        return SudoRun {
            exit_code: -1,
            stdout: String::new(),
            stderr: "no command in argv".to_string(),
            timed_out: false,
            error: Some("no command in argv".to_string()),
        };
    };
    let mut cmd = tokio::process::Command::new(cmd0);
    cmd.args(rest);
    // Pipe ALL THREE streams: the password travels the stdin pipe, and
    // stdout / stderr are captured into the shared buffers below. The
    // tokio default is INHERIT — without the explicit `piped()`,
    // `child.stdout` is `None` (the capture below would panic) and the
    // password is never written at all.
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // The child owns its process group (the suite's `detached: true`):
    // `setpgid(0, 0)` in the child (a single async-signal-safe libc call
    // — the only `unsafe` here) makes it the leader of a fresh group —
    // on timeout the WHOLE group is SIGKILLed (a root process that forked
    // further survives a bare kill of the child alone).
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SudoRun {
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                error: Some(format!("failed to spawn {cmd0}: {e}")),
            }
        }
    };
    // The password travels via stdin ONLY (`sudo -S` reads it from there —
    // it never appears in argv or env). A fatal EPIPE (the child finished
    // without reading it — e.g. a NOPASSWD sudo) is NOT an auth failure:
    // the child's exit settles the run (the suite's EPIPE policy — record
    // only, don't fail), so a write ERROR is ignored here.
    //
    // The write is BOUNDED: if the child never reads stdin AND the
    // password exceeds the pipe buffer (~64 KiB), `write_all` would block
    // unboundedly (no timeout, no close-guard) — a 5 s cap maps a stuck
    // write to a process-level failure (kill + reap + `error: Some(…)`).
    // It is NOT an auth failure: the cached credential is kept (`
    // handle_sudo_exec` maps `error: Some(…)` to a clean tool failure and
    // never clears the cache on it).
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = format!("{password}\n").into_bytes();
        let write = stdin.write_all(&bytes);
        let write = match tokio::time::timeout(Duration::from_secs(5), write).await {
            Ok(r) => r,
            Err(_) => {
                kill_process_group_and_reap(&mut child).await;
                return SudoRun {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: false,
                    error: Some("timed out writing password to sudo stdin".to_string()),
                };
            }
        };
        let _ = write;
        let _ = stdin.shutdown().await;
    }
    let mut stdout = child.stdout.take().expect("stdout handle");
    let mut stderr = child.stderr.take().expect("stderr handle");
    // Shared (chunked) buffers: the PARTIAL data survives a timeout (the
    // read tasks are dropped, the buffers keep what was read so far).
    let stdout_buf = Arc::new(StdMutex::new(Vec::new()));
    let stderr_buf = Arc::new(StdMutex::new(Vec::new()));
    let read = async {
        let (o, e) = tokio::join!(
            read_stream_into(&mut stdout, &stdout_buf),
            read_stream_into(&mut stderr, &stderr_buf),
        );
        (o, e, child.wait().await)
    };
    let result = tokio::time::timeout(timeout, read).await;
    let (stdout, stderr) = (
        String::from_utf8_lossy(&stdout_buf.lock().unwrap_or_else(|p| p.into_inner())).into_owned(),
        String::from_utf8_lossy(&stderr_buf.lock().unwrap_or_else(|p| p.into_inner())).into_owned(),
    );
    match result {
        Ok((o, e, w)) => {
            // A read failure (a broken pipe that is NOT an EPIPE) or a
            // `wait` failure: a clean transport failure (NOT an auth
            // failure — the cache is kept). Kill + reap (no zombie).
            let error = o
                .as_ref()
                .err()
                .or_else(|| e.as_ref().err())
                .or_else(|| w.as_ref().err())
                .map(|e| e.to_string());
            if let Some(error) = error {
                // Kill + reap ONLY when the `wait` did NOT succeed: when
                // `w` is `Ok` the process is already dead AND reaped — a
                // `kill` on a reaped pid is a bare `kill(pid, SIGKILL)`
                // with no reaped-state guard, and a reaped pid is free for
                // reuse (it could SIGKILL an unrelated process). A read
                // failure with a successful `wait` is just a transport
                // glitch (the child's own exit settles the run).
                if w.is_err() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                return SudoRun {
                    exit_code: -1,
                    stdout,
                    stderr,
                    timed_out: false,
                    error: Some(error),
                };
            }
            let status = w.expect("wait succeeded (checked above)");
            SudoRun {
                exit_code: status.code().unwrap_or(-1),
                stdout,
                stderr,
                timed_out: false,
                error: None,
            }
        }
        Err(_) => {
            // Timeout: kill the WHOLE process group + reap (the shared
            // helper — consistent with the write-timeout kill above).
            kill_process_group_and_reap(&mut child).await;
            SudoRun {
                // The suite's timeout sentinel (124).
                exit_code: 124,
                stdout,
                stderr,
                timed_out: true,
                error: None,
            }
        }
    }
}

/// Kill + reap a `run_sudo_real` child: SIGKILL the WHOLE process group
/// (the child is its own group on Unix — a negative pid targets the group;
/// ESRCH = the group is already gone — done), then the single-process kill
/// (belt-and-braces / non-Unix fallback), then the reap (no zombie). Shared
/// by the write-timeout and run-timeout paths so a stalled-write kill is
/// consistent with a run-timeout kill (a grandchild of the group leader
/// must not survive either).
async fn kill_process_group_and_reap(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Read a stream to EOF into `buf` (chunked — bounded; the buffer is
/// shared so the partial data survives a timeout, see
/// [`run_sudo_real`]).
async fn read_stream_into<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &Arc<StdMutex<Vec<u8>>>,
) -> std::io::Result<()> {
    let mut chunk = [0u8; 8192];
    loop {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend_from_slice(&chunk[..n]);
    }
    Ok(())
}

/// A small POSIX-ish shell-word splitter for the `sudo -S` argv
/// construction (the suite's `splitCommandIntoArgv`,
/// `packages/sudo/src/argv-split.ts`): single quotes (fully literal),
/// double quotes (backslash escapes `\"` and `\\`), and backslash escapes
/// outside quotes. Deliberately NO other shell semantics: pipes,
/// redirects, `&&`, env assignments stay in the words and pass through to
/// sudo's argv — never a shell.
fn split_command_into_argv(command: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_word = false;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '\'' => {
                // Single quotes: literal until the closing quote.
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let inner = chars[i];
                    if inner == '\'' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    current.push(inner);
                    i += 1;
                }
                has_word = true;
                if !closed {
                    break; // unterminated — the rest was literal
                }
            }
            '"' => {
                // Double quotes: backslash only escapes `"` and `\`.
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let inner = chars[i];
                    if inner == '\\'
                        && (chars.get(i + 1) == Some(&'"') || chars.get(i + 1) == Some(&'\\'))
                    {
                        current.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    if inner == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    current.push(inner);
                    i += 1;
                }
                has_word = true;
                if !closed {
                    break; // unterminated — the rest was literal
                }
            }
            '\\' => {
                // Outside quotes: escape the next character.
                let next = chars.get(i + 1);
                current.push(next.copied().unwrap_or('\\'));
                i += 2;
                has_word = true;
            }
            ' ' | '\t' | '\n' | '\r' => {
                if has_word {
                    args.push(std::mem::take(&mut current));
                    has_word = false;
                }
                i += 1;
            }
            _ => {
                current.push(ch);
                i += 1;
                has_word = true;
            }
        }
    }
    if has_word {
        args.push(current);
    }
    args
}

/// Build the full argv for `sudo -S <command>` (the suite's
/// `buildSudoArgv`, with one deliberate security-hardening divergence):
/// a proper POSIX-ish word split (quoted args stay single argv entries —
/// never a shell). A stray leading `sudo` is stripped so `sudo -S` is
/// applied exactly once. NO `-p` (the password prompt text is never
/// customized — the auth-failure signature depends on sudo's own stderr).
///
/// The `--` between the sudo options and the command words is the
/// DIVERGENCE (the suite's `buildSudoArgv` shares the hole — it passes
/// the words straight through): without it, a leading-dash word lands in
/// SUDO's own option space — `command: "-u nobody id"` → `sudo -S -u
/// nobody id` (runs as `nobody`, not root), and `command: "-p x id"` → a
/// custom `-p` prompt that DEFEATS `is_sudo_auth_failure` (the `[sudo]
/// password for` precondition never appears in stderr → a wrong password
/// is not detected as an auth failure → the bad credential stays cached
/// for the TTL). With `--`, sudo's options end there and the dash words
/// are the TARGET command's argv (a command named `-u` does not exist →
/// sudo fails cleanly). `--` is a no-op for the normal (no-leading-dash)
/// case (`sudo -S -- ls` ≡ `sudo -S ls`).
pub fn build_sudo_argv(command: &str) -> Vec<String> {
    let mut words = split_command_into_argv(command.trim());
    if words.first().is_some_and(|w| w == "sudo") {
        words.remove(0);
    }
    let mut argv = vec!["sudo".to_string(), "-S".to_string(), "--".to_string()];
    argv.extend(words);
    argv
}

/// Belt-and-braces: drop any line that contains the raw password so it
/// can never appear in tool details/result content (normally stderr is
/// clean — the password never appears in argv — but scrub defensively;
/// applied PER STREAM).
///
/// STRONGER than the suite's `scrubSecret` (a deliberate choice): the
/// suite replaces the secret SUBSTRING within the line; this replaces the
/// ENTIRE line containing the secret with `[redacted]`. The over-redaction
/// is safe (a line that mentions the password carries no useful content
/// worth preserving) and is simpler to reason about.
pub fn scrub_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.split('\n')
        .map(|line| {
            if line.contains(secret) {
                "[redacted]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The suite's EXACT auth-failure signature (`tool.ts:225-228`): the
/// sudo-prompt precondition (`/[sudo] password for/i`) AND the
/// "incorrect password" marker (`/incorrect password/i`) — case-insensitive,
/// BOTH conditions (not an OR on bare substrings, not a "parse error" —
/// there is no "parse error" in the suite).
pub fn is_sudo_auth_failure(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("[sudo] password for") && lower.contains("incorrect password")
}

/// How long a bridge request stays open before it auto-cancels (the agent's
/// 5-minute timeout + a 30 s margin, so the agent's cancel deterministically
/// wins).
pub const DEFAULT_BRIDGE_TIMEOUT: Duration = Duration::from_secs(330);

/// A started bridge listener (or a no-op on platforms where the bridge is
/// unavailable — fail-closed, ADR 0003).
#[derive(Clone)]
pub struct BridgeHandle {
    /// Flipping this to `true` stops the accept loop.
    stop_tx: watch::Sender<bool>,
    /// The bound socket path (Unix; `None` on Windows/macOS — Windows pipes
    /// vanish when the last handle closes, so there is nothing to unlink).
    socket_path: Option<PathBuf>,
    /// The session id carried in `bridge-request`/`bridge-event` payloads:
    /// the client-side placeholder until the ACP `session_id` is known
    /// (then updated by [`BridgeHandle::set_session_id`]).
    session_id: Arc<Mutex<String>>,
}

impl BridgeHandle {
    /// Update the session id carried in bridge payloads (called by the
    /// driver task once the ACP `session_id` is known).
    pub async fn set_session_id(&self, id: &str) {
        let mut sid = self.session_id.lock().await;
        *sid = id.to_string();
    }
}

/// Tear a bridge listener down (idempotent; a `None` handle is a no-op):
/// stop the accept loop and unlink the socket (Unix; Windows pipes vanish
/// when the last handle closes — no unlink API).
pub fn teardown(handle: Option<BridgeHandle>) {
    if let Some(h) = handle {
        let _ = h.stop_tx.send(true);
        if let Some(path) = h.socket_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Whether the bridge is available on this platform (Linux/Windows).
///
/// On **macOS** the bridge is unavailable (no `SO_PEERCRED`/`ucred`) — the
/// listener is not started (fail-closed, a documented v1 limitation).
pub fn available() -> bool {
    #[cfg(any(target_os = "linux", windows))]
    {
        true
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        false
    }
}

/// Start the bridge listener on `socket_path` (the per-spawn randomized
/// socket, see `session.rs`).
///
/// `anchor_pid` is the **desktop's own pid** (`std::process::id()`) — the
/// peer-verification anchor (ADR 0003). `close_tx` is the driver task's
/// close flag; the per-request waiters subscribe it so a session close
/// cancels every in-flight request. `timeout` caps each request (default
/// [`DEFAULT_BRIDGE_TIMEOUT`]; a test injects a short value).
///
/// `subagent` (main only — `Some`) is the subagent dispatch handle; the
/// `dispatch_subagent` method (method-aware: NO timeout) is serviced by it.
/// `cost_capture` (subagents only — `Some`) accumulates the `cost_update`
/// payloads (the v1 metrics source).
///
/// `todo_store` / `pending_sudo` / `sudo_password` / `runner` (Phase 2,
/// Task 1) service the method-aware `todo_update` / `sudo_exec` handlers:
/// the shared todo store, the sudo sub-prompt oneshots, the per-session
/// sudo credential cache, and the `sudo -S` execution seam.
///
/// **Platform policy:** on **macOS** (and other platforms) the listener is
/// NOT started — a no-op handle is returned (fail-closed, ADR 0003).
#[allow(clippy::too_many_arguments)]
pub async fn start_listener(
    placeholder_session_id: String,
    socket_path: &Path,
    anchor_pid: u32,
    sink: Arc<dyn EventSink>,
    pending_bridge: PendingBridge,
    close_tx: &watch::Sender<bool>,
    timeout: Duration,
    subagent: Option<SubagentSpawn>,
    cost_capture: Option<Arc<StdMutex<CostAccumulator>>>,
    todo_store: Arc<TodoStore>,
    pending_sudo: PendingSudo,
    sudo_password: Arc<Mutex<HashMap<String, CachedPassword>>>,
    runner: Arc<dyn SudoRunner>,
) -> Result<BridgeHandle, String> {
    #[cfg(target_os = "linux")]
    {
        let listener = tokio::net::UnixListener::bind(socket_path).map_err(|e| e.to_string())?;
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let session_id = Arc::new(Mutex::new(placeholder_session_id));
        let last_seq = Arc::new(AtomicU64::new(0));
        // Owned (and `'static`) so the spawned accept loop + per-connection
        // waiters can hold it; `watch::Sender::clone` shares the same channel.
        let close_tx: Arc<watch::Sender<bool>> = Arc::new(close_tx.clone());
        let session_id_for_loop = session_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => break,
                    accepted = listener.accept() => {
                        let (stream, _addr) = match accepted {
                            Ok(s) => s,
                            Err(e) => {
                                // A transient accept error (ECONNABORTED —
                                // the peer aborted before the accept;
                                // EMFILE/ENFILE — fd exhaustion) does NOT
                                // mean the listener is dead: log it and
                                // keep accepting. Anything else (e.g.
                                // EBADF — the listener is closed) is
                                // fatal: log it and stop the loop.
                                let fatal = !matches!(
                                    e.raw_os_error(),
                                    Some(
                                        libc::ECONNABORTED
                                        | libc::EMFILE
                                        | libc::ENFILE,
                                    ),
                                );
                                eprintln!(
                                    "bridge: accept failed ({e}) — {}",
                                    if fatal {
                                        "listener is closing, stopping the accept loop"
                                    } else {
                                        "transient error, continuing to accept"
                                    }
                                );
                                if fatal {
                                    break;
                                }
                                // EMFILE/ENFILE: `accept()` fails
                                // IMMEDIATELY while fds are exhausted, so
                                // without a backoff the loop would hot-spin
                                // (syscall → log → retry) at 100% CPU for as
                                // long as the exhaustion persists.
                                if matches!(
                                    e.raw_os_error(),
                                    Some(libc::EMFILE | libc::ENFILE),
                                ) {
                                    tokio::time::sleep(Duration::from_millis(100))
                                        .await;
                                }
                                continue;
                            }
                        };
                        // Peer verification (fail-closed): accept only if
                        // the connecting process is a descendant of the
                        // desktop. A rejection is LOGGED (pid + anchor)
                        // — a silent drop would make a false rejection
                        // look like "the bridge just doesn't work".
                        match peer_pid(&stream) {
                            Some(pid) if pid != 0 => {
                                match is_descendant(pid, anchor_pid, &ProcfsReader) {
                                    DescendantCheck::Accepted(_) => {
                                        let ctx = ConnCtx {
                                            session_id: session_id_for_loop.clone(),
                                            sink: sink.clone(),
                                            pending_bridge: pending_bridge.clone(),
                                            last_seq: last_seq.clone(),
                                            close_tx: close_tx.clone(),
                                            timeout,
                                            subagent: subagent.clone(),
                                            cost_capture: cost_capture.clone(),
                                            todo_store: todo_store.clone(),
                                            pending_sudo: pending_sudo.clone(),
                                            sudo_password: sudo_password.clone(),
                                            runner: runner.clone(),
                                        };
                                        tokio::spawn(handle_connection(
                                            stream,
                                            ctx,
                                        ));
                                    }
                                    DescendantCheck::Rejected(hops) => eprintln!(
                                        "bridge: rejecting peer {pid} (parent chain did not reach the desktop (anchor {anchor_pid}) after {hops} hops) — connection dropped"
                                    ),
                                }
                            }
                            _ => eprintln!(
                                "bridge: rejecting a peer with no verifiable pid (SO_PEERCRED unavailable; anchor {anchor_pid}) — connection dropped"
                            ),
                        }
                    }
                }
            }
        });
        Ok(BridgeHandle {
            stop_tx,
            socket_path: Some(socket_path.to_path_buf()),
            session_id,
        })
    }
    #[cfg(windows)]
    {
        // The stub consumes the (unused-on-Windows) parameters so the
        // no-op build is warning-free; a real implementation uses all of
        // them.
        let _ = (
            socket_path,
            anchor_pid,
            sink,
            pending_bridge,
            close_tx,
            timeout,
            subagent,
            cost_capture,
            todo_store,
            pending_sudo,
            sudo_password,
            runner,
        );
        // TODO(windows): minimal named-pipe listener — create a
        // `\\.\pipe\<name>` instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`
        // (pre-squat detection), wait for a connection, verify the client
        // pid via `GetNamedPipeClientProcessId` (the `is_descendant`
        // Toolhelp walk is a v1 limitation), and hand the handle to
        // `handle_connection`. v1 ships a no-op so the cross-platform build
        // compiles; the bridge is effectively unavailable on Windows until
        // this is implemented.
        let (stop_tx, _stop_rx) = watch::channel(true);
        Ok(BridgeHandle {
            stop_tx,
            socket_path: None,
            session_id: Arc::new(Mutex::new(placeholder_session_id)),
        })
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        // The stub consumes the (unused-on-macOS) parameters so the
        // no-op build is warning-free; a real implementation uses all of
        // them.
        let _ = (
            socket_path,
            anchor_pid,
            sink,
            pending_bridge,
            close_tx,
            timeout,
            subagent,
            cost_capture,
            todo_store,
            pending_sudo,
            sudo_password,
            runner,
        );
        // macOS (and other platforms): the bridge is unavailable —
        // fail-closed (ADR 0003). Return a no-op handle so the caller's
        // lifecycle code is uniform; `available()` is `false`.
        let (stop_tx, _stop_rx) = watch::channel(true);
        Ok(BridgeHandle {
            stop_tx,
            socket_path: None,
            session_id: Arc::new(Mutex::new(placeholder_session_id)),
        })
    }
}

/// Read the peer's pid from a connected Unix socket (`SO_PEERCRED`).
#[cfg(target_os = "linux")]
fn peer_pid(stream: &tokio::net::UnixStream) -> Option<u32> {
    let fd = stream.as_raw_fd();
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc == 0 {
        Some(cred.pid as u32)
    } else {
        None
    }
}

/// Read a process's parent pid (the `PPid` line of `/proc/<pid>/status`).
/// `None` when the process is gone (or the entry is unreadable).
///
/// This is the `#[cfg(test)]` seam: the real reader is [`ProcfsReader`]; a
/// test injects a reader backed by an explicit `pid -> parent` map so it
/// can fabricate the parent chain (a "foreign process" case is real without
/// a double-fork — any test-spawned process is a descendant of the test
/// process and would be accepted).
pub trait ProcReader: Send {
    fn parent_pid(&self, pid: u32) -> Option<u32>;
}

/// The real `/proc` reader.
pub struct ProcfsReader;

impl ProcReader for ProcfsReader {
    fn parent_pid(&self, pid: u32) -> Option<u32> {
        let content = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        content.lines().find_map(|line| {
            line.strip_prefix("PPid:")
                .and_then(|rest| rest.trim().parse::<u32>().ok())
        })
    }
}

/// The outcome of the parent-chain walk: `Accepted(hops)` when the chain
/// reached `anchor`, `Rejected(hops)` otherwise (the `hops` is the number
/// of parent-chain advances before the verdict — logged on rejection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescendantCheck {
    /// The chain reached `anchor` after `hops` parent-chain advances.
    Accepted(u32),
    /// The chain rejected (a dead end, a missing process, or a chain that
    /// never reached `anchor`) after `hops` parent-chain advances.
    Rejected(u32),
}

/// Walk the parent chain from `peer` up to `anchor` (bounded ≤8 hops).
///
/// The 8-hop bound assumes the bridge topology is shallow (desktop →
/// `pi` is 2 levels); a chain deeper than 8 is rejected
/// (fail-closed). The bound is also the CYCLE GUARD: a cycle in the chain
/// (42 → 7 → 42 → …) cannot loop forever.
///
/// A dead end, a missing process, or a chain that never reaches `anchor`
/// rejects (fail-closed).
pub fn is_descendant<R: ProcReader>(peer: u32, anchor: u32, reader: &R) -> DescendantCheck {
    let mut pid = peer;
    let mut hops = 0;
    for _ in 0..8 {
        if pid == anchor {
            return DescendantCheck::Accepted(hops);
        }
        match reader.parent_pid(pid) {
            // `parent == anchor` is allowed even when `parent == 1` (init):
            // when the desktop runs as PID 1 (a container), the anchor IS
            // init, and the walk must be allowed to reach it. A `parent`
            // of 1 that is NOT the anchor is a dead end — a reparented
            // orphan — and rejects.
            Some(parent) if parent > 1 || parent == anchor => {
                pid = parent;
                hops += 1;
            }
            _ => return DescendantCheck::Rejected(hops),
        }
    }
    DescendantCheck::Rejected(hops)
}

/// Per-connection context for [`handle_connection`]: the listener's shared
/// state, cloned once per accepted connection (the `Arc`s are cheap; this
/// bundles what would otherwise be six positional arguments).
#[derive(Clone)]
struct ConnCtx {
    session_id: Arc<Mutex<String>>,
    sink: Arc<dyn EventSink>,
    pending_bridge: PendingBridge,
    last_seq: Arc<AtomicU64>,
    close_tx: Arc<watch::Sender<bool>>,
    timeout: Duration,
    /// The subagent dispatch handle (main only — `Some`); `None` for tests /
    /// non-bridge setups (a `dispatch_subagent` frame on such a listener gets
    /// the unknown-method `error` response).
    subagent: Option<SubagentSpawn>,
    /// Accumulated `cost_update` usage (subagents only; `None` for main).
    cost_capture: Option<Arc<StdMutex<CostAccumulator>>>,
    /// The shared todo store (the `todo_update` handler — Phase 2).
    todo_store: Arc<TodoStore>,
    /// The pending `sudo_exec` sub-prompt oneshots (Phase 2).
    pending_sudo: PendingSudo,
    /// The per-session sudo credential cache (Phase 2; keyed by session id,
    /// cleared in the driver-task teardown alongside the `pending_bridge`
    /// prefix drain — mirroring the suite's `credentialCache` cleared at
    /// every session boundary).
    sudo_password: Arc<Mutex<HashMap<String, CachedPassword>>>,
    /// The `sudo -S` execution seam (Phase 2 — the real runner in
    /// production, a fake in tests).
    runner: Arc<dyn SudoRunner>,
}

/// Handle one bridge connection: read exactly ONE frame (a line — the agent
/// opens a connection per message and destroys it on the first data), then
/// dispatch.
///
/// A **request frame** becomes:
///
/// - a oneshot in `pending_bridge` (registered BEFORE the event is emitted
///   — see (a)), a `bridge-request` event, and a waiter that owns the
///   stream (the agent sends nothing while waiting, so EOF on the stream is
///   the immediate-cancel signal); the waiter writes the response frame —
///   the `result` verbatim — or the terminal `error:"cancelled"` frame,
///   then closes.
///
/// A **push frame** becomes a `seq` drop (drop `seq <= last_seq`), a
/// `bridge-event` event, an ack line, and a close.
async fn handle_connection<S>(mut stream: S, ctx: ConnCtx)
where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
{
    // Read exactly ONE frame (a line) with a hard cap: `read_line` has no
    // length cap of its own — a peer that streams bytes without a newline
    // would make it allocate without bound (blast radius: the entire desktop
    // process). A frame exceeding the cap is dropped (fail-closed, the same
    // posture as an unparseable frame) — and LOGGED: a silent drop is the
    // "the bridge just doesn't work" failure mode.
    const MAX_FRAME_BYTES: usize = 1 << 20; // 1 MiB
    let line = match read_capped_line(&mut stream, MAX_FRAME_BYTES).await {
        CappedRead::Line(l) if l.trim().is_empty() => return, // an empty frame is a no-frame
        CappedRead::Line(l) => l,
        CappedRead::Eof => return, // closed before a frame
        CappedRead::ExceededCap(len) => {
            eprintln!("bridge: dropping a {len}-byte frame over the {MAX_FRAME_BYTES}-byte cap");
            return;
        }
    };
    let frame: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(_) => {
            // A frame that fails JSON parse (or is non-UTF-8 via
            // `from_utf8_lossy`) — log it: a silent drop is the "the bridge
            // just doesn't work" failure mode (the same posture as the
            // `ExceededCap` arm above).
            eprintln!("bridge: dropping an unparseable frame");
            return;
        }
    };
    let ConnCtx {
        session_id,
        sink,
        pending_bridge,
        last_seq,
        close_tx,
        timeout,
        // The subagent dispatch handle (main only — `Some`); the
        // `dispatch_subagent` method is method-aware (NO timeout).
        subagent,
        cost_capture,
        // The Phase 2 method-aware handler state (`todo_update` /
        // `sudo_exec` — the desktop answers these itself).
        todo_store,
        pending_sudo,
        sudo_password,
        runner,
    } = ctx;
    match frame.get("type").and_then(Value::as_str) {
        Some("request") => {
            let id = frame
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let sid = session_id.lock().await.clone();
            let method = frame
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();

            // `dispatch_subagent` is method-aware: NO `pending_bridge`
            // entry (the desktop answers it itself — not the user), NO
            // `bridge-request` UI event (the panel is fed by
            // `subagent-session-started`), NO timeout arm (a subagent task
            // may run minutes; cancellation is the parent close / the
            // agent's EOF). All OTHER methods keep the existing 330 s
            // behavior below.
            if method == "dispatch_subagent" {
                handle_dispatch_subagent(
                    stream,
                    id,
                    sid,
                    frame.get("params"),
                    sink,
                    subagent,
                    close_tx,
                )
                .await;
                return;
            }

            // `todo_update` / `sudo_exec` are method-aware too (Phase 2,
            // Task 1): NO `pending_bridge` entry for the OUTER request
            // (the desktop answers it itself), NO `bridge-request` event
            // for the outer request, NO 330 s timeout arm for it. The
            // `sudo_exec` sub-prompts (`:confirm` / `:password`) reuse the
            // EXISTING modals via `pending_sudo` + `bridge-request` events
            // (the user answers them via the existing
            // `respond_bridge_request`). `todo_update` is fast (no user
            // interaction); `sudo_exec` is long-running (user-paced
            // sub-prompts).
            let source = frame
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("main");
            if method == "todo_update" {
                handle_todo_update(
                    &mut stream,
                    &id,
                    &sid,
                    source,
                    frame.get("params"),
                    &todo_store,
                    &sink,
                    &close_tx,
                )
                .await;
                return;
            }
            if method == "sudo_exec" {
                handle_sudo_exec(
                    &mut stream,
                    &id,
                    &sid,
                    source,
                    frame.get("params"),
                    &sink,
                    &runner,
                    &pending_sudo,
                    &sudo_password,
                    &close_tx,
                )
                .await;
                return;
            }

            // (a) Register the oneshot the user's answer flows through —
            // BEFORE the event is emitted. The UI (and the acp_flow test)
            // calls `respond_bridge_request` the instant it sees the
            // `bridge-request` event; if the entry were not in the map yet,
            // that call would miss and be a no-op, and the request would
            // wait out the full timeout (the agent gets
            // `error:"cancelled"`). Registering first makes the lookup
            // total: by the time the event is observed, the entry is already
            // there. (The same race class `b3c920a` fixed in `permission.rs`.)
            let key = bridge_key(&sid, &id);
            let (tx, rx) = oneshot::channel();
            {
                let mut map = pending_bridge.lock().await;
                map.insert(key.clone(), tx);
            }

            // (b) Tell the UI about the request — now that the answer path
            // is in place, so a `respond_bridge_request` racing the emission
            // always finds the entry. The `session_id` is the CURRENT value
            // — the ACP id after `set_session_id`, the placeholder before.
            let payload = json!({
                "sessionId": sid,
                "requestId": id,
                "method": frame.get("method"),
                "source": frame.get("source"),
                "toolCallId": frame.get("toolCallId"),
                "params": frame.get("params"),
            });
            sink.emit("bridge-request", payload);

            // (c) The waiter owns the stream: the agent sends NOTHING while
            // waiting for the response, so EOF on the stream is the
            // immediate-cancel signal.
            #[derive(Debug)]
            enum WaitResult {
                /// The user answered (the oneshot resolved with the `result`).
                Answered(Value),
                /// Timeout / session close / the agent's close (EOF) / a
                /// drained oneshot — the request is cancelled.
                Cancelled,
            }
            let mut close_rx = close_tx.subscribe();
            let close_already = *close_rx.borrow();
            let outcome = if close_already {
                WaitResult::Cancelled
            } else {
                tokio::select! {
                    r = rx => match r {
                        Ok(result) => WaitResult::Answered(result),
                        Err(_) => WaitResult::Cancelled,
                    },
                    _ = tokio::time::sleep(timeout) => WaitResult::Cancelled,
                    _ = close_rx.changed() => WaitResult::Cancelled,
                    // EOF drain: read until EOF, DISCARDING bytes in 4 KiB
                    // chunks (no `read_to_end` into a growing Vec — a peer
                    // that streams bytes without closing can't make this
                    // allocate without bound).
                    _ = drain_until_eof(&mut stream) => WaitResult::Cancelled,
                }
            };
            let response = match outcome {
                WaitResult::Answered(result) => {
                    json!({ "v": 1, "type": "response", "id": id, "result": result })
                }
                WaitResult::Cancelled => {
                    json!({ "v": 1, "type": "response", "id": id, "error": "cancelled" })
                }
            };
            let data = response.to_string() + "\n";
            let _ = stream.write_all(data.as_bytes()).await;
            let _ = stream.flush().await;
            // Drop the stream → close the connection.
            {
                let mut map = pending_bridge.lock().await;
                map.remove(&key);
            }
        }
        Some("push") => {
            let seq = frame.get("seq").and_then(Value::as_u64).unwrap_or(0);
            // `seq` dedupe: drop frames with `seq <= last_seq` (per
            // listener; `seq` starts at 1). `fetch_max` returns the
            // previous value — delivered iff the previous was lower.
            //
            // In-order assumption: the desktop accepts one connection at a
            // time (the accept loop is single-threaded, and the agent's
            // channel protocol is one connection per message, acked before
            // the next), so two pushes can't be accepted out of order —
            // the drop is correct for re-delivery (a re-delivered `seq` is
            // a duplicate).
            let delivered = seq == 0 || last_seq.fetch_max(seq, Ordering::Relaxed) < seq;
            if delivered {
                let sid = session_id.lock().await.clone();
                // (b) Accumulate the `cost_update` payload (subagents only;
                // `None` for main) — the v1 metrics source. The lock is
                // TOLERANT of a poisoned mutex (`into_inner` — a poisoned
                // capture degrades to its last good state, not a panic: a
                // panic here would chain into the driver task, and on a
                // subagent dispatch into a dropped oneshot — a crash
                // silently reported as a "cancelled" dispatch).
                if frame.get("event").and_then(Value::as_str) == Some("cost_update") {
                    if let Some(cc) = &cost_capture {
                        if let Some(p) = frame.get("payload") {
                            cc.lock().unwrap_or_else(|p| p.into_inner()).add_payload(p);
                        }
                    }
                }
                let payload = json!({
                    "sessionId": sid,
                    "seq": seq,
                    "event": frame.get("event"),
                    "payload": frame.get("payload"),
                });
                sink.emit("bridge-event", payload);
            }
            // Ack + close (the agent destroys on the first data). A dropped
            // duplicate is still acked.
            let _ = stream.write_all(b"ack\n").await;
            let _ = stream.flush().await;
        }
        _ => {
            // Unknown frame type: close (the agent sees the close as a
            // failure and retries / gives up per its own policy).
        }
    }
}

/// The outcome of the `dispatch_subagent` waiter.
#[derive(Debug)]
enum DispatchWait {
    /// The dispatch completed (the output + the metrics, verbatim in the
    /// response's `result`).
    Completed {
        output: String,
        metrics: SubagentMetrics,
    },
    /// The dispatch failed (the error, verbatim in the response's `error`).
    Failed { error: String },
    /// The parent closed / the agent's EOF / a dropped oneshot — the
    /// dispatch is cancelled (the terminal `error:"cancelled"` frame).
    Cancelled,
}

/// Parse + validate the `dispatch_subagent` frame's `params` →
/// `(task, LaunchConfig)`. `None` when the `task` is missing or empty (the
/// caller responds `error: "invalid params"` — a missing `task` must NOT
/// dispatch an empty-prompt subagent session). `tools: []` is treated as
/// `None` (an empty allowlist is malformed, not an allowlist — it would
/// produce `--tools ''`).
fn dispatch_params(params: &Value) -> Option<(String, LaunchConfig)> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if task.trim().is_empty() {
        return None;
    }
    Some((
        task.to_string(),
        LaunchConfig {
            system_prompt: params
                .get("systemPrompt")
                .and_then(Value::as_str)
                .map(str::to_string),
            model: params
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            thinking: params
                .get("thinking")
                .and_then(Value::as_str)
                .map(str::to_string),
            tools: params
                .get("tools")
                .and_then(Value::as_array)
                .and_then(|arr| {
                    let tools: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                    (!tools.is_empty()).then_some(tools)
                }),
        },
    ))
}

/// The `dispatch_subagent` request branch (method-aware — see the
/// `handle_connection` request arm): spawn the subagent dispatch on the
/// worker runtime (the oneshot is the only handoff — the caller never
/// blocks on the worker runtime) and wait for it. The waiter `select!`s on
/// `dispatch_rx` (→ the response frame), the parent close (the `close_tx`
/// flag — a subagent's own listener carries the external close here, so a
/// cancel also cancels its in-flight `ask` waiters), and the agent's EOF
/// (→ `cancel.cancel()` + the terminal `error:"cancelled"` frame). NO
/// timeout arm for this method (the `timeout` is ignored; all other
/// methods keep the existing 330 s behavior).
async fn handle_dispatch_subagent<S>(
    mut stream: S,
    id: String,
    parent_session_id: String,
    params: Option<&Value>,
    sink: Arc<dyn EventSink>,
    subagent: Option<SubagentSpawn>,
    close_tx: Arc<watch::Sender<bool>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
{
    // A `dispatch_subagent` frame on a listener without a subagent manager
    // (tests, non-bridge setups) gets the unknown-method `error` response.
    let Some(spawn) = subagent else {
        let response = json!({ "v": 1, "type": "response", "id": id, "error": "unknown method" });
        let data = response.to_string() + "\n";
        let _ = stream.write_all(data.as_bytes()).await;
        let _ = stream.flush().await;
        return;
    };

    // Validation (BEFORE the spawn): a missing / empty `task` is rejected
    // (`error: "invalid params"`, no dispatch — an empty-prompt subagent
    // session would be a wasted process burning tokens on nothing);
    // `tools: []` is treated as `None` (it would produce `--tools ''`).
    let params = params.cloned().unwrap_or(Value::Null);
    let (task, launch) = match dispatch_params(&params) {
        Some(p) => p,
        None => {
            let response =
                json!({ "v": 1, "type": "response", "id": id, "error": "invalid params" });
            let data = response.to_string() + "\n";
            let _ = stream.write_all(data.as_bytes()).await;
            let _ = stream.flush().await;
            return;
        }
    };
    // `agentName` is the dispatch's agent name (the `subagent-session-
    // started` payload) — NOT part of the launch config.
    let agent_name = params
        .get("agentName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Pre-check (BEFORE the spawn): a parent that is ALREADY closing must
    // NOT spawn a subagent at all — the dispatch would be ORPHANED (no
    // code path could close it: `SubagentCancel` has no `Drop`, and the
    // sender stays alive via `LiveSession.close_tx`): it would run until
    // its own prompt settles / app exit — a live agent process burning
    // tokens whose result nobody consumes. (The post-spawn
    // `close_rx.changed()` arm below is the belt-and-braces race guard for
    // a flag that flips AFTER the check — it cancels the spawned dispatch.)
    let mut close_rx = close_tx.subscribe();
    let close_already = *close_rx.borrow();
    let outcome = if close_already {
        DispatchWait::Cancelled
    } else {
        // The desktop spawns the dispatch (the whole lifecycle runs on the
        // worker runtime; `parent_session_id` is the parent's ACP id — the
        // `ConnCtx` session-id state after the parent's `set_session_id`).
        let (dispatch_rx, cancel) = spawn.manager.dispatch(
            &parent_session_id,
            &spawn.parent_cwd,
            &spawn.parent_agent_id,
            agent_name,
            launch,
            task,
            &sink,
        );
        tokio::select! {
            r = dispatch_rx => match r {
                Ok(SubagentOutcome::Completed { output, metrics }) => {
                    DispatchWait::Completed { output, metrics }
                }
                Ok(SubagentOutcome::Failed { error }) => DispatchWait::Failed { error },
                // The worker task vanished without resolving (app exit —
                // a panicked task is logged by the worker runtime,
                // unwind profiles only; release builds abort the process
                // on a panic by design): the dispatch is gone — cancel.
                Err(_) => DispatchWait::Cancelled,
            },
            // The parent closed AFTER the pre-check passed (the race the
            // pre-check can't cover — the flag flipped between the check
            // and here): cancel the spawned dispatch (it would otherwise
            // be orphaned — no code path could close it).
            _ = close_rx.changed() => {
                cancel.cancel();
                DispatchWait::Cancelled
            },
            // EOF drain: read until EOF, DISCARDING bytes in 4 KiB chunks
            // (no `read_to_end` into a growing Vec — a peer that streams
            // bytes without closing can't make this allocate without
            // bound).
            _ = drain_until_eof(&mut stream) => {
                cancel.cancel();
                DispatchWait::Cancelled
            }
        }
    };
    let response = match outcome {
        DispatchWait::Completed { output, metrics } => {
            json!({
                "v": 1, "type": "response", "id": id,
                "result": {
                    "output": output,
                    "metrics": {
                        "inputTokens": metrics.input_tokens,
                        "outputTokens": metrics.output_tokens,
                        "cost": metrics.cost,
                        "durationMs": metrics.duration_ms,
                    }
                }
            })
        }
        DispatchWait::Failed { error } => {
            json!({ "v": 1, "type": "response", "id": id, "error": error })
        }
        DispatchWait::Cancelled => {
            json!({ "v": 1, "type": "response", "id": id, "error": "cancelled" })
        }
    };
    let data = response.to_string() + "\n";
    let _ = stream.write_all(data.as_bytes()).await;
    let _ = stream.flush().await;
    // Drop the stream → close the connection.
}

/// The `todo_update` request branch (method-aware — the desktop answers
/// it itself; see the `handle_connection` request arm): parse the suite's
/// `ManageTodoListParams` (`{operation: "write"|"read", todoList?: [...]}`),
/// serve it from the shared [`TodoStore`], and — on a `write` — emit the
/// `todos_update` push the EXISTING `useBridge.applyTodoUpdate` /
/// `TodoBoardPanel` consume (the `BridgeEventPayload` shape — `sessionId`
/// is MANDATORY: `App.tsx` calls `applyTodoUpdate(payload.sessionId,
/// payload.payload)`, so a missing `sessionId` would land the todos under
/// `todos[undefined]` and the board (keyed by `activeSessionId`) would
/// never update). The result shapes/text mirror the suite's
/// `packages/todo/src/tool.ts` VERBATIM.
///
/// A cancelled session aborts the handler (the work is fast — a close-flag
/// check before the store write / push emit, and the response write races
/// the close flag: a write to a closed stream fails silently).
#[allow(clippy::too_many_arguments)]
async fn handle_todo_update<S>(
    stream: &mut S,
    id: &str,
    sid: &str,
    source: &str,
    params: Option<&Value>,
    todo_store: &Arc<TodoStore>,
    sink: &Arc<dyn EventSink>,
    close_tx: &Arc<watch::Sender<bool>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let params = params.cloned().unwrap_or(Value::Null);
    let operation = params
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");

    // A cancelled session aborts the handler (BEFORE the store write / the
    // push emit — a cancelled session's agent is gone).
    let mut close_rx = close_tx.subscribe();
    if *close_rx.borrow() {
        return;
    }

    let response = if operation == "read" {
        let todos = todo_store.get(sid);
        // The suite's read text (`tool.ts:75-84`): `JSON.stringify(todos,
        // null, 2)` when non-empty, else the "No todos" text.
        let text = if todos.is_empty() {
            "No todos. Use write operation to create a todo list.".to_string()
        } else {
            serde_json::to_string_pretty(&todos).unwrap_or_default()
        };
        json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": text }],
                "details": { "operation": "read", "todos": todos }
            }
        })
    } else {
        // write (the default for a missing/unknown operation — the suite's
        // schema requires `operation` in {"write","read"}, so anything
        // else is a malformed write).
        match params.get("todoList").and_then(Value::as_array) {
            None => {
                // The suite's text + flag (`tool.ts:89-94`): `todos` is
                // the CURRENT list, `error` is the marker.
                let current = todo_store.get(sid);
                json!({
                    "v": 1, "type": "response", "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": "Error: todoList is required for write operation." }],
                        "details": { "operation": "write", "todos": current, "error": "todoList required" },
                        "isError": true
                    }
                })
            }
            Some(arr) => {
                // The suite's `state.validate` (`state-manager.ts:36-59`): the
                // TypeBox schema lets an empty/whitespace `content` through, so
                // the validation is the guard here. (The `status` is checked
                // against the wire casing; a non-string `description` is an
                // error — the schema would have rejected it, but a
                // hand-rolled frame must not blow up the handler.)
                let valid_statuses = ["pending", "in_progress", "completed"];
                let mut errors: Vec<String> = Vec::new();
                let mut items: Vec<TodoItem> = Vec::new();
                for (i, item) in arr.iter().enumerate() {
                    let prefix = format!("Item {}", i + 1);
                    if item.is_null() {
                        errors.push(format!("{prefix}: undefined item"));
                        continue;
                    }
                    let content = item.get("content").and_then(Value::as_str);
                    let status = item.get("status").and_then(Value::as_str);
                    if content.is_none_or(|c| c.trim().is_empty()) {
                        errors.push(format!("{prefix}: missing or invalid 'content'"));
                    }
                    if !status.is_some_and(|s| valid_statuses.contains(&s)) {
                        errors.push(format!(
                            "{prefix}: 'status' must be one of: pending, in_progress, completed"
                        ));
                    }
                    if item.get("description").is_some()
                        && item.get("description").and_then(Value::as_str).is_none()
                    {
                        errors.push(format!("{prefix}: 'description' must be a string"));
                    }
                    if let (Some(content), Some(status)) = (content, status) {
                        items.push(TodoItem {
                            content: content.to_string(),
                            status: match status {
                                "pending" => TodoStatus::Pending,
                                "in_progress" => TodoStatus::InProgress,
                                _ => TodoStatus::Completed,
                            },
                            description: item
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        });
                    }
                }
                if !errors.is_empty() {
                    // The suite's validation text (`tool.ts:84-93`): the errors
                    // prefixed `  - ` and joined with newlines; `details.error`
                    // is the errors joined with `; `.
                    let text = format!(
                        "Validation failed:\n{}",
                        errors
                            .iter()
                            .map(|e| format!("  - {e}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    let current = todo_store.get(sid);
                    json!({
                        "v": 1, "type": "response", "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": text }],
                            "details": { "operation": "write", "todos": current, "error": errors.join("; ") },
                            "isError": true
                        }
                    })
                } else {
                    let stored = todo_store.set(sid, items.clone());
                    // Emit the `todos_update` push via the EXISTING
                    // `bridge-event` sink path (the `BridgeEventPayload` shape
                    // — the EXISTING `useBridge.applyTodoUpdate` +
                    // `TodoBoardPanel` consume it; do NOT invent a new
                    // `todo-update` event).
                    sink.emit(
                        "bridge-event",
                        json!({
                            "sessionId": sid,
                            "seq": 0,
                            "event": "todos_update",
                            "payload": { "source": source, "todos": stored }
                        }),
                    );
                    // The suite's write text (`tool.ts:138`): the stats from
                    // the stored items, + a warning appended when the list has
                    // <3 items.
                    let completed = stored
                        .iter()
                        .filter(|t| t.status == TodoStatus::Completed)
                        .count();
                    let total = stored.len();
                    let mut message = format!(
                    "Todos have been modified all. {completed}/{total} completed. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable."
                );
                    if stored.len() < 3 {
                        message.push_str(
                        "\n\nWarning: Small todo list (<3 items). This task might not need a todo list.",
                    );
                    }
                    json!({
                        "v": 1, "type": "response", "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": message }],
                            "details": { "operation": "write", "todos": stored }
                        }
                    })
                }
            }
        }
    };

    // Write the response, racing the close flag (a cancelled session
    // aborts the write — the agent is gone; a write to a closed stream
    // fails silently). `drain_until_eof` is NOT a separate arm: it and
    // the write would both need `&mut stream` in the same `select!` (a
    // double mutable borrow) — the close flag is the driver's cancel
    // path and the write itself is bounded (it cannot block on a dead
    // stream: `write_all` on a closed stream errors immediately).
    let data = response.to_string() + "\n";
    tokio::select! {
        _ = close_rx.changed() => {}
        _ = async {
            let _ = stream.write_all(data.as_bytes()).await;
            let _ = stream.flush().await;
        } => {}
    }
}

/// The `sudo_exec` request branch (method-aware — the desktop orchestrates
/// the confirm → password → run-sudo flow; the user answers the sub-prompts
/// via the EXISTING `respond_bridge_request`). Ports the suite's policy
/// VERBATIM (`packages/sudo/src/{tool,cache,config,guard}.ts`):
///
/// - **Confirm sub-prompt** (reuses the existing `SudoConfirmModal`): a
///   oneshot in `pending_sudo` keyed `"{sid}/{id}:confirm"` (REGISTERED
///   BEFORE the `bridge-request` event — the modals echo the `requestId`
///   VERBATIM into `respondBridgeRequest`, which composes
///   `bridge_key(sid, requestId)` = `"{sid}/{id}:confirm"`), a 330 s cap
///   (user-paced), and the suite's "not confirmed" result on a cancel /
///   timeout / session close (NO password prompt, NO execution).
/// - **Password sub-prompt — CACHE-HIT FIRST** (the suite's
///   `credentialCache.get()`, `tool.ts:442-456`): a valid entry (TTL 15
///   min, `config.ts`) skips the prompt entirely; on expiry the entry is
///   dropped and the `SudoPasswordModal` prompt fires (an EMPTY `""`
///   password is a cancel — the `BridgeResponseDto` convention).
/// - **Run** via the INJECTABLE [`SudoRunner`] (the suite's `SudoSpawner`
///   seam — tests use a fake runner, no real `sudo` in `cargo test`):
///   `buildSudoArgv` (quoted-arg split, leading-`sudo` strip, NO `-p`),
///   the password on stdin, the timeout from `params.timeoutMs` (absent =
///   the 120 s default — the RUN is NOT under the 330 s sub-prompt cap).
/// - **Auth-failure handling** (`tool.ts:225-228`): the EXACT two-condition
///   signature (`[sudo] password for` AND `incorrect password`, both
///   case-insensitive) clears the cached password (a mistyped password
///   must not stick for the TTL). SCOPED OUT (fast path only): the
///   two-strike ambiguous-failure rule with the `sudo -n -v` `authProbe`
///   and `failStreak`, and the `exitCode` conventions (-1/124/130/137
///   exclusions — a future task can add the `authProbe`).
/// - **Secret scrubbing** (the suite's `scrubSecret`, applied PER STREAM):
///   the password must not appear in the captured output that is persisted
///   in tool results.
#[allow(clippy::too_many_arguments)]
async fn handle_sudo_exec<S>(
    stream: &mut S,
    id: &str,
    sid: &str,
    source: &str,
    params: Option<&Value>,
    sink: &Arc<dyn EventSink>,
    runner: &Arc<dyn SudoRunner>,
    pending_sudo: &PendingSudo,
    sudo_password: &Arc<Mutex<HashMap<String, CachedPassword>>>,
    close_tx: &Arc<watch::Sender<bool>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let params = params.cloned().unwrap_or(Value::Null);
    let command = params
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    let mut close_rx = close_tx.subscribe();

    // The suite's empty-command rejection (`tool.ts:404-411`) — BEFORE any
    // sub-prompt (no runner call, no modal).
    if command.is_empty() {
        let response = json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": "The command is empty — provide the exact command to run with elevated privileges." }],
                "details": { "command": command, "reason": reason, "exitCode": -1, "stdout": "", "stderr": "", "error": "empty command" },
                "isError": true
            }
        });
        write_sudo_response(stream, &response, &mut close_rx).await;
        return;
    }

    // ── the confirm sub-prompt (BEFORE any credential is acquired) ──
    // Register the oneshot BEFORE the event is emitted (the UI may answer
    // the instant it sees the event — the same race class the generic path
    // guards, `b3c920a`). The `requestId` is the DERIVED `"{id}:confirm"`
    // (NOT the bare frame `id` — a bare id would miss the
    // `respond_bridge_request` lookup and silently time out at 330 s).
    let confirm_key = bridge_key(sid, &format!("{id}:confirm"));
    let (confirm_tx, confirm_rx) = oneshot::channel();
    {
        let mut map = pending_sudo.lock().await;
        map.insert(confirm_key.clone(), confirm_tx);
    }
    sink.emit(
        "bridge-request",
        json!({
            "sessionId": sid,
            "requestId": format!("{id}:confirm"),
            "method": "confirm",
            "source": source,
            "toolCallId": Value::Null,
            "params": { "command": command, "reason": reason }
        }),
    );
    let close_already = *close_rx.borrow();
    let r: Result<Value, ()> = if close_already {
        Err(())
    } else {
        // `Result<Value, ()>`: the cancel arms (timeout / close / EOF) all
        // map to `Err(())` — a cancel is a `false` confirm.
        tokio::select! {
            r = confirm_rx => r.map_err(|_| ()),
            _ = tokio::time::sleep(DEFAULT_BRIDGE_TIMEOUT) => Err(()),
            _ = close_rx.changed() => Err(()),
            // EOF drain: the agent hung up — cancel (the agent holds the
            // connection open while waiting, so EOF is the immediate-cancel
            // signal, exactly like the generic path).
            _ = drain_until_eof(stream) => Err(()),
        }
    };
    // EVERY exit path (answered, timeout, close, EOF — the `close_already`
    // pre-check path INCLUDED: the entry was inserted BEFORE the pre-check,
    // so it must be removed there too, mirroring the password block) removes
    // the entry (no leaked dead-receiver entries — mirrors the
    // `pending_bridge` remove in the generic path). A cancel resolves to
    // `false` (a missing/`false` `confirmed` is a cancel).
    pending_sudo.lock().await.remove(&confirm_key);
    let confirmed = r
        .ok()
        .and_then(|v| v.get("confirmed").and_then(Value::as_bool))
        .unwrap_or(false);
    if !confirmed {
        // The suite's result (`tool.ts:429-434`) — on a user cancel, a
        // timeout, OR a session close: no password prompt, no execution.
        let response = json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": "Command not confirmed — not executed, and no password was requested." }],
                "details": { "command": command, "reason": reason, "exitCode": -1, "stdout": "", "stderr": "", "error": "command not confirmed" },
                "isError": true
            }
        });
        write_sudo_response(stream, &response, &mut close_rx).await;
        return;
    }

    // ── the password (CACHE-HIT FIRST — the suite's `credentialCache.get()`,
    // `tool.ts:442-456`) ── a valid entry (TTL in the future) skips the
    // prompt entirely (no `:password` event, no re-prompt); on expiry the
    // entry is dropped and the prompt fires.
    let cached: Option<String> = {
        let mut cache = sudo_password.lock().await;
        if let Some(entry) = cache.get(sid) {
            if entry.expires_at > std::time::Instant::now() {
                Some(entry.password.clone())
            } else {
                cache.remove(sid);
                None
            }
        } else {
            None
        }
    };
    let mut prompted = false;
    let mut prompted_value: Option<Value> = None;
    if cached.is_none() {
        let password_key = bridge_key(sid, &format!("{id}:password"));
        let (pw_tx, pw_rx) = oneshot::channel();
        {
            let mut map = pending_sudo.lock().await;
            map.insert(password_key.clone(), pw_tx);
        }
        sink.emit(
            "bridge-request",
            json!({
                "sessionId": sid,
                "requestId": format!("{id}:password"),
                "method": "password",
                "source": source,
                "toolCallId": Value::Null,
                "params": { "command": command, "reason": reason }
            }),
        );
        let close_already = *close_rx.borrow();
        let r: Result<Value, ()> = if close_already {
            Err(())
        } else {
            tokio::select! {
                r = pw_rx => r.map_err(|_| ()),
                _ = tokio::time::sleep(DEFAULT_BRIDGE_TIMEOUT) => Err(()),
                _ = close_rx.changed() => Err(()),
                _ = drain_until_eof(stream) => Err(()),
            }
        };
        pending_sudo.lock().await.remove(&password_key);
        prompted = true;
        prompted_value = r.ok();
    }
    let password = cached.clone().or_else(|| {
        prompted_value
            .as_ref()
            .and_then(|v| v.get("password").and_then(Value::as_str))
            .map(str::to_string)
            .filter(|p| !p.is_empty()) // an EMPTY `""` password is a cancel
    });
    let Some(password) = password else {
        // Cancelled (an empty password, a timeout, a session close, or an
        // EOF) — the suite's result (`tool.ts:437-447`): nothing was
        // cached, nothing was run.
        let response = json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": "Password entry cancelled — not executed, and nothing was cached." }],
                "details": { "command": command, "reason": reason, "exitCode": -1, "stdout": "", "stderr": "", "error": "password entry cancelled" },
                "isError": true
            }
        });
        write_sudo_response(stream, &response, &mut close_rx).await;
        return;
    };
    if prompted {
        // Cache the password with the suite's TTL (15 min — `config.ts`;
        // the per-session slot, cleared in the driver-task teardown).
        let mut cache = sudo_password.lock().await;
        cache.insert(
            sid.to_string(),
            CachedPassword {
                password: password.clone(),
                expires_at: std::time::Instant::now() + SUDO_TTL,
            },
        );
    }

    // ── run sudo on the DESKTOP host via the INJECTABLE `SudoRunner` ──
    // `timeout` = `params.timeoutMs` (or the 120 s default) — the RUN is
    // bounded by the caller's `timeoutMs`, NOT by the 330 s sub-prompt cap.
    let timeout_ms = params
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .filter(|ms| *ms > 0)
        .unwrap_or(SUDO_DEFAULT_TIMEOUT_MS);
    let argv = build_sudo_argv(&command);
    let run = runner
        .run(argv, password.clone(), Duration::from_millis(timeout_ms))
        .await;

    // Secret scrubbing (the suite's `scrubSecret`, applied PER STREAM — the
    // password must not appear in the captured output that is persisted in
    // tool results). The auth-failure check runs on the RAW stderr (the
    // suite computes it inside `runSudo`, before any scrub).
    let auth_failed = is_sudo_auth_failure(&run.stderr);
    let stdout = scrub_secret(&run.stdout, &password);
    let stderr = scrub_secret(&run.stderr, &password);

    let response = if let Some(error) = &run.error {
        // Process-level failure (spawn `error`, spawner throw, fatal stdin
        // write) — the suite's `tool.ts:483-490`: a clean tool failure with
        // `exitCode: -1` + `error`, NEVER an auth failure — the cached
        // credential is KEPT (a transport glitch is transient relative to
        // the credential itself).
        json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": format!("failed to run privileged command: {error}") }],
                "details": { "command": command, "reason": reason, "exitCode": run.exit_code, "stdout": stdout, "stderr": stderr, "error": error },
                "isError": true
            }
        })
    } else if auth_failed {
        // The suite's auth-failure handling (`tool.ts:225-228`): CLEAR the
        // cached password (a mistyped password must not stick for the TTL —
        // the next `sudo_exec` re-prompts) and report the failure.
        sudo_password.lock().await.remove(sid);
        json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": format!("sudo authentication failed (incorrect password) — exit code {}. The in-memory credential cache was cleared; the user will be re-prompted on the next attempt.", run.exit_code) }],
                "details": { "command": command, "reason": reason, "exitCode": run.exit_code, "stdout": stdout, "stderr": stderr, "error": "authentication failed" },
                "isError": true
            }
        })
    } else if run.timed_out {
        // The suite's timeout result (`tool.ts:493-504`): the (partial)
        // stdout + the timeout error + `isError`.
        json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": stdout }],
                "details": { "command": command, "reason": reason, "exitCode": run.exit_code, "stdout": stdout, "stderr": stderr, "error": format!("timed out after {timeout_ms}ms — command was killed") },
                "isError": true
            }
        })
    } else if run.exit_code == 0 {
        // Success: the SCRUBBED stdout VERBATIM (`content[0].text` is what
        // the LLM sees — NOT a summary; `details` is not LLM-visible). NO
        // `isError`, NO `error`.
        json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": stdout }],
                "details": { "command": command, "reason": reason, "exitCode": run.exit_code, "stdout": stdout, "stderr": stderr }
            }
        })
    } else {
        // A non-zero exit (no auth markers, no timeout): `isError` + the
        // suite's failure text. (The suite's two-strike ambiguous-failure
        // rule with the `sudo -n -v` `authProbe` is SCOPED OUT — fast path
        // only, documented above.)
        json!({
            "v": 1, "type": "response", "id": id,
            "result": {
                "content": [{ "type": "text", "text": stdout }],
                "details": { "command": command, "reason": reason, "exitCode": run.exit_code, "stdout": stdout, "stderr": stderr, "error": format!("command failed with exit code {}", run.exit_code) },
                "isError": true
            }
        })
    };
    write_sudo_response(stream, &response, &mut close_rx).await;
}

/// Write a `sudo_exec` response frame, racing the write against the close
/// flag (the SAME posture as `handle_todo_update`): a dead-but-open peer
/// with a full socket buffer would otherwise park the `handle_sudo_exec`
/// task on the write past session close. A write to a closed stream fails
/// silently (the agent is gone). `drain_until_eof` is NOT a separate arm:
/// it and the write would both need `&mut stream` in the same `select!`
/// (a double mutable borrow) — the close flag is the driver's cancel path.
async fn write_sudo_response<S>(
    stream: &mut S,
    response: &Value,
    close_rx: &mut watch::Receiver<bool>,
) where
    S: AsyncWrite + Unpin,
{
    // The flag was ALREADY set at subscribe time: a `changed()` on a fresh
    // receiver would never fire (the value has not changed since the
    // subscribe), so skip the write — the agent is gone.
    if *close_rx.borrow() {
        return;
    }
    let data = response.to_string() + "\n";
    tokio::select! {
        _ = close_rx.changed() => {}
        _ = async {
            let _ = stream.write_all(data.as_bytes()).await;
            let _ = stream.flush().await;
        } => {}
    }
}

/// The outcome of a capped frame read. The enum makes the cap-exceeded
/// case UNREPRESENTABLE as a silent `None` — every caller must
/// acknowledge the truncation case (and log it).
#[derive(Debug, PartialEq)]
enum CappedRead {
    /// A complete frame (a line, `\n`-terminated).
    Line(String),
    /// The connection closed before a complete frame.
    Eof,
    /// The frame exceeded the cap (dropped — `len` is the byte count at
    /// the point the cap was tripped).
    ExceededCap(usize),
}

/// Read one frame (a line) with a hard byte cap (the frame read is bounded
/// — a peer that streams bytes without a newline can't make this allocate
/// without bound). `Eof` when the connection closed before a complete
/// frame; `ExceededCap` when the frame exceeded the cap (fail-closed, the
/// same posture as an unparseable frame — but distinguishable, so the drop
/// is logged rather than silent).
async fn read_capped_line<S: AsyncRead + Unpin>(stream: &mut S, cap: usize) -> CappedRead {
    let mut frame: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            Err(_) => return CappedRead::Eof,
        };
        if n == 0 {
            return CappedRead::Eof; // closed before a complete frame
        }
        frame.extend_from_slice(&chunk[..n]);
        if frame.len() > cap {
            return CappedRead::ExceededCap(frame.len()); // the frame exceeded the cap — fail closed
        }
        if frame.ends_with(b"\n") {
            return CappedRead::Line(String::from_utf8_lossy(&frame).into_owned());
        }
    }
}

/// Read until EOF, discarding bytes in 4 KiB chunks (a bounded drain — no
/// `read_to_end` into a growing `Vec`).
async fn drain_until_eof<R: AsyncRead + Unpin>(r: &mut R) {
    let mut buf = [0u8; 4096];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use std::task::{Context, Poll};
    use tokio::io::AsyncBufReadExt;
    use tokio::io::ReadBuf;

    /// An `EventSink` that captures emissions (the test double for
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

    /// A `ProcReader` backed by an explicit `pid -> parent` map (the
    /// `#[cfg(test)]` seam — fabricates the parent chain, so the "foreign
    /// process" case is real without a double-fork).
    struct MapReader(StdMutex<HashMap<u32, u32>>);

    impl MapReader {
        fn new(pairs: &[(u32, u32)]) -> Self {
            Self(StdMutex::new(pairs.iter().cloned().collect()))
        }
    }

    impl ProcReader for MapReader {
        fn parent_pid(&self, pid: u32) -> Option<u32> {
            self.0.lock().unwrap().get(&pid).copied()
        }
    }

    #[test]
    fn bridge_key_carries_the_trailing_slash_prefix() {
        assert_eq!(bridge_key("s1", "r1"), "s1/r1");
        assert_eq!(session_key_prefix("s1"), "s1/");
        // Closing "s1" must not drain "s10"'s pending entry (the same
        // trailing-slash-prefix invariant as `permission`).
        let mut map: HashMap<String, oneshot::Sender<Value>> = HashMap::new();
        let (tx_s1, _rx_s1) = oneshot::channel();
        let (tx_s10, _rx_s10) = oneshot::channel();
        map.insert(bridge_key("s1", "r1"), tx_s1);
        map.insert(bridge_key("s10", "r1"), tx_s10);
        map.retain(|key, _| !key.starts_with(&session_key_prefix("s1")));
        assert!(
            map.contains_key(&bridge_key("s10", "r1")),
            "closing s1 must not drain s10's pending entry"
        );
        assert!(
            !map.contains_key(&bridge_key("s1", "r1")),
            "s1's own pending entry must be drained"
        );
    }

    #[test]
    fn is_descendant_accepts_a_chain_that_reaches_the_anchor() {
        // peer 42 → 7 → anchor 1000 (two `/proc` reads).
        let reader = MapReader::new(&[(42, 7), (7, 1000)]);
        assert_eq!(
            is_descendant(42, 1000, &reader),
            DescendantCheck::Accepted(2)
        );
    }

    #[test]
    fn is_descendant_reaches_an_anchor_that_is_init() {
        // The desktop runs as PID 1 (a container): the anchor IS init,
        // so the walk must be allowed to reach pid 1 (42 → 7 → 1).
        let reader = MapReader::new(&[(42, 7), (7, 1)]);
        assert_eq!(is_descendant(42, 1, &reader), DescendantCheck::Accepted(2));
    }

    #[test]
    fn is_descendant_rejects_a_chain_that_never_reaches_the_anchor() {
        // A double-forked orphan: reparented to init — its chain
        // (400 → 2 → 1) never reaches the anchor (which is NOT init):
        // one `/proc` read (400 → 2), then `parent == 1` is a dead end.
        let reader = MapReader::new(&[(400, 2), (2, 1)]);
        assert_eq!(
            is_descendant(400, 1000, &reader),
            DescendantCheck::Rejected(1)
        );
    }

    #[test]
    fn is_descendant_rejects_a_cycle_bounded_by_eight_hops() {
        // A cycle (42 → 7 → 42 → …) must terminate (≤8 hops) and reject
        // (eight `/proc` reads, no verdict).
        let reader = MapReader::new(&[(42, 7), (7, 42)]);
        assert_eq!(
            is_descendant(42, 1000, &reader),
            DescendantCheck::Rejected(8)
        );
    }

    /// A `tokio` `AsyncRead` that returns a fixed byte buffer once, then
    /// EOF (a closed connection) — the test double for a peer that sends
    /// a bounded amount of data and hangs up.
    struct ByteStream(Vec<u8>);

    impl AsyncRead for ByteStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<Result<(), io::Error>> {
            let n = self.0.len().min(buf.capacity());
            if n == 0 {
                return Poll::Ready(Ok(())); // EOF
            }
            buf.put_slice(&self.0[..n]);
            self.0.drain(..n);
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn read_capped_line_returns_the_line_when_under_the_cap() {
        let mut stream = ByteStream(b"hello\n".to_vec());
        assert_eq!(
            read_capped_line(&mut stream, 1024).await,
            CappedRead::Line("hello\n".to_string())
        );
    }

    #[tokio::test]
    async fn read_capped_line_returns_eof_when_the_peer_closes_before_a_frame() {
        let mut stream = ByteStream(Vec::new()); // immediate EOF
        assert_eq!(read_capped_line(&mut stream, 1024).await, CappedRead::Eof);
    }

    #[tokio::test]
    async fn read_capped_line_returns_exceeded_cap_when_the_frame_exceeds_it() {
        // A frame larger than the cap, with NO newline: the cap is tripped
        // (distinguishable from an EOF — the drop is representable, so the
        // caller must log it rather than fail silently).
        let big = vec![b'x'; 1024 + 4096];
        let mut stream = ByteStream(big);
        match read_capped_line(&mut stream, 1024).await {
            CappedRead::ExceededCap(len) => assert!(len > 1024),
            other => panic!("expected ExceededCap, got {other:?}"),
        }
    }

    /// Bind a unique Unix socket, spawn `handle_connection` as the server
    /// (the agent opens a new connection per message and destroys it on the
    /// first data — one frame per connection), and return the path + the
    /// server task. A tokio-native `UnixListener`/`UnixStream::connect` is
    /// used (NOT `UnixStream::from_std` on a blocking fd, which tokio 1.53
    /// refuses to register). `subagent` (the `dispatch_subagent` dispatch
    /// handle) is `None` for the non-dispatch tests.
    async fn spawn_server(
        session_id: String,
        sink: Arc<CapturingSink>,
        pending: PendingBridge,
        last_seq: Arc<AtomicU64>,
        close_tx: Arc<watch::Sender<bool>>,
        timeout: Duration,
        subagent: Option<crate::agent::session::SubagentSpawn>,
    ) -> (std::path::PathBuf, tokio::task::JoinHandle<()>) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "archimedes-bridge-test-{}-{}.sock",
            std::process::id(),
            n
        ));
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ctx = ConnCtx {
                session_id: Arc::new(Mutex::new(session_id)),
                sink,
                pending_bridge: pending,
                last_seq,
                close_tx,
                timeout,
                subagent,
                cost_capture: None,
                todo_store: Arc::new(TodoStore::new()),
                pending_sudo: Arc::new(Mutex::new(HashMap::new())),
                sudo_password: Arc::new(Mutex::new(HashMap::new())),
                runner: Arc::new(FakeRunner::default()),
            };
            handle_connection(stream, ctx).await
        });
        (path, server_task)
    }

    /// Connect a client to `path` (tokio-native — no `from_std` on a
    /// blocking fd).
    async fn connect_client(
        path: &std::path::Path,
    ) -> tokio::io::BufReader<tokio::net::UnixStream> {
        let inner = tokio::net::UnixStream::connect(path)
            .await
            .expect("connect");
        tokio::io::BufReader::new(inner)
    }

    #[tokio::test]
    async fn push_frames_are_seq_deduped() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let last_seq = Arc::new(AtomicU64::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let close_tx = Arc::new(close_tx);

        let frame = json!({ "v": 1, "type": "push", "seq": 1, "event": "session", "payload": {} });

        // First connection: push `seq: 1` → delivered.
        let (path, server_task) = spawn_server(
            "sess".to_string(),
            sink.clone(),
            pending.clone(),
            last_seq.clone(),
            close_tx.clone(),
            Duration::from_secs(30),
            None,
        )
        .await;
        let mut client = connect_client(&path).await;
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;
        let mut ack = String::new();
        let _ = client.read_line(&mut ack).await;
        server_task.await.expect("server task");
        let _ = std::fs::remove_file(&path);
        assert_eq!(ack, "ack\n", "a delivered push is acked");
        assert_eq!(
            sink.events_named("bridge-event").len(),
            1,
            "a delivered push emits one bridge-event"
        );

        // Second connection (one frame per connection): the same `seq: 1`
        // → dropped (`seq <= last_seq`), but still acked.
        let (path, server_task) = spawn_server(
            "sess".to_string(),
            sink.clone(),
            pending.clone(),
            last_seq.clone(),
            close_tx.clone(),
            Duration::from_secs(30),
            None,
        )
        .await;
        let mut client = connect_client(&path).await;
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;
        let mut ack = String::new();
        let _ = client.read_line(&mut ack).await;
        server_task.await.expect("server task");
        let _ = std::fs::remove_file(&path);
        assert_eq!(ack, "ack\n", "a dropped duplicate is still acked");
        assert_eq!(
            sink.events_named("bridge-event").len(),
            1,
            "a duplicate seq emits no second bridge-event"
        );
    }

    #[tokio::test]
    async fn a_request_frame_round_trips_the_response_verbatim() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let last_seq = Arc::new(AtomicU64::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let close_tx = Arc::new(close_tx);

        let (path, server_task) = spawn_server(
            "sess-1".to_string(),
            sink.clone(),
            pending.clone(),
            last_seq,
            close_tx,
            Duration::from_secs(30),
            None,
        )
        .await;
        let mut client = connect_client(&path).await;
        let frame = json!({
            "v": 1, "type": "request", "id": "r-1", "method": "confirm",
            "source": "main", "params": { "command": "ls", "reason": "test" }
        });
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;

        // Wait for the pending entry to be registered.
        let mut interval = tokio::time::interval(Duration::from_millis(5));
        loop {
            interval.tick().await;
            if pending
                .lock()
                .await
                .contains_key(&bridge_key("sess-1", "r-1"))
            {
                break;
            }
        }

        // The user answers — the oneshot carries the `result` verbatim.
        let result = json!({ "confirmed": true });
        let sender = pending
            .lock()
            .await
            .remove(&bridge_key("sess-1", "r-1"))
            .expect("pending entry");
        let _ = sender.send(result);

        // The client reads the response frame (one frame per connection —
        // the agent destroys on the first data).
        let mut response_line = String::new();
        let _ = client.read_line(&mut response_line).await;
        server_task.await.expect("server task");

        let response: Value = serde_json::from_str(response_line.trim()).expect("response frame");
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response")
        );
        assert_eq!(response.get("id").and_then(Value::as_str), Some("r-1"));
        // The `result` verbatim — no wrapper, no double-nesting.
        assert_eq!(response.get("result"), Some(&json!({ "confirmed": true })));
        assert!(response.get("error").is_none());
        let _ = std::fs::remove_file(&path);

        // The `bridge-request` event carries the session id.
        let requests = sink.events_named("bridge-request");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].get("sessionId").and_then(Value::as_str),
            Some("sess-1")
        );
        assert_eq!(
            requests[0].get("requestId").and_then(Value::as_str),
            Some("r-1")
        );
        assert_eq!(
            requests[0].get("method").and_then(Value::as_str),
            Some("confirm")
        );
    }

    #[tokio::test]
    async fn a_timeout_writes_the_terminal_cancelled_frame() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let last_seq = Arc::new(AtomicU64::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let close_tx = Arc::new(close_tx);

        let (path, server_task) = spawn_server(
            "sess-1".to_string(),
            sink.clone(),
            pending.clone(),
            last_seq,
            close_tx,
            // The injectable timeout (the `Duration` parameter — Task 6 shrinks
            // it to 50 ms; here 200 ms).
            Duration::from_millis(200),
            None,
        )
        .await;
        let mut client = connect_client(&path).await;
        let frame = json!({ "v": 1, "type": "request", "id": "r-2", "method": "ask", "source": "main", "params": {} });
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;
        // Do NOT answer — the waiter times out and writes the explicit
        // terminal frame, then closes.
        let mut response_line = String::new();
        let _ = client.read_line(&mut response_line).await;
        server_task.await.expect("server task");
        let _ = std::fs::remove_file(&path);

        let response: Value = serde_json::from_str(response_line.trim()).expect("response frame");
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response")
        );
        assert_eq!(response.get("id").and_then(Value::as_str), Some("r-2"));
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("cancelled"),
            "a timeout writes the terminal error:\"cancelled\" frame"
        );
        assert!(response.get("result").is_none());
    }

    #[tokio::test]
    async fn a_session_close_cancels_the_request() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let last_seq = Arc::new(AtomicU64::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let close_tx = Arc::new(close_tx);

        let (path, server_task) = spawn_server(
            "sess-1".to_string(),
            sink.clone(),
            pending.clone(),
            last_seq,
            close_tx.clone(),
            Duration::from_secs(30),
            None,
        )
        .await;
        let mut client = connect_client(&path).await;
        let frame = json!({ "v": 1, "type": "request", "id": "r-3", "method": "password", "source": "main", "params": {} });
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;

        // Wait for the pending entry, then close the session — the waiter
        // observes the close flag and cancels.
        let mut interval = tokio::time::interval(Duration::from_millis(5));
        loop {
            interval.tick().await;
            if pending
                .lock()
                .await
                .contains_key(&bridge_key("sess-1", "r-3"))
            {
                break;
            }
        }
        close_tx.send(true).expect("close flag");
        let mut response_line = String::new();
        let _ = client.read_line(&mut response_line).await;
        server_task.await.expect("server task");
        let _ = std::fs::remove_file(&path);

        let response: Value = serde_json::from_str(response_line.trim()).expect("response frame");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("cancelled"),
            "a session close writes the terminal error:\"cancelled\" frame"
        );
    }

    // -------------------------------------------------------------------
    // `dispatch_subagent` param validation (the review finding: a missing
    // `task` must not dispatch an empty-prompt subagent session; `tools: []`
    // must not produce `--tools ''`).
    // -------------------------------------------------------------------

    /// A missing `task` is rejected (no dispatch is spawned).
    #[test]
    fn dispatch_params_rejects_a_missing_task() {
        assert!(dispatch_params(&json!({})).is_none());
        assert!(dispatch_params(&json!({ "agentName": "fake" })).is_none());
    }

    /// An empty (or blank) `task` is rejected (no dispatch is spawned).
    #[test]
    fn dispatch_params_rejects_an_empty_task() {
        assert!(dispatch_params(&json!({ "task": "" })).is_none());
        assert!(dispatch_params(&json!({ "task": "   " })).is_none());
    }

    /// `tools: []` is treated as `None` (an empty allowlist is malformed, not
    /// an allowlist — it would produce `--tools ''`); a non-empty list is
    /// kept verbatim; a missing `tools` is `None`.
    #[test]
    fn dispatch_params_treats_an_empty_tools_list_as_none() {
        let (task, launch) = dispatch_params(&json!({ "task": "t", "tools": [] })).unwrap();
        assert_eq!(task, "t");
        assert!(
            launch.tools.is_none(),
            "tools: [] is malformed, not an allowlist (it would produce `--tools ''`)"
        );
        let (_, launch) =
            dispatch_params(&json!({ "task": "t", "tools": ["read", "bash"] })).unwrap();
        assert_eq!(
            launch.tools.as_deref(),
            Some(&["read".to_string(), "bash".to_string()][..])
        );
        let (_, launch) = dispatch_params(&json!({ "task": "t" })).unwrap();
        assert!(launch.tools.is_none());
    }

    /// A `dispatch_subagent` frame with NO `task` gets `error: "invalid
    /// params"` (and NO dispatch is spawned — no `subagent-session-started`
    /// event). The server carries a real subagent manager (so the method is
    /// reachable — a `None` manager would get the unknown-method response
    /// instead).
    #[tokio::test]
    async fn a_dispatch_subagent_frame_without_a_task_gets_invalid_params() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let last_seq = Arc::new(AtomicU64::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let close_tx = Arc::new(close_tx);
        // A real subagent manager (an empty config dir — no agents; the
        // validation happens BEFORE any dispatch, so nothing is spawned).
        let dir =
            std::env::temp_dir().join(format!("bridge-dispatch-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = crate::agent::subagent::SubagentSessionManager::new(dir.clone())
            .expect("subagent manager should build");
        let spawn = crate::agent::session::SubagentSpawn {
            manager: Arc::new(manager),
            parent_cwd: dir.clone(),
            parent_agent_id: "fake".to_string(),
        };

        let (path, server_task) = spawn_server(
            "sess-1".to_string(),
            sink.clone(),
            pending,
            last_seq,
            close_tx,
            Duration::from_secs(30),
            Some(spawn),
        )
        .await;
        let mut client = connect_client(&path).await;
        // A `dispatch_subagent` frame with NO `task` (the other params are
        // present — only `task` is missing/invalid).
        let frame = json!({
            "v": 1, "type": "request", "id": "r-dispatch", "method": "dispatch_subagent",
            "source": "main", "params": { "agentName": "fake", "model": null }
        });
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;
        let mut response_line = String::new();
        let _ = client.read_line(&mut response_line).await;
        server_task.await.expect("server task");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);

        let response: Value = serde_json::from_str(response_line.trim()).expect("response frame");
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("response")
        );
        assert_eq!(
            response.get("id").and_then(Value::as_str),
            Some("r-dispatch")
        );
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("invalid params"),
            "a missing `task` is rejected with `error: \"invalid params\"`"
        );
        // NO dispatch was spawned (no `subagent-session-started` event).
        assert!(
            sink.events_named("subagent-session-started").is_empty(),
            "a rejected frame must not spawn a subagent dispatch"
        );
    }

    /// A `dispatch_subagent` frame with an EMPTY `task` gets `error:
    /// "invalid params"` (same rejection as a missing `task`).
    #[tokio::test]
    async fn a_dispatch_subagent_frame_with_an_empty_task_gets_invalid_params() {
        let sink = Arc::new(CapturingSink::default());
        let pending: PendingBridge = Arc::new(Mutex::new(HashMap::new()));
        let last_seq = Arc::new(AtomicU64::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let close_tx = Arc::new(close_tx);
        let dir =
            std::env::temp_dir().join(format!("bridge-dispatch-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = crate::agent::subagent::SubagentSessionManager::new(dir.clone())
            .expect("subagent manager should build");
        let spawn = crate::agent::session::SubagentSpawn {
            manager: Arc::new(manager),
            parent_cwd: dir.clone(),
            parent_agent_id: "fake".to_string(),
        };

        let (path, server_task) = spawn_server(
            "sess-1".to_string(),
            sink.clone(),
            pending,
            last_seq,
            close_tx,
            Duration::from_secs(30),
            Some(spawn),
        )
        .await;
        let mut client = connect_client(&path).await;
        let frame = json!({
            "v": 1, "type": "request", "id": "r-dispatch-2", "method": "dispatch_subagent",
            "source": "main", "params": { "agentName": "fake", "task": "" }
        });
        client
            .write_all((frame.to_string() + "\n").as_bytes())
            .await
            .expect("write frame");
        let _ = client.flush().await;
        let mut response_line = String::new();
        let _ = client.read_line(&mut response_line).await;
        server_task.await.expect("server task");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);

        let response: Value = serde_json::from_str(response_line.trim()).expect("response frame");
        assert_eq!(
            response.get("error").and_then(Value::as_str),
            Some("invalid params"),
            "an empty `task` is rejected with `error: \"invalid params\"`"
        );
        assert!(
            sink.events_named("subagent-session-started").is_empty(),
            "a rejected frame must not spawn a subagent dispatch"
        );
    }

    // -------------------------------------------------------------------
    // `todo_update` / `sudo_exec` (Phase 2, Task 1): the desktop-side
    // handlers (method-aware — the desktop answers them itself; the user
    // answers the `sudo_exec` sub-prompts via the existing
    // `respond_bridge_request`). Driven through a fake `SudoRunner` (no
    // real `sudo` in `cargo test`).
    // -------------------------------------------------------------------

    use std::future::Future;
    use std::time::Instant;

    use crate::agent::todo::{TodoItem, TodoStatus, TodoStore};

    /// A `SudoRunner` for tests: records the call and returns the preset
    /// result (no real `sudo` in `cargo test`).
    #[derive(Default)]
    struct FakeRunner {
        calls: StdMutex<Vec<(Vec<String>, String, Duration)>>,
        result: StdMutex<Option<SudoRun>>,
    }

    impl FakeRunner {
        fn with_result(r: SudoRun) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                result: StdMutex::new(Some(r)),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
        fn last_call(&self) -> Option<(Vec<String>, String, Duration)> {
            self.calls.lock().unwrap().last().cloned()
        }
    }

    impl SudoRunner for FakeRunner {
        fn run(
            &self,
            argv: Vec<String>,
            password: String,
            timeout: Duration,
        ) -> Pin<Box<dyn Future<Output = SudoRun> + Send + 'static>> {
            self.calls
                .lock()
                .unwrap()
                .push((argv.clone(), password.clone(), timeout));
            let r = self.result.lock().unwrap().clone().unwrap_or(SudoRun {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                error: None,
            });
            Box::pin(async move { r })
        }
    }

    /// A stream that never delivers bytes, never EOFs, and records what
    /// was written (the handler writes the response frame to it).
    struct TestStream {
        written: StdMutex<Vec<u8>>,
    }

    impl AsyncRead for TestStream {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<Result<(), io::Error>> {
            // Never ready, never EOF: the sub-prompt waiters block until
            // the test resolves the oneshot (or flips the close flag).
            Poll::Pending
        }
    }

    impl AsyncWrite for TestStream {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            self.written.lock().unwrap().extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Wait (up to ~2 s) until `key` appears in `pending_sudo`.
    async fn wait_for_sudo_key(pending: &PendingSudo, key: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if pending.lock().await.contains_key(key) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    /// Resolve a pending sudo sub-prompt (mirrors the manager's
    /// `respond_bridge_request` lookup — the key is `"{sid}/{id}:{phase}"`).
    async fn respond_sudo(pending: &PendingSudo, id: &str, phase: &str, value: Value) -> bool {
        let key = bridge_key("sid1", &format!("{id}:{phase}"));
        pending
            .lock()
            .await
            .remove(&key)
            .map(|tx| tx.send(value))
            .is_some()
    }

    /// Run `handle_todo_update` inline (it is fast — no user
    /// interaction) and return the response frame it wrote.
    async fn run_todo(params: Value, store: Arc<TodoStore>, sink: Arc<dyn EventSink>) -> Value {
        let mut stream = TestStream {
            written: StdMutex::new(Vec::new()),
        };
        let (close_tx, _close_rx) = watch::channel(false);
        handle_todo_update(
            &mut stream,
            "id-t",
            "sid1",
            "main",
            Some(&params),
            &store,
            &sink,
            &Arc::new(close_tx),
        )
        .await;
        let data = String::from_utf8(stream.written.lock().unwrap().clone()).unwrap();
        serde_json::from_str(data.lines().next().expect("a response frame was written")).unwrap()
    }

    /// Run `handle_sudo_exec` on a worker task (the handler may block on
    /// a user sub-prompt) and return the response frame it wrote (a
    /// `Value::Null` sentinel when NO frame was written — e.g. the write
    /// was skipped because the session closed first).
    async fn run_sudo(
        id: &str,
        params: Value,
        runner: Arc<dyn SudoRunner>,
        pending_sudo: PendingSudo,
        sudo_password: Arc<Mutex<HashMap<String, CachedPassword>>>,
        close_tx: Arc<watch::Sender<bool>>,
        sink: Arc<dyn EventSink>,
    ) -> tokio::task::JoinHandle<Value> {
        let id = id.to_string();
        tokio::spawn(async move {
            let mut stream = TestStream {
                written: StdMutex::new(Vec::new()),
            };
            handle_sudo_exec(
                &mut stream,
                &id,
                "sid1",
                "main",
                Some(&params),
                &sink,
                &runner,
                &pending_sudo,
                &sudo_password,
                &close_tx,
            )
            .await;
            let data = String::from_utf8(stream.written.lock().unwrap().clone()).unwrap();
            // No frame (the write was SKIPPED — the session closed before
            // the response, the agent is gone) → a `Value::Null` sentinel.
            data.lines()
                .next()
                .map(|l| serde_json::from_str(l).expect("a valid response frame"))
                .unwrap_or(Value::Null)
        })
    }

    /// The shared sudo-test fixtures (a fresh pending map / password cache
    /// / close flag / capturing sink).
    async fn sudo_fixtures(
        runner: Arc<FakeRunner>,
    ) -> (
        PendingSudo,
        Arc<Mutex<HashMap<String, CachedPassword>>>,
        Arc<watch::Sender<bool>>,
        Arc<CapturingSink>,
    ) {
        let pending_sudo: PendingSudo = Arc::new(Mutex::new(HashMap::new()));
        let sudo_password: Arc<Mutex<HashMap<String, CachedPassword>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (close_tx, _close_rx) = watch::channel(false);
        let sink = Arc::new(CapturingSink::default());
        let _ = &runner;
        (pending_sudo, sudo_password, Arc::new(close_tx), sink)
    }

    #[tokio::test]
    async fn todo_update_write_emits_the_push_and_returns_the_stored_todos() {
        let store = Arc::new(TodoStore::new());
        let sink = Arc::new(CapturingSink::default());
        let params = json!({
            "operation": "write",
            "todoList": [
                { "content": "a", "status": "pending" },
                { "content": "b", "status": "in_progress", "description": "d" },
                { "content": "c", "status": "completed" }
            ]
        });
        let response = run_todo(params, store.clone(), sink.clone()).await;
        assert_eq!(response["type"], "response");
        assert_eq!(response["id"], "id-t");
        let result = &response["result"];
        // The suite's write text VERBATIM (`tool.ts:138`).
        assert_eq!(
            result["content"][0]["text"],
            "Todos have been modified all. 1/3 completed. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable."
        );
        assert_eq!(result["details"]["operation"], "write");
        assert_eq!(result["details"]["todos"][0]["content"], "a");
        assert_eq!(result["details"]["todos"][1]["description"], "d");
        assert!(result.get("isError").is_none());
        assert_eq!(store.get("sid1").len(), 3);

        // The `todos_update` push (the EXISTING `useBridge.applyTodoUpdate`
        // shape — `sessionId` MANDATORY, `payload.{source,todos}`).
        let events = sink.events_named("bridge-event");
        assert_eq!(events.len(), 1, "one todos_update push");
        assert_eq!(events[0]["sessionId"], "sid1");
        assert_eq!(events[0]["event"], "todos_update");
        assert_eq!(events[0]["payload"]["source"], "main");
        assert_eq!(events[0]["payload"]["todos"][0]["content"], "a");
        assert!(
            events[0]["payload"]["todos"][0]
                .get("description")
                .is_none(),
            "an absent description is omitted in the push too"
        );
    }

    #[tokio::test]
    async fn todo_update_write_without_a_todo_list_gets_the_error_result() {
        let store = Arc::new(TodoStore::new());
        store.set(
            "sid1",
            vec![TodoItem {
                content: "cur".to_string(),
                status: TodoStatus::Pending,
                description: None,
            }],
        );
        let sink = Arc::new(CapturingSink::default());
        let response = run_todo(json!({ "operation": "write" }), store.clone(), sink.clone()).await;
        let result = &response["result"];
        // The suite's text + flag (`tool.ts:89-94`).
        assert_eq!(
            result["content"][0]["text"],
            "Error: todoList is required for write operation."
        );
        assert_eq!(result["details"]["error"], "todoList required");
        assert_eq!(result["details"]["todos"][0]["content"], "cur");
        assert_eq!(result["isError"], true);
        assert!(
            sink.events_named("bridge-event").is_empty(),
            "no push on a failed write"
        );
    }

    #[tokio::test]
    async fn todo_update_write_with_an_empty_content_item_fails_validation() {
        let store = Arc::new(TodoStore::new());
        let sink = Arc::new(CapturingSink::default());
        let response = run_todo(
            json!({
                "operation": "write",
                "todoList": [ { "content": "   ", "status": "pending" } ]
            }),
            store.clone(),
            sink.clone(),
        )
        .await;
        let result = &response["result"];
        // The suite's validation text (`tool.ts:84-93`).
        assert_eq!(
            result["content"][0]["text"],
            "Validation failed:\n  - Item 1: missing or invalid 'content'"
        );
        assert_eq!(
            result["details"]["error"],
            "Item 1: missing or invalid 'content'"
        );
        assert_eq!(result["isError"], true);
        assert!(
            store.get("sid1").is_empty(),
            "a failed write stores nothing"
        );
    }

    #[tokio::test]
    async fn todo_update_read_returns_the_stored_todos() {
        let store = Arc::new(TodoStore::new());
        let items = vec![
            TodoItem {
                content: "a".to_string(),
                status: TodoStatus::Pending,
                description: None,
            },
            TodoItem {
                content: "b".to_string(),
                status: TodoStatus::Completed,
                description: Some("x".to_string()),
            },
        ];
        store.set("sid1", items.clone());
        let sink = Arc::new(CapturingSink::default());
        let response = run_todo(json!({ "operation": "read" }), store, sink.clone()).await;
        let result = &response["result"];
        // The suite's read text: `JSON.stringify(todos, null, 2)`.
        assert_eq!(
            result["content"][0]["text"],
            serde_json::to_string_pretty(&items).unwrap()
        );
        assert_eq!(result["details"]["operation"], "read");
        assert_eq!(result["details"]["todos"][0]["content"], "a");
        assert!(result.get("isError").is_none());

        // An empty store gets the suite's "No todos" text.
        let response = run_todo(
            json!({ "operation": "read" }),
            Arc::new(TodoStore::new()),
            sink,
        )
        .await;
        assert_eq!(
            response["result"]["content"][0]["text"],
            "No todos. Use write operation to create a todo list."
        );
    }

    #[tokio::test]
    async fn sudo_exec_with_an_empty_command_gets_the_error_result_without_a_run() {
        let runner = Arc::new(FakeRunner::default());
        let (pending_sudo, _password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let response = run_sudo(
            "id1",
            json!({ "command": "  ", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            _password,
            close_tx,
            sink.clone(),
        )
        .await
        .await
        .unwrap();
        let result = &response["result"];
        // The suite's text + details (`tool.ts:404-411`).
        assert_eq!(
            result["content"][0]["text"],
            "The command is empty — provide the exact command to run with elevated privileges."
        );
        assert_eq!(result["details"]["error"], "empty command");
        assert_eq!(result["details"]["exitCode"], -1);
        assert_eq!(result["isError"], true);
        assert_eq!(runner.call_count(), 0, "no runner call");
        assert!(
            sink.events_named("bridge-request").is_empty(),
            "no sub-prompt was emitted"
        );
        assert!(pending_sudo.lock().await.is_empty());
    }

    #[tokio::test]
    async fn sudo_exec_without_confirmation_does_not_prompt_or_run() {
        let runner = Arc::new(FakeRunner::default());
        let (pending_sudo, _password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            _password,
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(
            wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await,
            "the confirm sub-prompt is registered before the event"
        );
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": false })
            )
            .await
        );
        let response = task.await.unwrap();
        let result = &response["result"];
        // The suite's text + details (`tool.ts:429-434`).
        assert_eq!(
            result["content"][0]["text"],
            "Command not confirmed — not executed, and no password was requested."
        );
        assert_eq!(result["details"]["error"], "command not confirmed");
        assert_eq!(result["isError"], true);
        assert_eq!(runner.call_count(), 0, "no runner call");
        assert!(
            !pending_sudo.lock().await.contains_key("sid1/id1:password"),
            "no password prompt after a cancel"
        );
        assert!(
            pending_sudo.lock().await.is_empty(),
            "the confirm entry was removed on every exit path"
        );
        // The confirm event carries the FULL modal payload (`SudoConfirmModal`
        // reads `request.method` + `params.{command,reason}` and echoes the
        // `requestId` VERBATIM into `respondBridgeRequest`).
        let reqs = sink.events_named("bridge-request");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0]["sessionId"], "sid1");
        assert_eq!(reqs[0]["requestId"], "id1:confirm");
        assert_eq!(reqs[0]["method"], "confirm");
        assert_eq!(reqs[0]["source"], "main");
        assert_eq!(reqs[0]["params"]["command"], "apt update");
        assert_eq!(reqs[0]["params"]["reason"], "r");
    }

    #[tokio::test]
    async fn sudo_exec_with_a_confirmed_password_runs_and_scrubs_the_secret() {
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: 0,
            stdout: "out line\nhunter2\nthird".to_string(),
            stderr: "prompt\nhunter2".to_string(),
            timed_out: false,
            error: None,
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "need it" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "hunter2" })
            )
            .await
        );
        let response = task.await.unwrap();
        let result = &response["result"];
        // The suite's result: the SCRUBBED stdout VERBATIM (the LLM sees it);
        // the password is scrubbed PER STREAM.
        assert_eq!(result["content"][0]["text"], "out line\n[redacted]\nthird");
        assert_eq!(result["details"]["stdout"], "out line\n[redacted]\nthird");
        assert_eq!(result["details"]["stderr"], "prompt\n[redacted]");
        assert_eq!(result["details"]["exitCode"], 0);
        assert_eq!(result["details"]["command"], "apt update");
        assert_eq!(result["details"]["reason"], "need it");
        assert!(result.get("isError").is_none());
        // The runner got `buildSudoArgv` (quoted-arg split, leading-`sudo`
        // strip, the `--` options terminator, NO `-p`) + the password + the
        // 120 s default timeout.
        assert_eq!(runner.call_count(), 1);
        let (argv, pw, timeout) = runner.last_call().unwrap();
        assert_eq!(
            argv,
            vec![
                "sudo".to_string(),
                "-S".to_string(),
                "--".to_string(),
                "apt".to_string(),
                "update".to_string()
            ]
        );
        assert_eq!(pw, "hunter2");
        assert_eq!(timeout, Duration::from_millis(120_000));
        // The password was cached (the suite's 15 min TTL) and no pending
        // entry leaked.
        assert!(password.lock().await.contains_key("sid1"));
        let entry = password.lock().await.get("sid1").unwrap().clone();
        assert!(entry.expires_at > std::time::Instant::now());
        assert!(pending_sudo.lock().await.is_empty());
    }

    #[tokio::test]
    async fn sudo_exec_with_an_empty_password_is_a_cancel() {
        let runner = Arc::new(FakeRunner::default());
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        // An EMPTY password is a cancel (the `BridgeResponseDto` convention).
        assert!(respond_sudo(&pending_sudo, "id1", "password", json!({ "password": "" })).await);
        let response = task.await.unwrap();
        let result = &response["result"];
        // The suite's text + details (`tool.ts:437-447`).
        assert_eq!(
            result["content"][0]["text"],
            "Password entry cancelled — not executed, and nothing was cached."
        );
        assert_eq!(result["details"]["error"], "password entry cancelled");
        assert_eq!(result["isError"], true);
        assert_eq!(runner.call_count(), 0, "no runner call");
        assert!(
            !password.lock().await.contains_key("sid1"),
            "nothing was cached"
        );
    }

    #[tokio::test]
    async fn sudo_exec_aborts_when_the_session_closes_mid_flow() {
        let runner = Arc::new(FakeRunner::default());
        let (pending_sudo, _password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            _password,
            close_tx.clone(),
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        close_tx.send(true).expect("close flag");
        let response = task.await.unwrap();
        // A session close maps to a cancel — and the response write is
        // SKIPPED (the agent is gone: the write races the close flag, the
        // `handle_todo_update` posture): no frame is written (the
        // `Value::Null` sentinel). No run, no password prompt, no leak.
        assert!(response.is_null(), "no response frame on a closed session");
        assert_eq!(runner.call_count(), 0, "no runner call");
        assert!(
            pending_sudo.lock().await.is_empty(),
            "the in-flight sub-prompt entry was removed"
        );
    }

    #[tokio::test]
    async fn sudo_exec_auth_failure_clears_the_cached_password() {
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: 1,
            stdout: String::new(),
            // The suite's EXACT two-condition auth-failure signature
            // (`tool.ts:225-228`): the sudo-prompt precondition + the
            // "incorrect password" marker.
            stderr: "[sudo] password for daniel\n3 incorrect password attempts".to_string(),
            timed_out: false,
            error: None,
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "wrong" })
            )
            .await
        );
        let response = task.await.unwrap();
        let result = &response["result"];
        // The suite's text (`tool.ts` auth-failure branch).
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("sudo authentication failed (incorrect password) — exit code 1."),
            "got: {}",
            result["content"][0]["text"]
        );
        assert_eq!(result["details"]["error"], "authentication failed");
        assert_eq!(result["isError"], true);
        // A mistyped password must NOT stick for the TTL.
        assert!(
            !password.lock().await.contains_key("sid1"),
            "the cached password was cleared on an auth failure"
        );
    }

    #[tokio::test]
    async fn a_second_sudo_exec_within_the_ttl_skips_the_password_prompt() {
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
            timed_out: false,
            error: None,
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        // First run: confirm + password (caches "pw1").
        let task = run_sudo(
            "id1",
            json!({ "command": "ls", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx.clone(),
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "pw1" })
            )
            .await
        );
        task.await.unwrap();
        assert!(
            password.lock().await.contains_key("sid1"),
            "the first run cached the password"
        );

        // Second run (a fresh id): the confirm STILL fires, the password
        // prompt is SKIPPED (the suite's `credentialCache.get()` hit).
        let task = run_sudo(
            "id2",
            json!({ "command": "ls", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id2:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id2",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        let response = task.await.unwrap();
        assert_eq!(response["result"]["details"]["exitCode"], 0);
        // The run used the CACHED password (no re-prompt).
        assert_eq!(runner.call_count(), 2);
        let (_, pw, _) = runner.last_call().unwrap();
        assert_eq!(pw, "pw1", "the cached password was reused");
        let reqs = sink.events_named("bridge-request");
        // Run 1 emitted confirm + password (the cache was empty); run 2
        // emitted ONLY the confirm (the cache hit skipped the prompt).
        assert_eq!(
            reqs.len(),
            3,
            "run 1: confirm + password; run 2: confirm only (the password prompt was skipped)"
        );
        assert!(reqs.iter().all(|r| r["requestId"] != "id2:password"));
        assert!(pending_sudo.lock().await.is_empty());
    }

    #[tokio::test]
    async fn sudo_exec_timeout_returns_the_partial_output_with_the_error() {
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: 124,
            stdout: "partial".to_string(),
            stderr: String::new(),
            timed_out: true,
            error: None,
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "pw" })
            )
            .await
        );
        let response = task.await.unwrap();
        let result = &response["result"];
        // The suite's timeout result (`tool.ts:493-504`): the (partial)
        // stdout + the timeout error + `isError`.
        assert_eq!(result["content"][0]["text"], "partial");
        assert_eq!(
            result["details"]["error"],
            "timed out after 120000ms — command was killed"
        );
        assert_eq!(result["isError"], true);
        // A timeout is NOT an auth failure: the cache is kept.
        assert!(password.lock().await.contains_key("sid1"));
    }

    #[tokio::test]
    async fn sudo_exec_nonzero_exit_is_an_error_and_a_spawn_failure_keeps_the_cache() {
        // Non-zero exit (no auth markers) → `isError` + the failure text.
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: 3,
            stdout: "some output".to_string(),
            stderr: "some error".to_string(),
            timed_out: false,
            error: None,
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "pw" })
            )
            .await
        );
        let response = task.await.unwrap();
        let result = &response["result"];
        assert_eq!(result["content"][0]["text"], "some output");
        assert_eq!(
            result["details"]["error"],
            "command failed with exit code 3"
        );
        assert_eq!(result["isError"], true);

        // A spawn failure (`error: Some(…)`) → a clean failure, NOT an auth
        // failure: the cached credential is KEPT.
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: -1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            error: Some("failed to spawn sudo: ENOENT".to_string()),
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        let task = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "pw" })
            )
            .await
        );
        let response = task.await.unwrap();
        let result = &response["result"];
        assert_eq!(
            result["content"][0]["text"],
            "failed to run privileged command: failed to spawn sudo: ENOENT"
        );
        assert_eq!(result["details"]["exitCode"], -1);
        assert_eq!(result["isError"], true);
        assert!(
            password.lock().await.contains_key("sid1"),
            "a spawn failure is not an auth failure — the cache is kept"
        );
    }

    #[test]
    fn build_sudo_argv_splits_quoted_args_and_strips_a_leading_sudo() {
        // `--` separates sudo's options from the command words (the
        // leading-dash hardening — the next test).
        assert_eq!(
            build_sudo_argv("apt install ripgrep"),
            vec!["sudo", "-S", "--", "apt", "install", "ripgrep"]
        );
        // A stray leading `sudo` is stripped so `sudo -S` is applied once.
        assert_eq!(
            build_sudo_argv("sudo apt update"),
            vec!["sudo", "-S", "--", "apt", "update"]
        );
        // Quoted args stay single argv entries (NO `-p`, never a shell).
        assert_eq!(
            build_sudo_argv(r"echo 'hello world'"),
            vec!["sudo", "-S", "--", "echo", "hello world"]
        );
        assert_eq!(
            build_sudo_argv(r#"apt install "ripgrep""#),
            vec!["sudo", "-S", "--", "apt", "install", "ripgrep"]
        );
        // Leading whitespace is trimmed; a lone `sudo` yields `sudo -S --`.
        assert_eq!(
            build_sudo_argv("  ls -la"),
            vec!["sudo", "-S", "--", "ls", "-la"]
        );
        assert_eq!(build_sudo_argv("sudo"), vec!["sudo", "-S", "--"]);
    }

    #[test]
    fn build_sudo_argv_a_leading_dash_word_is_not_a_sudo_flag() {
        // The security hole: without the `--`, a leading-dash word lands in
        // SUDO's own option space — `"-u nobody id"` → `sudo -S -u nobody
        // id` (runs as `nobody`, not root), and `"-p x id"` → a custom `-p`
        // prompt that DEFEATS `is_sudo_auth_failure` (the `[sudo] password
        // for` precondition never appears in stderr → a wrong password is
        // not detected as an auth failure → the bad credential stays cached
        // for the TTL). `--` ends sudo's options: the dash words are the
        // TARGET command's argv (a command named `-u` does not exist → sudo
        // fails cleanly). The suite's `buildSudoArgv` shares the hole —
        // this is a deliberate security-hardening divergence.
        assert_eq!(
            build_sudo_argv("-u nobody id"),
            vec!["sudo", "-S", "--", "-u", "nobody", "id"]
        );
        assert_eq!(
            build_sudo_argv("-p x id"),
            vec!["sudo", "-S", "--", "-p", "x", "id"]
        );
        // The normal (no-leading-dash) case is unchanged in behavior:
        // `sudo -S -- ls -la` ≡ `sudo -S ls -la` (`-la` is `ls`'s arg in
        // both — only a word BEFORE any non-option word was the hole).
        assert_eq!(
            build_sudo_argv("ls -la"),
            vec!["sudo", "-S", "--", "ls", "-la"]
        );
    }

    #[test]
    fn scrub_secret_replaces_any_line_containing_the_secret() {
        assert_eq!(scrub_secret("a\nhunter2\nc", "hunter2"), "a\n[redacted]\nc");
        assert_eq!(scrub_secret("a\nhunter2", ""), "a\nhunter2");
        assert_eq!(scrub_secret("clean", "hunter2"), "clean");
    }

    /// A write that parks (returns `Pending`) until the close flag flips —
    /// the test double for a dead-but-open peer with a full socket buffer
    /// (`write_all` parks until the write errors or the session close
    /// aborts it).
    struct ParkingStream {
        close: watch::Receiver<bool>,
    }

    impl AsyncWrite for ParkingStream {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            if *self.close.borrow() {
                Poll::Ready(Ok(buf.len()))
            } else {
                Poll::Pending
            }
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn write_sudo_response_races_the_write_against_the_close_flag() {
        let (close_tx, _close_rx) = watch::channel(false);
        let mut stream = ParkingStream {
            close: close_tx.subscribe(),
        };
        let mut rx = close_tx.subscribe();
        // The write parks (a dead-but-open peer with a full buffer): without
        // the close race, `write_sudo_response` would block FOREVER.
        let task = tokio::spawn(async move {
            write_sudo_response(&mut stream, &json!({ "v": 1 }), &mut rx).await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        close_tx.send(true).expect("close flag");
        let done = tokio::time::timeout(Duration::from_millis(2000), task).await;
        assert!(
            done.is_ok(),
            "the close flag aborts the parked write (a bare write would park forever)"
        );
    }

    #[tokio::test]
    async fn write_sudo_response_skips_the_write_when_the_session_is_already_closed() {
        let (close_tx, _close_rx) = watch::channel(true);
        let mut stream = ParkingStream {
            close: close_tx.subscribe(),
        };
        let mut rx = close_tx.subscribe();
        // The flag is ALREADY `true` at subscribe time: a `changed()` on a
        // fresh receiver would never fire (the value has not changed since
        // the subscribe) — the pre-check must skip the write, or the parked
        // write would block forever.
        let done = tokio::time::timeout(
            Duration::from_millis(500),
            write_sudo_response(&mut stream, &json!({ "v": 1 }), &mut rx),
        )
        .await;
        assert!(done.is_ok(), "an already-closed session skips the write");
    }

    #[tokio::test]
    async fn a_sudo_exec_on_an_already_closed_session_leaks_no_pending_entry() {
        let runner = Arc::new(FakeRunner::default());
        let (pending_sudo, _password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        // The session closed BEFORE the handler ran: the `close_already`
        // pre-check path (the select! is skipped entirely — the entry is
        // still inserted before the pre-check, so it must be removed here
        // too, not only on the select! exit paths). A keep-alive receiver
        // so the pre-close `send` does not `SendError` (the handler's own
        // subscription comes later).
        let _keepalive_rx = close_tx.subscribe();
        close_tx.send(true).expect("close flag");
        let response = run_sudo(
            "id1",
            json!({ "command": "apt update", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            _password,
            close_tx,
            sink.clone(),
        )
        .await
        .await
        .unwrap();
        // The close maps to a cancel — and the response write is SKIPPED
        // (the agent is gone: the write races the close flag, the
        // `handle_todo_update` posture): no frame is written (the
        // `Value::Null` sentinel). No run, no password prompt, no leak.
        assert!(response.is_null(), "no response frame on a closed session");
        assert_eq!(runner.call_count(), 0, "no runner call");
        assert!(
            pending_sudo.lock().await.is_empty(),
            "the confirm entry is removed on the close_already path too"
        );
    }

    #[tokio::test]
    async fn an_expired_cached_password_re_prompts_instead_of_reusing() {
        let runner = Arc::new(FakeRunner::with_result(SudoRun {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
            timed_out: false,
            error: None,
        }));
        let (pending_sudo, password, close_tx, sink) = sudo_fixtures(runner.clone()).await;
        // Pre-seed an ALREADY-EXPIRED entry (the `CachedPassword` fields are
        // `pub`, so the test seeds it directly — `expires_at` in the past;
        // the TTL test above covers only the HIT/future path).
        password.lock().await.insert(
            "sid1".to_string(),
            CachedPassword {
                password: "stale".to_string(),
                expires_at: std::time::Instant::now() - Duration::from_secs(1),
            },
        );
        let task = run_sudo(
            "id1",
            json!({ "command": "ls", "reason": "r" }),
            runner.clone(),
            pending_sudo.clone(),
            password.clone(),
            close_tx,
            sink.clone(),
        )
        .await;
        assert!(wait_for_sudo_key(&pending_sudo, "sid1/id1:confirm").await);
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "confirm",
                json!({ "confirmed": true })
            )
            .await
        );
        // The prompt FIRES (the expired entry is dropped, NOT reused — a
        // resumed session must not silently reuse a stale credential).
        assert!(
            wait_for_sudo_key(&pending_sudo, "sid1/id1:password").await,
            "an expired credential re-prompts"
        );
        assert!(
            respond_sudo(
                &pending_sudo,
                "id1",
                "password",
                json!({ "password": "fresh" })
            )
            .await
        );
        let response = task.await.unwrap();
        assert_eq!(response["result"]["details"]["exitCode"], 0);
        assert_eq!(runner.call_count(), 1);
        let (_, pw, _) = runner.last_call().unwrap();
        assert_eq!(pw, "fresh", "the expired credential was NOT reused");
        // The fresh password is cached with a new (future) TTL.
        assert!(password.lock().await.contains_key("sid1"));
        assert!(password.lock().await.get("sid1").unwrap().expires_at > std::time::Instant::now());
    }

    /// `run_sudo_real` with a HARMLESS binary (no real `sudo`, no root
    /// needed): the `argv[0]` is `/bin/sh` / `/bin/true` / a nonexistent
    /// path (the test asserts the binary exists or the spawn failure is the
    /// expectation).

    #[tokio::test]
    async fn run_sudo_real_an_empty_argv_is_a_failure() {
        let run = run_sudo_real(Vec::new(), "pw".to_string(), Duration::from_secs(5)).await;
        assert_eq!(run.exit_code, -1);
        assert!(!run.timed_out);
        assert_eq!(run.error.as_deref(), Some("no command in argv"));
    }

    #[tokio::test]
    async fn run_sudo_real_a_nonexistent_binary_is_a_spawn_failure() {
        assert!(
            !std::path::Path::new("/nonexistent/archimedes-test-binary-xyz").exists(),
            "the test binary must not exist"
        );
        let run = run_sudo_real(
            vec![
                "/nonexistent/archimedes-test-binary-xyz".to_string(),
                "arg".to_string(),
            ],
            "pw".to_string(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(run.exit_code, -1);
        assert!(!run.timed_out);
        assert!(
            run.error
                .as_deref()
                .is_some_and(|e| e.contains("failed to spawn")),
            "a spawn failure is recorded, got {:?}",
            run.error
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_sudo_real_exits_without_reading_stdin_is_not_an_error() {
        // `true` exits immediately without reading stdin. The password is
        // LARGER than the pipe buffer (~64 KiB), so the write cannot sit in
        // the buffer: it blocks until the child exits, then gets a fatal
        // EPIPE. The suite's EPIPE policy: record only, don't fail — the
        // child's exit settles the run (`error: None`), and the (new) 5 s
        // write timeout does NOT trip (the EPIPE arrives in milliseconds).
        assert!(
            std::path::Path::new("/bin/true").exists(),
            "/bin/true must exist"
        );
        let big_password = "x".repeat(200_000);
        let run = run_sudo_real(
            vec!["/bin/true".to_string()],
            big_password,
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(run.exit_code, 0, "the child's exit settles the run");
        assert!(!run.timed_out);
        assert!(
            run.error.is_none(),
            "a fatal EPIPE (the child finished without reading) is record-only, got {:?}",
            run.error
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_sudo_real_timeout_kills_the_whole_process_group() {
        assert!(
            std::path::Path::new("/bin/sh").exists(),
            "/bin/sh must exist"
        );
        // The script echoes a marker, backgrounds a `sleep` (the
        // GRANDCHILD), records its pid, then blocks in the foreground — the
        // timeout must kill the WHOLE group (a bare kill of the child alone
        // would leave the grandchild alive), and the PARTIAL stdout (the
        // marker) must survive the timeout (the read tasks are dropped, the
        // shared buffers keep what was read so far).
        let grandchild_pid_file = std::env::temp_dir().join(format!(
            "archimedes-sudo-group-test-{}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let script = format!(
            "echo partial; sleep 60 & echo $! > {}; sleep 60",
            grandchild_pid_file.display()
        );
        let run = run_sudo_real(
            vec!["/bin/sh".to_string(), "-c".to_string(), script],
            "pw".to_string(),
            Duration::from_millis(800),
        )
        .await;
        // The suite's timeout sentinel (124) + the flag; NO error (a timeout
        // is not a process-level failure).
        assert_eq!(run.exit_code, 124, "the suite's timeout sentinel");
        assert!(run.timed_out);
        assert!(run.error.is_none());
        // The PARTIAL stdout survives the timeout: the read tasks are
        // dropped mid-stream, but the shared buffers keep what was read so
        // far (the `from_utf8_lossy` conversion below the read phase picks
        // it up).
        assert!(
            run.stdout.contains("partial"),
            "the partial stdout (the pre-timeout echo) survived the timeout, got {:?}",
            run.stdout
        );
        // The GRANDCHILD (the backgrounded `sleep`) is dead too: the group
        // kill reached it. (Poll — the kernel reaps the orphaned, killed
        // grandchild promptly, but not instantaneously.)
        let grandchild_pid: u32 = std::fs::read_to_string(&grandchild_pid_file)
            .expect("the script wrote the grandchild's pid")
            .trim()
            .parse()
            .expect("a pid");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut gone = false;
        while std::time::Instant::now() < deadline && !gone {
            if unsafe { libc::kill(grandchild_pid as i32, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                gone = true;
            } else {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        assert!(
            gone,
            "the grandchild (pid {grandchild_pid}) is dead — the group kill reached it"
        );
        let _ = std::fs::remove_file(&grandchild_pid_file);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_sudo_real_captures_stdout_and_stderr() {
        // The capture path (piped stdout / stderr → shared buffers →
        // `from_utf8_lossy`) with REAL data: a benign command (no `sudo`,
        // no root) that writes to both streams.
        assert!(
            std::path::Path::new("/bin/sh").exists(),
            "/bin/sh must exist"
        );
        let run = run_sudo_real(
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo out; echo err >&2".to_string(),
            ],
            "pw".to_string(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(run.exit_code, 0, "a benign command exits 0");
        assert!(!run.timed_out);
        assert!(
            run.error.is_none(),
            "the child exits without reading stdin — the EPIPE is record-only, got {:?}",
            run.error
        );
        assert_eq!(run.stdout, "out\n", "stdout was captured end-to-end");
        assert_eq!(run.stderr, "err\n", "stderr was captured end-to-end");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_sudo_real_a_stalled_password_write_times_out() {
        // `sh -c "sleep 30"` never reads stdin, and the password is LARGER
        // than the pipe buffer (~64 KiB): the `write_all` would block
        // unboundedly without the (new) 5 s write timeout. The timeout maps
        // the stalled write to a process-level failure (kill + reap +
        // `error: Some(…)`), NOT an auth failure.
        assert!(
            std::path::Path::new("/bin/sh").exists(),
            "/bin/sh must exist"
        );
        let big_password = "x".repeat(200_000);
        let started = std::time::Instant::now();
        let run = run_sudo_real(
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 30".to_string(),
            ],
            big_password,
            Duration::from_secs(10),
        )
        .await;
        let elapsed = started.elapsed();
        assert_eq!(run.exit_code, -1);
        assert!(!run.timed_out, "a write timeout is not the run timeout");
        assert_eq!(
            run.error.as_deref(),
            Some("timed out writing password to sudo stdin"),
            "the stalled write is a process-level failure"
        );
        // Bounded by the 5 s write timeout (with a margin) — NOT the 10 s
        // run timeout, NOT unbounded.
        assert!(
            elapsed < Duration::from_secs(8),
            "the write was bounded, took {elapsed:?}"
        );
    }
}
