import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "./ui/tabs";
import { useSessions } from "../store/sessions";
import { useSubagents } from "../store/subagents";
import {
  getSidePaneCollapsed,
  setSidePaneCollapsed,
  subscribeSidePane,
} from "../lib/sidePaneState";
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
 *   shared with the header toggle (Task 6) via `sidePaneState`: this
 *   component hydrates it from `localStorage` on mount by calling
 *   `setSidePaneCollapsed` (a harmless same-value echo — the module is the
 *   single persistence owner; SidePane has no collapse control of its
 *   own).
 * - **Resize:** a 4px drag handle on the frame's left edge (a `w-1`
 *   `cursor-col-resize` div, transparent hit area — a 2px
 *   `bg-foreground-subtlest/50` line shows on hover/while dragging). The
 *   width is clamped 240–480px and persisted to `localStorage`
 *   (`"side-pane-width"`) write-through (no debounce).
 * - **Tabs:** manual selection, `localStorage` (`"side-pane-tab"`),
 *   default Todos, no auto-switching. Inactive triggers show a count
 *   badge: Todos = the `useMainTodoItems` derivation (the SAME extraction
 *   the panel's checklist consumes — the badge and the rendered list can
 *   never disagree); Subagents = the subagent entry count.
 */
export default function SidePane() {
  const [width, setWidthState] = useState(initialWidth);
  const [tab, setTab] = useState<"todos" | "subagents">(initialTab);
  // The shared collapsed flag (Task 6's header toggle consumes the same
  // module).
  const collapsed = useSyncExternalStore(
    subscribeSidePane,
    getSidePaneCollapsed,
  );

  // Hydrate the collapsed flag from `localStorage` on mount (a harmless
  // same-value echo — the module is the single persistence owner).
  useEffect(() => {
    setSidePaneCollapsed(localStorage.getItem("side-pane-collapse") === "true");
  }, []);

  // Write-through (no debounce) — stable identity (the drag effect's
  // dependency).
  const setWidth = useCallback((w: number) => {
    const clamped = clampWidth(w);
    setWidthState(clamped);
    localStorage.setItem(WIDTH_KEY, String(clamped));
  }, []);

  const selectTab = (value: string) => {
    const next = value === "subagents" ? "subagents" : "todos";
    setTab(next);
    localStorage.setItem(TAB_KEY, next);
  };

  // The drag (track `mousemove`/`mouseup` on `window` while dragging).
  const [dragging, setDragging] = useState(false);
  const dragStart = useRef<{ x: number; width: number } | null>(null);

  const onMouseDown = (e: ReactMouseEvent) => {
    e.preventDefault();
    dragStart.current = { x: e.clientX, width };
    setDragging(true);
  };

  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: MouseEvent) => {
      const start = dragStart.current;
      if (!start) return;
      // The handle is on the frame's LEFT edge: dragging LEFT widens.
      setWidth(start.width - (e.clientX - start.x));
    };
    const onUp = () => {
      dragStart.current = null;
      setDragging(false);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [dragging, setWidth]);

  // The tab count badges: the resolved main-column todo count for the
  // active session (the SAME derivation `TodoBoardPanel` uses) and the
  // subagent entry count.
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const mainTodoCount = useMainTodoItems(activeSessionId).length;
  const subagentCount = useSubagents((s) => Object.keys(s.entries).length);

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
        className="absolute inset-y-0 left-0 w-1 cursor-col-resize"
        onMouseDown={onMouseDown}
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
              {tab !== "subagents" && subagentCount > 0 && (
                <span className="ml-1 rounded-full bg-surface px-1.5 text-ui-xs">
                  {subagentCount}
                </span>
              )}
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
