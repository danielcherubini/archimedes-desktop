import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  listAgents,
  startSession,
  type SessionInfo,
  type SpaceRow,
} from "../lib/tauri";
import { spaceViewFor, useSessions } from "../store/sessions";
import { useStartNewConversation } from "./useStartNewConversation";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    startSession: vi.fn().mockResolvedValue({
      sessionId: "new1",
      agentId: "pi",
      cwd: "/tmp/ws",
      capabilities: { loadSession: false },
    }),
    listAgents: vi.fn().mockResolvedValue([{ id: "pi", name: "pi" }]),
    respondPermission: vi.fn(),
    respondBridgeRequest: vi.fn(),
    loadHistory: vi.fn().mockResolvedValue([]),
  };
});

const mockedStartSession = vi.mocked(startSession);
const mockedListAgents = vi.mocked(listAgents);

/**
 * Fixture data. `spaceViewFor` for the `spaces` row below yields a view
 * whose `liveSessionId` matches the fixture live session (agentId
 * `a-live`) and whose `storedSessionIds[0]` matches the fixture stored
 * session (agentId `a-stored`).
 */
const spaces: SpaceRow[] = [
  { path: "/tmp/alpha", createdAt: 1, lastOpenedAt: 1 },
];
const sessions: SessionInfo[] = [
  { sessionId: "s-live", agentId: "a-live", cwd: "/tmp/alpha", capabilities: {} },
];
const historySessions: SessionInfo[] = [
  { sessionId: "s-stored", agentId: "a-stored", cwd: "/tmp/alpha", capabilities: {} },
];
const view = spaceViewFor(spaces[0]!, sessions, historySessions, {});

beforeEach(() => {
  // Seed via `setState` directly — NOT `getState().setSpaces(…)` (that
  // triggers boot auto-selection + `openSession` → `loadHistory` IPC).
  useSessions.setState({
    spaces,
    sessions,
    historySessions,
    activeSessionId: "s1",
    closeReasons: {},
    messages: {},
    inTurn: {},
    stopReasons: {},
  });
  vi.clearAllMocks();
});

describe("useStartNewConversation", () => {
  // FIRST test in the file on purpose: the shared fetch is a module-level
  // memo, so the very first test sees a cold cache. (Two hook instances ⇒
  // `listAgents` must be called exactly once.)
  it("shares ONE `listAgents` fetch across hook instances", async () => {
    renderHook(() => useStartNewConversation(view));
    renderHook(() => useStartNewConversation(view));
    // Let both `useEffect`s + the memoized promise settle.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(mockedListAgents).toHaveBeenCalledTimes(1);
  });

  it("guards against double-invocation (two rapid calls start ONE session)", async () => {
    const { result } = renderHook(() => useStartNewConversation(view));
    await act(async () => {
      // Fire both WITHOUT awaiting the first — the second must be
      // suppressed by the `inFlight` guard, not create a second session.
      void result.current.startNewConversation();
      void result.current.startNewConversation();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(mockedStartSession).toHaveBeenCalledTimes(1);
  });

  it("starts with the live session's agentId (the fallback chain's first hit) and records the session + space", async () => {
    const { result } = renderHook(() => useStartNewConversation(view));
    await act(async () => {
      await result.current.startNewConversation();
    });
    expect(mockedStartSession).toHaveBeenCalledWith("a-live", "/tmp/alpha");
    // `addSession` + `addSpace` on success.
    expect(useSessions.getState().sessions).toContainEqual(
      expect.objectContaining({ sessionId: "new1" }),
    );
    expect(useSessions.getState().spaces).toContainEqual(
      expect.objectContaining({ path: "/tmp/ws" }),
    );
    // `addSession` always selects the session that was just started.
    expect(useSessions.getState().activeSessionId).toBe("new1");
  });

  it("falls back to the stored session's agentId when the live session is absent", async () => {
    const { result } = renderHook(() =>
      useStartNewConversation({ ...view, liveSessionId: null }),
    );
    await act(async () => {
      await result.current.startNewConversation();
    });
    expect(mockedStartSession).toHaveBeenCalledWith("a-stored", "/tmp/alpha");
  });

  it("no-ops when no agents are loaded (empty agentId)", async () => {
    // No live session and no stored session match → `agentId` is the
    // `firstAgentId` fallback, which is `""` until `listAgents` has loaded.
    const { result } = renderHook(() =>
      useStartNewConversation({
        ...view,
        liveSessionId: null,
        storedSessionIds: [],
      }),
    );
    await act(async () => {
      await result.current.startNewConversation();
    });
    expect(mockedStartSession).not.toHaveBeenCalled();
  });

  it("no-ops when the view is undefined", async () => {
    const { result } = renderHook(() => useStartNewConversation(undefined));
    await act(async () => {
      await result.current.startNewConversation();
    });
    expect(mockedStartSession).not.toHaveBeenCalled();
  });

  it("captures the start error and `clearError` clears it", async () => {
    mockedStartSession.mockRejectedValueOnce(new Error("boom"));
    const { result } = renderHook(() => useStartNewConversation(view));
    expect(result.current.error).toBeNull();
    await act(async () => {
      await result.current.startNewConversation();
    });
    expect(result.current.error).toBe("boom");
    act(() => {
      result.current.clearError();
    });
    expect(result.current.error).toBeNull();
  });
});
