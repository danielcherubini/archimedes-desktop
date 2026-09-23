import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SPIN_QUIPS } from "../lib/spin-quips";
import { useSpinQuip } from "./useSpinQuip";

// rand: () => 0 makes every delay exactly QUIP_ROTATION_MIN_SECS (15s) and
// every pick land on SPIN_QUIPS[0] (or its one re-roll, SPIN_QUIPS[1]).
const zeroRand = { rand: () => 0 };

const Q0 = SPIN_QUIPS[0]!;
const Q1 = SPIN_QUIPS[1]!;

describe("useSpinQuip", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("initial working=false returns a quip and schedules no timer", () => {
    const { result } = renderHook(() => useSpinQuip(false, zeroRand));
    expect(result.current).toBe(Q0);
    // No timer is scheduled until the first false->true transition.
    act(() => {
      vi.advanceTimersByTime(100_000);
    });
    expect(result.current).toBe(Q0);
  });

  it("re-picks at the exact inclusive 15s boundary (14s -> no change, 15s -> change)", () => {
    const { result, rerender } = renderHook(
      ({ working }: { working: boolean }) => useSpinQuip(working, zeroRand),
      { initialProps: { working: false } },
    );
    expect(result.current).toBe(Q0);

    // false -> true: pickQuip(Q0) re-rolls to Q1, schedules a 15s timer.
    act(() => {
      rerender({ working: true });
    });
    expect(result.current).toBe(Q1);

    act(() => {
      vi.advanceTimersByTime(14_000);
    });
    expect(result.current).toBe(Q1);

    // Exactly 15s: the boundary fires.
    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    expect(result.current).toBe(Q0);

    // Far past (46s more): the quip has re-picked to a different entry.
    act(() => {
      vi.advanceTimersByTime(46_000);
    });
    expect(result.current).toBe(Q1);
    expect(result.current).not.toBe(Q0);
  });

  it("working false again clears the timer (advancing time changes nothing)", () => {
    const { result, rerender } = renderHook(
      ({ working }: { working: boolean }) => useSpinQuip(working, zeroRand),
      { initialProps: { working: false } },
    );
    act(() => {
      rerender({ working: true });
    });
    expect(result.current).toBe(Q1);
    act(() => {
      vi.advanceTimersByTime(15_000);
    });
    expect(result.current).toBe(Q0);
    act(() => {
      rerender({ working: false });
    });
    act(() => {
      vi.advanceTimersByTime(100_000);
    });
    expect(result.current).toBe(Q0);
  });

  it("a second working episode does not return the first episode's quip", () => {
    const { result, rerender } = renderHook(
      ({ working }: { working: boolean }) => useSpinQuip(working, zeroRand),
      { initialProps: { working: false } },
    );
    // First episode: quip is Q1.
    act(() => {
      rerender({ working: true });
    });
    expect(result.current).toBe(Q1);
    // End the episode (last quip is Q1).
    act(() => {
      rerender({ working: false });
    });
    // Second episode: pickQuip(Q1) must not return Q1 back-to-back.
    act(() => {
      rerender({ working: true });
    });
    expect(result.current).not.toBe(Q1);
    expect(result.current).toBe(Q0);
  });
});
