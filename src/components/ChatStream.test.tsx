import { beforeEach, describe, expect, it, vi, beforeAll, type Mock } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import ChatStream from "./ChatStream";
import {
  sendPrompt,
  setSessionConfigOption,
  resumeSession,
  readClipboardImage,
  readFileBytes,
  cancelSession,
  listSkills,
  listMcpServersEffective,
  listAgentDefinitionsForSpace,
  listSpaceFiles,
} from "../lib/tauri";
import { open as openFilePicker } from "@tauri-apps/plugin-dialog";
import { clearSkillCatalogCache } from "../hooks/useSkillCatalog";
import { clearMentionCatalogsCache } from "../hooks/useMentionCatalogs";
import { useSessions } from "../store/sessions";
import { useInteractive } from "../store/interactive";
import { usePermissions } from "../store/permissions";
import { useSubagents } from "../store/subagents";
import { useSettings } from "../store/settings";
import { generateFrames, getVariantGridSize } from "../lib/braille-loader";
import type { AgentDefinitionDto, AppSettings, FileListDto } from "../lib/tauri";
import { setSidePaneCollapsed } from "../lib/sidePaneState";

/** A full settings fixture (the spinner-style tests seed the store with it). */
const SETTINGS_FIXTURE: AppSettings = {
  theme: "dark",
  palette: null,
  paneLayout: {},
  defaultTrustNewSpaces: false,
  defaultModel: null,
  defaultThinkingLevel: null,
  enabledTools: [],
  providers: [],
  mcpServers: {},
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
  defaultThinkingLevels: {},
  subagentModels: {},
  spinnerStyle: null,
  filePolicy: { reads: "allow", writes: "allow", shell: "allow" },
  mcpMentionsEnabled: false,
};

/**
 * Seed the settings store with the `#` MCP-mention trigger ENABLED.
 * The trigger is OFF by default (it collides with pasted developer text), so
 * every test that exercises the shipped `#` behavior opts in explicitly here —
 * the same seeding path the spinner-style tests use, so the gate is read from
 * the same store the composer reads.
 */
function seedMcpMentionsOn(): void {
  useSettings
    .getState()
    .setSettings({ ...SETTINGS_FIXTURE, mcpMentionsEnabled: true });
}

/**
 * Seed the settings store with the `#` MCP-mention trigger EXPLICITLY off
 * (the default when nothing is seeded is also off — `settings === null` —
 * so this pins the flag itself rather than the not-yet-loaded fallback).
 */
function seedMcpMentionsOff(): void {
  useSettings
    .getState()
    .setSettings({ ...SETTINGS_FIXTURE, mcpMentionsEnabled: false });
}

/** The `<mcp>` block `expandMentions` produces for the `postgres` fixture
 * server (a message PERSISTED while the `#` trigger was on). */
const PERSISTED_MCP_MESSAGE =
  "query #postgres\n\n" +
  '<mcp name="postgres">' +
  '\nThe user has explicitly named the MCP server "postgres" for this' +
  '\nrequest. Connect to it via the mcp tool (mcp({ connect: "postgres" }))' +
  "\nand use its tools. (Server summary: npx -y x-mcp.)" +
  "\n</mcp>";

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
    startSession: vi.fn().mockResolvedValue({
      sessionId: "s2",
      cwd: "/home/u/proj",
      capabilities: {},
      archived: false,
    }),
    closeSession: vi.fn().mockResolvedValue(undefined),
    sendPrompt: vi.fn().mockResolvedValue("end_turn"),
    resumeSession: vi.fn().mockResolvedValue({
      sessionId: "s1",
      cwd: "/home/u/proj",
      capabilities: { loadSession: true, promptCapabilities: { image: true } },
      archived: false,
    }),
    respondPermission: vi.fn().mockResolvedValue(undefined),
    respondInteractiveRequest: vi.fn().mockResolvedValue(undefined),
    setSessionConfigOption: vi.fn().mockResolvedValue([]),
    readClipboardImage: vi.fn().mockResolvedValue(null),
    readFileBytes: vi.fn().mockResolvedValue(null),
    cancelSession: vi.fn().mockResolvedValue(undefined),
    loadHistory: vi.fn().mockResolvedValue([]),
    listSkills: vi.fn().mockResolvedValue([
      {
        name: "debug",
        description: "Debug a failure",
        path: "/s/.agents/skills/debug/SKILL.md",
        dir: "/s/.agents/skills/debug",
        scope: "space",
        body: "Step 1. Step 2.",
      },
      {
        name: "beta",
        description: "Beta skill",
        path: "/s/.agents/skills/beta/SKILL.md",
        dir: "/s/.agents/skills/beta",
        scope: "space",
        body: "B body",
      },
    ]),
    listAgentDefinitionsForSpace: vi.fn().mockResolvedValue([
      {
        name: "scout",
        description: "Fast recon.",
        model: null,
        scope: "user",
      },
    ]),
    listMcpServersEffective: vi.fn().mockResolvedValue([
      {
        name: "postgres",
        kind: "stdio",
        summary: "npx -y x-mcp",
      },
    ]),
    // The `?` File-completion listing (ADR 0033). Mocked because EVERY
    // session-mounting test in this file calls it (the catalog hook fetches
    // unconditionally), and the `?` tests need to control the payload per
    // test — including a DEFERRED promise for the in-flight case. Empty by
    // default so the pre-existing `$`/`@`/`#` tests see no file rows.
    listSpaceFiles: vi.fn().mockResolvedValue({ entries: [], truncated: false }),
  };
});

// The native file picker (the `+` button's `open` — the real module would
// `invoke` Tauri's dialog plugin, which doesn't exist in jsdom).
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

/**
 * Seed a live session (fresh transcript) in the sessions store.
 */
function seedLiveSession(): void {
  useSessions.setState({
    activeSessionId: "s1",
    sessions: [
      { sessionId: "s1", cwd: "/home/u/proj", capabilities: {}, archived: false },
    ],
    spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1, trusted: false }],
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
    spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1, trusted: false }],
    historySessions: [
      { sessionId: "s1", cwd: "/home/u/proj", capabilities, archived: false },
    ],
    messages: { s1: [] },
    inTurn: {},
    stopReasons: {},
    closeReasons: {},
    configOptions: {},
  });
}

/**
 * Seed a session that lives ONLY in `archivedSessions` (opened from the
 * Archived section): `sessions` and `historySessions` are empty, so the
 * session is neither live nor in the stored list.
 */
function seedArchivedSession(
  capabilities: Record<string, unknown> = {},
): void {
  useSessions.setState({
    activeSessionId: "s9",
    sessions: [],
    spaces: [{ path: "/home/u/arch", createdAt: 1, lastOpenedAt: 1, trusted: false }],
    historySessions: [],
    archivedSessions: [
      { sessionId: "s9", cwd: "/home/u/arch", capabilities, archived: true },
    ],
    messages: { s9: [] },
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
        cwd: "/home/u/proj",
        capabilities: { promptCapabilities: { image: true } },
        archived: false,
      },
    ],
    spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1, trusted: false }],
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
  // `getByRole("textbox")` (the placeholder is the single static
  // "Ask for follow-up changes" — state-independent).
  const target = screen.getByRole("textbox");
  fireEvent.drop(target, { dataTransfer: { files } });
}

/**
 * Flush the microtask queue (the mocked IPC promises resolve after
 * `render` — their state updates must land BEFORE the assertions,
 * wrapped in `act` so React applies them to the DOM).
 */
async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

/**
 * Wait for the skill catalog to load AND render. The mocked `listSkills`
 * resolves in a microtask; the hook's `p.then` then runs `setRows` one or
 * more microtasks later. A `setTimeout(0)` macrotask wait guarantees every
 * pending microtask (the whole promise chain) has run, so the re-render with
 * the loaded rows has landed. (Needed for the send-expansion test, where the
 * picker is NOT open — `findByText` can't be used as the wait signal.)
 */
async function waitForCatalog(): Promise<void> {
  await waitFor(() => expect(vi.mocked(listSkills)).toHaveBeenCalled());
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
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
  // The skill catalog's module-level cache is shared across tests in this
  // file — clear it so each test starts with a COLD cache (a warm cache from
  // a previous test's `seedLiveSession` `cwd` key would make the fresh
  // `listSkills` mock moot: the hook would serve the cached value and never
  // re-fetch).
  clearSkillCatalogCache();
  clearMentionCatalogsCache();
  useSessions.setState({
    activeSessionId: null,
    sessions: [],
    historySessions: [],
    archivedSessions: [],
    spaces: [],
    messages: {},
    inTurn: {},
    stopReasons: {},
    closeReasons: {},
    configOptions: {},
  });
  useInteractive.getState().dismissSession("s1");
  usePermissions.getState().dismissSessionPrompts("s1");
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  useInteractive.getState().dismissSession("sub1");
  usePermissions.getState().dismissSessionPrompts("sub1");
  useSettings.setState({ settings: null, loaded: false });
});

