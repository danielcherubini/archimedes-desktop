---
status: committed
done-when: A Space can be marked trusted (the prompt's third option or the sidebar shield button); a trusted Space's bash/edit/write tool calls run without a permission prompt — main and subagent Sessions, immediate effect, persisted across restarts; sudo_exec and ask are unaffected
---

# Trusted Space Plan

**Goal:** Per-Space permission auto-approve — a Trusted Space's `bash`/`edit`/`write` tool calls are auto-approved desktop-side (the gate extension is unchanged), with a prompt option and a sidebar shield button to set the flag.
**Architecture:** A `trusted` column on the `spaces` SQLite table; the permission handler (`permission.rs`) looks up the session's Space per `Confirm` request and, when trusted, responds `confirmed: true` without emitting a prompt; the untrusted prompt gains a third option (`trust-space`) whose outcome sets the flag. Frontend: `SpaceRow`/`SpaceView` gain `trusted`, a shield hover-button on `SpaceGroup`, one new Tauri command. `sudo_exec` (desktop-owned modal) and `ask` (`Select` requests) are untouched.
**Tech Stack:** Rust (Tauri 2, tokio, rusqlite), React 19 + TypeScript (Zustand, Vitest).
**Branch:** execute on a feature branch `trusted-space`; one commit per task, squash-merge into `main` at the end (AGENTS.md convention).
**Design source:** the approved spec this file replaced (git history) + ADR 0010 (`docs/decisions/0010-trusted-space.md`). Key invariants: per-request lookup (toggle takes effect on the next tool call, no restart); fail-closed (no `spaces` row / canonicalize failure / db error = untrusted = prompt); `Confirm` is the gate's by invariant so trust can only suppress gate confirms.

---

### Task 1: DB — `trusted` column, idempotent migration, `set_space_trusted` + `space_trusted`

**Context:**
The trust flag lives on the `spaces` table (PK = canonical path). Fresh databases get the column from the `CREATE TABLE`; pre-existing databases get it via a one-time `ALTER TABLE` at open, ignoring the duplicate-column error — the codebase's idempotent-at-open pattern (same shape as the cwd→spaces backfill in `Db::open`). Two accessors: `set_space_trusted` (write) and `space_trusted` (read, canonicalizing, fail-closed). `upsert_space`, the backfill INSERT, `find_space`, and `delete_space` need NO changes — the column's `DEFAULT 0` covers existing INSERT statements.

**Files:**
- Modify: `src-tauri/src/storage/db.rs`
- Test: `src-tauri/tests/storage.rs`

**What to implement:**

In `src-tauri/src/storage/db.rs`:

1. `SpaceRow` (line ~67): add a field `pub trusted: bool` (after `last_opened_at`). It stays `#[serde(rename_all = "camelCase")]`-serialized — the frontend name is `trusted`.
2. `SCHEMA` (line ~332): the `spaces` table gains the column:
   ```sql
   CREATE TABLE IF NOT EXISTS spaces (
       path TEXT PRIMARY KEY,
       created_at INTEGER NOT NULL,
       last_opened_at INTEGER NOT NULL,
       trusted INTEGER NOT NULL DEFAULT 0
   );
   ```
3. `Db::open` (line ~89, right after `conn.execute_batch(SCHEMA)?;`): add the one-time migration, BEFORE the existing backfill loop:
   ```rust
   // One-time migration for pre-existing databases: add `trusted`
   // (fresh databases already have it from SCHEMA). Idempotent — the
   // duplicate-column error is ignored, mirroring the backfill below.
   if let Err(e) = conn
       .execute("ALTER TABLE spaces ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0", [])
   {
       if !e.to_string().contains("duplicate column name") {
           return Err(e.into());
       }
   }
   ```
4. New method (next to `upsert_space`, ~line 273):
   ```rust
   /// Set (or clear) a space's trust flag. No-op if the row is missing.
   pub fn set_space_trusted(&self, path: &str, trusted: bool) -> Result<(), DbError> {
       self.conn.lock().expect("db mutex poisoned").execute(
           "UPDATE spaces SET trusted = ?2 WHERE path = ?1",
           params![path, trusted],
       )?;
       Ok(())
   }
   ```
5. New method (next to `find_space`, ~line 303):
   ```rust
   /// The trust flag for the space at `cwd` (canonicalized). Fail-closed:
   /// a canonicalize failure or a missing row is `Ok(false)` — untrusted.
   pub fn space_trusted(&self, cwd: &std::path::Path) -> Result<bool, DbError> {
       let canonical = match std::fs::canonicalize(cwd) {
           Ok(c) => c,
           Err(_) => return Ok(false),
       };
       let p = canonical.display().to_string();
       let guard = self.conn.lock().expect("db mutex poisoned");
       let mut stmt = guard.prepare("SELECT trusted FROM spaces WHERE path = ?1")?;
       let found = stmt
           .query_map(params![p], |row| row.get::<_, i64>(0))
           .and_then(|rows| rows.next())
           .transpose()?;
       Ok(found.map(|t| t != 0).unwrap_or(false))
   }
   ```
6. `list_spaces` (line ~283): add `trusted` to the `SELECT` and to the `SpaceRow` construction (`trusted: row.get(3)? != 0`).

**Steps:**
- [ ] Add to `src-tauri/tests/storage.rs` (follow the file's existing `temp_db_path()` helper + the style of `spaces_upsert_find_delete_and_order`):
  - `spaces_trusted_column_fresh_and_migrated`: (a) fresh `Db::open` → `space_trusted` works and `list_spaces` returns `trusted: false` for a just-upserted space; (b) simulate a pre-migration database — open a fresh db, `DROP` the `trusted` column is NOT possible, so instead: create the `spaces` table manually with the OLD 3-column schema in a new db file (`CREATE TABLE spaces (path TEXT PRIMARY KEY, created_at INTEGER NOT NULL, last_opened_at INTEGER NOT NULL)` + one row), then `Db::open` it → the `ALTER` adds the column with default 0 (the row reads `trusted: false`); open a THIRD time → no error (idempotent).
  - `set_space_trusted_updates_and_flag_reads`: `upsert_space` a path → `set_space_trusted(path, true)` → `space_trusted(&path)` is `true` and `list_spaces` returns `trusted: true` → `set_space_trusted(path, false)` → both read `false`.
  - `space_trusted_fail_closed`: no row for the path → `Ok(false)`; a non-existent path (canonicalize fails) → `Ok(false)`.
- [ ] Run `cargo test --test storage` (from `src-tauri/`)
  - Did it fail with compile errors (the `trusted` field / methods don't exist yet)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–6 in `src-tauri/src/storage/db.rs`
- [ ] Run `cargo test --test storage`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test` (full suite — guards the `SCHEMA` change against the other db tests)
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt` (from `src-tauri/`)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Zero warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(db): trusted column on spaces with idempotent migration"

**Acceptance criteria:**
- [ ] Fresh and pre-existing databases both expose `spaces.trusted` (default 0), and re-opening is a no-op
- [ ] `set_space_trusted` is a no-op for missing rows; `space_trusted` is fail-closed (`Ok(false)`) on canonicalize failure or missing row
- [ ] `list_spaces` returns the flag (camelCase `trusted` over IPC)

---

### Task 2: Permission — trusted short-circuit + `trust-space` outcome

**Context:**
The permission handler (`permission.rs`) is the single decider (ADR 0010): on a `Confirm` request it now looks up the session's Space and, when trusted, responds `Confirmed { confirmed: true }` immediately — no `permission-request` event, no oneshot, nothing for the UI. Untrusted flows keep today's behavior but the prompt gains a third option, `trust-space` ("Don't ask again for this Space"); its outcome answers `confirmed: true` FIRST, then sets the flag best-effort (a failed write is logged; the Space stays untrusted and the next call prompts again). `Select` requests (the `ask` tool) are never auto-answered. The single call site is the session driver (`session.rs` ~line 817), which serves BOTH main and subagent Sessions — one change covers both.

**Files:**
- Modify: `src-tauri/src/agent/permission.rs`
- Modify: `src-tauri/src/agent/session.rs` (the call site, ~line 816)
- Test: `src-tauri/tests/rpc_flow.rs`

**What to implement:**

In `src-tauri/src/agent/permission.rs`:

1. Import: `use crate::storage::Db;`
2. `handle_extension_ui_request` signature — add two trailing parameters:
   ```rust
   pub async fn handle_extension_ui_request(
       session_id: &str,
       req: ExtensionUiRequest,
       handle: &PiRpcHandle,
       sink: &Arc<dyn EventSink>,
       pending_permissions: &PendingPermissions,
       db: Option<&Arc<Db>>,
       cwd: &std::path::Path,
   )
   ```
3. The `Confirm` arm — insert the trust short-circuit BEFORE `spawn_permission_waiter`:
   ```rust
   ExtensionUiRequest::Confirm { id, title, .. } => {
       // Trusted Space (ADR 0010): auto-approve — no prompt, no event,
       // no oneshot. Fail-closed: no db / db error / no space row /
       // canonicalize failure = untrusted = today's flow.
       let trusted = match db {
           Some(d) => d.space_trusted(cwd).unwrap_or(false),
           None => false,
       };
       if trusted {
           let handle = handle.clone();
           tokio::spawn(async move {
               let _ = handle
                   .respond_extension_ui(ExtensionUiResponse::Confirmed {
                       id,
                       confirmed: true,
                   })
                   .await;
           });
           return;
       }
       // Untrusted: today's flow, with the third option (always present —
       // a prompt only ever appears for an untrusted Space, so the option
       // is always actionable).
       spawn_permission_waiter(
           session_id, id,
           json!({
               "sessionId": session_id,
               "toolCall": { "title": title },
               "options": [
                   { "optionId": "allow", "name": "Allow", "kind": "allow" },
                   { "optionId": "reject", "name": "Block", "kind": "reject" },
                   { "optionId": "trust-space", "name": "Don't ask again for this Space", "kind": "allow" },
               ],
           }),
           ResponseKind::Confirm,
           handle, sink, pending_permissions,
           db.cloned(), cwd.display().to_string(),
       )
       .await;
   }
   ```
   (The `Select` / `Input` / `Editor` / notify arms are UNCHANGED — `Select` is never auto-answered, `input`/`editor` stay immediate-cancelled.)
4. `spawn_permission_waiter` — add two trailing parameters `db: Option<Arc<Db>>` and `cwd: String`, and extend the spawned waiter's outcome mapping. The `Confirm` arm of the match becomes:
   ```rust
   (ResponseKind::Confirm, PermissionOutcome::Selected { option_id }) => {
       match option_id.as_str() {
           "allow" => ExtensionUiResponse::Confirmed { id: request_id, confirmed: true },
           "reject" => ExtensionUiResponse::Confirmed { id: request_id, confirmed: false },
           // Trust this Space: answer FIRST (the user's immediate intent
           // never lost to a db error), then set the flag best-effort.
           "trust-space" => {
               if let Some(d) = &db {
                   if let Err(e) = d.set_space_trusted(&cwd, true) {
                       eprintln!("trust-space: failed to set trusted for {cwd}: {e}");
                   }
               }
               ExtensionUiResponse::Confirmed { id: request_id, confirmed: true }
           }
           // A non-allow/reject/trust-space selection is a dismissal.
           _ => ExtensionUiResponse::Cancelled { id: request_id },
       }
   }
   ```
   The `Select` arm stays as-is.
   NOTE: the existing "respond exactly once, best-effort" tail of the waiter is unchanged — the `trust-space` db write happens BEFORE the respond (both inside the spawned task; ordering within the task is sequential).

In `src-tauri/src/agent/session.rs` (the `ui_rx.recv()` arm, ~line 816):

5. The call gains the two new arguments — `db` is the `let db = self.db.clone();` from ~line 616 (`Option<Arc<Db>>`, in scope), `info.cwd` is the session's `PathBuf`:
   ```rust
   permission::handle_extension_ui_request(
       &info.session_id, req, &handle, &sink, &pending_permissions_arc,
       db.as_ref(), &info.cwd,
   )
   .await;
   ```

In `src-tauri/tests/rpc_flow.rs` (follow the file's existing patterns: `write_agents_json_pi` + `FAKE_PI` env, the fake `EventSink` that records events, `wait_for_event_named`, and the `SessionManager` + `attach_db` pattern from `session.rs`'s tests — `fake_pi`'s `FAKE_PI_GATE=1` mode emits the gate's `extension_ui_request` confirm and proceeds when it receives `extension_ui_response` `confirmed: true`):

6. New test `untrusted_space_prompts_with_third_option_and_trust_space_outcome_trusts`:
   - Start a session in a tempdir cwd (NO space row yet — or an untrusted one) with `FAKE_PI_GATE=1`.
   - Wait for the `permission-request` event; assert its payload's `request.options` contains an option with `optionId: "trust-space"`.
   - `manager.respond_permission(&session_id, &request_id, PermissionOutcome::Selected { option_id: "trust-space" })` (the manager method the Tauri command wraps).
   - Assert the session proceeds (the gate was confirmed — wait for the settle/closed event the other tests wait for) AND the flag is now set: `db.space_trusted(&cwd)` is `true` (attach a tempdir `Db` via `attach_db` first; `upsert_space` the cwd beforehand so the row exists — `set_space_trusted` is an UPDATE).
7. New test `trusted_space_skips_the_permission_prompt`:
   - `upsert_space` the cwd + `set_space_trusted(&cwd, true)` on the attached db BEFORE starting the session.
   - Start the session with `FAKE_PI_GATE=1`; wait for the settle/closed event.
   - Assert the fake sink recorded NO `permission-request` event for this session (the gate was auto-confirmed).
8. Run the EXISTING `rpc_flow` tests unchanged (regression: the untrusted flow still prompts; the `allow`/`reject`/cancelled mapping is intact).

**Steps:**
- [ ] Write tests 6–7 in `src-tauri/tests/rpc_flow.rs` (they reference the new behavior; the new `handle_extension_ui_request` signature does NOT exist yet)
- [ ] Run `cargo test --test rpc_flow`
  - Did it fail with compile errors (missing parameters / the behavior not implemented)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–5 in `permission.rs` + `session.rs`
- [ ] Run `cargo test --test rpc_flow`
  - Did all tests pass (new + existing)? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test` (full suite — the `session.rs` unit/integration tests call `handle_extension_ui_request`-adjacent paths; any test calling the old signature must be updated to the new one)
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt` (from `src-tauri/`)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Zero warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(permission): auto-approve gated tools for trusted spaces"

**Acceptance criteria:**
- [ ] A trusted Space's `Confirm` requests are answered `confirmed: true` with no `permission-request` event emitted
- [ ] An untrusted Space's prompt offers exactly `[Allow, Block, Don't ask again for this Space]`; picking `trust-space` answers `confirmed: true` AND sets the flag (a failed flag write is logged, not fatal)
- [ ] `Select` requests are never auto-answered, trusted or not; `input`/`editor` immediate-cancel and the timeout/drain behavior are unchanged
- [ ] The change is at the single shared call site — subagent Sessions inherit with no separate code

---

### Task 3: Tauri command `set_space_trusted`

**Context:**
The frontend needs one command to set/clear the flag (the prompt's `trust-space` option is handled Rust-side in Task 2 — this command serves the sidebar shield button only). It lives with the other spaces commands in `commands/spaces.rs` (all take `State<'_, Arc<Db>>` and return `Result<_, String>`) and is registered in `lib.rs`'s `invoke_handler` next to them.

**Files:**
- Modify: `src-tauri/src/commands/spaces.rs`
- Modify: `src-tauri/src/lib.rs` (the `generate_handler!` list, ~lines 92–95)
- Test: `src-tauri/tests/ipc.rs`

**What to implement:**

1. In `src-tauri/src/commands/spaces.rs` (next to `delete_space`):
   ```rust
   /// Set (or clear) a space's trust flag (Trusted Space, ADR 0010).
   /// No-op if the row is missing.
   #[tauri::command]
   pub async fn set_space_trusted(
       state: State<'_, Arc<Db>>,
       path: String,
       trusted: bool,
   ) -> Result<(), String> {
       state.set_space_trusted(&path, trusted).map_err(|e| e.to_string())
   }
   ```
2. In `src-tauri/src/lib.rs`: add `commands::spaces::set_space_trusted,` to the `generate_handler!` list (next to `list_spaces` / `delete_space`).
3. In `src-tauri/tests/ipc.rs`: add `archimedes_desktop_lib::commands::spaces::set_space_trusted` to `build_app`'s handler list, and extend (or add next to) `spaces_and_agents_commands_round_trip` with a `set_space_trusted` round-trip: `invoke(&webview, "set_space_trusted", json!({ "path": <the space's canonical path>, "trusted": true }))` → `list_spaces` shows `trusted: true` → set `false` → `list_spaces` shows `trusted: false` → a missing path is a success no-op.

**Steps:**
- [ ] Write the `ipc.rs` test case (item 3) first
- [ ] Run `cargo test --test ipc`
  - Did it fail (the command is not registered / not found)? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–2
- [ ] Run `cargo test --test ipc`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test` (full suite)
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt` (from `src-tauri/`)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Zero warnings? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(commands): set_space_trusted tauri command"

**Acceptance criteria:**
- [ ] `set_space_trusted` is invokable over IPC, round-trips through `list_spaces`, and is a success no-op for a missing row
- [ ] The command is registered in `lib.rs` (the frontend `invoke` will find it)

---

### Task 4: Frontend — shield button on `SpaceGroup` + store + tauri wrapper

**Context:**
The trust toggle's second surface: a shield icon button in the `SpaceGroup` hover actions (alongside the existing `+` and chevron buttons — the codebase has NO context-menu pattern, so we follow the inline hover-button pattern). Untrusted: muted `Shield` icon. Trusted: `ShieldCheck` with the success color — the trusted state is visible at a glance, not only on hover. Clicking calls the new `set_space_trusted` command with an OPTIMISTIC store update (the spaces store has no live refresh — the flag must update locally immediately) and rolls back on command error. `PermissionPrompt` needs NO change here (Task 5 guards that).

**Files:**
- Modify: `src/lib/tauri.ts`
- Modify: `src/store/sessions.ts`
- Modify: `src/components/SpacesList.tsx`
- Test: `src/components/SpacesList.test.tsx`

**What to implement:**

1. `src/lib/tauri.ts`:
   - `SpaceRow` interface (~line 66): add `trusted: boolean;`
   - New wrapper (next to `deleteSpace`, ~line 430):
     ```ts
     export async function setSpaceTrusted(path: string, trusted: boolean): Promise<void> {
       return invoke("set_space_trusted", { path, trusted });
     }
     ```
2. `src/store/sessions.ts`:
   - `SpaceView` interface (~line 427): add `trusted: boolean;`
   - `spaceViewFor` (~line 442): the returned object gains `trusted: space.trusted,`
   - `SessionsState` + the store implementation (next to the existing spaces actions, ~lines 586/607): a new action
     ```ts
     setSpaceTrusted: (path: string, trusted: boolean) => {
       // Optimistic: flip the flag now, roll back if the command fails
       // (the spaces store has no live refresh — the flag must not wait
       // for a round-trip).
       const previous = get().spaces.find((s) => s.path === path)?.trusted ?? false;
       set((state) => ({
         spaces: state.spaces.map((s) => (s.path === path ? { ...s, trusted } : s)),
       }));
       void import("../lib/tauri")
         .then((t) => t.setSpaceTrusted(path, trusted))
         .catch(() => {
           console.error(`Failed to set trusted for ${path}:`);
           set((state) => ({
             spaces: state.spaces.map((s) => (s.path === path ? { ...s, trusted: previous } : s)),
           }));
         });
     },
     ```
     (Match the file's actual import style — if `sessions.ts` already imports from `../lib/tauri` at the top level, use a direct import + `catch` instead of the `import()` dance; the semantics are what matter: optimistic set, rollback on rejection.)
3. `src/components/SpacesList.tsx` — `SpaceGroup`:
   - Imports: add `ShieldIcon, ShieldCheckIcon` to the `lucide-react` import (the file's existing style: `ChevronDownIcon` etc.).
   - The `SpaceGroup` component needs the store action: `const setSpaceTrusted = useSessions((s) => s.setSpaceTrusted);`
   - Insert a third button BETWEEN the existing `+` button and the chevron button (same `rounded-md size-6 hover:bg-surface-hover` styling as its siblings):
     ```tsx
     <button
       type="button"
       aria-label={view.trusted ? `Stop trusting ${name}` : `Trust ${name}`}
       title={
         view.trusted
           ? "Stop trusting this Space"
           : "Trust this Space — skip permission prompts for bash/edit/write"
       }
       onClick={() => setSpaceTrusted(view.path, !view.trusted)}
       className="rounded-md size-6 hover:bg-surface-hover"
     >
       {view.trusted ? (
         <ShieldCheckIcon className="size-4 text-success" />
       ) : (
         <ShieldIcon className="size-4 text-foreground-subtlest" />
       )}
     </button>
     ```
4. `src/components/SpacesList.test.tsx` — follow the file's EXISTING mock pattern (how it seeds the store and mocks `../lib/tauri`):
   - Untrusted space renders the muted `Shield` icon (aria-label `Trust <name>`); trusted renders `ShieldCheck` (aria-label `Stop trusting <name>`).
   - Clicking the button: the store's `spaces` entry flips optimistically (synchronously) AND the tauri `set_space_trusted` wrapper is called with `(path, !previous)`.
   - The wrapper REJECTS: the store rolls back to the previous value.

**Steps:**
- [ ] Write the `SpacesList.test.tsx` cases (item 4) first
- [ ] Run `pnpm test` (from the repo root)
  - Did the new cases fail (the button/store action don't exist yet)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–3
- [ ] Run `pnpm test`
  - Did all tests pass (new + existing — the `SpaceView` shape change ripples to `spaceViewFor` consumers)? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm build` (type-check + build — the `SpaceRow`/`SpaceView` shape changes are compile-checked here)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(ui): trust toggle on space groups"

