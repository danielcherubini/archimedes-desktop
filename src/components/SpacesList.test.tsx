import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { closeSession, startSession } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useBridge } from "../store/bridge";
import SpacesList from "./SpacesList";

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
    closeSession: vi.fn().mockRejectedValue(new Error("boom")),
  };
});

const mockedStartSession = vi.mocked(startSession);
const mockedCloseSession = vi.mocked(closeSession);

/**
 * Fixture: two spaces. `alpha` holds a live session `s1` (in-turn, with a
 * loaded transcript) + a stored session `h1` (loaded transcript, ~1h old);
 * `beta` holds a stored session `h2` that was NOT opened this boot (no
 * loaded messages → no relative time).
 */
function seed(): void {
  const now = Date.now();
  // Seed via `setState` directly — NOT `getState().setSpaces(…)` (that
  // triggers boot auto-selection + `openSession` → `loadHistory` IPC).
  useSessions.setState({
    spaces: [
      { path: "/tmp/alpha", createdAt: now, lastOpenedAt: now },
      { path: "/tmp/beta", createdAt: now, lastOpenedAt: now },
    ],
    sessions: [
      { sessionId: "s1", agentId: "pi", cwd: "/tmp/alpha", capabilities: {} },
    ],
    historySessions: [
      { sessionId: "h1", agentId: "pi", cwd: "/tmp/alpha", capabilities: {} },
      { sessionId: "h2", agentId: "pi", cwd: "/tmp/beta", capabilities: {} },
    ],
    activeSessionId: "s1",
    closeReasons: {},
    messages: {
      s1: [{ kind: "user", text: "Fix the login bug", at: now - 10_000 }],
      h1: [
        { kind: "user", text: "Refactor the parser", at: now - 3_600_000 },
      ],
    },
    inTurn: { s1: true },
    stopReasons: {},
  });
  usePermissions.setState({ prompts: {} });
  useBridge.setState({
    requests: {},
    todos: {},
    agentState: {},
    cost: {},
    session: {},
  });
}

beforeEach(() => {
  seed();
  vi.clearAllMocks();
});

