import { beforeEach, describe, expect, it, vi, beforeAll } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import ChatStream from "./ChatStream";
import {
  sendPrompt,
  setSessionConfigOption,
  resumeSession,
  readClipboardImage,
  cancelSession,
} from "../lib/tauri";
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
  // Node's global `URL` leaks into jsdom but only accepts Node `Blob`s —
  // stub the object-URL APIs so staged attachments get deterministic URLs.
  URL.createObjectURL = vi.fn((file: File) => `blob:mock-${file.name}`);
  URL.revokeObjectURL = vi.fn();
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
    resumeSession: vi.fn().mockResolvedValue({
      sessionId: "s1",
      agentId: "a1",
      cwd: "/home/u/proj",
      capabilities: { loadSession: true, promptCapabilities: { image: true } },
    }),
    respondPermission: vi.fn().mockResolvedValue(undefined),
    respondBridgeRequest: vi.fn().mockResolvedValue(undefined),
    setSessionConfigOption: vi.fn().mockResolvedValue([]),
    readClipboardImage: vi.fn().mockResolvedValue(null),
    cancelSession: vi.fn().mockResolvedValue(undefined),
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
 * Seed a live session whose agent advertises `promptCapabilities.image`
 * (the image-attachment capability gate — fail-closed otherwise).
 */
