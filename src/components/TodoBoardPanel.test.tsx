import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, renderHook, screen } from "@testing-library/react";
import TodoBoardPanel, { useMainTodoItems } from "./TodoBoardPanel";
import { useBridge } from "../store/bridge";
import { useSessions } from "../store/sessions";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    respondBridgeRequest: vi.fn().mockResolvedValue(undefined),
    respondPermission: vi.fn().mockResolvedValue(undefined),
  };
});

beforeEach(() => {
  useBridge.getState().dismissSession("main1");
  useSessions.setState({ messages: {}, activeSessionId: null });
});

describe("TodoBoardPanel", () => {
  it("renders the progress header N/M with a progress indicator for a main column", () => {
    // 1 done / 3 total.
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "done item", status: "completed" },
        { content: "active item", status: "in_progress" },
        { content: "later item", status: "pending" },
      ],
    });
    const { container } = render(<TodoBoardPanel sessionId="main1" />);
    expect(screen.getByText("1/3")).toBeTruthy();
    const indicator = container.querySelector<HTMLElement>(
      '[data-slot="progress-indicator"]',
    );
    expect(indicator).not.toBeNull();
    // The ported `progress.tsx` normalizes `value` to 0–100 and applies it
    // ONLY to the indicator's `width` style.
    expect(Math.abs(parseFloat(indicator!.style.width) - 100 / 3)).toBeLessThan(
      0.01,
    );
  });

  it("renders the three-state indicators (done / in-progress / pending)", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "done item", status: "completed" },
        { content: "active item", status: "in_progress" },
        { content: "later item", status: "pending" },
      ],
    });
    render(<TodoBoardPanel sessionId="main1" />);
    // Done: a `size-4` circle with a check icon, `text-success`.
    const doneRow = screen.getByText("done item").parentElement;
    const circle = doneRow?.querySelector(".text-success");
    expect(circle).not.toBeNull();
    expect(circle!.className).toContain("size-4");
    expect(circle!.querySelector("svg")).not.toBeNull();
    // In-progress: `◉` in `text-warning`.
    const inProgress = screen.getByText("◉");
    expect(inProgress.className).toContain("text-warning");
    // Pending: `○` in `text-foreground-subtlest`.
    const pending = screen.getByText("○");
    expect(pending.className).toContain("text-foreground-subtlest");
  });

  it("renders the empty state when there are no todos", () => {
    const { container } = render(<TodoBoardPanel sessionId="main1" />);
    const empty = screen.getByText("No todos yet");
    expect(empty.className).toContain("text-ui-sm");
    expect(empty.className).toContain("text-foreground-subtlest");
    // The progress header is NOT rendered for an empty board (M = 0).
    expect(
      container.querySelector('[data-slot="progress-indicator"]'),
    ).toBeNull();
  });

  it("renders subagent todo columns as indented sub-rows", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [{ content: "main item", status: "pending" }],
    });
    useBridge.getState().applyTodoUpdate("main1", {
      source: "subagent:explorer",
      todos: [
        { content: "sub task one", status: "in_progress" },
        { content: "sub task two", status: "pending" },
      ],
    });
    render(<TodoBoardPanel sessionId="main1" />);
    // The source label (the column header).
    const header = screen.getByText("subagent:explorer");
    expect(header.className).toContain("text-ui-xs");
    expect(header.className).toContain("text-foreground-subtlest");
    // The indented sub-rows.
    const block = header.parentElement;
    expect(block?.className).toContain("pl-6");
    const row = screen.getByText("sub task one").parentElement;
    expect(row?.className).toContain("text-ui-sm");
    expect(screen.getByText("sub task two")).toBeTruthy();
  });

  it("useMainTodoItems returns the rawInput fallback when the bridge column is absent", () => {
    // A non-bridge agent: no `todos_update` column, only a
    // `manage_todo_list` tool-call in the session's transcript.
    useSessions.getState().applySessionUpdate("main1", {
      sessionUpdate: "tool_call",
      toolCallId: "t1",
      title: "manage_todo_list",
      status: "in_progress",
      rawInput: {
        operation: "write",
        todoList: [
          { content: "raw one", status: "pending" },
          { content: "raw two", status: "completed" },
        ],
      },
    });
    const { result } = renderHook(() => useMainTodoItems("main1"));
    expect(result.current.map((t) => t.content)).toEqual([
      "raw one",
      "raw two",
    ]);
  });
});
