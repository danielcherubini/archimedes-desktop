import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { SubagentTranscript } from "./SubagentTranscript";
import { useSubagents } from "../store/subagents";
import { useInteractive } from "../store/interactive";
import { usePermissions } from "../store/permissions";
import { useSessions } from "../store/sessions";

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

beforeEach(() => {
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  useInteractive.getState().dismissSession("sub1");
  useInteractive.getState().dismissSession("sub2");
  useInteractive.getState().dismissSession("main1");
  usePermissions.getState().dismissSessionPrompts("sub1");
  usePermissions.getState().dismissSessionPrompts("sub2");
  usePermissions.getState().dismissSessionPrompts("main1");
  useSessions.setState({ messages: {} });
});

describe("SubagentTranscript (the read-only session transcript, expanded inline in the directory row)", () => {
  it("renders the agent-text stream (visible by default — the expansion IS the detail view, no auto-collapse)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentTranscript sessionId="sub1" />);
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
  });

  it("keeps the stream visible when the session finishes (no auto-collapse — the expansion is the detail view)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentTranscript sessionId="sub1" />);
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
    act(() => {
      useSubagents.getState().markClosed("sub1", "completed", undefined, {
        inputTokens: 0,
        outputTokens: 0,
        cost: 0,
        durationMs: 100,
      });
    });
    // The entry is read FROM THE STORE — the `act` re-rendered the
    // transcript with the closed entry: the stream stays visible (the
    // stream is ALWAYS visible — the expansion IS the detail view) and the
    // metrics line appears.
    expect(screen.getByText("hello from the subagent")).toBeTruthy();
    expect(screen.getByText(/100 ms/)).toBeTruthy();
  });

  it("renders a closed entry's metrics line from useSubagents (surviving useInteractive.dismissSession)", () => {
    useSubagents.getState().addSession(entry);
    useSubagents.getState().markClosed("sub1", "completed", undefined, {
      inputTokens: 0,
      outputTokens: 0,
      cost: 0,
      durationMs: 1234,
    });
    render(<SubagentTranscript sessionId="sub1" />);
    // The metrics line (the status word lives in the DIRECTORY row — the
    // transcript's header was dropped: the row IS the header).
    expect(screen.getByText(/1234 ms/)).toBeTruthy();
    // The race: the driver teardown's `session-closed` fires the interactive
    // store's `dismissSession` for the same id (DELETING `cost`/`agentState`).
    // The metrics line is read from the `subagent-closed` SNAPSHOT in
    // `useSubagents` — it must survive the interactive-store deletion.
    act(() => {
      useInteractive.getState().dismissSession("sub1");
    });
    expect(screen.getByText(/1234 ms/)).toBeTruthy();
  });

  it("renders a failed entry's error", () => {
    useSubagents.getState().addSession({ ...entry, status: "failed" });
    useSubagents.getState().markClosed("sub1", "failed", "boom");
    render(<SubagentTranscript sessionId="sub1" />);
    // The error text (the status word lives in the DIRECTORY row).
    expect(screen.getByText("boom")).toBeTruthy();
  });

  it("renders an `ask` request for the subagent session id as an AskQuestionCard", () => {
    useSubagents.getState().addSession(entry);
    // The subagent's OWN `ask` carries `source: "main"` (keyed by the
    // subagent's own session id) — the header provides the name.
    useInteractive.getState().addRequest("sub1", {
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
    render(<SubagentTranscript sessionId="sub1" />);
    expect(screen.getByText("Which color?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Red" })).toBeTruthy();
  });

  it("renders a permission prompt for the subagent session", () => {
    useSubagents.getState().addSession(entry);
    act(() => {
      usePermissions.getState().addPrompt("sub1", "p1", {
        toolCall: { title: "Bash (apt install ripgrep)" },
        options: [{ optionId: "allow", name: "Allow" }],
      });
    });
    render(<SubagentTranscript sessionId="sub1" />);
    // The prompt card renders in the transcript (the panel is the ONLY
    // render site for subagent-session requests).
    expect(screen.getByText("Bash (apt install ripgrep)")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Allow" })).toBeTruthy();
  });

  it("renders thinking (streaming — expanded)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: "pondering" },
      messageId: "m1",
    });
    render(<SubagentTranscript sessionId="sub1" />);
    expect(screen.getByText("Thinking")).toBeTruthy();
    expect(screen.getByText("pondering")).toBeTruthy();
  });

  it("renders thinking (completed — auto-collapsed, the main-chat treatment)", () => {
    useSubagents.getState().addSession({ ...entry, status: "completed" });
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: "done" },
      messageId: "m1",
    });
    render(<SubagentTranscript sessionId="sub1" />);
    // Completed reasoning auto-collapses (the SAME treatment as the main
    // chat column): the trigger is visible, the content is not.
    expect(screen.getByText("Thought")).toBeTruthy();
    expect(screen.queryByText("done")).toBeNull();
  });
});
