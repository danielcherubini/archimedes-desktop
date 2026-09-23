import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import FileSummaryCard from "./FileSummaryCard";

/** Build a single-hunk unified patch from added/removed content lines. */
const patch = (added: string[], removed: string[]) =>
  [
    "@@ -1 +1",
    ...removed.map((l) => `-${l}`),
    ...added.map((l) => `+${l}`),
  ].join("\n");

describe("FileSummaryCard", () => {
  it("sums the per-file stats into the header totals", () => {
    // file 1: +2 −1; file 2: +0 −3 → totals +2 −4
    render(
      <FileSummaryCard
        diffs={[
          { path: "src/a.ts", patch: patch(["x1", "x2"], ["y1"]) },
          { path: "src/b.ts", patch: patch([], ["z1", "z2", "z3"]) },
        ]}
      />,
    );
    expect(screen.getByText("2 files changed")).toBeTruthy();
    // The header total AND file 1's own stats are both `+2` (the fixture's
    // first file matches the total) — both render.
    expect(screen.getAllByText("+2")).toHaveLength(2);
    expect(screen.getByText("−4")).toBeTruthy();
    // Each file row shows its path + stats.
    const rowA = screen.getByText("src/a.ts").parentElement!;
    expect(rowA.textContent).toContain("+2");
    expect(rowA.textContent).toContain("−1");
    const rowB = screen.getByText("src/b.ts").parentElement!;
    expect(rowB.textContent).toContain("+0");
    expect(rowB.textContent).toContain("−3");
  });

  it("renders one row for a single-file patch", () => {
    const { container } = render(
      <FileSummaryCard diffs={[{ path: "src/a.ts", patch: patch(["x"], ["y"]) }]} />,
    );
    expect(screen.getByText("1 file changed")).toBeTruthy();
    // Exactly one `h-8` file row.
    expect(container.querySelectorAll(".h-8")).toHaveLength(1);
    expect(screen.getByText("src/a.ts")).toBeTruthy();
  });

  it("renders +0 −0 for a malformed patch (no crash)", () => {
    render(
      <FileSummaryCard
        diffs={[{ path: "src/a.ts", patch: "no plus or minus lines here" }]}
      />,
    );
    expect(screen.getByText("1 file changed")).toBeTruthy();
    // Header total and the file row both read +0 −0.
    expect(screen.getAllByText("+0")).toHaveLength(2);
    expect(screen.getAllByText("−0")).toHaveLength(2);
  });

  it("dedupes by path (last wins) before computing stats", () => {
    // The store re-emits a standalone `diff` on every `tool_call_update`
    // carrying content: the same file appears twice in a turn.
    render(
      <FileSummaryCard
        diffs={[
          { path: "src/a.ts", patch: patch(["x"], ["y"]) }, // +1 −1
          { path: "src/a.ts", patch: patch(["x1", "x2"], ["y1", "y2", "y3"]) }, // +2 −3
        ]}
      />,
    );
    expect(screen.getByText("1 file changed")).toBeTruthy();
    // The LATEST patch wins: +2 −3 (header total = the single file's
    // stats, so each appears twice: header + row), and the superseded
    // +1 −1 is gone.
    expect(screen.getAllByText("+2")).toHaveLength(2);
    expect(screen.getAllByText("−3")).toHaveLength(2);
    expect(screen.queryByText("+1")).toBeNull();
    expect(screen.queryByText("−1")).toBeNull();
  });
});
