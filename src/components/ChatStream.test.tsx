import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import ChatStream from "./ChatStream";
import { sendPrompt } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import { useBridge } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { useSubagents } from "../store/subagents";
import { getSidePaneCollapsed, setSidePaneCollapsed } from "../lib/sidePaneState";

// jsdom exposes a non-callable `window.matchMedia` (the `"matchMedia" in
// window` guard in the `BrailleLoader`'s `usePrefersReducedMotion` passes,
// then the call throws) — stub a full MediaQueryList so the working
// indicator renders (the `braille-loader` test's full-stub pattern).
vi.stubGlobal(
  "matchMedia",
  (q: string) => ({
    matches: false,
    media: q,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }),
);

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    listAgents: vi.fn().mockResolvedValue([{ id: "a1", name: "Agent" }]),
    startSession: vi.fn().mockResolvedValue({
      sessionId: "s2",
      agentId: "a1",
      cwd: "/home/u/proj",
      capabilities: {},
    }),
    closeSession: vi.fn().mockResolvedValue(undefined),
    sendPrompt: vi.fn().mockResolvedValue("end_turn"),
    respondPermission: vi.fn().mockResolvedValue(undefined),
    respondBridgeRequest: vi.fn().mockResolvedValue(undefined),
  };
});

/**
 * Seed a live session (fresh transcript) in the sessions store.
 */
function seedLiveSession(): void {
  useSessions.setState({
    activeSessionId: "s1",
    sessions: [
      { sessionId: "s1", agentId: "a1", cwd: "/home/u/proj", capabilities: {} },
    ],
    spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1 }],
    historySessions: [],
    messages: { s1: [] },
    inTurn: {},
    stopReasons: {},
    closeReasons: {},
  });
}

/**
 * Seed a STORED (non-live) session: `sessions` is empty, the session lives
 * in `historySessions` (so `isLive` is false). `capabilities` defaults to `{}`
 * (history-only); pass `{ loadSession: true }` for the resumable case.
 */
function seedStoredSession(
  capabilities: Record<string, unknown> = {},
): void {
  useSessions.setState({
    activeSessionId: "s1",
    sessions: [],
    spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1 }],
    historySessions: [
      { sessionId: "s1", agentId: "a1", cwd: "/home/u/proj", capabilities },
    ],
    messages: { s1: [] },
    inTurn: {},
    stopReasons: {},
    closeReasons: {},
  });
}

/**
 * Flush the microtask queue (the mocked `listAgents` promise resolves after
 * `render` — its `setAgents` re-render must land BEFORE the assertions,
 * wrapped in `act` so React applies it to the DOM).
 */
async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  setSidePaneCollapsed(false);
  localStorage.clear();
  useSessions.setState({
    activeSessionId: null,
    sessions: [],
    historySessions: [],
    spaces: [],
    messages: {},
    inTurn: {},
    stopReasons: {},
    closeReasons: {},
  });
  useBridge.getState().dismissSession("s1");
  usePermissions.getState().dismissSessionPrompts("s1");
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  useBridge.getState().dismissSession("sub1");
  usePermissions.getState().dismissSessionPrompts("sub1");
});

