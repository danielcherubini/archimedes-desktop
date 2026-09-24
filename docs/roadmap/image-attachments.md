---
status: approved
done-when: Pasting or dropping an image into the composer stages a removable thumbnail; sending it delivers ACP image content blocks to the agent (pi normalizes and forwards them to the model); the image renders in the user's transcript bubble after app restart.
---

# Image Attachments in the Composer

**One-liner:** The Client's composer accepts image **Attachments** (clipboard paste + drag-and-drop), previews them as removable thumbnails, sends them with the prompt as ACP `ImageContent` blocks, and persists them inline in the transcript.

## Behavior

- **Staging** — pasting an image (or dropping one) into the composer stages it as an Attachment: a 48px `rounded-lg` thumbnail appears in a strip above the textarea, with a hover `✕` remove button. Max **8** attachments, each ≤ **10 MiB** (deliberately lower than ZCode's 20 MiB — the desktop persists inline, so the cap bounds DB growth; ADR 0008); violations surface as an inline error line.
- **The spreadsheet trap** — if the clipboard also carries tabular text (TSV with `\t`, or HTML matching Excel/Office patterns), the **text paste wins** and no Attachment is created (ported from ZCode's `shouldPreferSpreadsheetClipboardText`).
- **Send** — the send button enables when there is text **or** ≥1 staged Attachment (image-only sends allowed). On send, each Attachment is read to base64 (`FileReader`, at send time only) and sent alongside the text; Attachments are cleared + object-URLs revoked **only on success** (a failed send keeps them staged — a re-pasted image is costly, a re-typed draft is not).
- **Capability gate (fail-closed)** — the paste/drop handlers, thumbnail strip, and image placeholder are wired only when the agent advertises `promptCapabilities.image` (the raw `capabilities` record is already in the store — `src/lib/tauri.ts:54` — no new IPC). Missing/`false` → everything falls through to normal text paste, exactly as today.
- **Locked session** — the textarea stays `disabled` during a turn (`!isLive || composerLocked` — paste impossible); drop is explicitly guarded. Staged Attachments **survive** the turn (strip stays visible, remove buttons enabled) and ride the next prompt.
- **Transcript** — user messages render their text plus a read-only thumbnail grid (no remove buttons; `alt` = filename). Image-only messages render thumbnails alone. Attachments persist **inline base64 in `messages.payload_json`** (ADR 0008): `{ "text", "images": [{ name, mimeType, sizeBytes, data }] }` — the `images` key is omitted when empty; pre-feature rows hydrate unchanged.

## Implementation seams

| # | File | Change |
|---|------|--------|
| 1 | `src/lib/chatAttachments.ts` (new) | Port from ZCode (`ZCode/packages/ui/src/lib/chatAttachments.ts`), images-only: attachment type `{ id, file, filename, mimeType, sizeBytes, objectUrl }`, `createChatComposerAttachment` (object-URL + mime normalize), `readAttachmentAsBase64` (send-time only, strips the `data:` prefix), `releaseAttachment` (revoke), caps (8 / 10 MiB) |
| 2 | `src/lib/chatAttachmentMetadata.ts` (new) | Port `shouldPreferSpreadsheetClipboardText` verbatim from `ZCode/packages/ui/src/chatAttachmentMetadata.ts` (the `\t` + Excel/Office-HTML regex) |
| 3 | `src/components/ChatStream.tsx` | `attachments` state; `onPaste` on the textarea (intercept: non-empty `files` + not-spreadsheet → `preventDefault` + `stopPropagation`, filter `image/*`, enforce caps → error line; else fall through); `onDrop`/`onDragOver` on the composer container (same pipeline, guarded by `!isLive \|\| composerLocked`); thumbnail strip (semantic tokens per ADR 0006 — ZCode's `border-border`/`bg-surface` map to the desktop's ported tokens; no lightbox in v1); send-button gating (`draft.trim() !== "" \|\| attachments.length > 0`); placeholder branch (live + unlocked + empty conversation + image-capable → "Ask anything — or paste an image…"); clear-on-success + unmount cleanup |
| 4 | `src/lib/tauri.ts` | `sendPrompt(sessionId, text, images?: ImagePayload[])`, `ImagePayload = { mimeType: string; data: string }` (base64, no prefix) |
| 5 | `src-tauri/src/commands/sessions.rs` | `send_prompt` + `images: Vec<ImagePayload>`; validate (mime prefix `image/`, decoded size ≤ 10 MiB — guards against hand-rolled IPC); text-first block ordering (`ContentBlock::Text` then `ContentBlock::Image(ImageContent::new(data, mime_type))` per attachment — matches pi's own ordering); persist `{ "text", "images": [...] }` via `db.record_message` |
| 6 | `src-tauri/src/acp/session.rs` | `SessionManager::send_prompt(&self, session_id, text, images)` mirrors the command (the command stays a thin wrapper, as today); no Rust-side capability re-check (the frontend gate is the single gate — YAGNI) |
| 7 | `src/store/sessions.ts` | `kind: "user"` gains `images?: ImageRef[]` (`ImageRef = { name, mimeType, sizeBytes, data }`); `addUserMessage(sessionId, text, images?)`; hydration (`rowToMessages`) → `{ kind: "user", text, at, images: payload.images }` tolerating the missing `images` key |
| 8 | `src/components/MessageBubble.tsx` | `kind: "user"` renders the existing text (`whitespace-pre-wrap`) plus a read-only thumbnail grid below it (same `size-12 rounded-lg object-cover border` thumbnails; `alt` = name; image-only messages render thumbnails alone) |

**Out of scope (v1):** attach button / file picker, lightbox preview, non-image file attachments (no meaningful ACP prompt path — pi-acp only converts image/text/resource_link), `steer`/`follow_up` images, the 15 KB long-paste→`.txt` conversion, any Rust-side capability re-check.

## Verification (AGENTS.md gate — all green)

- **TDD:** failing tests first, then implementation
  - *Frontend (`pnpm test`):* `chatAttachments` (object-URL creation, mime normalization, base64 prefix-strip, revoke-on-release, 8-attachment + 10 MiB caps); `chatAttachmentMetadata` (TSV with `\t` → true; Excel HTML → true; plain text → false; empty → false); `ChatStream` paste (image file → staged; text-only → falls through; spreadsheet TSV + synthetic PNG → **text wins**; intercept → `preventDefault` + `stopPropagation`; over-cap → error line); drop (image staged; non-image → error; locked → ignored); `sessions` store (round-trip with images; old-row hydration without the `images` key); `MessageBubble` (with images; image-only)
  - *Rust (`cargo test`):* `send_prompt` builds text-first + N image blocks in order; rejects non-`image/` mimes; rejects >10 MiB; persists `{text, images}` with the key and `{"text"}` without; `SessionManager::send_prompt` mirrors
- `pnpm test` + `pnpm build` (repo root)
- `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check` (`src-tauri/`)

## Context

- Research (2026-09-24, 3 angles): the agent side is fully image-capable today — ACP defines `ImageContent` gated by `promptCapabilities.image`; `pi-acp` 0.0.33 advertises `promptCapabilities.image: true` and translates image blocks into pi's RPC `images`; `pi` 0.87.x normalizes images and forwards them to provider vision APIs (non-vision models degrade gracefully). **No agent-side changes required.**
- Design-system precedent (ADR 0006/0007): faithful port of ZCode's image pipeline, adapted to the desktop's plain `<textarea>` (no Lexical); the simplified/hand-rolled port was rejected as the exact drift 0006 warned about.
- Decisions made during design: paste + drag-and-drop (no attach button); inline base64 persistence (ADR 0008); image-only sends allowed; 10 MiB per-image cap (deviation from ZCode's 20 MiB, justified by inline persistence).
