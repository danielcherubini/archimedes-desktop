import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import SidePane from "./SidePane";
import { useSubagents } from "../store/subagents";
import { useBridge } from "../store/bridge";
import { useSessions } from "../store/sessions";
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

beforeEach(() => {
  setSidePaneCollapsed(false);
  localStorage.clear();
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
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

  it("collapses the frame to width 0 (a pending modal stays mounted) and re-opens", () => {
    const { container } = render(<SidePane />);
    // The mount-time hydration from the empty localStorage ran first and is
    // a same-value echo — the flag is false, the frame is at its width.
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
