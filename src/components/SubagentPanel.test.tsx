import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import SubagentPanel from "./SubagentPanel";
import { useSubagents } from "../store/subagents";
import { useBridge } from "../store/bridge";
import { usePermissions } from "../store/permissions";
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

const entry = {
  sessionId: "sub1",
  parentSessionId: "main1",
  agentName: "reviewer",
  task: "review the diff",
  status: "running" as const,
};

beforeEach(() => {
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  useBridge.getState().dismissSession("sub1");
  useBridge.getState().dismissSession("main1");
  usePermissions.getState().dismissSessionPrompts("sub1");
  usePermissions.getState().dismissSessionPrompts("main1");
  useSessions.setState({ messages: {} });
});

describe("SubagentPanel", () => {
  it("renders the empty state when there are no entries", () => {
    render(<SubagentPanel />);
    expect(screen.getByText("No subagent sessions")).toBeTruthy();
  });

  it("renders a new entry's section", () => {
    const { container } = render(<SubagentPanel />);
    act(() => {
      useSubagents.getState().addSession(entry);
    });
    expect(screen.getByText("reviewer")).toBeTruthy();
    // The card carries a border WIDTH class alongside `border-card-border`
    // (a color class alone renders no visible border — Tailwind's reset
    // leaves `border-width: 0`). Assert the standalone `border` class via
    // a word-boundary match.
    const card = container.querySelector("section");
    expect(card).toBeTruthy();
    expect(card?.className).toMatch(/(^|\s)border(\s|$)/);
  });

  it("renders a header with the agentName, the state chip, and the status for a running entry", () => {
    useSubagents.getState().addSession(entry);
    useBridge.getState().applyState("sub1", { state: "working" });
    render(<SubagentPanel />);
    // Sections always render (the SidePane frame owns collapse now).
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("working")).toBeTruthy();
    expect(screen.getByText("running")).toBeTruthy();
  });

  it("shows the pending-permission badge", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentPanel />);
    act(() => {
      usePermissions.getState().addPrompt("sub1", "p1", {
        toolCall: { title: "Bash (apt install ripgrep)" },
        options: [{ optionId: "allow", name: "Allow" }],
      });
    });
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("1 awaiting")).toBeTruthy();
  });

  it("renders a closed entry's metrics line from useSubagents (surviving useBridge.dismissSession)", () => {
    useSubagents.getState().addSession(entry);
    useSubagents.getState().markClosed("sub1", "completed", undefined, {
      inputTokens: 0,
      outputTokens: 0,
      cost: 0,
      durationMs: 1234,
    });
    render(<SubagentPanel />);
    expect(screen.getByText("completed")).toBeTruthy();
    expect(screen.getByText(/1234 ms/)).toBeTruthy();
    // The race: the driver teardown's `session-closed` fires the bridge's
    // `dismissSession` for the same id (DELETING `cost`/`agentState`). The
    // metrics line is read from the `subagent-closed` SNAPSHOT in
    // `useSubagents` — it must survive the bridge-store deletion.
    act(() => {
      useBridge.getState().dismissSession("sub1");
    });
    expect(screen.getByText(/1234 ms/)).toBeTruthy();
    expect(screen.getByText("completed")).toBeTruthy();
  });

  it("renders a failed entry's error", () => {
    useSubagents.getState().addSession(entry);
    useSubagents.getState().markClosed("sub1", "failed", "boom");
    render(<SubagentPanel />);
    expect(screen.getByText("failed")).toBeTruthy();
    expect(screen.getByText("boom")).toBeTruthy();
  });

  it("renders an `ask` request for the subagent session id as an AskQuestionCard in the section", () => {
    useSubagents.getState().addSession(entry);
    // The subagent's OWN `ask` carries `source: "main"` (keyed by the
    // subagent's own session id) — the section header provides the name.
    useBridge.getState().addRequest("sub1", {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {
        questions: [
          {
            id: "q1",
            question: "Which color?",
            options: [{ label: "Red" }, { label: "Blue" }],
          },
        ],
      },
    });
    render(<SubagentPanel />);
    expect(screen.getByText("Which color?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Red" })).toBeTruthy();
  });

  it("renders a `confirm` request for the subagent session id as a SudoConfirmModal", () => {
    useSubagents.getState().addSession(entry);
    useBridge.getState().addRequest("sub1", {
      requestId: "r1",
      method: "confirm",
      source: "main",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    render(<SubagentPanel />);
    // `ChatStream` renders the `SudoConfirmModal` ONLY for the ACTIVE
    // session's requests — the subagent's session id is not active, so the
    // panel is the ONLY renderer (a `fixed` overlay, visible even while the
    // rail is collapsed — an unrendered request would hang until the bridge
    // timeout).
    expect(screen.getByText("Run this command with sudo?")).toBeTruthy();
    expect(screen.getByText("apt install ripgrep")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Run" })).toBeTruthy();
  });

  it("renders a `password` request for the subagent session id as a SudoPasswordModal", () => {
    useSubagents.getState().addSession(entry);
    useBridge.getState().addRequest("sub1", {
      requestId: "r1",
      method: "password",
      source: "main",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    render(<SubagentPanel />);
    expect(screen.getByText("Sudo password required")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeTruthy();
  });

  it("renders the compact stream (agent-text) for the subagent session", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentPanel />);
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
  });

  it("dismisses an entry via the header's dismiss button", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss reviewer" }));
    expect(useSubagents.getState().entries["sub1"]).toBeUndefined();
    expect(screen.queryByText("reviewer")).toBeNull();
  });

  it("renders thinking block (streaming)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "pondering" }, messageId: "m1" });
    render(<SubagentPanel />);
    expect(screen.getByText("Thinking")).toBeTruthy();
  });

  it("renders thinking block (completed)", () => {
    useSubagents.getState().addSession({ ...entry, status: "completed" });
    useSessions.getState().applySessionUpdate("sub1", { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "done" }, messageId: "m1" });
    render(<SubagentPanel />);
    expect(screen.getByText("Thought")).toBeTruthy();
  });

  it("styles the status chip per status (running → warning, failed → destructive)", () => {
    useSubagents.getState().addSession(entry);
    useSubagents.getState().addSession({
      sessionId: "sub2",
      parentSessionId: "main1",
      agentName: "worker",
      task: "work the diff",
      status: "failed",
      error: "boom",
    });
    render(<SubagentPanel />);
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("worker")).toBeTruthy();
    expect(screen.getByText("running").className).toContain("text-warning");
    expect(screen.getByText("failed").className).toContain("text-destructive");
  });
});
