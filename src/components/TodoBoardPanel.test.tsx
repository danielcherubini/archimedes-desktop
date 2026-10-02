import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, renderHook, screen } from "@testing-library/react";
import TodoBoardPanel, { useMainTodoItems, useMainOpenTodoCount } from "./TodoBoardPanel";
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
  it("renders the section header (the Todos label + N/M) with a progress indicator for a main column", () => {
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
    // The section label (the `SidePane` hosts the board as a section — the
    // ZCode `Goal`/`Progress` treatment: the header carries the label +
    // the count, the body the progress bar + checklist).
    expect(screen.getByText("Todos")).toBeTruthy();
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

  it("renders NOTHING when there are no todos (the board is visible only while open todos exist)", () => {
    const { container } = render(<TodoBoardPanel sessionId="main1" />);
    // No "No todos yet" placeholder — an empty board is hidden entirely.
    expect(screen.queryByText("No todos yet")).toBeNull();
    expect(container.firstChild).toBeNull();
  });

  it("renders NOTHING when ALL main todos are completed (the board hides once the list is done)", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "done one", status: "completed" },
        { content: "done two", status: "completed" },
      ],
    });
    const { container } = render(<TodoBoardPanel sessionId="main1" />);
    // No progress header, no checklist rows — a fully-completed board is
    // hidden (it carries no attention cue; the badge is gone too, via the
    // same open-count derivation).
    expect(
      container.querySelector('[data-slot="progress-indicator"]'),
    ).toBeNull();
    expect(screen.queryByText("done one")).toBeNull();
    expect(container.firstChild).toBeNull();
  });

  it("hides a subagent todo column when ALL of its todos are completed (open columns still render)", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "subagent:done-agent",
      todos: [
        { content: "sub done one", status: "completed" },
        { content: "sub done two", status: "completed" },
      ],
    });
    useBridge.getState().applyTodoUpdate("main1", {
      source: "subagent:busy-agent",
      todos: [
        { content: "sub open one", status: "in_progress" },
        { content: "sub open two", status: "completed" },
      ],
    });
    const { container } = render(<TodoBoardPanel sessionId="main1" />);
    // The all-completed column is hidden (no header, no rows)...
    expect(screen.queryByText("subagent:done-agent")).toBeNull();
    expect(screen.queryByText("sub done one")).toBeNull();
    // ...while the column with an open item still renders.
    expect(screen.getByText("subagent:busy-agent")).toBeTruthy();
    expect(screen.getByText("sub open one")).toBeTruthy();
    expect(container.firstChild).not.toBeNull();
  });

  it("shows the subagent column(s) when the main list is fully completed but a subagent column is still open", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [{ content: "main done", status: "completed" }],
    });
    useBridge.getState().applyTodoUpdate("main1", {
      source: "subagent:busy-agent",
      todos: [{ content: "sub open", status: "pending" }],
    });
    render(<TodoBoardPanel sessionId="main1" />);
    // The main section (progress header + checklist) is hidden (all done)...
    expect(screen.queryByText("main done")).toBeNull();
    // ...but the open subagent column keeps the board visible.
    expect(screen.getByText("subagent:busy-agent")).toBeTruthy();
    expect(screen.getByText("sub open")).toBeTruthy();
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

  it("useMainOpenTodoCount returns the OPEN (non-completed) main todo count (zero when fully completed)", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        { content: "a", status: "pending" },
        { content: "b", status: "completed" },
        { content: "c", status: "in_progress" },
        { content: "d", status: "completed" },
      ],
    });
    const { result, rerender } = renderHook(() => useMainOpenTodoCount("main1"));
    expect(result.current).toBe(2);
    // A fully-completed list counts as ZERO (the badge and the panel's
    // visibility share this derivation — they can never disagree).
    act(() => {
      useBridge.getState().applyTodoUpdate("main1", {
        source: "main",
        todos: [
          { content: "a", status: "completed" },
          { content: "b", status: "completed" },
        ],
      });
    });
    rerender();
    expect(result.current).toBe(0);
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

  it("renders multi-line todo items with wrapping and min-height rather than fixed height", () => {
    useBridge.getState().applyTodoUpdate("main1", {
      source: "main",
      todos: [
        {
          content:
            "Task 1: Fix hardcoded discriminator literal in UserPartyRestrictionDao and ensure proper wrapping",
          status: "in_progress",
        },
      ],
    });
    render(<TodoBoardPanel sessionId="main1" />);
    const textNode = screen.getByText(/Task 1: Fix hardcoded discriminator/);
    expect(textNode.className).toContain("break-words");
    const row = textNode.closest("li");
    expect(row).not.toBeNull();
    expect(row!.className).toContain("min-h-8");
    expect(row!.className).toContain("items-start");
    expect(row!.classList.contains("h-8")).toBe(false);
  });
});
