import { describe, it, expect } from "vitest";
import { fileIconFor } from "./fileIcons";
import {
  FileIcon,
  FileCodeIcon,
  FileJsonIcon,
  FileImageIcon,
} from "lucide-react";

/**
 * The file-type DESCRIPTOR colors (ZCode's DESIGN.md: "Keep file-type icons
 * on their own descriptor colors"). They are deliberately NOT the semantic
 * tokens: `--color-brand` is WHITE in zai-dark, so a `.tsx` chip painted with
 * it rendered colorless, and `.rs`/`.py` painted with `--color-destructive`/
 * `--color-success` claimed a file had failed or been added. Each hue is the
 * fill of the Material Icon Theme SVG ZCode renders for that type.
 */
describe("fileIconFor", () => {
  it("maps .ts / .tsx to the code icon + the typescript blue", () => {
    expect(fileIconFor("a.ts")).toEqual({
      icon: FileCodeIcon,
      className: "text-file-ts",
    });
    expect(fileIconFor("a.tsx")).toEqual({
      icon: FileCodeIcon,
      className: "text-file-ts",
    });
  });
  it("maps .js to the javascript amber and .jsx to the react cyan (ZCode's own icon map)", () => {
    expect(fileIconFor("a.js").className).toBe("text-file-js");
    expect(fileIconFor("a.jsx").className).toBe("text-file-react");
  });
  it("maps .json to the JSON icon + its own amber", () => {
    expect(fileIconFor("a.json")).toEqual({
      icon: FileJsonIcon,
      className: "text-file-json",
    });
  });
  it("maps .html to the code icon + the html orange", () => {
    expect(fileIconFor("a.html")).toEqual({
      icon: FileCodeIcon,
      className: "text-file-html",
    });
  });
  it("maps .css / .scss to their own purples (NOT the muted grey)", () => {
    expect(fileIconFor("a.css").className).toBe("text-file-css");
    expect(fileIconFor("a.scss").className).toBe("text-file-sass");
  });
  it("maps .md to the markdown blue", () => {
    expect(fileIconFor("README.md").className).toBe("text-file-md");
  });
  it("maps .rs to the rust orange (NOT the destructive red)", () => {
    expect(fileIconFor("main.rs").className).toBe("text-file-rs");
  });
  it("maps .py to the python blue", () => {
    expect(fileIconFor("x.py").className).toBe("text-file-py");
  });
  it("maps shell scripts to the terminal amber", () => {
    expect(fileIconFor("build.sh").className).toBe("text-file-sh");
    expect(fileIconFor("setup.bash").className).toBe("text-file-sh");
  });
  it("maps .yaml / .toml / .lock to their own hues", () => {
    expect(fileIconFor("ci.yaml").className).toBe("text-file-yaml");
    expect(fileIconFor("Cargo.toml").className).toBe("text-file-toml");
    expect(fileIconFor("pnpm-lock.yaml").className).toBe("text-file-yaml");
  });
  it("maps images to the image teal and .svg to its own amber", () => {
    expect(fileIconFor("a.png")).toEqual({
      icon: FileImageIcon,
      className: "text-file-image",
    });
    expect(fileIconFor("logo.svg").className).toBe("text-file-svg");
  });
  it("maps the web/system types ZCode ships descriptors for", () => {
    expect(fileIconFor("main.go").className).toBe("text-file-go");
    expect(fileIconFor("App.vue").className).toBe("text-file-vue");
    expect(fileIconFor("Page.svelte").className).toBe("text-file-svelte");
    expect(fileIconFor("Main.java").className).toBe("text-file-java");
    expect(fileIconFor("api.php").className).toBe("text-file-php");
    expect(fileIconFor("feed.xml").className).toBe("text-file-xml");
    expect(fileIconFor("schema.graphql").className).toBe("text-file-graphql");
    expect(fileIconFor("bundle.zip").className).toBe("text-file-archive");
  });
  it("falls back to the neutral default for unmapped extensions", () => {
    expect(fileIconFor("a.unknownext")).toEqual({
      icon: FileIcon,
      className: "text-foreground-subtlest",
    });
  });
  it("is case-insensitive on the extension", () => {
    expect(fileIconFor("a.TS").className).toBe("text-file-ts");
  });
  it("falls back to the default for dotfiles", () => {
    expect(fileIconFor(".gitignore").className).toBe("text-foreground-subtlest");
  });
  it("uses the LAST extension", () => {
    expect(fileIconFor("a.tar.gz").className).toBe("text-foreground-subtlest");
  });
});
