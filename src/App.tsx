import { useEffect } from "react";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import {
  listenBridgeEvent,
  listenBridgeRequest,
  listenPermissionRequest,
  listenSessionClosed,
  listenSessionUpdate,
  listSessions,
  listSpaces,
} from "./lib/tauri";
import { checkForUpdate, installUpdate } from "./lib/updater";
import { useSessions } from "./store/sessions";
import { usePermissions } from "./store/permissions";
import { useBridge } from "./store/bridge";
import SpacesList from "./components/SpacesList";
import ChatStream from "./components/ChatStream";

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
    return () => {
      for (const p of unlistenPromises) {
        p.then((unlisten) => unlisten()).catch(() => {});
      }
    };
  }, []);

  // On boot, load the stored sessions AND spaces (the client owns history:
  // every session survives a restart). `setSpaces` auto-selects (Task 5)
  // the recent landing for boot. No listener changes: the `replaced`
  // reason flows through the existing `handleSessionClosed`).
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
    <div className="flex h-screen overflow-hidden bg-neutral-950 text-neutral-100">
      <SpacesList />
      <ChatStream />
    </div>
  );
}

export default App;
