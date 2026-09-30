---
status: committed
done-when: Stored sessions can be archived/unarchived from the sidebar (Archived section, collapsed by default) and deleted from the Archived section (confirm dialog); the `archived` flag persists in SQLite across restarts; stored sessions have NO manual Resume button — the first send resumes them transparently; verification suite green (pnpm test, pnpm build, cargo test, cargo clippy --all-targets 0 warnings, cargo fmt --check).
---

# Session Archive & Delete — Plan

**Goal:** Let users archive (hide, keep) and delete (purge) stored sessions from the sidebar, and remove the redundant manual Resume button (first send already auto-resumes).

**Architecture:** An `archived` flag on the `sessions` SQLite row (one-time `ALTER TABLE` migration, the `spaces.trusted` pattern) + a `set_session_archived` command; the frontend store splits stored sessions into `historySessions` (flag off) and `archivedSessions` (flag on, sticky across resume/pause); the sidebar gains an Archive hover button on stored rows and a collapsible Archived section with Unarchive/Delete (confirm) actions; `ChatStream` loses its manual Resume affordances (the existing `send()` auto-resume is untouched).

**Tech Stack:** Rust (rusqlite, Tauri 2 commands) + React 19 / TypeScript (Zustand, lucide-react, the `ui/dialog` primitive).

**Design references:** ADR 0016 (archive-first delete, desktop-copy-only scope), CONTEXT.md "Archived session", ZCode's `WorkspaceArchivedTasksFlatSection` / `TaskActionMenuContent` (the interaction model).

**Verification per task (AGENTS.md):** Rust — `cargo test` (from `src-tauri/`), `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`. Frontend — `pnpm test` + `pnpm build` (from the repo root). TDD: failing test first, confirm it fails, then implement.

---

### Task 1: Rust — the `archived` column and `Db` methods

**Context:**
The `sessions` table (`src-tauri/src/storage/db.rs`) stores one row per session; `messages` and `native_messages` cascade off it. This task adds the `archived` flag at the storage layer: a column in the schema, a one-time migration for pre-existing databases (the exact pattern of the `spaces.trusted` migration already in `Db::open` — a `pragma_table_info` pre-check gates an `ALTER TABLE … ADD COLUMN … NOT NULL DEFAULT 0`), a filter on `list_sessions`, and a `set_session_archived` writer. `record_session` is deliberately UNCHANGED: its `ON CONFLICT … DO UPDATE` branch must never touch `archived` (no retroactive un-archive — the same rule as `upsert_space` never touching `trusted`). Subagent sessions are unaffected (they live in throwaway temp DBs).

**Files:**
- Modify: `src-tauri/src/storage/db.rs`

**What to implement:**
1. `SessionRow` gains a field: `pub archived: bool` (doc comment: "The desktop's archived flag (ADR 0016): `true` hides the session from its Space group into the Archived section; the transcript is kept."). The struct keeps its `#[serde(rename_all = "camelCase")]` (the field serializes as `archived` — single word, case-insensitive).
2. `SCHEMA`: the `sessions` table gains `archived INTEGER NOT NULL DEFAULT 0` (after `capabilities_json`).
3. `Db::open`: after the existing `trusted` migration block (the `has_trusted` pre-check + `ALTER TABLE spaces …`), add the analogous block for `sessions`:
   ```rust
   let has_archived: i64 = conn.query_row(
       "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'archived'",
       [],
       |row| row.get(0),
   )?;
   if has_archived == 0 {
       conn.execute(
           "ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
           [],
       )?;
   }
   ```
4. `list_sessions(&self, include_archived: bool) -> Result<Vec<SessionRow>, DbError>`: the existing query gains `archived` in the `SELECT` list, and a `WHERE archived = 0` clause ONLY when `include_archived` is `false` (compose the SQL string conditionally; keep the existing `ORDER BY created_at DESC, id DESC`).
5. `set_session_archived(&self, id: &str, archived: bool) -> Result<bool, DbError>`: `UPDATE sessions SET archived = ?2 WHERE id = ?1` via `params![id, archived]`; returns `Ok(n > 0)` (a missing row is a no-op, mirroring `set_space_trusted`'s documented behavior).
6. `session(&self, id: &str)` (the single-row lookup used by the resume paths): add `archived` to its `SELECT` and map it into the row.
7. Do NOT change: `record_session`, `delete_session`, `upsert_space`, `set_space_trusted`, the `trusted` / FK migrations, or any `spaces` behavior.

**Steps:**
- [ ] Write failing tests in `db.rs`'s `#[cfg(test)] mod tests` (follow the existing tests' pattern: a unique `std::env::temp_dir()` dir per test, `Db::open`, cleanup at the end):
  - `archived_column_migration_on_a_preexisting_db`: open a raw `rusqlite::Connection`, create the LEGACY `sessions` table (the current schema WITHOUT `archived`) + `messages`/`native_messages`/`spaces` as in `SCHEMA`, insert one row, drop the connection; then `Db::open` on the same file must SUCCEED, `pragma_table_info('sessions')` must contain `archived`, and `list_sessions(true)` must return the row with `archived == false` (the default).
  - `list_sessions_filters_archived_by_default`: `Db::open` fresh; `record_session` a `SessionInfo` (shape reference: `sample_session()` in `src-tauri/tests/storage.rs` — the `db.rs` test module itself has no `SessionInfo` literal to copy); `set_session_archived("…", true)`; assert `list_sessions(false)` excludes it and `list_sessions(true)` includes it with `archived == true`.
  - `set_session_archived_is_a_noop_for_an_unknown_id`: assert `Ok(false)`.
  - `record_session_preserves_the_archived_flag`: `record_session`, `set_session_archived(id, true)`, `record_session` again (the resume re-record), assert the flag is STILL `true` via `list_sessions(true)`.
