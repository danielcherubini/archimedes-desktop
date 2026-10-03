import { describe, expect, it } from "vitest";
import { fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch (case-insensitive subsequence — the model picker's search)", () => {
  it("matches a query whose characters appear in order (subsequence)", () => {
    expect(fuzzyMatch("qwen", "Qwen3.8")).toBe(true);
    expect(fuzzyMatch("gpt", "GPT-4 Turbo")).toBe(true);
    expect(fuzzyMatch("cla", "Claude")).toBe(true);
  });

  it("is case-insensitive", () => {
    expect(fuzzyMatch("QWEN", "Qwen3.8")).toBe(true);
    expect(fuzzyMatch("qWeN", "qwen")).toBe(true);
  });

  it("does NOT match when the order breaks (subsequence, not bag-of-chars)", () => {
    // `nw` — the `n` comes AFTER the `w` in "Qwen3.8", so the in-order
    // scan fails.
    expect(fuzzyMatch("nw", "Qwen3.8")).toBe(false);
  });

  it("does NOT match when a character is missing", () => {
    expect(fuzzyMatch("qwen9", "Qwen3.8")).toBe(false);
  });

  it("matches the empty query against anything", () => {
    expect(fuzzyMatch("", "anything")).toBe(true);
    expect(fuzzyMatch("", "")).toBe(true);
  });

  it("matches a query equal to the whole target", () => {
    expect(fuzzyMatch("Qwen3.8", "Qwen3.8")).toBe(true);
  });
});
