import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import SessionStalledBanner from "./SessionStalledBanner";
import { resumeSession } from "../lib/tauri";
import { openPath } from "@tauri-apps/plugin-opener";
import { useSessions } from "../store/sessions";

// Mock the Tauri IPC layer (the `resume_session` command — the store's
// `resumeSession` calls it; a success clears the store's `stalled` entry).
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    resumeSession: vi.fn().mockResolvedValue({
      sessionId: "s1",
      cwd: "/home/u/proj",
      capabilities: {},
      archived: false,
    }),
    loadHistory: vi.fn().mockResolvedValue([]),
  };
});

// The `tauri-plugin-opener` `openPath` (the crash-log link — the real
// module would `invoke` Tauri, which doesn't exist in jsdom).
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn().mockResolvedValue(undefined),
}));

beforeEach(() => {
  vi.clearAllMocks();
  useSessions.setState({
    activeSessionId: "s1",
    sessions: [
      {
        sessionId: "s1",
        cwd: "/home/u/proj",
        capabilities: {},
        archived: false,
      },
    ],
    stalled: {},
    messages: { s1: [] },
  });
});

describe("SessionStalledBanner", () => {
  it("renders the banner (the spec wording + the Resume button) when the session is stalled", () => {
    useSessions.setState({
      stalled: { s1: { at: 1700000000000, crashLog: null } },
    });
    render(<SessionStalledBanner sessionId="s1" />);
    // The spec's §4 wording: "Session stopped unexpectedly at <time> —
    // last turn incomplete."
    const text = screen.getByTestId("stalled-banner").textContent ?? "";
    expect(text).toContain("Session stopped unexpectedly at");
    expect(text).toContain("last turn incomplete");
    expect(screen.getByRole("button", { name: "Resume" })).toBeTruthy();
    // No crash log (the secondary line is absent).
    expect(screen.queryByTestId("stalled-crash-log")).toBeNull();
  });

  it("the Resume click calls resume_session and the success clears the stalled entry (the banner hides)", async () => {
    useSessions.setState({
      stalled: { s1: { at: 1700000000000, crashLog: null } },
    });
    render(<SessionStalledBanner sessionId="s1" />);
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(resumeSession).toHaveBeenCalledWith(
      "s1",
      "/home/u/proj",
    );
    // The store's `resumeSession` cleared `stalled[s1]` on success —
    // the banner hides (the session is live again).
    await waitFor(() =>
      expect(useSessions.getState().stalled["s1"]).toBeUndefined(),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("stalled-banner")).toBeNull(),
    );
  });

  it("the crash-log line renders the path and calls openPath when present", () => {
    useSessions.setState({
      stalled: {
        s1: { at: 1700000000000, crashLog: "/data/crash-123-worker.log" },
      },
    });
    render(<SessionStalledBanner sessionId="s1" />);
    const link = screen.getByTestId("stalled-crash-log");
    expect(link.textContent).toContain("/data/crash-123-worker.log");
    fireEvent.click(link);
    expect(openPath).toHaveBeenCalledWith("/data/crash-123-worker.log");
  });

  it("is hidden when the session is not stalled", () => {
    render(<SessionStalledBanner sessionId="s1" />);
    expect(screen.queryByTestId("stalled-banner")).toBeNull();
  });

  it("is dismissible (local state — the session stays stalled in the store)", () => {
    useSessions.setState({
      stalled: { s1: { at: 1700000000000, crashLog: null } },
    });
    render(<SessionStalledBanner sessionId="s1" />);
    expect(screen.getByTestId("stalled-banner")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss stalled banner" }));
    expect(screen.queryByTestId("stalled-banner")).toBeNull();
    // The store entry is UNTOUCHED (the banner reappears on re-open —
    // the component remounts with a fresh dismiss state).
    expect(useSessions.getState().stalled["s1"]).toEqual({
      at: 1700000000000,
      crashLog: null,
    });
  });
});
