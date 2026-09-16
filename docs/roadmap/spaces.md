---
status: committed
done-when: In `pnpm tauri dev` (this machine): "New space" → pick a folder → the agent dropdown defaults to `pi`; starting the session streams REAL inference (visible model latency, not instant boilerplate); a file write in the folder raises an in-UI permission prompt and, on approve, a real file-diff card appears; the sidebar lists Spaces (folder base name + path) with live/stored status; starting a session in a second space pauses the first ("replaced" banner) and Resume reconnects; `cargo test` and `pnpm test` stay green (including the new one-live-policy and spaces-storage tests).
---

# Spaces Plan

**Goal:** Turn the flat session list into folder-anchored **Spaces** (a Space = a folder; identity = canonical path; v1 = 1 active conversation per Space, at most 1 live session app-wide), and make the app ready to run real `pi` inference out of the box.
**Architecture:** A `spaces` table (path PK) plus a one-live-session policy enforced in the Rust `SessionManager` (close-then-spawn, `replaced` close reason); Tauri command surface gains `spaces` + `agents` queries; the React sidebar/main pane reskin around Space rows and per-space conversation selection. Macro terminology: see `CONTEXT.md` (Space / Session entries) and `docs/decisions/0002-one-live-acp-session.md`.
**Tech Stack:** Rust (Tauri 2, tokio, rusqlite, `agent-client-protocol` 2.x) + TypeScript/React (Vite, Zustand, Vitest).

**Repos, paths and commands used in every task below** (working dir: repo root `/home/daniel/Coding/AI/archimedes-desktop`):
- Rust: `cd src-tauri && cargo test --test <name>` (or full `cargo test`), `cargo fmt`, `cargo clippy -- -D warnings`, `cargo build`
- Front: `pnpm test` (Vitest), `pnpm build` (tsc + vite)
- Machine facts (2026-09-16, this box): both `pi` and `pi-acp` are on PATH (real inference works); the wave-1 working tree is green: `cargo test` 21/21, `pnpm test` 28/28.
- Wire case conventions (unchanged): command args camelCase with Tauri mapping; `SessionInfo` camelCase over IPC; ACP update discriminators snake_case.

---

### Task 1: Commit the uncommitted wave-1 fixes as the baseline

**Context:**
The working tree carries uncommitted review-wave-1 fixes (from the 2026-09-15 deep-review wave): the `EventSink` is now managed as `Arc<dyn EventSink>` (TypeId fix for "state not managed for field `sink`"), `SessionManager` is shared as a `Sync` `Arc` with per-method inner locks (the outer tokio `Mutex` that serialized the whole app is gone), and the PTY terminal layer is removed (Rust module, `TerminalPane` UI, xterm deps, terminal tests). All of this is verified green on 2026-09-16 (`cargo test` 21/21; `pnpm test` 28/28). The whole Spaces feature builds on it, so it must be committed first as a single baseline commit. `.work-notes.md` is git-ignored (`.git/info/exclude`) and must NOT be committed.

**Files:**
- Modify (already in the working tree — commit **all** of it as-is; verified count: **19 modified + 3 deleted + 2 untracked = 24 paths** in this commit): `src-tauri/src/lib.rs`, `src-tauri/src/acp/mod.rs`, `src-tauri/src/acp/session.rs`, `src-tauri/src/acp/permission.rs`, `src-tauri/src/commands/sessions.rs`, `src-tauri/src/commands/settings.rs`, `src-tauri/src/storage/db.rs`, `src-tauri/src/bin/fake_agent.rs`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tests/acp_flow.rs`, `src-tauri/tests/ipc.rs`, `package.json`, `pnpm-lock.yaml`, `src/App.tsx`, `src/lib/tauri.ts`, `src/store/sessions.ts`, `CONTEXT.md`, `docs/roadmap/archimedes-desktop.md` (one appended out-of-scope bullet about the removed PTY terminal; `CONTEXT.md` holds the new **Space** glossary entry)
- Delete (already in the working tree): `src-tauri/src/acp/terminal.rs`, `src-tauri/tests/terminal_flow.rs`, `src/components/TerminalPane.tsx`
- New (written during design, **deliberately included** in the baseline commit so the ADR + plan travel with the feature): `docs/decisions/0002-one-live-acp-session.md`, `docs/roadmap/spaces.md` (this plan)

**What to implement:**
- Nothing new. Do NOT edit any file. The task is verification + a single commit of everything already in the working tree.

**Steps:**
- [ ] `cd src-tauri && cargo test` — did ALL tests pass (21/21)? If any fail, stop and investigate; do not commit.
- [ ] `cd .. && pnpm test` — did all tests pass (28/28)? If not, stop and investigate.
- [ ] `git status --short` — confirm the changed set is **19 M + 3 D + 2 untracked** (the untracked ones being `docs/decisions/0002-one-live-acp-session.md` and `docs/roadmap/spaces.md`) and that `.work-notes.md` does NOT appear (it is git-ignored; if it somehow showed up as untracked, do not stage it).
- [ ] `git add -A && git status --short` — double-check the staged set contains nothing under `node_modules`/`dist` and no `.work-notes.md`; it SHOULD contain exactly the 24 paths listed above (this plan doc and the ADR are deliberately part of the baseline).
- [ ] Commit with message: `fix: review wave — drop global manager lock, trait-typed sink management, remove PTY terminal layer`
- [ ] `git log -1 --stat | head -40` — confirm the baseline commit landed with 24 files.

**Acceptance criteria:**
- [ ] Wave-1 changes are committed in a single commit on top of `main`'s HEAD.
- [ ] `cargo test` and `pnpm test` are green immediately after the commit (no functional change — they passed before it).

---

### Task 2: Spaces schema + storage methods

**Context:**
A **Space** is the app's reference to a folder (design decisions: identity = canonical absolute path; no user-supplied name; a Space is born when a conversation starts/resumes in its folder; "remove space" deletes only the bookkeeping row). This task adds the persistence layer: a `spaces` table, row type, and the `Db` methods every later task uses. `Db` (in `src-tauri/src/storage/db.rs`) already uses `CREATE TABLE IF NOT EXISTS` via one `execute_batch(SCHEMA)` in `Db::open` — so adding the table to the schema constant is a no-migration change for existing databases.

**Files:**
- Modify: `src-tauri/src/storage/db.rs`
- Modify: `src-tauri/src/storage/mod.rs` (add `SpaceRow` to the re-export: `pub use db::{Db, DbError, MessageRow, SessionRow, SpaceRow};`) — a later task's command returns `Vec<crate::storage::SpaceRow>`, so the type must be re-exported.
- Test: `src-tauri/tests/storage.rs`

**What to implement:**

In `src-tauri/src/storage/db.rs`:

1. Add a public row type next to `SessionRow`:

```rust
/// A row from the `spaces` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    pub path: String,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds.
    pub last_opened_at: i64,
}
```

2. Add to the `SCHEMA` constant (after the `messages` index, before the closing `"#;`):

```sql
CREATE TABLE IF NOT EXISTS spaces (
    path TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    last_opened_at INTEGER NOT NULL
);
```

3. Add these methods to `impl Db`:

```rust
/// Insert or touch the bookkeeping row for a folder.
///
/// `created_at` is preserved on conflict; `last_opened_at` is always
/// refreshed (an actual start/resume is a real "open"). No folder
/// validation happens here — whether the folder exists is the caller's
/// problem (the canonicalizing gate is in `start_session`/`space_for_path`).
pub fn upsert_space(&self, path: &str) -> Result<(), DbError>
```
SQL: `INSERT INTO spaces (path, created_at, last_opened_at) VALUES (?1, ?2, ?2) ON CONFLICT(path) DO UPDATE SET last_opened_at = excluded.last_opened_at` (params: `path`, `now_ms()`).

```rust
/// All spaces, most recently opened first.
pub fn list_spaces(&self) -> Result<Vec<SpaceRow>, DbError>   // ORDER BY last_opened_at DESC, path ASC

/// Look up one space row by exact stored path (None if absent).
pub fn find_space(&self, path: &str) -> Result<Option<String>, DbError>   // SELECT path FROM spaces WHERE path = ?1

/// Delete the bookkeeping row only (conversations/messages are NOT touched).
pub fn delete_space(&self, path: &str) -> Result<(), DbError>   // DELETE FROM spaces WHERE path = ?1
```

4. Backfill in `Db::open`, AFTER `conn.execute_batch(SCHEMA)` and BEFORE the function creates `Self { conn: StdMutex::new(conn) }`:

