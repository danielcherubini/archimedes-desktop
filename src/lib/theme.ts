export type AppTheme = "zai-light" | "zai-dark";
export type SettingsTheme = "system" | "dark" | "light";
/**
 * (ADR 0027) The color scheme — a second axis, ORTHOGONAL to `AppTheme` (the
 * light/dark mode). `dracula` has no light reading, so it PINS the app dark
 * and the mode is ignored (`system` included).
 */
export type AppPalette = "zai" | "dracula";

const DARK_QUERY = "(prefers-color-scheme: dark)";

function osPrefersDark(): boolean {
  // jsdom does not implement `window.matchMedia` — default to dark (the
  // app's existing default).
  if (typeof window.matchMedia !== "function") return true;
  return window.matchMedia(DARK_QUERY).matches;
}

/**
 * Resolve a settings theme to a concrete app theme ("system" → the OS scheme).
 * (ADR 0027) `palette: "dracula"` is answered FIRST — it pins dark, so the
 * mode (and the OS scheme behind `"system"`) is never consulted.
 */
export function resolveTheme(
  theme: SettingsTheme,
  palette?: AppPalette,
): AppTheme {
  if (palette === "dracula") return "zai-dark";
  if (theme === "system") return osPrefersDark() ? "zai-dark" : "zai-light";
  return theme === "dark" ? "zai-dark" : "zai-light";
}

/**
 * Apply a settings theme + palette: resolve + `applyThemeToDocument`. Returns
 * a cleanup function. For "system" it subscribes a `matchMedia` "change"
 * listener that re-applies live (the listener is removed by the cleanup). For
 * concrete themes the cleanup is a no-op function. The `palette` is threaded
 * through the listener so an OS flip under zai keeps working, and under
 * dracula keeps resolving dark.
 */
export function applySettingsTheme(
  theme: SettingsTheme,
  palette?: AppPalette,
): () => void {
  applyThemeToDocument(resolveTheme(theme, palette), palette);
  if (theme !== "system" || typeof window.matchMedia !== "function") {
    return () => {};
  }
  const query = window.matchMedia(DARK_QUERY);
  const onChange = () =>
    applyThemeToDocument(resolveTheme("system", palette), palette);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

/**
 * Apply the resolved app theme + the palette to `<html>`. The palette is
 * optional (callers on the zai axis pass only the theme, unchanged).
 *
 * (ADR 0027) At most ONE `.theme-*` class may ever be present: every palette
 * block is a single-class selector of equal specificity, so two co-existing
 * would be resolved by SOURCE ORDER in `index.css` — silently, and it would
 * flip on any reorder. Hence all four classes are `toggle`d (never `add`ed),
 * each with an explicit boolean.
 */
export function applyThemeToDocument(
  theme: AppTheme,
  palette?: AppPalette,
): void {
  const root = document.documentElement;
  const dracula = palette === "dracula";
  // Dracula is dark-only: it wins over the resolved mode, so a light (or
  // OS-light `system`) reading still lands on `.dark`.
  const dark = dracula || theme === "zai-dark";
  // Mirrors ZCode's useTheme.ts:66-68 — the `dark` class is toggled ALONGSIDE
  // the theme classes because the ported primitives use `dark:` utilities,
  // which (via the `@custom-variant dark` definition in src/index.css) key off
  // the `.dark` class; without the toggle they would never apply.
  root.classList.toggle("dark", dark);
  root.classList.toggle("theme-zai-light", !dracula && theme === "zai-light");
  root.classList.toggle("theme-zai-dark", !dracula && theme === "zai-dark");
  root.classList.toggle("theme-dracula", dracula);
}
