import { beforeEach, describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useSessionStatus } from "./useSessionStatus";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useInteractive } from "../store/interactive";

const live = (sessionId: string) => ({
  sessionId,
  cwd: "/w",
  capabilities: {},
  archived: false,
});

beforeEach(() => {
  useSessions.setState({
    sessions: [],
    historySessions: [],
    archivedSessions: [],
    activeSessionId: null,
    inTurn: {},
    messages: {},
  });
  usePermissions.setState({ prompts: {} });
  useInteractive.setState({ requests: {} });
});

/**
 * The rail's whole job is to say the same thing the row says, in less space —
 * so the derivation is ONE hook with two consumers (the open `SessionRow`'s
 * spinner + Waiting pill, and the collapsed rail's mark). These tests pin the
 * four states, because a rail that disagrees with the list it summarises is
 * worse than no rail.
 */
describe("useSessionStatus (one derivation, the list row AND the rail)", () => {
  const of = (id: string) => renderHook(() => useSessionStatus(id)).result.current;

  it("stored → stored (nothing live, nothing pending)", () => {
    useSessions.setState({ historySessions: [live("s1")] });
    expect(of("s1")).toBe("stored");
  });

  it("live → live (a live session not in a turn)", () => {
    useSessions.setState({ sessions: [live("s1")] });
    expect(of("s1")).toBe("live");
  });

  it("running → running (live AND in a turn — the row's spinner condition)", () => {
    useSessions.setState({ sessions: [live("s1")], inTurn: { s1: true } });
    expect(of("s1")).toBe("running");
  });

  it("a turn without a live session is NOT running (an old flag on a closed session)", () => {
    // The row shows the spinner on `isLive && inTurn`; the rail must not invent
    // activity for a session that is gone.
    useSessions.setState({ historySessions: [live("s1")], inTurn: { s1: true } });
    expect(of("s1")).toBe("stored");
  });

  it("a pending permission prompt → waiting", () => {
    useSessions.setState({ sessions: [live("s1")], inTurn: { s1: true } });
    usePermissions.setState({
      prompts: {
        s1: [{ requestId: "r1", toolTitle: "bash", options: [] }],
      },
    });
    // Waiting WINS over running: the session is not working, it is blocked on
    // the user, and that is the fact the cue exists to report.
    expect(of("s1")).toBe("waiting");
  });

  it("a pending ask/confirm/password → waiting", () => {
    useSessions.setState({ sessions: [live("s1")] });
    for (const method of ["ask", "confirm", "password"] as const) {
      act(() => {
        useInteractive.setState({
          requests: {
            s1: [{ requestId: "r1", method, source: "main", params: {} }],
          },
        });
      });
      expect(of("s1"), method).toBe("waiting");
    }
  });
});