```rust
// One-time backfill for pre-existing databases: give a space row to every
// distinct stored session cwd (canonicalized). A vanished folder is skipped
// silently — its sessions stay stored, just without a space.
// DO NOTHING on conflict: the backfill must NOT refresh last_opened_at on
// every open (that would collapse the sidebar's recent-first ordering to a
// tie on every restart — recency is refreshed by `upsert_space`, which runs
// on real starts/resumes, i.e. Task 3's `record_session` hook).
let cwds: Vec<String> = {
    let mut stmt = conn.prepare("SELECT DISTINCT cwd FROM sessions")?;
    stmt.query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
};
for cwd in cwds {
    if let Ok(c) = std::fs::canonicalize(&cwd) {
        let p = c.display().to_string();
        let now = now_ms();
        conn.execute(
            "INSERT INTO spaces (path, created_at, last_opened_at) VALUES (?1, ?2, ?2)
             ON CONFLICT(path) DO NOTHING",
            params![p, now],
        )?;
    }
}
```
CRITICAL ordering: the read of `cwds` uses the bare `conn` BEFORE `Self` is built (rewritten from the `Db::open` tail), so there is no lock; the backfill's `DO NOTHING` makes it safe to run again on every `Db::open` (idempotent, no recency side-effect). All-allowed edge cases: if a folder under `sessions.cwd` has been deleted, `canonicalize` fails and the loop skips it — `Db::open` never fails because of a missing folder. Use the bare `params!` in the new code — the file already imports it (`use rusqlite::{params, Connection};`) and its call sites are all bare `params![…]`; do not introduce a fully-qualified `rusqlite::params!`.

