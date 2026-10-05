//! The Worker event router + the crash / stalled / pending-modal cluster
//! (ADR 0025) — a CHILD module of `session`, so it sees the parent's private
//! items (`SessionManager`'s `live` / `stalled` / `config_state` fields, the
//! private `LiveWorkerSession` / `CloseKind`) with no visibility widening.

use std::path::PathBuf;

use serde_json::json;

use crate::agent::events::RpcEvent;
use crate::agent::normalize::now_ms;
use crate::agent::worker::client::WorkerInboundEvent;

use super::{CloseKind, ClosedReason, SessionManager, StalledInfo, StopReason};

impl SessionManager {
    // ── The Worker event router (ADR 0025 Task 4) ──────────────────────────
    // The `WorkerManager`'s `on_event` callback (wired in `lib.rs` with the
    // `OnceLock` late-wire). The reviewer-corrected routing: the UI's contract
    // is the `SinkFrame`s (re-emitted verbatim); the store frames drive
    // persistence only; the raw `RpcEvent` stream drives internal bookkeeping
    // only (settle detection + the `pending_turn` resolution — NEVER the UI, to
    // avoid double-delivering the `session-update` frames); the
    // `PermissionRequest` / `InteractiveRequest` frames are re-emitted with the
    // payload UNCHANGED (the frontend contract preserved by construction).
    pub fn route_event(&self, session_id: &str, is_subagent: bool, evt: WorkerInboundEvent) {
        match evt {
            WorkerInboundEvent::Store(frame) => {
                // Persistence ONLY (not the UI — the `TranscriptPersister` applies
                // the store frame to SQLite, the sole writer).
                if let Some(p) = self.persister.get() {
                    p.apply(is_subagent, &frame);
                }
            }
            WorkerInboundEvent::SinkFrame { event, payload } => {
                // The ENTIRE UI contract — re-emitted VERBATIM (the same event name +
                // payload; the frontend is unchanged).
                if let Some(sink) = self.sink.get() {
                    sink.emit(&event, payload);
                }
            }
            WorkerInboundEvent::Event(e) => {
                // Internal bookkeeping ONLY (NOT the `TauriSink` — forwarding would
                // DOUBLE-DELIVER every `session-update` the loop emits both as an
                // `RpcEvent` and as a `session-update` `SinkFrame`): the
                // `agent_settled` settle detection (the session's "busy" state) + the
                // `pending_turn` resolution.
                if matches!(e, RpcEvent::agent_settled) {
                    self.resolve_turn(session_id);
                }
            }
            WorkerInboundEvent::PermissionRequest { id, payload } => {
                // Re-emit as the `permission-request` Tauri event with the payload
                // UNCHANGED (the frontend contract preserved by construction —
                // `PermissionPrompt.tsx` / `store/permissions.ts` consume the
                // payload's `requestId` / `toolTitle` / `options` verbatim).
                if let Some(sink) = self.sink.get() {
                    sink.emit("permission-request", payload);
                }
                self.track_modal(session_id, &id);
            }
            WorkerInboundEvent::InteractiveRequest { id, payload } => {
                // Re-emit as the `interactive-request` Tauri event with the payload
                // UNCHANGED (same reasoning — `store/interactive.ts` consumes
                // `method` + `params`).
                if let Some(sink) = self.sink.get() {
                    sink.emit("interactive-request", payload);
                }
                self.track_modal(session_id, &id);
            }
            WorkerInboundEvent::Exited(code) => {
                // EXPECTED (a `detach` / `reap_all` was in flight) → the
                // `session-closed` cleanup. UNEXPECTED (a crash — the `WorkerManager`
                // fired `on_crash`) → `is_exit_expected` is `false` → no-op (the
                // `on_crash` path handles the stalled marking + the same cleanup).
                if let Some(wm) = self.worker_manager.get() {
                    if wm.is_exit_expected(session_id) {
                        self.on_session_exited_expected(session_id);
                    }
                }
                let _ = code;
            }
            WorkerInboundEvent::Ready { .. }
            | WorkerInboundEvent::WorkerError { .. }
            | WorkerInboundEvent::SubagentDispatch(_)
            | WorkerInboundEvent::SubagentCancel { .. } => {
                // The `WorkerManager`'s pump handles the `SubagentDispatch` (the
                // Supervisor-side `dispatch_subagent` flow); the `Ready` / `WorkerError`
                // / `SubagentCancel` frames are bookkeeping (the `SubagentCancel` →
                // `cancel_subagent` is Task 5). Ignored here.
            }
        }
    }

