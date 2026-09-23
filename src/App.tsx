import { useEffect } from "react";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import {
  listenBridgeEvent,
  listenBridgeRequest,
  listenPermissionRequest,
  listenSessionClosed,
  listenSessionUpdate,
  listenSubagentClosed,
  listenSubagentSessionStarted,
  listSessions,
  listSpaces,
} from "./lib/tauri";
import { checkForUpdate, installUpdate } from "./lib/updater";
import { discardSessionMessages, useSessions } from "./store/sessions";
import { usePermissions } from "./store/permissions";
import { useBridge } from "./store/bridge";
import { useSubagents } from "./store/subagents";
import SpacesList from "./components/SpacesList";
import ChatStream from "./components/ChatStream";
import SidePane from "./components/SidePane";

function App() {
  // Register the Tauri event listeners once; dispatch into the stores.
  // `listen()` is async, so cleanup must await the pending registrations
  // before unlistening: React StrictMode re-runs the effect in dev, and a
  // stale listener that never gets unregistered doubles every event.
  useEffect(() => {
    const unlistenPromises: Array<Promise<() => void>> = [];
    unlistenPromises.push(
      listenSessionUpdate((payload) =>
        useSessions
          .getState()
          .applySessionUpdate(payload.sessionId, payload.update),
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
  // every session survives a restart). `setSpaces` auto-selects (Task 5)
  // the recent landing for boot. No listener changes: close reasons flow
  // through the existing `handleSessionClosed`).
  useEffect(() => {
    Promise.all([listSessions(), listSpaces()])
      .then(([rows, spaces]) => {
        useSessions.getState().setHistorySessions(rows);
        useSessions.getState().setSpaces(spaces);
      })
      .catch((err) =>
        console.error("failed to load stored sessions/spaces", err),
      );
  }, []);

  // The "Check for updates" menu item (Rust side) emits this event; the
  // updater plugin runs in the JS context, so the check happens here.
  useEffect(() => {
    const pending = listen("update-check-requested", async () => {
      try {
        const { available, version, currentVersion } = await checkForUpdate();
        if (!available) {
          await message(`You're up to date (v${currentVersion ?? "unknown"}).`);
          return;
        }
        const ok = await ask(
          `Version ${version} is available (you have v${currentVersion ?? "?"}). Install now?`,
          { title: "Update available", kind: "info" },
        );
        if (!ok) return;
        await installUpdate();
        await message("Update installed — restart the app to run the new version.");
      } catch (err) {
        console.error("update check failed", err);
      }
    });
    return () => {
      pending.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  return (
    <div className="flex h-screen w-screen bg-background text-foreground">
      <SpacesList />
      <ChatStream />
      <SidePane />
    </div>
  );
}

export default App;