**Steps:**
- [ ] Write the failing tests FIRST in `src-tauri/tests/storage.rs` (add new top-level test fns alongside the existing `record_...` and `reopens_an_existing_database` tests; reuse the file's `temp_db_path` and `sample_session` helpers):
  - New test `spaces_upsert_find_delete_and_order` (needs a `std::thread::sleep` for ordering):
    - `Db::open` a fresh temp path; `list_spaces` → empty.
    - `upsert_space("/tmp/pa")`; `std::thread::sleep(Duration::from_millis(5))`; `upsert_space("/tmp/pb")`.
    - `list_spaces` → 2 rows; **row 0 is `/tmp/pb`** (most recently opened); for the `/tmp/pa` row (row 1): `created_at <= last_opened_at`.
    - `upsert_space("/tmp/pa")` again (after another 5 ms sleep) → `list_spaces` row 0 is now `/tmp/pa` (a re-touch wins ordering).
    - `find_space("/tmp/pa")` → `Some("/tmp/pa")`; `find_space("/nope")` → `None`.
    - `delete_space("/tmp/pb")` → `list_spaces` back to 1 row; `upsert_space("/tmp/pb")` again → it reappears with a fresh `created_at`.
  - New test `open_backfills_space_rows_from_existing_sessions`:
    - `base` = a fresh unique dir (e.g. `std::env::temp_dir().join(format!("archimedes-spaces-bk-{}", uuid::Uuid::new_v4()))`); inside it `create_dir_all` `cwd_a` and `cwd_b` (they MUST exist for canonicalization); `db_path = base/"archimedes.db"`.
    - `db1 = Db::open(&db_path)`; `record_session` 3 sessions (distinct ids): two with `cwd = cwd_a` (the `SELECT DISTINCT` must collapse them) and one with `cwd = "<base>/gone-<fresh uuid>"` — a path string for a directory that was NEVER created (backfill must skip it without failing `Db::open`).
    - `db2 = Db::open(&db_path)` (a second open of the same file — the backfill runs; `db1` is still open, which is fine: the backfill only writes `INSERT … DO NOTHING`). `db2.list_spaces()` → **exactly 2 rows** (the canonical forms of `cwd_a` and `cwd_b`); `find_space(std::fs::canonicalize(&cwd_a).unwrap().display().to_string())` → `Some(…)`; `find_space("<base>/gone-…" as stored)` → `None`.
    - `db3 = Db::open(&db_path)` → `list_spaces` still exactly those 2 rows (idempotent backfill), and their `last_opened_at`/`created_at` values are UNCHANGED from `db2`'s read (the `DO NOTHING` form must not refresh recency — capture the values after `db2` and assert equality after `db3`).
- [ ] Run `cd src-tauri && cargo test --test storage` — did the new tests fail (methods / `SpaceRow` export missing)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the changes above in `db.rs`, and extend the `storage/mod.rs` re-export with `SpaceRow`.
- [ ] Run `cd src-tauri && cargo test --test storage` — all storage tests pass (existing + new)?
- [ ] Run `cd src-tauri && cargo test` — ALL pass (the pre-existing 21 tests: 8 acp_flow + 5 fs_backend + 2 storage + 1 ipc + 5 lib unit — matching the verified 21/21 baseline — plus the 2 new storage tests = 23 total)?
- [ ] `cargo fmt` then `cargo clippy -- -D warnings` then `cargo build` — all clean?
- [ ] Commit with message: `feat: spaces table — schema, upsert/list/find/delete, open-backfill`

**Acceptance criteria:**
- [ ] A fresh database gains a `spaces` table with no migration step.
- [ ] A pre-existing database (sessions already stored) gets canonical space rows for its existing session cwd folders at next open; vanished folders are skipped silently, and the backfill never refreshes `last_opened_at` (`DO NOTHING` — recency stays whatever real starts/resumes set).
- [ ] `last_opened_at` ordering is set by `upsert_space` alone and survives repeated opens; `delete_space` touches only the `spaces` table (the row can be recreated by a later upsert).

---

### Task 3: One-live session policy in the SessionManager (+ canonical cwd)

**Context:**
Two decisions from the design (see `docs/decisions/0002-one-live-acp-session.md`): (1) at most ONE live session app-wide — a new start/resume closes any live session first, with a new `replaced` close reason, so the closed space shows a "Paused — replaced" banner (generated by the UI in Task 6); (2) session `cwd` becomes the space's canonical path — canonicalized in `start_session`/`resume_session` (before agent spawn, and before the space row is touched), so the `spaces` join (`spaces.path = session.cwd`) is consistent even if the user typed a `~/x` or symlink spelling. The one-live policy is the app-side workaround for the unresolved two-session SDK hang (never re-open that investigation in this task).

**Files:**
- Modify: `src-tauri/src/acp/errors.rs`
- Modify: `src-tauri/src/acp/session.rs`
- Modify: `src-tauri/src/bin/fake_agent.rs` (test double only — see step 3 below; default behaviour must remain byte-identical so the 8 existing acp_flow tests keep passing unchanged)
- Test: `src-tauri/tests/acp_flow.rs`

**What to implement:**

1. `errors.rs` — add one variant to `AcpError` (derive already carries `Serialize` with `snake_case` tags and `thiserror`):

```rust
/// The requested working directory does not exist (or is not a folder).
#[error("folder not found: {path}")]
FolderMissing { path: String },
```

2. `session.rs` — close-reason mechanism. Current code: `drive_session` creates `let user_closed = Arc::new(AtomicBool::new(false))` (+ a replica capture for the closure), the closure's `select!` sets it on `close_rx.changed()`, and after `connect_with` returns the task derives `User`/`AgentExited` from it. Replace that mechanism:

   **Compiler notes (do not skip):** `CloseKind` needs `#[derive(Debug, Clone, Copy)]` — `Debug` because the `LiveSession` struct derives `Debug` and would not compile otherwise; `Clone, Copy` because the kind read is `let kind = *close_kind.lock().expect(..);` — dereferencing a `MutexGuard` moves the `Option<CloseKind>` out of it, which only type-checks when `CloseKind` is `Copy` (a `let kind = close_kind.lock().expect(..).clone();` works too; pick one — but the guard must be held until AFTER the read). The `close_kind` Arc must be held by the driver task until AFTER the cleanup's kind read. Deleting `user_closed` orphans `use std::sync::atomic::{AtomicBool, Ordering};` — remove that import. Extend the existing config import to `use crate::config::{AgentEntry, ConfigError, Registry};` (the entries getter in item 6 uses `AgentEntry`/`ConfigError`, and `ConfigError` is already referenced by `start_session`).

- Add a private enum near `ClosedReason`:

```rust
/// How a live session was (or was about to be) closed. `None` at
/// teardown time means the agent process exited on its own.
#[derive(Debug, Clone, Copy)]
enum CloseKind {
    /// An explicit user close (`close_session`).
    User,
    /// Closed because another session started (one-live policy, ADR 0002).
    Replaced,
}
```

- Add a field to `LiveSession`: `close_kind: Arc<StdMutex<Option<CloseKind>>>` (import `StdMutex`/`Arc` — `std::sync` — already used in the file for the transcript accumulators). In `drive_session`: create `let close_kind: Arc<StdMutex<Option<CloseKind>>> = Arc::new(StdMutex::new(None));` and move it into the `LiveSession` value (alongside `close_tx`).
- Delete `user_closed` / `user_closed_for_closure` entirely. The `select!` arm on `close_rx.changed()` becomes a no-op arm (the closure simply returns; the reason is decided from `close_kind`, never from the closure).
- After `connect_with` returns, the task's reason derivation becomes:

```rust
let kind = *close_kind.lock().expect("close-kind mutex poisoned");
let reason = match kind {
    Some(CloseKind::User) => ClosedReason::User,
    Some(CloseKind::Replaced) => ClosedReason::Replaced,
    None => ClosedReason::AgentExited,
};
```
(everything else in the cleanup block — map removal, permission-drain prefix, `session-closed` emit with `reason.as_str()` — stays as-is).

- Add `ClosedReason::Replaced` variant with `/// Closed because another session started (one-live policy)`, and extend `as_str` with `ClosedReason::Replaced => "replaced"`.
- `close_session` (explicit user close): after cloning `close_tx`, BEFORE `close_tx.send(true)`, also set the kind if not already set:
  `*kind.lock().expect(..) = Some(CloseKind::User)` only when it is still `None` (first-set-wins on clone: a kind already present means the close is in progress or the reason is already decided). Read the kind through the CLONE of the `LiveSession`'s `close_kind` Arc — acquire it in the same map-lock scope as the `close_tx` clone, then set the value after dropping the map lock (the kind mutex is always unlocked when you touch it, so no relay borrow is needed). Keep the existing `SendError` handling.
- New public method on `SessionManager`:

```rust
/// One-live policy (ADR 0002): initiate a `replaced` close of every live
/// session (best-effort flag send; the actual teardown runs concurrently
/// in the superseded driver tasks). Called at the top of `start_session`
/// and `resume_session`, BEFORE the new agent is spawned, so the steady
/// state is at most one live session at a time.
pub async fn supersede_live_sessions(&self)
```
Implementation: lock `self.sessions` (the existing inner `Arc<Mutex<HashMap>>`); for every entry, set `close_kind` to `Some(CloseKind::Replaced)` if it is still `None` (first-set-wins: a kind already present means the reason is already decided), then call `close_tx.send(true)` and ignore `SendError` (the target may already be closing); same effect if the map is empty. Keep the borrow under the lock short (collect `(&SessionId, sender, kind)`-style triples first, drop the map lock, then send — either order is fine as long as the map lock is NOT held across an `.await`).

Semantic note (keep the docs honest): the supersede SETS the close flag; the superseded session's driver task tears down concurrently with the new session's establishment, so a brief window where two ACP transport tasks coexist is **accepted in v1** (the motivating two-session-hang scenario — two sessions *prompting at the same time* — cannot happen: one of them is a tearing-down zombie mid-flight). Do not promise a stronger guarantee in code comments or docs. See also the test's step-3/5 ordering note (the count assert waits for the close EVENT, which lands after the map removal — do not assert the count right after the superseding `start_session` returns).

- `start_session` and `resume_session`: as their FIRST action after the `registry.get` check, call `self.supersede_live_sessions().await`. (If the map holds only sessions of the same cwd this is a near no-op, but it must run unconditionally — the policy is app-wide.)
- Canonicalize in both `start_session` and `resume_session`, ALSO immediately before the registry lookup:

```rust
let cwd = std::fs::canonicalize(&cwd)
    .map_err(|_| AcpError::FolderMissing {
        path: cwd.display().to_string(),
    })?;
```
(`resume_session`'s `PathBuf` parameter is renamed/bound just like `start_session`.) Everything downstream (`NewSessionRequest::new(cwd_owned)`, `load_session`, `SessionInfo.cwd`, `record_session`, `FsBackend` root) then uses the canonicalized value — no other code changes.
- `record_session` (the private persistence hook called at the end of `start_session`/`resume_session`): inside its existing `if let Some(db) = &self.db`, after the `record_session` call, add:
  `let _ = db.upsert_space(&info.cwd.display().to_string());`
  (a start/resume UPDATES or creates the space row, and `resume` re-touches `last_opened_at` — design rule: a space is born/touched when a conversation starts or resumes in it.)
- Add a small public getter (used by Task 4's `list_agents` command):
  `pub fn agents(&self) -> &[AgentEntry] { &self.registry.agents }`
- Update the doc comments that describe the `User`/`AgentExited` derivation (the "the closure sets the flag on a user close" paragraph) so they describe the `close_kind` mechanism.

Do NOT touch: `send_prompt`, `respond_permission`, `permission.rs`, `fs_backend.rs`. (The fake agent binary IS modified — env-only, see the first step — nothing user-facing.)

**Steps:**
- [ ] Extend the fake agent (env-only, default behaviour must remain byte-identical so the 8 existing acp_flow tests keep passing unchanged): in `src-tauri/src/bin/fake_agent.rs`, replace `const SESSION_ID: &str = "fake-session-1"` with:
  ```rust
  /// Session id this agent reports: an explicit `FAKE_SESSION_ID` env
  /// override, or the default `fake-session-1` (existing tests rely on it).
  fn session_id() -> String {
      std::env::var("FAKE_SESSION_ID").unwrap_or_else(|_| "fake-session-1".to_string())
  }
  ```
  and update the 4 use-sites (the `session/new` result, the `session/load` result, the `session/update` chunk params, and the `session/request_permission` params) to use `session_id()`.
  Rationale: the `sessions` map is keyed by the agent-reported session id; if both test sessions reported the same id, the second session's map insert could stomp the superseded session's cleanup. Distinct env-per-entry ids make the test unambiguously well-defined.
- [ ] Write the failing test in `src-tauri/tests/acp_flow.rs` — `fn one_live_supersede_policy()`, next to the existing fake-agent tests (reuse `temp_config_dir`, `unique_fake_agent`, the existing `TestSink`/`wait_for_events`/`find_fake_agent_pid` helpers):
    *Setup.* `temp_config_dir` + two unique copies of the fake agent via `unique_fake_agent` (`bin_a`, `bin_b` — DISTINCT paths are needed so the teardown-reap assertions can discriminate); write `agents.json` with two entries:
    ```json
    { "agents": [
      { "id": "l1", "name": "L1", "command": "<bin_a>", "args": ["resume"], "env": { "FAKE_SESSION_ID": "s1" } },
      { "id": "l2", "name": "L2", "command": "<bin_b>", "args": ["resume"], "env": { "FAKE_SESSION_ID": "s2" } }
    ] }
    ```
    `args: ["resume"]` is deliberate: in `resume` mode the fake answers `session/new` (it is not `hang` mode), advertises `loadSession: true`, answers `session/load`, and answers `session/prompt` with its default handler — ONE mode covers both `start_session` and `resume_session` (see the fake-agent module docs). `env` is delivered via the existing `AgentEntry.env` pass-through. Then `SessionManager::new(config_dir)` + `attach_db` (temp DB) + `set_establish_timeout(Duration::from_secs(2))` + two REAL temp dirs `cwd_a`/`cwd_b` (`create_dir_all`).
    *Steps (in order — sequencing matters, teardown is async):*
    1. `start_session("l1", <nonexistent path (base)/nope-<uuid>>, &sink)` → `Err` matching `AcpError::FolderMissing { .. }` — and `session_count()` stays 0 (nothing spawned, nothing superseded).
    2. `start_session("l1", cwd_a, &sink)` → `Ok`. `session_count()` == 1.
    3. `start_session("l2", cwd_b, &sink)` → `Ok.` (Do NOT assert a count here — see the sequencing liability below: the supersede of s1 is an async flag send, and s2 is already inserted into `sessions` before s1's driver has had a chance to run its cleanup and remove s1, so `session_count()` may still read 2 for a short window.)
    4. `wait_for_events` until a `session-closed` with `sessionId == "s1"` and `reason == "replaced"` arrives (budget ≤ 5 s; if events overshoot — an `s1` close + the s2 establishment can race — keep scanning until that exact pair is seen). This event is emitted in s1's driver task *after* s1 has been removed from the `sessions` map, so observing it guarantees s1 is gone.
    5. `session_count()` == 1 (s1 gone, s2 present). **This assert belongs here — AFTER the s1 close event — NOT directly after step 3, because the map removal races the s2 insertion.**
    6. `send_prompt("s2", "hi")` → `Ok` (the new live session is fully functional after the supersede; the default-mode prompt handler streams two chunks before `end_turn` — the chunk events may arrive interleaved, which is fine).
    7. `close_session("s2")` → `Ok`; **wait** for the `session-closed` with `sessionId == "s2"` and `reason == "user"` BEFORE asserting anything about counts (teardown is an async flag send); then `session_count()` == 0.
    8. `resume_session("l1", "s1", cwd_a, &sink)` → `Ok` (a superseded session is re-resumable — `loadSession` is true because of the `resume` mode); step 7 closed everything, so there is no live session to supersede (the policy's no-op branch) and s1 establishes cleanly → `session_count()` == 1.
    9. `start_session("l2", cwd_b, &sink)` → `Ok`; `wait` for a SECOND `session-closed` with `sessionId == "s1"` and `reason == "replaced"` — `start_session` supersedes a *resumed* session too; **then** (after the event) `session_count()` == 1.
    10. `close_session("s2")` → `Ok`; wait for its `session-closed` (`"user"`); `session_count()` == 0.
    11. **Process teardown proof:** the two agents above are DISTINCT bins (`unique_fake_agent` called twice → `bin_a` ≠ `bin_b`), so poll `find_fake_agent_pid(&bin_a)` AND `find_fake_agent_pid(&bin_b)` until BOTH return `None` (deadline 10 s; the same `kill(pid, 0)`-on-ESRCH idiom the existing reap check uses) — the superseded/closed fake processes were actually reaped, none leaked. (Two DISTINCT paths is deliberate: `unique_fake_agent` gives each agent its own copy, so the two predicates discriminating the two processes is what makes this a real leak check rather than a single shared-binary assertion.)
  - Keep the existing 8 `acp_flow` tests untouched and passing — the env-only fake change keeps their behaviour byte-identical; if any of them breaks, stop and investigate (you changed something the default path depends on).
- [ ] Run `cd src-tauri && cargo test --test acp_flow` — did the new test fail (no `FolderMissing` / `Replaced` / `supersede_live_sessions` yet)? If the new test passed unexpectedly, stop and investigate why.
- [ ] Implement the changes above in `session.rs` + `errors.rs` (+ the same fake-agent env change if you wrote the test first, per TDD order: the env change is a PREREQUISITE of the test, so it may already be done — do not duplicate it).
- [ ] `cd src-tauri && cargo test` — all pass (the pre-existing 23: 8 acp_flow + 5 fs_backend + 2 storage + 1 ipc + 5 lib unit, plus Task 2's 2 new storage tests; + the 1 new acp_flow test = 24 total)?
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo build` — clean.
- [ ] Commit with message: `feat: one-live session policy (replaced close reason) + canonical cwd`

**Acceptance criteria:**
- [ ] Starting or resuming a session INITIATES a `replaced` close of any live session before spawning the agent (the `session-closed`/`"replaced"` event is observed, and never more than one live session at a time — the steady state the ADR 0002 paperwork is written for; the teardown concurrency window is by design, see the semantic note above).
- [ ] A superseded session is fully resumable afterwards (the test exercises `resume_session` after a supersede).
- [ ] `start_session`/`resume_session` with a non-existent folder return the `folder_not_found` error (diagnostic on the wire: `kind: "folder_missing"`) instead of spawning.
- [ ] Existing `agent-exited` detection (the existing kill-the-agent test) still passes — the `None` arm of `close_kind` still maps to `AgentExited`.

---

### Task 4: Command surface — list_agents / list_spaces / delete_space / space_for_path

**Context:**
The UI needs four new IPC entry points (design decisions: agent dropdown is registry-driven with the first entry as default; the dialog needs to know whether the chosen folder is already a space, and wants its canonical path for the store's `spaces` list). The commands are thin: one delegates to `SessionManager::agents()` (added in Task 3) and three to the `Db` methods from Task 2. `space_for_path` is the canonicalization gate the dialog runs against the user's typed/picked path (the backend re-canonicalizes at start anyway — double safety, design rule from Task 3).

**Files:**
- Create: `src-tauri/src/commands/spaces.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/tests/ipc.rs`

**What to implement:**

1. `commands/spaces.rs` (new file):

```rust
//! Tauri commands for the space and agent registries (read-only views plus
//! the dialog's folder check). See `docs/roadmap/spaces.md` Task 4.
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::acp::SessionManager;
use crate::storage::Db;

/// A registry entry (camelCase over IPC) — the agent dropdown's data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEntryDto {
    pub id: String,
    pub name: String,
}

/// `list_agents` — every configured agent (v1 default registry: single
/// `pi` entry). Powers the dialog dropdown; default selection is
/// `agents[0]` (a decision in the frontend: `pi` first, no `fake` default).
#[tauri::command]
pub async fn list_agents(state: State<'_, Arc<SessionManager>>) -> Result<Vec<AgentEntryDto>, String>
```
(implementation: `state.agents().iter().map(|e| AgentEntryDto { id: e.id.clone(), name: e.name.clone() }).collect()`, wrapped in `Ok(…)`) — no `io`/`json` failure can occur on this path; the `Err` arm is defensive only:

```rust
/// The folder check + canonicalizer for the new-space dialog.
///
/// Returns the CANONICAL path and whether a space row already exists for
/// it. A typed `~/x` or a missing folder is an ERROR ("no such folder"),
/// not a `None`: the dialog shows it inline instead of letting a broken
/// path travel to `start_session`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCheck {
    pub canonical_path: String,
    pub is_space: bool,
}

#[tauri::command]
pub async fn space_for_path(
    state: State<'_, Arc<Db>>,
    path: String,
) -> Result<SpaceCheck, String> {
    let canonical = std::fs::canonicalize(&path)
        .map_err(|_| format!("no such folder: {path}"))?;
    if !canonical.is_dir() {
        return Err(format!("not a folder: {path}"));
    }
    let p = canonical.display().to_string();
    let is_space = state.find_space(&p).map_err(|e| e.to_string())?.is_some();
    Ok(SpaceCheck { canonical_path: p, is_space })
}

/// All spaces, most recently opened first.
#[tauri::command]
pub async fn list_spaces(state: State<'_, Arc<Db>>) -> Result<Vec<crate::storage::SpaceRow>, String> {
    state.list_spaces().map_err(|e| e.to_string())
}

/// "Forget this space" — deletes the bookkeeping row only (conversations
/// stay stored; design decision). No-op if the row is already gone.
#[tauri::command]
pub async fn delete_space(state: State<'_, Arc<Db>>, path: String) -> Result<(), String> {
    state.delete_space(&path).map_err(|e| e.to_string())
}
```

2. `commands/mod.rs`: add `pub mod spaces;` next to the existing `pub mod`s.

3. `lib.rs`: add the 4 commands to the `tauri::generate_handler!` list in `run()` (append after `commands::settings::save_settings`).

4. `tests/ipc.rs`: the `build_app` helper ALREADY runs the REAL setup — it calls `setup_dirs` (the same function `run()`'s setup closure calls, verbatim, on the built mock app), which manages the `Arc<dyn EventSink>` (a `TauriSink` on `app.handle()`), the `Arc<SessionManager>`, and the `Arc<Db>`; it then SWAPS the managed sink for an observable `TestSink` (the `unmanage` + re-`manage` idiom with a `swapped` assertion) whose constructor takes an `events: mpsc::Sender<(String, serde_json::Value)>` — so the test gets BOTH the full real manager wiring AND event observation with NO extra `app.manage`. The only change to `build_app` itself: append the 4 new commands to its `generate_handler!` list (mirror `lib.rs`'s `run()` handler list). Then add a NEW test named `spaces_and_agents_commands_round_trip`, structured exactly like `history_settings_and_resume_commands_round_trip` (temp config/app-data dirs, `write_agents_json(config_dir)`, `std::sync::mpsc::channel()` → `build_app(cfg, dat, tx)`, `WebviewWindowBuilder` webview, the file's `invoke` helper). Body (every `snake_case` command name is the wire name; args camelCase via the Tauri mapping):
    1. `invoke(space_for_path, {path: <base>/newproj (create_dir_all'd real dir)}` → JSON `{ canonicalPath: ..., isSpace: false }` — assert `canonicalPath` equals `std::fs::canonicalize` of that dir (on a symlink-free temp dir canonical == the passed path; the assertion additionally guards against a platform whose temp dir IS symlinked — in that case the returned `canonicalPath` is the resolved target, which is the behaviour we want to observe, not the one to fail on).
    2. `invoke(list_agents)` → an array with **exactly** `[{ "id": "fake", "name": "Fake Agent" }]` (the test's `write_agents_json` registry has one entry), camelCase keys.
    3. `invoke(start_session, {agentId: "fake", cwd: <base>/newproj})` → `Ok`; record that session's `sessionId` (= `FAKE_SESSION_ID` with the unmodified default fake).
    4. `invoke(list_spaces)` → exactly **1** row, `path` == the step-1 `canonicalPath` (the `record_session` → `upsert_space` hook from Task 3 wrote it).
    5. `invoke(space_for_path, {path: <base>/newproj})` again → `isSpace: true` (step 4's row made it a space).
    6. `invoke(delete_space, {path: canonicalPath})` → `Ok`; `invoke(list_spaces)` → `[]`.
    7. `invoke(list_sessions)` → **still 1** row (deleting the space did NOT touch the stored session), `sessionId` matches step 3.
    8. `invoke(space_for_path, {path: <nonexistent path (base)/nope-<uuid>}` → error response: use `get_ipc_response(…).expect_err(…)` (the `invoke` helper's `.expect` makes this impossible) and assert the error string contains `"no such folder"` — mirror the file's existing NotResumable error-check pattern (the `err.to_string()` contains-check) word for word.
    9. **Cleanup (leak prevention — mirror `history_settings_and_resume_commands_round_trip`'s teardown tail VERBATIM, `ipc.rs:`):** `invoke(&webview, "close_session", json!({"sessionId": <the fake id from step 3>}))`, then **poll the `events_rx`** (the `TestSink` `build_app` swapped into the managed sink delivers every `session-closed` on it) with the file's exact `recv_timeout` loop — `Ok((event, _)) if event == "session-closed" => break`, `Ok(_) => continue`, `Err => panic!` — until it fires (proves the driver task observed the close and tore the agent down, so the process is reaped before the test ends). (No `find_fake_agent_pid` here — that helper lives in `acp_flow.rs`, not `ipc.rs`; the `session-closed` event wait IS `ipc.rs`'s reap evidence, exactly as `history_…` uses it.)

**Steps:**
- [ ] Write the failing test in `tests/ipc.rs` first (commands missing → test fails to compile or `list_agents` `404`s).
- [ ] `cd src-tauri && cargo test --test ipc` — did it fail as expected? Then implement the 4 commands + handler registrations.
- [ ] `cd src-tauri && cargo test` — everything green (25 tests = 24 + the 1 new ipc test).
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo build` — clean.
- [ ] Commit with message: `feat: command surface for spaces + agents (list_agents, list_spaces, delete_space, space_for_path)`

**Acceptance criteria:**
- [ ] `list_agents` returns the registry (camelCase `id`+`name`) — the dialog's dropdown data, default = first entry.
- [ ] `space_for_path` returns the canonical path + `isSpace`; errors on missing/non-folder paths; the same path is canonicalized identically by `start_session` (Task 3) → the dialog's `spaces` update and `session.cwd` match on the wire.
- [ ] `delete_space` deletes the row only — a follow-up `list_sessions` in the same test (optional assertion) shows conversations are untouched.

---

### Task 5: Frontend — types, store, pure helpers

**Context:**
The store is the substrate for the UI reskin (Task 6). It needs: the 4 new IPC wrappers + types; the `replaced` close reason in the payload union; a `spaces` slice with `setSpaces` (boot) / `addSpace` (after a successful start) / `removeSpace` (the "forget this space" action); per-session close reasons (recorded on `session-closed` so the UI can render the "replaced" banner copy); and `spaces`-grouping helpers extracted as pure functions so they are unit-testable with the repo's existing Vitest pattern (current tests cover pure reducers only — no `@tauri-apps/api` invoke in unit tests).

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/store/sessions.ts`
- Create: `src/lib/paths.ts`
- Test: `src/lib/paths.test.ts` (new)
- Test: `src/store/sessions.test.ts` (extend)

**What to implement:**

1. `src/lib/tauri.ts`:
- `SessionClosedPayload.reason`: extend the union to `"user" | "agent-exited" | "error" | "replaced"` (and add `export type CloseReasonStr = "user" | "agent-exited" | "error" | "replaced";` and have `SessionClosedPayload` use it).
- Add mirror types:

```ts
/** A registry entry over IPC (camelCase). */
export interface AgentEntryDto {
  id: string;
  name: string;
}

/** A space bookkeeping row (camelCase over IPC). */
export interface SpaceRow {
  path: string;
  createdAt: number;
  lastOpenedAt: number;
}

/** The folder-check result (camelCase over IPC). */
export interface SpaceCheck {
  canonicalPath: string;
  isSpace: boolean;
}
```

- Add wrappers (same `invoke` style as the rest of the file): `listAgents(): Promise<AgentEntryDto[]>` (`list_agents`), `listSpaces(): Promise<SpaceRow[]>` (`list_spaces`), `deleteSpace(path: string): Promise<void>` (`delete_space`, arg `{ path }`), `spaceForPath(path: string): Promise<SpaceCheck>` (`space_for_path`, arg `{ path }`).

2. `src/lib/paths.ts` (new):

```ts
/**
 * Folder display label: the base name of a (canonical, absolute) path.
 * Handles both `/` and `\` separators (Windows cwd strings).
 * Returns `""` for empty input or bare roots — the caller falls back to
 * the full path.
 */
export function basenameOfPath(p: string): string {
  if (p === "") return "";
  const trimmed = p.replace(/[\\/]+$/, "");
  if (trimmed === "") return "";
  const parts = trimmed.split(/[\\/]/);
  const last = parts[parts.length - 1];
  return last === "" ? "" : last;
}
```

3. `src/lib/paths.test.ts`: `basenameOfPath` cases — `"/a/b" → "b"`, `"C:\\x\\y" → "y"`, `"/a/b/" → "b"`, `"/" → ""`, `"" → ""`, `"/home/daniel/Coding/AI/archimedes-desktop" → "archimedes-desktop"`.

4. `src/store/sessions.ts` — extend the store (interface + implementation), following the existing shape exactly:

- New state fields:
  ```ts
  /** Space bookkeeping rows (from `list_spaces` on boot; upserted by `addSpace`). */
  spaces: SpaceRow[];
  /** Close reason recorded on `session-closed`, per session id (for the "replaced" banner copy). */
  closeReasons: Record<string, CloseReasonStr>;
  ```
- New actions:
  ```ts
  /** Boot: replace the spaces list. If nothing is active yet, select the most recent
   *  space's most recent session (live preferred over stored) so a restart lands
   *  on the last space.
   */
  setSpaces: (rows: SpaceRow[]) => void;
  /** Single-space upsert after a successful `start_session` (from the dialog):
   *  existing row → refresh `lastOpenedAt`; missing row → append with `createdAt = lastOpenedAt = Date.now()`
   *  (approximate; the next boot's `list_spaces` corrects reality on the server side).
   */
  addSpace: (path: string) => void;
  /** "Forget this space": `delete_space` command + local removal. Does NOT touch
   *  `sessions`/`historySessions` (conversations stay stored — design decision).
   *  `activeSessionId` is left alone.
   */
  removeSpace: (path: string) => Promise<void>;
  ```
- **`addSession` — ONE deliberate change** (the rest of `addSession` stays as-is; the store keeps per-session state slices independently of spaces): the `activeSessionId` line changes from `state.activeSessionId ?? info.sessionId` to the UNCONDITIONAL `activeSessionId: info.sessionId` — a session that was just *started* (dialog or "new conversation") is the one the user wants to look at, so start flows always switch the view to it. (The live-session handling in `openSession` is unchanged.)
- `handleSessionClosed`: TWO changes to the existing implementation (currently it renames nothing and sets `activeSessionId: state.activeSessionId === sessionId ? null : state.activeSessionId`):
  1. Rename the `_reason` parameter to `reason: CloseReasonStr` and record it: `closeReasons: { ...state.closeReasons, [sessionId]: reason }` (merge — never delete keys: the reason outlives the event.
  2. Change the `activeSessionId` line to `activeSessionId: state.activeSessionId` (KEEP it — do NOT null it when the closed session was the active one). Rationale: a close is a *pause*, not a discard — the conversation moves to `historySessions` but stays the displayed one, so `ChatStream` renders its stored/paused banner (including the `replaced` copy from Task 6) instead of the `No active session` empty state. This is required for the Task 6 `Pause` behavior. (The pre-existing behavior of the other `handleSessionClosed` lines — moving the session to `historySessions`, `finalizeSessionMessages`, `inTurn` reset, permission dismissal — stays as-is.)
- Boot auto-select helper — extract as a PURE export so it is unit-testable (the store action wraps it):
  ```ts
  /** The session a fresh boot should land on: the most recently opened
   *  space's newest session, live (in `sessions`) preferred over stored.
   *  `null` when there's nothing. `spaces` arrives `lastOpenedAt`-desc (from
   *  `list_spaces`) and `historySessions` newest-first (from `list_sessions`),
   *  so NO `createdAt` field is needed — see the ordering note below. */
  export function autoSelectActive(
    spaces: SpaceRow[],
    sessions: SessionInfo[],
    historySessions: SessionInfo[],
  ): string | null;
  ```
  **Ordering note (read first):** `SessionInfo` over IPC has NO `createdAt` field (`src/lib/tauri.ts` is only `{ sessionId, agentId, cwd, capabilities }`; the Rust `list_sessions` command drops `created_at` when mapping `SessionRow → SessionInfo`, and the acp `SessionInfo` struct has no such field). So we cannot sort by `createdAt`. Instead we rely on SERVER order: `Db::list_sessions` already returns `ORDER BY created_at DESC, id DESC` (`storage/db.rs`) and `Db::list_spaces` returns `ORDER BY last_opened_at DESC, path ASC`, and the store slices only `filter`/`map` those rows (which preserves order). Therefore "newest-first" **is** the input order — `spaces[0]` is the most recently opened space, and within a space the head of its `historySessions` subsequence is its newest stored session. Do NOT re-sort by a field that doesn't exist.
  **Implementation:** iterate `spaces` in input order (already `lastOpenedAt` desc — no re-sort). For each space: `liveId` = the `sessions` entry with `cwd === space.path` (at most one live app-wide, `null` if none); `storedSub` = `historySessions.filter(s ⇒ s.cwd === space.path).map(s ⇒ s.sessionId)` (input order = newest-first). If `liveId` → return it. Else if `storedSub.length > 0` → return `storedSub[0]` (the newest in that space). Else continue to the next space. `null` if no space has a session.
  `setSpaces` uses it: `activeSessionId: activeSessionId === null ? autoSelectActive(rows, sessions, historySessions) : activeSessionId` — AND because `autoSelectActive` lands on a *stored* session (live ones never survive restart), the implementation additionally calls `get().openSession(selectedId)` when the selection is non-null: that loads the stored transcript into the initial view (no-op for live/already-loaded sessions; the fetch is self-guarding against a mid-flight switch, so boot lands on *content*, not an empty chat).

- **Grouping helpers** — ALSO pure exports (used by both tasks 5's tests and task 6's components):
  ```ts
  export interface SpaceView {
    path: string;
    /** Display label (base name; `""` if the base name is empty — the UI falls back to `path`). */
    title: string;
    /** The live session in this space (there is at most one app-wide), or `null`.
     *  `null` can occur for a space that has no live session.
     */
    liveSessionId: string | null;
    /** Stored sessions of this space, in `historySessions` order (newest-first
     *  as delivered by `list_sessions` — `SessionInfo` has no `createdAt`). */
    storedSessionIds: string[];
    /** Close reason of the space's newest session (live one preferred, else the head of the stored subsequence), if any. */
    lastReason: CloseReasonStr | undefined;
  }
  export function spaceViewFor(
    space: SpaceRow,
    sessions: SessionInfo[],
    historySessions: SessionInfo[],
    closeReasons: Record<string, CloseReasonStr>,
  ): SpaceView;
  ```
  Implementation (same input-order rule as `autoSelectActive`): `liveSessionId` = the `sessions` entry with `cwd === space.path` (`null` if none); `storedSessionIds` = `historySessions.filter(s ⇒ s.cwd === space.path).map(s ⇒ s.sessionId)` **in `historySessions` order, NOT re-sorted** (it is already newest-first from `list_sessions`); `lastReason` = `closeReasons[liveSessionId ?? storedSessionIds[0]]` (the newest session = the live one, else the head of the stored subsequence). `title = basenameOfPath(space.path)`.

- **Unknown session handling**: DO NOT add a "session without a space" group to `SpaceView` (design decision: a space is born at the same time as a session — the `record_session → upsert_space` hook in Task 3 guarantees this; a session references a space row on boot after backfill, the only edge being a legacy path where the row doesn't exist yet — a trivial `addSpace` call in the dialog covers it; no UI treatment needed).

5. `src/store/sessions.test.ts` — add tests (import the new exports from `./sessions` and `../lib/paths`). Build fixtures with EXACTLY the four `SessionInfo` wire fields (`{ sessionId, agentId, cwd, capabilities }` — **do NOT invent a `createdAt`; it does not exist on the wire**) plus `SpaceRow` rows `{ path, createdAt, lastOpenedAt }` (`SpaceRow` genuinely does carry `lastOpenedAt` — that's the `spaces` table, not `sessions`):
- `spaceViewFor`: two fake spaces — one with a live session plus a stored session whose `closeReasons[thatId] = "replaced"`, the other with two stored sessions passed in a specific order (assert `storedSessionIds` is returned in THAT input order — e.g. pass `historySessions` in the order `[idX, idY]` and expect `[idX, idY]`; this proves no re-sort by a nonexistent field); a third space with no sessions at all; assert the view shape (title labels, `liveSessionId`, `storedSessionIds` = input order, `lastReason` = `undefined` for the empty space and `"replaced"` for the live one when the head of its subsequence has that recorded reason).
- `autoSelectActive` (pass `spaces` in `lastOpenedAt` desc — that is input order, the function must NOT re-sort): (a) `[]`/`[]`/`[]` → `null`; (b) the first space has a live session (store `sessions` with it) → live id; (c) first space has no live but has stored sessions → the **head** of that space's stored subsequence (pass `historySessions` ordered `[newest, older]` and assert it returns `newest`); (d) first space has no sessions at all, second space has stored → the second space's head (skip-and-advance); (e) two spaces with stored sessions → the FIRST space's head (input/recency order wins), even if the second space's session is newer.
- `basenameOfPath` (in `paths.test.ts`, separate file as defined above).
- Do NOT unit-test the `removeSpace` store action (its `delete_space` invoke is unavailable in the unit env — same as the existing repo pattern). `addSpace` is a pure local upsert (no invoke): it is exercised through `NewSpaceDialog`'s flow in Task 7 and is trivial enough to skip unit coverage; `setSpaces` is covered by the `autoSelectActive` tests above (its only non-trivial logic IS that helper).

**Steps:**
- [ ] Write the failing tests first — `src/lib/paths.test.ts` (new file) and the new `describe` blocks in `src/store/sessions.test.ts`. The new store exports don't exist yet, so those tests FAIL at run (vitest does not stop on the missing named import at build time — the import of the undefined export throws at module load: the expected red).
- [ ] `pnpm test` — did the new tests fail (missing exports)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement the changes above (`tauri.ts`, `paths.ts`, `sessions.ts`).
- [ ] `pnpm test` — all pass.
- [ ] `pnpm build` — tsc passes (type-check across the entire app; if a pre-existing file referenced something you renamed — e.g. `SessionClosedPayload` — fix that call site now and log it).
- [ ] Commit with message: `feat: frontend — spaces store, close reasons, IPC wrappers, pure helpers`

**Acceptance criteria:**
- [ ] `pnpm test` and `pnpm build` both green.
- [ ] The store now holds `spaces` + `closeReasons`; `session-closed` events with `reason: "replaced"` are recorded per session.
- [ ] `autoSelectActive`/`spaceViewFor` are pure and tested — the UI in the next task is only wiring.

---

### Task 6: UI reskin — Spaces sidebar, new-space dialog, chat pane

**Context:**
The visual layer of the design (decisions approved: 1 space = 1 active conversation in v1; the sidebar becomes space rows; the dialog's agent becomes a registry-driven dropdown with the first entry as default, replacing the broken `fake` default; the chat pane gains a per-space conversation selector + "new conversation"/"pause" actions; the close-reason `replaced` renders the "other space is running" banner copy). All plumbing is present from Task 5; this task is pure React wiring + copy.

**Files:**
- Rename (`git mv` + rewrite): `src/components/SessionList.tsx` → `src/components/SpacesList.tsx`
- Rename (`git mv` + rewrite): `src/components/NewSessionDialog.tsx` → `src/components/NewSpaceDialog.tsx`
- Modify: `src/components/ChatStream.tsx`
- Modify: `src/App.tsx`

**What to implement:**

1. `src/components/SpacesList.tsx` (full rewrite of the old `SessionList` reskinned to spaces):
- Header: `Spaces` + a `+ New space` button (opens `NewSpaceDialog`).
- Data: `useSessions` selectors for `spaces`, `sessions`, `historySessions`, `closeReasons` (each a primitive/`[]`-stable selector in the existing style), then `views = useMemo(() => spaces.map((s) => spaceViewFor(s, sessions, historySessions, closeReasons)), [spaces, sessions, historySessions, closeReasons])` — keep the `spaces` list order (already `lastOpenedAt` desc from `list_spaces`) so the view rows follow the server's recent-first order.
- Row per space (a `SpaceView` from `spaceViewFor`):
  - Left: a status dot — `●` (green, `text-emerald-400`) when `view.liveSessionId !== null`, else `○` (`text-neutral-600`); the row's `title` attribute (hover tooltip): live → `Live — <agentId>`; otherwise by `view.lastReason` (this is where `lastReason` is consumed): `replaced` → `Paused — another space started a conversation`, `user` → `Paused`, `agent-exited` → `The agent exited`, `error` → `Ended in an error`, no recorded reason (normal on boot) → `Stored`. Then the display label: `view.title !== "" ? view.title : view.path` (the `basenameOfPath` contract returns `""` for empty roots — fall back to the full path), second line: the full `path` (truncated, `title` attribute = full path).
  - On `onClick`/`Enter` (keep the existing `role="button"` pattern): select the space's active conversation — `const target = view.liveSessionId ?? view.storedSessionIds[0] ?? null; if (target) openSession(target);` (guard: no session at all → the space has just been created and `start` hasn't completed yet; in that case do nothing on click, it's a transient state).
  - On `hover` (mirror `SessionItem`'s `hidden group-hover:block` ✕ pattern): a **remove-space** button — `title="Forget this space (conversations stay stored)"` — calls `removeSpace(view.path)`; rendered only when `view.liveSessionId === null` (you can't forget a space's row while its live session is in flight; stale but live sessions are closed first — the space becomes stored afterwards and can be removed).
- Empty state: `No spaces yet. Open a folder to get started.`

2. `src/components/NewSpaceDialog.tsx` (full rewrite of the old `NewSessionDialog` reskinned to a space flow — keep the outer shade/panel layout and Tailwind classes as-is):
- On mount: `useEffect` → `listAgents()` → `setAgents` (on failure: `agentsError = <message>`, the agent field shows the error and Start stays disabled). `selectedAgentId` state defaults to `""`, and the effective value is `const effectiveAgentId = selectedAgentId || agents[0]?.id || ""` — the first registry entry IS the default (the fix for the broken `"fake"` default: on the default registry that is `pi`); `Start` is disabled while `effectiveAgentId === ""` or `cwd` is empty or `folderError` is set.
- Agent field: `<select value={effectiveAgentId} onChange={(e) => setSelectedAgentId(e.target.value)}>` over `agents` (`value = id`, `label = name`, placeholder option `Select an agent…` shown while `selectedAgentId === ""`) replacing the free-text `input`.
- Folder field: keep the type-in + `Browse…` (existing `@tauri-apps/plugin-dialog` `open({ directory: true, multiple: false })` pattern, silent catch outside Tauri).
- Whenever the `cwd` field becomes non-empty (user edit OR `Browse` pick): `try { setCheck(await spaceForPath(cwd)); setFolderError(null); } catch (e) { setCheck(null); setFolderError(e instanceof Error ? e.message : String(e)); }` — Tauri rejects `Err(String)` commands with a PLAIN string, so `e.message` alone would be `undefined` for every invalid folder; the `String(e)` fallback (same idiom the existing dialog already uses for Start errors) keeps the message visible. `setCheck` stores `SpaceCheck`; `existingSpace = check?.isSpace`.
- Re-validate on the next `cwd` change (both `check` and `folderError` are replaced by the next `spaceForPath` outcome; an empty `cwd` clears both).
- Title: `existingSpace ? `New conversation in ${basenameOfPath(cwd)}` : "New space"` (use `basenameOfPath` from `../lib/paths`; fall back to the raw `cwd` if `basename` is `""`).
- Start: `startSession(effectiveAgentId, cwd)` (**`effectiveAgentId`, NOT the raw `selectedAgentId`** — on the default path `selectedAgentId` is `""` and `effectiveAgentId` is the pre-selected first entry; sending `selectedAgentId` would `UnknownAgent`-error on the acceptance run's exact click). The backend canonicalizes (Task 3); the dialog sends the raw typed/picked path. → on success:
  `addSession(info)` (existing — `info.cwd` is already canonical), `addSpace(info.cwd)` (single upsert by the canonical path from the server — use `info.cwd`, NOT `check?.canonicalPath`: the response's `cwd` is the source of truth by design; the view switches to this session automatically via Task 5's `addSession` change), `setDialogOpen(false)`.
  On error (e.g. `no such folder` from `space_for_path`, or `spawning failed` from `start_session` → invokes reject a plain `String`): keep the dialog open and show `err instanceof Error ? err.message : String(err)` (the existing idiom) in the existing error slot.
- The dialog no longer "starts a session in a folder that is not a space" — that is still a space's START (a session in that folder IS the dialog's start, and the space is upserted by the response) — one flow, one action (decisions: no empty spaces; a space is born when a conversation starts in it).

3. `src/components/ChatStream.tsx` — add 4 things around the existing streaming/permission/diff logic (which remains unchanged):
- Compute the current space's `SpaceView`: from the store's `spaces`/`sessions`/`historySessions`/`closeReasons` (same `useMemo` shape as `SpacesList`), find the view where `view.liveSessionId === activeSessionId` **|| `view.storedSessionIds.includes(activeSessionId)`** (NOT `[0]`-only: the conversation selector below lets the user open `#2`+ sessions — those are stored sessions of the space too, and the space bar must stay rendered while they are open). `undefined` when the active session belongs to no space (e.g. a legacy pre-Spaces DB row) — render exactly as today.
- Header (new top bar, above the message scroll region): the space `title` (or the active session's `cwd` `basename`) + a status word: `live` (when `isLive`) / `stored` (when `isHistoryOnly`).
- Conversation selector (same bar, right side): a `<select>` listing the space's sessions — first entry: `live` session (label `Live`), then `storedSessionIds` in order with labels `#1`, `#2`, `…` (when the live session is absent, the first stored gets label `Latest`); `onChange` → `openSession(id)`. When `view` is `undefined` (no session in any space): don't render the selector.
- Action buttons (same bar, right side):
  - `isLive`: two buttons — `New conversation` and `Pause`.
    - `Pause`: `closeSession(activeSessionId)`. The session moves to `historySessions` AND — because Task 5 keeps `activeSessionId` through a close (see the `handleSessionClosed` note) — the pane stays on that conversation showing the stored/paused banner (NOT the `No active session` empty state; re-selecting another space from the sidebar is how the user leaves it). This is the deliberate v1 "pause = the conversation stays visible, dimmed, resumable" behavior.
    - `New conversation`: derive `agentId = liveSession?.agentId ?? storedMostRecent?.agentId ?? firstAgentId` (fetch `listAgents` `useEffect`-style like the dialog if none of those are set — `firstAgentId` is the default), then `startSession(agentId, spacePath)` → `addSession(info)` → `addSpace(info.cwd)`. Rely on the backend's one-live policy to close the displaced session; its `session-closed (replaced)` event takes it to `stored` automatically. On error: display in the existing `error` slot.
  - `isHistoryOnly && canResume`: also add a `New conversation` button next to the existing `Resume` button (the stored banner row — same `agentId` derivation; `spacePath = activeSession.cwd`).
- `replaced` banner copy: when `!isLive && closeReasons[activeSessionId] === "replaced"`, show the stored banner with the copy `Paused — a conversation started in another space. Resume to reconnect.` (only when `canResume`; for a `replaced` session the agent advertised `loadSession` or not — the existing `history-only` copy path applies when it did not). The default stored banner line stays as-is for non-`replaced` closes.
- Placeholder copy (the existing `isLive` ternary): `isLive ? "Send a prompt…" : (canResume ? "This space's conversation is paused — Resume to reconnect" : "This session is closed / history only" )`, keeping a sensible no-resume fallback (a `replaced`-paused session without `loadSession` support is still viewable, not resummable).

4. `src/App.tsx` — `SessionList` import → `SpacesList` (component was renamed); add to the existing boot `useEffect` (the one that calls `list_sessions() → setHistorySessions`): replace the single `listSessions()` fetch with BOTH, keeping the existing error-handling shape:
  ```tsx
  Promise.all([listSessions(), listSpaces()])
    .then(([rows, spaces]) => {
      useSessions.getState().setHistorySessions(rows);
      useSessions.getState().setSpaces(spaces); // setSpaces auto-selects (Task 5) the recent landing for boot
    })
    .catch((err) => console.error("failed to load stored sessions/spaces", err));
  ```
  (keep the `.catch` fallback — don't drop the error handling when merging the two fetches). No listener changes (the `replaced` reason flows through the existing `handleSessionClosed`).

Copy conventions: user-facing copy may say `conversation`; variable/function names in code still follow the glossary (`session`, `space`).

**Steps:**
- [ ] `git mv src/components/SessionList.tsx src/components/SpacesList.tsx` and `git mv src/components/NewSessionDialog.tsx src/components/NewSpaceDialog.tsx`; update the import in `App.tsx`.
- [ ] Implement the rewrite above (all 4 files).
- [ ] `pnpm build` — tsc clean (all the renamed imports resolve; `App.tsx` uses the new names).
- [ ] `pnpm test` — all existing tests still pass (the `Permissions` test doesn't import the renamed components; if any test imported the old names, update the import — no behavior change). Note: re-writing `NewSessionDialog.tsx` as `NewSpaceDialog.tsx` means the old file's name disappears — the `git mv` in the previous step already renamed it, so there is nothing else to clean up.
- [ ] **Derived behavior (FROM Task 5's `addSession` change — do NOT hand-hold this):** after `startSession` succeeds in the `New conversation` flow, the view switches to the new session automatically (`addSession` sets `activeSessionId` unconditionally); the displaced conversation shows up in the conversation selector as stored via its `replaced` close event. No manual view-switch call is needed.
- [ ] Commit with message: `feat: UI reskinned to spaces — sidebar, new-space dialog, chat-pane space bar`

**Acceptance criteria:**
- [ ] `pnpm build` + `pnpm test` green.
- [ ] The sidebar lists spaces (name, path, status dot, remove-space action on hover), and the dialog grows the agent dropdown (default = first entry = `pi` on the default registry) + folder validation via `space_for_path`.
- [ ] The chat pane shows the space title/status/conversation selector/actions, and `replaced` renders the `Paused — other space` copy.

---

### Task 7: Acceptance — the real-inference loop in the app, and docs

**Context:**
The spec's verification bar (decided): verified end-to-end in the real app on this machine with real inference (`pi` + `pi-acp` both on `PATH` here — no install step needed), plus the committed-facing docs. The automated suite (Tasks 2–6) is the regression umbrella; this task is the manual proof + the documentation that the `ready to go` promise is true.

**Files:**
- Modify: `README.md`
- (Optional, machine-local only — do not commit if you don't want a new file in the repo: a scratch note in `docs/` — NOT required)
- Verify: `CONTEXT.md`, `docs/decisions/0002-one-live-acp-session.md` (already committed by Task 1 — no change expected)

**What to implement:**

1. `README.md` — update the `## What it does` section's session-centric copy to the Spaces framing (keep the prose style, keep the rest of the README as-is):
   - The app presents **spaces** — each space is a folder; your conversations live inside spaces, and the agent's file access is sandboxed to the space's folder.
   - One conversation is live at a time (starting a conversation in another space pauses the current one — it stays resumable; this sidesteps a known two-session runtime constraint, see `docs/decisions/0002`).
   - The default agent is `pi` (via `pi-acp`); the space's folder is the conversation's working directory.
   - **Also delete** the now-stale `## What it does` bullet `Streams the conversation: agent text, tool calls, file diffs, and a PTY-backed terminal pane.` (wave-1 / Task 1 removed the PTY layer — rewrite that bullet to `Streams the conversation: agent text, tool calls, and file diffs.` so the README stops describing a removed feature).

2. **Pre-condition — make the machine's app state deterministic (this machine, one-time, BEFORE step 1 of the run)**: the dev boxes run so far carry a hand-rolled `agents.json` with two `fake*` entries pointing at debug binaries and **no `pi` entry** — without moving it, the dialog would default to `Fake Agent (permission)` instead of `pi` and the real-inference proof is impossible. TWO candidate files exist and both must be moved, because Tauri 2.11.5's `config_dir()` is the platform config dir **without** the bundle identifier (i.e. `~/.config/` on Linux — the app's live registry is `~/.config/agents.json`), while a copy at `~/.config/com.archimedes.desktop/agents.json` is a leftover from an earlier Tauri build that DID append the identifier (move it too so NO `fake`-entry file survives regardless of which the build reads):
   - `mv ~/.config/agents.json ~/.config/agents.json.bak-$(date +%s)` — the primary registry file; moving it makes `Registry::load` take its file-missing fallback = the built-in default registry (a single `pi → pi-acp` entry).
   - `mv ~/.config/com.archimedes.desktop/agents.json ~/.config/com.archimedes.desktop/agents.json.bak-$(date +%s)` — the stale identifier-suffixed copy (inert for Tauri 2.11.5, moved anyway so no `fake`-registry file lingers).
   - `mv ~/.local/share/com.archimedes.desktop/archimedes.db ~/.local/share/com.archimedes.desktop/archimedes.db.bak-$(date +%s)` — the app data dir holds exactly one stale `fake-session-1` session row (`cwd = /home/daniel/Coding/Javascript/pi-archimedes`) a backfill would otherwise surface; moving the whole DB makes the app recreate an empty one — the acceptance run must see ONLY the spaces it makes. (Note: `~/.local/share/com.archimedes.desktop/agents.json` is an inert copy NOT read by `Registry::load` — leave it; do not chase it. All `.bak` files live OUTSIDE the repo and are never committed.)

3. **The acceptance run** — follow in order (on this machine; the app is the dev build):
   - [ ] `git status --short` — clean (all Tasks 1–6 committed); `which pi pi-acp` shows both on `PATH`.
   - [ ] `pnpm tauri dev` (leave it running in a terminal).
   - [ ] In the app: `+ New space` → `Browse` (or type) `/home/daniel/Coding/AI/archimedes-desktop` → the dialog title toggles to `New space` (a fresh folder has `isSpace: false` on the first check — or `New conversation in archimedes-desktop` if an earlier acceptance pass already created the space; both are valid) → **assert the agent dropdown contains EXACTLY ONE entry, `Pi`, and it is pre-selected** (the `default_registry` proof: no `agents.json` → `Registry::load` → single `pi` entry) → **Start**.
   - [ ] Wait for the session to come up (a real model agent is slower than the fake one — spawn + `initialize` can take a while; bounded by the 30 s establishment timeout).
   - [ ] Send the prompt: `Create a file named probe-acp.txt in the root of this folder with one short sentence in it. Do not modify anything else.` (Enter).
   - [ ] **Assert the real-inference signature**: the response **streams over several seconds** (not an instant full block — a real model's latency and token cadence; the fake agent is instant — this is the discriminator), the agent's tool-call card appears (the file edit), and a **permission prompt** appears in the UI for the write (the `write_text_file` request for `probe-acp.txt` — the fs backend's sandbox gate, working on the real agent).
   - [ ] **Approve** the permission in the UI. Assert: a file-diff card appears for `probe-acp.txt` (or the tool-call card completes), and `ls probe-acp.txt` at the repo root shows a file with actual content (not empty).
   - [ ] Send: `Delete probe-acp.txt.` → assert it is gone (`git status --short` shows no `probe-acp.txt`) — repo left clean.
   - [ ] **One-live check**: create a second space (any temp folder; `mktemp -d` + `New space` + `Browse` to it + start). Assert: the first space's session shows the `Paused — a conversation started in another space. Resume to reconnect.` banner and the new space is live (exactly one live session app-wide — the Task 3 policy, observed end-to-end through the real agent).
   - [ ] **Resume check**: go back to the first space's paused conversation, hit `Resume` → it reconnects (and the second space becomes paused — a DIRECTION too: `resume_session` supersedes a LIVE session per the Task 3 implementation rule — both `start_session` and `resume_session` run `supersede_live_sessions()` first; Task 3's integration test asserts the policy's no-op resume branch, so THIS run through the real agent is the live-supersede-on-resume proof).
   - [ ] **Restart boot check** *(optional, cheap): close the app and re-run `pnpm tauri dev` → the `spaces` list is present (the `spaces` table persisted; the backfill is `DO NOTHING`-idempotent and does not reset the recent order — the sidebar shows the stored recency), and the app lands on the most-recently-touched space — which is the REPO-FOLDER space, because the step-10 Resume check ran `resume_session` on it, and the `record_session → upsert_space` hook (Task 3) bumped its `last_opened_at` AFTER the throwaway space's last touch (step 9) — `autoSelectActive` picks the newest `lastOpenedAt` first — and lands ON CONTENT: the stored transcript re-renders on boot (Task 5's `setSpaces` → `openSession` transcript load). If boot comes up on the wrong space or an empty pane, re-check that the step-10 resume actually succeeded before doubting `autoSelectActive`.

4. Repo hygiene: `git status --short` leaves only the `README.md` change (`probe-acp.txt` must be gone by the end of the box sequence — that is the step-8 delete; the throwaway `mktemp` dir lives under `/tmp`, outside the repo; any other agent residue in this repo from the run — e.g. if the real agent did something unexpected during the step-5 prompt (box 5) — is removed BEFORE the commit; the two `.bak` files live under `~/.config` / `~/.local/share`, outside the repo, and stay there for the user's inspection).

**Steps:**
- [ ] Edit `README.md` as above.
- [ ] Run the acceptance steps 2–11 (all boxes above) — did each assertion hold? If a step fails, STOP: do not commit; diagnose (run the relevant `cargo test` / `pnpm test`; check the app's console + the agent's own logging) and fix, then re-run the failed step to green.
- [ ] After all assertions pass: `git add README.md && git commit -m "docs: README updated to Spaces framing; acceptance verified end-to-end"`.
- [ ] `git log --oneline -8` — the plan's 7 commits sit on `main`'s HEAD (1 fix wave + 5 feature commits, Tasks 2–6 + 1 docs).

**Acceptance criteria:**
- [ ] All 11 acceptance boxes above held on this machine (screenshot or transcript of the key moment — the permission prompt — is fine as evidence, attach to the commit message body if desired).
- [ ] `probe-acp.txt` does not exist post-run; `git status` is clean after the docs commit.
- [ ] The `done-when` line of this plan file is verifiably satisfied.

**Known edges (accepted during design — do NOT expand scope to fix them):**
- Folder rename/move: a space whose folder moved away stops resolving (its sessions stay stored; re-creating the space via `New space` with the new path is the manual re-link).
- Legacy pre-Spaces databases: a stored session whose raw `cwd` cannot be canonicalized (e.g. its folder was deleted before the backfill ran) has no space in the new sidebar — the session is still resumable, unchanged (its `record_session → upsert_space` hook recreates the space row under the canonical path on the next resume: self-healing, no migration code needed).
- The `spaces` ordering in the sidebar is boot-time only (no live re-fetch on actions) — accepted for v1. (Live starts/resumes still update the DB `last_opened_at`; it surfaces on the next boot. If it niggles in review/QA, surface it — don't quietly change it.)
- `replaced` copy: the ADR's `replaced` banner copy says the other conversation was started "in another space" — in 1-space-1-session v1 the two *can* share a cwd (first paused, second needs a distinct `agentId` to be a distinct conversation at the same space; see the Task 7 path-b direction). The copy is left as-is (the ADR is a historical record, not a live doc; the UI's `replaced` banner copy in Task 6 is the user-facing text and is accurate: "a newer conversation took this space").
