import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { BrailleLoader } from "@/components/ui/braille-loader";
import {
  brailleLoaderVariants,
  generateFrames,
  getVariantGridSize,
  normalizeVariant,
} from "@/lib/braille-loader";

/**
 * FULL matchMedia stub — the component's usePrefersReducedMotion calls
 * addEventListener/removeEventListener on the MediaQueryList, so a bare
 * { matches } object throws.
 */
function installMatchMediaStub(reduced: boolean) {
  // The registry stub is a structural subset of MediaQueryList (the component
  // only uses matches/media/addEventListener/removeEventListener) — cast it.
  const impl = (
    (q: string) => ({
      matches: q === "(prefers-reduced-motion: reduce)" ? reduced : false,
      media: q,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })
  ) as unknown as (q: string) => MediaQueryList;
  // jsdom does not implement window.matchMedia — provide the function first so
  // vi.spyOn has a function to wrap (and the component's "matchMedia" in window
  // guard passes) before the spy takes over.
  if (typeof window.matchMedia !== "function") {
    window.matchMedia = impl;
  }
  return vi.spyOn(window, "matchMedia").mockImplementation(impl);
}

describe("braille-loader lib", () => {
  it("exposes 25 variants including typing", () => {
    expect(brailleLoaderVariants).toHaveLength(25);
    expect(brailleLoaderVariants).toContain("typing");
  });

  it("generateFrames is deterministic via the frameCache", () => {
    const [w, h] = getVariantGridSize("typing");
    const a = generateFrames("typing", w, h);
    const b = generateFrames("typing", w, h);
    // same cached array (reference identity) and same content
    expect(b.frames).toBe(a.frames);
    expect(b.frames).toEqual(a.frames);
  });

  it("typing frames[0] is braille codepoints only — never ASCII spaces", () => {
    const [w, h] = getVariantGridSize("typing");
    const { frames } = generateFrames("typing", w, h);
    // braille codepoints U+2800–U+28FF (including the blank U+2800)
    expect(frames[0]).toMatch(new RegExp(`^[\\u2800-\\u28ff]{${w}}$`));
    // the field buffer maps mask 0 → \u2800, not " "
    expect(frames[0]).not.toContain(" ");
  });

  it("normalizeVariant maps unknown/absent variants to breathe", () => {
    expect(normalizeVariant("bogus")).toBe("breathe");
    expect(normalizeVariant("typing")).toBe("typing");
    expect(normalizeVariant(undefined)).toBe("breathe");
  });
});

describe("BrailleLoader component", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("renders with role=status, aria-live=polite, and an sr-only label", () => {
    installMatchMediaStub(false);
    render(<BrailleLoader variant="typing" />);
    const status = screen.getByRole("status");
    expect(status.getAttribute("aria-live")).toBe("polite");
    const srOnly = status.querySelector(".sr-only");
    expect(srOnly?.textContent).toBe("Loading");
  });

  it("advances frames on the interval (typing 50ms × 0.6 fast = 30ms)", () => {
    installMatchMediaStub(false);
    const [w, h] = getVariantGridSize("typing");
    const { frames, interval } = generateFrames("typing", w, h);
    const { unmount } = render(<BrailleLoader variant="typing" speed="fast" />);
    const span = screen
      .getByRole("status")
      .querySelector("span[aria-hidden='true']") as HTMLSpanElement;
    // mount → frame 0
    expect(span.textContent).toBe(frames[0]);
    vi.advanceTimersByTime(interval * 0.6);
    // one tick → frame 1
    expect(span.textContent).toBe(frames[1]);
    unmount();
  });

  it("stays static under prefers-reduced-motion (full matchMedia stub)", () => {
    installMatchMediaStub(true);
    const [w, h] = getVariantGridSize("typing");
    const { frames } = generateFrames("typing", w, h);
    const { unmount } = render(<BrailleLoader variant="typing" speed="fast" />);
    const span = screen
      .getByRole("status")
      .querySelector("span[aria-hidden='true']") as HTMLSpanElement;
    expect(span.textContent).toBe(frames[0]);
    vi.advanceTimersByTime(1000);
    expect(span.textContent).toBe(frames[0]);
    unmount();
  });
});