describe("SpacesList", () => {
  it("renders the two action buttons with their labels and kbd hints", () => {
    render(<SpacesList />);
    expect(screen.getByRole("button", { name: /New Session/ })).toBeTruthy();
    expect(screen.getByText("⌘N")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Open Space/ })).toBeTruthy();
    expect(screen.getByText("⌘O")).toBeTruthy();
    expect(screen.getByText("Sessions")).toBeTruthy();
  });

  it("opens the Open Space dialog from the action button", () => {
    render(<SpacesList />);
    fireEvent.click(screen.getByRole("button", { name: /Open Space/ }));
    // A fresh (unvalidated) folder renders the literal "New space" title.
    expect(screen.getByText("New space")).toBeTruthy();
  });

  it("opens the Open Space dialog from the New Session button when no session is active", () => {
    useSessions.setState({ activeSessionId: null });
    render(<SpacesList />);
    fireEvent.click(screen.getByRole("button", { name: /New Session/ }));
    expect(screen.getByText("New space")).toBeTruthy();
  });

  it("renders a space group with its folder icon and base name; the chevron collapses the rows", () => {
    const { container } = render(<SpacesList />);
    expect(container.querySelector(".lucide-folder")).not.toBeNull();
    expect(screen.getByText("alpha")).toBeTruthy();
    // `beta` appears twice: the group header's base name + the `h2` row's
    // fallback title (no loaded messages → the space's base name).
    expect(screen.getAllByText("beta")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: /Collapse alpha/ }));
    expect(screen.queryByText("Fix the login bug")).toBeNull();
    expect(screen.queryByText("Refactor the parser")).toBeNull();
    // Re-expand.
    fireEvent.click(screen.getByRole("button", { name: /Expand alpha/ }));
    expect(screen.getByText("Fix the login bug")).toBeTruthy();
  });

  it("starts a session in a space via the group's + button", async () => {
    render(<SpacesList />);
    fireEvent.click(
      screen.getByRole("button", { name: /New session in beta/ }),
    );
    await waitFor(() =>
      expect(mockedStartSession).toHaveBeenCalledWith("pi", "/tmp/beta"),
    );
  });

  it("renders a live session row with its title, a spinner while in-turn, and its relative time", () => {
    render(<SpacesList />);
    const row = screen
      .getByText("Fix the login bug")
      .closest('[role="button"]');
    expect(row).not.toBeNull();
    // The `spinner` primitive (a `LoaderIcon`).
    expect(row!.querySelector('[role="status"]')).not.toBeNull();
    // Last message ~10s ago → `now`.
    expect(row!.textContent).toContain("now");
  });

  it("renders no spinner for a stored session row, with its relative time", () => {
    render(<SpacesList />);
    const row = screen
      .getByText("Refactor the parser")
      .closest('[role="button"]');
    expect(row).not.toBeNull();
    expect(row!.querySelector('[role="status"]')).toBeNull();
    // Last message ~1h ago → `1h`.
    expect(row!.textContent).toContain("1h");
  });

  it("renders an empty time slot (no time text) for a session with no loaded messages", () => {
    render(<SpacesList />);
    // `h2` was not opened this boot: no loaded messages → the title falls
    // back to the space's base name (`beta` — the group header + the row
    // title, both `beta`) and the time slot is empty.
    const betas = screen.getAllByText("beta");
    expect(betas).toHaveLength(2);
    const row = betas[1].closest('[role="button"]');
    expect(row).not.toBeNull();
    // Title only — no time text.
    expect(row!.textContent).toBe("beta");
  });

  it("renders the Waiting pill (not the relative time) for a session with a pending permission prompt", () => {
    usePermissions.setState({
      prompts: {
        s1: [{ requestId: "r1", toolTitle: "Run a tool", options: [] }],
      },
    });
    render(<SpacesList />);
    expect(screen.getByText("Waiting")).toBeTruthy();
    const row = screen
      .getByText("Fix the login bug")
      .closest('[role="button"]');
    expect(row!.textContent).not.toContain("now");
  });

  it("renders the Waiting pill for a pending bridge ask/confirm/password request", () => {
    useBridge.setState({
      requests: {
        h1: [
          {
            requestId: "br1",
            method: "password",
            source: "main",
            params: { command: "sudo apt install foo", reason: "test" },
          },
        ],
      },
    });
    render(<SpacesList />);
    expect(screen.getByText("Waiting")).toBeTruthy();
    const row = screen
      .getByText("Refactor the parser")
      .closest('[role="button"]');
    expect(row!.textContent).not.toContain("1h");
  });

  it("opens a session on row click (the store's activeSessionId + the selection styling)", () => {
    render(<SpacesList />);
    expect(useSessions.getState().activeSessionId).toBe("s1");
    fireEvent.click(screen.getByText("Refactor the parser"));
    expect(useSessions.getState().activeSessionId).toBe("h1");
    const row = screen
      .getByText("Refactor the parser")
      .closest('[role="button"]');
    expect(row!.className).toContain("bg-selected");
  });

  it("truncates a long title on a single line (the fade mask is the sole cue)", () => {
    // A 120-char first user message → `titleFor` slices it to 80 chars; the
    // title span must carry the truncation classes so it overflows under the
    // fade instead of wrapping.
    useSessions.setState({
      messages: {
        s1: [{ kind: "user", text: "x".repeat(120), at: Date.now() - 10_000 }],
      },
    });
    render(<SpacesList />);
    const title = screen.getByText("x".repeat(80));
    expect(title.className).toContain("overflow-hidden");
    expect(title.className).toContain("whitespace-nowrap");
    expect(title.className).toContain("min-w-0");
  });

  it("logs a console error when Pause fails to close the session", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<SpacesList />);
    fireEvent.click(screen.getByRole("button", { name: /Pause/ }));
    await waitFor(() => expect(mockedCloseSession).toHaveBeenCalledWith("s1"));
    expect(consoleError).toHaveBeenCalledWith(
      "Failed to pause session:",
      expect.anything(),
    );
    consoleError.mockRestore();
  });

  it("ignores ⌘N / Ctrl+O while the target is an input or a dialog", async () => {
    render(<SpacesList />);
    const input = document.createElement("input");
    document.body.appendChild(input);
    // Ctrl+N on the input: hijacked by the guard (the target is an input).
    fireEvent.keyDown(input, { key: "n", ctrlKey: true });
    await waitFor(() => expect(mockedStartSession).not.toHaveBeenCalled());
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.appendChild(dialog);
    // Ctrl+O on a `[role="dialog"]`: also ignored.
    fireEvent.keyDown(dialog, { key: "o", metaKey: true });
    expect(screen.queryByText("New space")).toBeNull();
  });

  it("⌘N / Ctrl+N trigger the New Session handler (a bare key does not)", async () => {
    render(<SpacesList />);
    // No modifier: ignored.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "n" }));
    expect(mockedStartSession).not.toHaveBeenCalled();
    // ⌘N: the active session's view owns `s1` (agentId `pi`, cwd
    // `/tmp/alpha`) → `startSession("pi", "/tmp/alpha")`.
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "n", metaKey: true }),
    );
    await waitFor(() =>
      expect(mockedStartSession).toHaveBeenCalledWith("pi", "/tmp/alpha"),
    );
    // Ctrl+N (uppercase key): the same handler.
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "N", ctrlKey: true }),
    );
    await waitFor(() => expect(mockedStartSession).toHaveBeenCalledTimes(2));
  });

  it("⌘O opens the Open Space dialog", () => {
    render(<SpacesList />);
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "o", metaKey: true }),
      );
    });
    expect(screen.getByText("New space")).toBeTruthy();
  });
});
