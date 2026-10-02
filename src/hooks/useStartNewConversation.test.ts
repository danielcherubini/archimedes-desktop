import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { startSession, type SessionInfo, type SpaceRow } from "../lib/tauri";
import { spaceViewFor, useSessions } from "../store/sessions";
import { useStartNewConversation } from "./useStartNewConversation";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    startSession: vi.fn().mockResolvedValue({
      sessionId: "new1",
      cwd: "/tmp/ws",
      capabilities: { loadSession: false },
      archived: false,
    }),
    respondPermission: vi.fn(),
    respondInteractiveRequest: vi.fn(),
    loadHistory: vi.fn().mockResolvedValue([]),
  };
});

const mockedStartSession = vi.mocked(startSession);

/**
 * Fixture data. `spaceViewFor` for the `spaces` row below yields a view
 * whose `liveSessionId` matches the fixture live session and whose
 * `storedSessionIds[0]` matches the fixture stored session.
 */
const spaces: SpaceRow[] = [
  { path: "/tmp/alpha", createdAt: 1, lastOpenedAt: 1, trusted: false },
];
const sessions: SessionInfo[] = [
  { sessionId: "s-live", cwd: "/tmp/alpha", capabilities: {}, archived: false },
];
const historySessions: SessionInfo[] = [
  { sessionId: "s-stored", cwd: "/tmp/alpha", capabilities: {}, archived: false },
];
const view = spaceViewFor(spaces[0]!, sessions, historySessions, {}, []);

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
  it("starts a native session in the Space (one harness — no agent to choose) and records the session + space", async () => {
    const { result } = renderHook(() => useStartNewConversation(view));
    await act(async () => {
      await result.current.startNewConversation();
    });
    // The call is `startSession(view.path)` — the backend canonicalizes.
    expect(mockedStartSession).toHaveBeenCalledWith("/tmp/alpha");
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
