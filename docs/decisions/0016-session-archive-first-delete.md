---
status: accepted
date: 2026-10-01
superseded-by:
---

# Session archive-first delete, and delete removes the desktop's copy only

Deleting a stored Session is offered **only from the Archived section** of the sidebar, never from a Space group's session rows. A stored row's hover action is **Archive** (hide from the Space group, keep in storage, still openable/resumable); the Archived section offers **Unarchive** and **Delete** (confirm dialog). The pattern is taken from ZCode: the main list's action is archive, so a transcript is never destroyed by a single misclick — deletion is two deliberate steps away (archive → find in Archived → confirm).

Deleting a session removes **the desktop's copy only**: the `sessions` row and its `messages` / `native_messages` rows (the existing `delete_session` cascade). The agent's own on-disk session file (the `piSessionFile` capability, e.g. under `~/.pi/`) is **left untouched**.

**Considered Options**

- **Delete directly on main-list rows** (a trash icon beside Archive, confirm dialog): rejected for v1 — one click away from destroying a transcript, and it makes Archive a redundant affordance. Archive-first is the ZCode pattern and the safer default; a direct delete can be added later without breaking the archive flow.
- **Delete the agent's session file too** (truly permanent): rejected — the file lives in the user's pi home directory, which they may use from the pi CLI directly; the desktop deleting files out from under another tool's home dir is surprising, and a bug in the delete path (the path is a string from a JSON blob) could delete the wrong file irrecoverably. Orphaned pi session files are small; "purge agent files" is a separate, future feature if ever wanted.
- **Archive = a separate table / soft-delete flag with no UI view**: rejected — without search, an archived session with no way to find it is a dead end; the Archived section is what makes archive a real state instead of a tombstone.

**Consequences**

- The `sessions` table gains an `archived` flag (one-time `ALTER TABLE` migration, same pattern as `spaces.trusted`); `list_sessions` filters on it. The flag is **sticky** — it survives resume/pause and changes only via explicit Archive/Unarchive.
- Archiving hides a session from its Space group but NOT from resumability: an archived session opens and resumes exactly like a stored one.
- Orphaned agent-side session files accumulate on disk after deletes. They are unresumable from the desktop (resume reads the desktop's DB) and harmless; cleanup is out of scope.
- Live sessions are never archived: a live row's hover action stays Pause; pause → stored → archive is the two-step path.
