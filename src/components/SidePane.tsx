import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "./ui/tabs";
import { useSessions } from "../store/sessions";
import { useSubagents } from "../store/subagents";
import { useBridge } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { getSidePaneCollapsed, subscribeSidePane } from "../lib/sidePaneState";
import TodoBoardPanel, { useMainTodoItems } from "./TodoBoardPanel";
import SubagentPanel from "./SubagentPanel";

const WIDTH_KEY = "side-pane-width";
const TAB_KEY = "side-pane-tab";
const MIN_WIDTH = 240;
const MAX_WIDTH = 480;
const DEFAULT_WIDTH = 320;

function clampWidth(w: number): number {
  return Math.min(Math.max(w, MIN_WIDTH), MAX_WIDTH);
}

function initialWidth(): number {
  const stored = localStorage.getItem(WIDTH_KEY);
  if (stored === null) return DEFAULT_WIDTH;
  const n = Number(stored);
  if (!Number.isFinite(n)) return DEFAULT_WIDTH;
  return clampWidth(n);
}

function initialTab(): "todos" | "subagents" {
  return localStorage.getItem(TAB_KEY) === "subagents" ? "subagents" : "todos";
}

/**
 * The right-hand side pane: a 320px-default resizable + collapsible frame
 * hosting the `TodoBoardPanel` (Todos tab) and the `SubagentPanel`
 * (Subagents tab) — moved up from the `ChatStream` to the `App` level so
 * they render for every session state.
 *
 * - **Collapse mechanism: the frame's `width: 0` + `overflow: hidden` —
 *   NOT `display: none`, NOT a transform, NOT unmount.** The content stays
 *   mounted and `fixed` overlays escape `overflow` clipping, so a collapsed
 *   pane never hides a pending `SudoConfirmModal`/`SudoPasswordModal` (the
 *   panel's doc comment requires this invariant). The collapsed flag is
 *   shared with the header toggle (Task 6) via `sidePaneState`: the module
 *   seeds the flag from `localStorage` at import (it is the single
 *   persistence owner) — `SidePane` only READS it via
 *   `useSyncExternalStore` (it has no collapse control of its own).
 * - **Resize:** a 4px drag handle on the frame's left edge (a `w-1`
 *   `cursor-col-resize` div, transparent hit area — a 2px
 *   `bg-foreground-subtlest/50` line shows on hover/while dragging). The
 *   width is clamped 240–480px; the live state updates on every
 *   `mousemove`, but the width is flushed to `localStorage`
 *   (`"side-pane-width"`) on `mouseup` only (NOT on every `mousemove` —
 *   ~60/s sync writes). The handle captures the pointer on `pointerdown`
 *   (released on `pointerup`/`pointercancel`) so a release outside the
 *   webview still ends the drag; a `blur` listener is the fallback.
 * - **Tabs:** manual selection, `localStorage` (`"side-pane-tab"`),
 *   default Todos, no auto-switching. Inactive triggers show a count
 *   badge: Todos = the `useMainTodoItems` derivation (the SAME extraction
 *   the panel's checklist consumes — the badge and the rendered list can
 *   never disagree); Subagents = the subagent entry count — REPLACED by
 *   a green "Waiting" pill (the SAME treatment as the sidebar session
 *   row's `waiting` badge in `SpacesList`) while ANY subagent session has
 *   a pending interactive request (`useBridge` `ask`/`confirm`/`password`
 *   requests + `usePermissions` prompts): without the cue, a pending
 *   subagent request would strand until the bridge timeout when the tab
 *   is inactive or the pane is collapsed (the entry count does not change
 *   when a request arrives, and the `ChatStream` toggle dot counts only
 *   the ACTIVE session's requests — subagent session ids are never
 *   active).
 */
