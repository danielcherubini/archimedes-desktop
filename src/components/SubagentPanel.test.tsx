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
  it("renders nothing when there are no entries", () => {
    const { container } = render(<SubagentPanel />);
    expect(container.firstChild).toBeNull();
  });

  it("auto-expands when the first entry arrives (0 → >0)", () => {
    const { container } = render(<SubagentPanel />);
    expect(container.firstChild).toBeNull();
    act(() => {
      useSubagents.getState().addSession(entry);
    });
    // The 0 → >0 transition flipped the expanded state: the section is
    // visible WITHOUT a manual expand click.
    expect(screen.getByText("reviewer")).toBeTruthy();
  });

  it("renders a header with the agentName, the state chip, and the status for a running entry", () => {
    useSubagents.getState().addSession(entry);
    useBridge.getState().applyState("sub1", { state: "working" });
    render(<SubagentPanel />);
    // The rail starts collapsed (a slim bar with the count badge) — expand it.
    fireEvent.click(screen.getByRole("button", { name: "Expand subagents" }));
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("working")).toBeTruthy();
    expect(screen.getByText("running")).toBeTruthy();
  });

  it("auto-expands on a pending permission prompt (badge 0 → >0) and shows the badge", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentPanel />);
    // Collapsed: the section is hidden.
    expect(screen.queryByText("reviewer")).toBeNull();
    act(() => {
      usePermissions.getState().addPrompt("sub1", "p1", {
        toolCall: { title: "Bash (apt install ripgrep)" },
        options: [{ optionId: "allow", name: "Allow" }],
      });
    });
    // The 0 → >0 transition auto-expanded the panel.
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
    fireEvent.click(screen.getByRole("button", { name: "Expand subagents" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Expand subagents" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Expand subagents" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Expand subagents" }));
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
  });

  it("dismisses an entry via the header's dismiss button", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Expand subagents" }));
    fireEvent.click(screen.getByRole("button", { name: "Dismiss reviewer" }));
    expect(useSubagents.getState().entries["sub1"]).toBeUndefined();
    expect(screen.queryByText("reviewer")).toBeNull();
  });
});
