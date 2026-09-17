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
//! topology is desktop (D) → `pi-acp` (A, a direct child of D) → `pi` (P, a
//! grandchild); the peer connecting to the desktop's socket is `pi` (P). On
//! Linux the peer's pid comes from `SO_PEERCRED` and the parent chain is
//! walked via `/proc/<pid>/status` (bounded ≤8 hops). On **macOS** (no
//! `SO_PEERCRED`/`ucred`) the listener is **not started** (fail-closed — the
//! bridge is unavailable, a documented v1 limitation).

use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, watch, Mutex};

use crate::acp::session::EventSink;

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
/// **Platform policy:** on **macOS** (and other platforms) the listener is
/// NOT started — a no-op handle is returned (fail-closed, ADR 0003).
pub async fn start_listener(
    placeholder_session_id: String,
    socket_path: &Path,
    anchor_pid: u32,
    sink: Arc<dyn EventSink>,
    pending_bridge: PendingBridge,
    close_tx: &watch::Sender<bool>,
    timeout: Duration,
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
/// `pi-acp` → `pi` is 3 levels); a chain deeper than 8 is rejected
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
    } = ctx;
    match frame.get("type").and_then(Value::as_str) {
        Some("request") => {
            let id = frame
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let sid = session_id.lock().await.clone();

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
    /// refuses to register).
    async fn spawn_server(
        session_id: String,
        sink: Arc<CapturingSink>,
        pending: PendingBridge,
        last_seq: Arc<AtomicU64>,
        close_tx: Arc<watch::Sender<bool>>,
        timeout: Duration,
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
}
