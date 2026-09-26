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

  it("dismisses an entry via keyboard (Enter on the focused dismiss button)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentPanel />);
    const dismissButton = screen.getByRole("button", { name: "Dismiss reviewer" });
    // The keydown bubbles to the header's `onKeyDown`, which must NOT
    // `preventDefault` it — in a real browser that cancels the button's
    // native Enter activation, so keyboard users could never dismiss.
    // jsdom does not perform the keydown default action, so simulate the
    // browser: the button activates natively IFF the default was not
    // prevented.
    let defaultPrevented = false;
    const onKey = (e: KeyboardEvent) => {
      defaultPrevented = e.defaultPrevented;
    };
    document.addEventListener("keydown", onKey);
    fireEvent.keyDown(dismissButton, { key: "Enter" });
    document.removeEventListener("keydown", onKey);
    if (!defaultPrevented) {
      fireEvent.click(dismissButton); // the browser's native activation
    }
    // Dismissed — the entry is gone from the store / the section unmounted
    // (NOT merely toggled by the header's handler, which would leave the
    // entry in the store).
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
    // A completed section STARTS COLLAPSED (the auto-collapse behavior) —
    // expand the header row (the agent-name's `cursor-pointer` ancestor)
    // before asserting the stream content.
    const header = screen.getByText("reviewer").closest("div.cursor-pointer");
    expect(header).not.toBeNull();
    fireEvent.click(header!);
    expect(screen.getByText("Thought")).toBeTruthy();
  });

  it("auto-collapses a subagent section when the session finishes", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentPanel />);
    // Running → the stream is visible.
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
    // The session finishes (edge-triggered auto-collapse).
    act(() => {
      useSubagents.getState().markClosed("sub1", "completed", undefined, {
        inputTokens: 0,
        outputTokens: 0,
        cost: 0,
        durationMs: 100,
      });
    });
    // The stream content is GONE; the header (agent name) is still visible.
    expect(screen.queryByText("hello from the subagent")).toBeNull();
    expect(screen.getByText("reviewer")).toBeTruthy();
  });

  it("lets the user re-open a collapsed section", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentPanel />);
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
    act(() => {
      useSubagents.getState().markClosed("sub1", "completed", undefined, {
        inputTokens: 0,
        outputTokens: 0,
        cost: 0,
        durationMs: 100,
      });
    });
    expect(screen.queryByText("hello from the subagent")).toBeNull();
    // Click the header row (the agent-name's `cursor-pointer` ancestor).
    const header = screen.getByText("reviewer").closest("div.cursor-pointer");
    expect(header).not.toBeNull();
    fireEvent.click(header!);
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
  });

  it("starts collapsed for a session that is already finished", () => {
    useSubagents.getState().addSession({ ...entry, status: "completed" });
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentPanel />);
    // Already finished on first render → the stream is NOT visible.
    expect(screen.queryByText("hello from the subagent")).toBeNull();
    expect(screen.getByText("reviewer")).toBeTruthy();
  });

  it("toggles a collapsed section via keyboard (Enter / space on the header)", () => {
    useSubagents.getState().addSession({ ...entry, status: "completed" });
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentPanel />);
    // Already finished → starts collapsed.
    expect(screen.queryByText("hello from the subagent")).toBeNull();
    // The header row is a `role="button"` whose accessible name is its text
    // content (agent name + status word); the dismiss `X` is a separate
    // button with its own aria-label, so the exact name targets the header.
    const header = screen.getByRole("button", { name: "reviewer completed" });
    fireEvent.keyDown(header, { key: "Enter" });
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
    // Space toggles too (with `preventDefault` — no page scroll).
    fireEvent.keyDown(header, { key: " " });
    expect(screen.queryByText("hello from the subagent")).toBeNull();
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
