import { describe, expect, it } from "vitest";
import {
  pickQuip,
  QUIP_ROTATION_MAX_SECS,
  QUIP_ROTATION_MIN_SECS,
  SPIN_QUIPS,
} from "./spin-quips";

describe("SPIN_QUIPS invariants (mirrors the core's spin-quips.test.ts)", () => {
  it("has 20-60 entries", () => {
    expect(SPIN_QUIPS.length).toBeGreaterThanOrEqual(20);
    expect(SPIN_QUIPS.length).toBeLessThanOrEqual(60);
  });

  it("every entry is 1-64 printable-ASCII chars", () => {
    for (const quip of SPIN_QUIPS) {
      expect(quip.length).toBeGreaterThanOrEqual(1);
      expect(quip.length).toBeLessThanOrEqual(64);
      expect(quip).toMatch(/^[\x20-\x7E]+$/);
    }
  });

  it("entries are unique", () => {
    expect(new Set(SPIN_QUIPS).size).toBe(SPIN_QUIPS.length);
  });

  it('"Working..." is the first entry', () => {
    expect(SPIN_QUIPS[0]).toBe("Working...");
  });

  it("rotation bounds are 15s and 45s", () => {
    expect(QUIP_ROTATION_MIN_SECS).toBe(15);
    expect(QUIP_ROTATION_MAX_SECS).toBe(45);
  });
});

describe("pickQuip", () => {
  it("with no exclude and rand=0 returns the first entry", () => {
    expect(pickQuip(undefined, () => 0)).toBe(SPIN_QUIPS[0]);
  });

  it("a first draw equal to exclude triggers exactly one re-roll restricted to entries != exclude", () => {
    expect(pickQuip("Working...", () => 0)).toBe(SPIN_QUIPS[1]);
  });

  it("a first draw different from exclude is returned without a re-roll", () => {
    // floor(0.5 * 28) = 14; SPIN_QUIPS[14] !== "Working...", so no re-roll.
    expect(pickQuip("Working...", () => 0.5)).toBe(SPIN_QUIPS[14]);
  });
});
