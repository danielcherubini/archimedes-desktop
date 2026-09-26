import { render, screen } from "@testing-library/react";
import { fireEvent } from "@testing-library/react";
import { describe, it, expect } from "vitest";
import type { ToolCallMessage } from "../lib/toolGroups";
import ChangesGroupCard from "./ChangesGroupCard";

// Fixture helper (same shape as `toolGroups.test.ts`, but the NARROWED
// `ToolCallMessage` type — the component's prop is `ToolCallMessage[]`,
// not the `Message` union): the `tool-call` variant needs `id`, `title`,
// `status`, `at`.
const tool = (
  id: string,
  title: string,
  at: number,
  status: "pending" | "completed" | "failed" = "completed",
  rawInput?: unknown,
): ToolCallMessage => ({ kind: "tool-call", id, title, status, at, rawInput });

describe("ChangesGroupCard", () => {
  it("renders the Changes header with the file count", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "completed", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    expect(screen.getByText("Changes")).toBeTruthy();
    expect(screen.getByText("2 files")).toBeTruthy();
    // Both basenames are present in the DOM — in the header chip list AND
    // the (now auto-open) member cards — so `getAllByText`, never
    // `getByText`. (In jsdom all measurements are 0, so the header chips
    // carry the `invisible` class — do NOT assert visibility, assert
    // presence only; hidden chips stay MOUNTED.)
    expect(screen.getAllByText("c.ts").length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByText("d.ts").length).toBeGreaterThanOrEqual(1);
  });

  it("renders total edit stats", () => {
    const { container } = render(
      <ChangesGroupCard
        messages={[
          tool("t1", "edit", 1, "completed", {
            path: "/a/b/c.ts",
            edits: [{ oldText: "a", newText: "x\ny\nz" }], // +3 -1
          }),
          tool("t2", "edit", 2, "completed", {
            path: "/a/b/d.ts",
            edits: [{ oldText: "b", newText: "p\nq" }], // +2 -1
          }),
        ]}
      />,
    );
    // `+5` / `-2` are split across nested elements (`DiffCount` renders `+`
    // and the number separately) — assert via `textContent`, NOT `getByText`.
    expect(container.textContent).toContain("+5");
    expect(container.textContent).toContain("-2");
  });

  it("collapses chips to +N when they do not fit", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "completed", { path: "/a/b/d.ts" }),
          tool("t3", "write", 3, "completed", { path: "/a/b/e.ts" }),
        ]}
      />,
    );
    // jsdom measures zero widths → `visibleCount` 0 → the overflow label is
    // present. The visible `+{hiddenCount}` span AND the invisible measuring
    // marker both read `+3` — hence `getAllByText`, never `getByText`.
    const matches = screen.getAllByText("+3");
    expect(matches.length).toBeGreaterThanOrEqual(1);
    // The chip basenames are present in the DOM but HIDDEN (the wrapper span
    // is `aria-hidden` + `invisible`) — assert the hidden state, never
    // absence. The FIRST match is the header chip list's entry (the member
    // cards' chips render later in the DOM).
    expect(
      screen.getAllByText("c.ts")[0].closest("[aria-hidden]")?.getAttribute("aria-hidden"),
    ).toBe("true");
  });

  it("collapses chips to +N when total stats are present", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "edit", 1, "completed", {
            path: "/a/b/c.ts",
            edits: [{ oldText: "a", newText: "x" }],
          }),
          tool("t2", "edit", 2, "completed", {
            path: "/a/b/d.ts",
            edits: [{ oldText: "b", newText: "y" }],
          }),
          tool("t3", "edit", 3, "completed", {
            path: "/a/b/e.ts",
            edits: [{ oldText: "c", newText: "z" }],
          }),
        ]}
      />,
    );
    // The trailing content (stats + chevron) is measured via `trailingRef`
    // — jsdom measures 0 for everything → `available` 0 → `visibleCount` 0
    // → the overflow label is present (same pattern as the chips-collapse
    // test above; assert via `getAllByText`).
    const matches = screen.getAllByText("+3");
    expect(matches.length).toBeGreaterThanOrEqual(1);
  });

  it("shows a failure cue in the header when a member failed", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "failed", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    expect(screen.getByLabelText("Some changes failed")).toBeTruthy();
  });

  it("does not show the failure cue when no member failed", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "completed", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    expect(screen.queryByLabelText("Some changes failed")).toBeNull();
  });

  it("shows the latest member's chip while any member is pending", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "pending", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "pending", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    // While pending, the full chip list is NOT rendered — the latest
    // member's chip replaces it — so the pending member's basename appears
    // exactly ONCE (all members are pending, so the group starts CLOSED and
    // no member card double-renders the chip either).
    expect(screen.getAllByText("d.ts").length).toBe(1);
    expect(screen.queryByText("c.ts")).toBeNull();
  });

  it("renders the member cards indented when expanded", () => {
    const { container } = render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "pending", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "pending", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    // All pending → the group starts CLOSED — the member cards (the
    // indented container) are not rendered. (The header shows the latest
    // pending member's live chip — that is the header, not the member
    // cards.)
    expect(container.querySelector(".border-l")).toBeNull();
    // Unique before expansion — the member cards' buttons render only when
    // open.
    fireEvent.click(screen.getByRole("button"));
    // The member cards' verbs render.
    expect(screen.getAllByText("Writing").length).toBe(2);
    // The indented container exists.
    expect(container.querySelector(".border-l")).not.toBeNull();
  });

  it("starts open when it forms with an already-finished member", () => {
    const { container } = render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "completed", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    // All members finished on first render → the group starts OPEN — the
    // member cards are visible without clicking.
    expect(screen.getAllByText("Wrote").length).toBe(2);
    expect(container.querySelector(".border-l")).not.toBeNull();
  });

  it("keeps a manual close when the group already started open", () => {
    const input = {
      t1: { path: "/a/b/c.ts" },
      t2: { path: "/a/b/d.ts" },
    };
    const { container, rerender } = render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", input.t1),
          tool("t2", "write", 2, "pending", input.t2),
        ]}
      />,
    );
    // One member already finished → the group starts OPEN — the member
    // cards are visible without clicking.
    expect(container.querySelector(".border-l")).not.toBeNull();
    // The user closes it.
    fireEvent.click(screen.getByRole("button", { name: /Changes/ }));
    expect(container.querySelector(".border-l")).toBeNull();
    // The pending member finishes (`anyPending` true → false edge) — a
    // group that started open has already "auto-opened", so the edge
    // effect must NOT re-open it: the manual close sticks.
    rerender(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", input.t1),
          tool("t2", "write", 2, "completed", input.t2),
        ]}
      />,
    );
    expect(container.querySelector(".border-l")).toBeNull();
  });

  it("auto-opens once when the last pending member finishes", () => {
    const input = {
      t1: { path: "/a/b/c.ts" },
      t2: { path: "/a/b/d.ts" },
    };
    const { container, rerender } = render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "pending", input.t1),
          tool("t2", "write", 2, "pending", input.t2),
        ]}
      />,
    );
    // All pending → starts CLOSED.
    expect(screen.queryByText("Writing")).toBeNull();
    expect(container.querySelector(".border-l")).toBeNull();
    // The FIRST member finishes (others still pending) — `anyPending` is
    // still true → still closed.
    rerender(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", input.t1),
          tool("t2", "write", 2, "pending", input.t2),
        ]}
      />,
    );
    expect(container.querySelector(".border-l")).toBeNull();
    // The LAST member finishes (`anyPending` → false) → opens once.
    rerender(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", input.t1),
          tool("t2", "write", 2, "completed", input.t2),
        ]}
      />,
    );
    expect(container.querySelector(".border-l")).not.toBeNull();
    expect(screen.getAllByText("Wrote").length).toBe(2);
    // The user closes it → it never re-opens (one-shot).
    fireEvent.click(screen.getByRole("button", { name: /Changes/ }));
    rerender(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", input.t1),
          tool("t2", "write", 2, "completed", input.t2),
        ]}
      />,
    );
    expect(container.querySelector(".border-l")).toBeNull();
  });
});
