import { describe, it, expect } from "vitest";
import { fileIconFor } from "./fileIcons";
import {
  FileIcon,
  FileCodeIcon,
  FileJsonIcon,
  FileImageIcon,
} from "lucide-react";

describe("fileIconFor", () => {
  it("maps .ts to the code icon + brand color", () => {
    expect(fileIconFor("a.ts")).toEqual({
      icon: FileCodeIcon,
      className: "text-brand",
    });
  });
  it("maps .js to the code icon + warning color", () => {
    expect(fileIconFor("a.js")).toEqual({
      icon: FileCodeIcon,
      className: "text-warning",
    });
  });
  it("maps .json to the JSON icon", () => {
    expect(fileIconFor("a.json")).toEqual({
      icon: FileJsonIcon,
      className: "text-warning",
    });
  });
  it("maps .html to the code icon + destructive color", () => {
    expect(fileIconFor("a.html")).toEqual({
      icon: FileCodeIcon,
      className: "text-destructive",
    });
  });
  it("maps .png to the image icon", () => {
    expect(fileIconFor("a.png")).toEqual({
      icon: FileImageIcon,
      className: "text-foreground-subtle",
    });
  });
  it("falls back to the default for unmapped extensions", () => {
    expect(fileIconFor("a.unknownext")).toEqual({
      icon: FileIcon,
      className: "text-foreground-subtlest",
    });
  });
  it("falls back to the default for .txt (unmapped)", () => {
    expect(fileIconFor("a.txt")).toEqual({
      icon: FileIcon,
      className: "text-foreground-subtlest",
    });
  });
  it("is case-insensitive on the extension", () => {
    expect(fileIconFor("a.TS")).toEqual({
      icon: FileCodeIcon,
      className: "text-brand",
    });
  });
  it("falls back to the default for dotfiles", () => {
    expect(fileIconFor(".gitignore")).toEqual({
      icon: FileIcon,
      className: "text-foreground-subtlest",
    });
  });
  it("uses the LAST extension", () => {
    expect(fileIconFor("a.tar.gz")).toEqual({
      icon: FileIcon,
      className: "text-foreground-subtlest",
    });
  });
});
