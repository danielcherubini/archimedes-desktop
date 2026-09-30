import { getSettings, type AppSettings, type FontSettings } from "./tauri";
import { applySettingsTheme, applyThemeToDocument } from "./theme";

/** The font-size clamp bounds (the settings UI's slider range). */
export const MIN_FONT_PX = 12;
export const MAX_FONT_PX = 20;

/** The design system's existing font stacks' tails (verbatim from
 * `src/index.css` `@theme`) — a custom family is PREPENDED to the tail so
 * the platform fallbacks (incl. the CJK fonts) are always preserved. */
const SANS_TAIL =
  'ui-sans-serif, system-ui, sans-serif, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol", "Noto Color Emoji"';
const MONO_TAIL =
  'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", "Microsoft YaHei UI", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", monospace';

/**
 * Apply the settings' font to `document.documentElement` as CSS custom
 * properties: `--ui-font-size: <clamped>px` always (set via
 * `style.setProperty`); `--font-sans: "<family>, <tail>"` when `font.uiFamily`
 * is non-null, else `style.removeProperty("--font-sans")` (a PREVIOUSLY-set
 * inline override must be REMOVED when the family goes back to `null` — the
 * `index.css` `@theme` value cannot override a stale inline custom
 * property; same for `--font-mono`). An out-of-range `sizePx` (a
 * hand-edited file) is CLAMPED to [12, 20] (the file is not rewritten).
 */
export function applySettingsFont(font: FontSettings): void {
  const root = document.documentElement;
  const sizePx = Math.min(MAX_FONT_PX, Math.max(MIN_FONT_PX, font.sizePx));
  root.style.setProperty("--ui-font-size", `${sizePx}px`);
  if (font.uiFamily !== null) {
    root.style.setProperty("--font-sans", `${font.uiFamily}, ${SANS_TAIL}`);
  } else {
    root.style.removeProperty("--font-sans");
  }
  if (font.codeFamily !== null) {
    root.style.setProperty("--font-mono", `${font.codeFamily}, ${MONO_TAIL}`);
  } else {
    root.style.removeProperty("--font-mono");
  }
}

/** Apply BOTH (theme + font) — returns the theme cleanup. */
export function applySettingsToDocument(settings: AppSettings): () => void {
  applySettingsFont(settings.font);
  return applySettingsTheme(settings.theme);
}

/**
 * Load the settings + apply them (boot path). A `getSettings` failure (the
 * backend's `get_settings` always succeeds in practice — `load_settings`
 * swallows the write-defaults IO error) → log + the concrete dark theme
 * (the first-frame state stands).
 */
export async function loadAndApplySettings(): Promise<AppSettings | null> {
  try {
    const settings = await getSettings();
    applySettingsToDocument(settings);
    return settings;
  } catch (error) {
    console.error(
      "Failed to load settings; keeping the dark first-frame state",
      error,
    );
    applyThemeToDocument("zai-dark");
    return null;
  }
}
