import { describe, expect, it } from "vitest";
import { parseDiffStats, unifiedPatch } from "./diff";

describe("parseDiffStats", () => {
  it("counts 2 additions + 1 deletion in a single-hunk patch", () => {
    const patch = [
      "--- a/src/foo.ts",
      "+++ b/src/foo.ts",
      "@@ -1,3 +1,3 @@",
      " line1",
      "-old",
      "+new1",
      "+new2",
      " line3",
    ].join("\n");
    expect(parseDiffStats(patch)).toEqual({ additions: 2, deletions: 1 });
  });

  it("sums additions and deletions across multiple hunks", () => {
    const patch = [
      "--- a/src/foo.ts",
      "+++ b/src/foo.ts",
      "@@ -1,2 +1,2 @@",
      " ctx",
      "-old",
      "+add1",
      "@@ -5,2 +5,3 @@",
      " ctx2",
      "-del",
      "+add2",
      "+add3",
    ].join("\n");
    expect(parseDiffStats(patch)).toEqual({ additions: 3, deletions: 2 });
  });

  it("treats +++ /--- file headers as metadata, not content", () => {
    const patch = "--- a/src/foo.ts\n+++ b/src/foo.ts";
    expect(parseDiffStats(patch)).toEqual({ additions: 0, deletions: 0 });
  });

  it("counts a real added line whose content starts with --- as an addition", () => {
    const patch = [
      "--- a/src/foo.ts",
      "+++ b/src/foo.ts",
      "@@ -1 +1 @@",
      "+--- separator",
    ].join("\n");
    expect(parseDiffStats(patch)).toEqual({ additions: 1, deletions: 0 });
  });

  it("counts a deleted line whose content is `-- x` (patch line `--- x`) after a hunk as a deletion", () => {
    const patch = [
      "--- a/src/foo.md",
      "+++ b/src/foo.md",
      "@@ -1,2 +1,1 @@",
      " keep",
      "--- x",
    ].join("\n");
    expect(parseDiffStats(patch)).toEqual({ additions: 0, deletions: 1 });
  });

  it("counts an added line whose content is `++ y` (patch line `+++ y`) after a hunk as an addition", () => {
    const patch = [
      "--- a/src/foo.md",
      "+++ b/src/foo.md",
      "@@ -1,1 +1,2 @@",
      " keep",
      "+++ y",
    ].join("\n");
    expect(parseDiffStats(patch)).toEqual({ additions: 1, deletions: 0 });
  });

  it("returns 0/0 for a context-only patch", () => {
    const patch = [
      "--- a/src/foo.ts",
      "+++ b/src/foo.ts",
      "@@ -1,2 +1,2 @@",
      " a",
      " b",
    ].join("\n");
    expect(parseDiffStats(patch)).toEqual({ additions: 0, deletions: 0 });
  });

  it("returns 0/0 for an empty string", () => {
    expect(parseDiffStats("")).toEqual({ additions: 0, deletions: 0 });
  });
});

describe("unifiedPatch (regression guard)", () => {
  it("emits exactly one new file's patch per call with /dev/null headers", () => {
    const patch = unifiedPatch("src/foo.ts", null, "line1\nline2");
    expect(patch).toBe(
      [
        "--- /dev/null",
        "+++ bsrc/foo.ts",
        "",
        "@@ -1,0 +1,2 @@",
        "+line1",
        "+line2",
      ].join("\n"),
    );
  });

  it("emits exactly one modified file's patch per call with a/b headers", () => {
    const patch = unifiedPatch("src/foo.ts", "a\nb\nc", "a\nX\nc");
    expect(patch).toBe(
      [
        "--- asrc/foo.ts",
        "+++ bsrc/foo.ts",
        "",
        "@@ -1,3 +1,3 @@",
        " a",
        "-b",
        "+X",
        " c",
      ].join("\n"),
    );
  });
});
