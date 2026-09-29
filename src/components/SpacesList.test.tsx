import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { closeSession, deleteSpace, listSkills, setSpaceTrusted, startSession } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useBridge } from "../store/bridge";
import { clearSkillCatalogCache } from "../hooks/useSkillCatalog";
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
    setSpaceTrusted: vi.fn().mockResolvedValue(undefined),
    deleteSpace: vi.fn().mockResolvedValue(undefined),
    listSkills: vi.fn().mockResolvedValue([
      {
        name: "alpha",
        description: "Alpha skill",
        path: "/p/.agents/skills/alpha/SKILL.md",
        dir: "/p/.agents/skills/alpha",
        scope: "space",
        body: "B",
      },
    ]),
  };
});

const mockedStartSession = vi.mocked(startSession);
const mockedCloseSession = vi.mocked(closeSession);
const mockedSetSpaceTrusted = vi.mocked(setSpaceTrusted);
const mockedDeleteSpace = vi.mocked(deleteSpace);
const mockedListSkills = vi.mocked(listSkills);

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
      { path: "/tmp/alpha", createdAt: now, lastOpenedAt: now, trusted: false },
      { path: "/tmp/beta", createdAt: now, lastOpenedAt: now, trusted: true },
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
// The hook's module-level catalog cache is shared across tests in a
// file: clear it so each test refetches (without this, test 1's warm
// cache — keyed by the `seed()` fixture's `cwd` — makes a later test's
// fresh `listSkills` mock moot).
beforeEach(clearSkillCatalogCache);

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

  it("opens the Open Space dialog from the New Session button for an orphaned active session (a legacy session that belongs to no current Space)", () => {
    // The active session's cwd (`/tmp/gamma`) matches NO space in the
    // fixture, so `activeView` is `undefined` even though
    // `activeSessionId` is non-null: the button must open the Open Space
    // dialog instead of silently no-oping (the hook no-ops on an
    // undefined view).
    useSessions.setState({
      sessions: [
        {
          sessionId: "s-orphan",
          agentId: "pi",
          cwd: "/tmp/gamma",
          capabilities: {},
        },
      ],
      activeSessionId: "s-orphan",
    });
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

  it("renders a muted Shield for an untrusted space and a success-colored ShieldCheck for a trusted space", () => {
    render(<SpacesList />);
    // `alpha` (untrusted): the muted `Shield` (no check variant).
    const untrusted = screen.getByRole("button", { name: "Trust alpha" });
    expect(untrusted.querySelector(".lucide-shield")).not.toBeNull();
    // `getAttribute("class")` (not `className` — an SVG element's
    // `className` is an `SVGAnimatedString` in jsdom, not a string).
    expect(untrusted.querySelector(".lucide-shield")!.getAttribute("class")).toContain(
      "text-foreground-subtlest",
    );
    expect(untrusted.querySelector(".lucide-shield-check")).toBeNull();
    // `beta` (trusted): the success-colored `ShieldCheck`.
    const trusted = screen.getByRole("button", { name: "Stop trusting beta" });
    expect(trusted.querySelector(".lucide-shield-check")).not.toBeNull();
    expect(trusted.querySelector(".lucide-shield-check")!.getAttribute("class")).toContain(
      "text-success",
    );
  });

  it("flips the space's trusted flag optimistically and calls the set_space_trusted wrapper", () => {
    render(<SpacesList />);
    fireEvent.click(screen.getByRole("button", { name: "Trust alpha" }));
    // Optimistic: the store flipped BEFORE the (async) command resolves —
    // the spaces store has no live refresh, so the flag must not wait for
    // a round-trip.
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/alpha")!.trusted,
    ).toBe(true);
    expect(mockedSetSpaceTrusted).toHaveBeenCalledWith("/tmp/alpha", true);
  });

  it("rolls the trusted flag back to the previous value when the command rejects", async () => {
    mockedSetSpaceTrusted.mockRejectedValueOnce(new Error("boom"));
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<SpacesList />);
    fireEvent.click(screen.getByRole("button", { name: "Stop trusting beta" }));
    // Optimistic flip first (beta trusted → untrusted)...
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/beta")!.trusted,
    ).toBe(false);
    // ...then rolled back once the command rejects.
    await waitFor(() =>
      expect(
        useSessions.getState().spaces.find((s) => s.path === "/tmp/beta")!.trusted,
      ).toBe(true),
    );
    expect(consoleError).toHaveBeenCalledWith(
      "Failed to set trusted for /tmp/beta:",
      expect.anything(),
    );
    consoleError.mockRestore();
  });

  it("serializes rapid trust toggles for a space (the second command runs only after the first settles)", async () => {
    // The first command stays pending on a deferred promise: without a
    // per-path queue, the second click's command would be issued
    // immediately (two overlapping Tauri invokes could commit in either
    // order and the DB could end on the OPPOSITE value from the
    // optimistic UI).
    let resolveFirst!: () => void;
    const first = new Promise<void>((resolve) => {
      resolveFirst = resolve;
    });
    mockedSetSpaceTrusted.mockImplementationOnce(() => first);
    mockedSetSpaceTrusted.mockImplementationOnce(() => Promise.resolve());
    render(<SpacesList />);
    // Toggle trust on...
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Trust alpha" }));
    });
    expect(mockedSetSpaceTrusted).toHaveBeenCalledTimes(1);
    // ...then immediately off (the optimistic flip re-labeled the button).
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Stop trusting alpha" }),
      );
    });
    // The second command is NOT issued until the first settles.
    expect(mockedSetSpaceTrusted).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolveFirst();
    });
    await waitFor(() =>
      expect(mockedSetSpaceTrusted).toHaveBeenCalledTimes(2),
    );
    expect(mockedSetSpaceTrusted).toHaveBeenNthCalledWith(1, "/tmp/alpha", true);
    expect(mockedSetSpaceTrusted).toHaveBeenNthCalledWith(2, "/tmp/alpha", false);
    // The final UI state ends on the last click's value.
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/alpha")!.trusted,
    ).toBe(false);
  });

  it("settles on the DB-committed value when BOTH of two rapid toggles reject (not the first click's optimistic value)", async () => {
    // Seed through `setSpaces` (the production load path) so the store's
    // committed-trusted baseline is seeded from the DB rows like in
    // production. `delta` is a fresh path (no baseline entry from an
    // earlier test) committed as `trusted: false`.
    const now = Date.now();
    useSessions.getState().setSpaces([
      { path: "/tmp/delta", createdAt: now, lastOpenedAt: now, trusted: false },
    ]);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    // Both commands reject, on deferred promises so the two rollbacks
    // land in click order: the first click's rollback first, the
    // second's LAST — the last rollback is what the UI settles on.
    let rejectFirst!: (err: Error) => void;
    let rejectSecond!: (err: Error) => void;
    mockedSetSpaceTrusted.mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectFirst = reject;
        }),
    );
    mockedSetSpaceTrusted.mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectSecond = reject;
        }),
    );
    render(<SpacesList />);
    // Click 1 (false → true): the optimistic flip lands before the
    // command settles.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Trust delta" }));
    });
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/delta")!.trusted,
    ).toBe(true);
    // Click 2 (true → false) while click 1's command is still pending:
    // the per-path queue defers the second command.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop trusting delta" }));
    });
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/delta")!.trusted,
    ).toBe(false);
    // Reject click 1's command: its rollback lands first, and the queue
    // then issues click 2's (still pending) command.
    await act(async () => {
      rejectFirst(new Error("boom"));
    });
    // Reject click 2's command LAST: its rollback settles the UI.
    await act(async () => {
      rejectSecond(new Error("boom"));
    });
    // The UI settles on the DB-committed value (false) — NOT the first
    // click's optimistic value (true): inferring the second rollback
    // target from the live UI value (the first click's optimistic flip,
    // which is NOT the committed value while a toggle is in flight) would
    // settle on the wrong side.
    expect(
      useSessions.getState().spaces.find((s) => s.path === "/tmp/delta")!.trusted,
    ).toBe(false);
    expect(mockedSetSpaceTrusted).toHaveBeenCalledTimes(2);
    expect(mockedSetSpaceTrusted).toHaveBeenNthCalledWith(1, "/tmp/delta", true);
    expect(mockedSetSpaceTrusted).toHaveBeenNthCalledWith(2, "/tmp/delta", false);
    expect(consoleError).toHaveBeenCalledWith(
      "Failed to set trusted for /tmp/delta:",
      expect.anything(),
    );
    consoleError.mockRestore();
  });

  it("does NOT leave a stale committed-trusted baseline after a remove (a late in-flight toggle success must not re-insert the pruned entry)", async () => {
    // Seed through `setSpaces` (the production load path) so the store's
    // committed-trusted baseline is seeded from the DB rows: `epsilon`
    // is committed as `trusted: false`.
    const now = Date.now();
    useSessions.getState().setSpaces([
      { path: "/tmp/epsilon", createdAt: now, lastOpenedAt: now, trusted: false },
    ]);
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    // A toggle in flight on a deferred command...
    let resolveToggle!: () => void;
    mockedSetSpaceTrusted.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveToggle = resolve;
        }),
    );
    await act(async () => {
      useSessions.getState().setSpaceTrusted("/tmp/epsilon", true);
    });
    // ...and a removal while it is in flight (deferred delete): the
    // store prunes the path's baseline + queue once the delete settles.
    let resolveDelete!: () => void;
    mockedDeleteSpace.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveDelete = resolve;
        }),
    );
    const removing = useSessions.getState().removeSpace("/tmp/epsilon");
    await act(async () => {
      resolveDelete();
    });
    await act(async () => {
      await removing;
    });
    // The in-flight toggle's command succeeds AFTER the prune: its
    // success handler must NOT re-insert a baseline entry for the
    // removed path (it would survive the `addSpace` guard and poison a
    // fresh re-add).
    await act(async () => {
      resolveToggle();
    });
    // Re-add the path: a fresh DB row is committed as `trusted: false`,
    // so the (pruned) baseline must re-seed to false.
    await act(async () => {
      useSessions.getState().addSpace("/tmp/epsilon");
    });
    // A failed toggle must roll back to the FRESH baseline (false), not
    // a stale in-flight value (true).
    let rejectToggle!: (err: Error) => void;
    mockedSetSpaceTrusted.mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejectToggle = reject;
        }),
    );
    await act(async () => {
      useSessions.getState().setSpaceTrusted("/tmp/epsilon", true);
    });
    await act(async () => {
      rejectToggle(new Error("boom"));
    });
    expect(
      useSessions
        .getState()
        .spaces.find((s) => s.path === "/tmp/epsilon")!.trusted,
    ).toBe(false);
    expect(consoleError).toHaveBeenCalledWith(
      "Failed to set trusted for /tmp/epsilon:",
      expect.anything(),
    );
    consoleError.mockRestore();
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
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    // Appended to `document.body` OUTSIDE RTL's render tree — RTL cleanup
    // does not touch manually appended nodes, so remove them explicitly
    // (cross-test pollution: a stray `[role="dialog"]` in the body would
    // make later tests' shortcuts no-op via the `e.target` guard).
    document.body.append(input, dialog);
    try {
      // Ctrl+N on the input: hijacked by the guard (the target is an input).
      fireEvent.keyDown(input, { key: "n", ctrlKey: true });
      await waitFor(() => expect(mockedStartSession).not.toHaveBeenCalled());
      // Ctrl+O on a `[role="dialog"]`: also ignored.
      fireEvent.keyDown(dialog, { key: "o", metaKey: true });
      expect(screen.queryByText("New space")).toBeNull();
    } finally {
      input.remove();
      dialog.remove();
    }
  });

  it("does NOT ignore ⌘N while the target is a contenteditable=\"false\" element (the guard matches `contenteditable` except `false`)", async () => {
    render(<SpacesList />);
    const div = document.createElement("div");
    div.setAttribute("contenteditable", "false");
    // Same manual-append caveat as the guard test above: remove it in
    // `finally` (RTL cleanup does not touch it).
    document.body.appendChild(div);
    try {
      // `contenteditable="false"` is NOT editable: the shortcut fires.
      fireEvent.keyDown(div, { key: "n", metaKey: true });
      await waitFor(() =>
        expect(mockedStartSession).toHaveBeenCalledWith("pi", "/tmp/alpha"),
      );
    } finally {
      div.remove();
    }
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

  it("the_skills_button_sits_in_the_button_row", async () => {
    render(<SpacesList />);
    // The third button in the `New Session` / `Open Space` row (the skill
    // LIST no longer renders in the pane — the button opens a modal).
    expect(await screen.findByText("Skills")).toBeTruthy();
    expect(screen.getByText("Sessions")).toBeTruthy();
    // The one-fetch property (single consumer): one `listSkills` call.
    expect(mockedListSkills).toHaveBeenCalledTimes(1);
  });

  it("the_skills_modal_shows_the_empty_state_when_there_are_no_skills", async () => {
    // `clearAllMocks` PRESERVES the factory's 1-skill implementation, so
    // override it here. Declared LAST so the override cannot leak into
    // the other test (the `beforeEach` cache-clear makes each test
    // refetch; because implementations are preserved, this test's
    // `mockResolvedValue([])` would leak to any test declared after it —
    // declaration order is what keeps the two independent).
    vi.mocked(listSkills).mockResolvedValue([]);
    render(<SpacesList />);
    // Open the modal from the button.
    fireEvent.click(screen.getByRole("button", { name: "Skills" }));
    await screen.findByText("No skills found.");
  });
});
