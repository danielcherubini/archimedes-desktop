import { describe, expect, test, vi, beforeAll } from "vitest";
import { createChatComposerAttachment, readAttachmentAsBase64, releaseAttachment, addImageAttachments, agentSupportsImages } from "./chatAttachments";

describe("chatAttachments", () => {
  beforeAll(() => {
    URL.createObjectURL = vi.fn((file: File) => `blob:mock-${file.name}`);
    URL.revokeObjectURL = vi.fn();
  });

  test("createChatComposerAttachment creates correct structure", () => {
    const file = new File(["x"], "s.png", { type: "image/png" });
    const attachment = createChatComposerAttachment(file);
    expect(attachment.mimeType).toBe("image/png");
    expect(attachment.sizeBytes).toBe(1);
    expect(attachment.objectUrl).toBe("blob:mock-s.png");
  });

  test("createChatComposerAttachment infers mime type", () => {
    const file = new File(["x"], "s.webp", { type: "" });
    const attachment = createChatComposerAttachment(file);
    expect(attachment.mimeType).toBe("image/webp");
  });

  test("readAttachmentAsBase64 reads correctly", async () => {
    const file = new File([new Uint8Array([1, 2, 3])], "a.png", { type: "image/png" });
    const attachment = createChatComposerAttachment(file);
    const base64 = await readAttachmentAsBase64(attachment);
    expect(base64).toBe("AQID");
  });

  test("releaseAttachment calls revokeObjectURL", () => {
    const file = new File(["x"], "s.png", { type: "image/png" });
    const attachment = createChatComposerAttachment(file);
    releaseAttachment(attachment);
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:mock-s.png");
  });

  describe("addImageAttachments", () => {
    test("rejects non-image files", () => {
      const file = new File(["x"], "a.txt", { type: "text/plain" });
      const { attachments, rejected } = addImageAttachments([], [file]);
      expect(attachments.length).toBe(0);
      expect(rejected[0]).toContain("is not a supported image format");
    });

    test("rejects unsupported image types", () => {
      const file = new File(["x"], "a.svg", { type: "image/svg+xml" });
      const { attachments, rejected } = addImageAttachments([], [file]);
      expect(attachments.length).toBe(0);
      expect(rejected[0]).toContain("is not a supported image format");
    });

    test("rejects files over 10 MiB", () => {
      const file = new File([new Uint8Array(11 * 1024 * 1024)], "big.png", { type: "image/png" });
      const { attachments, rejected } = addImageAttachments([], [file]);
      expect(attachments.length).toBe(0);
      expect(rejected[0]).toContain("exceeds the 10 MiB limit");
    });

    test("rejects more than 8 attachments", () => {
      const existing = Array(8).fill(null).map((_, i) => ({ id: `${i}`, file: new File(["x"], "s.png"), filename: "s.png", mimeType: "image/png", sizeBytes: 1, objectUrl: "url" }));
      const file = new File(["x"], "extra.png", { type: "image/png" });
      const { attachments, rejected } = addImageAttachments(existing, [file]);
      expect(attachments.length).toBe(8);
      expect(rejected[0]).toBe("at most 8 images per message");
    });

    test("accepts valid files", () => {
      const file1 = new File(["x"], "a.png", { type: "image/png" });
      const file2 = new File(["y"], "b.jpg", { type: "image/jpeg" });
      const { attachments, rejected } = addImageAttachments([], [file1, file2]);
      expect(attachments.length).toBe(2);
      expect(rejected.length).toBe(0);
    });
  });

  describe("agentSupportsImages", () => {
    test("returns true for correct capability", () => {
      expect(agentSupportsImages({ promptCapabilities: { image: true } })).toBe(true);
    });
    test("returns false for incorrect capability", () => {
      expect(agentSupportsImages({ promptCapabilities: { image: false } })).toBe(false);
    });
    test("returns false for empty", () => {
      expect(agentSupportsImages({})).toBe(false);
      expect(agentSupportsImages(undefined)).toBe(false);
    });
  });
});
