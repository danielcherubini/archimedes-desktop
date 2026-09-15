import { useEffect } from "react";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import {
  listenPermissionRequest,
  listenSessionClosed,
  listenSessionUpdate,
  listenTerminalOutput,
  listSessions,
} from "./lib/tauri";
import { checkForUpdate, installUpdate } from "./lib/updater";
import { useSessions } from "./store/sessions";
import { usePermissions } from "./store/permissions";
import SessionList from "./components/SessionList";
import ChatStream from "./components/ChatStream";
import TerminalPane from "./components/TerminalPane";

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
      listenTerminalOutput((payload) =>
        useSessions
          .getState()
          .appendTerminalOutput(payload.terminalId, payload.data),
      ),
    );
    return () => {
      for (const p of unlistenPromises) {
        p.then((unlisten) => unlisten()).catch(() => {});
      }
    };
  }, []);

  // On boot, load the stored sessions into the history list (the client
  // owns history: every session survives a restart).
  useEffect(() => {
    listSessions()
      .then((rows) => useSessions.getState().setHistorySessions(rows))
      .catch((err) => console.error("failed to load stored sessions", err));
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
      <SessionList />
      <ChatStream />
      <TerminalPane />
    </div>
  );
}

export default App;