describe("ChatStream", () => {
  it("renders the empty state when there is no active session", () => {
    render(<ChatStream />);
    expect(
      screen.getByText(
        "No active session — open a Space from the tabs above",
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
        "No active session — open a Space from the tabs above",
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

  it("renders the working indicator (BrailleLoader) when inTurn with no interactive state", () => {
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
    // Verify the "Thinking" label (scoped to the transcript's gradient text —
    // the composer's thinking stub shows a "Thinking" placeholder too).
    expect(screen.getByText("Thinking", { selector: ".animated-gradient-text" })).toBeTruthy();
    
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

  it("renders the working indicator from the interactive agentState alone (no inTurn)", async () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useInteractive.getState().applyState("s1", { state: "working" });
    render(<ChatStream />);
    await flush();
    expect(screen.getByRole("status")).toBeTruthy();
  });

  it("renders 'Waiting for your input…' when the interactive agentState is blocked", async () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useInteractive.getState().applyState("s1", { state: "blocked" });
    render(<ChatStream />);
    await flush();
    expect(screen.getByText("Waiting for your input…")).toBeTruthy();
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("pins the working indicator above the composer (below the messages, outside the scroll region)", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useSessions.getState().beginTurn("s1");
    const { container } = render(<ChatStream />);
    const scroll = container.querySelector(".overflow-y-auto");
    const indicator = container.querySelector(
      '[data-testid="working-indicator"]',
    );
    expect(indicator).toBeTruthy();
    // Pinned: NOT inside the scroll region…
    expect(indicator?.closest(".overflow-y-auto")).toBeNull();
    // No border under the row (the faint separator is gone — the row is
    // bare against the composer below it), and no dim filler line after
    // the quip (the row is just the spinner + the quip).
    expect(indicator?.className).not.toContain("border-b");
    expect(indicator?.querySelector(".bg-foreground-subtlest")).toBeNull();
    // Pulled a few px down toward the composer (the row hugs the input
    // box — the gap between them is the composer's `m-3` minus the
    // row's `-mb-2`).
    expect(indicator?.className).toContain("-mb-2");
    // …and BELOW it (right above the composer — NOT at the top of the
    // page): the indicator follows the scroll region in DOM order.
    expect(
      scroll!.compareDocumentPosition(indicator!) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("pins the 'Waiting for your input…' line above the composer too", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useInteractive.getState().applyState("s1", { state: "blocked" });
    const { container } = render(<ChatStream />);
    const scroll = container.querySelector(".overflow-y-auto");
    const indicator = container.querySelector(
      '[data-testid="working-indicator"]',
    );
    expect(indicator).toBeTruthy();
    expect(indicator?.textContent).toContain("Waiting for your input…");
    expect(indicator?.closest(".overflow-y-auto")).toBeNull();
    expect(
      scroll!.compareDocumentPosition(indicator!) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("runs the configured spinner style (the settings store's spinnerStyle)", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useSessions.getState().beginTurn("s1");
    useSettings.getState().setSettings({
      ...SETTINGS_FIXTURE,
      spinnerStyle: "pendulum",
    });
    const { container } = render(<ChatStream />);
    const span = container.querySelector(
      '[data-testid="working-indicator"] span[aria-hidden="true"]',
    );
    // The initial frame of the configured variant (the `BrailleLoader`
    // renders `frames[0]` before its interval ticks).
    const [w, h] = getVariantGridSize("pendulum");
    expect(span?.textContent).toBe(generateFrames("pendulum", w, h).frames[0]);
  });

  it("falls back to the typing spinner when no spinnerStyle is set", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    useSessions.getState().beginTurn("s1");
    const { container } = render(<ChatStream />);
    const span = container.querySelector(
      '[data-testid="working-indicator"] span[aria-hidden="true"]',
    );
    const [w, h] = getVariantGridSize("typing");
    expect(span?.textContent).toBe(generateFrames("typing", w, h).frames[0]);
  });

  it("hides the working indicator when idle (no inTurn, no agentState)", () => {
    seedLiveSession();
    useSessions.getState().addUserMessage("s1", "hi");
    const { container } = render(<ChatStream />);
    expect(
      container.querySelector('[data-testid="working-indicator"]'),
    ).toBeNull();
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

  it("renders the composer as a borderless ISLAND wearing the content surface", () => {
    // Two decisions, both pinned as negatives because both are one-keyboard
    // away from being silently reverted.
    //
    // NO BORDER: the frame used to be `border border-input-border`, i.e.
    // Dracula's Comment `#6272a4` — a slate VIOLET that reads as a purple
    // outline around the chat (`focus-within:border-input-border-focused` put
    // Functional Purple `#815cd6` on it while typing). The token is SHARED with
    // `input.tsx`/`select.tsx`/`ModelPicker`, which KEEP their borders, so this
    // is a per-site decision and only a class assertion can tell the two apart.
    //
    // SAME FILL AS THE TRANSCRIPT: the island is `bg-chat`, the same
    // token the transcript column and the side pane wear. That is only legible
    // because it is a SIBLING of the `main` with chrome between them — see the
    // next test. As a child it would have been invisible at 1.00.
    seedLiveSession();
    const { container } = render(<ChatStream />);
    const shell = container.querySelector('[data-testid="composer-island"]');
    expect(shell).toBeTruthy();
    expect(shell!.className).toContain("bg-composer");
    expect(shell!.className).not.toContain("border-input-border");
    expect(shell!.className).not.toMatch(/(^|\s)border(\s|$)/);
  });

  it("keeps the composer a SIBLING of the transcript, not its child", () => {
    // THE LOAD-BEARING STRUCTURE of the island layout. Chrome frames each
    // island, so the gap between them IS the separation: siblings of the same
    // fill read as two surfaces, a nested one reads as one. If the composer is
    // ever moved back inside the `main`, `bg-chat` on both collapses
    // to a single invisible plane (CR 1.00) — and with the border gone there is
    // nothing left to show the boundary.
    seedLiveSession();
    const { container } = render(<ChatStream />);
    const main = container.querySelector("main")!;
    const island = container.querySelector('[data-testid="composer-island"]')!;
    expect(main.contains(island)).toBe(false);
    // Same visual family as the transcript and the side pane: same radius, same
    // 4px margin, and `shrink-0` so the island never absorbs the column's
    // height shortfall (the `main` is `flex-1`/basis-0 and cannot shrink).
    expect(island.className).toContain("rounded-xl");
    expect(island.className).toContain("shrink-0");
    expect(main.className).toContain("rounded-xl");
  });

  it("the root main carries min-h-0 (a vertical flex item must be allowed to shrink — the transcript scrolls internally, not the window)", () => {
    seedLiveSession();
    const { container } = render(<ChatStream />);
    const main = container.querySelector("main");
    expect(main).toBeTruthy();
    // The `main` is a `flex-1` item of the center column (a `flex-col`):
    // without `min-h-0` the item's `min-height: auto` keeps it at the
    // transcript's height — it overflows the window (the composer is
    // pushed off-screen) and the inner `overflow-y-auto` div never scrolls.
    expect(main!.className).toContain("min-h-0");
  });

  it("shows the SINGLE placeholder 'Ask for follow-up changes' for a fresh live session (no history — the state if/else is gone)", () => {
    seedLiveSession();
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
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

  it("keeps the SINGLE placeholder for a live working session (the 'Agent is working…' if/else is gone — the working indicator carries the state)", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
  });

  it("shows the SAME placeholder for a stored resumable session (no pause concept shown)", () => {
    seedStoredSession({ loadSession: true });
    render(<ChatStream />);
    // The stored session looks normal: the SAME single placeholder (the
    // auto-resume on send is silent — the pause/resume concept is not
    // shown).
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
    expect(
      screen.queryByPlaceholderText("Resume this session to send"),
    ).toBeNull();
  });

  it("shows the normal follow-up placeholder for a stored resumable session with messages", () => {
    seedStoredSession({ loadSession: true });
    useSessions.getState().addUserMessage("s1", "hi");
    render(<ChatStream />);
    // Identical to a live session with messages — no "Resume" wording.
    expect(
      screen.getByPlaceholderText("Ask for follow-up changes"),
    ).toBeTruthy();
  });

  it("shows the SAME placeholder for a history-only (closed) session (the 'This session is closed' if/else is gone)", () => {
    seedStoredSession();
    render(<ChatStream />);
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
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

  it("keeps the SEND disabled for a non-resumable stored session (a dead session can't receive a prompt — the harness rejects it)", () => {
    seedStoredSession(); // capabilities {} — no `loadSession`
    render(<ChatStream />);
    // The textarea stays editable (you can DRAFT — it just can't be SENT:
    // a non-resumable session has no agent process to deliver to, and the
    // harness's `send_prompt` rejects a prompt for an unknown session).
    const textarea = screen.getByRole("textbox");
    expect(textarea.hasAttribute("disabled")).toBe(false);
    expect(
      screen.getByRole("button", { name: "Send" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("renders the composer's bottom row for a closed (history-only) session (the context bar + model/thinking selects — the `isLive` gates are gone)", () => {
    seedStoredSession();
    const { container } = render(<ChatStream />);
    const composer = container.querySelector('[data-testid="composer-island"]')!;
    // The context bar renders (an empty 0% track — the session's context is
    // dropped on close and re-emits on resume; the label waits for data).
    const bar = composer.querySelector(
      '[data-testid="context-usage-bar"]',
    ) as HTMLElement;
    expect(bar).toBeTruthy();
    const progress = bar.querySelector(
      '[role="progressbar"][aria-label="Context used"]',
    ) as HTMLElement;
    expect(progress.getAttribute("aria-valuenow")).toBe("0");
    expect(bar.querySelector('[data-testid="context-usage"]')).toBeNull();
    // The model/thinking selects render as DISABLED stubs (the config
    // options are dropped on close — no data to show; the identity stays).
    const model = composer.querySelector('[aria-label="Model"]') as HTMLElement;
    expect(model).toBeTruthy();
    expect(model.hasAttribute("disabled")).toBe(true);
    const thinking = composer.querySelector(
      '[aria-label="Thinking"]',
    ) as HTMLElement;
    expect(thinking).toBeTruthy();
    expect(thinking.hasAttribute("disabled")).toBe(true);
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

  // --- Task 5 (ADR 0016): the manual Resume affordances are GONE — the
  // banner is purely informational and the first send auto-resumes. ---

  it("a resumable stored session renders NO banner and no Resume button (it looks normal)", () => {
    seedStoredSession({ loadSession: true });
    render(<ChatStream />);
    // The pause concept is not shown: no stored/resume banner, no
    // history-only banner, no Resume button — the first send auto-resumes
    // silently.
    expect(
      screen.queryByText(
        "This session is stored. Sending a message resumes it.",
      ),
    ).toBeNull();
    expect(screen.queryByText(/History only/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
  });

  it("an archived-only session renders no banner and an enabled composer, and first send resumes", async () => {
    seedArchivedSession({ loadSession: true, promptCapabilities: { image: true } });
    render(<ChatStream />);
    // The `historySession` selector searches BOTH stored lists, so an
    // archived-only session gets an enabled composer (not "This session
    // is closed") and NO banner (the pause concept is not shown).
    expect(
      screen.queryByText(
        "This session is stored. Sending a message resumes it.",
      ),
    ).toBeNull();
    const textarea = screen.getByRole("textbox");
    expect(textarea.hasAttribute("disabled")).toBe(false);
    fireEvent.change(textarea, {
      target: { value: "resume the archived one" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // The first send auto-resumes the archived session…
    await waitFor(() =>
      expect(resumeSession).toHaveBeenCalledWith(
        "s9",
        "/home/u/arch",
      ),
    );
    // …and the message landed in `messages[s9]`.
    await waitFor(() =>
      expect(useSessions.getState().messages["s9"]).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            kind: "user",
            text: "resume the archived one",
          }),
        ]),
      ),
    );
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
    const textarea = screen.getByRole("textbox");
    // Empty draft → disabled.
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    // Non-empty draft, idle → enabled.
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    // inTurn (working) → send disabled again (the textarea stays EDITABLE
    // — you can draft while the agent works; Enter no-ops via `send()`'
    // `composerLocked` guard, the draft is kept for when the turn ends).
    await act(async () => {
      useSessions.getState().beginTurn("s1");
    });
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(false);
  });

  it("calls sendPrompt when Enter is pressed with a draft", () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox");
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

  it("renders a stacked interactive ask on an empty transcript (no fresh-session hint)", () => {
    seedLiveSession();
    // No `toolCallId` → the card is NOT queued (it renders immediately,
    // unanchored).
    useInteractive.getState().addRequest("s1", {
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
    useInteractive.getState().addRequest("s1", {
      requestId: "req-pw",
      method: "password",
      source: "main",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    render(<ChatStream />);
    expect(screen.getByText("Sudo password required")).toBeTruthy();
    expect(screen.queryByText("Send a prompt to start")).toBeNull();
  });

  it("renders 'Waiting for your input…' on an empty transcript when the interactive agentState is blocked", async () => {
    seedLiveSession();
    useInteractive.getState().applyState("s1", { state: "blocked" });
    render(<ChatStream />);
    await flush();
    expect(screen.getByText("Waiting for your input…")).toBeTruthy();
    expect(screen.queryByText("Send a prompt to start")).toBeNull();
  });

  // --- Composer mismatch states (ONE predicate: `workingOrInTurn`). ---

  it("disables the send button and no-ops send() when the interactive agentState is working (no inTurn)", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByRole("textbox");
    // Idle → enabled (sanity).
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    // The interactive agentState says working (the `state` push races `turnCompleted`, or
    // the state latches after a turn) → the send is dead: button disabled,
    // the textarea stays editable (the draft is kept; Enter no-ops).
    await act(async () => {
      useInteractive.getState().applyState("s1", { state: "working" });
    });
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(false);
    // The placeholder is the single idle copy (the `working` mismatch does
    // not leave a stale copy behind — the working indicator carries the
    // state).
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).not.toHaveBeenCalled();
  });

  it("disables the send button and no-ops Enter while the interactive agentState is blocked (mid-turn awaiting a request response)", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    const textarea = screen.getByRole("textbox");
    // Idle → enabled (sanity: `blocked` has NOT been pushed yet).
    fireEvent.change(textarea, { target: { value: "hi" } });
    expect(sendButton.hasAttribute("disabled")).toBe(false);
    expect(textarea.hasAttribute("disabled")).toBe(false);
    // `inTurn` is true (a `blocked` agent is mid-turn — it is awaiting a
    // request response, so `inTurn` alone used to lock the composer) +
    // the interactive agentState says blocked → the send is dead for the whole
    // wait: button disabled (the textarea stays editable — the draft is
    // kept; Enter no-ops, no double-send, no clobbered turn bookkeeping).
    await act(async () => {
      useSessions.getState().beginTurn("s1");
      useInteractive.getState().applyState("s1", { state: "blocked" });
    });
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(false);
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).not.toHaveBeenCalled();
  });

  it("keeps the composer LOCKED when the interactive agentState is idle but the store is inTurn", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    useInteractive.getState().applyState("s1", { state: "idle" });
    render(<ChatStream />);
    const sendButton = screen.getByRole("button", { name: "Send" });
    // A stale `idle` from the previous turn must not unlock the composer
    // while a turn is in flight (`inTurn` is the ground truth — the store
    // does not reset `agentState` on `turnCompleted`, a store follow-up):
    // the send is disabled and Enter no-ops (the textarea stays editable
    // — the draft is kept). The working indicator shows (the turn IS in
    // flight).
    const textarea = screen.getByRole("textbox");
    expect(sendButton.hasAttribute("disabled")).toBe(true);
    expect(textarea.hasAttribute("disabled")).toBe(false);
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(sendPrompt).not.toHaveBeenCalled();
    expect(screen.queryByRole("status")).toBeTruthy();
  });

  it("prefers the 'Waiting for your input…' line over the braille loader when blocked AND inTurn", () => {
    seedLiveSession();
    useSessions.getState().beginTurn("s1");
    useInteractive.getState().applyState("s1", { state: "blocked" });
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
    // (left of the send button).
    const composer = container.querySelector('[data-testid="composer-island"]')!;
    expect(composer.querySelector('[aria-label="Model"]')).toBeTruthy();
    expect(composer.querySelector('[aria-label="Thinking"]')).toBeTruthy();
    // The model trigger shows the BARE id + the provider suffix (the
    // composed `provider/id` key is the VALUE, not the display text).
    expect(screen.getByText(/alpha/)).toBeTruthy();
    expect(screen.getByText("Medium")).toBeTruthy();
  });

  it("groups the model + thinking triggers in ONE tight cluster (they are one block of session config, not three evenly-spaced controls)", () => {
    seedLiveSession();
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
          {
            id: "thought_level",
            name: "Thinking",
            type: "select",
            currentValue: "medium",
            options: [{ value: "medium", name: "Medium" }],
          },
        ],
      },
    });
    const { container } = render(<ChatStream />);
    // Both triggers sit inside a single wrapper whose inner gap (`gap-1`) is
    // TIGHTER than the `gap-2` that separates the cluster from Send.
    const group = container.querySelector(
      '[data-testid="composer-config-controls"]',
    ) as HTMLElement;
    expect(group).toBeTruthy();
    expect(group.className).toContain("gap-1");
    expect(group.className).not.toContain("gap-2");
    expect(
      group.querySelector('[aria-label="Model"]') &&
        group.querySelector('[aria-label="Thinking"]'),
    ).toBeTruthy();
    // The cluster is the toolbar's last item before Send (the toolbar's own
    // `gap-2` is the wider separation).
    const toolbar = group.parentElement!;
    expect(toolbar.className).toContain("gap-2");
    expect(toolbar.lastElementChild?.getAttribute("aria-label")).toBe("Send");
  });

  // --- The context bar (the dynamic percentage — a progress bar spanning
  // from the `+` button to the model selector, the reference UI's
  // `🧠 [====bar====] 61%` look). ---

  it("renders a dynamic context bar in the composer when the usage is known", () => {
    seedLiveSession();
    useSessions.setState({
      contextUsage: { s1: { used: 53760, window: 128000 } },
    });
    const { container } = render(<ChatStream />);
    // 53760 / 128000 = 42% — inside the composer (NOT the header).
    const composer = container.querySelector('[data-testid="composer-island"]')!;
    const group = composer.querySelector(
      '[data-testid="context-usage-bar"]',
    ) as HTMLElement | null;
    expect(group).toBeTruthy();
    const bar = group!.querySelector(
      '[role="progressbar"][aria-label="Context used"]',
    ) as HTMLElement | null;
    expect(bar).toBeTruthy();
    expect(bar!.getAttribute("aria-valuemin")).toBe("0");
    expect(bar!.getAttribute("aria-valuemax")).toBe("100");
    expect(bar!.getAttribute("aria-valuenow")).toBe("42");
    // The fill's width follows the percentage, and 42% (< 50%) is the
    // green band (the label follows the fill's color).
    const fill = bar!.firstElementChild as HTMLElement;
    expect(fill.style.width).toBe("42%");
    expect(fill.className).toContain("bg-success");
    expect(fill.className).not.toContain("bg-caution");
    // The label sits at the bar's right edge (the reference UI's `61%`).
    const labelEl = group!.querySelector('[data-testid="context-usage"]') as HTMLElement;
    expect(labelEl.textContent).toBe("42%");
    expect(labelEl.className).toContain("text-success");
    // The brain icon cues the bar (the reference UI's `🧠` placement).
    expect(group!.querySelector('[data-testid="context-bar-icon"]')).toBeTruthy();
  });

  // The token counts are HOVER-ONLY on the number (the bar row is dense —
  // `53,760 / 128,000 tokens` inline would push the model selector off the
  // row, and the native `title` tooltip is unstyled and ~1s slow).
  it("the context number reveals the token counts on hover, not inline", () => {
    seedLiveSession();
    useSessions.setState({
      contextUsage: { s1: { used: 53760, window: 128000 } },
    });
    render(<ChatStream />);
    const label = screen.getByTestId("context-usage");
    // The inline text stays the bare percentage.
    expect(label.textContent).toBe("42%");
    // No native tooltip (the styled Radix one replaces it).
    expect(label.getAttribute("title")).toBeNull();
    // CLOSED until hover — the counts are not in the DOM at rest.
    expect(document.body.textContent).not.toContain("53,760");
    // Radix `TooltipTrigger` opens on pointer/focus (see the
    // `ToolCallCard` failure-tooltip test — `mouseEnter` does not fire it
    // in this Radix version; `focus` does; `TooltipProvider`'s
    // `delayDuration = 0` mounts the portal synchronously).
    fireEvent.focus(label);
    expect(document.body.textContent).toContain("53,760 / 128,000 tokens");
  });

  // The counts are keyboard-accessible: the tooltip trigger is a focusable
  // control (a Tab stop) whose accessible name carries the token counts,
  // and the progressbar exposes them as its text alternative — so nothing
  // about the context is hover-only.
  it("the context counts are reachable by keyboard, not hover alone", () => {
    seedLiveSession();
    useSessions.setState({
      contextUsage: { s1: { used: 53760, window: 128000 } },
    });
    render(<ChatStream />);
    const label = screen.getByTestId("context-usage");
    // A real control in the tab order (a bare `<span>` was not).
    expect(label.tagName).toBe("BUTTON");
    expect(label.getAttribute("tabindex")).not.toBe("-1");
    // The accessible name carries the counts (a screen reader hears them
    // without opening the tooltip).
    expect(label.getAttribute("aria-label")).toBe(
      "Context used: 53,760 of 128,000 tokens (42%)",
    );
    // The progressbar's text alternative carries them too.
    const bar = screen
      .getByTestId("context-usage-bar")
      .querySelector('[role="progressbar"][aria-label="Context used"]') as HTMLElement;
    expect(bar.getAttribute("aria-valuetext")).toBe(
      "53,760 of 128,000 tokens (42%)",
    );
    // And focus (keyboard) opens the tooltip, as hover does.
    fireEvent.focus(label);
    expect(document.body.textContent).toContain("53,760 / 128,000 tokens");
  });

  it("a caution-band context percentage (50–69%) renders the fill in caution", () => {
    seedLiveSession();
    useSessions.setState({
      contextUsage: { s1: { used: 78080, window: 128000 } },
    });
    render(<ChatStream />);
    const group = screen.getByTestId("context-usage-bar");
    const bar = group.querySelector(
      '[role="progressbar"][aria-label="Context used"]',
    ) as HTMLElement;
    expect(bar.getAttribute("aria-valuenow")).toBe("61");
    // 61% is the caution band (50–69%): caution (NOT green, NOT warning),
    // and the label follows the fill's color.
    const fill = bar.firstElementChild as HTMLElement;
    expect(fill.className).toContain("bg-caution");
    expect(fill.className).not.toContain("bg-success");
    expect(fill.className).not.toContain("bg-warning");
    const label = group.querySelector('[data-testid="context-usage"]') as HTMLElement;
    expect(label.textContent).toBe("61%");
    expect(label.className).toContain("text-caution");
  });

  it("a mid context percentage (70–89%) escalates the fill to the warning color", () => {
    seedLiveSession();
    useSessions.setState({
      contextUsage: { s1: { used: 96000, window: 128000 } },
    });
    render(<ChatStream />);
    const group = screen.getByTestId("context-usage-bar");
    const bar = group.querySelector(
      '[role="progressbar"][aria-label="Context used"]',
    ) as HTMLElement;
    expect(bar.getAttribute("aria-valuenow")).toBe("75");
    // 75% is the warning band (70–89%): warning (NOT caution, NOT red),
    // and the label follows the fill's color.
    const fill = bar.firstElementChild as HTMLElement;
    expect(fill.className).toContain("bg-warning");
    expect(fill.className).not.toContain("bg-caution");
    expect(fill.className).not.toContain("bg-destructive");
    const label = group.querySelector('[data-testid="context-usage"]') as HTMLElement;
    expect(label.textContent).toBe("75%");
    expect(label.className).toContain("text-warning");
  });

  it("a high context percentage (>= 90%) escalates the fill to the destructive color", () => {
    seedLiveSession();
    useSessions.setState({
      contextUsage: { s1: { used: 115200, window: 128000 } },
    });
    render(<ChatStream />);
    const group = screen.getByTestId("context-usage-bar");
    const bar = group.querySelector(
      '[role="progressbar"][aria-label="Context used"]',
    ) as HTMLElement;
    expect(bar.getAttribute("aria-valuenow")).toBe("90");
    // 90% is the red band (90–100%): the destructive red (the label
    // follows the fill's color).
    const fill = bar.firstElementChild as HTMLElement;
    expect(fill.className).toContain("bg-destructive");
    expect(fill.className).not.toContain("bg-warning");
    const label = group.querySelector('[data-testid="context-usage"]') as HTMLElement;
    expect(label.textContent).toBe("90%");
    expect(label.className).toContain("text-destructive");
  });

  it("renders an empty context bar (no label) when the usage is unknown", () => {
    seedLiveSession();
    useSessions.setState({ contextUsage: {} });
    render(<ChatStream />);
    // The bar renders from the start (layout stability — the fill appears
    // as the session grows) but the label waits for the first frame.
    const bar = screen.getByRole("progressbar", { name: "Context used" });
    expect(bar.getAttribute("aria-valuenow")).toBe("0");
    const fill = bar.firstElementChild as HTMLElement;
    expect(fill.style.width).toBe("0%");
    expect(bar.querySelector('[data-testid="context-usage"]')).toBeNull();
  });

  it("renders the context bar for a stored session as an empty 0% track (the context is dropped on close and re-emits on resume)", () => {
    seedStoredSession();
    render(<ChatStream />);
    // The bar is ALWAYS rendered (the `isLive` gate is gone): a stored
    // session's context is dropped on close — the track is the empty 0%
    // fill (the label waits for the resume to re-emit the usage).
    const bar = screen.getByRole("progressbar", { name: "Context used" });
    expect(bar.getAttribute("aria-valuenow")).toBe("0");
    expect(screen.queryByTestId("context-usage")).toBeNull();
  });

  it("renders the bottom bar POPULATED for a stored session (config options + context usage from the session row — the selectors are disabled)", () => {
    // The `list_sessions` shape: the row carries `configOptions` (the
    // stored `model` / `thinkingLevel` synthesized) + `contextUsage` (the
    // last known usage — persisted on every `context_usage_update` frame).
    useSessions.setState({
      activeSessionId: "s1",
      sessions: [],
      spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1, trusted: false }],
      historySessions: [
        {
          sessionId: "s1",
          cwd: "/home/u/proj",
          capabilities: { loadSession: true },
          archived: false,
          configOptions: [
            {
              id: "model",
              name: "Model",
              type: "select",
              currentValue: "tama/m1",
              options: [{ value: "tama/m1", name: "m1" }],
            },
            {
              id: "thought_level",
              name: "Thinking",
              category: "thought_level",
              type: "select",
              currentValue: "high",
              options: [{ value: "high", name: "High" }],
            },
          ],
          contextUsage: { used: 53760, window: 128000 },
        },
      ],
      messages: { s1: [] },
      inTurn: {},
      stopReasons: {},
      closeReasons: {},
      configOptions: {},
      contextUsage: {},
    });
    render(<ChatStream />);
    // The context bar is FILLED from the row's usage (53760 / 128000 ≈
    // 42% — the green band).
    const bar = screen.getByRole("progressbar", { name: "Context used" });
    expect(bar.getAttribute("aria-valuenow")).toBe("42");
    expect(screen.getByTestId("context-usage").textContent).toBe("42%");
    // The model selector is POPULATED with the stored model's name, but
    // DISABLED (a stored session can't set config — no agent process to
    // deliver `set_config_option` to).
    const model = screen.getByRole("button", { name: "Model" });
    expect(model.hasAttribute("disabled")).toBe(true);
    expect(model.textContent).toContain("m1 · tama");
    // The thinking selector is POPULATED with the stored level + disabled.
    const thinking = screen.getByRole("combobox");
    expect(thinking.hasAttribute("disabled")).toBe(true);
    expect(thinking.textContent).toContain("High");
  });

  it("a stored session's live config options win over the row's (the store's map is fresher — the last frame before the close)", () => {
    // The store's map has a FRESHER value (the session was live in this
    // app session, closed, and the store kept its last known config — the
    // `handleSessionClosed` no longer drops it); the row's value is the
    // DB's (persisted on the same frame — same value, but the store's
    // map is the primary source).
    seedStoredSession();
    useSessions.setState({
      configOptions: {
        s1: [
          {
            id: "model",
            name: "Model",
            type: "select",
            currentValue: "tama/m2",
            options: [{ value: "tama/m2", name: "m2" }],
          },
        ],
      },
      contextUsage: { s1: { used: 100000, window: 128000 } },
    });
    render(<ChatStream />);
    const model = screen.getByRole("button", { name: "Model" });
    expect(model.textContent).toContain("m2 · tama");
    const bar = screen.getByRole("progressbar", { name: "Context used" });
    expect(bar.getAttribute("aria-valuenow")).toBe("78"); // 100000/128000 ≈ 78%
  });

  // --- The `+` attach button (the native file picker) ---

  it("renders an enabled `+` (Attach files) button for a live image-capable session", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    const button = screen.getByRole("button", { name: "Attach files" });
    expect(button.hasAttribute("disabled")).toBe(false);
  });

  it("the `+` button is disabled when the agent does not advertise image support (fail-closed)", () => {
    seedLiveSession(); // capabilities: {} — no `promptCapabilities.image`
    render(<ChatStream />);
    const button = screen.getByRole("button", { name: "Attach files" });
    expect(button.hasAttribute("disabled")).toBe(true);
  });

  it("the `+` button is disabled while a turn is in flight (the composer is locked)", () => {
    seedLiveSessionWithImages();
    useSessions.setState({ inTurn: { s1: true } });
    render(<ChatStream />);
    const button = screen.getByRole("button", { name: "Attach files" });
    expect(button.hasAttribute("disabled")).toBe(true);
  });

  it("clicking `+` opens the file picker and stages the picked image", async () => {
    seedLiveSessionWithImages();
    vi.mocked(openFilePicker).mockResolvedValueOnce([
      "/home/u/pics/shot.png",
    ]);
    vi.mocked(readFileBytes).mockResolvedValueOnce([
      0x89, 0x50, 0x4e, 0x47, 1, 2, 3,
    ]);
    render(<ChatStream />);
    fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
    // The picker opens (the dialog's image filter — the backend re-validates
    // the extension, the frontend re-validates the MIME on staging).
    expect(vi.mocked(openFilePicker)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(openFilePicker).mock.calls[0][0]).toMatchObject({
      multiple: true,
    });
    // The picked file is staged as an attachment (the thumbnail appears —
    // the bytes were read via `read_file_bytes` and the MIME inferred from
    // the extension).
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Remove image attachment" }),
      ).toBeTruthy(),
    );
    expect(vi.mocked(readFileBytes)).toHaveBeenCalledWith(
      "/home/u/pics/shot.png",
    );
  });

  it("a cancelled picker (null) stages nothing", async () => {
    seedLiveSessionWithImages();
    vi.mocked(openFilePicker).mockResolvedValueOnce(null);
    render(<ChatStream />);
    fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
    await waitFor(() => expect(vi.mocked(openFilePicker)).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: "Remove image attachment" })).toBeNull();
    expect(vi.mocked(readFileBytes)).not.toHaveBeenCalled();
  });

  it("a picker selection of a non-image path is skipped (the read returns null)", async () => {
    seedLiveSessionWithImages();
    vi.mocked(openFilePicker).mockResolvedValueOnce([
      "/home/u/pics/notes.txt",
    ]);
    vi.mocked(readFileBytes).mockResolvedValueOnce(null);
    render(<ChatStream />);
    fireEvent.click(screen.getByRole("button", { name: "Attach files" }));
    await waitFor(() => expect(vi.mocked(readFileBytes)).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "Remove image attachment" })).toBeNull();
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
    // The model picker is a DIALOG (the trigger is a button — the catalog
    // is too long for a Radix dropdown).
    fireEvent.click(screen.getByRole("button", { name: "Model" }));
    // The row name is the BARE id; the sent value is the full composed key.
    fireEvent.click(screen.getByText("beta"));
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
        { sessionId: "s1", cwd: "/home/u/proj", capabilities: {}, archived: false },
        { sessionId: "s2", cwd: "/home/u/proj", capabilities: {}, archived: false },
      ],
      spaces: [{ path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1, trusted: false }],
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

  it("shows the single placeholder for an image-capable fresh session (the capability if/else is gone)", () => {
    seedLiveSessionWithImages();
    render(<ChatStream />);
    expect(
      screen.getByPlaceholderText("Ask for follow-up changes"),
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
    // The single placeholder (the paste hint if/else is gone).
    expect(screen.getByPlaceholderText("Ask for follow-up changes")).toBeTruthy();
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
            cwd: "/home/u/proj",
            capabilities: {},
            archived: false,
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
    const composer = container.querySelector('[data-testid="composer-island"]')!;
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
    const composer = container.querySelector('[data-testid="composer-island"]')!;
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
            cwd: "/home/u/proj",
            capabilities: {},
            archived: false,
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

  it("a paste-fallback clipboard read that straddles a session switch stages nothing (session guard)", async () => {
    seedLiveSessionWithImages();
    // A second image-capable session to switch TO (its composer would
    // stage the image without the guard).
    useSessions.setState({
      sessions: [
        {
          sessionId: "s1",
          cwd: "/home/u/proj",
          capabilities: { promptCapabilities: { image: true } },
          archived: false,
        },
        {
          sessionId: "s2",
          cwd: "/home/u/proj",
          capabilities: { promptCapabilities: { image: true } },
          archived: false,
        },
      ],
    });
    render(<ChatStream />);
    // Deferred promise: the clipboard read is SLOW (in flight across the
    // session switch — the race the guard exists for).
    let resolveRead: (value: number[] | null) => void = () => {};
    vi.mocked(readClipboardImage).mockImplementationOnce(
      () => new Promise<number[] | null>((r) => (resolveRead = r)),
    );
    // WebKitGTK quirk path: an empty paste event (no files, no text).
    pasteToComposer([]);
    expect(vi.mocked(readClipboardImage)).toHaveBeenCalledTimes(1);
    // Switch the active session WHILE the read is in flight.
    await act(async () => {
      useSessions.setState({ activeSessionId: "s2" });
    });
    // The session-change effect cleared s1's staged attachments (sanity).
    expect(screen.queryByAltText("screenshot.png")).toBeNull();
    // Settle the read: the image was captured for s1 — it must NOT be
    // staged into the NEW active session (s2).
    await act(async () => {
      resolveRead([0x89, 0x50, 0x4e, 0x47]);
      await Promise.resolve();
    });
    expect(screen.queryByAltText("screenshot.png")).toBeNull();
  });

  it("Esc on a focused agent question card dismisses the card WITHOUT cancelling the turn", async () => {
    seedLiveSession();
    // A pending agent question (no `toolCallId` → not queued; unanchored →
    // the card takes focus on mount, so its root div is the focusable
    // element an Esc keypress lands on).
    useInteractive.getState().addRequest("s1", {
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
    // A turn is in flight (the window Esc listener is active) — the Esc
    // meant for the card must NOT also cancel the turn.
    useSessions.getState().beginTurn("s1");
    render(<ChatStream />);
    // The listener is registered in an effect — settle the re-render
    // before the keypress.
    await act(async () => {});
    // The card root is the focusable element (`tabIndex={-1}`).
    const card = screen.getByText("Which approach?").closest(
      "[tabindex='-1']",
    );
    expect(card).toBeTruthy();
    await act(async () => {
      fireEvent.keyDown(card!, { key: "Escape" });
    });
    // The card's own Esc handler dismissed the request (cancelled)…
    await waitFor(() =>
      expect(useInteractive.getState().requests["s1"]).toHaveLength(0),
    );
    // …and the window listener did NOT fire (no `session/cancel`).
    expect(vi.mocked(cancelSession)).not.toHaveBeenCalled();
  });

  it("Esc on the sudo password modal's input dismisses the modal WITHOUT cancelling the turn", async () => {
    seedLiveSession();
    useInteractive.getState().addRequest("s1", {
      requestId: "req-pw",
      method: "password",
      source: "main",
      params: { command: "apt install ripgrep", reason: "install the tool" },
    });
    // A turn is in flight (the window Esc listener is active).
    useSessions.getState().beginTurn("s1");
    render(<ChatStream />);
    await act(async () => {});
    const input = screen.getByRole("textbox", { name: "Password" });
    await act(async () => {
      fireEvent.keyDown(input, { key: "Escape" });
    });
    // The modal's Esc handler dismissed the request (cancelled)…
    await waitFor(() =>
      expect(useInteractive.getState().requests["s1"]).toHaveLength(0),
    );
    // …and the window listener did NOT fire (no `session/cancel`).
    expect(vi.mocked(cancelSession)).not.toHaveBeenCalled();
  });

  it("a resumed session whose FRESH agent lacks image capability sends text-only without the staged image (no stale capability)", async () => {
    // The STORED session's SAVED capabilities advertise images (an older
    // agent) — so the paste is staged and the send button is enabled.
    seedStoredSession({ loadSession: true, promptCapabilities: { image: true } });
    // The FRESH agent (the resume result) does NOT advertise images.
    vi.mocked(resumeSession).mockResolvedValueOnce({
      sessionId: "s1",
      cwd: "/home/u/proj",
      capabilities: { loadSession: true },
      archived: false,
    });
    render(<ChatStream />);
    pasteToComposer(
      [new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" })],
    );
    expect(screen.getByAltText("s.png")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "look at this" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(resumeSession).toHaveBeenCalledWith("s1", "/home/u/proj"),
    );
    // Fail-closed: the fresh agent can't take images → 2-arg send, the
    // image is NOT attached (and not sent a second time).
    await waitFor(() =>
      expect(sendPrompt).toHaveBeenCalledWith("s1", "look at this"),
    );
    expect(vi.mocked(sendPrompt)).toHaveBeenCalledTimes(1);
  });

  it("an image-only send on a resumed session whose FRESH agent lacks image capability is blocked (fail-closed)", async () => {
    seedStoredSession({ loadSession: true, promptCapabilities: { image: true } });
    vi.mocked(resumeSession).mockResolvedValueOnce({
      sessionId: "s1",
      cwd: "/home/u/proj",
      capabilities: { loadSession: true },
      archived: false,
    });
    render(<ChatStream />);
    pasteToComposer(
      [new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" })],
    );
    // No text — an image-only send (enabled by the SAVED capability).
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(resumeSession).toHaveBeenCalledWith("s1", "/home/u/proj"),
    );
    // Blocked: no prompt is sent (an empty prompt + unsent images is
    // meaningless — the same fail-closed posture as `!imageCapable`).
    expect(vi.mocked(sendPrompt)).not.toHaveBeenCalled();
  });

  // --- Mentions: the `$`/`#`/`@`-trigger picker + the send-path expansion.
  // One picker, filtered to the active prefix's catalog; rows carry a prefix
  // badge. The send-path expansion assertions below are the `expandMentions`
  // regressions (all three catalogs).

  it("typing $ opens the mention picker with the skill rows ($ badge)", async () => {
    seedLiveSession();
    render(<ChatStream />);
    // A bare `$` (empty token remainder) opens the picker with the FULL list.
    // The catalog arrives via an ASYNC effect (the `listSkills` mock), so the
    // picker only renders once the fetch resolves — `findByText` awaits both
    // the fetch and the render.
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "$" } });
    await screen.findByText("debug");
    // One row per skill, each with a `$` prefix badge (the generalization's
    // only visual delta vs the old skill picker).
    const picker = screen.getByTestId("mention-picker");
    expect(picker.querySelectorAll("button")).toHaveLength(2);
    // The prefix badge is the `text-ui-xs` span (the only such span per row).
    const firstBadge = picker.querySelector(".text-ui-xs")!;
    expect(firstBadge.textContent).toBe("$");
  });

  it("typing # opens the picker with the MCP rows (# badge)", async () => {
    seedLiveSession();
    seedMcpMentionsOn();
    render(<ChatStream />);
    // `#` → the ACTIVE prefix's catalog is the effective MCP servers (the
    // `listMcpServersEffective` mock). `findByText` awaits the async fetch.
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "#" } });
    const row = await screen.findByText("postgres");
    // The `#` badge is on the row (the `@`/`$` glyphs must not mix in).
    expect(row.closest("button")!.querySelector(".text-ui-xs")!.textContent).toBe("#");
    // The skill catalog is NOT listed under `#` (no mixing).
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("typing @ opens the picker with the agent rows (@ badge)", async () => {
    seedLiveSession();
    render(<ChatStream />);
    // `@` → the ACTIVE prefix's catalog is the agent definitions (the
    // `listAgentDefinitionsForSpace` mock). `findByText` awaits the async fetch.
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "@" } });
    const row = await screen.findByText("scout");
    // The `@` badge is on the row (the `#`/`$` glyphs must not mix in).
    expect(row.closest("button")!.querySelector(".text-ui-xs")!.textContent).toBe("@");
    // Neither the MCP server nor a skill mixes in under `@`.
    expect(screen.queryByText("postgres")).toBeNull();
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("a mixed draft lists only the prefix of the token at the caret", async () => {
    seedLiveSession();
    seedMcpMentionsOn();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // The caret is at the END of what is typed (where a user's caret sits
    // after typing). Caret on the `$a` token → the picker is on the `$`
    // prefix: the skill row `beta` matches the `a` substring, and NO `#`/
    // `@` row mixes in (a `$`/`#`/`@` never mixes in one open list).
    fireEvent.change(textarea, { target: { value: "$a" } });
    expect(await screen.findByText("beta")).toBeTruthy();
    expect(screen.queryByText("postgres")).toBeNull();
    expect(screen.queryByText("scout")).toBeNull();
    // The user keeps typing to the `#pos` token: the picker flips to the `#`
    // prefix (the `postgres` server matches `pos`) and the `$` row drops out.
    fireEvent.change(textarea, { target: { value: "$a #pos" } });
    expect(await screen.findByText("postgres")).toBeTruthy();
    expect(screen.queryByText("beta")).toBeNull();
  });

  it("the picker filters as the token is typed", async () => {
    seedLiveSession();
    render(<ChatStream />);
    // `$de` → the single `debug` skill matches (the catalog is still loading
    // here — `findByText` awaits it).
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "$de" } });
    await screen.findByText("debug");
    // `$zzz` → no name contains `zzz` → the picker is not visible. The catalog
    // is already warm at this point, so no await is needed for the change.
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "$zzz" } });
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("enter selects the highlighted skill and inserts the token", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "$de" } });
    await screen.findByText("debug"); // catalog ready, picker open
    // The caret is at the END of the text (where a user's caret is after
    // typing — jsdom's `selectionStart` is 0 by default).
    textarea.setSelectionRange(3, 3);
    // Enter selects the highlighted row (`debug`) and inserts `$debug `.
    fireEvent.keyDown(textarea, { key: "Enter" });
    // The draft is local `useState` in `ChatStream` (NOT in any store) — read
    // it back from the TEXTAREA's `value` on the next render.
    expect(
      (screen.getByRole("textbox") as HTMLTextAreaElement).value,
    ).toBe("$debug ");
    // The picker is closed after selection.
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("escape closes the picker without inserting", async () => {
    seedLiveSession();
    render(<ChatStream />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "$de" } });
    await screen.findByText("debug"); // catalog ready, picker open
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Escape" });
    // The picker is gone…
    expect(screen.queryByText("debug")).toBeNull();
    // …and the draft is UNCHANGED (no insertion).
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("$de");
  });

  it("send expands the mention before addUserMessage and sendPrompt", async () => {
    seedLiveSession();
    render(<ChatStream />);
    // Ensure the catalog is loaded (the expansion depends on it) — the picker
    // is NOT open here (the trailing space closes it), so `findByText` can't
    // be the wait signal.
    await waitForCatalog();
    // The trailing space ends the active token → the picker is NOT open and
    // Enter is NOT intercepted by the picker's `selectMention` branch. `rawText`
    // = `draft.trim()` = `fix $debug`.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "fix $debug " },
    });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    // The expanded text (the exact pi-format block, Task 3).
    const expanded =
      'fix $debug\n\n<skill name="debug" location="/s/.agents/skills/debug/SKILL.md">' +
      "\nReferences are relative to /s/.agents/skills/debug." +
      "\n\nStep 1. Step 2.\n</skill>";
    // `addUserMessage` got the EXPANDED text (the live bubble = the agent
    // input — the REFINEMENT invariant).
    const msgs = useSessions.getState().messages["s1"];
    const userMsg = msgs?.find((m) => m.kind === "user");
    expect(userMsg).toBeTruthy();
    if (userMsg?.kind === "user") {
      expect(userMsg.text.startsWith("fix $debug")).toBe(true);
      expect(userMsg.text).toContain(
        '<skill name="debug" location="/s/.agents/skills/debug/SKILL.md">',
      );
      expect(userMsg.text).toContain(
        "References are relative to /s/.agents/skills/debug.",
      );
      expect(userMsg.text).toContain("Step 1. Step 2.");
      expect(userMsg.text).toBe(expanded);
    }
    // `sendPrompt` got the SAME expanded text (the 2-arg form — no images).
    expect(sendPrompt).toHaveBeenCalledWith("s1", expanded);
  });

  it("an unmatched token is sent verbatim", async () => {
    seedLiveSession();
    render(<ChatStream />);
    await waitForCatalog();
    // `$nope` matches no skill → `expandMentions` finds no match and
    // returns the text UNCHANGED. The trailing space closes the picker.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "hi $nope " },
    });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    // `rawText` is `hi $nope` and nothing was expanded.
    expect(sendPrompt).toHaveBeenCalledWith("s1", "hi $nope");
  });

  // --- Task 6 (composer-mentions): `send()` expands via the THREE-catalog
  // `expandMentions` (the `@` agent / `#` MCP soft blocks alongside the
  // hard `$` skill block). ---

  it("send expands an @ agent mention into the soft agent block",
    async () => {
      seedLiveSession();
      render(<ChatStream />);
      await waitForCatalog();
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "ping @scout " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      // The EXACT agent-block literal (Task 3) — the `—` is an EM DASH.
      const expanded =
        "ping @scout\n\n" +
        '<agent name="scout">' +
        "\nscout \u2014 Fast recon." +
        '\nThe user has explicitly named the agent definition "scout" for this' +
        '\nrequest. Dispatch a subagent with agentName "scout" to handle it' +
        "\n(the definition's frontmatter defines its model, tools, and system" +
        "\nprompt; your explicit tool params layer over the definition)." +
        "\n</agent>";
      // The live bubble and the agent input carry the SAME text (the
      // `mergeDedupeKey` invariant).
      const userMsg = useSessions
        .getState()
        .messages["s1"]?.find((m) => m.kind === "user");
      expect(userMsg?.kind === "user" ? userMsg.text : null).toBe(expanded);
      expect(sendPrompt).toHaveBeenCalledWith("s1", expanded);
      // End-to-end (the `done-when`): the LIVE bubble the send just appended
      // renders the soft-hint chip, not the raw block text.
      expect(screen.getByText("named by the user")).toBeTruthy();
      expect(screen.queryByText(/Dispatch a subagent with agentName/)).toBeNull();
    });

  it("send expands a # MCP mention into the soft mcp block",
    async () => {
      seedLiveSession();
      seedMcpMentionsOn();
      render(<ChatStream />);
      await waitForCatalog();
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "query #postgres " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      const expanded =
        "query #postgres\n\n" +
        '<mcp name="postgres">' +
        '\nThe user has explicitly named the MCP server "postgres" for this' +
        '\nrequest. Connect to it via the mcp tool (mcp({ connect: "postgres" }))' +
        "\nand use its tools. (Server summary: npx -y x-mcp.)" +
        "\n</mcp>";
      expect(sendPrompt).toHaveBeenCalledWith("s1", expanded);
    });

  it("send expands a mixed $ / @ / # draft into blocks in first-mention order",
    async () => {
      seedLiveSession();
      seedMcpMentionsOn();
      render(<ChatStream />);
      await waitForCatalog();
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "x $debug @scout #postgres " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      const sent = vi.mocked(sendPrompt).mock.calls[0]![1] as string;
      expect(sent.startsWith("x $debug @scout #postgres")).toBe(true);
      // One block per kind, appended in TEXT order (first-mention order).
      const skill = sent.indexOf('<skill name="debug"');
      const agent = sent.indexOf('<agent name="scout">');
      const mcp = sent.indexOf('<mcp name="postgres">');
      expect(skill).toBeGreaterThan(-1);
      expect(agent).toBeGreaterThan(skill);
      expect(mcp).toBeGreaterThan(agent);
    });

  it("an unmatched # token passes through byte-identically into the sent text",
    async () => {
      seedLiveSession();
      render(<ChatStream />);
      await waitForCatalog();
      // `#nope` matches no MCP server → NO block and the token is NOT
      // stripped (the `mergeDedupeKey` byte-identity invariant).
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "hi #nope " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "hi #nope");
    });

  it("an unmatched @ token passes through byte-identically into the sent text",
    async () => {
      seedLiveSession();
      render(<ChatStream />);
      await waitForCatalog();
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "hi @nope " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "hi @nope");
    });

  it("arrow_down_moves_the_highlight_and_enter_selects_the_second_row", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "$" } });
    await screen.findByText("debug"); // catalog ready, picker open (2 rows)
    const debugRow = screen.getByRole("button", { name: /debug/ });
    const betaRow = screen.getByRole("button", { name: /beta/ });
    // `debug` (index 0) is highlighted by default.
    expect(debugRow.className).toContain("bg-surface-hover");
    expect(betaRow.className).not.toContain("bg-surface-hover");
    // The caret is at the END of the text (where a user's caret is after
    // typing the `$` — jsdom's `selectionStart` is 0 by default).
    textarea.setSelectionRange(1, 1);
    fireEvent.keyDown(textarea, { key: "ArrowDown" });
    // The highlight moved to the `beta` row.
    expect(debugRow.className).not.toContain("bg-surface-hover");
    expect(betaRow.className).toContain("bg-surface-hover");
    // Enter selects the HIGHLIGHTED row (`beta`).
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(textarea.value).toBe("$beta ");
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("arrow_up_wraps_around_to_the_last_row", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "$" } });
    await screen.findByText("debug"); // catalog ready, picker open (2 rows)
    const debugRow = screen.getByRole("button", { name: /debug/ });
    const betaRow = screen.getByRole("button", { name: /beta/ });
    textarea.setSelectionRange(1, 1);
    fireEvent.keyDown(textarea, { key: "ArrowUp" }); // wraps to the LAST row (index 1)
    expect(betaRow.className).toContain("bg-surface-hover");
    expect(debugRow.className).not.toContain("bg-surface-hover");
    fireEvent.keyDown(textarea, { key: "ArrowUp" }); // back to index 0
    expect(debugRow.className).toContain("bg-surface-hover");
    // Enter selects the FIRST row.
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(textarea.value).toBe("$debug ");
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("tab_accepts_the_highlighted_candidate", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "$" } });
    await screen.findByText("debug"); // catalog ready, picker open
    textarea.setSelectionRange(1, 1);
    // Tab accepts the highlighted candidate (ZCode parity: `KEY_TAB_COMMAND`
    // → `selectOption(selectedIndex)` — the same handler Enter uses).
    fireEvent.keyDown(textarea, { key: "Tab" });
    expect(textarea.value).toBe("$debug ");
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("tab_with_the_picker_closed_falls_through", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "hello " } });
    // No `$` → the picker never opens.
    expect(screen.queryByText("debug")).toBeNull();
    // Tab is NOT intercepted (no `preventDefault` — `fireEvent` returns
    // `false` when the default was prevented) and the value is untouched.
    const notPrevented = fireEvent.keyDown(textarea, { key: "Tab" });
    expect(notPrevented).toBe(true);
    expect(textarea.value).toBe("hello ");
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("enter_after_the_caret_leaves_the_token_closes_the_picker_instead_of_sending_or_swallowing", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // `$debug` matches the `debug` skill → the picker is open, the token
    // active at the caret.
    fireEvent.change(textarea, { target: { value: "$debug" } });
    await screen.findByText("debug"); // picker open
    // Move the caret AWAY from the token WITHOUT `onChange` (a mouse click
    // or Home does this — no keystroke fires `change`): the picker is now
    // STALE (it would still `selectMention` the highlighted `debug` on Enter
    // if the keydown didn't re-evaluate the token at the caret).
    textarea.setSelectionRange(0, 0);
    fireEvent.keyDown(textarea, { key: "Enter" });
    // The stale picker must NOT swallow Enter: the picker closes AND the
    // message SENDS the draft as-typed (the send path expands the `$debug`
    // mention — the picker did not re-select or rewrite it: the verbatim
    // token stays in the text, the block is APPENDED by `send()`).
    // (The picker's CLOSED state is asserted via the picker's testid — the
    // skill's NAME now also renders in the sent message's collapsed skill
    // card header, so a text query is no longer a picker-only signal.)
    expect(screen.queryByTestId("mention-picker")).toBeNull();
    expect(textarea.value).toBe("");
    const sent =
      "$debug\n\n" +
      '<skill name="debug" location="/s/.agents/skills/debug/SKILL.md">' +
      "\nReferences are relative to /s/.agents/skills/debug." +
      "\n\nStep 1. Step 2.\n</skill>";
    expect(sendPrompt).toHaveBeenCalledWith("s1", sent);
  });

  it("shift_enter_inserts_a_newline_with_the_picker_open", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "$de" } });
    await screen.findByText("debug"); // picker open
    textarea.setSelectionRange(3, 3);
    // Shift+Enter must NOT be intercepted (no `preventDefault` — the
    // browser default inserts a newline; jsdom doesn't run it, so the
    // value only changes if the handler itself changes it).
    const notPrevented = fireEvent.keyDown(textarea, {
      key: "Enter",
      shiftKey: true,
    });
    expect(notPrevented).toBe(true);
    // NOT a skill selection (the draft is unchanged — not `$debug `) and
    // NOT swallowed.
    expect(textarea.value).toBe("$de");
    // Model the browser default (the newline lands): the token ends at the
    // newline → the picker closes on `onChange`.
    fireEvent.change(textarea, { target: { value: "$de\n" } });
    expect(textarea.value).toBe("$de\n");
    expect(screen.queryByText("debug")).toBeNull();
  });

  it("shift_tab_falls_through_with_the_picker_open", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "$de" } });
    await screen.findByText("debug"); // picker open, token active
    textarea.setSelectionRange(3, 3);
    // Shift+Tab must NOT be intercepted (a11y: it moves focus BACKWARD —
    // the browser default; intercepting it would be a keyboard trap). No
    // `preventDefault` (`fireEvent` returns `false` when the default was
    // prevented) and the draft is unchanged.
    const notPrevented = fireEvent.keyDown(textarea, {
      key: "Tab",
      shiftKey: true,
    });
    expect(notPrevented).toBe(true);
    expect(textarea.value).toBe("$de");
    // The picker is still open (no selection happened).
    expect(screen.getByTestId("mention-picker")).toBeTruthy();
  });

  it("the_insert_event_closes_the_picker", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // The picker is open with a token active at the caret (the v1.1 flow:
    // the user typed `$x`, then clicks a SkillsDialog row).
    fireEvent.change(textarea, { target: { value: "$de" } });
    await screen.findByText("debug"); // picker open
    textarea.focus();
    textarea.setSelectionRange(3, 3); // caret at the end of the token
    // The `act` wrap is REQUIRED (the file's insert-event test convention):
    // a raw `dispatchEvent` does not flush React's state updates.
    await act(async () => {
      window.dispatchEvent(
        new CustomEvent("archimedes:insert-skill", { detail: "$beta " }),
      );
    });
    // The stale picker is CLOSED (no lingering popup until the next
    // keydown) …
    expect(screen.queryByTestId("mention-picker")).toBeNull();
    // …and the inserted text was spliced at the caret.
    expect(textarea.value).toBe("$de$beta ");
  });

  it("the left-pane insert event appends at the caret", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // An existing draft, caret at the end (the focus + selection the event
    // reads from the ref/DOM).
    fireEvent.change(textarea, { target: { value: "hello " } });
    textarea.focus();
    textarea.setSelectionRange(6, 6);
    // The `act` wrap is REQUIRED: a raw `dispatchEvent` does not flush React's
    // state updates (the listener's `setDraft` would be invisible to a
    // synchronous assertion — the file's `fireEvent.paste` comment documents
    // this convention). No catalog dependency: the insert event is independent
    // of `listSkills`.
    await act(async () => {
      window.dispatchEvent(
        new CustomEvent("archimedes:insert-skill", { detail: "$debug " }),
      );
    });
    expect(textarea.value).toBe("hello $debug ");
  });

  // --- Task 5 (composer-mentions): the picker generalizes to `#` / `@`. ---

  it("enter selects an MCP row and inserts the #-token", async () => {
    seedLiveSession();
    seedMcpMentionsOn();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "#" } });
    await screen.findByText("postgres"); // catalog ready, picker open
    textarea.setSelectionRange(1, 1);
    // Enter selects the highlighted row and inserts `#postgres ` (LOWER-cased —
    // the inserted token must stay expandable).
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(textarea.value).toBe("#postgres ");
    expect(screen.queryByText("postgres")).toBeNull();
  });

  it("tab selects an agent row and inserts the @-token", async () => {
    seedLiveSession();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: "@" } });
    await screen.findByText("scout"); // catalog ready, picker open
    textarea.setSelectionRange(1, 1);
    fireEvent.keyDown(textarea, { key: "Tab" });
    expect(textarea.value).toBe("@scout ");
    expect(screen.queryByText("scout")).toBeNull();
  });

  it("an unknown # token leaves the picker closed and the draft untouched", async () => {
    seedLiveSession();
    seedMcpMentionsOn();
    render(<ChatStream />);
    const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
    // `#nosauch` matches no server in the catalog → no rows → the picker is
    // gated off (the token is untouched — no error, no stripping).
    fireEvent.change(textarea, { target: { value: "#nosauch" } });
    await waitFor(() => expect(vi.mocked(listMcpServersEffective)).toHaveBeenCalled());
    expect(screen.queryByTestId("mention-picker")).toBeNull();
    expect(textarea.value).toBe("#nosauch");
  });

  // --- Review follow-ups (F3/F4/F6): the lowercase-insertion policy, the
  // `filtered` memo's catalog deps, and the picker row affordances. ---

  it("a picker selection inserts the LOWER-cased token even for an uppercase catalog name",
    async () => {
      // The case policy: tokens are lowercase-only, so a picker-selected row
      // must insert `<prefix><name.toLowerCase()> ` or the inserted token
      // could never expand on send (the regex is lowercase-only). The
      // catalog name stays VERBATIM only inside the expanded BLOCK.
      vi.mocked(listAgentDefinitionsForSpace).mockResolvedValueOnce([
        { name: "Scout", description: "Fast recon.", model: null, scope: "user" },
      ]);
      seedLiveSession();
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "@" } });
      // The ROW shows the catalog name verbatim (`Scout`) …
      const row = await screen.findByText("Scout");
      expect(row.closest("button")).toBeTruthy();
      textarea.setSelectionRange(1, 1);
      // … but the INSERTED token is lowercased.
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "@scout ",
      );
    });

  it("a picker opened while the agents catalog is still loading shows the rows when the fetch resolves (no further keystroke)",
    async () => {
      // The `filtered` `useMemo` deps are `[skills, agents, mcpServers,
      // picker]`. Drop `agents` (or `mcpServers`) and the memo keeps the
      // EMPTY array it computed while the fetch was in flight — the picker
      // stays empty until the NEXT keystroke re-runs the memo.
      let resolveAgents: (rows: AgentDefinitionDto[]) => void = () => {};
      vi.mocked(listAgentDefinitionsForSpace).mockImplementationOnce(
        () => new Promise<AgentDefinitionDto[]>((r) => (resolveAgents = r)),
      );
      seedLiveSession();
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      // Open the picker on the `@` prefix while the agents fetch PENDING.
      fireEvent.change(textarea, { target: { value: "@" } });
      await waitFor(() =>
        expect(vi.mocked(listAgentDefinitionsForSpace)).toHaveBeenCalled(),
      );
      // Nothing to show yet (the fetch has not resolved).
      expect(screen.queryByTestId("mention-picker")).toBeNull();
      // The fetch resolves — and NO further keystroke happens.
      await act(async () => {
        resolveAgents([
          {
            name: "scout",
            description: "Fast recon.",
            model: null,
            scope: "user",
          },
        ]);
        await Promise.resolve();
      });
      const row = screen.getByText("scout");
      expect(row.closest("button")!.querySelector(".text-ui-xs")!.textContent).toBe(
        "@",
      );
      // And the late row is SELECTABLE (the memo recomputed the rows, not
      // just the render): Enter inserts it.
      textarea.setSelectionRange(1, 1);
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "@scout ",
      );
    });

  it("a mouse click on a # row inserts the token (mousedown, not click)",
    async () => {
      seedLiveSession();
      seedMcpMentionsOn();
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "#" } });
      const row = (await screen.findByText("postgres")).closest("button")!;
      textarea.focus();
      textarea.setSelectionRange(1, 1);
      // The row handles `onMouseDown` with `preventDefault` (so the textarea
      // never loses focus to the button) — a `click` would NOT select.
      const notPrevented = fireEvent.mouseDown(row);
      expect(notPrevented).toBe(false);
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "#postgres ",
      );
      // The picker closed after the mouse selection.
      expect(screen.queryByTestId("mention-picker")).toBeNull();
    });

  it("a # row's description (the MCP summary) renders as a title tooltip",
    async () => {
      seedLiveSession();
      seedMcpMentionsOn();
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), { target: { value: "#" } });
      const row = (await screen.findByText("postgres")).closest("button")!;
      // The `#` catalog has no `description` field — the row's tooltip is the
      // server's one-line `summary`.
      const desc = row.querySelector("[title]")!;
      expect(desc.getAttribute("title")).toBe("npx -y x-mcp");
      expect(desc.textContent).toBe("npx -y x-mcp");
    });

  it("a mouse click on an @ row inserts the token (mousedown, not click)",
    async () => {
      seedLiveSession();
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "@" } });
      const row = (await screen.findByText("scout")).closest("button")!;
      textarea.focus();
      textarea.setSelectionRange(1, 1);
      // The `@` (agents) twin of the `#` test above: the row handles
      // `onMouseDown` with `preventDefault` (so the textarea never loses focus
      // to the button) — a `click` would NOT select.
      const notPrevented = fireEvent.mouseDown(row);
      expect(notPrevented).toBe(false);
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "@scout ",
      );
      // The picker closed after the mouse selection.
      expect(screen.queryByTestId("mention-picker")).toBeNull();
    });

  it("an @ row's description (the agent description) renders as a title tooltip",
    async () => {
      seedLiveSession();
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), { target: { value: "@" } });
      const row = (await screen.findByText("scout")).closest("button")!;
      // The `@` catalog DOES have a `description` field — the row's tooltip is
      // the agent definition's description verbatim.
      const desc = row.querySelector("[title]")!;
      expect(desc.getAttribute("title")).toBe("Fast recon.");
      expect(desc.textContent).toBe("Fast recon.");
    });

  // ---------------------------------------------------------------------
  // The `#` MCP-mention toggle (ADR 0031's follow-up). `#` is the ONE prefix
  // that collides with pasted developer text (`#include`, `#123`, `#hashtag`
  // all satisfy the token grammar), so the trigger is opt-in. The gate is
  // expressed as an EMPTY MCP catalog: the picker and the expansion are
  // already catalog-gated, so no new code path exists. The DISPLAY side
  // (`splitMentionBlocks`) is deliberately NOT gated — a message persisted
  // while the trigger was on keeps rendering its chip.
  // ---------------------------------------------------------------------

  it("the # trigger is OFF by default: typing # opens no picker", async () => {
    seedLiveSession();
    seedMcpMentionsOff();
    render(<ChatStream />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "#" } });
    // The MCP fetch HAS run (the fetch is NOT gated — gating it would make a
    // live toggle require a remount), but no rows are fed to the picker.
    await waitFor(() =>
      expect(vi.mocked(listMcpServersEffective)).toHaveBeenCalled(),
    );
    await flush();
    expect(screen.queryByTestId("mention-picker")).toBeNull();
    expect(screen.queryByText("postgres")).toBeNull();
  });

  it("with the # trigger OFF, a matching # token is sent BYTE-IDENTICAL (no expansion)",
    async () => {
      seedLiveSession();
      seedMcpMentionsOff();
      render(<ChatStream />);
      await waitForCatalog();
      // `postgres` IS in the catalog — the OFF state must still leave the text
      // untouched (the empty-catalog gate hits `expandMentions`, which then
      // finds no match and returns the text verbatim: invariant 1).
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "query #postgres " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "query #postgres");
      const userMsg = useSessions
        .getState()
        .messages["s1"]?.find((m) => m.kind === "user");
      expect(userMsg?.kind === "user" ? userMsg.text : null).toBe(
        "query #postgres",
      );
      // And the bubble renders no chip (the text carried no block).
      expect(screen.queryByText("named by the user")).toBeNull();
    });

  it("with the # trigger OFF, Enter with a bare # draft SENDS it (no swallowed keys)",
    async () => {
      seedLiveSession();
      seedMcpMentionsOff();
      render(<ChatStream />);
      await waitForCatalog();
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "#123" } });
      await flush();
      // No picker → the keyboard branches (all gated on `filtered.length > 0`)
      // are inert, so Enter is a plain send. `#123` is exactly the pasted-issue
      // number that must reach the agent untouched.
      textarea.setSelectionRange(4, 4);
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "#123");
    });

  it("with the # trigger OFF, the $ and @ triggers still pick and expand (only # is gated)",
    async () => {
      seedLiveSession();
      seedMcpMentionsOff();
      render(<ChatStream />);
      await waitForCatalog();
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      // The pickers: `@` still opens (the `@` catalog is untouched).
      fireEvent.change(textarea, { target: { value: "@" } });
      expect(await screen.findByText("scout")).toBeTruthy();
      // And a MIXED draft expands `$` + `@` but NOT `#` — one block per
      // ENABLED kind, in first-mention order, with the `#` token left verbatim
      // in the user's text.
      fireEvent.change(textarea, {
        target: { value: "x $debug @scout #postgres " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      const sent = vi.mocked(sendPrompt).mock.calls[0]![1] as string;
      expect(sent.startsWith("x $debug @scout #postgres")).toBe(true);
      expect(sent).toContain('<skill name="debug"');
      expect(sent).toContain('<agent name="scout">');
      expect(sent).not.toContain('<mcp name="postgres">');
    });

  it("the # trigger is gated while the settings are still loading (the fail-safe default)",
    async () => {
      // No settings seeded at all (`settings === null`) — the composer must
      // behave as OFF, never as ON (`settings?.mcpMentionsEnabled ?? false`).
      seedLiveSession();
      render(<ChatStream />);
      await waitForCatalog();
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "query #postgres " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "query #postgres");
      expect(screen.queryByTestId("mention-picker")).toBeNull();
    });

  it("flipping the setting while the # picker is open feeds the rows without a remount",
    async () => {
      // The `filtered` memo's deps include the flag: the picker is open on `#`
      // (no rows, so nothing renders), and flipping the store to ON makes the
      // row appear with NO further keystroke and NO remount.
      seedLiveSession();
      seedMcpMentionsOff();
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), { target: { value: "#" } });
      await waitFor(() =>
        expect(vi.mocked(listMcpServersEffective)).toHaveBeenCalled(),
      );
      await flush();
      expect(screen.queryByTestId("mention-picker")).toBeNull();
      await act(async () => {
        seedMcpMentionsOn();
      });
      expect((await screen.findByText("postgres")).closest("button")).toBeTruthy();
    });

  it("a persisted <mcp> block still renders its chip with the # trigger OFF (display is not gated)",
    () => {
      // The gate is on the TRIGGER, not the display: a message sent while the
      // toggle was ON keeps rendering its soft-hint chip after the toggle goes
      // OFF (the record is unchanged, and `mergeDedupeKey` must stay stable).
      seedLiveSession();
      seedMcpMentionsOff();
      useSessions
        .getState()
        .addUserMessage("s1", PERSISTED_MCP_MESSAGE);
      render(<ChatStream />);
      expect(screen.getByText("named by the user")).toBeTruthy();
      expect(screen.getByText("postgres")).toBeTruthy();
      // The user's own text renders without the block prose.
      expect(screen.queryByText(/Connect to it via the mcp tool/)).toBeNull();
    });
  // ---------------------------------------------------------------------
  // `?` File completion (ADR 0033) — the FOURTH picker prefix, and the ONLY
  // one that is NOT a Mention: selecting a row inserts a PATH (the trigger is
  // CONSUMED, the case is PRESERVED) and the send path expands NOTHING. The
  // block below pins the rows, the insertion policy, the render cap, the note,
  // and the two degradations (in-flight listing / no-match note).
  //
  // BEFORE these tests exist, `filtered` has no `?` branch, so a `?` token
  // falls through to the SKILLS catalog — which is exactly the transient state
  // these tests are written to catch.
  // ---------------------------------------------------------------------

  // The picker's row `<button>` whose PRIMARY line is `name` — the
  // `text-ui-base` span. A `?` row's button holds TWO spans on the primary
  // line (the `text-ui-xs` prefix badge and the name), so "the row has one
  // span" is never the right assertion; single-liness is pinned by the
  // ABSENCE of the description node (see the single-line test below).
  async function findFileRow(name: string): Promise<HTMLButtonElement> {
    const matches = await screen.findAllByText(name);
    const primary = matches.find((el) => el.classList.contains("text-ui-base"));
    return primary!.closest("button") as HTMLButtonElement;
  }

  it("typing ?RE opens the picker with the file rows and a ? badge", async () => {
    seedLiveSession();
    vi.mocked(listSpaceFiles).mockResolvedValue({
      entries: ["README.md", "docs/plan.md"],
      truncated: false,
    });
    render(<ChatStream />);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "?RE" } });
    const row = await findFileRow("README.md");
    // The `?` badge (the picker renders `row.prefix`, so `?` needs no new
    // markup) …
    expect(row.querySelector(".text-ui-xs")!.textContent).toBe("?");
    // … and NO skill row mixes in (the fall-through bug: `?` listed skills).
    expect(screen.queryByText("debug")).toBeNull();
    expect(screen.queryByText("beta")).toBeNull();
  });

  it("a path-shaped ? query matches by subsequence, not substring", async () => {
    seedLiveSession();
    vi.mocked(listSpaceFiles).mockResolvedValue({
      entries: ["src/components/Formula.tsx", "README.md"],
      truncated: false,
    });
    render(<ChatStream />);
    // `src/comp/Form` is NOT a substring of `src/components/Formula.tsx` — a
    // path query is path-SHAPED, which is why the `?` branch filters with
    // `fuzzyMatch` (a subsequence match) instead of `includes`.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "?src/comp/Form" },
    });
    expect(await findFileRow("src/components/Formula.tsx")).toBeTruthy();
    // `README.md` matches nothing in that query, so it is not listed.
    expect(screen.queryAllByText("README.md")).toEqual([]);
  });

  it("enter on a file row inserts the path with the ? CONSUMED and the CASE INTACT",
    async () => {
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md"],
        truncated: false,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      // The ADR's own example: the query is case-INSENSITIVE, the INSERTED path
      // carries the real name.
      fireEvent.change(textarea, { target: { value: "?REA" } });
      expect(await findFileRow("README.md")).toBeTruthy();
      textarea.setSelectionRange(4, 4);
      fireEvent.keyDown(textarea, { key: "Enter" });
      // NOT `?README.md ` (the trigger is consumed) and NOT `readme.md `
      // (the insertion is VERBATIM — no lowercasing rule applies to a path, so
      // the Mention lowercase-only rule does NOT apply to `?`. The path is
      // then ordinary draft text and obeys the mention grammar like
      // hand-typed text — ADR 0033).
      expect(
        (screen.getByRole("textbox") as HTMLTextAreaElement).value,
      ).toBe("README.md ");
      expect(screen.queryByTestId("mention-picker")).toBeNull();
    });

  it("tab on a file row inserts the path (trigger consumed, case intact)",
    async () => {
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["src/README.md"],
        truncated: false,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "?src/READ" } });
      expect(await findFileRow("src/README.md")).toBeTruthy();
      textarea.setSelectionRange(9, 9);
      fireEvent.keyDown(textarea, { key: "Tab" });
      expect(
        (screen.getByRole("textbox") as HTMLTextAreaElement).value,
      ).toBe("src/README.md ");
    });

  it("a mousedown on a file row inserts the path (trigger consumed, case intact)",
    async () => {
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md"],
        truncated: false,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "?REA" } });
      const row = await findFileRow("README.md");
      textarea.focus();
      textarea.setSelectionRange(4, 4);
      // `onMouseDown` (with `preventDefault`) is the row's handler — a `click`
      // would NOT select (the same affordance as the Mention rows).
      const notPrevented = fireEvent.mouseDown(row);
      expect(notPrevented).toBe(false);
      expect(
        (screen.getByRole("textbox") as HTMLTextAreaElement).value,
      ).toBe("README.md ");
      expect(screen.queryByTestId("mention-picker")).toBeNull();
    });

  it("a file row is a single line: no basename second line, no tooltip, full path still shown",
    async () => {
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["src/nested/README.md"],
        truncated: false,
      });
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "?src/nested/READ" },
      });
      const row = await findFileRow("src/nested/README.md");
      // The PRIMARY line is the full relative path — what gets INSERTED, so it
      // must never be shortened to the basename (`selectMention` inserts
      // `row.name`).
      expect(row.querySelector(".text-ui-base")!.textContent).toBe(
        "src/nested/README.md",
      );
      // And there is NO secondary line. The basename used to be repeated there
      // — the tail of the path already shown above it — which made every `?`
      // row two lines tall (≈49px) for ZERO information. `ComposerMentions`
      // renders the second line only when `description !== ""`, so the
      // description node, and with it the `title` tooltip, must be absent.
      expect(row.querySelector("[title]")).toBeNull();
      expect(row.querySelector(".text-ui-sm")).toBeNull();
      // Pinned against the wrong "fix" (dropping `name` instead of
      // `description`): the row still has its prefix badge and its name span.
      expect(row.querySelector(".text-ui-xs")!.textContent).toBe("?");
    });

  it("a truncated listing shows the cap note, which is NOT a selectable row, and Enter still sends",
    async () => {
      seedLiveSession();
      // Truncated AND nothing matches: the note is exactly what explains the
      // EMPTY picker (the container gate widened to `open && (rows || note)`).
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md"],
        truncated: true,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "?zzzz" } });
      const picker = await screen.findByTestId("mention-picker");
      const note = picker.querySelector(
        "[data-testid='mention-picker-note']",
      )!;
      // NO number in the string: the DTO carries only `truncated`, so naming
      // the Rust `MAX_PICKER_ENTRIES` here would let the UI lie the moment the
      // Rust cap moves (ADR 0033's note is qualitative on purpose).
      // And NO "keep typing" advice: the walk is deterministic
      // (`sort_by_file_name`), so a truncated listing holds the alphabetically
      // FIRST 5,000 entries and a truncated-away file is unreachable by
      // narrowing — advising it would be a lie.
      expect(note.textContent).toBe(
        "File listing capped — some files may be missing",
      );
      // The note is NEVER a row: it is not in the button set `activeIndex`,
      // the arrow-wrap and Enter operate over.
      expect(picker.querySelectorAll("button")).toHaveLength(0);
      expect(note.tagName).not.toBe("BUTTON");
      // And Enter still SENDS — the keydown intercept is gated on
      // `filtered.length > 0`, which the note does not touch.
      textarea.setSelectionRange(6, 6);
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "?zzzz");
    });

  it("a query matching more rows than the render cap trims the list and says so",
    async () => {
      seedLiveSession();
      // 150 matching entries against the 10-row render cap (`MAX_PICKER_ROWS`):
      // `fuzzyMatch` is a loose subsequence and the listing is the whole Space,
      // so an unbounded picker would re-render thousands of buttons per
      // keystroke — and 10 single-line rows is ≈308px, a normal dropdown.
      const entries = Array.from({ length: 150 }, (_, i) => `file${i}.ts`);
      vi.mocked(listSpaceFiles).mockResolvedValue({ entries, truncated: false });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "?file" } });
      const picker = await screen.findByTestId("mention-picker");
      await waitFor(() => expect(picker.querySelectorAll("button")).toHaveLength(10));
      const note = picker.querySelector(
        "[data-testid='mention-picker-note']",
      )!;
      expect(note.textContent).toContain("Too many matches");
      // The wrap stays consistent with the RENDERED list (the cap is applied
      // INSIDE `filtered`, so `activeIndex`, the `% filtered.length` wrap and
      // the DOM agree): ArrowUp from index 0 wraps to the LAST rendered row.
      textarea.setSelectionRange(5, 5);
      fireEvent.keyDown(textarea, { key: "ArrowUp" });
      const buttons = Array.from(picker.querySelectorAll("button"));
      expect(buttons[9]!.className).toContain("bg-surface-hover");
      expect(buttons[0]!.className).not.toContain("bg-surface-hover");
      // And the wrapped row is selectable (no crash on the capped tail).
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(
        (screen.getByRole("textbox") as HTMLTextAreaElement).value,
      ).toBe("file9.ts ");
    });

  it("a normal ? picker with matching rows and a whole listing renders NO note",
    async () => {
      seedLiveSession();
      // The `undefined` arm: nothing is capped and nothing is truncated, so
      // the picker must stay silent (a note here would be noise on the
      // everyday path).
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md", "docs/plan.md"],
        truncated: false,
      });
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "?READ" },
      });
      const picker = await screen.findByTestId("mention-picker");
      expect(picker.querySelectorAll("button")).toHaveLength(1);
      expect(
        screen.queryByTestId("mention-picker-note"),
      ).toBeNull();
    });

  it("a truncated listing with matching rows shows the note ALONGSIDE the rows, and the note is not a row",
    async () => {
      seedLiveSession();
      // The other half of the widened gate: the note is NOT only an
      // empty-picker explanation — it rides along whenever the listing is
      // capped, rows or not. Below the render cap (5 matches < 10), so the
      // wording is the walk-cap one.
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["a1.ts", "a2.ts", "a3.ts", "a4.ts", "a5.ts"],
        truncated: true,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      // ≥1 path char after the trigger (a bare `?` opens nothing, ADR 0033).
      fireEvent.change(textarea, { target: { value: "?a" } });
      const picker = await screen.findByTestId("mention-picker");
      const note = screen.getByTestId("mention-picker-note");
      // The walk-cap arm states the LIMITATION — no "keep typing" advice (a
      // truncated-away file cannot be reached by narrowing).
      expect(note.textContent).toBe(
        "File listing capped — some files may be missing",
      );
      expect(note.textContent).not.toContain("keep typing");
      // The rows are all still there (the note never replaces them) …
      const buttons = Array.from(picker.querySelectorAll("button"));
      expect(buttons).toHaveLength(5);
      // … and the note is outside the button set `activeIndex` / the
      // arrow-wrap / Enter operate over.
      expect(buttons).not.toContain(note);
      expect(note.tagName).not.toBe("BUTTON");
      expect(picker.contains(note)).toBe(true);
      // Arrow navigation still lands on a row, never on the note. The caret is
      // at the END of the token (the keydown re-evaluates it: a caret INSIDE
      // `?a` at index 1 leaves an empty remainder, which is not a `?` token).
      textarea.setSelectionRange(2, 2);
      fireEvent.keyDown(textarea, { key: "ArrowUp" });
      expect(
        Array.from(picker.querySelectorAll("button"))[4]!.className,
      ).toContain("bg-surface-hover");
    });

  it("a truncated listing AND more matches than the render cap: the too-many-matches wording wins",
    async () => {
      seedLiveSession();
      // Precedence pin: both conditions hold, and the actionable one (keep
      // typing) is what renders — the walk-cap note is shadowed.
      const entries = Array.from({ length: 150 }, (_, i) => `file${i}.ts`);
      vi.mocked(listSpaceFiles).mockResolvedValue({ entries, truncated: true });
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "?file" },
      });
      const picker = await screen.findByTestId("mention-picker");
      await waitFor(() =>
        expect(picker.querySelectorAll("button")).toHaveLength(10),
      );
      const note = screen.getByTestId("mention-picker-note");
      expect(note.textContent).toBe(
        "Too many matches — keep typing to narrow the list",
      );
      expect(note.textContent).not.toContain("capped");
    });

  it.each([
    { prefix: "$" as const, kind: "skills" as const },
    { prefix: "@" as const, kind: "agents" as const },
    { prefix: "#" as const, kind: "mcp" as const },
  ])(
    "the $prefix picker is UNCAPPED and note-free with more matches than the render cap ($kind catalog)",
    async ({ prefix, kind }) => {
      seedLiveSession();
      // The render cap belongs to `?` ALONE (its loose SUBSEQUENCE filter can
      // match nearly a whole 5,000-entry listing). The three Mentions keep the
      // shipped status quo: their filter is a name SUBSTRING, and ADR 0033
      // promises those pickers are unchanged — a silent 10-row cap there
      // would make row 11 unreachable with no explanation.
      // `…Once` (not `…Value`): `beforeEach` only `clearAll`s the mocks, so a
      // PERSISTENT override would leak this catalog into later tests.
      if (prefix === "#") seedMcpMentionsOn();
      const many = Array.from({ length: 150 }, (_, i) => `${kind}${i}`);
      if (kind === "skills") {
        vi.mocked(listSkills).mockResolvedValueOnce(
          many.map((name) => ({
            name,
            description: `d ${name}`,
            path: `/s/${name}/SKILL.md`,
            dir: `/s/${name}`,
            scope: "space",
            body: "B",
          })),
        );
      } else if (kind === "agents") {
        vi.mocked(listAgentDefinitionsForSpace).mockResolvedValueOnce(
          many.map((name) => ({
            name,
            description: `d ${name}`,
            model: null,
            scope: "user",
          })),
        );
      } else {
        vi.mocked(listMcpServersEffective).mockResolvedValueOnce(
          many.map((name) => ({ name, kind: "stdio", summary: `s ${name}` })),
        );
      }
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: prefix } });
      const picker = await screen.findByTestId("mention-picker");
      await waitFor(() =>
        // ALL 150 matches render — nothing is silently dropped.
        expect(picker.querySelectorAll("button")).toHaveLength(150),
      );
      expect(screen.queryByTestId("mention-picker-note")).toBeNull();
      // And the last row is reachable: the arrow-wrap counts the rendered
      // rows, so ArrowUp from index 0 lands on row 150 (index 149).
      textarea.setSelectionRange(1, 1);
      fireEvent.keyDown(textarea, { key: "ArrowUp" });
      const buttons = Array.from(picker.querySelectorAll("button"));
      expect(buttons[149]!.className).toContain("bg-surface-hover");
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect(
        (screen.getByRole("textbox") as HTMLTextAreaElement).value,
      ).toBe(`${prefix}${many[149]} `);
    },
  );

  it("while the file listing is in flight, ? opens NO picker and Enter SENDS",
    async () => {
      seedLiveSession();
      // A skill whose name matches the query SUBSTRING: while the `?` branch is
      // missing this renders a skills picker (and Enter INSERTS instead of
      // sending) — the degradation this test pins is "no rows at all".
      vi.mocked(listSkills).mockResolvedValueOnce([
        {
          name: "debug",
          description: "Debug a failure",
          path: "/s/debug/SKILL.md",
          dir: "/s/debug",
          scope: "space",
          body: "B",
        },
      ]);
      // A DEFERRED listing: the in-flight state ADR 0033 calls out (an empty
      // `filtered` means no picker and inert keyboard branches, so Enter sends
      // — the same degradation ADR 0032 relies on).
      let resolveFiles: (v: FileListDto) => void = () => {};
      vi.mocked(listSpaceFiles).mockImplementation(
        () => new Promise<FileListDto>((r) => (resolveFiles = r)),
      );
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      await waitFor(() => expect(vi.mocked(listSpaceFiles)).toHaveBeenCalled());
      await waitFor(() => expect(vi.mocked(listSkills)).toHaveBeenCalled());
      fireEvent.change(textarea, { target: { value: "hi ?de" } });
      expect(screen.queryByTestId("mention-picker")).toBeNull();
      textarea.setSelectionRange(7, 7);
      fireEvent.keyDown(textarea, { key: "Enter" });
      // Sent, and BYTE-IDENTICAL: no block appended, nothing stripped.
      expect(sendPrompt).toHaveBeenCalledWith("s1", "hi ?de");
      // The late listing lands with NO further keystroke (the `files` dep on
      // the `filtered` memo), and it must not resurrect a picker for a token
      // that no longer exists in the (now-empty) draft.
      await act(async () => {
        resolveFiles({ entries: ["README.md"], truncated: false });
        await Promise.resolve();
      });
      expect(screen.queryByTestId("mention-picker")).toBeNull();
      expect(screen.queryAllByText("README.md")).toEqual([]);
    });

  it("a Space switch serves no file rows until the new Space listing resolves",
    async () => {
      // Two live sessions in DIFFERENT Spaces (the catalog is keyed by the
      // Space path, so switching sessions is a catalog KEY change).
      useSessions.setState({
        activeSessionId: "s1",
        sessions: [
          { sessionId: "s1", cwd: "/home/u/proj", capabilities: {}, archived: false },
          { sessionId: "s2", cwd: "/home/u/other", capabilities: {}, archived: false },
        ],
        spaces: [
          { path: "/home/u/proj", createdAt: 1, lastOpenedAt: 1, trusted: false },
          { path: "/home/u/other", createdAt: 1, lastOpenedAt: 1, trusted: false },
        ],
        historySessions: [],
        messages: { s1: [], s2: [] },
        inTurn: {},
        stopReasons: {},
        closeReasons: {},
        configOptions: {},
      });
      let resolveOther: (v: FileListDto) => void = () => {};
      vi.mocked(listSpaceFiles).mockImplementation((p) =>
        p === "/home/u/proj"
          ? Promise.resolve({ entries: ["README.md"], truncated: false })
          : new Promise<FileListDto>((r) => (resolveOther = r)),
      );
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "?README" } });
      expect(await findFileRow("README.md")).toBeTruthy();
      // Switch the Space WHILE the picker is open: the old Space's rows must
      // NOT be offered against the new one (a stale row would insert a path the
      // new Space does not have).
      await act(async () => {
        useSessions.setState({ activeSessionId: "s2" });
      });
      await flush();
      expect(screen.queryAllByText("README.md")).toEqual([]);
      expect(screen.queryByTestId("mention-picker")).toBeNull();
      // The new listing resolves; the picker is still closed (the draft never
      // changed, so no `onChange` re-ran), and a fresh query offers ONLY the
      // new Space's file.
      await act(async () => {
        resolveOther({ entries: ["OTHER.md"], truncated: false });
        await Promise.resolve();
      });
      fireEvent.change(textarea, { target: { value: "?OTHER" } });
      expect(await findFileRow("OTHER.md")).toBeTruthy();
      expect(screen.queryAllByText("README.md")).toEqual([]);
    });

  it("a ? path in the draft is sent BYTE-IDENTICAL (never expanded, never stripped)",
    async () => {
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md"],
        truncated: false,
      });
      render(<ChatStream />);
      await waitForCatalog();
      // `README.md` IS in the listing — the send path must still leave the text
      // untouched (`expandMentions` never sees `?`; the trailing space also
      // closes the token, so Enter is not an insertion).
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "read ?README.md " },
      });
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
      expect(sendPrompt).toHaveBeenCalledWith("s1", "read ?README.md");
      const userMsg = useSessions
        .getState()
        .messages["s1"]?.find((m) => m.kind === "user");
      expect(userMsg?.kind === "user" ? userMsg.text : null).toBe(
        "read ?README.md",
      );
      // And no Mention chip was rendered for it (no block was appended).
      expect(screen.queryByText("named by the user")).toBeNull();
    });

  it("??, a?.b and a bare ? open NO picker and all three drafts SEND verbatim",
    async () => {
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md"],
        truncated: false,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      await waitForCatalog();
      const drafts = ["?? README.md", "a?.b", "?"];
      for (const [i, d] of drafts.entries()) {
        fireEvent.change(textarea, { target: { value: d } });
        await flush();
        // None is a `?` token (`??` and a bare `?` need ≥1 path char; `a?.b`
        // fails the `(^|\s)` boundary) — so no picker and Enter stays a SEND.
        expect(screen.queryByTestId("mention-picker")).toBeNull();
        textarea.setSelectionRange(d.length, d.length);
        fireEvent.keyDown(textarea, { key: "Enter" });
        await waitFor(() =>
          expect(sendPrompt).toHaveBeenNthCalledWith(i + 1, "s1", d),
        );
        // Unlock the composer for the next iteration (the mocked send resolves
        // in a microtask; `turnCompleted` clears `inTurn`).
        await waitFor(() =>
          expect(useSessions.getState().inTurn["s1"]).toBe(false),
        );
      }
    });

  it("the $ and @ pickers still list and insert their lowercase mention tokens",
    async () => {
      // The regression pin: `?` SHARES the picker but not the semantics — the
      // Mention prefixes keep the substring filter AND the lowercase insertion.
      seedLiveSession();
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: ["README.md", "debug-notes.md"],
        truncated: false,
      });
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      // `$` → the skills catalog only (the file `debug-notes.md` must NOT mix
      // into a `$` list, even though it matches the query as a substring).
      fireEvent.change(textarea, { target: { value: "$de" } });
      expect(await screen.findByText("debug")).toBeTruthy();
      expect(screen.queryByText("debug-notes.md")).toBeNull();
      textarea.setSelectionRange(3, 3);
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "$debug ",
      );
      // `@` → the agents catalog, and the inserted token is still LOWER-cased.
      fireEvent.change(textarea, { target: { value: "@" } });
      const scout = await screen.findByText("scout");
      expect(
        scout.closest("button")!.querySelector(".text-ui-xs")!.textContent,
      ).toBe("@");
      textarea.setSelectionRange(1, 1);
      fireEvent.keyDown(textarea, { key: "Enter" });
      expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe(
        "@scout ",
      );
    });

  // ---------------------------------------------------------------------
  // Picker GEOMETRY: the box clamps its own height and scrolls, and the
  // highlighted row is scrolled into view as the arrows move it. This is what
  // bounds the UNCAPPED Mention lists (`$`/`@`/`#` are deliberately not
  // row-capped — a sliced row is an unreachable row), so the promise "the
  // picker never dominates the window" is geometry, not a row count.
  //
  // What jsdom CAN and cannot check: it has NO layout engine, so row heights
  // and scroll offsets are unmeasurable here (hence the clamp is a deliberate
  // ESTIMATE in the component, and these tests assert the CLASSES and the
  // SCROLL CALL, never a pixel height).
  // ---------------------------------------------------------------------

  it("the picker clamps its height to the UI font size and scrolls instead of filling the window",
    async () => {
      seedLiveSession();
      // Enough rows that the list is taller than the clamp: `?` is capped at
      // 10 rows, and the clamp is sized to ≈10 rows, so 10 rows plus the note
      // line is already more than the box can show.
      vi.mocked(listSpaceFiles).mockResolvedValue({
        entries: Array.from({ length: 10 }, (_, i) => `file${i}.ts`),
        truncated: true,
      });
      render(<ChatStream />);
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "?file" },
      });
      const picker = await screen.findByTestId("mention-picker");
      await waitFor(() =>
        expect(picker.querySelectorAll("button")).toHaveLength(10),
      );
      // It must SCROLL: without `overflow-y-auto` a `max-height` just clips.
      expect(picker.classList.contains("overflow-y-auto")).toBe(true);
      // And the clamp must track the UI font size, which is a USER setting
      // applied as an inline px value on `<html>` (`src/lib/settings.ts`) — a
      // fixed `rem` clamp like `max-h-64` would show FEWER rows for a user who
      // raised the font size, i.e. it punishes exactly the user who needs the
      // room. So the class is a `calc()` over `var(--ui-font-size)`.
      const clamp = Array.from(picker.classList).find((c) =>
        c.startsWith("max-h-["),
      );
      expect(clamp).toBeDefined();
      expect(clamp).toContain("var(--ui-font-size)");
    });

  it("arrow-keying the picker scrolls the highlighted row into view",
    async () => {
      seedLiveSession();
      // The UNCAPPED side: the `$` catalog is not row-limited, so this is the
      // list the clamp actually protects. 40 rows is far more than the box can
      // show, and every one of them stays keyboard-reachable.
      const many = Array.from({ length: 40 }, (_, i) => `skill${i}`);
      vi.mocked(listSkills).mockResolvedValueOnce(
        many.map((name) => ({
          name,
          description: `d ${name}`,
          path: `/s/${name}/SKILL.md`,
          dir: `/s/${name}`,
          scope: "space",
          body: "B",
        })),
      );
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "$" } });
      const picker = await screen.findByTestId("mention-picker");
      await waitFor(() =>
        expect(picker.querySelectorAll("button")).toHaveLength(40),
      );
      // Which ROW was scrolled is the whole question, and the tool for it is
      // the file's OWN `beforeAll` stub (`HTMLElement.prototype.scrollIntoView =
      // vi.fn()`, there for Radix) read through `mock.contexts` — the RECEIVER
      // of each call. Two alternatives were tried and are wrong here: a bare
      // "was `scrollIntoView` called" assertion could be satisfied by an
      // unrelated widget, and a per-row `vi.spyOn` cannot answer WHICH row —
      // `scrollIntoView` is INHERITED, so 40 spies chain through the same stub
      // and each records the one call.
      const scrollStub = HTMLElement.prototype.scrollIntoView as Mock;
      /** Every element ever asked to scroll, in call order. */
      const scrolled = (): Element[] =>
        scrollStub.mock.contexts as unknown as Element[];
      const buttons = Array.from(
        picker.querySelectorAll("button"),
      ) as HTMLButtonElement[];
      // Settle the in-flight catalogs BEFORE measuring, and wait until the log
      // goes quiet. The agents / MCP / file fetches land independently of the
      // skills one, and every resolution re-derives `filtered` — which
      // legitimately re-runs the effect for whatever row is active AT THAT
      // MOMENT (still row 0, before any arrow key). That is noise for a test
      // about what an ARROW KEY does, so the window starts after the quiet
      // point; there is then no `await` between a keypress and its assertion,
      // so nothing can interleave.
      let quietAt = -1;
      while (quietAt !== scrolled().length) {
        quietAt = scrolled().length;
        await act(async () => {
          await new Promise((resolve) => setTimeout(resolve, 0));
        });
      }
      let mark = scrolled().length;

      textarea.setSelectionRange(1, 1);
      fireEvent.keyDown(textarea, { key: "ArrowDown" });

      // The row that is NOW highlighted (index 0 → 1) is the one scrolled into
      // view — focus-follows, so a keyboard user arrowing down a long list
      // never loses the selection off the top or bottom of the box. ONE scroll,
      // on the moved-to row, with `block: "nearest"` (what keeps an
      // already-visible row from jumping).
      const down = scrolled().slice(mark);
      expect(down).toEqual([buttons[1]]);
      expect(scrollStub.mock.calls[mark]![0]).toEqual({ block: "nearest" });
      expect(buttons[1]!.className).toContain("bg-surface-hover");

      // The row-above case, which is the one a ref-on-the-active-row can get
      // WRONG: ArrowUp moves the highlight BACK to a row that was mounted the
      // whole time, so a stale ref would scroll nothing (or the wrong row).
      mark = scrolled().length;
      fireEvent.keyDown(textarea, { key: "ArrowUp" });
      expect(scrolled().slice(mark)).toEqual([buttons[0]]);
      expect(scrollStub.mock.calls[mark]![0]).toEqual({ block: "nearest" });

      // And the WRAP: ArrowUp from index 0 lands on the LAST row — the one
      // furthest outside the visible box, so this is the scroll that matters
      // most.
      mark = scrolled().length;
      fireEvent.keyDown(textarea, { key: "ArrowUp" });
      expect(scrolled().slice(mark)).toEqual([buttons[39]]);
      expect(buttons[39]!.className).toContain("bg-surface-hover");
      // Still every row rendered: the clamp is GEOMETRY, never a slice of
      // `filtered` — nothing here may make a row unreachable.
      expect(picker.querySelectorAll("button")).toHaveLength(40);
    });

  it("typing narrows the list under an UNCHANGED index and the newly-active row is the one scrolled",
    async () => {
      // The pin for `filtered` being in the scroll effect's dep array (see
      // `ComposerMentions`' DEPS note). `filtered` is NOT there for the arrow
      // keys — the test above covers those — it is there for TYPING: `onChange`
      // resets `picker.index` to 0, so a keystroke that narrows the list leaves
      // `activeIndex` the SAME NUMBER (0) while `filtered[0]` becomes a
      // DIFFERENT row. Deps `[activeIndex]` alone therefore never re-run, and
      // the row the user is now on is never made visible — a silent
      // keyboard/a11y failure with no other test to catch it (mutation-verified:
      // dropping `filtered` from the array keeps every other test in this file
      // green).
      seedLiveSession();
      vi.mocked(listSkills).mockResolvedValueOnce([
        {
          name: "ab-alpha",
          description: "d a",
          path: "/s/ab-alpha/SKILL.md",
          dir: "/s/ab-alpha",
          scope: "space",
          body: "B",
        },
        {
          name: "ab-beta",
          description: "d b",
          path: "/s/ab-beta/SKILL.md",
          dir: "/s/ab-beta",
          scope: "space",
          body: "B",
        },
        {
          name: "ac-gamma",
          description: "d g",
          path: "/s/ac-gamma/SKILL.md",
          dir: "/s/ac-gamma",
          scope: "space",
          body: "B",
        },
      ]);
      render(<ChatStream />);
      const textarea = screen.getByRole("textbox") as HTMLTextAreaElement;
      fireEvent.change(textarea, { target: { value: "$ab" } });
      const picker = await screen.findByTestId("mention-picker");
      await waitFor(() =>
        expect(picker.querySelectorAll("button")).toHaveLength(2),
      );
      // The same receiver-reading technique as the arrow test above: the file's
      // own `beforeAll` stub, read through `mock.contexts`, because
      // `scrollIntoView` is INHERITED and a per-row `vi.spyOn` records the same
      // single call for every row.
      const scrollStub = HTMLElement.prototype.scrollIntoView as Mock;
      const scrolled = (): Element[] =>
        scrollStub.mock.contexts as unknown as Element[];
      // Quiet point: the agents / MCP / file catalogs resolve independently of
      // skills and each resolution re-runs the effect for the row active AT
      // THAT MOMENT. Wait the log stops growing, then leave no `await` between
      // the keystroke and the assertion.
      let quietAt = -1;
      while (quietAt !== scrolled().length) {
        quietAt = scrolled().length;
        await act(async () => {
          await new Promise((resolve) => setTimeout(resolve, 0));
        });
      }
      const before = Array.from(
        picker.querySelectorAll("button"),
      ) as HTMLButtonElement[];
      // Precondition: index 0 is active and it is `ab-alpha`.
      expect(before[0]!.className).toContain("bg-surface-hover");
      const mark = scrolled().length;

      // The keystroke: one more character. `filtered` narrows to ONE row
      // (`ab-beta` — `ab-alpha` no longer matches) while `activeIndex` stays 0,
      // so the highlighted row is now a row that was NOT highlighted before.
      textarea.setSelectionRange(8, 8);
      fireEvent.change(textarea, { target: { value: "$ab-beta" } });

      const after = Array.from(
        picker.querySelectorAll("button"),
      ) as HTMLButtonElement[];
      expect(after).toHaveLength(1);
      expect(after[0]!.textContent).toContain("ab-beta");
      // `activeIndex` is UNCHANGED (still the first row), so this assertion
      // holds for the buggy and correct code alike — it is here to pin the
      // precondition, not to do the detecting.
      expect(after[0]!.className).toContain("bg-surface-hover");
      // THE pin: exactly one new scroll, and its receiver is the row that is
      // active NOW (`ab-beta`), not the row that was active before the
      // keystroke (`ab-alpha`, now gone from the list). Without `filtered` in
      // the deps this slice is EMPTY — the effect never re-runs — so the test
      // is red, not vacuously green.
      expect(scrolled().slice(mark)).toEqual([after[0]]);
      expect(scrollStub.mock.calls[mark]![0]).toEqual({ block: "nearest" });
      expect(scrolled().slice(mark)).not.toContain(before[0]!);
    });
});