**Acceptance criteria:**
- [ ] Every Space group shows a shield button: muted `Shield` when untrusted, success-colored `ShieldCheck` when trusted
- [ ] Clicking flips the flag optimistically (visible immediately), calls `set_space_trusted`, and rolls back on rejection
- [ ] `PermissionPrompt.tsx` is UNTOUCHED in this task

---

### Task 5: Frontend — `PermissionPrompt` 3-option guard test

**Context:**
The design's load-bearing frontend assumption: `PermissionPrompt` renders one button per payload option (first = primary, the rest = outline), so the Rust-side third option (`trust-space`) appears with ZERO component changes. This task adds the guard test that pins that assumption — if a future refactor breaks the generic rendering, this test fails. There is NO implementation step: a test that passes immediately is the expected outcome here (it is a guard, not a TDD cycle).

**Files:**
- Test: `src/components/PermissionPrompt.test.tsx`

**What to implement:**

1. In `src/components/PermissionPrompt.test.tsx` (follow the file's existing store-seeding + tauri-mocking pattern): a new case where the prompt's `options` are exactly the Task-2 payload shape:
   ```ts
   options: [
     { optionId: "allow", name: "Allow", kind: "allow" },
     { optionId: "reject", name: "Block", kind: "reject" },
     { optionId: "trust-space", name: "Don't ask again for this Space", kind: "allow" },
   ]
   ```
   Assert: three option buttons render with the payload's `name`s (plus the existing Cancel); clicking the third calls `respondPermission` with `{ selected: { option_id: "trust-space" } }`.

**Steps:**
- [ ] Write the test case (item 1)
- [ ] Run `pnpm test`
  - This is a GUARD test — the expected outcome is that it PASSES immediately (the component is already generic). If it fails, the "zero frontend change" assumption is wrong: stop and report the failure — do NOT fix `PermissionPrompt.tsx` silently.
- [ ] Run `pnpm build`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "test(ui): permission prompt 3-option guard"

**Acceptance criteria:**
- [ ] The 3-option case passes WITHOUT any change to `PermissionPrompt.tsx`
- [ ] Clicking the `trust-space` option sends `{ selected: { option_id: "trust-space" } }` (the Task-2 Rust mapping's trigger)

---

## Final verification (after all 5 tasks, before merge)

- [ ] `pnpm test` (repo root) — green
- [ ] `pnpm build` (repo root) — green
- [ ] `cargo test` (from `src-tauri/`) — green
- [ ] `cargo clippy --all-targets` (from `src-tauri/`) — zero warnings
- [ ] `cargo fmt --check` (from `src-tauri/`) — clean
- [ ] Manual smoke (optional, per AGENTS.md's wire-smoke habit): start a session in a tempdir Space, toggle the shield, run a `bash` tool call — no prompt; toggle off — prompt returns
