import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { respondBridgeRequest } from "../lib/tauri";
import SubagentDetailHost from "./SubagentDetailHost";
import { useSubagents } from "../store/subagents";
import { useSubagentSelection } from "../store/subagentSelection";
import { useBridge } from "../store/bridge";
import { usePermissions } from "../store/permissions";
import { useSessions } from "../store/sessions";

// Mock the Tauri IPC layer; everything else (stores) is the real code.
vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    respondBridgeRequest: vi.fn().mockResolvedValue(undefined),
    respondPermission: vi.fn().mockResolvedValue(undefined),
  };
});

const entry = {
  sessionId: "sub1",
  parentSessionId: "main1",
  agentName: "reviewer",
  task: "review the diff",
  status: "running" as const,
};

const otherEntry = {
  sessionId: "sub2",
  parentSessionId: "main1",
  agentName: "explorer",
  task: "explore the codebase",
  status: "running" as const,
};

/**
 * The hidden host's visibility: hidden via the wrapper's `hidden`
 * ATTRIBUTE (the content stays MOUNTED — see the `SubagentDetailHost`
 * doc). The modal's `hidden` CLASS does not match `[hidden]`, so this
 * distinguishes the two: a match means the content is in the hidden
 * host, not the modal.
 */
function isHidden(el: Element): boolean {
  return el.closest("[hidden]") !== null;
}

/** Seed the selection and flush the re-render. */
function select(id: string | null): void {
  act(() => {
    useSubagentSelection.getState().select(id);
  });
}

beforeEach(() => {
  useSubagentSelection.setState({ selectedSessionId: null });
  for (const id of Object.keys(useSubagents.getState().entries)) {
    useSubagents.getState().dismiss(id);
  }
  useBridge.getState().dismissSession("sub1");
  useBridge.getState().dismissSession("sub2");
  useBridge.getState().dismissSession("main1");
  usePermissions.getState().dismissSessionPrompts("sub1");
  usePermissions.getState().dismissSessionPrompts("sub2");
  usePermissions.getState().dismissSessionPrompts("main1");
  useSessions.setState({ messages: {} });
});

