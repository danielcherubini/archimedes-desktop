import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { createElement, type RefObject } from "react";
import MessageList from "./MessageList";
import type { RenderUnit } from "../../lib/toolGroups";
import type { Message } from "../../store/sessions";

/**
 * The transcript's VERTICAL RHYTHM (ZCode's model: rows carry no padding and
 * the container owns the spacing — `gap-4` between work items, `gap-5` where
 * prose meets the work block, never per-row `py-*`).
 *
 * The ask: tool calls need more air around them than `Thought`/prose do —
 * prose and thinking stay as tight as they are, and a tool row gets a hidden
 * (margin, not border/band) breathing gap on each side it touches. So the gap
 * is a function of the PAIR: `base + breathing` whenever either neighbour is
 * a tool row, `base` otherwise.
 */

const text = (id: string): Message => ({
  kind: "agent-text",
  messageId: id,
  text: "hello",
  at: 1,
});
const thought = (id: string): Message => ({
  kind: "agent-thought",
  messageId: id,
  text: "thinking",
  at: 1,
});
const tool = (id: string, title = "bash"): Message => ({
  kind: "tool-call",
  id,
  title,
  status: "completed",
  at: 1,
});

const single = (message: Message): RenderUnit => ({ kind: "single", message });

function rows(units: RenderUnit[]): HTMLElement[] {
  const { container } = render(
    createElement(MessageList, {
      scrollRef: { current: null } as RefObject<HTMLDivElement | null>,
      units,
      messageCount: units.length,
      activeSessionId: "s1",
      inTurn: false,
      askRequests: [],
      prompts: [],
      stackedAskRequests: [],
      hasPendingRequest: false,
      workingOrInTurn: false,
      agentState: "idle",
      turnDiffs: [],
      stopReason: undefined,
    }),
  );
  const scroll = container.querySelector('[data-testid="transcript-scroll"]');
  if (!scroll) throw new Error("no transcript scroll container");
  return [...scroll.children] as HTMLElement[];
}

const extra = (el: HTMLElement) =>
  el.className.split(/\s+/).filter((c) => /^(mt|my)-/.test(c));

describe("the transcript's vertical rhythm", () => {
  it("the container owns the base gap (a true `gap`, not `space-y` on a block box)", () => {
    const scroll = render(
      createElement(MessageList, {
        scrollRef: { current: null } as RefObject<HTMLDivElement | null>,
        units: [single(text("a"))],
        messageCount: 1,
        activeSessionId: "s1",
        inTurn: false,
        askRequests: [],
        prompts: [],
        stackedAskRequests: [],
        hasPendingRequest: false,
        workingOrInTurn: false,
        agentState: "idle",
        turnDiffs: [],
        stopReason: undefined,
      }),
    ).container.querySelector('[data-testid="transcript-scroll"]') as HTMLElement;
    const tokens = scroll.className.split(/\s+/);
    expect(tokens).toContain("gap-3");
    expect(tokens).not.toContain("space-y-3");
  });

  it("prose → tool: the tool row gets the breathing gap", () => {
    const [prose, toolRow] = rows([single(text("a")), single(tool("t1"))]);
    expect(extra(prose!)).toEqual([]);
    expect(extra(toolRow!)).toEqual(["mt-4"]);
  });

  it("tool → prose: the breathing gap is symmetric (it is the PAIR that is airy, not the tool row)", () => {
    const [, prose] = rows([single(tool("t1")), single(text("a"))]);
    expect(extra(prose!)).toEqual(["mt-4"]);
  });

  it("thinking → chat keeps the tight base gap (unchanged)", () => {
    const [thoughtRow, prose] = rows([
      single(thought("th")),
      single(text("a")),
    ]);
    expect(extra(thoughtRow!)).toEqual([]);
    expect(extra(prose!)).toEqual([]);
  });

  it("chat → thinking also stays tight", () => {
    const [, thoughtRow] = rows([single(text("a")), single(thought("th"))]);
    expect(extra(thoughtRow!)).toEqual([]);
  });

  it("tool → tool keeps ONE breathing gap (a run of tool calls is a steady 28px, not alternating)", () => {
    const [, second] = rows([single(tool("t1")), single(tool("t2"))]);
    expect(extra(second!)).toEqual(["mt-4"]);
  });

  it("a folded `Changes` group counts as a tool row", () => {
    const group: RenderUnit = {
      kind: "changes-group",
      messages: [tool("t1", "edit"), tool("t2", "write")] as never,
    };
    const [, prose] = rows([group, single(text("a"))]);
    expect(extra(prose!)).toEqual(["mt-4"]);
  });

  it("the FIRST row gets no breathing gap (the container's own padding is its air)", () => {
    const [first] = rows([single(tool("t1")), single(text("a"))]);
    expect(extra(first!)).toEqual([]);
  });

  it("every row is wrapped in a flex column so the row keeps its exact height (a block wrapper re-adds the line box's descender space)", () => {
    const [row] = rows([single(thought("th"))]);
    expect(row!.className.split(/\s+/)).toEqual(
      expect.arrayContaining(["flex", "flex-col"]),
    );
  });
});
