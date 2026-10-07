import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import type { SessionInfo, SpaceRow } from "../lib/tauri";
import SpaceTabs from "./SpaceTabs";

// Radix (the `DropdownMenu` + `NewSpaceDialog`'s `Dialog`) needs the
// pointer-capture stubs in jsdom (the ChatStream / SettingsPage tests'
// `beforeAll`, verbatim) + `matchMedia`.
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = vi.fn(() => false);
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
  vi.stubGlobal("matchMedia", (q: string) => ({
    matches: false,
    media: q,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
});

vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    startSession: vi.fn().mockResolvedValue({
      sessionId: "new-1",
      cwd: "/w/new",
      capabilities: {},
      archived: false,
    }),
  };
});

const space = (path: string): SpaceRow => ({
  path,
  createdAt: 0,
  lastOpenedAt: 0,
  trusted: false,
});

const info = (sessionId: string, cwd: string): SessionInfo => ({
  sessionId,
  cwd,
  capabilities: {},
  archived: false,
});

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
});

describe("SpaceTabs (the Spaces as browser-style tabs at the top of the chat)", () => {
  it("renders one tab per space (the base name), the active one selected", () => {
    useSessions.setState({
      spaces: [space("/w/alpha"), space("/w/beta")],
      activeSpacePath: "/w/beta",
    });
    render(<SpaceTabs />);
    const alpha = screen.getByRole("tab", { name: "alpha" });
    const beta = screen.getByRole("tab", { name: "beta" });
    expect(alpha.getAttribute("aria-selected")).toBe("false");
    expect(beta.getAttribute("aria-selected")).toBe("true");
  });

  it("clicking a tab selects the space (the store's `selectSpace`)", () => {
    useSessions.setState({
      spaces: [space("/w/alpha"), space("/w/beta")],
      activeSpacePath: "/w/alpha",
    });
    render(<SpaceTabs />);
    fireEvent.click(screen.getByRole("tab", { name: "beta" }));
    expect(useSessions.getState().activeSpacePath).toBe("/w/beta");
  });

  it("a tab for a space with a live session opens it on click", () => {
    useSessions.setState({
      spaces: [space("/w/alpha"), space("/w/beta")],
      activeSpacePath: "/w/alpha",
    });
    useSessions.getState().addSession(info("live-1", "/w/beta"));
    render(<SpaceTabs />);
    fireEvent.click(screen.getByRole("tab", { name: "beta" }));
    expect(useSessions.getState().activeSessionId).toBe("live-1");
  });

  it("the `+` button opens the New Space dialog (picking a folder IS creating a space)", () => {
    useSessions.setState({ spaces: [space("/w/alpha")] });
    render(<SpaceTabs />);
    fireEvent.click(screen.getByRole("button", { name: "New space" }));
    // The dialog (the `NewSpaceDialog` — its "New space" title).
    expect(screen.getByText("New space")).toBeTruthy();
  });

  it("the tab bar holds NO `...` menu, NO pane toggle and NO warning dot (the menu lives in the sidebar's `Sessions` header; the pane toggles in their panes, or the chrome bar while collapsed)", () => {
    useSessions.setState({
      spaces: [space("/w/alpha")],
      activeSpacePath: "/w/alpha",
    });
    const { container } = render(<SpaceTabs />);
    expect(
      screen.queryByRole("button", { name: "Session actions" }),
    ).toBeNull();
    // The pane toggles live in their panes (or, collapsed, in the chrome bar
    // — never the tab strip). ALL four names, because the old assertion here
    // queried a label that had since been renamed and so could never fail.
    for (const label of [
      "Collapse sidebar",
      "Expand sidebar",
      "Collapse side pane",
      "Expand side pane",
    ]) {
      expect(screen.queryByRole("button", { name: label }), label).toBeNull();
    }
    expect(container.querySelector(".bg-warning")).toBeNull();
  });
});
