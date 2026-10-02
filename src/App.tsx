import { useEffect, useState } from "react";
import {
  listenBridgeEvent,
  listenBridgeRequest,
  listenBridgeRequestClose,
  listenPermissionRequest,
  listenSessionClosed,
  listenSessionUpdate,
  listenSubagentClosed,
  listenSubagentSessionStarted,
  listSessions,
  listSpaces,
} from "./lib/tauri";
import { createBatchedSessionUpdate } from "./lib/batchSessionUpdates";
import { discardSessionMessages, useSessions } from "./store/sessions";
import { usePermissions } from "./store/permissions";
import { useBridge } from "./store/bridge";
import { useSubagents } from "./store/subagents";
import SpacesList from "./components/SpacesList";
import ChatStream from "./components/ChatStream";
import SidePane from "./components/SidePane";
import WindowControls from "./components/WindowControls";
import SubagentDetailHost from "./components/SubagentDetailHost";
import SettingsPage from "./components/settings/SettingsPage";

function App() {
  // Register the Tauri event listeners once; dispatch into the stores.
  // `listen()` is async, so cleanup must await the pending registrations
  // before unlistening: React StrictMode re-runs the effect in dev, and a
  // stale listener that never gets unregistered doubles every event.
  useEffect(() => {
    const unlistenPromises: Array<Promise<() => void>> = [];
    // Coalesce the `session-update` events into one store update per frame:
    // a local agent at a high thinking level emits ~1000 chunks/sec, and one
    // React render per event would saturate the webview main thread (the UI
    // renders at ~1fps while the GPU sits idle — see `batchSessionUpdates.ts`).
    const batchedSessionUpdate = createBatchedSessionUpdate(
      (items) => useSessions.getState().applySessionUpdates(items),
      (fn) => requestAnimationFrame(fn),
    );
    unlistenPromises.push(
      listenSessionUpdate((payload) =>
        batchedSessionUpdate({
          sessionId: payload.sessionId,
          update: payload.update,
        }),
      ),
    );
    unlistenPromises.push(
      listenSessionClosed((payload) => {
        // Dismiss the session's bridge state (its pending prompts are
        // drained as cancelled server-side; the password is never kept).
        useBridge.getState().dismissSession(payload.sessionId);
        useSessions
          .getState()
          .handleSessionClosed(payload.sessionId, payload.reason);
        // Ephemeral subagent sessions have no stored history: their
        // transcript is deleted (a no-op for MAIN sessions, whose
        // `handleSessionClosed` behavior above stays unchanged).
        discardSessionMessages(payload.sessionId);
      }),
    );
    unlistenPromises.push(
      listenPermissionRequest((payload) =>
        usePermissions
          .getState()
          .addPrompt(payload.sessionId, payload.requestId, payload.request),
      ),
    );
    unlistenPromises.push(
      listenBridgeRequest((payload) =>
        useBridge.getState().addRequest(payload.sessionId, payload),
      ),
    );
    // A DROPPED `sudo_exec` sub-prompt (a turn cancel skips the flow's
    // exit-path cleanup) closes its modal (the Rust `SudoPromptCleanup`
    // drop guard emits `bridge-request-close` — pre-fix the modal stayed
    // open with no pending response, and a late answer got `Ok(true)` with
    // the send silently failing).
    unlistenPromises.push(
      listenBridgeRequestClose((payload) =>
        useBridge.getState().removeRequest(payload.sessionId, payload.requestId),
      ),
    );
    unlistenPromises.push(
      listenBridgeEvent((payload) => {
        const s = useBridge.getState();
        // Wire names: the bus `COST_UPDATE` maps to `cost_update` (the
        // `archimedes:`-prefix-strip lookup), NOT `cost`.
        if (payload.event === "todos_update") {
          s.applyTodoUpdate(payload.sessionId, payload.payload);
        } else if (payload.event === "todos_clear") {
          s.applyTodoClear(payload.sessionId, payload.payload);
        } else if (payload.event === "state") {
          s.applyState(payload.sessionId, payload.payload);
        } else if (payload.event === "cost_update") {
          s.applyCost(payload.sessionId, payload.payload);
        } else if (payload.event === "session") {
          s.applySession(payload.sessionId, payload.payload);
        }
      }),
    );
    // Subagent sessions (Task 4): the `subagent-session-started` /
    // `subagent-closed` events feed the subagents store (the panel's
    // authority for subagent STATUS + the metrics SNAPSHOT — the existing
    // `listenSessionClosed` handler above stays as-is; its bridge-store
    // deletion is exactly why the metrics live in the snapshot).
    unlistenPromises.push(
      listenSubagentSessionStarted((p) =>
        useSubagents.getState().addSession({
          sessionId: p.sessionId,
          parentSessionId: p.parentSessionId,
          agentName: p.agentName,
          task: p.task,
          status: "running",
          model: p.model,
          thinkingLevel: p.thinkingLevel,
        }),
      ),
    );
    unlistenPromises.push(
      listenSubagentClosed((p) => {
        // Discard the transcript BEFORE `markClosed` (order matters):
        // `markClosed` runs `evictOldestClosed`, which can evict this
        // entry synchronously (the oldest of 21+ closed entries by
        // insertion order) — a discard running AFTER would then no-op
        // its `entries[sessionId]` guard. The entry exists from
        // `subagent-session-started` (the worker task emits `started`
        // before `subagent-closed` sequentially) and `running` entries
        // are never evicted, so the discard-first order is deterministic
        // (absent an explicit user `dismiss`).
        //
        // Why discard here AT ALL (not only in `listenSessionClosed`):
        // `subagent-session-started` is emitted by the WORKER task
        // (after `drive_session` returns) while `session-closed` is
        // emitted by the DRIVER task (after teardown) — different
        // tasks. If the agent dies (or a cancel lands) immediately
        // post-establishment, `session-closed` can beat `started`: the
        // `session-closed` handler's `discardSessionMessages` no-ops (no
        // subagent entry yet) while `handleSessionClosed` creates
        // `messages[sid]`, and nothing else re-runs the discard — a
        // small unreclaimed leak (at most a partial transcript) per
        // occurrence. Discarding here covers that race independent of
        // the cross-task ordering.
        discardSessionMessages(p.sessionId);
        useSubagents
          .getState()
          .markClosed(p.sessionId, p.status, p.error, p.metrics);
      }),
    );
    return () => {
      for (const p of unlistenPromises) {
        p.then((unlisten) => unlisten()).catch(() => {});
      }
    };
  }, []);

  // On boot, load the stored sessions AND spaces (the client owns history:
  // every session survives a restart). `listSessions(true)` fetches the
  // FULL list (archived included); the store splits it by the flag, and
  // `setSpaces` auto-selects (Task 5) the recent landing for boot. No
  // listener changes: close reasons flow through the existing
  // `handleSessionClosed`).
  //
  // The content-area view: the workspace (the default) OR the settings
  // page (opened from the `SpacesList`'s gear icon). While settings is
  // open the workspace UNMOUNTS (the ZCode `opacity-0` + `inert` pattern
  // is for keeping the workspace MOUNTED — the stores are the source of
  // truth and re-hydrate on remount, so the simpler unmount is used): a
  // session running in the background keeps streaming into the stores
  // (the listener `useEffect`s above are view-independent), and the
  // `SpacesList` / `ChatStream` / `SidePane` re-read them on remount.
  const [view, setView] = useState<"workspace" | "settings">("workspace");
  useEffect(() => {
    Promise.all([listSessions(true), listSpaces()])
      .then(([rows, spaces]) => {
        useSessions.getState().setHistorySessions(rows);
        useSessions.getState().setSpaces(spaces);
      })
      .catch((err) =>
        console.error("failed to load stored sessions/spaces", err),
      );
  }, []);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden rounded-xl bg-background text-foreground">
      {/* Frameless window chrome (the window is `decorations: false` in
          tauri.conf.json, so this bar replaces the native titlebar). The left
          region is the drag area — `data-tauri-drag-region` moves the window;
          the controls sit OUTSIDE it so a click on a button never starts a
          drag. */}
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-border bg-background pr-2 pl-3">
        <div data-tauri-drag-region className="flex h-full flex-1 items-center gap-2">
          <img
            src="/app-icon.png"
            alt="Archimedes"
            className="h-4 w-4 rounded-sm object-contain select-none pointer-events-none"
            draggable={false}
          />
          <span className="text-ui-caption font-medium text-foreground-subtle">Archimedes</span>
        </div>
        <WindowControls />
      </div>
      <div className="flex min-h-0 flex-1">
        {view === "settings" ? (
          // Full width — the `SpacesList` is NOT rendered in the settings
          // view (the `SettingsPage`'s own 268px section sidebar is the
          // left edge).
          <SettingsPage onBack={() => setView("workspace")} />
        ) : (
          <>
            <SpacesList onOpenSettings={() => setView("settings")} />
            <ChatStream />
            <SidePane />
            {/* The dedicated subagent transcript view (Task 5): the single
                ALWAYS-MOUNTED host — the visible modal for the selected
                subagent + a hidden `SubagentTranscript` for every other one
                (a `fixed` overlay, so its position in the flex row does not
                affect layout). */}
            <SubagentDetailHost />
          </>
        )}
      </div>
    </div>
  );
}

export default App;
