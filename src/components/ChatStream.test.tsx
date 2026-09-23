import { beforeEach, describe, expect, it, vi, beforeAll } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import ChatStream from "./ChatStream";
import { sendPrompt, setSessionConfigOption } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import { useBridge } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { useSubagents } from "../store/subagents";
import { getSidePaneCollapsed, setSidePaneCollapsed } from "../lib/sidePaneState";

// jsdom exposes a non-callable `window.matchMedia` (the `"matchMedia" in
// window` guard in the `BrailleLoader`'s `usePrefersReducedMotion` passes,
// then the call throws) — stub a full MediaQueryList so the working
// indicator renders (the `braille-loader` test's full-stub pattern).
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = vi.fn(() => false);
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  HTMLElement.prototype.hasPointerCapture = vi.fn(() => false);
  HTMLElement.prototype.setPointerCapture = vi.fn();
  HTMLElement.prototype.releasePointerCapture = vi.fn();
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
});

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
    setSessionConfigOption: vi.fn().mockResolvedValue([]),
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
    configOptions: {},
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
    configOptions: {},
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
    configOptions: {},
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

  it("survives the no-session → active-session transition (all hooks are called unconditionally, before the early return)", () => {
    // Render the early-return path first (no active session → the hook
    // count is N), then seed a live session and re-render (full frame →
    // the hook count must STILL be N). Any hook called only on the
    // full-frame path (after the `!activeSessionId` early return) would
    // throw "Rendered more hooks than during the previous render" here
    // (the white-page crash) — e.g. `usePendingSubagentRequests`.
    render(<ChatStream />);
    expect(
      screen.getByText(
        "No active session — open a Space from the list on the left",
      ),
    ).toBeTruthy();
    act(() => {
      seedLiveSession();
    });
    expect(screen.getByRole("button", { name: "Send" })).toBeTruthy();
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

  it("renders thinking block in transcript", async () => {
    seedLiveSession();
    const sessionId = "s1";
    useSessions.getState().addUserMessage(sessionId, "hi");
    useSessions.getState().applySessionUpdate(sessionId, { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "pondering" }, messageId: "m1" });
    useSessions.getState().beginTurn(sessionId);
    
    render(<ChatStream />);
    // Verify "Thinking" label
    expect(screen.getByText("Thinking")).toBeTruthy();
    
    // Complete the turn
    act(() => {
      useSessions.getState().turnCompleted(sessionId, "end_turn");
    });
    
    // Verify "Thought" label (assert ONLY "Thought")
    expect(screen.getByText("Thought")).toBeTruthy();
  });

  it("renders done thinking block in history (not streaming)", () => {
    seedLiveSession();
    const sessionId = "s1";
    useSessions.getState().addUserMessage(sessionId, "hi");
    useSessions.getState().applySessionUpdate(sessionId, { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "done thought" }, messageId: "m1" });
    // Not in turn
    
    render(<ChatStream />);
    // Should show "Thought" (it never streamed)
    expect(screen.getByText("Thought")).toBeTruthy();
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

  it("renders a pending sudo password request on an empty transcript (no fresh-session hint)", () => {
    seedLiveSession();
    // No `toolCallId` (absent for `password`) → the modal renders
    // immediately (unanchored).
    useBridge.getState().addRequest("s1", {
      requestId: "req-pw",
      method: "password",
      source: "main",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    render(<ChatStream />);
    expect(screen.getByText("Sudo password required")).toBeTruthy();
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

  it("keeps the composer LOCKED when the bridge is idle but the store is inTurn", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    useBridge.getState().applyState("s1", { state: "idle" });
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    // A stale `idle` from the previous turn must not unlock the composer
    // while a turn is in flight (`inTurn` is the ground truth — the store
    // does not reset `agentState` on `turnCompleted`, a store follow-up):
    // the placeholder says working, the controls are disabled, and Enter
    // no-ops. The working indicator shows (the turn IS in flight).
    const textarea = screen.getByPlaceholderText("Agent is working…");
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(true);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).not.toHaveBeenCalled();
    expect(screen.queryByRole("status")).toBeTruthy();
  });

  it("prefers the 'Waiting for your input…' line over the braille loader when blocked AND inTurn", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    useBridge.getState().applyState("s1", { state: "blocked" });
    render(<ChatStream />);
    // `blocked` + `inTurn` → `workingOrInTurn` is true, but the blocked
    // line takes precedence: the waiting line renders and the braille
    // loader (role="status") does NOT (both would render otherwise).
    expect(screen.getByText("Waiting for your input…")).toBeTruthy();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("a live session WITH a model + thinking config option renders BOTH selectors", async () => {
    seedLiveSession();
    useSessions.setState({
      configOptions: {
        s1: [
          {
            id: "model",
            name: "Model",
            type: "select",
            currentValue: "acme/alpha",
            options: [
              { value: "acme/alpha", name: "acme/Alpha" },
              { value: "acme/beta", name: "acme/Beta" },
            ],
          },
          {
            id: "thought_level",
            name: "Thinking",
            type: "select",
            currentValue: "medium",
            options: [
              { value: "low", name: "Low" },
              { value: "medium", name: "Medium" },
            ],
          },
        ],
      },
    });
    render(<ChatStream />);
    expect(screen.getByRole("combobox", { name: "Model" })).toBeTruthy();
    expect(screen.getByRole("combobox", { name: "Thinking" })).toBeTruthy();
    expect(screen.getByText("acme/Alpha")).toBeTruthy();
    expect(screen.getByText("Medium")).toBeTruthy();
  });

  it("choosing a model item invokes setSessionConfigOption", async () => {
    seedLiveSession();
    useSessions.setState({
      configOptions: {
        s1: [
          {
            id: "model",
            name: "Model",
            type: "select",
            currentValue: "acme/alpha",
            options: [
              { value: "acme/alpha", name: "acme/Alpha" },
              { value: "acme/beta", name: "acme/Beta" },
            ],
          },
        ],
      },
    });
    render(<ChatStream />);
    fireEvent.click(screen.getByRole("combobox", { name: "Model" }));
    fireEvent.click(screen.getByRole("option", { name: "acme/Beta" }));
    expect(setSessionConfigOption).toHaveBeenCalledWith("s1", "model", "acme/beta");
  });

  it("a live session WITHOUT configOptions renders neither selector", () => {
    seedLiveSession();
    render(<ChatStream />);
    expect(screen.queryByRole("combobox", { name: "Model" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Thinking" })).toBeNull();
  });

  it("does NOT leak Reasoning state across sessions (per-session key)", () => {
    useSessions.setState({
      activeSessionId: "s1",
      sessions: [
        { sessionId: "s1", agentId: "a1", cwd: "/home/u/proj", capabilities: {} },
        { sessionId: "s2", agentId: "a1", cwd: "/home/u/proj", capabilities: {} },
      ],
      spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1 }],
      historySessions: [],
      messages: {
        s1: [
          { kind: "user", text: "hi1", at: 1 },
          { kind: "agent-thought", messageId: "m1", text: "thought-one", at: 2 },
        ],
        s2: [
          { kind: "user", text: "hi2", at: 1 },
          { kind: "agent-thought", messageId: "m2", text: "thought-two", at: 2 },
        ],
      },
      inTurn: {},
      stopReasons: {},
      closeReasons: {},
      configOptions: {},
    });
    render(<ChatStream />);
    // s1's thinking block is collapsed by default…
    expect(screen.queryByText("thought-one")).toBeNull();
    // …and expands on click.
    fireEvent.click(screen.getByTestId("reasoning-trigger"));
    expect(screen.getByText("thought-one")).toBeTruthy();
    // Switching sessions must NOT reuse the same Reasoning instance at
    // the same index: s2's block starts collapsed (no inherited
    // expanded state or duration) and s1's content is gone.
    act(() => {
      useSessions.setState({ activeSessionId: "s2" });
    });
    expect(screen.queryByText("thought-two")).toBeNull();
    expect(screen.queryByText("thought-one")).toBeNull();
  });

  it("a STORED session with configOptions renders neither selector", () => {
    seedStoredSession();
    useSessions.setState({
      configOptions: {
        s1: [
          {
            id: "model",
            name: "Model",
            type: "select",
            currentValue: "acme/alpha",
            options: [{ value: "acme/alpha", name: "acme/Alpha" }],
          },
        ],
      },
    });
    render(<ChatStream />);
    expect(screen.queryByRole("combobox", { name: "Model" })).toBeNull();
  });
});
