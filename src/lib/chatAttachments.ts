import { inferAttachmentMimeType } from "./chatAttachmentMetadata";

export const MAX_CHAT_ATTACHMENTS = 8;
/**
 * Deliberately LOWER than ZCode's 20 MiB: the desktop persists attachments
 * inline (base64) in SQLite (ADR 0008), so the cap bounds DB growth.
 */
export const INLINE_IMAGE_ATTACHMENT_MAX_BYTES = 10 * 1024 * 1024;
/**
 * Deliberately NARROWER than `image/*` (ZCode accepts all image types — it
 * uploads to its own backend): the destination is LLM vision APIs, which
 * only accept png/jpeg/gif/webp. An SVG would pass `image/*` and fail the
 * whole turn at the provider, so the allowlist is enforced here AND in the
 * Rust `build_prompt_blocks` validation.
 */
export const SUPPORTED_IMAGE_TYPES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
];

/** A persisted / in-memory image (one `images` entry of a user message). */
export interface ImageRef {
  name: string;
  mimeType: string;
  sizeBytes: number;
  /** base64, WITHOUT a `data:` prefix. */
  data: string;
}

/** An image staged in the composer (CONTEXT.md: Attachment). */
export interface ChatComposerAttachment {
  id: string;
  file: File;
  filename: string;
  mimeType: string;
  sizeBytes: number;
  objectUrl: string;
}

function normalizeComposerMimeType(mimeType: string): string {
  return mimeType.split(";", 1)[0]?.trim().toLowerCase() ?? "";
}

/**
 * Ported from ZCode's `createChatComposerAttachment` (images-only: no `localPath`,
 * no text-file fields). `id` uses `crypto.randomUUID()` instead of `nanoid`
 * (no new dependency).
 */
export function createChatComposerAttachment(file: File): ChatComposerAttachment {
  const mimeType = normalizeComposerMimeType(file.type || inferAttachmentMimeType(file.name));
  return {
    id: crypto.randomUUID(),
    file,
    filename: file.name,
    mimeType,
    objectUrl: URL.createObjectURL(file),
    sizeBytes: file.size,
  };
}

/** Release the object URL (call on remove / clear / unmount — prevents leaks). */
export function releaseAttachment(attachment: ChatComposerAttachment): void {
  URL.revokeObjectURL(attachment.objectUrl);
}

/**
 * Read the attachment as base64 (no `data:` prefix). Ported from ZCode's
 * `readAttachmentBase64` (Chinese error strings → English). Call ONLY at send
 * time — the UI preview uses the object URL (ZCode's pattern).
 */
export async function readAttachmentAsBase64(
  attachment: ChatComposerAttachment,
): Promise<string> {
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () =>
      typeof reader.result === "string"
        ? resolve(reader.result)
        : reject(new Error("failed to read attachment"));
    reader.onerror = () => reject(reader.error ?? new Error("failed to read attachment"));
    reader.readAsDataURL(attachment.file);
  });
  const commaIndex = dataUrl.indexOf(",");
  if (commaIndex === -1) throw new Error("malformed data URL");
  return dataUrl.slice(commaIndex + 1);
}

/**
 * Apply the caps (PURE — a later task wires the state):
 * - only `SUPPORTED_IMAGE_TYPES` files (an empty `type` is inferred from the
 *   extension; `image/*` types outside the allowlist — e.g. `image/svg+xml` —
 *   are REJECTED, see the constant's comment);
 * - at most `MAX_CHAT_ATTACHMENTS` staged in total;
 * - each file ≤ `INLINE_IMAGE_ATTACHMENT_MAX_BYTES`.
 * Returns the new attachment list plus human-readable rejection reasons
 * (the composer shows them in its error line).
 */
export function addImageAttachments(
  existing: ChatComposerAttachment[],
  files: File[],
): { attachments: ChatComposerAttachment[]; rejected: string[] } {
  const rejected: string[] = [];
  let attachments = existing;
  for (const file of files) {
    const mimeType = normalizeComposerMimeType(file.type || inferAttachmentMimeType(file.name));
    if (!SUPPORTED_IMAGE_TYPES.includes(mimeType)) {
      rejected.push(
        `"${file.name || "pasted file"}" is not a supported image format (png, jpeg, gif, webp)`,
      );
      continue;
    }
    if (file.size > INLINE_IMAGE_ATTACHMENT_MAX_BYTES) {
      rejected.push(`"${file.name || "pasted image"}" exceeds the 10 MiB limit`);
      continue;
    }
    if (attachments.length >= MAX_CHAT_ATTACHMENTS) {
      rejected.push(`at most ${MAX_CHAT_ATTACHMENTS} images per message`);
      break;
    }
    attachments = [...attachments, createChatComposerAttachment(file)];
  }
  return { attachments, rejected };
}

/**
 * Capability gate (FAIL-CLOSED): true only when the agent advertises
 * `promptCapabilities.image === true` in its initialize response.
 * `capabilities` is the raw camelCase wire record from `SessionInfo`
 * (`src/lib/tauri.ts`) — a missing field or missing `promptCapabilities`
 * means "not supported" (same posture as the bridge's macOS fail-closed).
 */
export function agentSupportsImages(
  capabilities: Record<string, unknown> | undefined,
): boolean {
  const prompt = capabilities?.promptCapabilities as Record<string, unknown> | undefined;
  return prompt?.image === true;
}