describe("ChatStream", () => {
  it("renders the empty state when there is no active session", () => {
    render(<ChatStream />);
    expect(
      screen.getByText(
        "No active session — open a Space from the list on the left",
      ),
    ).toBeTruthy();
  });

  it("renders the fresh-session hint for an empty transcript", () => {
    seedLiveSession();
    render(<ChatStream />);
    expect(screen.getByText("Send a prompt to start")).toBeTruthy();
  });

  it("derives the header title from the first user message (truncated)", () => {
    seedLiveSession();
    const long = "a".repeat(120);
    useSessions.getState().addUserMessage("s1", long);
    render(<ChatStream />);
    // The header shows the first 80 chars; the stream shows the full text.
    expect(screen.getByText(long.slice(0, 80))).toBeTruthy();
    expect(screen.getByText(long)).toBeTruthy();
  });

  it("renders the working indicator (BrailleLoader) when inTurn with no bridge state", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useSessions.getState().beginTurn("s1");
    render(<ChatStream />);
    // The `BrailleLoader` renders `role="status"`.
    expect(screen.getByRole("status")).toBeTruthy();
  });

  it("renders the working indicator from the bridge agentState alone (no inTurn)", async () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useBridge.getState().applyState("s1", { state: "working" });
    render(<ChatStream />);
    await flush();
    expect(screen.getByRole("status")).toBeTruthy();
  });

  it("renders 'Waiting for your input…' when the bridge agentState is blocked", async () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useBridge.getState().applyState("s1", { state: "blocked" });
    render(<ChatStream />);
    await flush();
    expect(screen.getByText("Waiting for your input…")).toBeTruthy();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("renders a FileSummaryCard with the turn's diff totals", () => {
    seedLiveSession();
    useSessions.setState({
      messages: {
        s1: [
          { kind: "user", text: "edit the files", at: 1 },
          // a.ts: +2 −1
          { kind: "diff", path: "src/a.ts", patch: "+x1\n+x2\n-y1", at: 2 },
          // b.ts: +0 −3
          { kind: "diff", path: "src/b.ts", patch: "-z1\n-z2\n-z3", at: 3 },
        ],
      },
    });
    render(<ChatStream />);
    // Totals: +2 −4.
    expect(screen.getByText("2 files changed")).toBeTruthy();
    expect(screen.getByText("−4")).toBeTruthy();
  });

  it("counts a re-emitted diff once (last patch wins)", async () => {
    seedLiveSession();
    useSessions.setState({
      messages: {
        s1: [
          { kind: "user", text: "edit", at: 1 },
          // The store re-emits the standalone `diff` on every
          // `tool_call_update` carrying content: the same file twice.
          { kind: "diff", path: "src/a.ts", patch: "+x\n-y", at: 2 }, // +1 −1
          {
            kind: "diff",
            path: "src/a.ts",
            patch: "+x1\n+x2\n-y1\n-y2\n-y3",
            at: 3,
          }, // +2 −3
        ],
      },
    });
    render(<ChatStream />);
    await flush();
    // The LATEST patch wins: +2 −3 (header total = the single file's
    // stats, so each appears twice: header + row); the superseded
    // +1 −1 is not counted.
    expect(screen.getByText("1 file changed")).toBeTruthy();
    expect(screen.getAllByText("−3")).toHaveLength(2);
    expect(screen.queryByText("+1")).toBeNull();
    expect(screen.queryByText("−1")).toBeNull();
  });

  it("toggles the side pane (aria-pressed follows the shared flag)", () => {
    seedLiveSession();
    render(<ChatStream />);
    const toggle = screen.getByRole("button", { name: "Toggle side pane" });
    // The pane is open (the flag is false) → aria-pressed=true.
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(toggle);
    // The shared flag flipped…
    expect(getSidePaneCollapsed()).toBe(true);
    // …and the button's aria-pressed follows.
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
  });

  it("renders the composer shell (rounded-2xl, bg-input, border + border-input-border)", () => {
    seedLiveSession();
    const { container } = render(<ChatStream />);
    const shell = container.querySelector(".rounded-2xl");
    expect(shell).toBeTruthy();
    expect(shell!.className).toContain("bg-input");
    expect(shell!.className).toContain("border-input-border");
    // The `border` WIDTH class (Tailwind preflight sets `border-width: 0`
    // — the hover/focus border-COLOR states are dead without it).
    expect(shell!.className).toMatch(/(^|\s)border(\s|$)/);
  });

  it("shows the composer placeholder 'Send a prompt…' for a live idle session", () => {
    seedLiveSession();
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Send a prompt…")).toBeTruthy();
  });

  it("shows the composer placeholder 'Agent is working…' for a live working session", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Agent is working…")).toBeTruthy();
  });

  it("shows the composer placeholder 'Paused — Resume to reconnect' for a stored resumable session", () => {
    seedStoredSession({ loadSession: true });
    render(<ChatStream />);
    expect(
      screen.getByPlaceholderText("Paused — Resume to reconnect"),
    ).toBeTruthy();
  });

  it("shows the composer placeholder 'This session is closed' for a history-only session", () => {
    seedStoredSession();
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("This session is closed")).toBeTruthy();
  });

  it("disables the send button while inTurn and when the draft is empty", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByPlaceholderText("Send a prompt…");
    // Empty draft → disabled.
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    // Non-empty draft, idle → enabled.
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    // inTurn (working) → disabled again — the textarea too.
    await act(async () => {
      useSessions.getState().beginTurn("s1");
    });
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(true);
  });

  it("calls sendPrompt when Enter is pressed with a draft", () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByPlaceholderText("Send a prompt…");
    fireEvent.change(textarea, { target: { value: "hello" } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).toHaveBeenCalledWith("s1", "hello");
  });

  // --- Empty transcript + pending request (the request cards must NOT be
  // swallowed by the "Send a prompt to start" branch). ---

  it("renders a pending permission prompt on an empty transcript (no fresh-session hint)", () => {
    seedLiveSession();
    usePermissions.getState().addPrompt("s1", "req1", {
      toolCall: { title: "bash" },
      options: [{ optionId: "allow_once", name: "Allow once" }],
    });
    render(<ChatStream />);
    expect(screen.getByText("bash")).toBeTruthy();
    expect(screen.getByText("Allow once")).toBeTruthy();
    expect(screen.queryByText("Send a prompt to start")).toBeNull();
  });

  it("renders a stacked bridge ask on an empty transcript (no fresh-session hint)", () => {
    seedLiveSession();
    // No `toolCallId` → the card is NOT queued (it renders immediately,
    // unanchored).
    useBridge.getState().addRequest("s1", {
      requestId: "req-ask",
      method: "ask",
      source: "main",
      params: {
        questions: [
          {
            id: "q1",
            question: "Which approach?",
            options: [{ label: "A" }, { label: "B" }],
          },
        ],
      },
    });
    render(<ChatStream />);
    expect(screen.getByText("Which approach?")).toBeTruthy();
    expect(screen.queryByText("Send a prompt to start")).toBeNull();
  });

  it("renders 'Waiting for your input…' on an empty transcript when the bridge agentState is blocked", async () => {
    seedLiveSession();
    useBridge.getState().applyState("s1", { state: "blocked" });
    render(<ChatStream />);
    await flush();
    expect(screen.getByText("Waiting for your input…")).toBeTruthy();
    expect(screen.queryByText("Send a prompt to start")).toBeNull();
  });

  // --- Composer mismatch states (ONE predicate: `workingOrInTurn`). ---

  it("disables the send button and no-ops send() when the bridge agentState is working (no inTurn)", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByPlaceholderText("Send a prompt…");
    // Idle → enabled (sanity).
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    // The bridge says working (the `state` push races `turnCompleted`, or
    // the state latches after a turn) → the composer is dead: button AND
    // textarea disabled, Enter no-ops.
    await act(async () => {
      useBridge.getState().applyState("s1", { state: "working" });
    });
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(true);
    // The placeholder pins the working copy (the `working` mismatch does
    // not leave the idle copy behind).
    expect(screen.getByPlaceholderText("Agent is working…")).toBeTruthy();
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).not.toHaveBeenCalled();
  });

  it("disables the send button, the textarea, and no-ops Enter while the bridge agentState is blocked (mid-turn awaiting a request response)", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByPlaceholderText("Send a prompt…");
    // Idle → enabled (sanity: `blocked` has NOT been pushed yet).
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    expect(textarea.hasAttribute("disabled")).toBe(false);
    // `inTurn` is true (a `blocked` agent is mid-turn — it is awaiting a
    // request response, so `inTurn` alone used to lock the composer) +
    // the bridge says blocked → the composer is dead for the whole
    // wait: button AND textarea disabled, Enter no-ops (no
    // double-send, no clobbered turn bookkeeping).
    await act(async () => {
      useSessions.getState().beginTurn("s1");
      useBridge.getState().applyState("s1", { state: "blocked" });
    });
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(true);
    expect(screen.getByPlaceholderText("Agent is working…")).toBeTruthy();
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).not.toHaveBeenCalled();
  });

  it("shows the bg-warning toggle dot for a pending SUBAGENT request (the pane's pill is clipped when collapsed — the dot is the cue)", () => {
    seedLiveSession();
    useSubagents.getState().addSession({
      sessionId: "sub1",
      parentSessionId: "s1",
      agentName: "reviewer",
      task: "review the diff",
      status: "running",
    });
    useBridge.getState().addRequest("sub1", {
      requestId: "r1",
      method: "ask",
      source: "subagent:reviewer",
      params: { question: "which library?" },
    });
    render(<ChatStream />);
    const toggle = screen.getByRole("button", { name: "Toggle side pane" });
    // The subagent session id is never the ACTIVE session — the dot
    // counts the subagent's pending request via the shared hook.
    expect(toggle.querySelector(".bg-warning")).toBeTruthy();
  });

  it("does NOT show the toggle dot when there are no pending requests (active or subagent)", () => {
    seedLiveSession();
    useSubagents.getState().addSession({
      sessionId: "sub1",
      parentSessionId: "s1",
      agentName: "reviewer",
      task: "review the diff",
      status: "running",
    });
    render(<ChatStream />);
    const toggle = screen.getByRole("button", { name: "Toggle side pane" });
    expect(toggle.querySelector(".bg-warning")).toBeNull();
  });

  it("keeps the composer consistent when the bridge is idle but the store is inTurn", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    useBridge.getState().applyState("s1", { state: "idle" });
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByPlaceholderText("Send a prompt…");
    // The unified predicate (agentState present → `agentState ===
    // "working"` → false): the placeholder says idle, the controls are
    // enabled, and Enter actually sends — ONE consistent state (the
    // `inTurn` latch is a store follow-up, not a composer concern).
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    expect(textarea.hasAttribute("disabled")).toBe(false);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).toHaveBeenCalledWith("s1", "hi");
  });
});
