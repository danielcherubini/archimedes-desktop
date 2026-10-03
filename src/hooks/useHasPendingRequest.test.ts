import { beforeEach, describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";
import { useHasPendingRequest } from "./useHasPendingRequest";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useInteractive } from "../store/interactive";
import { useSubagents } from "../store/subagents";

beforeEach(() => {
  useSessions.setState({
    sessions: [],
    historySessions: [],
    archivedSessions: [],
    activeSessionId: null,
    activeSpacePath: null,
    spaces: [],
    closeReasons: {},
    messages: {},
    configOptions: {},
  });
  usePermissions.setState({ prompts: {} });
  useInteractive.setState({
    requests: {},
    todos: {},
    agentState: {},
    cost: {},
    session: {},
  });
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
});

describe("useHasPendingRequest", () => {
  it("is false with no active session and no subagent requests", () => {
    const { result } = renderHook(() => useHasPendingRequest());
    expect(result.current).toBe(false);
  });

  it("is true for a pending permission prompt on the ACTIVE session", () => {
    useSessions.setState({
      sessions: [
        {
          sessionId: "s1",
          cwd: "/tmp/a",
          capabilities: {},
          archived: false,
        },
      ],
      activeSessionId: "s1",
    });
    usePermissions.getState().addPrompt("s1", "r1", {
      requestId: "r1",
      toolTitle: "bash",
      options: ["Allow", "Deny"],
    });
    const { result } = renderHook(() => useHasPendingRequest());
    expect(result.current).toBe(true);
  });

  it("is false for a prompt on a NON-active session", () => {
    usePermissions.getState().addPrompt("s-other", "r1", {
      requestId: "r1",
      toolTitle: "bash",
      options: ["Allow", "Deny"],
    });
    const { result } = renderHook(() => useHasPendingRequest());
    expect(result.current).toBe(false);
  });

  it("is true for a pending ask / confirm / password request on the ACTIVE session", () => {
    useSessions.setState({
      sessions: [
        {
          sessionId: "s1",
          cwd: "/tmp/a",
          capabilities: {},
          archived: false,
        },
      ],
      activeSessionId: "s1",
    });
    for (const method of ["ask", "confirm", "password"] as const) {
      useInteractive.getState().addRequest("s1", {
        requestId: `r-${method}`,
        method,
        params: {},
        source: "main",
      });
      const { result } = renderHook(() => useHasPendingRequest());
      expect(result.current).toBe(true);
      useInteractive.getState().removeRequest("s1", `r-${method}`);
    }
  });

  it("is true for a pending subagent request (a subagent session id is never active)", () => {
    useSubagents.getState().addSession({
      sessionId: "sub1",
      parentSessionId: "main1",
      agentName: "reviewer",
      task: "review",
      status: "running",
    });
    useInteractive.getState().addRequest("sub1", {
      requestId: "r1",
      method: "password",
      params: {},
      source: "main",
    });
    const { result } = renderHook(() => useHasPendingRequest());
    expect(result.current).toBe(true);
  });
});