export default function SidePane() {
  const [width, setWidthState] = useState(initialWidth);
  const [tab, setTab] = useState<"todos" | "subagents">(initialTab);
  // The shared collapsed flag (Task 6's header toggle consumes the same
  // module — the module seeds it from localStorage at import).
  const collapsed = useSyncExternalStore(
    subscribeSidePane,
    getSidePaneCollapsed,
  );

  // Live width state (the drag updates it on every `mousemove`); `widthRef`
  // mirrors it so the `mouseup` flush reads the latest value (the drag
  // effect's closure would be stale).
  const widthRef = useRef(width);
  const setWidth = useCallback((w: number) => {
    const clamped = clampWidth(w);
    widthRef.current = clamped;
    setWidthState(clamped);
  }, []);

  const selectTab = (value: string) => {
    const next = value === "subagents" ? "subagents" : "todos";
    setTab(next);
    localStorage.setItem(TAB_KEY, next);
  };

  // The drag (track `mousemove` on `window` while dragging; the handle
  // captures the pointer so a release outside the webview still delivers
  // the `pointerup` — a `blur` listener is the fallback).
  const [dragging, setDragging] = useState(false);
  const dragStart = useRef<{ x: number; width: number } | null>(null);
  const handleRef = useRef<HTMLDivElement>(null);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    dragStart.current = { x: e.clientX, width };
    setDragging(true);
    // Capture the pointer: the `pointerup` is delivered to the handle even
    // if the mouse is released outside the webview (it does NOT get lost).
    // (`pointerdown` is the only DOM event carrying `pointerId` — a
    // `mousedown` handler could not capture.)
    handleRef.current?.setPointerCapture(e.pointerId);
  };

  useEffect(() => {
    if (!dragging) return;
    // End the drag: clear `dragging` and flush the width to localStorage
    // ONCE (NOT on every `mousemove` — ~60/s sync writes). A
    // quota/lockdown throw is best-effort (the live width stays in state).
    const endDrag = () => {
      if (!dragStart.current) return;
      dragStart.current = null;
      setDragging(false);
      try {
        localStorage.setItem(WIDTH_KEY, String(widthRef.current));
      } catch {
        // persistence is best-effort; the width lives on in state
      }
    };
    const onMove = (e: MouseEvent) => {
      const start = dragStart.current;
      if (!start) return;
      // The handle is on the frame's LEFT edge: dragging LEFT widens.
      setWidth(start.width - (e.clientX - start.x));
    };
    const onPointerEnd = (e: PointerEvent) => {
      handleRef.current?.releasePointerCapture(e.pointerId);
      endDrag();
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", endDrag);
    window.addEventListener("pointerup", onPointerEnd);
    window.addEventListener("pointercancel", onPointerEnd);
    // The pointer is lost (released outside the webview): `blur` clears
    // `dragging` (it does NOT stick).
    window.addEventListener("blur", endDrag);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", endDrag);
      window.removeEventListener("pointerup", onPointerEnd);
      window.removeEventListener("pointercancel", onPointerEnd);
      window.removeEventListener("blur", endDrag);
    };
  }, [dragging, setWidth]);

  // The tab count badges: the resolved main-column todo count for the
  // active session (the SAME derivation `TodoBoardPanel` uses) and the
  // subagent entry count.
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const mainTodoCount = useMainTodoItems(activeSessionId).length;
  // Stable-reference selectors (no fresh values built INSIDE the selector —
  // Zustand re-renders forever otherwise); the pending counts are derived
  // in the body across ALL subagent entry session ids (the entry count does
  // NOT change when a request arrives, and the `ChatStream` toggle dot
  // counts only the ACTIVE session's requests — subagent session ids are
  // never active — so without this cue a pending request would strand
  // until the bridge timeout when the tab is inactive or the pane is
  // collapsed).
  const subagentEntries = useSubagents((s) => s.entries);
  const bridgeRequests = useBridge((s) => s.requests);
  const permissionPrompts = usePermissions((s) => s.prompts);
  const subagentCount = Object.keys(subagentEntries).length;
  let subagentPendingCount = 0;
  for (const id of Object.keys(subagentEntries)) {
    subagentPendingCount += (bridgeRequests[id] ?? []).filter(
      (r) => r.method === "ask" || r.method === "confirm" || r.method === "password",
    ).length;
    subagentPendingCount += (permissionPrompts[id] ?? []).length;
  }
  const subagentWaiting = subagentPendingCount > 0;

  return (
    // Collapse = `width: 0` + `overflow: hidden` (the content stays mounted;
    // `fixed` overlays escape the clipping).
    <div
      style={{ width: collapsed ? 0 : width }}
      className="relative m-1 flex shrink-0 flex-col overflow-hidden rounded-xl bg-background-alt"
    >
      {/* The 4px drag handle (left edge): transparent hit area, a 2px
          `bg-foreground-subtlest/50` line on hover / while dragging. */}
      <div
        ref={handleRef}
        className="absolute inset-y-0 left-0 w-1 cursor-col-resize"
        onPointerDown={onPointerDown}
      >
        <div
          className={`h-full w-0.5 bg-foreground-subtlest/50 ${
            dragging ? "opacity-100" : "opacity-0 hover:opacity-100"
          }`}
        />
      </div>
      {/* The `Tabs` root wraps the WHOLE pane (Radix requires the
          `TabsContent`s to be descendants of the root): the tab bar +
          the content. */}
      <Tabs value={tab} onValueChange={selectTab} className="flex h-full flex-col">
        <div className="flex h-9 items-center gap-0.5 px-2">
          <TabsList variant="line" className="h-full">
            <TabsTrigger
              value="todos"
              className="rounded-md px-2 text-ui-sm data-active:bg-selected"
            >
              Todos
              {tab !== "todos" && mainTodoCount > 0 && (
                <span className="ml-1 rounded-full bg-surface px-1.5 text-ui-xs">
                  {mainTodoCount}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger
              value="subagents"
              className="rounded-md px-2 text-ui-sm data-active:bg-selected"
            >
              Subagents
              {tab !== "subagents" &&
                (subagentWaiting ? (
                  // The green "Waiting" pill (the SAME treatment as the
                  // sidebar session row's `waiting` badge in `SpacesList`) —
                  // REPLACES the entry-count badge while a request is
                  // pending (the count does not change when a request
                  // arrives, so it carries no attention cue).
                  <span className="ml-1 rounded-full bg-success/14 px-2 text-ui-sm font-medium text-success">
                    Waiting
                  </span>
                ) : subagentCount > 0 ? (
                  <span className="ml-1 rounded-full bg-surface px-1.5 text-ui-xs">
                    {subagentCount}
                  </span>
                ) : null)}
            </TabsTrigger>
          </TabsList>
        </div>
        <div className="flex-1 overflow-y-auto p-3">
          {/* `forceMount`: the content stays MOUNTED for every tab (the
              panel is the ONLY render site for subagent-session requests
              — an unrendered request would hang until the bridge timeout);
              the `data-[state=inactive]:hidden` class provides the visual
              tab swap (the primitive's own `hidden` attribute is suppressed
              by `forceMount`). */}
          <TabsContent
            value="todos"
            forceMount
            className="m-0 data-[state=inactive]:hidden"
          >
            <TodoBoardPanel sessionId={activeSessionId} />
          </TabsContent>
          <TabsContent
            value="subagents"
            forceMount
            className="m-0 data-[state=inactive]:hidden"
          >
            <SubagentPanel />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  );
}
