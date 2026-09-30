import { beforeEach, describe, expect, it, vi } from "vitest";

// The settings module reads the settings through the tauri binding — mock
// the whole module (the real one drags in @tauri-apps/api, unavailable in
// jsdom).
vi.mock("./tauri", () => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
}));

import { getSettings } from "./tauri";
import { applyThemeToDocument } from "./theme";
import { applySettingsFont, loadAndApplySettings } from "./settings";

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

describe("applySettingsFont", () => {
  beforeEach(() => {
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
    document.documentElement.classList.remove(
      "dark",
      "theme-zai-light",
      "theme-zai-dark",
    );
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
      paneLayout: {},
      defaultAgent: null,
      defaultTrustNewSpaces: false,
      defaultModel: null,
      providers: [],
      font: { sizePx: 16, uiFamily: "Inter", codeFamily: null },
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
});
