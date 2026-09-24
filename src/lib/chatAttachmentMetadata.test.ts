import { describe, expect, test } from "vitest";
import { shouldPreferSpreadsheetClipboardText, inferAttachmentMimeType } from "./chatAttachmentMetadata";

describe("chatAttachmentMetadata", () => {
  describe("shouldPreferSpreadsheetClipboardText", () => {
    test("returns true for text containing tab and empty html", () => {
      expect(shouldPreferSpreadsheetClipboardText("a\tb", "")).toBe(true);
    });

    test("returns true for plain text and Excel HTML", () => {
      expect(shouldPreferSpreadsheetClipboardText("a", "<html><body><table><tr><td>Excel.Sheet</td></tr></table></body></html>")).toBe(true);
    });

    test("returns false for plain text and plain HTML", () => {
      expect(shouldPreferSpreadsheetClipboardText("a", "<html><body><p>hello</p></body></html>")).toBe(false);
    });

    test("returns false for empty text", () => {
      expect(shouldPreferSpreadsheetClipboardText("", "<html></html>")).toBe(false);
    });
  });

  describe("inferAttachmentMimeType", () => {
    test("infers image types correctly", () => {
      expect(inferAttachmentMimeType("x.png")).toBe("image/png");
      expect(inferAttachmentMimeType("y.jpg")).toBe("image/jpeg");
      expect(inferAttachmentMimeType("z.webp")).toBe("image/webp");
    });

    test("infers text/plain correctly", () => {
      expect(inferAttachmentMimeType("a.txt")).toBe("text/plain");
    });

    test("infers application/octet-stream for no extension", () => {
      expect(inferAttachmentMimeType("noext")).toBe("application/octet-stream");
    });
  });
});
