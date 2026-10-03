import { useEffect, useState, useSyncExternalStore } from "react";
import {
  listenInteractiveEvent,
  listenInteractiveRequest,
  listenInteractiveRequestClose,
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
import { useInteractive } from "./store/interactive";
import { useSubagents } from "./store/subagents";
import { getLeftPaneCollapsed, LEFT_PANE_WIDTH, subscribeLeftPane, setLeftPaneCollapsed } from "./lib/leftPaneState";
import { getSidePaneCollapsed, subscribeSidePane, setSidePaneCollapsed } from "./lib/sidePaneState";
import { useHasPendingRequest } from "./hooks/useHasPendingRequest";
import { PanelLeftIcon, PanelRightIcon } from "lucide-react";
import { Button } from "./components/ui/button";
import SpacesList from "./components/SpacesList";
import SpaceTabs from "./components/SpaceTabs";
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
        // Dismiss the session's interactive state (its pending prompts are
        // drained as cancelled server-side; the password is never kept).
        useInteractive.getState().dismissSession(payload.sessionId);
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
      listenInteractiveRequest((payload) =>
        useInteractive.getState().addRequest(payload.sessionId, payload),
      ),
    );
    // A DROPPED `sudo_exec` sub-prompt (a turn cancel skips the flow's
    // exit-path cleanup) closes its modal (the Rust `SudoPromptCleanup`
    // drop guard emits `interactive-request-close` — pre-fix the modal
    // stayed open with no pending response, and a late answer got
    // `Ok(true)` with the send silently failing).
    unlistenPromises.push(
      listenInteractiveRequestClose((payload) =>
        useInteractive
          .getState()
          .removeRequest(payload.sessionId, payload.requestId),
      ),
    );
    unlistenPromises.push(
      listenInteractiveEvent((payload) => {
        const s = useInteractive.getState();
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
    // `listenSessionClosed` handler above stays as-is; its interactive-store
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
  // The panes' collapsed flags (the shared `leftPaneState` /
  // `sidePaneState` modules — the chrome bar's collapse buttons consume
  // the SAME flags the panes read, so the buttons + the panes agree
  // without event guessing). The left logo segment is the sidebar's
  // width (the tabs start where the center column begins; it follows
  // the sidebar's collapse state: 260px expanded, 0px collapsed — the
  // label hides while collapsed, the logo only, clipped away). The
  // buttons' `bg-warning` dot: the shared `useHasPendingRequest`
  // (the active session's prompts / `ask` / `confirm` / `password`
  // requests + pending subagent requests).
  const sidebarCollapsed = useSyncExternalStore(
    subscribeLeftPane,
    getLeftPaneCollapsed,
  );
  const sidePaneCollapsed = useSyncExternalStore(
    subscribeSidePane,
    getSidePaneCollapsed,
  );
  const hasPendingRequest = useHasPendingRequest();
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
          tauri.conf.json, so this bar replaces the native titlebar).
          BROWSER-STYLE: the Space tabs live HERE (the window titlebar
          row — the reference screenshot's tabs-in-menubar look), NOT in
          the center column. The WHOLE bar is the drag region
          (`data-tauri-drag-region="deep"` on the container): Tauri's
          drag-region script honors a BARE attribute only on the element
          a mousedown lands on DIRECTLY — the bar's children (the logo
          segment, the tab spacer) would be dead zones. "deep" extends
          the region to the whole subtree, so the bar's empty areas drag
          the window (double-click maximizes / restores) while the inner
          buttons / tabs (interactive elements without the attribute)
          still block it and stay clickable. The left segment's width
          is the sidebar's width (the tabs start where the center column
          begins; it follows the sidebar's collapse state: 260px expanded,
          0px collapsed — the icon clips away with it). The LEFT collapse button (the
          sidebar's old footer control, moved up) sits left of the tabs;
          the RIGHT collapse button (the `SidePane`'s old footer toggle,
          moved up) sits at the tab bar's rightmost area, before the
          window controls. The gear stays in the sidebar footer (only
          the collapse buttons moved up). In the settings view (the
          workspace unmounts) the segment is `flex-1` (full width — the
          old chrome-bar layout; no tabs, no collapse buttons). */}
      <div
        data-tauri-drag-region="deep"
        className="flex h-10 shrink-0 items-center bg-background pr-2"
        data-testid="chrome-bar"
      >
        {view === "settings" ? (
          <div className="flex h-full flex-1 items-center pl-3">
            <img
              src="/app-icon-large.png"
              alt="Archimedes"
              className="h-4 w-4 rounded-sm object-contain select-none pointer-events-none"
              draggable={false}
            />
          </div>
        ) : (
          <>
            <div
              style={{ width: sidebarCollapsed ? 0 : LEFT_PANE_WIDTH }}
              className="flex h-full shrink-0 items-center pl-3"
            >
              <img
                src="/app-icon-large.png"
                alt="Archimedes"
                className="h-4 w-4 rounded-sm object-contain select-none pointer-events-none"
                draggable={false}
              />
            </div>
            {/* The LEFT collapse button (the sidebar's old footer control
                moved up — left of the tabs; the tabs shift right to
                accommodate it) + the `bg-warning` pending-request dot.
                (The gear stays in the sidebar footer — only the collapse
                buttons moved up.) */}
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar"}
              aria-pressed={!sidebarCollapsed}
              onClick={() => setLeftPaneCollapsed(!sidebarCollapsed)}
              className="relative"
            >
              <PanelLeftIcon className="size-4" />
              {hasPendingRequest && (
                <span
                  className="absolute top-0.5 right-0.5 size-1.5 rounded-full bg-warning"
                  aria-hidden
                />
              )}
            </Button>
            <SpaceTabs />
            {/* The RIGHT collapse button (the `SidePane`'s old footer
                toggle moved up — the tab bar's rightmost area, before
                the window controls) + the `bg-warning` dot. */}
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Toggle side pane"
              aria-pressed={!sidePaneCollapsed}
              onClick={() => setSidePaneCollapsed(!sidePaneCollapsed)}
              className="relative"
            >
              <PanelRightIcon className="size-4" />
              {hasPendingRequest && (
                <span
                  className="absolute top-0.5 right-0.5 size-1.5 rounded-full bg-warning"
                  aria-hidden
                />
              )}
            </Button>
          </>
        )}
        <WindowControls />
      </div>
      <div className="flex min-h-0 flex-1" data-testid="content-row">
        {view === "settings" ? (
          // Full width — the `SpacesList` is NOT rendered in the settings
          // view (the `SettingsPage`'s own 268px section sidebar is the
          // left edge).
          <SettingsPage onBack={() => setView("workspace")} />
        ) : (
          <>
            <SpacesList onOpenSettings={() => setView("settings")} />
            {/* The center column: the chat (the Spaces are the chrome
                bar's TABS — the old top row is gone). */}
            <div className="flex min-w-0 flex-1 flex-col">
              <ChatStream />
            </div>
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
