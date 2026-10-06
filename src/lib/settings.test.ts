import { beforeEach, describe, expect, it, vi } from "vitest";

// The settings module reads the settings through the tauri binding — mock
// the whole module (the real one drags in @tauri-apps/api, unavailable in
// jsdom).
vi.mock("./tauri", () => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
}));

import { getSettings, type AppSettings } from "./tauri";
import { applyThemeToDocument } from "./theme";
import {
  applySettingsFont,
  applySettingsToDocument,
  loadAndApplySettings,
} from "./settings";

/** A simple matchMedia stub with a flippable `matches` (the settings tests
 * never invoke the "change" listener — the live re-apply is covered in
 * theme.test.ts). */
function stubMatchMedia(initialMatches: boolean) {
  let matches = initialMatches;
  vi.stubGlobal("matchMedia", (q: string) => ({
    get matches() {
      return matches;
    },
    media: q,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}

/** All custom-property assertions read via `style.getPropertyValue` —
 * jsdom's `getComputedStyle` cascade for custom properties is unreliable,
 * while the inline `style` read is reliable. */
const style = () => document.documentElement.style;

/**
 * A complete `AppSettings` document for the wiring tests. `theme: "light"` on
 * purpose: the palette assertions below then fail if the palette ever stops
 * reaching `theme.ts`, because a light mode that stayed in charge would put
 * `.theme-zai-light` on `<html>` instead of `.dark` + `.theme-dracula`.
 */
function settingsFixture(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    theme: "light",
    palette: null,
    paneLayout: {},
    defaultTrustNewSpaces: false,
    defaultModel: null,
    defaultThinkingLevel: null,
    enabledTools: [],
    providers: [],
    mcpServers: {},
    font: { sizePx: 14, uiFamily: null, codeFamily: null },
    defaultThinkingLevels: {},
    subagentModels: {},
    spinnerStyle: null,
    ...overrides,
  };
}

/** The four classes `applyThemeToDocument` owns. Every test starts from a
 * document that carries none of them, so a `contains(…) === false` assertion
 * cannot pass merely because a previous test never set the class. */
const THEME_CLASSES = ["dark", "theme-zai-light", "theme-zai-dark", "theme-dracula"];
const resetRoot = () => document.documentElement.classList.remove(...THEME_CLASSES);

describe("applySettingsFont", () => {
  beforeEach(() => {
    resetRoot();
    document.documentElement.style.removeProperty("--ui-font-size");
    document.documentElement.style.removeProperty("--font-sans");
    document.documentElement.style.removeProperty("--font-mono");
    vi.unstubAllGlobals();
  });

  it("sets the size variable clamped to [12, 20]", () => {
    applySettingsFont({ sizePx: 500, uiFamily: null, codeFamily: null });
    expect(style().getPropertyValue("--ui-font-size")).toBe("20px");
    applySettingsFont({ sizePx: 5, uiFamily: null, codeFamily: null });
    expect(style().getPropertyValue("--ui-font-size")).toBe("12px");
    applySettingsFont({ sizePx: 14, uiFamily: null, codeFamily: null });
    expect(style().getPropertyValue("--ui-font-size")).toBe("14px");
  });

  it("overrides the families with the existing tails; nulls leave nothing inline", () => {
    applySettingsFont({ sizePx: 14, uiFamily: "Inter", codeFamily: null });
    const sans = style().getPropertyValue("--font-sans");
    expect(sans.toLowerCase().startsWith("inter, ")).toBe(true);
    expect(sans).toContain("Noto Color Emoji");

    applySettingsFont({ sizePx: 14, uiFamily: null, codeFamily: "JetBrains Mono" });
    const mono = style().getPropertyValue("--font-mono");
    expect(mono.toLowerCase().startsWith("jetbrains mono, ")).toBe(true);
    // The CJK tail is preserved.
    expect(mono).toContain("Noto Sans CJK SC");
    expect(mono).toContain("monospace");

    // Both null → nothing inline-set (the index.css `@theme` values stand).
    applySettingsFont({ sizePx: 14, uiFamily: null, codeFamily: null });
    expect(style().getPropertyValue("--font-sans")).toBe("");
    expect(style().getPropertyValue("--font-mono")).toBe("");
  });

  it("removes a stale family override when set back to null", () => {
    applySettingsFont({ sizePx: 14, uiFamily: "Serif", codeFamily: null });
    expect(style().getPropertyValue("--font-sans")).not.toBe("");
    applySettingsFont({ sizePx: 14, uiFamily: null, codeFamily: null });
    expect(style().getPropertyValue("--font-sans")).toBe("");
  });
});

describe("loadAndApplySettings", () => {
  beforeEach(() => {
    // Simulate the first-frame state (main.tsx's `applyThemeToDocument("zai-dark")`).
    resetRoot();
    applyThemeToDocument("zai-dark");
    document.documentElement.style.removeProperty("--ui-font-size");
    document.documentElement.style.removeProperty("--font-sans");
    document.documentElement.style.removeProperty("--font-mono");
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("applies the stored settings (system theme + font)", async () => {
    vi.mocked(getSettings).mockResolvedValue({
      theme: "system",
      palette: null,
      paneLayout: {},
      defaultTrustNewSpaces: false,
      defaultModel: null,
      defaultThinkingLevel: null,
      enabledTools: [],
      providers: [],
      mcpServers: {},
      font: { sizePx: 16, uiFamily: "Inter", codeFamily: null },
      defaultThinkingLevels: {},
      subagentModels: {},
      spinnerStyle: null,
    });
    stubMatchMedia(true); // OS is dark → "system" resolves to zai-dark
    const settings = await loadAndApplySettings();
    expect(settings).not.toBeNull();
    expect(document.documentElement.classList.contains("theme-zai-dark")).toBe(
      true,
    );
    expect(style().getPropertyValue("--ui-font-size")).toBe("16px");
    expect(style().getPropertyValue("--font-sans")).not.toBe("");
  });

  it("a getSettings failure: resolves null, the dark first-frame state stands (no throw)", async () => {
    vi.mocked(getSettings).mockRejectedValue(new Error("boom"));
    const settings = await loadAndApplySettings();
    expect(settings).toBeNull();
    expect(document.documentElement.classList.contains("theme-zai-dark")).toBe(
      true,
    );
  });

  it("applies a stored `dracula` palette to the document (boot wiring, ADR 0027)", async () => {
    // The persisted `palette` reaches `<html>` ONLY through
    // `applySettingsToDocument` → `applySettingsTheme`'s second argument, and
    // this boot path is the only place that happens on launch. Sever that one
    // argument and everything else still looks healthy: the save succeeds, the
    // store updates, code blocks recolour (they subscribe to the store
    // directly), and the chrome silently stays Zai forever.
    vi.mocked(getSettings).mockResolvedValue(
      settingsFixture({ theme: "light", palette: "dracula" }),
    );
    stubMatchMedia(false); // OS prefers LIGHT: only the palette pins dark.

    await loadAndApplySettings();

    const classes = document.documentElement.classList;
    expect(classes.contains("theme-dracula")).toBe(true);
    expect(classes.contains("dark")).toBe(true);
    expect(classes.contains("theme-zai-light")).toBe(false);
    expect(classes.contains("theme-zai-dark")).toBe(false);
  });

  it("applies a stored `zai` palette to the document (the default stays wired too)", async () => {
    vi.mocked(getSettings).mockResolvedValue(
      settingsFixture({ theme: "light", palette: "zai" }),
    );
    stubMatchMedia(false);

    await loadAndApplySettings();

    const classes = document.documentElement.classList;
    expect(classes.contains("theme-zai-light")).toBe(true);
    expect(classes.contains("theme-dracula")).toBe(false);
    expect(classes.contains("dark")).toBe(false);
  });
});

describe("applySettingsToDocument — the palette → <html> wiring (ADR 0027)", () => {
  beforeEach(() => {
    resetRoot();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it("puts theme-dracula + dark on <html> for a dracula palette over the LIGHT theme", () => {
    // `theme: "light"` is the point: Dracula has no light reading, so the
    // palette must pin dark rather than defer to the mode.
    applySettingsToDocument(settingsFixture({ palette: "dracula" }));

    const classes = document.documentElement.classList;
    expect(classes.contains("theme-dracula")).toBe(true);
    expect(classes.contains("dark")).toBe(true);
    expect(classes.contains("theme-zai-light")).toBe(false);
    expect(classes.contains("theme-zai-dark")).toBe(false);
  });

  it("puts theme-zai-light on <html> for an explicit zai palette", () => {
    applySettingsToDocument(settingsFixture({ palette: "zai" }));

    const classes = document.documentElement.classList;
    expect(classes.contains("theme-zai-light")).toBe(true);
    expect(classes.contains("theme-dracula")).toBe(false);
    expect(classes.contains("dark")).toBe(false);
  });

  it("treats the null default as zai (a pre-feature settings.json keeps its chrome)", () => {
    applySettingsToDocument(settingsFixture({ palette: null }));

    const classes = document.documentElement.classList;
    expect(classes.contains("theme-zai-light")).toBe(true);
    expect(classes.contains("theme-dracula")).toBe(false);
  });

  it("keeps the font wiring alive on the palette path (both halves apply)", () => {
    applySettingsToDocument(
      settingsFixture({ palette: "dracula", font: { sizePx: 17, uiFamily: null, codeFamily: null } }),
    );
    expect(style().getPropertyValue("--ui-font-size")).toBe("17px");
    expect(document.documentElement.classList.contains("theme-dracula")).toBe(
      true,
    );
  });
});
