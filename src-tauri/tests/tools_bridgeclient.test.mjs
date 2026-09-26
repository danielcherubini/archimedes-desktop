// The `tools.ts` `bridgeRequest` SOCKET CLIENT check (Phase 2 review —
// test-coverage finding). The selfgate test (`tools_selfgate.test.mjs`)
// only stubs `pi.on` / `registerTool` and never drives the socket; this
// one drives the REAL `node:net` client against a REAL Unix-socket
// server (temp dir) to verify the FAILURE PATHS that `sudo_exec`
// depends on (`timeoutMs: null` → no client timer — its only safety
// nets are the `conn.on("close")` handler and the `AbortSignal` wiring).
//
// Run under `pnpm test` (vitest's default include `**/*.test.(c|m)js`
// picks this up alongside `tools_selfgate.test.mjs`). It imports
// `src-tauri/assets/tools.ts` directly (vitest transforms the raw TS)
// and gets the client through the test-only `pi.__test` seam (attached
// by the factory ONLY after the platform + env gates pass — inert in
// production: pi never reads `pi.__test`).
//
// Cases:
//   1. timeout fires (server accepts, never responds; 50 ms timer)
//   2. clean close → terminal "bridge connection closed" (THE critical
//      `sudo_exec` safety net — `timeoutMs: null`, only `close` settles)
//   3. error frame → typed error ({isError, raw}), not a resolve
//   4. pre-aborted AbortSignal → "cancelled" (no ReferenceError)
//   5. post-connect abort → "cancelled"
//   6. no leaked `abort` listeners after 4 + 5 settle
//   7. success path → resolves with the frame's result
// @vitest-environment node

import { it, expect, beforeEach, afterEach } from "vitest";
import net from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import toolsFactory from "../assets/tools.ts";

const BRIDGE_ENV = [
  "PI_ARCHIMEDES_BRIDGE",
  "PI_ARCHIMEDES_BRIDGE_SOCKET",
  "PI_ARCHIMEDES_BRIDGE_SESSION",
  "PI_ARCHIMEDES_BRIDGE_SERVER_PID",
];

const ORIGINAL_PLATFORM = process.platform;
const ORIGINAL_ENV = Object.fromEntries(BRIDGE_ENV.map((k) => [k, process.env[k]]));

let tmp; // temp dir holding the socket
let socketPath;
let server; // the test Unix-socket server
let accepted; // accepted server-side sockets (destroyed in afterEach)
let bridgeRequest; // the real client, via the `pi.__test` seam

/// Poll `fn` until truthy (or reject after `timeoutMs`) — waits for
/// socket-close events without fake timers (real short timeouts keep the
/// IO deterministic).
function eventually(fn, timeoutMs = 1500) {
  return new Promise((resolve, reject) => {
    const t0 = Date.now();
    const tick = () => {
      let value;
      try {
        value = fn();
      } catch {
        value = false;
      }
      if (value) {
        resolve(value);
      } else if (Date.now() - t0 > timeoutMs) {
        reject(new Error("condition not met in time"));
      } else {
        setTimeout(tick, 10);
      }
    };
    tick();
  });
}

/// A controller whose signal tracks the `abort` listeners that are
/// CURRENTLY registered (Node's `AbortSignal` — an `EventTarget` —
/// exposes no `listenerCount`, so the live set is tracked by wrapping
/// the signal's own methods; a `Set` mirrors `EventTarget` semantics —
/// removing a never-added listener is a no-op, not a negative count;
/// the client is the only listener here). Exposes
/// `signal.listenerCount("abort")` for the assertion.
function makeTrackedController() {
  const controller = new AbortController();
  const signal = controller.signal;
  const live = new Set();
  const add = signal.addEventListener.bind(signal);
  const remove = signal.removeEventListener.bind(signal);
  signal.addEventListener = (type, listener, opts) => {
    if (type === "abort") live.add(listener);
    return add(type, listener, opts);
  };
  signal.removeEventListener = (type, listener) => {
    if (type === "abort") live.delete(listener);
    return remove(type, listener);
  };
  signal.listenerCount = (type) => (type === "abort" ? live.size : 0);
  return controller;
}

/// A stub `pi` (the selfgate test's shape) — registration is irrelevant
/// here; we only need the factory to run far enough to attach the seam.
function makeStubPi() {
  return { on: () => {}, registerTool: () => {} };
}

/// Start a Unix-socket server at `socketPath`; `handler` runs per
/// accepted socket. Resolves once the server is listening.
function listen(handler) {
  server = net.createServer((socket) => {
    accepted.push(socket);
    handler(socket);
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => {
      server.removeListener("error", reject);
      resolve(server);
    });
  });
}

beforeEach(async () =>
  // linux + env present → the factory runs (gate passes) and attaches
  // the `pi.__test` seam. `bridgeRequest` reads the socket path at CALL
  // time (live `process.env`), so the per-test socket path below wins.
  (async () => {
    Object.defineProperty(process, "platform", { value: "linux", configurable: true });
    for (const k of BRIDGE_ENV) {
      process.env[k] = k === "PI_ARCHIMEDES_BRIDGE" ? "1" : "stub";
    }
    tmp = await mkdtemp(path.join(os.tmpdir(), "bridgeclient-"));
    socketPath = path.join(tmp, "bridge.sock");
    process.env.PI_ARCHIMEDES_BRIDGE_SOCKET = socketPath;
    accepted = [];
    const pi = makeStubPi();
    toolsFactory(pi);
    // The test-only seam (approach A — `tools.ts` stays self-contained;
    // pi loads it as a single file, so the client can't live elsewhere).
    bridgeRequest = pi.__test.bridgeRequest;
  })()
);

