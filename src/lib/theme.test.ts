import { describe, expect, it, beforeEach, vi } from "vitest";
import { applySettingsTheme, applyThemeToDocument, resolveTheme } from "./theme";

/** A CAPTURING matchMedia stub: `addEventListener` stores the callback so
 * the test can invoke it (a plain `vi.fn()` stub DISCARDS the callback and
 * the cleanup assertion would be vacuous). */
function stubMatchMedia(initialMatches: boolean) {
  let matches = initialMatches;
  const listeners: Record<string, () => void> = {};
  const removeEventListener = vi.fn();
  vi.stubGlobal("matchMedia", (q: string) => ({
    get matches() {
      return matches;
    },
    media: q,
    addEventListener: (event: string, cb: () => void) => {
      listeners[event] = cb;
    },
    removeEventListener,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
  return {
    removeEventListener,
    flip: (value: boolean) => {
      matches = value;
    },
    fire: (event: string) => listeners[event]?.(),
  };
}

describe("applyThemeToDocument", () => {
  beforeEach(() => {
    document.documentElement.classList.remove(
      "dark",
      "theme-zai-light",
      "theme-zai-dark",
    );
  });

  it("adds theme-zai-dark AND dark for the dark theme and removes theme-zai-light", () => {
    applyThemeToDocument("zai-dark");
    const classes = document.documentElement.classList;
    expect(classes.contains("theme-zai-dark")).toBe(true);
    expect(classes.contains("dark")).toBe(true);
    expect(classes.contains("theme-zai-light")).toBe(false);
  });

  it("flips all three classes when switching to the light theme", () => {
    applyThemeToDocument("zai-dark");
    applyThemeToDocument("zai-light");
    const classes = document.documentElement.classList;
    expect(classes.contains("theme-zai-light")).toBe(true);
    expect(classes.contains("dark")).toBe(false);
    expect(classes.contains("theme-zai-dark")).toBe(false);
  });
});

describe("resolveTheme", () => {
  beforeEach(() => {
    document.documentElement.classList.remove(
      "dark",
      "theme-zai-light",
      "theme-zai-dark",
    );
    vi.unstubAllGlobals();
  });

  it("maps the concrete themes", () => {
    expect(resolveTheme("dark")).toBe("zai-dark");
    expect(resolveTheme("light")).toBe("zai-light");
  });

  it("resolves 'system' to the OS scheme", () => {
    stubMatchMedia(true);
    expect(resolveTheme("system")).toBe("zai-dark");
    vi.unstubAllGlobals();
    stubMatchMedia(false);
    expect(resolveTheme("system")).toBe("zai-light");
  });
});

describe("applySettingsTheme", () => {
  beforeEach(() => {
    document.documentElement.classList.remove(
      "dark",
      "theme-zai-light",
      "theme-zai-dark",
    );
    vi.unstubAllGlobals();
  });

  it("applies the concrete theme; the cleanup is a no-op", () => {
    const cleanup = applySettingsTheme("light");
    expect(document.documentElement.classList.contains("theme-zai-light")).toBe(
      true,
    );
    expect(() => cleanup()).not.toThrow();
  });

  it("re-applies on an OS scheme change and the cleanup removes the listener", () => {
    const mm = stubMatchMedia(true);
    const cleanup = applySettingsTheme("system");
    // OS is dark → the dark theme applied.
    expect(document.documentElement.classList.contains("theme-zai-dark")).toBe(
      true,
    );
    // The OS flips to light → the captured "change" listener re-applies.
    mm.flip(false);
    mm.fire("change");
    expect(document.documentElement.classList.contains("theme-zai-light")).toBe(
      true,
    );
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    // The cleanup removes the SAME listener.
    cleanup();
    expect(mm.removeEventListener).toHaveBeenCalledTimes(1);
    expect(mm.removeEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );
  });
});