function seedLiveSessionWithImages(): void {
  useSessions.setState({
    activeSessionId: "s1",
    sessions: [
      {
        sessionId: "s1",
        agentId: "a1",
        cwd: "/home/u/proj",
        capabilities: { promptCapabilities: { image: true } },
      },
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

// `fireEvent.paste` wraps the dispatch in `act` (a raw `dispatchEvent` does NOT —
// state changes then need `await act(...)` to become visible) and jsdom has no
// `DataTransfer`, so the plain `clipboardData` object is attached as-is and
// reaches React's `onPaste` with `e.clipboardData.items` intact. A pasted image
// lives in `items` as a `ClipboardItem` you pull via `getAsFile()`; `files` is
// EMPTY in real webviews (WebKit/WebView2), so the stub mirrors that — it builds
// `items` (NOT `files`) and each item models the real `ClipboardItem` shape.
function pasteToComposer(
  files: File[],
  extra: { text?: string; html?: string } = {},
): boolean {
  const target = screen.getByRole("textbox") as HTMLTextAreaElement;
  return fireEvent.paste(target, {
    clipboardData: {
      items: files.map((f) => ({
        kind: "file",
        type: f.type,
        getAsFile: () => f,
      })),
      getData: (type: string) =>
        type === "text/plain" ? (extra.text ?? "") : (extra.html ?? ""),
    },
  });
}

function dropOnComposer(files: File[]): void {
  // `getByRole("textbox")`, NOT `getByPlaceholderText(/Ask/i)` — the placeholder
  // changes with session state (e.g. "Agent is working…" when locked).
  const target = screen.getByRole("textbox");
  fireEvent.drop(target, { dataTransfer: { files } });
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
  // Restore real timers in case a previous test installed fake ones (the
  // mid-read tests below use `vi.useFakeTimers()` to settle the macrotask-
  // based `FileReader` deterministically) — a no-op for tests that don't.
  vi.useRealTimers();
  setSidePaneCollapsed(false);
  localStorage.clear();
  // `clearAllMocks` clears the call history (the `URL` stub implementations
  // from `beforeAll` survive — `mockClear` semantics, not `mockReset`).
  vi.clearAllMocks();
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

  it("shows the ZCode placeholder 'Ask anything…' for a fresh live session (no history)", () => {
    seedLiveSession();
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Ask anything…")).toBeTruthy();
  });

  it("shows the ZCode placeholder 'Ask for follow-up changes' for a live session WITH history", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hello");
    render(<ChatStream />);
    expect(
      screen.getByPlaceholderText("Ask for follow-up changes"),
    ).toBeTruthy();
  });

  it("renders the composer textarea at full width (w-full — a width:auto <textarea> falls back to its intrinsic cols width, ~177px)", () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox");
    expect(textarea.className).toContain("w-full");
  });

  it("renders the ZCode-style send button (icon-md: size-7 rounded-lg — not the old circular size-8 rounded-full)", () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    expect(sendButton.className).toContain("rounded-lg");
    expect(sendButton.className).not.toContain("rounded-full");
  });

  it("shows the composer placeholder 'Agent is working…' for a live working session", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Agent is working…")).toBeTruthy();
  });

  it("shows the composer placeholder 'Resume this session to send' for a stored resumable session", () => {
    seedStoredSession({ loadSession: true });
    render(<ChatStream />);
    expect(
      screen.getByPlaceholderText("Resume this session to send"),
    ).toBeTruthy();
  });

  it("shows the composer placeholder 'This session is closed' for a history-only session", () => {
    seedStoredSession();
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("This session is closed")).toBeTruthy();
  });

  // --- Auto-resume: a RESUMABLE stored session gets an enabled composer that
  // auto-resumes on send; a NON-RESUMABLE stored session stays fully disabled. ---

  it("enables the composer for a resumable stored session (no live session)", () => {
    seedStoredSession({ loadSession: true, promptCapabilities: { image: true } });
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox");
    // The textarea is NOT disabled (it used to be — stored sessions were read-only).
    expect(textarea.hasAttribute("disabled")).toBe(false);
    const sendButton = screen.getByRole("button", { name: "Send" });
    // Empty draft → still disabled…
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    // …and enabled once there's a draft.
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
  });

  it("keeps the composer disabled for a non-resumable stored session", () => {
    seedStoredSession(); // capabilities {} — no `loadSession`
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox");
    expect(textarea.hasAttribute("disabled")).toBe(true);
    // The Send button stays disabled too (no draft can even be typed — a
    // disabled textarea can't be changed; the capability gate fails closed).
    expect(
      screen.getByRole("button", { name: "Send" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("send in a resumable stored session auto-resumes it, then sends", async () => {
    seedStoredSession({ loadSession: true, promptCapabilities: { image: true } });
    render(<ChatStream />);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "resume and send" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // The stored session was resumed FIRST (the store action delegates to the
    // Tauri `resume_session` command, mocked above)…
    await waitFor(() =>
      expect(resumeSession).toHaveBeenCalledWith(
        "a1",
        "s1",
        "/home/u/proj",
      ),
    );
    // …then the prompt was sent (2-arg, text-only).
    await waitFor(() =>
      expect(sendPrompt).toHaveBeenCalledWith("s1", "resume and send"),
    );
  });

  it("a failed resume aborts the send", async () => {
    seedStoredSession({ loadSession: true, promptCapabilities: { image: true } });
    vi.mocked(resumeSession).mockRejectedValueOnce(new Error("nope"));
    render(<ChatStream />);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "hi" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // The resume failure surfaces its own error line…
    await waitFor(() => expect(screen.getByText("nope")).toBeTruthy());
    // …and the send was aborted (no prompt was sent to the stored session).
    expect(vi.mocked(sendPrompt)).not.toHaveBeenCalled();
  });

  it("paste stages a thumbnail in a resumable stored session (capability read from the stored session)", () => {
    seedStoredSession({ loadSession: true, promptCapabilities: { image: true } });
    render(<ChatStream />);
    const intercepted = pasteToComposer([
      new File([new Uint8Array([1])], "s.png", { type: "image/png" }),
    ]);
    // The paste WAS intercepted — the capability came from the STORED
    // session's `capabilities` (a live-only read would fail closed).
    expect(intercepted).toBe(false);
    expect(screen.getByAltText("s.png")).toBeTruthy();
  });

  it("disables the send button while inTurn and when the draft is empty", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByPlaceholderText("Ask anything…");
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
    const textarea = screen.getByPlaceholderText("Ask anything…");
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
    const textarea = screen.getByPlaceholderText("Ask anything…");
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
    const textarea = screen.getByPlaceholderText("Ask anything…");
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

  it("a live session WITH a model + thinking config option renders BOTH selects IN THE COMPOSER (ZCode's placement — the header carries no config selects)", async () => {
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
    const { container } = render(<ChatStream />);
    // ZCode's composer carries the model/thought controls in its toolbar
    // (left of the send button) — the header does NOT.
    const composer = container.querySelector(".rounded-2xl")!;
    expect(composer.querySelector('[aria-label="Model"]')).toBeTruthy();
    expect(composer.querySelector('[aria-label="Thinking"]')).toBeTruthy();
    const header = container.querySelector(".h-12")!;
    expect(header.querySelector('[aria-label="Model"]')).toBeNull();
    expect(header.querySelector('[aria-label="Thinking"]')).toBeNull();
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

  // --- Image attachments: paste / drop / thumbnail strip / capability gate. ---

  it("paste stages a thumbnail", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    const intercepted = pasteToComposer([
      new File([new Uint8Array([1])], "s.png", { type: "image/png" }),
    ]);
    // The paste WAS intercepted (`preventDefault` → `defaultPrevented`).
    expect(intercepted).toBe(false);
    expect(screen.getByAltText("s.png")).toBeTruthy();
  });

  it("paste of plain text is NOT intercepted", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    const intercepted = pasteToComposer([], { text: "hello" });
    // Default text paste falls through (jsdom does not implement the browser's
    // default paste action — no text is inserted; assert on the return value).
    expect(intercepted).toBe(true);
    expect(screen.queryByAltText("s.png")).toBeNull();
  });

  it("paste of plain text does NOT trigger the Rust clipboard read", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([], { text: "hello" });
    await flush();
    // A text paste is a text paste — no image read (no stale image staged).
    expect(vi.mocked(readClipboardImage)).not.toHaveBeenCalled();
    expect(screen.queryByAltText("s.png")).toBeNull();
  });

  it("paste with file items does NOT trigger the Rust clipboard read", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1])], "s.png", { type: "image/png" }),
    ]);
    await flush();
    // The standard `items` path handles it — no fallback read.
    expect(vi.mocked(readClipboardImage)).not.toHaveBeenCalled();
    expect(screen.getByAltText("s.png")).toBeTruthy();
  });

  it("WebKitGTK quirk: empty paste event falls back to the Rust clipboard read", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    // WebKitGTK: a pasted image produces a `paste` event with NO items and
    // NO text (the DataTransfer is empty). The composer reads the image
    // from the system clipboard (Rust/arboard) instead.
    vi.mocked(readClipboardImage).mockResolvedValueOnce([0x89, 0x50, 0x4e, 0x47]);
    pasteToComposer([]);
    await flush();
    expect(vi.mocked(readClipboardImage)).toHaveBeenCalledTimes(1);
    // The read image is staged as a thumbnail (`screenshot.png`).
    expect(screen.getByAltText("screenshot.png")).toBeTruthy();
  });

  it("WebKitGTK quirk fallback: no image on the clipboard stages nothing", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    vi.mocked(readClipboardImage).mockResolvedValueOnce(null);
    pasteToComposer([]);
    await flush();
    expect(vi.mocked(readClipboardImage)).toHaveBeenCalledTimes(1);
    expect(screen.queryByAltText("screenshot.png")).toBeNull();
  });

  it("WebKitGTK quirk fallback: a read error stages nothing (silent)", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    vi.mocked(readClipboardImage).mockRejectedValueOnce(new Error("no display"));
    pasteToComposer([]);
    await flush();
    expect(screen.queryByAltText("screenshot.png")).toBeNull();
    // No error line for a silent fallback miss (an empty-clipboard paste is
    // not an error condition).
    expect(screen.queryByText(/clipboard/i)).toBeNull();
  });

  it("spreadsheet TSV is NOT intercepted over the synthetic PNG", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    const intercepted = pasteToComposer(
      [new File([new Uint8Array([1])], "shot.png", { type: "image/png" })],
      { text: "a\tb" },
    );
    // The text paste wins (the spreadsheet heuristic) — no thumbnail.
    expect(intercepted).toBe(true);
    expect(screen.queryByAltText("shot.png")).toBeNull();
  });

  it("non-image paste is rejected with an error line", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer(
      [new File([new Uint8Array([1])], "a.txt", { type: "text/plain" })],
      { text: "x" },
    );
    expect(screen.queryByAltText("a.txt")).toBeNull();
    const errorLine = screen.getByText(/is not a supported image format/);
    expect(errorLine.className).toContain("text-destructive");
  });

  it("SVG is rejected (allowlist)", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1])], "a.svg", { type: "image/svg+xml" }),
    ]);
    expect(screen.queryByAltText("a.svg")).toBeNull();
    expect(
      screen.getByText(/is not a supported image format/).className,
    ).toContain("text-destructive");
  });

  it("oversized paste is rejected", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    // 11 MiB > the 10 MiB inline cap.
    const oversized = new File(
      [new Uint8Array(11 * 1024 * 1024)],
      "big.png",
      { type: "image/png" },
    );
    pasteToComposer([oversized]);
    expect(screen.queryByAltText("big.png")).toBeNull();
    expect(
      screen.getByText(/exceeds the 10 MiB limit/).className,
    ).toContain("text-destructive");
  });

  it("ninth image is rejected at the cap", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    const firstEight = Array.from({ length: 8 }, (_, i) =>
      new File([new Uint8Array([1])], `f${i}.png`, { type: "image/png" }),
    );
    pasteToComposer(firstEight);
    // Separate dispatch — the ref-based `stageFiles` keeps the list current
    // between them.
    pasteToComposer(
      [new File([new Uint8Array([1])], "ninth.png", { type: "image/png" })],
    );
    // 8 staged, the 9th rejected.
    expect(screen.getAllByRole("img")).toHaveLength(8);
    expect(screen.queryByAltText("ninth.png")).toBeNull();
    expect(
      screen.getByText(/at most 8/).className,
    ).toContain("text-destructive");
  });

  it("remove button un-stages", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1])], "s.png", { type: "image/png" }),
    ]);
    expect(screen.getByAltText("s.png")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Remove image attachment" }),
    );
    expect(screen.queryByAltText("s.png")).toBeNull();
  });

  it("drop stages a thumbnail", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    dropOnComposer([
      new File([new Uint8Array([1])], "d.png", { type: "image/png" }),
    ]);
    expect(screen.getByAltText("d.png")).toBeTruthy();
  });

  it("drop is ignored while locked", () => {
    seedLiveSessionWithImages();
    // `inTurn` set BEFORE `render` (the store is global) → the textarea is
    // `disabled` on first render and the drop is swallowed.
    useSessions.setState({ inTurn: { s1: true } });
    render(<ChatStream />);
    dropOnComposer([
      new File([new Uint8Array([1])], "d.png", { type: "image/png" }),
    ]);
    expect(screen.queryByAltText("d.png")).toBeNull();
  });

  it("window-level drop backstop prevents navigation (drop outside the composer)", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    // The window `drop` listener (registered in `useEffect`, present after
    // `render`'s `act` flush) swallows file drops OUTSIDE the composer — the
    // webview's default for a file drop would otherwise NAVIGATE to the file
    // (replacing the app; the reason `dragDropEnabled: false` + this
    // backstop exist). `fireEvent` returns `!defaultPrevented` → `false`
    // when the listener's `preventDefault` marked the event.
    const intercepted = fireEvent.drop(window, {
      dataTransfer: {
        files: [
          new File([new Uint8Array([1])], "w.png", { type: "image/png" }),
        ],
      },
    });
    expect(intercepted).toBe(false);
    // The event targeted `window` (the top of the tree — it does NOT bubble
    // down through the composer), so the composer's `onDrop` (`handleDrop`)
    // never fired: the drop was swallowed, NOT staged.
    expect(screen.queryByAltText("w.png")).toBeNull();
    // `dragover` variant: a file drag anywhere on the page must make the
    // page a valid drop target (otherwise the browser shows the "no-drop"
    // cursor and the drop would navigate).
    expect(fireEvent.dragOver(window, { dataTransfer: {} })).toBe(false);
  });

  it("unmount releases staged object URLs", () => {
    seedLiveSessionWithImages();
    const { unmount } = render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    expect(screen.getByAltText("s.png")).toBeTruthy();
    // The unmount-cleanup effect revokes the staged object URLs exactly once
    // (a revoked URL can no longer render the `img` — the effect runs on
    // unmount ONLY, never on state changes).
    unmount();
    // The `beforeAll` stub: `createObjectURL(file) => \`blob:mock-${file.name}\``.
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:mock-s.png");
  });

  it("placeholder advertises paste when the agent supports images", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    expect(
      screen.getByPlaceholderText("Ask anything — or paste an image…"),
    ).toBeTruthy();
  });

  it("feature is inert without the capability", () => {
    seedLiveSession(); // capabilities {}
    render(<ChatStream />);
    const intercepted = pasteToComposer([
      new File([new Uint8Array([1])], "s.png", { type: "image/png" }),
    ]);
    // NOT intercepted (default text paste falls through) and no thumbnail.
    expect(intercepted).toBe(true);
    expect(screen.queryByAltText("s.png")).toBeNull();
    // The plain placeholder (the paste hint is capability-gated too).
    expect(screen.getByPlaceholderText("Ask anything…")).toBeTruthy();
  });

  it("switching sessions clears staged attachments", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1])], "s.png", { type: "image/png" }),
    ]);
    expect(screen.getByAltText("s.png")).toBeTruthy();
    // The `act` wrap is required — the session-change effect's `setAttachments`
    // only reliably flushes inside `act`.
    await act(async () => {
      useSessions.setState({
        activeSessionId: "s2",
        sessions: [
          {
            sessionId: "s2",
            agentId: "a1",
            cwd: "/home/u/proj",
            capabilities: {},
          },
        ],
      });
    });
    expect(screen.queryByAltText("s.png")).toBeNull();
    // The session-change effect released the staged attachment (NOT just
    // cleared the strip) — the object URL was revoked. `toHaveBeenCalledWith`
    // (NOT `toHaveBeenCalledTimes`): `beforeEach`'s `vi.clearAllMocks()`
    // clears the call history per test, but a `some`-style assertion stays
    // robust if that ever changes.
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:mock-s.png");
  });

  // --- Send wiring: images ride along with the prompt (Task 5). ---

  it("send delivers images with the prompt", async () => {
    seedLiveSessionWithImages();
    const { container } = render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "look at this" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // `send()` is ASYNC (it awaits the FileReader read before `sendPrompt`) —
    // wait on the outcome, never assert synchronously.
    await waitFor(() =>
      expect(sendPrompt).toHaveBeenCalledWith("s1", "look at this", [
        { name: "s.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" },
      ]),
    );
    // The store's last user message carries the images.
    const msgs = useSessions.getState().messages.s1;
    const last = msgs[msgs.length - 1];
    expect(last).toMatchObject({ kind: "user", text: "look at this" });
    if (last?.kind === "user") {
      expect(last.images).toEqual([
        { name: "s.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" },
      ]);
    } else {
      expect(last).toBeNull();
    }
    // The composer thumbnail is GONE after success (released + cleared —
    // scoped to the composer: the transcript now renders the image too).
    const composer = container.querySelector(".rounded-2xl")!;
    const remaining = screen.getAllByAltText("s.png");
    expect(remaining.some((el) => composer.contains(el))).toBe(false);
    // …while the transcript renders it (the `data:` URL, read-only).
    expect(remaining).toHaveLength(1);
    expect(remaining[0].getAttribute("src")).toBe(
      "data:image/png;base64,AQID",
    );
  });

  it("a failed send keeps attachments staged", async () => {
    seedLiveSessionWithImages();
    const { container } = render(<ChatStream />);
    vi.mocked(sendPrompt).mockRejectedValueOnce(new Error("boom"));
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // Wait on the OUTCOME (the error line) — NOT on the Send button being
    // enabled: with a staged attachment it is enabled before the click and
    // stays enabled right after (the composer isn't locked until `beginTurn`,
    // which runs only after the FileReader `await`), so a button-state wait
    // would pass immediately and make the test flaky.
    await waitFor(() => expect(screen.getByText("boom")).toBeTruthy());
    // The attachment was NOT released/cleared (a failed send keeps it) —
    // the composer thumbnail is still present.
    const composer = container.querySelector(".rounded-2xl")!;
    const matches = screen.getAllByAltText("s.png");
    expect(matches.some((el) => composer.contains(el))).toBe(true);
  });

  it("a non-Error rejection shows the message, not [object Object]", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    // Tauri IPC errors arrive as plain objects, not `Error` instances.
    vi.mocked(sendPrompt).mockRejectedValueOnce({
      kind: "error",
      message: "proto",
    });
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.getByText("proto")).toBeTruthy());
  });

  it("image-only send is allowed", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    // No text typed — the send button is enabled for an image-only send.
    const sendButton = screen.getByRole("button", { name: "Send" });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    fireEvent.click(sendButton);
    await waitFor(() =>
      expect(sendPrompt).toHaveBeenCalledWith("s1", "", [
        { name: "s.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" },
      ]),
    );
  });

  it("empty composer with no attachments: send stays disabled", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    expect(
      screen.getByRole("button", { name: "Send" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("double-send is guarded by sendingRef", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    expect(screen.getByAltText("s.png")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "hi" },
    });
    const sendButton = screen.getByRole("button", { name: "Send" });
    // Two synchronous clicks BEFORE the first send's FileReader `await`
    // resolves: `beginTurn` (which would lock the composer) runs only AFTER
    // the read, so the composer is still unlocked for the second click —
    // the race the `sendingRef` guard exists for. Without it, both `send()`
    // calls would reach `sendPrompt`.
    fireEvent.click(sendButton);
    fireEvent.click(sendButton);
    // Exactly ONE send, even though the composer was not locked during the
    // first send's FileReader await. Assert the count only AFTER both send
    // continuations have run: a `toHaveBeenCalledTimes(1)` inside `waitFor`
    // would pass the moment it first observed one call — a false-green window
    // if the `sendingRef` guard were removed, since the second send's
    // continuation lands a few ms after the first while `waitFor` samples
    // every 50 ms.
    await waitFor(() => expect(vi.mocked(sendPrompt)).toHaveBeenCalled());
    await flush();
    expect(vi.mocked(sendPrompt)).toHaveBeenCalledTimes(1);
  });

  it("send is fail-closed without the capability", async () => {
    seedLiveSession(); // capabilities {}
    render(<ChatStream />);
    // Task 4: NOT staged (the helper returns `true` — default paste).
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "text" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // A 2-arg call with NO images argument, even though a `File` was on the
    // clipboard (the `imageCapable` guard in `send()`).
    await waitFor(() =>
      expect(sendPrompt).toHaveBeenCalledWith("s1", "text"),
    );
  });

  // --- Mid-read races: the composer is NOT locked until `beginTurn` (which
  // runs only AFTER the FileReader `await`), so the session can switch and
  // thumbnails can be removed DURING the read. `send()` must reconcile
  // against the LIVE state after the `await`, not the stale closure. ---

  it("send is aborted if the session switches during the read", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "hi" },
    });
    // Fake timers BEFORE the send: the read is macrotask-based in jsdom, so
    // faking the timer and advancing it settles the read deterministically
    // (a bare `waitFor` on a negative predicate would pass on the first poll
    // — false green — because the read hasn't settled yet).
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // SYNCHRONOUSLY (before the read settles) switch sessions — the same
    // `act` pattern as `switching sessions clears staged attachments`.
    await act(async () => {
      useSessions.setState({
        activeSessionId: "s2",
        sessions: [
          {
            sessionId: "s2",
            agentId: "a1",
            cwd: "/home/u/proj",
            capabilities: {},
          },
        ],
      });
    });
    // Let the pending FileReader read settle (the continuation then runs and
    // sees the live `activeSessionIdRef` changed → aborts).
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    // `send()` aborted: it never reached `sendPrompt` (no send to the stale
    // `s1`).
    expect(vi.mocked(sendPrompt)).not.toHaveBeenCalled();
    // The draft is PRESERVED (the abort `return` runs before `setDraft("")`):
    // the textarea (now `s2`'s) still holds "hi".
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("hi");
    // NO user message was added to either session's transcript.
    const msgs = useSessions.getState().messages;
    expect((msgs.s1 ?? []).some((m) => m.kind === "user")).toBe(false);
    expect((msgs.s2 ?? []).some((m) => m.kind === "user")).toBe(false);
  });

  it("an image removed during the read is not sent", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "text" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // SYNCHRONOUSLY (before the FileReader resolves) remove the thumbnail —
    // the composer isn't locked until `beginTurn`, so the removal happens
    // during the read.
    fireEvent.click(
      screen.getByRole("button", { name: "Remove image attachment" }),
    );
    // `send()` is ASYNC (it awaits the FileReader read before `sendPrompt`) —
    // wait on the outcome (a 2-arg `sendPrompt` call, images filtered out),
    // never assert synchronously.
    await waitFor(() =>
      expect(sendPrompt).toHaveBeenCalledWith("s1", "text"),
    );
  });

  it("an image-only send with the image removed during the read sends nothing", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    // No text typed — an image-only send.
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // SYNCHRONOUSLY (before the read settles) remove the thumbnail.
    fireEvent.click(
      screen.getByRole("button", { name: "Remove image attachment" }),
    );
    // Let the pending FileReader read settle (the continuation then runs and
    // sees every image removed + no text → nothing left to send).
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    // `send()` aborted (nothing meaningful left to send): `sendPrompt` was
    // NEVER called. (A bare `waitFor` on the negative would be false green —
    // the fake-timer advance guarantees the read has settled first.)
    expect(vi.mocked(sendPrompt)).not.toHaveBeenCalled();
  });

  it("a draft edited during the read is not sent and is not wiped", async () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    pasteToComposer([
      new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" }),
    ]);
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "original" },
    });
    // Fake timers BEFORE the send: the read is macrotask-based in jsdom, so
    // faking the timer and advancing it settles the read deterministically
    // (a bare `waitFor` on a negative predicate would pass on the first poll
    // — false green — because the read hasn't settled yet).
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // SYNCHRONOUSLY (before the read settles) edit the draft — the composer
    // isn't locked until `beginTurn`, so the edit happens during the read.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "original + more" },
    });
    // Let the pending FileReader read settle (the continuation then runs and
    // sees the live draft differs from the stale captured `text` → aborts).
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    // `send()` aborted (the draft changed during the read): `sendPrompt` was
    // NEVER called — neither the stale text nor the new one was sent.
    expect(vi.mocked(sendPrompt)).not.toHaveBeenCalled();
    // The NEW draft is PRESERVED (the abort `return` runs before
    // `setDraft("")`): the textarea still holds the edited text.
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
      "original + more",
    );
    // NO user message was added to the transcript.
    const msgs = useSessions.getState().messages;
    expect((msgs.s1 ?? []).some((m) => m.kind === "user")).toBe(false);
  });

  it("Esc while a turn is in flight calls cancelSession for the active session", async () => {
    seedLiveSession();
    render(<ChatStream />);
    // A turn is in flight (the composer is locked, `inTurn` true). The
    // re-render must settle BEFORE the keypress (the listener is registered
    // in an effect that runs on the re-render).
    useSessions.getState().beginTurn("s1");
    await act(async () => {});
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(vi.mocked(cancelSession)).toHaveBeenCalledWith("s1");
  });

  it("Esc with no turn in flight does NOT call cancelSession", async () => {
    seedLiveSession();
    render(<ChatStream />);
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(vi.mocked(cancelSession)).not.toHaveBeenCalled();
  });

  it("Esc while a turn is in flight of ANOTHER session does NOT call cancelSession", async () => {
    seedLiveSession();
    render(<ChatStream />);
    // The turn is in flight for a DIFFERENT session (not the active one):
    // the listener is gated on the ACTIVE session's `inTurn`, so it is
    // not active and nothing is cancelled.
    useSessions.getState().beginTurn("other");
    await act(async () => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(vi.mocked(cancelSession)).not.toHaveBeenCalled();
  });
});
