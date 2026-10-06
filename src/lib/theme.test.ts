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
      "theme-dracula",
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
      "theme-dracula",
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
      "theme-dracula",
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

describe("palette axis", () => {
  // (ADR 0027) The palette is orthogonal to the light/dark mode, and
  // `dracula` pins dark. The invariant that matters is that at most ONE
  // `.theme-*` class is ever on `<html>` — the palette blocks are single-class
  // selectors of equal specificity, so two co-existing would be resolved by
  // source order in `index.css` (silently, and it would flip on any reorder).
  beforeEach(() => {
    document.documentElement.classList.remove(
      "dark",
      "theme-zai-light",
      "theme-zai-dark",
      "theme-dracula",
    );
    vi.unstubAllGlobals();
  });

  it("applies dark + theme-dracula for dracula over the LIGHT theme, with no zai class", () => {
    applyThemeToDocument("zai-light", "dracula");
    const classes = document.documentElement.classList;
    expect(classes.contains("dark")).toBe(true);
    expect(classes.contains("theme-dracula")).toBe(true);
    expect(classes.contains("theme-zai-light")).toBe(false);
    expect(classes.contains("theme-zai-dark")).toBe(false);
  });

  it("removes theme-dracula when the palette goes back to zai", () => {
    applyThemeToDocument("zai-dark", "dracula");
    applyThemeToDocument("zai-dark", "zai");
    const classes = document.documentElement.classList;
    expect(classes.contains("theme-dracula")).toBe(false);
    expect(classes.contains("theme-zai-dark")).toBe(true);
    expect(classes.contains("dark")).toBe(true);
  });

  it("resolves dracula to the dark app theme before the mode (even 'light')", () => {
    // No `matchMedia` stub: dracula must short-circuit BEFORE any
    // mode/OS-scheme resolution, so even an unimplemented `matchMedia`
    // (jsdom) cannot influence the answer.
    expect(resolveTheme("light", "dracula")).toBe("zai-dark");
    expect(resolveTheme("dark", "dracula")).toBe("zai-dark");
  });

  it("keeps theme-dracula + dark on an OS-light machine when the theme is 'system'", () => {
    stubMatchMedia(false);
    applySettingsTheme("system", "dracula");
    const classes = document.documentElement.classList;
    expect(classes.contains("theme-dracula")).toBe(true);
    expect(classes.contains("dark")).toBe(true);
    expect(classes.contains("theme-zai-light")).toBe(false);
  });
});

/**
 * THE central hazard of ADR 0027, stated in `theme.ts`'s own comment and in the
 * ADR: every palette block is a single-class selector of EQUAL specificity, so
 * if two `.theme-*` classes ever co-exist on `<html>` the winner is decided by
 * SOURCE ORDER in `index.css` — silently, and it flips on any reorder. Nothing
 * here before this matrix enforced that: `theme.ts` toggles all four classes
 * with explicit booleans, and dropping the `!dracula` guard from the two zai
 * toggles (so `theme-zai-dark` and `theme-dracula` co-apply) left 113 tests
 * green.
 *
 * So: exhaust the state space (mode × palette × OS scheme), and in every cell
 * assert (a) EXACTLY ONE `.theme-*` class, (b) `dark` present iff the resolved
 * mode is dark (Dracula always dark), and (c) which class it is. A cell that
 * asserted only "not two" would still pass if a toggle were dropped entirely,
 * so the expected class is named, not counted around.
 */
describe("the class invariant: exactly ONE .theme-* class, in every state", () => {
  const MODES = ["light", "dark", "system"] as const;
  // Every value the persisted `palette` can hold: the two real palettes, the
  // `null` default the Settings page writes, an out-of-enum value from a
  // hand-edited `settings.json`, and `undefined` (a pre-feature document, and
  // the shape every non-palette call site uses).
  const PALETTES: (string | null | undefined)[] = [
    "zai",
    "dracula",
    null,
    "bogus",
    undefined,
  ];
  const OS_PREFS = [true, false] as const; // prefers-color-scheme: dark?
  const THEME_CLASSES = ["theme-zai-light", "theme-zai-dark", "theme-dracula"];

  beforeEach(() => {
    document.documentElement.classList.remove(
      "dark",
      "theme-zai-light",
      "theme-zai-dark",
      "theme-dracula",
    );
    vi.unstubAllGlobals();
  });

  for (const mode of MODES) {
    for (const palette of PALETTES) {
      for (const osDark of OS_PREFS) {
        // `palette: "zai"` is deliberately listed twice so the matrix also
        // drives the `applyThemeToDocument(theme)` single-arg call shape that
        // the rest of the app uses.
        const label = `${mode} / palette=${JSON.stringify(palette)} / OS ${osDark ? "dark" : "light"}`;
        it(`applies exactly one theme class for ${label}`, () => {
          stubMatchMedia(osDark);
          const settings = applySettingsTheme(
            mode,
            palette as "zai" | "dracula" | undefined,
          );

          const root = document.documentElement;
          const present = THEME_CLASSES.filter((c) =>
            root.classList.contains(c),
          );
          // Dracula pins dark, so the mode (and the OS scheme behind
          // `"system"`) never decides it; zai defers to the resolved mode.
          const modeIsDark =
            palette === "dracula" ||
            (mode === "system" ? osDark : mode === "dark");
          const expected =
            palette === "dracula"
              ? "theme-dracula"
              : modeIsDark
                ? "theme-zai-dark"
                : "theme-zai-light";

          expect(present, `theme classes present for ${label}`).toEqual([
            expected,
          ]);
          expect(root.classList.contains("dark"))
            .toBe(modeIsDark);
          settings(); // the cleanup must not throw
          vi.unstubAllGlobals();
        });
      }
    }
  }

  it("also holds when applyThemeToDocument is driven directly (the resolved-theme entry point)", () => {
    // The same invariant from the other entry point: `main.tsx` and the error
    // path in `loadAndApplySettings` call this one, and a caller may hand it
    // `zai-light` together with `dracula` (the resolution lives upstream).
    for (const theme of ["zai-light", "zai-dark"] as const) {
      for (const palette of ["zai", "dracula", undefined] as const) {
        document.documentElement.classList.remove(
          "dark",
          "theme-zai-light",
          "theme-zai-dark",
          "theme-dracula",
        );
        applyThemeToDocument(theme, palette);
        const present = THEME_CLASSES.filter((c) =>
          document.documentElement.classList.contains(c),
        );
        expect(
          present,
          `theme=${theme} palette=${JSON.stringify(palette)}`,
        ).toHaveLength(1);
      }
    }
  });

  it("is not a vacuous matrix (both palettes and both modes are really covered)", () => {
    // A gate that skips everything and a gate that passes are the same green,
    // so pin the coverage: 3 modes × 6 palette values × 2 OS schemes.
    expect(MODES.length * PALETTES.length * OS_PREFS.length).toBe(30);
    // And each of the three theme classes is reachable in at least one cell.
    const reached = new Set<string>();
    for (const mode of MODES) {
      for (const palette of PALETTES) {
        for (const osDark of OS_PREFS) {
          stubMatchMedia(osDark);
          applySettingsTheme(mode, palette as "zai" | "dracula" | undefined);
          for (const c of THEME_CLASSES) {
            if (document.documentElement.classList.contains(c)) reached.add(c);
          }
          vi.unstubAllGlobals();
        }
      }
    }
    expect([...reached].sort()).toEqual([
      "theme-dracula",
      "theme-zai-dark",
      "theme-zai-light",
    ]);
  });
});
