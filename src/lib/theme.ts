export type AppTheme = "zai-light" | "zai-dark";
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
