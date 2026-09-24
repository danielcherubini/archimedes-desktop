---
status: accepted
date: 2026-09-24
superseded-by:
---

# Attachments persist inline (base64) in the user message's payload

The composer can stage image **Attachments** (paste / drag-and-drop) that are sent with the next prompt. We decided the transcript persists them **inline — base64 inside `messages.payload_json`** (`{ "text": ..., "images": [{ name, mimeType, sizeBytes, data }] }` — the `images` key omitted when empty) — rather than writing the image bytes to on-disk files referenced from the payload, or storing metadata only.

**Why:** the `messages` table already stores `payload_json TEXT` (arbitrary JSON), so inline storage needs **zero schema migration and zero file-lifecycle code** (no per-session attachments dir, no cleanup on session/space delete, no missing-file → placeholder rendering). The growth cost is bounded by the 10 MiB per-image cap (a 10 MiB image stores ~13.5 MB; worst case 8 attachments ≈ 108 MB per message; typical pasted screenshots are 1–5 MB).

**Considered Options**

1. **Inline base64 in `payload_json` (chosen)**: zero migration, zero file lifecycle; growth bounded by the cap.
2. *Disk file + path reference* (write to a per-session attachments dir, store `{ path, mimeType, sizeBytes, name }`): the DB stays small forever, but adds file-lifecycle machinery — cleanup on session/space delete, missing-file → placeholder rendering, and the decision of where the dir lives. More moving parts for v1.
3. *Metadata only* (`{ name, mimeType, sizeBytes }`, no image data): zero bloat, but the user bubble shows a placeholder on reload — the transcript loses the images.

**Consequences**

- The SQLite `messages` table can hold multi-megabyte rows; total growth scales with usage (bounded per message by the caps).
- **Escape hatch:** if growth ever bites, a one-off data migration rewrites rows to `path` references (option 2) — the JSON-blob shape makes this a data migration, not a schema change; the payload format stays forward-compatible.
- The 10 MiB cap is deliberately **lower than ZCode's 20 MiB** (whose attachments upload to a server): the desktop persists inline, so the cap bounds DB growth.
- The Rust `send_prompt` command re-validates the caps (mime prefix + decoded size) so hand-rolled IPC cannot bloat the DB out-of-band.
