import { describe, expect, it, beforeEach } from "vitest";
import { applyThemeToDocument } from "./theme";

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