- [ ] Run `cargo test --lib storage::db` — confirm the new tests FAIL (the `archived` field/methods don't exist yet: compile errors count as the failing state).
- [ ] Implement items 1–6.
- [ ] Run `cargo test --lib storage::db` — all pass (including the pre-existing tests: the migration must not disturb them).
- [ ] Run `cargo clippy --all-targets` — 0 warnings.
- [ ] Run `cargo fmt` then `cargo fmt --check` — clean.
- [ ] Commit with message: "feat(storage): sessions.archived column (migration) + list_sessions filter + set_session_archived"

**Acceptance criteria:**
- [ ] A pre-existing database (no `archived` column) opens cleanly; existing rows read `archived == false`.
- [ ] `list_sessions(false)` never returns archived rows; `list_sessions(true)` returns them with the flag set.
- [ ] `set_session_archived` returns `true` when a row matched, `false` otherwise; a re-`record_session` never clears the flag.
- [ ] Full `cargo test` green; clippy 0 warnings; fmt clean.

---

### Task 2: Rust — `SessionInfo.archived` + the Tauri commands

**Context:**
`SessionInfo` (`src-tauri/src/agent/session.rs`, ~line 236) is the wire type for `start_session` / `resume_session` / `list_sessions` (camelCase over IPC). It gains `pub archived: bool` so the frontend can split the boot list: newly STARTED and ephemeral (subagent) sessions are never archived (`false`); `list_sessions` maps the stored row's flag verbatim; the RESUME paths carry the stored flag (the desktop is the source of truth for the flag — a resumed session may be re-archived by the user later, and the client's sticky view must agree with the DB). `commands/history.rs` gains the `set_session_archived` command and an `includeArchived` parameter on `list_sessions`.

**Files:**
- Modify: `src-tauri/src/agent/session.rs` (the struct + every `SessionInfo { … }` constructor in it, including the two test literals at ~4547 and ~4611)
- Modify: `src-tauri/src/commands/history.rs`
- Modify: `src-tauri/src/agent/harness/loop.rs` (one test constructor)
- Modify: `src-tauri/src/agent/subagent.rs` (five constructors)
- Modify: `src-tauri/src/lib.rs` (the `generate_handler!` registration — `list_sessions`/`delete_session` are registered at ~lines 88–92)
- Modify: `src-tauri/tests/storage.rs` (three `db.list_sessions()` call sites at ~63, ~103, ~287 + the `SessionInfo` literals at ~19 and ~370)
- Modify: every other `src-tauri/tests/*.rs` with a `SessionInfo` literal (grep `SessionInfo {` in `src-tauri/tests/` — 9 literals across `harness_loop.rs:~139`, `harness_store.rs:~29`, `harness_subagent_dispatch.rs:~136`, `harness_dispatch_native.rs:~1055/1470/1609`, `rpc_flow.rs:~284`, `storage.rs:~19/370`)

