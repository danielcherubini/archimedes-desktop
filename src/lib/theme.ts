export type AppTheme = "zai-light" | "zai-dark";
export type SettingsTheme = "system" | "dark" | "light";

const DARK_QUERY = "(prefers-color-scheme: dark)";

function osPrefersDark(): boolean {
  // jsdom does not implement `window.matchMedia` — default to dark (the
  // app's existing default).
  if (typeof window.matchMedia !== "function") return true;
  return window.matchMedia(DARK_QUERY).matches;
}

/** Resolve a settings theme to a concrete app theme ("system" → the OS scheme). */
export function resolveTheme(theme: SettingsTheme): AppTheme {
  if (theme === "system") return osPrefersDark() ? "zai-dark" : "zai-light";
  return theme === "dark" ? "zai-dark" : "zai-light";
}

/**
 * Apply a settings theme: resolve + `applyThemeToDocument`. Returns a cleanup
 * function. For "system" it subscribes a `matchMedia` "change" listener that
 * re-applies live (the listener is removed by the cleanup). For concrete
 * themes the cleanup is a no-op function.
 */
export function applySettingsTheme(theme: SettingsTheme): () => void {
  applyThemeToDocument(resolveTheme(theme));
  if (theme !== "system" || typeof window.matchMedia !== "function") {
    return () => {};
  }
  const query = window.matchMedia(DARK_QUERY);
  const onChange = () => applyThemeToDocument(resolveTheme("system"));
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

export function applyThemeToDocument(theme: AppTheme): void {
  const root = document.documentElement;
  // Mirrors ZCode's useTheme.ts:66-68 — the `dark` class is toggled ALONGSIDE
  // the theme classes because the ported primitives use `dark:` utilities,
  // which (via the `@custom-variant dark` definition in src/index.css) key off
  // the `.dark` class; without the toggle they would never apply (the app is
  // always Zai Dark).
  root.classList.toggle("dark", theme === "zai-dark");
  root.classList.toggle("theme-zai-light", theme === "zai-light");
  root.classList.toggle("theme-zai-dark", theme === "zai-dark");
}