describe("SubagentDetailHost (the dedicated transcript modal + the always-mounted host)", () => {
  it("renders the modal HIDDEN (no header) when selectedSessionId is null", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentDetailHost />);
    // The sheet is ALWAYS mounted...
    const dialog = screen.getByRole("dialog", { name: /transcript/ });
    expect(dialog.className).toContain("hidden");
    // ...and the inner content (the header) is NOT mounted.
    expect(
      screen.queryByRole("button", { name: "Close subagent transcript" }),
    ).toBeNull();
  });

  it("OPENS the modal (the header renders) when select(sessionId) is called for an existing entry", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentDetailHost />);
    select("sub1");
    const dialog = screen.getByRole("dialog", { name: /transcript/ });
    expect(dialog.className).not.toContain("hidden");
    // The header: the status icon + the agent name + the status chip.
    expect(screen.getByText("reviewer")).toBeTruthy();
    expect(screen.getByText("running")).toBeTruthy();
  });

  it("shows the selected subagent's transcript in the modal (rendered exactly once)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentDetailHost />);
    select("sub1");
    const dialog = screen.getByRole("dialog", { name: /transcript/ });
    const msg = screen.getByText("hello from the subagent");
    // The transcript is inside the modal...
    expect(dialog.contains(msg)).toBe(true);
    // ...rendered EXACTLY ONCE (the hidden host skips the selected entry
    // — the "every subagent's transcript is rendered exactly once"
    // invariant).
    expect(screen.getAllByText("hello from the subagent")).toHaveLength(1);
  });

  it("CLOSES on the X button (selectedSessionId → null)", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentDetailHost />);
    select("sub1");
    fireEvent.click(
      screen.getByRole("button", { name: "Close subagent transcript" }),
    );
    expect(
      screen.queryByRole("button", { name: "Close subagent transcript" }),
    ).toBeNull();
    expect(useSubagentSelection.getState().selectedSessionId).toBeNull();
  });

  it("CLOSES on Escape (a React onKeyDown on the SHEET — fired on the sheet, NOT window)", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentDetailHost />);
    select("sub1");
    // The implementation is a React `onKeyDown` on the sheet, so the
    // event must be fired ON THE SHEET (an element inside the modal —
    // a window-targeted keydown would never reach a React handler and
    // would contradict the known-limitation text: Esc only closes the
    // modal when focus is INSIDE the sheet).
    fireEvent.keyDown(screen.getByRole("dialog", { name: /transcript/ }), {
      key: "Escape",
    });
    expect(
      screen.queryByRole("button", { name: "Close subagent transcript" }),
    ).toBeNull();
    expect(useSubagentSelection.getState().selectedSessionId).toBeNull();
  });

  it("a sheet-scoped Escape does NOT reach the window (ChatStream's turn-cancel listener does not fire)", () => {
    useSubagents.getState().addSession(entry);
    render(<SubagentDetailHost />);
    select("sub1");
    // `ChatStream`'s Esc handler is a WINDOW-level `keydown` listener
    // (active while `inTurn`) that cancels the turn. The sheet's
    // `onKeyDown` `stopPropagation()`s the NATIVE event (React forwards
    // it to `nativeEvent.stopPropagation()`) BEFORE it reaches
    // `window` — the spy models that window listener.
    const windowEsc = vi.fn();
    window.addEventListener("keydown", windowEsc);
    fireEvent.keyDown(screen.getByRole("dialog", { name: /transcript/ }), {
      key: "Escape",
    });
    window.removeEventListener("keydown", windowEsc);
    // The modal closed...
    expect(useSubagentSelection.getState().selectedSessionId).toBeNull();
    // ...but the native event was stopped AT THE SHEET (it never
    // reached `window` — the turn is NOT cancelled).
    expect(windowEsc).not.toHaveBeenCalled();
  });

  it("closing does NOT dismiss the entry (it stays in useSubagents; the transcript moves to the hidden host in the same render)", () => {
    useSubagents.getState().addSession(entry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    render(<SubagentDetailHost />);
    select("sub1");
    // Open: the transcript is in the modal (visible).
    expect(isHidden(screen.getByText("hello from the subagent"))).toBe(false);
    fireEvent.click(
      screen.getByRole("button", { name: "Close subagent transcript" }),
    );
    // The entry is NOT dismissed (closing is `select(null)` only — it
    // never calls `dismiss`)...
    expect(useSubagents.getState().entries["sub1"]).toBeDefined();
    // ...and the transcript stays MOUNTED (it moved from the modal to
    // the hidden host in the SAME render — the prompt cards never
    // unmount, so they never hang until the bridge timeout).
    expect(isHidden(screen.getByText("hello from the subagent"))).toBe(true);
  });

  it("renders a HIDDEN SubagentTranscript for every NON-selected entry (the always-mounted host)", () => {
    useSubagents.getState().addSession(entry);
    useSubagents.getState().addSession(otherEntry);
    useSessions.getState().applySessionUpdate("sub1", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the subagent" },
    });
    useSessions.getState().applySessionUpdate("sub2", {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello from the explorer" },
    });
    render(<SubagentDetailHost />);
    select("sub1");
    // sub1 (selected): in the modal (visible)...
    expect(isHidden(screen.getByText("hello from the subagent"))).toBe(false);
    // ...sub2 (NOT selected): in the hidden host (in the DOM, hidden
    // via the `hidden` ATTRIBUTE = `display: none` — the component stays
    // MOUNTED, so the prompt cards are always rendered).
    expect(screen.getByText("hello from the explorer")).toBeTruthy();
    expect(isHidden(screen.getByText("hello from the explorer"))).toBe(true);
  });

  it("an Escape consumed by a nested AskQuestionCard does NOT close the modal (the card's stopPropagation stops it before the sheet)", async () => {
    useSubagents.getState().addSession(entry);
    // The subagent's OWN `ask` (keyed by the subagent's session id) —
    // the card renders inside the modal's transcript.
    useBridge.getState().addRequest("sub1", {
      requestId: "r1",
      method: "ask",
      source: "main",
      params: {
        questions: [
          {
            id: "q1",
            question: "Which color?",
            options: [{ label: "Red" }, { label: "Blue" }],
          },
        ],
      },
    });
    render(<SubagentDetailHost />);
    select("sub1");
    // The card's ROOT (the only `tabindex="-1"` element in the card)
    // consumes the Escape: its `stopPropagation()` stops the event
    // BEFORE it reaches the sheet's (parent) React handler.
    const card = screen.getByText("Which color?").closest("[tabindex='-1']")!;
    fireEvent.keyDown(card, { key: "Escape" });
    // The modal header is STILL present (the modal did NOT close)...
    expect(
      screen.getByRole("button", { name: "Close subagent transcript" }),
    ).toBeTruthy();
    expect(useSubagentSelection.getState().selectedSessionId).toBe("sub1");
    // ...and the CARD's own Esc-dismiss fired (the request was
    // responded to as cancelled — the event was consumed by the card,
    // not swallowed on the way).
    await act(async () => {
      await Promise.resolve();
    });
    expect(vi.mocked(respondBridgeRequest)).toHaveBeenCalled();
  });
});
