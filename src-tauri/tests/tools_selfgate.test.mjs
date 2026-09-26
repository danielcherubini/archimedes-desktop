// The `tools.ts` SELF-GATE check (Phase 2, Task 3) — the one thing the
// `fake_pi` e2e (`desktop_tools.rs`) cannot verify, since the fake simulates
// the wire rather than executing `tools.ts`.
//
// Run under `pnpm test` (VITEST — `vitest run`; vitest's default include
// `**/*.test.(c|m)js` picks this up alongside the `.ts`/`.tsx` tests). It
// imports `src-tauri/assets/tools.ts` DIRECTLY (vitest transforms the raw TS
// — NO jiti/tsx loader needed; `tsconfig.json` `include: ["src"]` keeps
// `pnpm build` from type-checking `src-tauri/assets/tools.ts`, same as
// `gate.ts` today).
//
// The extension is SELF-GATED on the platform (Linux only) + the four
// `PI_ARCHIMEDES_BRIDGE_*` env vars. This check loads the factory with a stub
// `pi` whose `on` / `registerTool` are spies + a stubbed `process.platform`
// and asserts:
//   (a) `win32` / `darwin` + env PRESENT → `registerTool` NOT called (the
//       platform gate keeps the override inert — the Windows/macOS
//       no-regression case, even though the env is set).
//   (b) `linux` + env ABSENT → `registerTool` NOT called (the env gate — a
//       bridge-LESS spawn registers nothing; the suite's original tools
//       remain, no regression on macOS).
//   (c) `linux` + env PRESENT → `registerTool` IS called (DEFERRED — inside
//       the `session_start` handler, invoked by calling the captured handler)
//       for the three tools (`ask` / `sudo_exec` / `manage_todo_list`).

import { it, expect, afterEach } from "vitest";
import toolsFactory from "../assets/tools.ts";

const BRIDGE_ENV = [
  "PI_ARCHIMEDES_BRIDGE",
  "PI_ARCHIMEDES_BRIDGE_SOCKET",
  "PI_ARCHIMEDES_BRIDGE_SESSION",
  "PI_ARCHIMEDES_BRIDGE_SERVER_PID",
];

const ORIGINAL_PLATFORM = process.platform;
const ORIGINAL_ENV = Object.fromEntries(BRIDGE_ENV.map((k) => [k, process.env[k]]));

/// Set (or clear) the four bridge env vars. `present` = all four set
/// (`PI_ARCHIMEDES_BRIDGE=1` + the others truthy); `!present` = all four
/// deleted.
function setBridgeEnv(present) {
  for (const k of BRIDGE_ENV) {
    if (present) {
      process.env[k] = k === "PI_ARCHIMEDES_BRIDGE" ? "1" : "stub";
    } else {
      delete process.env[k];
    }
  }
}

/// Stub the `process.platform` (the extension reads it at load time — i.e.
/// when the factory is invoked).
function setPlatform(value) {
  Object.defineProperty(process, "platform", { value, configurable: true });
}

/// A stub `pi` whose `on` / `registerTool` are spies. `handlers` captures the
/// registered event handlers (the `session_start` handler is where the
/// DEFERRED `registerTool` calls live); `registered` captures the registered
/// tools.
function makeStubPi() {
  const registered = [];
  const handlers = {};
  const pi = {
    on: (event, handler) => {
      handlers[event] = handler;
    },
    registerTool: (tool) => {
      registered.push(tool);
    },
  };
  return { pi, registered, handlers };
}

afterEach(() => {
  // Restore the platform + the env (so a mutation in one case doesn't leak
  // into the next — the env vars are desktop-specific and absent by default,
  // but restore the originals to be robust on a machine that has them set).
  setPlatform(ORIGINAL_PLATFORM);
  for (const k of BRIDGE_ENV) {
    if (ORIGINAL_ENV[k] === undefined) {
      delete process.env[k];
    } else {
      process.env[k] = ORIGINAL_ENV[k];
    }
  }
});

it("(a) is inert on Windows (the platform gate) even with the env set", () => {
  setPlatform("win32");
  setBridgeEnv(true);
  const { pi, registered, handlers } = makeStubPi();
  toolsFactory(pi);
  // The platform gate returns BEFORE `pi.on` — no `session_start` handler, no
  // `registerTool`. The suite's original tools remain (no regression).
  expect(handlers.session_start).toBeUndefined();
  expect(registered).toHaveLength(0);
});

it("(a) is inert on macOS (the platform gate) even with the env set", () => {
  setPlatform("darwin");
  setBridgeEnv(true);
  const { pi, registered, handlers } = makeStubPi();
  toolsFactory(pi);
  expect(handlers.session_start).toBeUndefined();
  expect(registered).toHaveLength(0);
});

it("(b) is inert without the bridge env (the env gate) on Linux", () => {
  setPlatform("linux");
  setBridgeEnv(false);
  const { pi, registered, handlers } = makeStubPi();
  toolsFactory(pi);
  // The env gate returns BEFORE `pi.on` — a bridge-LESS spawn registers
  // nothing (the suite's original agent-local tools remain, no regression).
  expect(handlers.session_start).toBeUndefined();
  expect(registered).toHaveLength(0);
});

it("(c) registers the three tools DEFERRED (in session_start) on Linux with the env set", () => {
  setPlatform("linux");
  setBridgeEnv(true);
  const { pi, registered, handlers } = makeStubPi();
  toolsFactory(pi);
  // DEFERRED registration: the factory registers a `session_start` handler
  // (NOT `registerTool` at load time — a load-time same-name registration is
  // a `process.exit(1)` conflict with the suite's load-time tools).
  expect(typeof handlers.session_start).toBe("function");
  // At load time, `registerTool` is NOT called (the deferred registration is
  // absent at load-finalization → no conflict).
  expect(registered).toHaveLength(0);
  // Invoking the `session_start` handler registers the three tools (the
  // override wins: CLI `-e` loads first + first-wins merge).
  handlers.session_start();
  const names = registered.map((t) => t.name).sort();
  expect(names).toEqual(["ask", "manage_todo_list", "sudo_exec"]);
  // The override re-registers with the suite's labels (the same-name override).
  const byName = Object.fromEntries(registered.map((t) => [t.name, t]));
  expect(byName.ask.label).toBe("Ask");
  expect(byName.sudo_exec.label).toBe("sudo");
  expect(byName.manage_todo_list.label).toBe("Todo List");
});
