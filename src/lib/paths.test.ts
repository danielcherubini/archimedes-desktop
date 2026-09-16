import { describe, expect, it } from "vitest";
import { basenameOfPath } from "./paths";

describe("basenameOfPath", () => {
  it("returns the base name of a unix path", () => {
    expect(basenameOfPath("/a/b")).toBe("b");
  });

  it("returns the base name of a windows path (backslash separators)", () => {
    expect(basenameOfPath("C:\\x\\y")).toBe("y");
  });

  it("strips a trailing separator", () => {
    expect(basenameOfPath("/a/b/")).toBe("b");
  });

  it("returns \"\" for a bare root", () => {
    expect(basenameOfPath("/")).toBe("");
  });

  it("returns \"\" for empty input", () => {
    expect(basenameOfPath("")).toBe("");
  });

  it("returns the base name of a realistic path", () => {
    expect(
      basenameOfPath("/home/daniel/Coding/AI/archimedes-desktop"),
    ).toBe("archimedes-desktop");
  });
});

