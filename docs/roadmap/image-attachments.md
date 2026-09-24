---
status: committed
done-when: Pasting or dropping an image into the composer stages a removable thumbnail; sending it delivers ACP image content blocks to the agent (pi normalizes and forwards them to the model); the image renders in the user's transcript bubble after app restart.
---

# Image Attachments Plan

**Goal:** Let the user stage images (clipboard paste + drag-and-drop) in the composer, send them with the prompt as ACP `ImageContent` blocks, and see them in the persisted transcript.
**Architecture:** The webview reads pasted/dropped `File` blobs to base64 at send time and passes them over Tauri IPC; the Rust side validates them, appends `ContentBlock::Image` blocks to the ACP `session/prompt` request, and persists the images inline in `messages.payload_json` (ADR 0008). The frontend gates the whole feature on the agent's `promptCapabilities.image` (fail-closed). The UI port follows ZCode's attachment pipeline (ADR 0006/0007 precedent: faithful port, adapt only imports/i18n).
**Tech Stack:** React 19 + TypeScript (Vitest + @testing-library/react), Tauri 2 IPC, Rust `agent-client-protocol` 2.1.0 (schema v1), SQLite (rusqlite).

## Conventions (apply to every task)

- **TDD (AGENTS.md):** write the failing test first, run it and confirm it fails, then implement, then confirm it passes. Never skip the "confirm it fails" step.
- **Validation gates (AGENTS.md):** frontend — `pnpm test` + `pnpm build` (repo root); Rust — `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (run from `src-tauri/`). A task is done only when all of its gates are green.
- **Port rule (ADR 0006/0007):** code ported from ZCode (`/home/daniel/Coding/AI/ZCode/packages/ui/src/`) is ported VERBATIM — adapt only imports, i18n (ZCode's Chinese strings → literal English), and the deliberate deviations called out per task. Do not re-implement ported logic from scratch.
- **Tokens (ADR 0006):** all UI colors via the semantic token classes already in `src/index.css` (`bg-input`, `border-input-border`, `text-foreground`, `text-foreground-subtle`, `bg-primary`, `text-primary-foreground`, `text-destructive`, `text-ui-*` scale). Never bare `neutral-*`/`sky-*` values.
- **Do NOT touch:** `src-tauri/src/acp/bridge.rs`, `permission.rs`, `subagent.rs`, `worker_runtime.rs`, the ACP wire protocol, the agent side (`pi`/`pi-acp` — verified fully image-capable, zero changes needed), `docs/decisions/` (append-only).

---

### Task 1: Attachment lib modules (`chatAttachments` + `chatAttachmentMetadata`)

**Context:**
This task creates the two new frontend lib modules that hold ALL attachment domain logic (model, caps, object-URL lifecycle, base64 reading, the spreadsheet-clipboard heuristic, the capability gate). It is pure logic with no component wiring — later tasks consume it. It ports ZCode's `packages/ui/src/lib/chatAttachments.ts` and `packages/ui/src/lib/chatAttachmentMetadata.ts` (read those files first), adapted to images-only and to the desktop's dependency set (no `nanoid`, no `@zcode/shared`).

**Files:**
- Create: `src/lib/chatAttachments.ts`
- Create: `src/lib/chatAttachmentMetadata.ts`
- Test: `src/lib/chatAttachments.test.ts`
- Test: `src/lib/chatAttachmentMetadata.test.ts`

**What to implement:**

`src/lib/chatAttachmentMetadata.ts` — two functions ported VERBATIM from `ZCode/packages/ui/src/lib/chatAttachmentMetadata.ts` (translate the Chinese comments to English, keep the regex and logic byte-identical):
- `shouldPreferSpreadsheetClipboardText(text: string, html: string): boolean` — with the `SPREADSHEET_CLIPBOARD_HTML_PATTERN` constant above it.
- `inferAttachmentMimeType(filename: string): string` — the full extension map (all entries, not just image ones — port verbatim).

`src/lib/chatAttachments.ts`:
```ts
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
```

**Steps:**
- [ ] Write `src/lib/chatAttachmentMetadata.test.ts`: `shouldPreferSpreadsheetClipboardText` — (a) text containing `\t` + empty html → `true`; (b) plain text + Excel HTML (`<html>...Excel.Sheet...</html>`) → `true`; (c) plain text + plain HTML → `false`; (d) empty text → `false`. `inferAttachmentMimeType` — (a) `"x.png"` → `"image/png"`, `"y.jpg"` → `"image/jpeg"`, `"z.webp"` → `"image/webp"`; (b) `"a.txt"` → `"text/plain"`; (c) `"noext"` → `"application/octet-stream"`.
- [ ] Run `pnpm test`
  - Did the new tests FAIL (module does not exist)? If they passed unexpectedly, stop and investigate why.
- [ ] Write `src/lib/chatAttachments.test.ts`. In `beforeAll` stub the object-URL APIs (Node's global `URL` leaks into jsdom but only accepts Node `Blob`s and throws on jsdom `File`s — the stub is required either way): `URL.createObjectURL = vi.fn((file: File) => \`blob:mock-\${file.name}\`); URL.revokeObjectURL = vi.fn();` Tests: (a) `createChatComposerAttachment(new File(["x"], "s.png", { type: "image/png" }))` → `mimeType === "image/png"`, `sizeBytes === 1`, `objectUrl` is the stub value; (b) empty `file.type` + name `"s.webp"` → `mimeType === "image/webp"` (inference); (c) `readAttachmentAsBase64` on `new File([new Uint8Array([1, 2, 3])], "a.png", { type: "image/png" })` resolves to `"AQID"` (the base64 of those bytes, WITHOUT a `data:` prefix — jsdom's FileReader is real); (d) `releaseAttachment` → `URL.revokeObjectURL` called with the objectUrl; (e) `addImageAttachments` — non-image file (`a.txt`) → rejected with `"is not a supported image format"`, no attachment; **`image/svg+xml` file → rejected with the same reason (allowlist)**; file > 10 MiB (`new File([new Uint8Array(11 * 1024 * 1024)], "big.png", { type: "image/png" })`) → rejected with `"exceeds the 10 MiB limit"`; 8 existing + 1 new → rejected with `"at most 8 images per message"`; 2 valid files → 2 attachments, no rejections; (f) `agentSupportsImages` — `{ promptCapabilities: { image: true } }` → `true`; `{ promptCapabilities: { image: false } }` → `false`; `{}` → `false`; `undefined` → `false`.
- [ ] Run `pnpm test`
  - Did the new tests FAIL? (Some will — the module doesn't exist yet.)
- [ ] Implement `src/lib/chatAttachmentMetadata.ts` and `src/lib/chatAttachments.ts` exactly as specified above.
- [ ] Run `pnpm test`
  - Did ALL tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat: add image-attachment lib modules (ported from ZCode)"

**Acceptance criteria:**
- [ ] Both modules exist with the exact exports above; `pnpm test` + `pnpm build` green; all new tests pass.
- [ ] `addImageAttachments` is pure (no React, no side effects beyond `createChatComposerAttachment`'s object-URL creation).
- [ ] `agentSupportsImages` is fail-closed (missing/unknown → `false`).

---

### Task 2: Store — user messages with images

**Context:**
The transcript data model must carry images on user messages so a sent image renders in the bubble AND survives reload (hydration from SQLite). The `messages` table stores `payload_json TEXT` — no schema change (ADR 0008). This task changes ONLY the store; the components come in Tasks 4–5.

**Files:**
- Modify: `src/store/sessions.ts`
- Test: `src/store/sessions.test.ts`

**What to implement:**

In `src/store/sessions.ts`:
1. Import the type: `import type { ImageRef } from "../lib/chatAttachments";`
2. `Message` union (line ~37): the user variant becomes
   `| { kind: "user"; text: string; at: number; images?: ImageRef[] }`
3. Store interface (line ~470): `addUserMessage: (sessionId: string, text: string, images?: ImageRef[]) => void;`
4. `addUserMessage` implementation (line ~624):
   ```ts
   addUserMessage: (sessionId, text, images) =>
     set((state) => ({
       messages: {
         ...state.messages,
         [sessionId]: [
           ...(state.messages[sessionId] ?? []),
           { kind: "user" as const, text, at: Date.now(), ...(images ? { images } : {}) },
         ],
       },
     })),
   ```
   (The `images` key is OMITTED from the in-memory message when absent — same rule as the payload.)
5. `rowToMessages` user branch (line ~228):
   ```ts
   case "user": {
     if (typeof payload.text !== "string") return [];
     const rawImages = Array.isArray(payload.images) ? payload.images : [];
     const images = rawImages
       .filter(
         (img): img is Record<string, unknown> =>
           img !== null && typeof img === "object" &&
           typeof (img as Record<string, unknown>).data === "string" &&
           typeof (img as Record<string, unknown>).mimeType === "string",
       )
       .map((img) => ({
         name: typeof img.name === "string" ? img.name : "",
         mimeType: img.mimeType as string,
         sizeBytes: typeof img.sizeBytes === "number" ? img.sizeBytes : 0,
         data: img.data as string,
       }));
     return [
       {
         kind: "user",
         text: payload.text,
         at: row.createdAt,
         ...(images.length > 0 ? { images } : {}),
       },
     ];
   }
   ```
   (Defensive: a malformed `images` entry is dropped, never crashes hydration.)

**Steps:**
- [ ] In `src/store/sessions.test.ts` add tests: (a) `addUserMessage("s1", "hi", [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }])` → the stored message is `{ kind: "user", text: "hi", images: [...] }` (assert the `images` array matches); (b) `addUserMessage("s1", "hi")` (no images) → the stored message has NO `images` key (`expect("images" in msg).toBe(false)`); (c) `rowToMessages` on a user row whose `payloadJson` is `JSON.stringify({ text: "old", images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "QUJD" }] })` → a user message WITH `images` (round-trip); (d) `rowToMessages` on a user row with `payloadJson` `JSON.stringify({ text: "old" })` (pre-feature row) → a user message with NO `images` key; (e) `rowToMessages` on a user row with a malformed images entry (`JSON.stringify({ text: "x", images: [{ data: 42 }] })`) → a user message with NO `images` key (malformed entries dropped).
- [ ] Run `pnpm test`
  - Did the new tests FAIL? (b) and (d) may pass already — (a), (c), (e) must fail. If a must-fail test passed, stop and investigate.
- [ ] Implement the five changes above.
- [ ] Run `pnpm test`
  - Did ALL tests pass (including the pre-existing ones)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: carry image attachments on user messages in the sessions store"

**Acceptance criteria:**
- [ ] `Message`'s user variant carries optional `images: ImageRef[]`; `addUserMessage` accepts optional images; hydration round-trips images and tolerates missing/malformed ones.
- [ ] All pre-existing `sessions.test.ts` tests still pass (no behavior change for image-less messages).

---

### Task 3: Send path — `sendPrompt` IPC + Rust `send_prompt` (both paths)

**Context:**
The prompt send path is text-only today. This task extends it end-to-end: the frontend `sendPrompt` wrapper gains an optional `images` argument; the Rust `send_prompt` Tauri command and `SessionManager::send_prompt_with_images` (the in-process path — the command does NOT go through it, see the comment at `commands/sessions.rs:60`) both **validate the images FIRST — before `begin_user_turn` and `record_message`** (a rejected payload must not be persisted and must not start a user turn), then append `ContentBlock::Image` blocks (text first, then images — matches pi's own ordering), and persist `{ "text", "images": [...] }` in `messages.payload_json` (the `images` key omitted when empty — ADR 0008). Validation is a guard against hand-rolled IPC: `mime_type` must be in the `SUPPORTED_IMAGE_TYPES` allowlist, and the DECODED size (padding-stripped base64 length × 3/4 — an upper bound; do not trust `sizeBytes`) must be ≤ 10 MiB.

**Files:**
- Create: `src-tauri/src/acp/prompt.rs`
- Modify: `src-tauri/src/acp/mod.rs`
- Modify: `src-tauri/src/acp/errors.rs`
- Modify: `src-tauri/src/acp/session.rs`
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src/lib/tauri.ts`
- Test: (inside `src-tauri/src/acp/prompt.rs`'s `#[cfg(test)]` module)

**What to implement:**

`src-tauri/src/acp/errors.rs` — add ONE variant to `AcpError` (no existing match sites are exhaustive — verified):
```rust
/// A user prompt payload failed validation (invalid image mime type,
/// oversized image).
#[error("invalid prompt payload: {message}")]
InvalidPrompt { message: String },
```

`src-tauri/src/acp/prompt.rs` (new module — all logic is PURE and unit-testable without an agent):
```rust
//! Pure helpers for building prompt requests and persisting user messages.
//! Shared by the `send_prompt` Tauri command and `SessionManager::send_prompt`
//! (the two paths are separate — see `commands/sessions.rs`).

use agent_client_protocol::schema::v1::{ContentBlock, ImageContent, TextContent};

use crate::acp::AcpError;

/// One image attachment over IPC. The frontend sends CAMEL CASE
/// (`mimeType`, `sizeBytes`) — Tauri camel-cases only the TOP-LEVEL command
/// args, so this nested struct renames explicitly.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePayload {
    pub mime_type: String,
    /// base64, WITHOUT a `data:` prefix.
    pub data: String,
    pub name: String,
    pub size_bytes: u64,
}

/// 10 MiB — mirrors the frontend cap (ADR 0008).
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

/// Deliberately NARROWER than `image/*`: the destination is LLM vision APIs
/// (png/jpeg/gif/webp only — an SVG would fail the whole turn at the
/// provider). Mirrors the frontend `SUPPORTED_IMAGE_TYPES`.
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Build the prompt's content blocks: the text block FIRST, then one image
/// block per attachment (matches pi's own user-content ordering).
///
/// Validation (guards against hand-rolled IPC):
/// - `mime_type` must be in `SUPPORTED_IMAGE_TYPES`;
/// - decoded size must be ≤ `MAX_IMAGE_BYTES`. The estimate strips trailing
///   `=` padding first, then takes `len * 3 / 4` (an upper bound; `size_bytes`
///   is NOT trusted). Stripping the padding keeps the bound consistent with
///   the frontend's `file.size <= 10 MiB` cap (no off-by-2 rejections).
pub fn build_prompt_blocks(
    text: &str,
    images: &[ImagePayload],
) -> Result<Vec<ContentBlock>, AcpError> {
    let mut blocks = vec![ContentBlock::Text(TextContent::new(text))];
    for img in images {
        if !SUPPORTED_IMAGE_TYPES.contains(&img.mime_type.as_str()) {
            return Err(AcpError::InvalidPrompt {
                message: format!("unsupported image type: {} (expected png, jpeg, gif, webp)", img.mime_type),
            });
        }
        let trimmed = img.data.trim_end_matches('=');
        let decoded_bytes = (trimmed.len() as u64) * 3 / 4;
        if decoded_bytes > MAX_IMAGE_BYTES {
            return Err(AcpError::InvalidPrompt {
                message: "image exceeds the 10 MiB limit".to_string(),
            });
        }
        blocks.push(ContentBlock::Image(ImageContent::new(
            img.data.clone(),
            img.mime_type.clone(),
        )));
    }
    Ok(blocks)
}

/// The user-message payload persisted in `messages.payload_json` (ADR 0008):
/// `{ "text": ..., "images": [{ name, mimeType, sizeBytes, data }] }` —
/// the `images` key is OMITTED when empty (pre-feature rows stay `{"text"}`).
pub fn user_message_payload(text: &str, images: &[ImagePayload]) -> serde_json::Value {
    if images.is_empty() {
        serde_json::json!({ "text": text })
    } else {
        serde_json::json!({
            "text": text,
            "images": images.iter().map(|img| {
                serde_json::json!({
                    "name": img.name,
                    "mimeType": img.mime_type,
                    "sizeBytes": img.size_bytes,
                    "data": img.data,
                })
            }).collect::<Vec<_>>(),
        })
    }
}
```

`src-tauri/src/acp/mod.rs` — add `pub mod prompt;` to the module list and `pub use prompt::ImagePayload;` to the re-exports.

`src-tauri/src/commands/sessions.rs` — `send_prompt` (line ~50):
- Add the arg `images: Option<Vec<ImagePayload>>,` (import `use crate::acp::prompt::{self, ImagePayload};`). Tauri maps a MISSING key to `None` (`deserialize_option`); the frontend always sends `images` (possibly `[]`) so it arrives as `Some`. **Do NOT use `#[serde(default)]` on the parameter — the `#[tauri::command]` macro re-emits the function verbatim and `#[serde]` is an unknown attribute there (compile error).**
- **VALIDATION FIRST — order matters:** the current body is `connection` → `begin_user_turn` → `record_message(payload)` → `PromptRequest` → `send_request`. The new body is `connection` → `let images = images.unwrap_or_default();` → **`let blocks = prompt::build_prompt_blocks(&text, &images)?;`** → `begin_user_turn` → `let payload = prompt::user_message_payload(&text, &images);` → `db.record_message(...)` → `PromptRequest::new(..., blocks)` → `send_request`. (A rejected payload — SVG / oversized from hand-rolled IPC — must NOT be written to SQLite and must NOT start a user turn; validating before persisting is what keeps the "cap bounds DB growth" guarantee real.)
- Remove imports that are now unused (`ContentBlock`, `TextContent` — keep `PromptRequest`); `cargo clippy` will flag anything missed.

`src-tauri/src/acp/session.rs` — **do NOT change the `send_prompt` signature** (it has no production callers — only the ~16 integration-test call sites in `tests/acp_flow.rs`, `tests/subagent_dispatch.rs`, `tests/subagent_concurrency.rs` — and changing it would break the `cargo test` build). Instead, turn the existing method into a thin wrapper and move the body into a new method:
```rust
/// Send a prompt to a live session and wait for the turn to finish.
///
/// Returns the [`StopReason`] the agent reported (the frontend needs the
/// turn-completion signal).
pub async fn send_prompt(
    &self,
    session_id: &str,
    text: String,
) -> Result<StopReason, AcpError> {
    self.send_prompt_with_images(session_id, text, Vec::new()).await
}

/// Like `send_prompt`, but with image attachments: validates them
/// (`prompt::build_prompt_blocks`), appends `ContentBlock::Image` blocks
/// (text first), and persists `{ "text", "images": [...] }` in the transcript
/// (the `images` key omitted when empty — ADR 0008).
pub async fn send_prompt_with_images(
    &self,
    session_id: &str,
    text: String,
    images: Vec<ImagePayload>,
) -> Result<StopReason, AcpError> {
    // ... the EXISTING `send_prompt` body, with the two replacements below ...
}
```
In the moved body — **same validation-first order as the command**:
- `let blocks = prompt::build_prompt_blocks(&text, &images)?;` goes BEFORE `self.begin_user_turn(session_id).await;` (after the `cx` lookup).
- Replace `let payload = serde_json::json!({ "text": text });` with `let payload = prompt::user_message_payload(&text, &images);` (inside the existing `if let Some(db)` block, after `begin_user_turn`).
- Replace `let request = PromptRequest::new(sid, vec![ContentBlock::Text(TextContent::new(text))]);` with `let request = PromptRequest::new(sid, blocks);`
- **Import fix (clippy 0-warnings gate):** `TextContent` is no longer used by non-test code in `session.rs` after the replacement — REMOVE it from the top-level `use agent_client_protocol::schema::v1::{...}` (line ~35) and ADD it to `session_tests`'s own `use agent_client_protocol::schema::v1::{...}` (line ~1522 — the tests at line ~1551 still use it via `super::*`). `ContentBlock` STAYS top-level (still used at lines ~465, ~1244, ~1273). Add `ImagePayload` to the top-level import (`use crate::acp::ImagePayload;` or via the `super` re-export).

`src/lib/tauri.ts` — `sendPrompt` (line ~323):
```ts
import type { ImageRef } from "./chatAttachments";

export async function sendPrompt(
  sessionId: string,
  text: string,
  images?: ImageRef[],
): Promise<StopReason> {
  return invoke<StopReason>("send_prompt", { sessionId, text, images: images ?? [] });
}
```
(Always send `images` — `[]` when absent; the Rust `Option` + `unwrap_or_default` is belt-and-braces for older frontends.)

`src-tauri/src/acp/prompt.rs` — `#[cfg(test)] mod tests` (the TDD core of this task):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn img(mime: &str, data_len: usize, name: &str) -> ImagePayload {
        ImagePayload {
            mime_type: mime.to_string(),
            data: "A".repeat(data_len),
            name: name.to_string(),
            size_bytes: (data_len as u64) * 3 / 4,
        }
    }

    #[test]
    fn zero_images_is_a_single_text_block() { ... }
    #[test]
    fn text_block_first_then_images_in_order() { ... }
    #[test]
    fn non_image_mime_is_rejected() { ... }
    #[test]
    fn svg_is_rejected() { ... }
    #[test]
    fn oversized_image_is_rejected() { ... }
    #[test]
    fn payload_omits_images_key_when_empty() { ... }
    #[test]
    fn payload_includes_images_with_camel_case_keys() { ... }
    #[test]
    fn db_round_trip_preserves_images() { ... }
}
```
(`AcpError` has NO `PartialEq` — check error cases with `assert!(matches!(result, Err(AcpError::InvalidPrompt { .. })))`, never `assert_eq!`.)
Test bodies:
- `zero_images_is_a_single_text_block`: `build_prompt_blocks("hi", &[])` → `Ok` with exactly 1 block, `ContentBlock::Text` with text `"hi"`.
- `text_block_first_then_images_in_order`: 2 images (`image/png` data `"AAA"`, `image/jpeg` data `"BBB"`) → 3 blocks: `[Text("hi"), Image{mime image/png, data "AAA"}, Image{mime image/jpeg, data "BBB"}]` (assert order + fields). `ContentBlock` is `#[non_exhaustive]` — do NOT `match` on it; destructure with `let ContentBlock::Image(i) = &blocks[1] else { panic!("expected an image block"); };` (and `let ContentBlock::Text(t) = &blocks[0] else { panic!(...); };`).
- `non_image_mime_is_rejected`: `img("text/plain", 10, "x")` → `assert!(matches!(build_prompt_blocks("x", &[im]), Err(AcpError::InvalidPrompt { .. })))`.
- `svg_is_rejected`: `img("image/svg+xml", 10, "x.svg")` → `Err(InvalidPrompt)` (the allowlist, not a prefix check).
- `oversized_image_is_rejected`: `img("image/png", 14_000_000, "big")` (decoded 10.5 MiB > cap) → `Err(InvalidPrompt)`. (With the padding-strip bound, a file the frontend accepts at exactly 10 MiB must NOT be rejected — the `=`-stripped estimate is what makes that hold.)
- `payload_omits_images_key_when_empty`: `user_message_payload("hi", &[])` → `json!({"text": "hi"})` exactly (`assert!(v.get("images").is_none())` — do NOT write `assert!(!v.get("images").is_some())`, clippy's `nonminimal_bool` fires on it and breaks the 0-warnings gate).
- `payload_includes_images_with_camel_case_keys`: 1 image → `images[0]` has keys `name`, `mimeType`, `sizeBytes`, `data` with the right values.
- `db_round_trip_preserves_images`: `temp_config_dir` is NOT importable from `prompt.rs` (it lives inside `session.rs`'s private test module) — inline the 3-line helper in `prompt.rs`'s test module: `let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()); std::fs::create_dir(&dir).unwrap();` (`uuid` is already a dependency). Then follow the EXISTING fixture at `session.rs:~1535` (copy its field list verbatim — `SessionInfo` has NO `created_at` field and REQUIRES `config_options`): `let db = Db::open(&dir.join("archimedes.db")).expect("db should open");` + `db.record_session(&SessionInfo { session_id: "x".into(), agent_id: "fake".into(), cwd: "/tmp/x".into(), capabilities: AgentCapabilities::default(), config_options: None })` **FIRST** (FK constraint — `record_message` fails without it; import `crate::acp::SessionInfo` and `agent_client_protocol::schema::v1::AgentCapabilities`) → then `db.record_message(&"x", "user", None, &user_message_payload("hi", &[img(..)]).to_string())` + `db.messages_for(&"x")` → the stored `payload_json` parses back with the `images` array intact.

**`src-tauri/tests/ipc.rs` — a NEW `#[test]` (do NOT modify the existing `history_settings_and_resume_commands_round_trip` — its exact `kinds == ["user", "agent-text"]` assertions would break):** `fn send_prompt_images_ipc()`, repeating the existing test's setup verbatim (`temp_dir("config")` + `temp_dir("data")` + `write_agents_json` + `build_app` + `WebviewWindowBuilder` + `start_session` → `FAKE_SESSION_ID`):
1. **Invalid first (validation-before-persistence proof):** invoke `send_prompt` with `images: [{ "mimeType": "image/svg+xml", "data": "AQID", "name": "a.svg", "sizeBytes": 3 }]` — the `invoke` helper's `.expect` would panic, so write a local `invoke_err` variant that does NOT expect success (same `get_ipc_response` call, then `expect_err("the invalid image should be rejected")` + `.deserialize::<serde_json::Value>()`). Assert the error payload mentions `"unsupported image type"`, then `Db::open(&app_data_dir.join("archimedes.db"))` + `db.messages_for(FAKE_SESSION_ID)` → **NO row with `kind == "user"` exists** (the rejected payload was not persisted).
2. **Then valid (nested camelCase deserialization proof):** `invoke(&webview, "send_prompt", json!({ "sessionId": FAKE_SESSION_ID, "text": "look", "images": [{ "mimeType": "image/png", "data": "AQID", "name": "a.png", "sizeBytes": 3 }] }))` → `"end_turn"`; poll `db.messages_for` until the `user` row lands (same deadline pattern as the existing test) → its `payload_json` parses with `images[0].mimeType == "image/png"` and `images[0].data == "AQID"`. This proves the `#[serde(rename_all = "camelCase")]` on the nested `ImagePayload` actually takes effect over the real Tauri IPC wire (the unit tests above never exercise Tauri's argument deserialization).

**Steps:**
- [ ] Write the 8 tests in `prompt.rs`'s `#[cfg(test)]` module. To compile the red state you need: the `ImagePayload` struct + `MAX_IMAGE_BYTES` + `SUPPORTED_IMAGE_TYPES` + **the `AcpError::InvalidPrompt` variant** (add it in this step — the tests match on it) + function STUBS (`build_prompt_blocks` returning `Err(AcpError::InvalidPrompt { message: "stub".into() })`, `user_message_payload` returning `serde_json::Value::Null`). Do NOT implement the real logic yet.
- [ ] Run `cargo test --lib prompt` (from `src-tauri/`)
  - **Expected red state (do NOT stop if the validation tests pass — they pass by design in the red phase, because the stub already returns `Err(InvalidPrompt)`):** MUST FAIL: `zero_images_is_a_single_text_block`, `text_block_first_then_images_in_order`, `payload_omits_images_key_when_empty`, `payload_includes_images_with_camel_case_keys`, `db_round_trip_preserves_images` (the stub returns `Err`/`Null`). EXPECTED TO PASS ALREADY: `non_image_mime_is_rejected`, `svg_is_rejected`, `oversized_image_is_rejected`. If a MUST-FAIL test passed, stop and investigate.
- [ ] Implement `build_prompt_blocks` + `user_message_payload` for real (the variant is already in place from the stubs step).
- [ ] Wire `commands/sessions.rs` (validation-first order) and `acp/session.rs` (the `send_prompt_with_images` wrapper, validation-first order) as specified.
- [ ] Add the new `send_prompt_images_ipc` test to `src-tauri/tests/ipc.rs` as specified (invalid-first, then valid).
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did ALL tests pass (including the ~16 pre-existing `send_prompt` integration-test call sites, which must compile UNCHANGED)? If not, fix and re-run.
- [ ] Update `src/lib/tauri.ts` `sendPrompt` as specified.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - 0 warnings? (Watch the `TextContent` import move — see the session.rs section.) If not, fix and re-run.
- [ ] Run `cargo fmt --check` (from `src-tauri/`)
  - Did it succeed? If not, run `cargo fmt` and re-check.
- [ ] Run `pnpm test` + `pnpm build` (repo root)
  - Green? (The tauri.ts change must not break the frontend mocks/tests.)
- [ ] Commit with message: "feat: send_prompt accepts image attachments (ACP ImageContent + inline persistence)"

**Acceptance criteria:**
- [ ] Both `send_prompt` paths build text-first + N image blocks, validate (mime + decoded size), and persist `{text, images}` / `{"text"}`.
- [ ] `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` + `pnpm test` + `pnpm build` all green.
- [ ] Sending text-only (no images) is byte-identical to today's behavior (`{"text"}` payload, single text block).

---

### Task 4: Composer — paste / drop / thumbnail strip / capability gate

**Context:**
This task wires the Task 1 lib modules into the composer (`ChatStream.tsx`): pasting or dropping images stages them as removable thumbnails, the spreadsheet-clipboard heuristic protects text pastes, and the whole feature is gated on `promptCapabilities.image` (fail-closed). It does NOT change `send()` yet — that's Task 5 (so this task stays independently green: staged images that can't be sent yet are simply not cleared, and the send button's existing text-only gating is untouched).

**Files:**
- Modify: `src/components/ChatStream.tsx`
- Modify: `src-tauri/tauri.conf.json`
- Test: `src/components/ChatStream.test.tsx`

**What to implement:**

**CRITICAL — hook placement:** `ChatStream` has an early return at ~line 266 (`if (!activeSessionId) return ...`), and `send`/`resume` live AFTER it. ALL new hooks (the `attachments` state, `attachmentsRef`, `sendingRef`, the unmount-cleanup `useEffect`, the session-change `useEffect`, the window-listener `useEffect`) go **next to `const [draft, setDraft] = useState("")` / `composerRef` (~lines 184-188), BEFORE the early return** — hooks after a conditional return crash React when the active session appears/disappears (the file already warns about this exact bug at ~lines 176-181).

In `src/components/ChatStream.tsx` (all items below, in order):

1. Imports: `import { addImageAttachments, agentSupportsImages, releaseAttachment, type ChatComposerAttachment } from "../lib/chatAttachments";` `import { shouldPreferSpreadsheetClipboardText } from "../lib/chatAttachmentMetadata";` `import { X } from "lucide-react";` (`ArrowUp` is already imported from lucide-react — extend that import.)
2. State + refs (next to `draft`/`composerRef`, BEFORE the early return):
   ```tsx
   const [attachments, setAttachments] = useState<ChatComposerAttachment[]>([]);
   const attachmentsRef = useRef(attachments);
   attachmentsRef.current = attachments; // re-sync every render
   const sendingRef = useRef(false); // Task 5's double-send guard (a hook — must be here)
   const imageCapable = agentSupportsImages(liveSession?.capabilities);
   ```
3. Unmount cleanup (revoke staged object URLs exactly once, on unmount ONLY — do NOT revoke on every state change or live previews leak/break) — same block as step 2:
   ```tsx
   useEffect(() => () => {
     attachmentsRef.current.forEach(releaseAttachment);
   }, []);
   ```
4. **Session-change guard** (same block): `ChatStream` is one long-lived component (`App.tsx:~189` renders it with no `key`), so `attachments` would otherwise survive a switch to another session — images staged in session A (image-capable) could be sent to session B, even if B's agent doesn't advertise `promptCapabilities.image` (breaking the fail-closed guarantee). Clear + release on switch:
   ```tsx
   const prevSessionIdRef = useRef(activeSessionId);
   useEffect(() => {
     if (prevSessionIdRef.current === activeSessionId) return;
     prevSessionIdRef.current = activeSessionId;
     attachmentsRef.current.forEach(releaseAttachment);
     attachmentsRef.current = [];
     setAttachments([]);
   }, [activeSessionId]);
   ```
5. **Window-level drop backstop** (same block): with `dragDropEnabled: false` (see the `tauri.conf.json` item below), the webview receives native file drops — and the webview DEFAULT for a file drop is to NAVIGATE to the file (replacing the app). A file dropped anywhere OUTSIDE the composer must be swallowed:
   ```tsx
   useEffect(() => {
     const prevent = (e: Event) => e.preventDefault();
     window.addEventListener("dragover", prevent);
     window.addEventListener("drop", prevent);
     return () => {
       window.removeEventListener("dragover", prevent);
       window.removeEventListener("drop", prevent);
     };
   }, []);
   ```
   (The composer's own `onDrop` fires first during bubbling; the window listener is the backstop for drops outside the composer.)
6. Handlers (component scope — these are plain functions, NOT hooks, so their placement is flexible; keep them with the other handlers):
   ```tsx
   const stageFiles = (files: File[]) => {
     // Read the CURRENT list from the ref — a render closure would be stale
     // for fast successive events (e.g. pasting 8 files then a 9th in
     // separate dispatches without a re-render in between).
     const { attachments: next, rejected } = addImageAttachments(attachmentsRef.current, files);
     if (next !== attachmentsRef.current) {
       attachmentsRef.current = next;
       setAttachments(next);
     }
     // A successful stage also CLEARS a stale rejection line (null when nothing
     // was rejected) — otherwise a rejection stays visible after the user
     // successfully stages a different image.
     setError(rejected.length > 0 ? rejected.join("; ") : null);
   };

   const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
     if (!imageCapable) return; // fall through: default text paste
     const files = Array.from(e.clipboardData.files);
     const text = e.clipboardData.getData("text/plain");
     const html = e.clipboardData.getData("text/html");
     if (files.length === 0 || shouldPreferSpreadsheetClipboardText(text, html)) return;
     e.preventDefault();
     e.stopPropagation();
     stageFiles(files);
   };

   const handleDrop = (e: React.DragEvent) => {
     if (!isLive || composerLocked || !imageCapable) return;
     e.preventDefault();
     stageFiles(Array.from(e.dataTransfer.files));
   };

   const removeAttachment = (id: string) => {
     // Revoke OUTSIDE the state updater (updaters must be pure — StrictMode
     // runs them twice; a double revoke is harmless but the wrong pattern).
     const target = attachmentsRef.current.find((a) => a.id === id);
     if (target) releaseAttachment(target);
     const next = attachmentsRef.current.filter((a) => a.id !== id);
     attachmentsRef.current = next;
     setAttachments(next);
   };
   ```
   (NO composer-level `onDragOver` handler: the window-level `dragover` listener in step 5 already calls `preventDefault` on every dragover (the event bubbles to the window), which makes the whole page a valid drop target — a composer-level conditional one would be dead code. The composer's `onDrop` still fires for drops over the composer.)
7. Textarea (line ~573): add `onPaste={handlePaste}` — NOTHING else changes (`disabled`, `onKeyDown`, placeholder all as-is except step 8).
8. Placeholder — the `isLive && !composerLocked && messages.length === 0` branch becomes:
   `imageCapable ? "Ask anything — or paste an image…" : "Ask anything…"` (all other branches byte-identical).
9. Thumbnail strip — inside the composer container `div` (the `m-3 rounded-2xl border ...` at line ~572), rendered BETWEEN the container's top and the `<textarea>`:
   ```tsx
   {attachments.length > 0 && (
     <div className="mb-2 flex flex-wrap gap-2">
       {attachments.map((att) => (
         <div key={att.id} className="group relative size-12 overflow-hidden rounded-lg border border-input-border bg-input">
           <img src={att.objectUrl} alt={att.filename} className="size-full object-cover" />
           <button
             type="button"
             aria-label="Remove image attachment"
             onClick={() => removeAttachment(att.id)}
             className="absolute right-0.5 top-0.5 size-4 rounded-full bg-input p-0 opacity-0 transition-opacity group-hover:opacity-100"
           >
             <X className="size-3 text-foreground" />
           </button>
         </div>
       ))}
     </div>
   )}
   ```
10. Container `div`: add `onDrop={handleDrop}` (no `onDragOver` — see the note after step 6).

`src-tauri/tauri.conf.json` — **REQUIRED for drag-and-drop to work in the real app** (the jsdom tests pass either way — this is the difference between a test that passes and a feature that works): `dragDropEnabled` defaults to `true`, and with it on, wry intercepts native file drops and the webview NEVER receives them. Add `"dragDropEnabled": false` to `app.windows[0]` (alongside `title`/`width`/`height`). With it off, HTML5 `dragover`/`drop` events reach the webview — which is why the window-level backstop in step 5 exists (a file dropped outside the composer would otherwise navigate the webview to the file).

**Design notes (do not deviate):**
- The strip stays visible while `composerLocked`. **Note the turn-in-flight behavior (Task 5):** `sendPrompt` resolves only when the WHOLE turn ends, and attachments are cleared only on success — so sent thumbnails stay in the strip for the duration of the turn (possibly minutes), alongside the same images now in the transcript bubble; at turn end the sent ones are cleared, and a turn-level failure leaves them staged for resend. This is the approved spec behavior ("a failed send keeps them staged").
- Rejections reuse the existing `error` state (the `text-destructive` line above the composer) — `setError` is already cleared at the top of `send()`.
- The send button's `disabled` is UNCHANGED in this task (Task 5 extends it).
- Staged attachments are CLEARED on session switch (step 4) — they belong to the session they were staged in.

**Steps:**
- [ ] In `src/components/ChatStream.test.tsx`: add `beforeAll` stubs for the object-URL APIs (Node's global `URL` leaks into jsdom but only accepts Node `Blob`s — the stub is required): `URL.createObjectURL = vi.fn((file: File) => \`blob:mock-\${file.name}\`); URL.revokeObjectURL = vi.fn();`. Add a fixture `seedLiveSessionWithImages()` = `seedLiveSession()` but with `capabilities: { promptCapabilities: { image: true } }` in the session entry. Add helpers:
  ```ts
  // `fireEvent.paste` wraps the dispatch in `act` (a raw `dispatchEvent` does NOT —
  // state changes then need `await act(...)` to become visible) and jsdom has no
  // `DataTransfer`, so the plain `clipboardData` object is attached as-is and
  // reaches React's `onPaste` with `e.clipboardData.files` intact.
  function pasteToComposer(files: File[], extra: { text?: string; html?: string } = {}): boolean {
    const target = screen.getByRole("textbox") as HTMLTextAreaElement;
    return fireEvent.paste(target, {
      clipboardData: {
        files,
        getData: (type: string) =>
          type === "text/plain" ? (extra.text ?? "") : (extra.html ?? ""),
      },
    });
  }

  function dropOnComposer(files: File[]): void {
    // `getByRole("textbox")`, NOT `getByPlaceholderText(/Ask/i)` — the placeholder
    // changes with session state (e.g. "Agent is working…" when locked).
    const target = screen.getByRole("textbox");
    fireEvent.drop(target, { dataTransfer: { files } });
  }
  ```
- [ ] Write the tests (each in its own `it`):
  1. `paste stages a thumbnail` — `seedLiveSessionWithImages()`; `pasteToComposer([new File([new Uint8Array([1])], "s.png", { type: "image/png" })])` → `screen.getByAltText("s.png")` appears; the paste WAS intercepted (assert `pasteToComposer` returned `false` — `defaultPrevented`).
  2. `paste of plain text is NOT intercepted` — `pasteToComposer([], { text: "hello" })` → the helper returns `true` (default action left alone) and NO thumbnail appears. **Do NOT assert on the textarea value — jsdom does not implement the browser's default paste action (no text is inserted into a textarea by a synthetic paste event).**
  3. `spreadsheet TSV is NOT intercepted over the synthetic PNG` — `pasteToComposer([new File([new Uint8Array([1])], "shot.png", { type: "image/png" })], { text: "a\tb" })` → the helper returns `true` (text paste wins) and NO thumbnail appears.
  4. `non-image paste is rejected with an error line` — `pasteToComposer([new File([new Uint8Array([1])], "a.txt", { type: "text/plain" })], { text: "x" })` → no thumbnail; the error line (`.text-destructive`) contains `"is not a supported image format"`.
  5. `SVG is rejected (allowlist)` — `pasteToComposer([new File([new Uint8Array([1])], "a.svg", { type: "image/svg+xml" })])` → no thumbnail; error line contains `"is not a supported image format"`.
  6. `oversized paste is rejected` — 11 MiB `File` → no thumbnail; error line contains `"exceeds the 10 MiB limit"`.
  7. `ninth image is rejected at the cap` — `seedLiveSessionWithImages()`; `pasteToComposer` with 8 small files, then `pasteToComposer` with a 9th (separate calls — the ref-based `stageFiles` keeps the list current between dispatches) → the 9th is NOT staged (8 thumbnails present); error line contains `"at most 8"`.
  8. `remove button un-stages` — paste 1 file, click `screen.getByRole("button", { name: "Remove image attachment" })` → `screen.queryByAltText("s.png")` is gone.
  9. `drop stages a thumbnail` — `seedLiveSessionWithImages()`; `dropOnComposer([new File([new Uint8Array([1])], "d.png", { type: "image/png" })])` → `screen.getByAltText("d.png")` appears.
  10. `drop is ignored while locked` — seed live + images; set `inTurn` to `{ s1: true }` BEFORE `render` (call `useSessions.setState({ inTurn: { s1: true } })` in the same setup as the render — the store is global, so setting it before `render` makes the textarea `disabled` on first render); `dropOnComposer([...])` → NO thumbnail.
  11. `placeholder advertises paste when the agent supports images` — `seedLiveSessionWithImages()`, empty conversation → `screen.getByPlaceholderText("Ask anything — or paste an image…")`.
  12. `feature is inert without the capability` — `seedLiveSession()` (capabilities `{}`); `pasteToComposer([image file])` → returns `true` (NOT intercepted — default text paste falls through), NO thumbnail; placeholder is the plain `"Ask anything…"`.
  13. `switching sessions clears staged attachments` — `seedLiveSessionWithImages()`; `pasteToComposer` with 1 file (thumbnail present); then `await act(async () => { useSessions.setState({ activeSessionId: "s2", sessions: [{ sessionId: "s2", agentId: "a1", cwd: "/home/u/proj", capabilities: {} }] }); })` (the `act` wrap is required — the session-change effect's `setAttachments` only reliably flushes inside `act`, matching the existing post-render store-update pattern in this file) → the thumbnail is GONE (the session-change effect cleared + released it).
- [ ] Run `pnpm test`
  - Did the new tests FAIL? (Most should — no handlers yet.)
- [ ] Implement the 10 numbered changes above (including the `tauri.conf.json` `dragDropEnabled: false` edit).
- [ ] Run `pnpm test`
  - Did ALL tests pass (new + pre-existing)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: composer paste/drop stages image attachments"

**Acceptance criteria:**
- [ ] Paste/drop stage image attachments with the exact caps + allowlist + spreadsheet heuristic; the strip renders per spec; the feature is inert (byte-identical behavior) for agents without `promptCapabilities.image`.
- [ ] Staged attachments are cleared + released on session switch; the window-level backstop swallows drops outside the composer; `dragDropEnabled: false` is set in `tauri.conf.json` (drag-and-drop actually works in the real app).
- [ ] All new hooks are placed before the early return; all pre-existing `ChatStream.test.tsx` tests pass.

---

### Task 5: Send wiring + transcript rendering

**Context:**
Final task: `send()` reads staged attachments to base64 (at send time only — Task 1's pattern) and sends them with the prompt; the send button enables for image-only sends; attachments are cleared + released ONLY on success (a failed send keeps them staged — spec decision: a re-pasted image is costly, a re-typed draft is not). `MessageBubble` renders user-message images as a read-only thumbnail grid (the transcript is history — no remove buttons).

**Files:**
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/components/MessageBubble.tsx`
- Test: `src/components/ChatStream.test.tsx`
- Test: `src/components/MessageBubble.test.tsx`

**What to implement:**

`src/components/ChatStream.tsx`:

1. `send()` (line ~277) becomes (`sendingRef` is a hook declared in Task 4's step-2 block — do NOT re-declare it here):
   ```tsx
   const send = async () => {
     const text = draft.trim();
     // `hasImages` (component scope, defined just above `send`):
     //   const hasImages = attachments.length > 0 && imageCapable;
     // The `imageCapable` part is the last line of the fail-closed defense —
     // staged attachments are already cleared on session switch (Task 4), but a
     // same-session capability regression must not ship images to an agent that
     // can't take them. With it, an image-ONLY send while `!imageCapable` is
     // blocked (an empty prompt + unsent images is meaningless), while a
     // text-only send simply doesn't attach images.
     if ((!text && !hasImages) || composerLocked || !isLive) return;
     if (sendingRef.current) return; // guard: the composer is not locked until
     // `beginTurn`, which now runs AFTER an `await` (the base64 read) — a second
     // Enter/click during the read would otherwise double-send.
     sendingRef.current = true;
     try {
       setError(null);
       let images: ImageRef[] | undefined;
       if (hasImages) {
         try {
           images = await Promise.all(
             attachments.map(async (att) => ({
               name: att.filename,
               mimeType: att.mimeType,
               sizeBytes: att.sizeBytes,
               data: await readAttachmentAsBase64(att),
             })),
           );
         } catch {
           setError("Failed to read an attached image");
           return; // attachments stay staged; `sendingRef` resets in `finally`
         }
       }
       // Snapshot the ids being sent: on success, release/clear ONLY these.
       // Anything staged after this snapshot — e.g. a drop during the FileReader
       // `await` (the composer isn't locked until `beginTurn`), or attachments
       // staged in ANOTHER session while this turn was running — must survive.
       const sentIds = new Set(attachments.map((a) => a.id));
       setDraft("");
       addUserMessage(activeSessionId, text, images);
       beginTurn(activeSessionId);
       // Pass the third arg ONLY when there are images: a text-only send calls
       // `sendPrompt(id, text)` — the pre-existing 2-arg call the existing tests
       // assert (`toHaveBeenCalledWith("s1", "hello")`; vitest compares arg
       // arrays by length, so an explicit `undefined` third arg would break it).
       const stopReason = images
         ? await sendPrompt(activeSessionId, text, images)
         : await sendPrompt(activeSessionId, text);
       // Release/clear ONLY the sent attachments, OUTSIDE the state updater
       // (updaters must be pure — StrictMode runs them twice).
       const still = attachmentsRef.current.filter((a) => !sentIds.has(a.id));
       attachmentsRef.current
         .filter((a) => sentIds.has(a.id))
         .forEach(releaseAttachment);
       attachmentsRef.current = still;
       setAttachments(still);
       turnCompleted(activeSessionId, stopReason);
     } catch (err) {
       turnCompleted(activeSessionId, "end_turn");
       // Tauri IPC errors are plain objects (`{ kind, message }`), not `Error`
       // instances — `String(err)` would show `[object Object]` (a pre-existing
       // gap the new `InvalidPrompt` validation error would hit; fix it here).
       const msg =
         err instanceof Error
           ? err.message
           : err && typeof err === "object" && "message" in err
             ? String((err as { message: unknown }).message)
             : String(err);
       setError(msg);
       // Attachments deliberately stay staged (a failed send keeps them).
     } finally {
       sendingRef.current = false;
     }
   };
   ```
   (Import `readAttachmentAsBase64` + `type ImageRef` from `../lib/chatAttachments` — extend the Task 4 import. `setDraft("")` before the await is the PRE-EXISTING behavior — keep it. Note the draft is lost on failure today; that is unchanged, out of scope. `sendingRef` is the hook from Task 4's step 2 — it is NOT declared in this block.)
2. In the component scope (immediately above `send`): `const hasImages = attachments.length > 0 && imageCapable;` (a plain const — not a hook; fine after the early return).
3. Send button `disabled` (line ~616): `disabled={!isLive || composerLocked || (draft.trim() === "" && !hasImages)}`

`src/components/MessageBubble.tsx` — the `case "user"` (line ~156) becomes:
```tsx
case "user":
  return (
    <div className="whitespace-pre-wrap text-ui-base text-foreground">
      {message.text}
      {message.images && message.images.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-2">
          {message.images.map((img, i) => (
            <img
              key={i}
              src={`data:${img.mimeType};base64,${img.data}`}
              alt={img.name}
              title={img.name}
              className="max-h-48 max-w-64 rounded-lg border border-input-border object-contain"
            />
          ))}
        </div>
      )}
    </div>
  );
```
(Read-only: NO remove buttons. `data:` URL is safe here — the transcript is local. The `memo` wrapper is untouched.)

**Steps:**
- [ ] In `ChatStream.test.tsx` add tests. `send()` is now ASYNC (it awaits `FileReader` before `sendPrompt`), so every expectation that depends on a send uses `await waitFor(() => ...)` — do NOT assert synchronously. `sendPrompt` is already a `vi.fn()` (line ~51, mocks cleared between tests) — use the repo's `vi.mocked(sendPrompt)` pattern (see `AskQuestionCard.test.tsx:~17`) for overrides; do NOT cast `as vi.Mock` (it fails `tsc` — `TS2503: Cannot find namespace 'vi'`):
  1. `send delivers images with the prompt` — `seedLiveSessionWithImages()`; paste `new File([new Uint8Array([1, 2, 3])], "s.png", { type: "image/png" })` (base64 `AQID`); set the textarea value to `"look at this"` via `fireEvent.change`; click `getByRole("button", { name: "Send" })` → `await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("s1", "look at this", [{ name: "s.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" }]))` (3 args — images present); the store's last user message has `images` (assert via `useSessions.getState().messages.s1`); the thumbnail is GONE after success.
  2. `a failed send keeps attachments staged` — `vi.mocked(sendPrompt).mockRejectedValueOnce(new Error("boom"))`; paste a file; click Send → `await waitFor(() => expect(screen.getByText("boom")).toBeTruthy())` (wait on the OUTCOME — do NOT wait on the Send button being enabled: with a staged attachment it is enabled before the click and stays enabled right after, because `beginTurn` runs only after the FileReader `await`; a button-state wait would pass immediately and make the test flaky), then assert the thumbnail is STILL present.
  3. `a non-Error rejection shows the message, not [object Object]` — `vi.mocked(sendPrompt).mockRejectedValueOnce({ kind: "error", message: "proto" })` (a plain object, as Tauri IPC errors arrive); paste a file; click Send → `await waitFor(() => expect(screen.getByText("proto")).toBeTruthy())`. (Do NOT use `toBeInTheDocument()` / `toBeEnabled()` — `@testing-library/jest-dom` is NOT installed in this repo; the existing tests use `toBeTruthy()` and `hasAttribute("disabled")`.)
  4. `image-only send is allowed` — `seedLiveSessionWithImages()`; paste a file, type nothing → `getByRole("button", { name: "Send" })` is NOT disabled; clicking sends — `await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("s1", "", [image]))` (3 args — images present).
  5. `empty composer with no attachments: send stays disabled` — `seedLiveSessionWithImages()`; no paste → Send IS disabled.
  6. `send is fail-closed without the capability` — `seedLiveSession()` (capabilities `{}`); `pasteToComposer([image file])` (Task 4: NOT staged — the helper returns `true`); set the textarea value to `"text"`; click Send → `await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("s1", "text"))` — a 2-arg call with NO images argument, even though a `File` was on the clipboard (the `imageCapable` guard in `send()`).
- [ ] Run `pnpm test`
  - Did the new tests FAIL?
- [ ] In `MessageBubble.test.tsx` add tests: (a) a user message with `images: [{ name: "a.png", mimeType: "image/png", sizeBytes: 3, data: "AQID" }]` → renders an `<img>` with `src === "data:image/png;base64,AQID"` and `alt === "a.png"`; (b) an image-only user message (`text: ""`, 1 image) → the img renders; (c) a plain user message (no `images`) → NO `<img>` (pre-feature behavior unchanged).
- [ ] Run `pnpm test`
  - Did the new tests FAIL?
- [ ] Implement the changes above.
- [ ] Run `pnpm test`
  - Did ALL tests pass (new + pre-existing — INCLUDING `calls sendPrompt when Enter is pressed with a draft`, which asserts the 2-arg `sendPrompt` call)? If not, fix and re-run.
- [ ] Run `pnpm build`
  - Did it succeed? If not, fix and re-run.
- [ ] Run the FULL validation gate (AGENTS.md): `pnpm test` + `pnpm build` (repo root) and `cargo test` + `cargo clippy --all-targets` + `cargo fmt --check` (from `src-tauri/`)
  - All green? (Rust should be untouched by this task — if any Rust gate fails, investigate before committing.)
- [ ] Commit with message: "feat: send image attachments with the prompt; render them in the transcript"

**Acceptance criteria:**
- [ ] `sendPrompt` receives `{ name, mimeType, sizeBytes, data }` per staged attachment (3-arg call) and is called with 2 args for text-only sends (pre-existing tests stay green); the store's user message carries them; attachments clear + release only on success.
- [ ] Image-only sends work; the send button's disabled logic matches the spec; the double-send guard + `imageCapable` guard hold.
- [ ] `MessageBubble` renders user images read-only; image-less messages render byte-identically to before.
- [ ] The full AGENTS.md validation gate is green.

---

## Out of scope (v1 — do NOT implement in any task)

- Attach button / file picker, lightbox preview, non-image file attachments (no meaningful ACP prompt path — pi-acp only converts image/text/resource_link), `steer`/`follow_up` images, the 15 KB long-paste→`.txt` conversion, any Rust-side capability re-check, `image/*` types outside the png/jpeg/gif/webp allowlist (deliberate — provider vision APIs don't take them; see `SUPPORTED_IMAGE_TYPES`).

## Post-ship (informational — not tasks)

- On ship: fold durable content into `docs/features/` and delete this roadmap doc (per the docs convention).
- If SQLite growth ever bites: the ADR 0008 escape hatch — a one-off data migration rewrites `images` entries to `path` references (the JSON-blob shape makes it a data migration, not a schema change).
