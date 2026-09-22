import { useEffect, useRef, useState } from "react";
import {
  pickQuip,
  QUIP_ROTATION_MAX_SECS,
  QUIP_ROTATION_MIN_SECS,
} from "../lib/spin-quips";

/**
 * Rotation quip for a "working" indicator.
 *
 * - At mount (before the first `working` episode) the quip is
 *   `pickQuip(undefined, rand)` — a quip is always available; no timer is
 *   scheduled.
 * - When `working` transitions false→true a fresh quip is picked via
 *   `pickQuip(lastQuip, rand)` — `lastQuip` is the previous episode's quip,
 *   so it is never returned back-to-back — and a re-pick is scheduled at a
 *   random delay drawn inclusive between `QUIP_ROTATION_MIN_SECS` and
 *   `QUIP_ROTATION_MAX_SECS` seconds: `delay = MIN + rand() * (MAX - MIN)`
 *   (so `rand: () => 0` → exactly 15s). While the episode continues the quip
 *   re-picks on that window.
 * - When `working` goes true→false the timer is cleared and the last quip is
 *   kept (the indicator is hidden while idle, so no reset-to-default).
 *
 * `deps.rand` is injectable for deterministic tests (the timer is driven by
 * vitest fake timers; the delay is a fixed function of `rand`).
 */
export function useSpinQuip(
  working: boolean,
  deps?: { rand?: () => number },
): string {
  const randRef = useRef(deps?.rand ?? Math.random);
  randRef.current = deps?.rand ?? Math.random;

  const [quip, setQuip] = useState(() => pickQuip(undefined, randRef.current));
  // The previous episode's quip — never returned back-to-back by pickQuip.
  const lastQuipRef = useRef(quip);

  useEffect(() => {
    if (!working) return;
    let cancelled = false;
    let timer: number | undefined;

    const pick = () => {
      const next = pickQuip(lastQuipRef.current, randRef.current);
      lastQuipRef.current = next;
      setQuip(next);
    };

    const delayMs = () =>
      (QUIP_ROTATION_MIN_SECS +
        randRef.current() * (QUIP_ROTATION_MAX_SECS - QUIP_ROTATION_MIN_SECS)) *
      1000;

    const schedule = () => {
      timer = window.setTimeout(() => {
        if (cancelled) return;
        pick();
        schedule();
      }, delayMs());
    };

    pick();
    schedule();

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [working]);

  return quip;
}
