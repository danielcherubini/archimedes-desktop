import { useEffect } from "react";
import {
  listenPermissionRequest,
  listenSessionClosed,
  listenSessionUpdate,
  listenTerminalOutput,
} from "./lib/tauri";
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

  return (
    <div className="flex h-screen overflow-hidden bg-neutral-950 text-neutral-100">
      <SessionList />
      <ChatStream />
      <TerminalPane />
    </div>
  );
}

export default App;
