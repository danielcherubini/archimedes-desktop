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
    // Both basenames are present in the DOM. (In jsdom all measurements are
    // 0, so the chips carry the `invisible` class — do NOT assert
    // visibility, assert presence only; hidden chips stay MOUNTED.)
    expect(screen.getByText("c.ts")).toBeTruthy();
    expect(screen.getByText("d.ts")).toBeTruthy();
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
    // absence.
    expect(
      screen.getByText("c.ts").closest("[aria-hidden]")?.getAttribute("aria-hidden"),
    ).toBe("true");
  });

  it("shows the latest member's chip while any member is pending", () => {
    render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "pending", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    // While pending, the full chip list is NOT rendered — the latest
    // member's chip replaces it — so the pending member's basename appears
    // exactly ONCE (this also proves the list is not double-rendering it).
    expect(screen.getAllByText("d.ts").length).toBe(1);
  });

  it("renders the member cards indented when expanded", () => {
    const { container } = render(
      <ChangesGroupCard
        messages={[
          tool("t1", "write", 1, "completed", { path: "/a/b/c.ts" }),
          tool("t2", "write", 2, "completed", { path: "/a/b/d.ts" }),
        ]}
      />,
    );
    // Unique before expansion — the member cards' buttons render only when
    // open.
    fireEvent.click(screen.getByRole("button"));
    // The member cards' verbs render.
    expect(screen.getAllByText("Wrote").length).toBe(2);
    // The indented container exists.
    expect(container.querySelector(".border-l")).not.toBeNull();
  });
});
