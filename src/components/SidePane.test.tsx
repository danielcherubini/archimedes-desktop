import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import SidePane from "./SidePane";
import { useSubagents } from "../store/subagents";
import { useInteractive } from "../store/interactive";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { getSidePaneCollapsed, setSidePaneCollapsed, SIDE_PANE_RAIL } from "../lib/sidePaneState";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    respondInteractiveRequest: vi.fn().mockResolvedValue(undefined),
    respondPermission: vi.fn().mockResolvedValue(undefined),
  };
});

const entry = {
  sessionId: "sub1",
  parentSessionId: "main1",
  agentName: "reviewer",
  task: "review the diff",
  status: "running" as const,
};

beforeAll(() => {
  // jsdom has no pointer capture: stub it (the component guards the ELEMENT
  // with `?.`, not the method).
  if (!HTMLElement.prototype.setPointerCapture) {
    HTMLElement.prototype.setPointerCapture = vi.fn();
    HTMLElement.prototype.releasePointerCapture = vi.fn();
  }
});

beforeEach(() => {
  setSidePaneCollapsed(false);
  localStorage.clear();
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  usePermissions.getState().dismissSessionPrompts("sub1");
  usePermissions.getState().dismissSessionPrompts("main1");
  useInteractive.getState().dismissSession("sub1");
  useInteractive.getState().dismissSession("main1");
  useSessions.setState({ messages: {}, activeSessionId: null });
});

