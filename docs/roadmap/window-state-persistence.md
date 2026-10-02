---
status: committed
done-when: The app window restores its previous size, position, and maximized state on every launch (all three platforms), with no flash of the default 800x600, and the standard gates (cargo test / clippy / fmt / pnpm test / pnpm build) are green.
---

# Window state persistence — Plan

**Goal:** Make the Client remember its window size + position + maximized state across restarts, on all three platforms, using the official `tauri-plugin-window-state` plugin.
**Architecture:** Register the plugin in `run()` so it auto-restores window state on `on_window_ready` and auto-saves on `RunEvent::Exit`. Set the `main` window to `visible: false` in the Tauri config so the user does not flash the default 800×600 before the saved state is applied. Zero frontend changes — the entire behavior is in the Rust plugin.
**Tech Stack:** Tauri 2 (2.11.5) + `tauri-plugin-window-state` (2.x).
**Reference:** `docs/decisions/0021-window-state-plugin.md` (ADR 0021 — rationale, rejected alternatives, accepted risks).

---

### Task 0: Fix the `bridge_integration` python3 availability gate (baseline / pre-existing CI failure)

**Context:**
The 4 tests in `tests/bridge_integration.rs` (Linux-only; they spawn a real Python stub over a Unix-socket bridge) were **falsely failing on a clean `main`** before this plan started. Root cause: the test's availability gate `python3_available()` (lines 319-329) checks `python3 --version`, which **succeeds even when the interpreter is broken** (e.g. `PYTHONHOME` points at a PyInstaller/flatpak mount dir that lacks `encodings`, so `python3 -c 'import sys'` dies with "Failed to import encodings"). The gate returns `true` → the test does not skip → it spawns the stub → the stub crashes (can't import stdlib) → writes nothing → the test **times out and fails**. The test's own doc comment states the intent: *"a CI runner without python3 must not hard-fail — the tests skip instead."* This task makes the detection match that intent so the tests **skip (pass vacuously) instead of falsely failing** when `python3` is not actually usable. This is a test-infrastructure robustness fix only — no app code changes. It is a prerequisite so the branch's `cargo test` baseline is green before the feature work.

**Files:**
- Modify: `src-tauri/tests/bridge_integration.rs`

**What to implement:**

1. In `src-tauri/tests/bridge_integration.rs`, change `python3_available()` (lines 319-329) so it checks actual interpreter capability instead of `--version`. Replace the `.arg("--version")` line with a probe that imports the stdlib:
   ```rust
   fn python3_available() -> bool {
       std::process::Command::new("python3")
           .arg("-c")
           .arg("import sys")
           .stdout(std::process::Stdio::null())
           .stderr(std::process::Stdio::null())
           .spawn()
           .ok()
           .and_then(|mut c| c.wait().ok())
           .map(|s| s.success())
           .unwrap_or(false)
   }
   ```
   Rationale (verified): on a healthy machine `python3 -c 'import sys'` exits 0 → gate true → the real end-to-end tests run. On a machine where `python3` is present but broken (e.g. `PYTHONHOME` misconfigured), `python3 -c 'import sys'` exits non-zero → gate false → the 4 tests `return` early (pass vacuously) instead of timing out. `python3 --version` is the wrong probe because it is handled before stdlib import and succeeds even when the interpreter cannot run.
2. Update the doc comment immediately above `python3_available()` (lines 316-318) to reflect the stronger detection. It currently reads:
   ```rust
   /// Detect whether `python3` is usable (the suite is the end-to-end
   /// gate on dev machines, but a CI runner without python3 must not
   /// hard-fail — the tests skip instead).
   ```
   Change it to:
   ```rust
   /// Detect whether `python3` is USABLE (can actually import the stdlib,
   /// not merely respond to `--version` — `--version` succeeds even when
   /// `PYTHONHOME` is misconfigured and the interpreter cannot run). The
   /// suite is the end-to-end gate on dev machines, but a CI runner without
   /// a usable python3 must not hard-fail — the tests skip instead.
   ```

