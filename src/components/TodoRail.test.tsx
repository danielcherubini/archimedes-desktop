import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import TodoRail from "./TodoRail";
import type { TodoItem } from "../store/interactive";

const items = (...statuses: TodoItem["status"][]): TodoItem[] =>
  statuses.map((status) => ({ content: `t-${status}`, status }));

/**
 * The rail is a READOUT, not a control: everything asserted here is either
 * something only this file can catch (the three states collapsing into one
 * glyph, a scroll container appearing in a 40px sliver) or the geometry that
 * keeps the strip clear of the pane's own toggle.
 */
describe("TodoRail (the todo board as ticks and dots, for the collapsed rail)", () => {
  it("renders one mark per todo, in order, and NO text at all", () => {
    render(<TodoRail items={items("completed", "in_progress", "pending")} />);
    const rail = screen.getByTestId("todo-rail");
    expect(screen.getAllByTestId("todo-mark")).toHaveLength(3);
    // The whole point of the rail: the SHAPE of the board in a 40px sliver
    // that cannot hold words. A label sneaking in here would render as the
    // one-character-per-line column this replaced.
    expect(rail.textContent).toBe("");
  });

  it("draws the three states apart — tick / filled dot / hollow dot", () => {
    render(<TodoRail items={items("completed", "in_progress", "pending")} />);
    const [done, active, todo] = screen.getAllByTestId("todo-mark");
    // Completed: the success ring, and the check while a mark is big enough.
    expect(done!.className).toContain("border-success");
    expect(done!.querySelector("svg")).toBeTruthy();
    // In progress: the warning FILL (the rail's one hot dot — the board's `◉`).
    expect(active!.className).toContain("bg-warning");
    expect(active!.className).not.toContain("border-success");
    // Pending: the hollow dot.
    expect(todo!.className).toContain("border-foreground-subtlest");
    expect(todo!.className).not.toMatch(/\bbg-/);
  });

  it("tells the states apart by SHAPE as well as colour (the dots are read at a glance, and colour-blind at a glance)", () => {
    render(<TodoRail items={items("completed", "in_progress", "pending")} />);
    const shapes = screen
      .getAllByTestId("todo-mark")
      .map((m) => (m.querySelector("svg") ? "check" : m.className.includes("bg-") ? "filled" : "hollow"));
    expect(shapes).toEqual(["check", "filled", "hollow"]);
  });

  it("distributes its rows over the height it is given, and CLIPS instead of scrolling", () => {
    const { container } = render(<TodoRail items={items("pending", "pending", "pending")} />);
    const rail = screen.getByTestId("todo-rail");
    // Never a scroll container: a 40px sliver with a scrollbar is the exact
    // artefact this feature replaces. `overflow-hidden` and NO scroll utility.
    expect(rail.className).toContain("overflow-hidden");
    expect(rail.className).not.toMatch(/overflow-[a-z]*-(auto|scroll)/);
    // Every row grows (`flex-1`) so the marks spread over the pane's height
    // instead of stacking at the top, and carries a MINIMUM height: below the
    // floor the rows stop shrinking, the strip overflows, and `overflow-hidden`
    // clips the tail — which honestly says "more items than fit" where a
    // scrollbar would say "go scroll inside the sliver".
    const rows = [...container.querySelectorAll("[data-todo-row]")];
    expect(rows).toHaveLength(3);
    for (const row of rows) {
      expect(row.className).toContain("flex-1");
      expect(row.className).toMatch(/min-h-/);
      // …and a CEILING, or a 3-item board stretches its marks 600px apart down
      // a tall pane and stops reading as a list.
      expect(row.className).toMatch(/max-h-/);
    }
    // The column is TOP-anchored. This is not cosmetic: an overflowing flex
    // column CENTERS its overflow when justified to the middle, which would cut
    // the FIRST todos off the top of the rail. The head of the list is what
    // says "this is a list", so the tail is what must get cut.
    expect(rail.className).toContain("justify-start");
    expect(rail.className).not.toContain("justify-center");
  });

  it("stays out of the way: no pointer capture (the toggle under it must stay clickable)", () => {
    render(<TodoRail items={items("pending")} />);
    expect(screen.getByTestId("todo-rail").className).toContain("pointer-events-none");
  });

  it("clears the frame's bottom-left corner, where the pane's own toggle lives", () => {
    render(<TodoRail items={items("pending")} />);
    const cls = screen.getByTestId("todo-rail").className;
    // Anchored to the top and offset from the bottom by at least the toggle's
    // box (`bottom-2` + `size-6` = 32px), or the last dot lands under it.
    const bottom = cls.match(/\bbottom-(\d+)/);
    expect(bottom, "the rail is not offset from the bottom").toBeTruthy();
    expect(Number(bottom![1]) * 4).toBeGreaterThanOrEqual(32);
  });

  it("announces itself as a todo summary for the accessible name", () => {
    render(<TodoRail items={items("completed", "completed", "pending")} />);
    // `role="img"` + the count: the marks carry no text, so without this the
    // rail is an empty region to a screen reader (and the real board behind it
    // is `invisible` while collapsed, so it is not in the tree either).
    expect(screen.getByRole("img", { name: "Todos 2/3" })).toBeTruthy();
  });

  it("renders nothing for an empty board", () => {
    const { container } = render(<TodoRail items={[]} />);
    expect(container.textContent).toBe("");
    expect(screen.queryByTestId("todo-rail")).toBeNull();
  });
});