afterEach(async () => {
  // Clean up: destroy accepted sockets, close the server, remove the
  // temp dir (a leaked server socket would pin the vitest worker).
  for (const s of accepted) s.destroy();
  if (server) {
    await new Promise((resolve) => server.close(resolve));
  }
  server = undefined;
  await rm(tmp, { recursive: true, force: true });
  Object.defineProperty(process, "platform", { value: ORIGINAL_PLATFORM, configurable: true });
  for (const k of BRIDGE_ENV) {
    if (ORIGINAL_ENV[k] === undefined) {
      delete process.env[k];
    } else {
      process.env[k] = ORIGINAL_ENV[k];
    }
  }
});

it("(1) the timeout fires: accepts, never responds → rejects with a timeout error within ~1 s and the connection is cleaned up", async () => {
  // Accept and stay silent (but DRAIN the request — Node holds the peer's
  // FIN (`end`) until buffered data is consumed, so a no-op `data`
  // listener is needed for the cleanup assertion below); the ONLY thing
  // that can settle the promise is the client's 50 ms timer.
  await listen((socket) => {
    socket.on("data", () => {});
  });
  const t0 = Date.now();
  await expect(bridgeRequest("m", {}, new AbortController().signal, 50)).rejects.toThrow(
    "timeout",
  );
  expect(Date.now() - t0).toBeLessThan(1000); // fired on the timer, not a stall
  // The client cleaned up: it `conn?.destroy()`s on the terminal fail,
  // which sends a FIN the accepted socket sees as `readableEnded`
  // (a peer FIN does NOT set the server socket's own `destroyed`).
  await eventually(() => accepted[0].readableEnded);
});

it("(2) clean close with no response → terminal 'bridge connection closed' (the critical sudo_exec safety net: timeoutMs = null = no timer)", async () => {
  // Accept, wait for the request line, then end() (a clean FIN — NOT a
  // RST: a RST would surface as an `error` event and mask the `close`
  // handler, which is exactly the path under test).
  await listen((socket) => {
    socket.once("data", () => socket.end());
  });
  // `null` = no client timer: the `close` handler is the ONLY thing that
  // can settle this promise (without it the agent turn would hang until
  // a manual abort — the hazard the code comment names).
  await expect(bridgeRequest("m", {}, new AbortController().signal, null)).rejects.toThrow(
    "bridge connection closed",
  );
});

it("(3) an error frame → a typed error ({isError, raw}), NOT a resolve", async () => {
  // Accept, then answer the request with an error frame.
  await listen((socket) => {
    socket.once("data", () => {
      socket.write(JSON.stringify({ v: 1, type: "response", id: "test-id", error: "cancelled" }) + "\n");
    });
  });
  let err;
  try {
    await bridgeRequest("m", {}, new AbortController().signal, 5000);
  } catch (e) {
    err = e;
  }
  expect(err).toBeInstanceOf(Error); // rejected — NOT resolved with the frame
  expect(err.message).toBe("cancelled"); // the frame's error text
  expect(err.isError).toBe(true); // the typed marker the override maps to an isError result
  expect(err.raw).toEqual({ v: 1, type: "response", id: "test-id", error: "cancelled" }); // the raw frame
});

it("(4) a pre-aborted AbortSignal → rejects 'cancelled' quickly (the pre-connect path; no ReferenceError from conn?.destroy())", async () => {
  // No server needed: the pre-abort check fires BEFORE connect().
  const controller = makeTrackedController();
  controller.abort();
  const t0 = Date.now();
  let err;
  try {
    await bridgeRequest("m", {}, controller.signal, 50);
  } catch (e) {
    err = e;
  }
  expect(err).toBeInstanceOf(Error); // rejected, not thrown at the call site
  expect(err.message).toBe("cancelled");
  expect(Date.now() - t0).toBeLessThan(500); // settled immediately, not via the 50 ms timer
});

it("(5) aborting AFTER connect → rejects 'cancelled' (the AbortSignal → fail wiring on a live connection)", async () => {
  // Accept and stay silent — only the abort can settle the promise.
  await listen(() => {});
  const controller = makeTrackedController();
  const p = bridgeRequest("m", {}, controller.signal, null); // null = no timer
  await eventually(() => accepted.length > 0); // the connection is live before the abort
  controller.abort();
  await expect(p).rejects.toThrow("cancelled");
});

it("(6) no leaked `abort` listeners after the pre-abort + post-connect-abort paths settle", async () => {
  // Regression guard: the client removes its abort listener on settle —
  // a leak would pin a listener on the turn's long-lived AbortSignal.
  const pre = makeTrackedController();
  pre.abort();
  try {
    await bridgeRequest("m", {}, pre.signal, 50);
  } catch {
    // expected (case 4)
  }
  await listen(() => {});
  const post = makeTrackedController();
  const p = bridgeRequest("m", {}, post.signal, null);
  await eventually(() => accepted.length > 0);
  post.abort();
  try {
    await p;
  } catch {
    // expected (case 5)
  }
  expect(pre.signal.listenerCount("abort")).toBe(0);
  expect(post.signal.listenerCount("abort")).toBe(0);
});

it("(7) the success path: a result frame → resolves with the frame's result", async () => {
  // Accept, then answer the request with a result frame.
  await listen((socket) => {
    socket.once("data", () => {
      socket.write(JSON.stringify({ v: 1, type: "response", id: "test-id", result: { ok: true } }) + "\n");
    });
  });
  await expect(bridgeRequest("m", {}, new AbortController().signal, 5000)).resolves.toEqual({
    ok: true,
  });
});