**What to implement:**
1. `SessionInfo` gains `pub archived: bool` with a doc comment: "The desktop's archived flag (ADR 0016). `false` for a newly started or ephemeral session; the resume paths and `list_sessions` read it from the stored row."
2. Constructor sites (grep `SessionInfo {` in `src-tauri/src/` to find them all — the known list):
   - `commands/history.rs` (`list_sessions` row map): `archived: row.archived`.
   - `agent/session.rs` `build_native_session` (~line 2010, the block-scoped `SessionInfo`): `archived: false` (a native START mints a fresh session).
   - `agent/session.rs` external start establisher closure (~line 2136): `archived: false`.
   - `agent/session.rs` external RESUME establisher closure (~line 2298): the closure already captures `db: Option<Arc<Db>>` (used for `replay_messages`) and `session_id_owned`; BEFORE constructing the `SessionInfo`, add:
     ```rust
     let archived = db
         .as_ref()
         .and_then(|db| db.session(&session_id_owned).ok().flatten())
         .map(|row| row.archived)
         .unwrap_or(false);
     ```
     (a missing row fails closed to `false` — the command's pre-check has already rejected an unresumable row, so in practice the row exists), and set `archived` on the `SessionInfo`.
   - `agent/session.rs` `resume_native_session` (~line 1815): the existing `db.session(session_id)` read (which currently extracts only `capabilities_json` via `.map(|row| row.capabilities_json)`) must KEEP THE WHOLE ROW; after `let info = self.build_native_session(…).await?;` change to `let mut info = …` and add `info.archived = <the row's archived>;` before `self.record_session(&info);`. (The row read is fail-closed `NotResumable` when absent — unchanged.)
   - `agent/harness/loop.rs` (~line 1909, a test that records a session): `archived: false`.
   - `agent/subagent.rs` (five sites: ~583, ~904, ~1862, ~1977, ~2067 — subagent/ephemeral sessions): `archived: false` at each. While in `subagent.rs`, glance at the ~line 897 comment ("the `SessionInfo` has no `Default` — all fields…") and keep its field rationale accurate if it enumerates fields.
3. `commands/history.rs`:
   - `list_sessions(state: State<'_, Arc<Db>>, include_archived: Option<bool>) -> Result<Vec<SessionInfo>, String>`: pass `include_archived.unwrap_or(false)` to `state.list_sessions(…)`; map `archived: row.archived` (item 2).
   - New command:
     ```rust
     /// Archive (or unarchive) a stored session (ADR 0016): sets the
     /// `sessions.archived` flag. The transcript is NOT touched.
     #[tauri::command]
     pub async fn set_session_archived(
         state: State<'_, Arc<Db>>,
         session_id: String,
         archived: bool,
     ) -> Result<bool, String> {
         state.set_session_archived(&session_id, archived).map_err(|e| e.to_string())
     }
     ```
4. `src-tauri/tests/` (separate crates — the crate-internal "every literal" wording does NOT reach them): every `SessionInfo` literal (the 9 sites listed in Files) gains `archived: false`; `tests/storage.rs`'s three `db.list_sessions()` call sites become `db.list_sessions(true)` (a faithful port — the existing assertions expect the unfiltered behavior). `tests/storage.rs:~129` uses `..session.clone()` and survives untouched. `tests/ipc.rs` needs NO change for this task (its `invoke("list_sessions", {})` still works — Tauri maps a missing arg to `None` → the `false` default, and its assertions are field-level, so the extra `archived` key is safe); note for the future: an IPC-level test of `set_session_archived` would also need the command registered in `tests/ipc.rs`'s own `generate_handler!` (~line 66), not just `lib.rs`.
5. Docs: update `commands/history.rs`'s module doc ("list, load, delete" → include archive) and the `list_sessions` doc comment's stale reference to "the frontend's Resume button" (Task 5 removes that button — reword to "the frontend's resume path").
6. Register `set_session_archived` in the `generate_handler!` macro in `src-tauri/src/lib.rs` (next to `list_sessions` / `delete_session` at ~lines 88–92).
7. Do NOT change: `delete_session`, `resume_session` command logic, `normalize_capabilities`, any event payload (none carries `SessionInfo`).

**Steps:**
- [ ] Write failing tests in `session.rs`'s `#[cfg(test)] mod tests` — **two** resume tests (the flag must be covered on BOTH resume paths, not just one):
  - `resume_carries_the_archived_flag` (the EXTERNAL path): follow the `resume_replays_messages` test at ~line 4536 — it registers an external fake-pi agent via `write_agents_json_pi` (~line 4396; `command: fake_pi_bin()`, fixture capabilities carry `piSessionFile`), so `manager.resume_session(…)` takes the external establisher closure (~line 2298). Record the session (update that test's `SessionInfo` literal with `archived: false`), `set_session_archived(id, true)`, run the resume, assert the returned `SessionInfo.archived == true` AND that the stored row's flag is still `true` (the resume's `record_session` re-record did not clear it).
  - `native_resume_carries_the_archived_flag` (the NATIVE path — `resume_native_session`'s keep-whole-row change): use the existing native test helpers at ~line 4742+ (`write_agents_json_native` + `native_test_catalog` + the `set_provider_factory` seam, as `start_native_session_with` uses): start a native session, `db.set_session_archived(id, true)`, `manager.resume_session("nativetest", …)` with a stub `Provider`, assert `info.archived == true` and the row's flag intact.
  - `session_info_round_trips_the_archived_flag_camel_case` (a small serde test, the `settings.rs` round-trip pattern): `serde_json::to_string` of a `SessionInfo` contains `"archived":true` for a flagged value.
- [ ] Run `cargo test --lib agent::session` — confirm the new tests FAIL (compile errors from the missing field count as the failing state).
- [ ] Implement items 1–7.
- [ ] Run `cargo test` (full) — all pass (every `SessionInfo` literal in the crate, including tests, now compiles).
- [ ] Run `cargo clippy --all-targets` — 0 warnings.
- [ ] Run `cargo fmt` then `cargo fmt --check` — clean.
- [ ] Commit with message: "feat(backend): SessionInfo.archived + set_session_archived command (ADR 0016)"

**Acceptance criteria:**
- [ ] `start_session` / native-start / subagent paths report `archived: false`; `list_sessions` and both resume paths report the stored flag.
- [ ] `set_session_archived` is a registered Tauri command returning `Ok(true)`/`Ok(false)`.
- [ ] Full `cargo test` green; clippy 0 warnings; fmt clean.

---

### Task 3: Frontend — the `archivedSessions` store

**Context:**
The Zustand store (`src/store/sessions.ts`) holds `sessions` (live), `historySessions` (stored, flag off) and — new — `archivedSessions` (stored, flag on). **Sticky membership** is the core rule: a session stays in `archivedSessions` while it is live after a resume-from-archive (the Archived *view* filters out live ids); a closing session lands in `historySessions` ONLY if it is not in `archivedSessions`; the flag changes only via the new `archiveSession` / `unarchiveSession` actions (which call the backend). `resumeSession` must search ALL THREE lists (today it searches only `sessions` + `historySessions` — resuming an archived session would throw `unknown session`). Boot: one `list_sessions({ includeArchived: true })` call, split client-side by the flag.

**Files:**
- Modify: `src/lib/tauri.ts` (the `SessionInfo` interface + two wrappers)
- Modify: `src/store/sessions.ts`
- Modify: `src/App.tsx` (the boot call, ~line 162)
- Modify: `src/components/SpacesList.tsx` (one-liner: pass `archivedSessions` to the `views` `useMemo`'s `spaceViewFor` call — Task 4 owns the rest of that file)
- Modify: `src/components/ChatStream.tsx` (one-liner: pass `archivedSessions` to the `views` `useMemo`'s `spaceViewFor` call — Task 5 owns the rest of that file)
- Test: `src/store/sessions.test.ts` (fixtures + 5 `spaceViewFor` call sites + 3 `toEqual` view expectations)
- Test: `src/hooks/useStartNewConversation.test.ts` (1 `spaceViewFor` call site + typed fixtures)
- Test (fixture updates): `src/components/ChatStream.test.tsx`, `src/components/SpacesList.test.tsx`, `src/components/NewSpaceDialog.test.tsx` (every `SessionInfo` literal gains `archived: false` — grep `capabilities:` in each)

**What to implement:**
1. `src/lib/tauri.ts`:
   - `SessionInfo` gains `archived: boolean` (REQUIRED — the Rust side always sends it; every TS `SessionInfo` literal in the test fixtures gets `archived: false`).
   - `listSessions(includeArchived?: boolean): Promise<SessionInfo[]>` → `invoke<SessionInfo[]>("list_sessions", { includeArchived })` (Tauri's `Option<bool>` accepts `undefined` → the Rust default `false`).
   - `setSessionArchived(sessionId: string, archived: boolean): Promise<boolean>` → `invoke("set_session_archived", { sessionId, archived })`.
2. `src/store/sessions.ts`:
   - State: `archivedSessions: SessionInfo[]` (initial `[]`), added to the interface + initial state + the JSDoc block that documents the three lists. **Explicitly** add the `archiveSession: (sessionId: string) => Promise<void>` and `unarchiveSession: (sessionId: string) => Promise<void>` action signatures to the `SessionsState` interface next to `archivedSessions`.
   - `spaceViewFor` gains a 5th parameter: `archivedSessions: SessionInfo[]`. `SpaceView` gains a field `archivedSessionIds: string[]` — built exactly like `storedSessionIds` but from `archivedSessions` (`.filter((s) => s.cwd === space.path).map((s) => s.sessionId)`), documented as "Archived sessions of this space (NOT rendered in the Space group — used only for `view` membership, so a space whose only sessions are archived still resolves its `view`)." `storedSessionIds` is UNCHANGED (the Space group renders stored sessions only). The `lastReason` computation's `newestId` becomes `liveSessionId ?? storedSessionIds[0] ?? archivedSessionIds[0]` (an archived session is still the space's newest stored session — the close-reason banner must not silently vanish for an archived-only space).
   - `setHistorySessions(rows)`: split by the flag —
     ```ts
     const liveIds = new Set(state.sessions.map((s) => s.sessionId));
     const archived = rows.filter((r) => r.archived && !liveIds.has(r.sessionId));
     const history = rows.filter((r) => !r.archived && !liveIds.has(r.sessionId));
     return { historySessions: history, archivedSessions: archived };
     ```
   - `archiveSession: async (sessionId: string) => Promise<void>`: look the entry up in `historySessions` (no-op when absent — the UI only offers archive there); `await setSessionArchived(sessionId, true)`; move the entry `historySessions` → `archivedSessions` (append, preserving order).
   - `unarchiveSession: async (sessionId: string) => Promise<void>`: the mirror (look up in `archivedSessions`; `setSessionArchived(sessionId, false)`; move back, appending to `historySessions`).
   - `resumeSession`: the session lookup becomes `[...state.sessions, ...state.historySessions, ...state.archivedSessions].find(…)` (the error message unchanged). The `set` update keeps the existing `historySessions` filter and does NOT touch `archivedSessions` (sticky — the entry stays while the session is live).
   - `addSession`: also filter the id out of `archivedSessions` (defensive no-op — a fresh session id never collides; keeps the "live ids are out of Archived" invariant cheaply at the write site).
   - `handleSessionClosed(sessionId, reason)`: the `historySessions` update becomes `closedInfo && !state.archivedSessions.some((s) => s.sessionId === sessionId) ? […existing filter…, closedInfo] : state.historySessions` (a closing session that is archived stays ONLY in `archivedSessions`; the `closeReasons` / `messages` / `activeSessionId` updates are unchanged).
   - `deleteSession`: also remove the id from `archivedSessions` (alongside the existing `sessions` / `historySessions` / `messages` / `activeSessionId` cleanup).
   - Do NOT change: `openSession`, `applyConfigOptions`, the `closeReasons` merge rule, `finalizeSessionMessages`.
5. `src/App.tsx` (~line 162): `Promise.all([listSessions(true), listSpaces()])` (the boot fetches the full list; the store splits). NOTE: `setHistorySessions` is called ONCE at boot (`App.tsx:~164`, the empty-deps `useEffect`) — its live-id exclusion is boot-only; do NOT reuse it for a mid-run refresh (re-running it would drop sticky live+archived entries and silently un-archive on the next close).
6. Thread the `spaceViewFor` 5th argument through BOTH component call sites (one-liner each, so Task 3's `pnpm build` gate passes on its own): `src/components/SpacesList.tsx`'s `views` `useMemo` (~line 78) and `src/components/ChatStream.tsx`'s `views` `useMemo` (~line 434) each pass `archivedSessions` (select it via `useSessions((s) => s.archivedSessions)` if not already selected in that file) AND add `archivedSessions` to each memo's dependency array. Tasks 4 and 5 then own only the `v.archivedSessionIds` predicate extensions in their files.
7. Test fallout of the `spaceViewFor` / `SpaceView` shape change (beyond the fixtures in item 4): the **5 four-argument `spaceViewFor` call sites** in tests fail tsc — `sessions.test.ts:~506, ~521, ~534, ~554` and `useStartNewConversation.test.ts:~48` — update each to pass `[]` as the 5th argument; and the **3 full-shape `toEqual` view expectations** in `sessions.test.ts` (~507, ~521, ~534) fail at RUNTIME (they match the whole returned object) — add `archivedSessionIds: []` to each expectation.
4. Test fixtures — **all five** test files with `SessionInfo` literals get `archived: false`: `src/store/sessions.test.ts` (every typed `SessionInfo` literal — the `info(): SessionInfo` factory at ~line 42 AND all `addSession`/`setState` literals; grep `capabilities:` in the file to find them all — e.g. the hits at ~30, ~42, ~942, ~953, ~961, ~1140, ~1201, ~1270), `src/components/ChatStream.test.tsx` (contextually-typed `setState` literals), `src/components/SpacesList.test.tsx` (`seed()` `setState` literals), `src/hooks/useStartNewConversation.test.ts` (typed `SessionInfo[]` consts at ~lines 43/46), `src/components/NewSpaceDialog.test.tsx` (untyped mock data at ~line 19 — not a compile break, but update for consistency). NOTE THE MECHANISM: fixture omissions fail **`pnpm build` (tsc), NOT `pnpm test`** (`vitest run` does not type-check). Additionally, the untyped `vi.mock` factory literals (`NewSpaceDialog.test.tsx:~19`, `ChatStream.test.tsx:~60 AND ~68` — the factory holds two `SessionInfo` literals, `useStartNewConversation.test.ts:~21`) do not compile-break but get `archived: false` for runtime consistency — the `resumeSession` mock entry in `sessions.test.ts` itself is UNCHANGED, but its resolved-value literal gains `archived: false`. **CRITICAL — mock factories:** `sessions.test.ts` and `SpacesList.test.tsx` each `vi.mock("../lib/tauri", …)` with a factory that spreads `...actual`; there is NO global Tauri mock. The factory MUST gain `setSessionArchived: vi.fn().mockResolvedValue(true)` and `deleteSession: vi.fn().mockResolvedValue(undefined)` — without them the REAL wrappers run (via `...actual`), `invoke` rejects in jsdom, and every archive/unarchive/delete assertion fails.

**Steps:**
- [ ] Write failing tests in `sessions.test.ts` (follow the file's existing patterns — it seeds `useSessions.setState(…)` and asserts on `useSessions.getState()`):
  - `setHistorySessions splits rows by the archived flag`: seed `sessions: [live]`; call `setHistorySessions` with three rows (one live id, one `archived: true`, one `archived: false`); assert the live id is in NEITHER list, the flagged row is in `archivedSessions` only, the other in `historySessions` only.
  - `archiveSession moves the entry and calls the backend`: seed `historySessions: [row]`; call `archiveSession`; assert the entry moved to `archivedSessions` (and the mocked `setSessionArchived` wrapper was called with `(id, true)` — the file's existing pattern for mocking `../lib/tauri`).
  - `unarchiveSession is the mirror`.
  - `resumeSession finds an archived session (sticky)`: the mocked `resumeSession` in `sessions.test.ts` resolves to a FIXED `SessionInfo` with `sessionId: "s1"` — seed the `archivedSessions` fixture with `sessionId: "s1"` (or use `vi.mocked(resumeSession).mockResolvedValueOnce(…)` with the seeded id); call `resumeSession(id)`; assert the session is now in `sessions` AND still in `archivedSessions` (sticky).
  - `spaceViewFor reports archivedSessionIds without polluting storedSessionIds`: `spaceViewFor(space, [live], [storedRow], {…}, [archivedRow])` (all in the same space `path`); assert `view.archivedSessionIds` contains the archived id, `view.storedSessionIds` contains the stored id and NOT the archived one, and `view.lastReason` resolves from the archived row when it is the only session (the `?? archivedSessionIds[0]` fallback).
  - `a resumed-then-closed archived session stays archived`: follow the previous test, then `handleSessionClosed(id, "user")`; assert the id is in `archivedSessions` and NOT in `historySessions`.
  - `deleteSession removes the id from all three lists`.
- [ ] Run `pnpm test src/store/sessions.test.ts` — confirm the new tests FAIL.
- [ ] Implement items 1–7 (the `SessionInfo` interface + `spaceViewFor` signature changes make the typed fixture/call-site omissions fail **`pnpm build`/tsc**, and the 3 `toEqual` expectations fail `pnpm test` — those are the expected failing states; `pnpm test` alone does not catch the tsc breakage).
- [ ] Run `pnpm test` (full) — all pass (the `archived: false` fixture updates keep the pre-existing tests green).
- [ ] Run `pnpm build` — tsc + vite clean.
- [ ] Commit with message: "feat(store): archivedSessions (sticky) + archive/unarchive/delete actions"

**Acceptance criteria:**
- [ ] Boot: `list_sessions` rows split into `historySessions` / `archivedSessions` by the flag, live ids in neither.
- [ ] Sticky rule holds across resume + close; the flag changes only via `archiveSession` / `unarchiveSession`.
- [ ] `deleteSession` clears the id from all three lists; the 5 test `spaceViewFor` call sites + 3 `toEqual` expectations are updated and `pnpm test` passes; `pnpm build` green (all typed fixtures + both component one-liners in place).

---

### Task 4: Frontend — the sidebar Archive affordance and Archived section

**Context:**
`SpacesList.tsx` renders the sidebar: Space groups (each with a live row + stored rows) in a scroll area, above the footer. This task adds the archive affordance (a hover icon button on stored rows, the exact pattern of the live rows' existing Pause button: a `group-hover:hidden` time slot + a `hidden group-hover:flex` button) and the Archived section (a flat, cross-space list below the Space groups, ZCode's `WorkspaceArchivedTasksFlatSection` model adapted to Archimedes' row style: title + subtle space name + relative time, hover = Unarchive + Delete). Delete is the only destructive action and gets a confirm dialog (`ui/dialog`, the `SudoConfirmModal` pattern). Archive/unarchive are reversible — no confirm.

**Files:**
- Modify: `src/components/SpacesList.tsx`
- Create: `src/components/DeleteSessionDialog.tsx`
- Test: `src/components/SpacesList.test.tsx`
- Test: `src/components/DeleteSessionDialog.test.tsx`

**What to implement:**
1. `SessionRow` (in `SpacesList.tsx`): for a STORED row (`isLive` false — the current branch renders `rightSlot`), replace the plain `rightSlot` with the hover-swap pattern the live branch already uses:
   ```tsx
   <span className="group-hover:hidden">{rightSlot}</span>
   <button
     type="button"
     title="Archive"
     aria-label={`Archive ${title}`}
     onClick={(e) => { e.stopPropagation(); void archiveSession(sessionId); }}
     className="hidden size-6 items-center justify-center rounded-md group-hover:flex hover:bg-surface-hover"
   >
     <ArchiveIcon className="size-4" />
   </button>
   ```
   (`Archive` from `lucide-react`, imported alongside the existing icons; `archiveSession` from `useSessions`.) Live rows are UNCHANGED (Pause stays).
2. `DeleteSessionDialog.tsx` (new file, the `SudoConfirmModal` structure — `Dialog open onOpenChange` + `DialogContent showCloseButton={false} className="max-w-md"` + header/body/footer — NOTE: there is NO `SudoConfirmModal.test.tsx`; for the TEST pattern see `NewSpaceDialog.test.tsx` / `SubagentModals.test.tsx`, which render `ui/dialog`-based modals and assert buttons via `@testing-library/react`):
   - Props: `title: string` (the session's title), `onConfirm: () => void`, `onClose: () => void`.
   - Title: "Delete session?"; body: the session title (subtle, truncated) + "This permanently removes the session's transcript from Archimedes' storage. This can't be undone."
   - Footer: `Cancel` (ghost, `onClose`) + `Delete` (the `destructive` variant of `ui/button.tsx` — it exists, ~line 21), `onConfirm`.
3. `ArchivedSection` (new component in `SpacesList.tsx`, rendered after the `views.map(…)` block inside the `flex-1 overflow-y-auto` div so it scrolls with the list):
   - Data: `const archived = useSessions((s) => s.archivedSessions)`; `const sessions = useSessions((s) => s.sessions)`; `const visible = archived.filter((s) => !sessions.some((l) => l.sessionId === s.sessionId))` (the sticky live ids are view-filtered out).
   - Header row (a `div` with the group's existing `px-2.5 py-1` rhythm): `Archive` icon + "Archived" + a count badge (the `visible.length`, `text-foreground-subtlest`) + a chevron toggle; `const [open, setOpen] = useState(false)` — **collapsed by default**.
   - Rows (one `ArchivedSessionRow` per `visible` entry, the `SessionRow` visual pattern): 16px leading slot (empty — archived sessions are never live), the title (`titleFor(messages, spaceName)` — the space name: match `space.path === session.cwd` against the `spaces` list (both are canonical-path keys) and use `basenameOfPath(space.path)` (the `spaceViewFor` pattern, `basenameOfPath` from `src/lib/paths.ts`); fall back to `basenameOfPath(session.cwd)` when NO space matches (a legacy session whose cwd is no longer a Space)), the space name as a subtle `· {spaceName}` suffix ONLY when the title is message-derived (skip the suffix when the title IS the space name — avoid "alpha · alpha"), the relative time (`relativeTimeFor` — `null` → empty slot, same as stored rows). Hover: `ArchiveX` (`unarchiveSession`) + `Trash2` (opens the delete confirm). Click: `openSession(sessionId)` (identical to a stored row — the transcript opens; first send resumes per Task 5).
   - `activeView` lookup (~lines 84–91): the predicate extends to `v.liveSessionId === activeSessionId || v.storedSessionIds.includes(activeSessionId) || v.archivedSessionIds.includes(activeSessionId)` — so ⌘N / New Session while an archived session is active routes to THAT space (not the Open Space dialog). (The `views` `useMemo`'s 5th `spaceViewFor` argument was already threaded in Task 3.)
   - `activeHistory` lookup (~lines 98–101, `const activeHistory = activeSession ? undefined : historySessions.find(…)`): extend to `historySessions.find(…) ?? archivedSessions.find(…)` — this feeds `activeSpacePath` → `useSkillCatalog(activeSpacePath)`, which must keep working for an archived-only active session (otherwise the Skills catalog silently degrades to user-level skills). NOTE: the `useStartNewConversation` hook's agent derivation (`live ?? storedMostRecent ?? firstAgentId`) intentionally IGNORES archived sessions (a consequence of `archivedSessionIds` being view-membership-only) — a new conversation in an all-archived space uses the registry-default agent; do not "fix" this in this task.
   - Empty state: when `visible.length === 0`, render a subtle hint line ("No archived sessions.") in the section body when `open`.
   - The section header renders even when empty (the count `0` keeps the entry point discoverable) — but if `visible.length === 0` AND the section has never had content, a plain "Archived" header with count 0 is fine (do not special-case).
4. Delete wiring in `SpacesList`: `const [deleteTarget, setDeleteTarget] = useState<SessionInfo | null>(null)`; `Trash2` click → `setDeleteTarget(row)`; the dialog's `onConfirm` → `void deleteSession(deleteTarget.sessionId)` then `setDeleteTarget(null)` (the store's `deleteSession` already removes the row from all lists — the dialog closes and the row vanishes).
5. Do NOT change: the Space group structure (the `views` `useMemo` 5th-arg threading was done in Task 3 — this task adds only the `activeView` predicate), the New Session / Open Space / Skills buttons, the footer, the keyboard-shortcut handling (only the `activeView` predicate they route through changes).

**Steps:**
- [ ] Write failing tests in `SpacesList.test.tsx` (follow the file's existing patterns — it seeds the stores and queries by role/label):
  - `a stored row offers an Archive hover action that archives it`: seed a space + a stored session (`archived: false`); query the `Archive` button (by `aria-label`); click it; assert `useSessions.getState().archivedSessions` contains the id and `historySessions` does not.
  - `a live row offers Pause, not Archive`: seed a live session; assert no `Archive` button is present (the Pause button is).
  - `the Archived section is collapsed by default and lists archived sessions`: seed `archivedSessions: [row]`; assert the section header ("Archived") is visible and the row is NOT (collapsed); click the header; assert the row is visible.
  - `unarchive moves the row back`: follow the previous, click the row's `ArchiveX` action; assert the id is back in `historySessions`.
  - `delete from the Archived section confirms and removes`: click the row's `Trash2`; assert the confirm dialog appears ("Delete session?"); click Cancel — the row remains; click `Trash2` again, click `Delete` — assert the id is gone from `archivedSessions` (and the dialog closed).
  - `a live session that is also archived is view-filtered out of the Archived section`: seed the same id in `sessions` (live) and `archivedSessions`; assert the row is NOT in the section.
  - `⌘N with an archived session active starts a conversation in that space (not the Open Space dialog)`: seed a space + an archived session with `activeSessionId` set to it; trigger the New Session flow (the file's existing pattern for the top `New Session` button); assert the `startSession` tauri mock was called with the space's `path` and the `NewSpaceDialog` did NOT open.
  - `an archived-only active session still resolves its space for the skill catalog`: seed a space + an archived session with `activeSessionId` set to it; assert the `useSkillCatalog` input path (the `activeSpacePath` derivation) is the space's path (the `activeHistory` `?? archivedSessions` lookup) rather than `null`.
- [ ] Write the `DeleteSessionDialog.test.tsx` tests (the `NewSpaceDialog.test.tsx` / `SubagentModals.test.tsx` pattern — `SudoConfirmModal.test.tsx` does NOT exist): renders the title + body copy; `Cancel` calls `onClose`; `Delete` calls `onConfirm`.
- [ ] Run `pnpm test src/components/SpacesList.test.tsx src/components/DeleteSessionDialog.test.tsx` — confirm the new tests FAIL.
- [ ] Implement items 1–5.
- [ ] Run `pnpm test` (full) — all pass (pre-existing `SpacesList` tests: the stored-row right slot change must not break the time/Waiting-pill assertions — the pill and time still render when not hovering; the `spaceViewFor` 5th-arg change is satisfied by Task 3's signature).
- [ ] Run `pnpm build` — clean.
- [ ] Commit with message: "feat(sidebar): archive stored sessions + Archived section (delete with confirm)"

**Acceptance criteria:**
- [ ] Stored rows show an Archive icon on hover; clicking archives (row leaves the Space group, appears in Archived).
- [ ] The Archived section is collapsed by default, shows a count, and its rows offer Unarchive + Delete (confirm dialog); delete removes the session.
- [ ] Live sessions never render Archive; a live-archived id is filtered out of the section.
- [ ] `pnpm test` + `pnpm build` green.

---

### Task 5: Frontend — remove the manual Resume affordances

**Context:**
`ChatStream.tsx` renders, for a resumable stored session, a banner with a **Resume** button (~line 853) and a **Resume** item in the header dropdown (~line 911). The first `send()` ALREADY auto-resumes the session before sending (the `if (!isLive)` block in `send`, ~line 503), so the manual affordances are redundant friction — the user's request is that the act of engaging with the session (sending) resumes it, with no extra Resume click. This task removes both affordances and makes the banner purely informational. The `resume()` helper and its `resuming` guard STAY (the guard still prevents a double-resume when `send()` runs; only the visible label goes with the button).

**Files:**
- Modify: `src/components/ChatStream.tsx`
- Test: `src/components/ChatStream.test.tsx`

**What to implement:**
1. **The `historySession` selector (~lines 99–103) must search BOTH stored lists** — as it stands it only finds `historySessions`, so a session living ONLY in `archivedSessions` (opened from the Archived section) would render NO banner, a disabled composer ("This session is closed"), and an unreachable `send()` auto-resume. Change it to:
   ```ts
   const historySession = useSessions((s) =>
     s.activeSessionId
       ? (s.historySessions.find((x) => x.sessionId === s.activeSessionId) ??
          s.archivedSessions.find((x) => x.sessionId === s.activeSessionId))
       : undefined,
   );
   ```
   (One selector returning a `??` of two finds — stable-reference rule preserved.) This fixes `isHistoryOnly` / `canResume` / `composerEnabled` / the banner for archived sessions, AND the same lookup's other consumers: `capabilities` (~line 262), `spacePath` (~line 351), the title (~line 658), the footer `agentId` (~line 1208).
2. The `view` lookup (~lines 434–444): the `views.find(…)` predicate extends to `v.liveSessionId === activeSessionId || v.storedSessionIds.includes(activeSessionId) || v.archivedSessionIds.includes(activeSessionId)` — so a space whose only sessions are archived still renders its space bar / conversation selector. (The `views` `useMemo`'s 5th `spaceViewFor` argument was already threaded in Task 3.) AND the conversation-selector `options` (~lines 645–654, built from `view.liveSessionId` + `view.storedSessionIds` only) gains one entry: when `view.archivedSessionIds.includes(activeSessionId)`, push `{ id: activeSessionId, label: "Archived" }` — otherwise an archived-only space renders a `Select` with zero items and a `value` matching nothing (an empty trigger + empty dropdown).
3. The banner (the `isHistoryOnly` block, ~lines 844–858): the `canResume` branch becomes text ONLY — "This session is stored. Sending a message resumes it." — the `Button` (and its `resuming` label usage) is removed. The non-resumable branch ("History only — continuing starts a new session.") is UNCHANGED.
4. The header dropdown (~lines 911–914): remove the `canResume && <DropdownMenuItem …>Resume</DropdownMenuItem>` block entirely. The `Pause` and `New Session in this Space` items are unchanged.
5. KEEP: the `resume` helper + the `resuming` state (the guard `if (!activeSessionId || resuming) return false;` stays — it still protects the `send()` auto-resume path), `send()`'s auto-resume block, the composer `composerEnabled` rule (`isLive || canResume`), the placeholder ("Resume this session to send"), the `liveSession` selector (unchanged — only `historySession` changes, per item 1).
6. Do NOT change: the live-session UI, the `pause` handler, `startNewConversation`, the transcript rendering.

**Steps:**
- [ ] Write failing tests in `ChatStream.test.tsx` (follow the file's existing patterns for the stored-session banner — it seeds `historySessions` rows with `capabilities`):
  - `a resumable stored session renders an informational banner with no Resume button`: seed a stored session with `capabilities: { loadSession: true, … }`; assert the banner text "Sending a message resumes it." is present and NO button with the text "Resume" exists in the document.
  - `the header dropdown has no Resume item`: open the dropdown (the file's existing pattern for the `MoreHorizontal` trigger); assert the items are Pause-absent + "New Session in this Space" only (a stored session has no Pause either) — i.e. no "Resume" item.
  - `send auto-resumes a stored session before sending` (the existing auto-resume test — if one already exists, keep it; if not, add it following the file's `send` test pattern: seed a resumable stored session, type a message, submit; assert the `resumeSession` store action ran and the message landed in `messages[id]`).
  - `the conversation selector lists the active archived session`: seed a space whose only session is archived (`archivedSessionIds: [id]`, `storedSessionIds` empty) with `activeSessionId: id`; assert the `Select` renders an option labeled `Archived` (not an empty trigger).
  - `an archived-only session renders the stored banner and an enabled composer, and first send resumes` (the integration test for item 1): seed `archivedSessions: [row]` with `capabilities: { loadSession: true, … }`, `activeSessionId: row.sessionId`, `sessions: []`; assert the banner text "Sending a message resumes it." renders and the composer is NOT disabled; type a message and submit; assert the `resumeSession` store action was called (and the message landed in `messages[row.sessionId]`).
- [ ] Run `pnpm test src/components/ChatStream.test.tsx` — confirm the new tests FAIL (the button/dropdown item still render; the archived-only test fails on the missing banner).
- [ ] Implement items 1–6.
- [ ] Run `pnpm test` (full) — all pass (the pre-existing banner assertions that referenced the Resume button are updated by the new tests; any other test asserting the old copy is updated to the new copy).
- [ ] Run `pnpm build` — clean.
- [ ] Commit with message: "feat(chat): drop the manual Resume button — first send resumes (ADR 0016 flow)"

**Acceptance criteria:**
- [ ] No "Resume" button or dropdown item anywhere in `ChatStream`; the banner is informational.
- [ ] An archived-only session (from the Archived section) renders the stored banner, an enabled composer, the space bar, and a conversation-selector option labeled `Archived` — and first send resumes it (item 1's integration test passes).
- [ ] `send()` still auto-resumes a resumable stored session and sends (the `resuming` guard intact).
- [ ] The non-resumable banner and disabled composer are unchanged.
- [ ] `pnpm test` + `pnpm build` green.

---

**Task order & dependencies:** 1 → 2 → 3 → 4 → 5. Tasks 4 and 5 are independent of each other (both depend on Task 3's store + `spaceViewFor` / `SpaceView` changes) and could run in parallel. Each task is independently commitable.

**Final verification (after Task 5, per AGENTS.md):** from the repo root: `pnpm test` + `pnpm build`; from `src-tauri/`: `cargo test` + `cargo clippy --all-targets` (0 warnings) + `cargo fmt --check`. All green = done-when met.
