import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import SidePane from "./SidePane";
import { useSubagents } from "../store/subagents";
import { useBridge } from "../store/bridge";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { setSidePaneCollapsed } from "../lib/sidePaneState";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    respondBridgeRequest: vi.fn().mockResolvedValue(undefined),
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
  useBridge.getState().dismissSession("sub1");
  useBridge.getState().dismissSession("main1");
  useSessions.setState({ messages: {}, activeSessionId: null });
});

/** The subagents `TabsContent` (located via its content text). */
function subagentsContent(): Element {
  return screen
    .getByText("No subagent sessions")
    .closest('[data-slot="tabs-content"]')!;
}

/** The Radix `data-state` of the subagents content ("active"/"inactive"). */
function subagentsContentState(): string {
  return subagentsContent().getAttribute("data-state") ?? "";
}

describe("SidePane", () => {
  it("renders the two tab triggers", () => {
    render(<SidePane />);
    expect(screen.getByRole("tab", { name: /Todos/ })).toBeTruthy();
    expect(screen.getByRole("tab", { name: /Subagents/ })).toBeTruthy();
  });

  it("clicking Subagents swaps the content to the subagent panel", () => {
    render(<SidePane />);
    // The `todos` tab is active by default: the subagent content is
    // inactive (force-mounted — it stays in the document, visually hidden).
    expect(subagentsContentState()).toBe("inactive");
    // Radix's `TabsTrigger` activates on `mousedown` (not `click`).
    fireEvent.mouseDown(screen.getByRole("tab", { name: /Subagents/ }));
    expect(subagentsContentState()).toBe("active");
  });

  it("persists the selected tab to localStorage", () => {
    render(<SidePane />);
    fireEvent.mouseDown(screen.getByRole("tab", { name: /Subagents/ }));
    expect(localStorage.getItem("side-pane-tab")).toBe("subagents");
  });

  it("restores the selected tab from localStorage on mount", () => {
    localStorage.setItem("side-pane-tab", "subagents");
    render(<SidePane />);
    expect(subagentsContentState()).toBe("active");
  });

  it("shows a count badge on the inactive Todos trigger when the session has todos", () => {
    useSessions.setState({ activeSessionId: "main1" });
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "a", status: "pending" },
        { content: "b", status: "completed" },
      ],
    });
    render(<SidePane />);
    // The badge shows only on the INACTIVE trigger.
    expect(screen.queryByText("2")).toBeNull();
    fireEvent.mouseDown(screen.getByRole("tab", { name: /Subagents/ }));
    expect(screen.getByText("2")).toBeTruthy();
  });

  it("shows a 'Waiting' pill on the Subagents trigger while a subagent has a pending request (replacing the count badge)", () => {
    useSubagents.getState().addSession(entry);
    useBridge.getState().addRequest("sub1", {
      requestId: "r1",
      method: "ask",
      source: "subagent:reviewer",
      params: { question: "which library?" },
    });
    render(<SidePane />);
    const trigger = screen.getByRole("tab", { name: /Subagents/ });
    // The SAME green "Waiting" treatment as the sidebar session row's badge.
    expect(trigger.textContent).toContain("Waiting");
    const pill = trigger.querySelector(".bg-success\\/14")!;
    expect(pill.className).toContain("text-success");
    // The entry-count badge is NOT shown while a request is pending.
    expect(screen.queryByText("1")).toBeNull();
  });

  it("shows the 'Waiting' pill on a pending permission prompt too (and drops it once settled)", () => {
    useSubagents.getState().addSession(entry);
    act(() => {
      usePermissions.getState().addPrompt("sub1", "p1", {
        sessionId: "sub1",
        toolCall: { title: "Run rm -rf" },
        options: [{ optionId: "allow_once", name: "Allow once", kind: "allow_once" }],
      });
    });
    const { rerender } = render(<SidePane />);
    expect(screen.getByRole("tab", { name: /Subagents/ }).textContent).toContain(
      "Waiting",
    );
    act(() => {
      usePermissions.getState().removePrompt("sub1", "p1");
    });
    rerender(<SidePane />);
    // Settled: the entry-count badge is back.
    expect(screen.getByRole("tab", { name: /Subagents/ }).textContent).toContain(
      "1",
    );
  });

  it("keeps the entry-count badge when there are entries but NO pending requests", () => {
    useSubagents.getState().addSession(entry);
    render(<SidePane />);
    const trigger = screen.getByRole("tab", { name: /Subagents/ });
    expect(trigger.textContent).not.toContain("Waiting");
    expect(trigger.textContent).toContain("1");
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

  it("collapses the frame to width 0 (a pending modal stays mounted) and re-opens", () => {
    const { container } = render(<SidePane />);
    // The module seeds the flag from localStorage at import — the flag is
    // false, the frame is at its width.
    const frame = container.firstChild as HTMLElement;
    expect(frame.style.width).toBe("320px");
    act(() => {
      setSidePaneCollapsed(true);
    });
    // The `w-0 overflow-hidden` mechanism (NOT `display: none` / unmount).
    expect(frame.style.width).toBe("0px");
    expect(frame.className).toContain("overflow-hidden");
    // A pending subagent `password` request's `SudoPasswordModal` is STILL
    // in the document (the pane is clipped, not hidden).
    act(() => {
      useSubagents.getState().addSession(entry);
      useBridge.getState().addRequest("sub1", {
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
});