    /// The `WorkerManager`'s `on_crash` callback (the UNEXPECTED `Exited` — a Worker
    /// crash): mark the session STALLED (the `stalled` registry + the
    /// `session-stalled` event + the `stalled_info` query), perform the pending-modal
    /// cleanup (the `interactive-request-close` frames — a crashed session's open
    /// permission / interactive modals are dismissed, no stuck modal alongside the
    /// banner), and resolve the in-flight `send_prompt` (the crash outcome — the
    /// composer unlocks; it never stays locked forever).
    pub fn handle_crash(&self, session_id: &str, code: Option<i32>) {
        let at = now_ms();
        let crash_log = find_newest_crash_log();
        // Mark the session STALLED (the `stalled` registry — the frontend reads it via
        // `stalled_info` + the `session-stalled` event).
        self.stalled
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session_id.to_string(), StalledInfo { at, crash_log });
        // The pending-modal cleanup (the `interactive-request-close` frames — the
        // unanswered-request-id diff: a crashed session's open permission / interactive
        // modals are dismissed, no stuck modal alongside the banner).
        self.cleanup_pending_modals(session_id);
        // Resolve the in-flight `send_prompt` (the crash outcome — the turn is
        // incomplete; `Cancelled` unlocks the composer. The `StopReason` has no error
        // variant — the `session-stalled` event is the frontend's crash signal).
        self.resolve_turn_forced(session_id, StopReason::Cancelled);
        // Emit the `session-stalled` event (the frontend's banner — the `StalledInfo`
        // shape: `at` + `crash_log`).
        if let Some(sink) = self.sink.get() {
            sink.emit(
                "session-stalled",
                json!({
                    "sessionId": session_id,
                    "at": at,
                    "crashLog": find_newest_crash_log(),
                }),
            );
        }
        let _ = code;
    }

    /// The `stalled_info` query (the frontend's banner query — the `SessionManager`
    /// exposes it; the `get_stalled_info` command wraps it).
    pub fn stalled_info(&self, session_id: &str) -> Option<StalledInfo> {
        self.stalled
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .cloned()
    }

    /// Resolve the session's in-flight turn (the `pending_turn` oneshot) on an
    /// `agent_settled` (the `cancel_requested` flag maps the settle to `Cancelled`,
    /// else `EndTurn` — the existing settle semantics). A no-op when no turn is in
    /// flight (the slot is empty) or the session is gone.
    fn resolve_turn(&self, session_id: &str) {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        let Some(live) = live.get(session_id) else {
            return;
        };
        let cancelled_guard = live
            .cancel_requested
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let cancelled = *cancelled_guard;
        let reason = if cancelled {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        };
        let mut pending_guard = live.pending_turn.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = pending_guard.take() {
            let _ = tx.send(reason);
        }
    }

    /// Resolve the session's in-flight turn with a FORCED reason (the `Exited`
    /// / crash path — the turn is over regardless of the `cancel_requested` flag;
    /// a mid-turn death / a close must not tell `send_prompt` "turn ended normally").
    fn resolve_turn_forced(&self, session_id: &str, reason: StopReason) {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(live) = live.get(session_id) {
            let mut pending_guard = live.pending_turn.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(tx) = pending_guard.take() {
                let _ = tx.send(reason);
            }
        }
    }

    /// The EXPECTED `Exited` handling (a `detach` / `reap_all` was in flight — the
    /// `close_session` / `reap_all` path): emit `session-closed` (the `ClosedReason`
    /// payload — the `close_kind`: `User` for a `close_session`, `AgentExited` for a
    /// clean self-exit), resolve the in-flight `pending_turn` → `Cancelled`, perform
    /// the pending-modal cleanup (the `interactive-request-close` frames), and remove
    /// the live-session bookkeeping.
    fn on_session_exited_expected(&self, session_id: &str) {
        let (reason, exists) = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            let Some(live) = live.get(session_id) else {
                return; // the session is already gone (idempotent).
            };
            let kind_guard = live.close_kind.lock().unwrap_or_else(|p| p.into_inner());
            let kind = *kind_guard;
            // The `ClosedReason` mapping (the current `session.rs:876` shape): a
            // `close_session` (`CloseKind::User`) → `User`; a clean self-exit (`None`
            // — the Worker exited on its own) → `AgentExited`.
            let reason = match kind {
                Some(CloseKind::User) => ClosedReason::User,
                None => ClosedReason::AgentExited,
            };
            (reason, true)
        };
        if !exists {
            return;
        }
        // Resolve the in-flight `pending_turn` → `Cancelled` (a close / a clean self-
        // exit kills the turn — the composer unlocks; it never stays locked forever).
        self.resolve_turn_forced(session_id, StopReason::Cancelled);
        // The pending-modal cleanup (the `interactive-request-close` frames — the
        // unanswered-request-id diff: a session ending with an open prompt must NOT
        // leave a stuck modal).
        self.cleanup_pending_modals(session_id);
        // Remove the live-session bookkeeping (the `live` / `config_state` / `stalled`
        // entries — the session is over).
        {
            let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            live.remove(session_id);
        }
        self.config_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(session_id);
        // Emit the `session-closed` event (the `ClosedReason` payload — the frontend's
        // close handling).
        if let Some(sink) = self.sink.get() {
            sink.emit(
                "session-closed",
                json!({
                    "sessionId": session_id,
                    "reason": reason.as_str(),
                }),
            );
        }
    }

    /// The pending-modal cleanup (the round-3 fix — the Worker's pending maps die with
    /// it, so the Supervisor tracks the cleanup state itself: per session, the
    /// `PermissionRequest` / `InteractiveRequest` frames it relayed MINUS the
    /// `*Response`s it sent = the UNANSWERED request ids — the Supervisor sees both
    /// halves of every round-trip, so the diff is exact). Emits the same
    /// `interactive-request-close` frames the in-process teardown emits for unanswered
    /// prompts (keyed by the unanswered ids; a session ending with an open
    /// permission / interactive prompt must NOT leave a stuck modal).
    fn cleanup_pending_modals(&self, session_id: &str) {
        let unanswered = {
            let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            let Some(live) = live.get(session_id) else {
                return;
            };
            let ids_guard = live
                .pending_modal_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ids_guard.clone()
        };
        if unanswered.is_empty() {
            return;
        }
        let Some(sink) = self.sink.get() else {
            return;
        };
        for request_id in unanswered {
            sink.emit(
                "interactive-request-close",
                json!({
                    "sessionId": session_id,
                    "requestId": request_id,
                }),
            );
        }
    }

    /// Track a relayed `PermissionRequest` / `InteractiveRequest` frame (the
    /// `pending_modal_ids` set — the pending-modal cleanup's source). The `id` is the
    /// request's `requestId` (the payload's key — the `interactive-request-close`
    /// frame's `requestId` field).
    fn track_modal(&self, session_id: &str, id: &str) {
        let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(live) = live.get_mut(session_id) {
            let mut ids_guard = live
                .pending_modal_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ids_guard.insert(id.to_string());
        }
    }
}

/// The newest `crash-<ts>-worker*.log` in the data dir (the `StalledInfo`'s
/// `crash_log` — globbed and FROZEN at crash time; best-effort attribution across
/// concurrent sessions — crash logs are diagnostic, not load-bearing). `None` when
/// the data dir is unresolvable or no crash log is found.
fn find_newest_crash_log() -> Option<PathBuf> {
    let dir = dirs::data_dir()?.join("archimedes");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("crash-") && name.ends_with("-worker.log") {
            if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    newest = Some((mtime, entry.path()));
                }
            }
        }
    }
    newest.map(|(_, p)| p)
}