describe("SidePane (the status panel)", () => {
  it("renders NO sections when there are no todos", () => {
    const { container } = render(<SidePane />);
    // The section is data-gated (the ZCode `canRender*` treatment): an
    // empty pane is an empty frame — no "No todos yet" placeholders.
    expect(screen.queryByText("Todos")).toBeNull();
    expect(container.textContent).toBe("");
  });

  it("shows the session title (the first user message, truncated to 80 chars) at the top of the pane", () => {
    // The old chat header's title moved here (the header is gone): the
    // first `user` message, truncated to ~80 chars (the SAME derivation
    // the sidebar rows use).
    const long = "x".repeat(120);
    useSessions.setState({
      activeSessionId: "main1",
      messages: {
        main1: [{ kind: "user", text: long, at: Date.now() }],
      },
    });
    render(<SidePane />);
    const title = screen.getByText(long.slice(0, 80));
    expect(title).toBeTruthy();
    // The title sits ABOVE the Todos section (the pane's top section).
    const todos = screen.queryByText("Todos");
    expect(todos).toBeNull(); // no todos — but the title still renders
    // (the title is NOT data-gated on todos).
  });

  it("falls back to the Space's base name when the session has no user message yet", () => {
    useSessions.setState({
      activeSessionId: "main1",
      sessions: [
        {
          sessionId: "main1",
          cwd: "/tmp/MyProject",
          capabilities: {},
          archived: false,
        },
      ],
      messages: {},
    });
    render(<SidePane />);
    // No `user` message → the title is the active session's cwd's base
    // name (a session whose cwd is no known Space still shows its
    // folder's base name).
    expect(screen.getByText("MyProject")).toBeTruthy();
  });

  it("renders the Todos section (label + N/M + progress) when there are open todos", () => {
    useSessions.setState({ activeSessionId: "main1" });
    useInteractive.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "a", status: "pending" },
        { content: "b", status: "completed" },
      ],
    });
    const { container } = render(<SidePane />);
    // The section label + the open-count derivation (1 of 2 open).
    expect(screen.getByText("Todos")).toBeTruthy();
    expect(screen.getByText("1/2")).toBeTruthy();
    const indicator = container.querySelector('[data-slot="progress-indicator"]');
    expect(indicator).not.toBeNull();
  });

  it("hides the Todos section when all todos are completed", () => {
    useSessions.setState({ activeSessionId: "main1" });
    useInteractive.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "a", status: "completed" },
        { content: "b", status: "completed" },
      ],
    });
    render(<SidePane />);
    expect(screen.queryByText("Todos")).toBeNull();
    expect(screen.queryByText("1/2")).toBeNull();
  });

  // -- Auto open/close (the "popup" rule: the pane is driven by TODOS
  // -- ONLY — a subagent session never opens it; edge-triggered, manual
  // -- choice wins) --

  it("auto-expands the frame when open todos appear (0 → visible)", () => {
    useSessions.setState({ activeSessionId: "main1" });
    act(() => {
      setSidePaneCollapsed(true);
    });
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
    // Open todos appear (the 0 → visible edge): the frame EXPANDS.
    act(() => {
      useInteractive.getState().applyTodoUpdate("main1", {
        source: "main",
        todos: [{ content: "a", status: "pending" }],
      });
    });
    expect(frame.style.width).toBe("320px");
  });

  it("does NOT auto-expand on mount when todos already exist (no 0 → visible edge on the first render)", () => {
    useSessions.setState({ activeSessionId: "main1" });
    useInteractive.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [{ content: "a", status: "pending" }],
    });
    act(() => {
      setSidePaneCollapsed(true);
    });
    const { container } = render(<SidePane />);
    // The pane was collapsed while the todos were already open: the first
    // render is NOT an edge — the user's collapsed choice is respected.
    expect((container.firstChild as HTMLElement).style.width).toBe(`${SIDE_PANE_RAIL}px`);
  });

  it("auto-collapses the frame when all todos complete (visible → 0)", () => {
    useSessions.setState({ activeSessionId: "main1" });
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    act(() => {
      useInteractive.getState().applyTodoUpdate("main1", {
        source: "main",
        todos: [
          { content: "a", status: "pending" },
          { content: "b", status: "pending" },
        ],
      });
    });
    expect(frame.style.width).toBe("320px"); // auto-expanded
    // All todos complete (the visible → 0 edge): the frame COLLAPSES (the
    // Todos section is hidden — the pane is hidden with it).
    act(() => {
      useInteractive.getState().applyTodoUpdate("main1", {
        source: "main",
        todos: [
          { content: "a", status: "completed" },
          { content: "b", status: "completed" },
        ],
      });
    });
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
  });

  it("does NOT auto-expand the frame when a subagent session starts (the pane is driven by todos only)", () => {
    act(() => {
      setSidePaneCollapsed(true);
    });
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
    // A subagent starts: NO 0 → visible edge (the pane is todos-only —
    // a subagent no longer opens it): the frame STAYS COLLAPSED.
    act(() => {
      useSubagents.getState().addSession(entry);
    });
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
  });

  it("respects a MANUAL collapse while work is in flight (a change without the edge does not re-expand)", () => {
    useSessions.setState({ activeSessionId: "main1" });
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    act(() => {
      useInteractive.getState().applyTodoUpdate("main1", {
        source: "main",
        todos: [{ content: "a", status: "pending" }],
      });
    });
    expect(frame.style.width).toBe("320px"); // auto-expanded
    // The user manually collapses (the header toggle) while work is open.
    act(() => {
      setSidePaneCollapsed(true);
    });
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
    // Another todo arrives (1 → 2 — still visible, NO edge): the manual
    // collapse is respected (no re-expand).
    act(() => {
      useInteractive.getState().applyTodoUpdate("main1", {
        source: "main",
        todos: [
          { content: "a", status: "pending" },
          { content: "b", status: "pending" },
        ],
      });
    });
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
  });

  // -- The footer toggle (the old header's toggle moved to the frame's
  // -- bottom-right; the collapsed rail keeps it reachable) --

  // -- The pane's OWN collapse toggle (bottom-inner corner = bottom LEFT for
  // -- the right pane, pointing back at the chat). Collapsed, it hands off to
  // -- the chrome bar — see `App.test.tsx` for the handoff. --

  it("holds its own collapse toggle at the frame's inner-bottom corner", () => {
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    const collapse = screen.getByRole("button", { name: "Collapse side pane" });
    expect(frame.contains(collapse)).toBe(true);
    // Anchored to the frame's bottom-left — the INNER side for a right-hand
    // pane (it points back at the chat). Asserted as classes because jsdom
    // resolves no layout: `absolute` + both bottom and left, so it cannot
    // silently migrate to the outer edge.
    const cls = collapse.className;
    expect(cls).toMatch(/absolute/);
    expect(cls).toMatch(/\bbottom-/);
    expect(cls).toMatch(/\bleft-/);
    expect(cls).not.toMatch(/\bright-/);
    expect(cls).not.toMatch(/\btop-/);
    // Pressed = open, the same meaning the chrome bar's control carries.
    expect(collapse.getAttribute("aria-pressed")).toBe("true");
    expect(screen.queryByRole("button", { name: "Expand side pane" })).toBeNull();
  });

  it("insets the toggle clear of the 4px resize handle (the handle stays grabbable full-height)", () => {
    render(<SidePane />);
    const collapse = screen.getByRole("button", { name: "Collapse side pane" });
    const left = collapse.className.match(/\bleft-\[?([0-9]+)\]?/);
    expect(left, "the toggle is not offset from the left edge").toBeTruthy();
    // Tailwind spacing = 4px; the handle is `w-1` = 4px. The toggle must start
    // at least one unit clear of it, or resizing is impossible in that band.
    const unit = left![1].includes("[") ? Number(left![1]) : Number(left![1]) * 4;
    expect(unit, `left offset ${unit}px sits on the 4px handle`).toBeGreaterThanOrEqual(8);
  });

  it("KEEPS its toggle while collapsed, in the sliver — it never moves to the chrome bar", () => {
    setSidePaneCollapsed(true);
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
    // The STABLE hook: the label flips with the state, so a name query would
    // miss on exactly the collapsed side this test checks.
    const collapse = screen.getByTestId("right-pane-toggle");
    expect(frame.contains(collapse)).toBe(true);
    // One control does both jobs (no shortcut exists for this pane), and the
    // label flips to the direction it now performs.
    expect(container.querySelectorAll('[data-testid="right-pane-toggle"]').length).toBe(1);
    expect(collapse.getAttribute("aria-label")).toBe("Expand side pane");
    expect(collapse.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(collapse);
    expect(getSidePaneCollapsed()).toBe(false);
  });

  it("hides the resize handle while collapsed (resizing a 40px sliver is moot) but keeps the toggle", () => {
    setSidePaneCollapsed(true);
    const { container } = render(<SidePane />);
    expect(container.querySelector(".cursor-col-resize")).toBeNull();
    expect(screen.getByTestId("right-pane-toggle")).toBeTruthy();
  });

  it("collapses to the sliver (the content stays mounted, clipped — the pane keeps its own toggle)", () => {
    useSessions.setState({ activeSessionId: "main1" });
    useInteractive.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [{ content: "a", status: "pending" }],
    });
    act(() => {
      setSidePaneCollapsed(true);
    });
    const { container } = render(<SidePane />);
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
    // The content (the Todos section) is clipped (width 0 +
    // `overflow: hidden`) — NOT unmounted (the `fixed` sudo modals +
    // the todo state stay alive).
    expect(screen.queryByText("Todos")).not.toBeNull();
    // Expanding via the shared flag restores the full frame.
    act(() => {
      setSidePaneCollapsed(false);
    });
    expect(frame.style.width).toBe("320px");
  });

  it("flushes the drag width to localStorage on mouseup only (not on every mousemove)", () => {
    const { container } = render(<SidePane />);
    const handle = container.querySelector(".cursor-col-resize") as HTMLElement;
    const line = handle.querySelector("div") as HTMLElement;
    // `pointerdown` (the event carrying `pointerId` — `mousedown` does not
    // in the DOM) starts the drag and captures the pointer.
    fireEvent.pointerDown(handle);
    // Dragging: the line is visible (the class ends with `opacity-100`;
    // the non-drag class ends with `hover:opacity-100` — anchor at the end).
    expect(line.className).toMatch(/ opacity-100$/);
    fireEvent.mouseMove(window, { clientX: 300 });
    // The live width updates (320 - 300 clamped to the 240 min) but
    // localStorage is NOT written mid-drag.
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe("240px");
    expect(localStorage.getItem("side-pane-width")).toBeNull();
    // The release (captured: delivered to the handle even outside the
    // webview) flushes the width ONCE and ends the drag.
    fireEvent.pointerUp(handle);
    expect(localStorage.getItem("side-pane-width")).toBe("240");
    expect(line.className).not.toMatch(/ opacity-100$/);
  });

  it("still ends the drag and flushes the width when releasePointerCapture throws", () => {
    const { container } = render(<SidePane />);
    const handle = container.querySelector(".cursor-col-resize") as HTMLElement;
    const line = handle.querySelector("div") as HTMLElement;
    // An expired `pointerId` / non-compliant webview: `releasePointerCapture`
    // throws — the drag must still end (the width must still flush; `dragging`
    // must NOT stick on).
    const release = vi
      .spyOn(handle, "releasePointerCapture")
      .mockImplementation(() => {
        throw new Error("InvalidPointerId");
      });
    try {
      fireEvent.pointerDown(handle);
      expect(line.className).toMatch(/ opacity-100$/);
      fireEvent.mouseMove(window, { clientX: 300 });
      fireEvent.pointerUp(handle);
      expect(line.className).not.toMatch(/ opacity-100$/);
      expect(localStorage.getItem("side-pane-width")).toBe("240");
    } finally {
      release.mockRestore();
    }
  });

  it("ends the drag on window blur (a release outside the webview)", () => {
    const { container } = render(<SidePane />);
    const handle = container.querySelector(".cursor-col-resize") as HTMLElement;
    const line = handle.querySelector("div") as HTMLElement;
    fireEvent.pointerDown(handle);
    expect(line.className).toMatch(/ opacity-100$/);
    fireEvent.mouseMove(window, { clientX: 300 });
    // The pointer is lost (released outside the webview): `blur` must end
    // the drag (it does NOT leave `dragging` stuck on).
    fireEvent.blur(window);
    expect(line.className).not.toMatch(/ opacity-100$/);
    expect(localStorage.getItem("side-pane-width")).toBe("240");
  });

  it("collapses the frame to the sliver (a pending modal stays mounted) and re-opens", () => {
    const { container } = render(<SidePane />);
    // The module seeds the flag from localStorage at import — the flag is
    // false, the frame is at its width.
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe("320px");
    act(() => {
      setSidePaneCollapsed(true);
    });
    // The `w-0 overflow-hidden` mechanism (NOT `display: none` / unmount).
    expect(frame.style.width).toBe(`${SIDE_PANE_RAIL}px`);
    expect(frame.className).toContain("overflow-hidden");
    // A pending subagent `password` request's `SudoPasswordModal` is STILL
    // in the document (the pane is clipped, not hidden).
    act(() => {
      useSubagents.getState().addSession(entry);
      useInteractive.getState().addRequest("sub1", {
        requestId: "r1",
        method: "password",
        source: "subagent:reviewer",
        params: { command: "apt install ripgrep", reason: "install the tool" },
      });
    });
    expect(screen.getByText("Sudo password required")).toBeTruthy();
    act(() => {
      setSidePaneCollapsed(false);
    });
    expect(frame.style.width).toBe("320px");
  });

  it("the right rail is WIDE ENOUGH to show its toggle (the sliver is a guarantee, not a vibe)", () => {
    // The entire design rests on one number: a collapsed pane keeps a sliver
    // of width so its bottom toggle stays on screen. jsdom never clips, so NO
    // DOM assertion here can notice a rail that shrank below the button — the
    // toggle would silently vanish while every test stayed green and the pane
    // became unopenable (no keyboard shortcut exists). Hence arithmetic:
    //   24px button (`size-6`) + 8px inset/padding + 8px clear of the edge.
    expect(SIDE_PANE_RAIL).toBeGreaterThanOrEqual(24 + 8 + 8);
  });
});