**What NOT to change:**
- Do not touch the 4 test bodies, the `STUB` constant, or any other helper in `bridge_integration.rs`.
- Do not modify any application code in `src/` — this is test-infrastructure only.
- Do not set/clear `PYTHONHOME` or any environment variable inside the test (that would be a hack and wouldn't help a real CI runner; correct detection is the fix).

**Steps:**

- [ ] Apply the two edits above (the `python3_available()` probe + the doc comment) in `src-tauri/tests/bridge_integration.rs`.
- [ ] Run `cargo test --test bridge_integration` (from `src-tauri/`)
  - On a machine with a healthy `python3`: the 4 tests should **run and pass** (real end-to-end). On a machine where `python3` is present-but-broken (like this sandbox, where `PYTHONHOME` is misconfigured): the 4 tests should **pass vacuously** (each `return`s early after printing "python3 ... skipping"), NOT time out and fail. Either outcome is a PASS for this task.
- [ ] Run `cargo test` (full, from `src-tauri/`)
  - Did all tests pass (424 unit + the 4 bridge_integration no longer failing)? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --check` (from `src-tauri/`)
  - Did it succeed? If not, run `cargo fmt` and re-run `cargo fmt --check` before continuing.
- [ ] Commit with message: `test: skip bridge_integration when python3 is unusable (not merely absent)`.

**Acceptance criteria:**
- [ ] `python3_available()` probes `python3 -c 'import sys'` (real interpreter capability), not `python3 --version`.
- [ ] The doc comment above `python3_available()` explains the stronger detection.
- [ ] `cargo test` (full) is green — the 4 `bridge_integration` tests no longer falsely fail (they either run-and-pass or skip-and-pass).
- [ ] `cargo clippy --all-targets` is 0 warnings and `cargo fmt --check` is clean.
- [ ] No application code (`src/`) and no other test file were modified.

---

### Task 1: Register the `window-state` plugin and suppress the startup flash

**Context:**
The Client currently opens at a fixed 800×600 every launch because Tauri core has no window-state persistence — the size comes solely from `src-tauri/tauri.conf.json` and nothing observes or saves resize events. This single task wires in the official `tauri-plugin-window-state` so the window's full state (size + position + maximized + visible + decorations + fullscreen, i.e. `StateFlags::all()`) is restored on launch and saved on quit, and adds `visible: false` so the default size is not flashed before restore. This is the entire feature in one commit.

Why `visible: false` is safe on first run (verified from the plugin source, `plugins/window-state/src/lib.rs`, v2 branch): in `restore_state`, `should_show` is initialized to `true` (line 196) and is only overwritten by the cached `state.visible` when saved state **exists** (line 244). On first run there is no cache entry, so `should_show` stays `true` and the plugin calls `self.show()?` + `self.set_focus()?` (lines 279-282). A `visible: false` window therefore cannot stay hidden forever on a fresh install. On **subsequent** runs the plugin sets `should_show = state.visible` and shows the window only if the persisted `visible` is `true`; that value is re-derived from `is_visible()` at save time (the save fires on `CloseRequested`, while the window is still visible, so the normal quit path persists `visible: true` and the window re-shows). Residual low-probability risk: `restore_state` returns early (via `?`) if `set_position`/`set_size`/`maximize`/`set_fullscreen` errors before `show()`, which would leave a `visible: false` window hidden — this is gated by the acceptance test asserting the window APPEARS on relaunch (below).

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock` (auto-updated by `cargo build` — a new `[[package]]` entry for `tauri-plugin-window-state` + transitive deps; MUST be committed alongside the other changes)
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/tauri.conf.json`
- (No test file — see TDD note below.)

**Version note:** `tauri-plugin-window-state = "2"` will resolve to **2.4.0** (NOT 2.5.0) because `Cargo.lock` pins `tauri 2.11.5` and 2.5.0 requires `tauri >= 2.12`. This is fine — the restore/show logic verified below is identical in 2.4.0 (the cited line numbers are from the v2-branch source; in the 2.4.0 crate the corresponding lines are ~179/~227, not 196/244). If escalating later to 2.5.0, a `tauri` bump to 2.12+ is required first.

**What to implement:**

1. `src-tauri/Cargo.toml` — in the `[dependencies]` section, immediately after the existing `tauri-plugin-opener = "2"` line, add:
   ```toml
   tauri-plugin-window-state = "2"
   ```
   Do NOT add it to any `[target.'cfg(...)'.dependencies]` block — the plugin is cross-platform (Windows/macOS/Linux all supported) and belongs in the main `[dependencies]` list alongside the other Tauri plugins. Do NOT add the npm `@tauri-apps/plugin-window-state` package — the automatic save-on-exit/restore-on-launch needs zero frontend code and zero capability changes (verified: the behavior is entirely in the Rust plugin; the JS package + `window-state:default` capability are only needed if you call the manual `saveWindowState`/`restoreStateCurrent` commands from the webview, which we do not).

2. `src-tauri/src/lib.rs` — in `run()`, the plugin registration chain currently reads (lines 76-78):
   ```rust
   tauri::Builder::default()
       .plugin(tauri_plugin_dialog::init())
       .plugin(tauri_plugin_opener::init())
   ```
   Add one line after the `opener` line:
   ```rust
       .plugin(tauri_plugin_window_state::Builder::default().build())
   ```
   Use `Builder::default().build()` — this gives `StateFlags::all()` (size + position + maximized + visible + decorations + fullscreen) and the default filename `.window-state.json`. Do NOT configure a custom builder (no `with_state_flags`, `with_filename`, `with_denylist`, `skip_initial_state`, or `map_label`); the defaults are exactly what this feature needs.

3. `src-tauri/tauri.conf.json` — in the `app.windows[0]` object (currently lines 14-21), add a `"visible": false` field. The object should become:
   ```json
   "windows": [
     {
       "title": "Archimedes",
       "width": 800,
       "height": 600,
       "dragDropEnabled": false,
       "decorations": false,
       "transparent": true,
       "visible": false
     }
   ]
   ```
   Keep `width: 800` / `height: 600` as the first-run default. Do NOT change `decorations` or `transparent` — they stay as-is (and `decorations: false` partly mitigates the plugin's Linux CSD drift, ADR 0021).

**What NOT to change:**
- Do not touch the `.setup` closure, `setup_dirs`, the `invoke_handler`, or any command.
- Do not add any `window_state` field to `Settings` in `src-tauri/src/commands/settings.rs` — that is the manual approach B, which was rejected (ADR 0021). The plugin writes its own `.window-state.json`; it does not use the app's `settings.json`.
- Do not add any frontend/React code, any `@tauri-apps/plugin-window-state` import, or any capability entry in `src-tauri/capabilities/default.json`.
- Do not add `minWidth`/`maxWidth`/`center` to the window config — out of scope (the plugin does no off-screen clamping; that is an accepted limitation, ADR 0021).

**Steps:**

- [ ] **TDD note (why there is no automated failing test):** this change is a window-lifecycle behavior that the headless `tests/ipc.rs` (mock `tauri::test` runtime) cannot exercise — it cannot create a real OS window, resize it, or observe `on_window_ready`/`RunEvent::Exit`. The executable verification for this task is therefore the manual acceptance test in the Acceptance criteria below, run against a real `tauri dev` build. This is the one deliberate exception to the repo's TDD rule, and it is documented here so the executing agent does not fabricate a non-runnable test.
- [ ] Add the three changes above (Cargo.toml dep, `lib.rs` plugin line, `tauri.conf.json` `visible: false`).
- [ ] Run `cargo build` (from `src-tauri/`)
  - Did it succeed (the plugin compiles and links)? If not, fix and re-run before continuing. (First build will fetch `tauri-plugin-window-state` and update `Cargo.lock` — this is expected.)
- [ ] Run `cargo clippy --all-targets` (from `src-tauri/`)
  - Did it succeed with 0 warnings? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --check` (from `src-tauri/`)
  - Did it succeed? If not, run `cargo fmt` and re-run `cargo fmt --check` before continuing.
- [ ] Run `cargo test` (from `src-tauri/`)
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `pnpm test` and `pnpm build` (from the repo root)
  - Did both succeed? (Expected to be unaffected — no frontend changes — but confirm the build still passes with the new dependency graph.)
- [ ] **Manual acceptance test** (run against a real `tauri dev` / packaged build, on the platform(s) available):
  1. Remove any existing state file (e.g. `rm -f ~/.local/share/codes.archimedes.desktop/.window-state.json` on Linux; the equivalent Tauri app-data dir on macOS/Windows) → launch → the window **appears at 800×600** (proves first-run show works — the window is NOT left invisible).
  2. Resize the window, move it, and maximize it; then quit normally (the custom close button → `close()`). Confirm the state file now exists in the Tauri app-data dir.
  3. Relaunch → the window **appears** (becomes visible) AND comes back at the same size/position/maximized, with **no visible flash** of 800×600. (Assert visibility explicitly — not just geometry — since re-show depends on the persisted `visible` field.)
  4. (all three shipped platforms: macOS Retina, Linux, and Windows — Windows is in `bundle.targets`) Repeat steps 2–3 twice more; the window must **appear** and the size must be **stable** — it must not grow/double per launch (regression watch for plugin #3521 macOS doubling / #3553 Linux drift).
  5. (Linux, WebKitGTK) Confirm the window reliably APPEARS on first run with `visible: false` + `transparent: true` + `decorations: false` (the three interact on WebKitGTK). If the window does NOT appear on some Linux configuration, the fallback is to remove `"visible": false` and accept the brief flash of 800×600 before restore — report this as a known platform limitation rather than shipping an invisible app.
- [ ] Commit with message: `feat: remember window size/position/maximized across restarts (window-state plugin)` — stage `Cargo.toml`, `Cargo.lock` (the auto-updated lockfile MUST be in the commit), `src/lib.rs`, and `tauri.conf.json` together.

**Acceptance criteria:**
- [ ] `cargo build`, `cargo clippy --all-targets` (0 warnings), `cargo fmt --check`, `cargo test` (from `src-tauri/`) and `pnpm test` + `pnpm build` (from repo root) are all green.
- [ ] A fresh launch with no state file shows the window at 800×600 (not invisible).
- [ ] After a normal quit, a state file exists in the Tauri app-data dir; the next launch restores the previous size/position/maximized with no flash of the default size.
- [ ] Two consecutive relaunches on macOS (Retina), Linux, and Windows show the window appearing at a stable size (no per-launch growth/doubling).
- [ ] No frontend files, no npm package, and no capability file were modified.
