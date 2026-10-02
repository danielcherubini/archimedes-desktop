import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useSubagents } from "../store/subagents";
import { useInteractive } from "../store/interactive";
import { usePermissions } from "../store/permissions";
import { usePendingSubagentRequests } from "./usePendingSubagentRequests";

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
  useInteractive.getState().dismissSession("main1");
  usePermissions.getState().dismissSessionPrompts("sub1");
  usePermissions.getState().dismissSessionPrompts("main1");
});

describe("usePendingSubagentRequests", () => {
  it("returns 0 with no subagent entries", () => {
    const { result } = renderHook(() => usePendingSubagentRequests());
    expect(result.current).toBe(0);
  });

  it("returns 1 with a subagent entry and a pending ask request for its session", () => {
    useSubagents.getState().addSession(entry);
    useInteractive.getState().addRequest("sub1", {
      requestId: "r1",
      method: "ask",
      source: "subagent:reviewer",
      params: { question: "which library?" },
    });
    const { result } = renderHook(() => usePendingSubagentRequests());
    expect(result.current).toBe(1);
  });

  it("returns 2 with a pending ask and a pending password request", () => {
    useSubagents.getState().addSession(entry);
    useInteractive.getState().addRequest("sub1", {
      requestId: "r1",
      method: "ask",
      source: "subagent:reviewer",
      params: { question: "which library?" },
    });
    useInteractive.getState().addRequest("sub1", {
      requestId: "r2",
      method: "password",
      source: "subagent:reviewer",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    const { result } = renderHook(() => usePendingSubagentRequests());
    expect(result.current).toBe(2);
  });

  it("returns 0 after removeRequest / removePrompt settle the requests", () => {
    useSubagents.getState().addSession(entry);
    useInteractive.getState().addRequest("sub1", {
      requestId: "r1",
      method: "confirm",
      source: "subagent:reviewer",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    usePermissions.getState().addPrompt("sub1", "p1", {
      toolCall: { title: "Run rm -rf" },
      options: [{ optionId: "allow_once", name: "Allow once" }],
    });
    const { result } = renderHook(() => usePendingSubagentRequests());
    expect(result.current).toBe(2);
    // `act`: the store change must trigger a re-render for the hook to
    // pick up the settled request (a bare `getState()` mutation outside
    // `act` would leave `result.current` stale).
    act(() => {
      useInteractive.getState().removeRequest("sub1", "r1");
    });
    expect(result.current).toBe(1);
    act(() => {
      usePermissions.getState().removePrompt("sub1", "p1");
    });
    expect(result.current).toBe(0);
  });

  it("does NOT count requests for sessions with no subagent entry (the main session)", () => {
    useInteractive.getState().addRequest("main1", {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: { question: "which library?" },
    });
    const { result } = renderHook(() => usePendingSubagentRequests());
    expect(result.current).toBe(0);
  });
});
