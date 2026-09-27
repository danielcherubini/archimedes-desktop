import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { useSessions } from "../store/sessions";
import { getSidePaneCollapsed, setSidePaneCollapsed, subscribeSidePane } from "../lib/sidePaneState";
import TodoBoardPanel, { useMainOpenTodoCount } from "./TodoBoardPanel";
import { SubagentModals } from "./SubagentModals";

const WIDTH_KEY = "side-pane-width";
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

/**
 * The right-hand side pane: a 320px-default resizable + collapsible frame
 * hosting a STATUS PANEL (the ZCode `ConversationStatusPanel` treatment —
 * NOT a tabbed sidebar): a single data-gated section in one content
 * area, rendering only while it has something to show:
 *
 * - **Todos section** (`TodoBoardPanel`): the `Todos` label + `N/M` +
 *   progress header, the checklist, the subagent todo columns — hidden
 *   while there are no OPEN todos (no todos at all, or a
 *   fully-completed list; the `useMainOpenTodoCount` derivation).
 *
 * - **Auto open/close (the "popup" rule): the frame EXPANDS when open
 *   todos go 0 → >0 and COLLAPSES when there are no open todos. The
 *   pane is driven by TODOS ONLY — a subagent session never opens it
 *   (the subagents live under the "Delegating" card + the dedicated
 *   modal; their sudo modals stay mounted at the frame root regardless).
 *   The logic is EDGE-TRIGGERED (a `prevVisible` ref): a MANUAL
 *   collapse/expand while `visible` is unchanged is respected — a count
 *   change without the edge never flips the flag.
 * - **Collapse mechanism: the frame's `width: 0` + `overflow: hidden` —
 *   NOT `display: none`, NOT a transform, NOT unmount.** The content
 *   stays mounted and `fixed` overlays escape `overflow` clipping, so a
 *   collapsed pane never hides a pending `SudoConfirmModal`/
 *   `SudoPasswordModal` (rendered by `SubagentModals` at the frame root).
 *   The collapsed flag is shared with the header toggle (Task 6) via
 *   `sidePaneState`: the module seeds the flag from `localStorage` at
 *   import (it is the single persistence owner) — `SidePane` READS it
 *   via `useSyncExternalStore` and WRITES it only from the auto
 *   open/close edge (the toggle writes it itself).
 * - **Resize:** a 4px drag handle on the frame's left edge (a `w-1`
 *   `cursor-col-resize` div, transparent hit area — a 2px
 *   `bg-foreground-subtlest/50` line shows on hover/while dragging). The
 *   width is clamped 240–480px; the live state updates on every
 *   `mousemove`, but the width is flushed to `localStorage`
 *   (`"side-pane-width"`) on `mouseup` only (NOT on every `mousemove` —
 *   ~60/s sync writes). The handle captures the pointer on `pointerdown`
 *   (released on `pointerup`/`pointercancel`) so a release outside the
 *   webview still ends the drag; a `blur` listener is the fallback.
 * - **`SubagentModals`:** the bridge sudo modals for the subagent
 *   entries, rendered at the frame ROOT (a `fixed` overlay, NOT inside
 *   the scrollable content — see the component doc in
 *   `SubagentModals`).
 */
export default function SidePane() {
  const [width, setWidthState] = useState(initialWidth);
  // The shared collapsed flag (the header toggle consumes the same
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
      // `endDrag` FIRST: if `releasePointerCapture` throws (an expired
      // `pointerId` / a non-compliant webview) the drag must still end —
      // otherwise `dragging` sticks and the width never flushes. (It is
      // idempotent via the `dragStart` guard: a later `mouseup` is a no-op.)
      endDrag();
      try {
        handleRef.current?.releasePointerCapture(e.pointerId);
      } catch {
        // capture already expired — the drag is ended above
      }
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

  // The data: the resolved main-column OPEN todo count for the active
  // session (the SAME derivation the Todos section's visibility uses).
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const mainTodoCount = useMainOpenTodoCount(activeSessionId);

  // The auto open/close (the "popup" rule — see the component doc): the
  // frame follows `visible` EDGE-TRIGGERED (the `prevVisible` ref — a
  // manual collapse/expand while `visible` is unchanged is respected: a
  // count change without the edge never flips the flag). The pane is
  // driven by TODOS ONLY (a subagent session never opens it — the
  // subagents live under the "Delegating" card + the dedicated modal).
  const visible = mainTodoCount > 0;
  const prevVisibleRef = useRef(visible);
  useEffect(() => {
    const was = prevVisibleRef.current;
    prevVisibleRef.current = visible;
    if (was === visible) return;
    setSidePaneCollapsed(!visible);
  }, [visible]);

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
      {/* The STATUS PANEL (the ZCode `ConversationStatusPanel` treatment):
          the data-gated sections stacked in one content area (no tabs). */}
      <div className="flex-1 overflow-y-auto p-3">
        <div className="flex flex-col gap-4">
          {/* The Todos section (data-gated by `TodoBoardPanel` — renders
              `null` while there are no open todos). */}
          <TodoBoardPanel sessionId={activeSessionId} />
        </div>
      </div>
      {/* The bridge sudo modals for the entries (at the frame ROOT —
          `fixed` overlays, NOT inside the scrollable content: a collapsed
          pane never hides a pending modal). */}
      <SubagentModals />
    </div>
  );
}
