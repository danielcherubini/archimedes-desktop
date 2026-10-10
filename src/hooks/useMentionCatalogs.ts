import { useEffect, useRef, useState } from "react";
import {
  listAgentDefinitionsForSpace,
  listCompletionEntries,
  listMcpServersEffective,
  listSpaceFiles,
  type AgentDefinitionDto,
  type CompletionDirDto,
  type FileListDto,
  type McpServerInfo,
  type SkillInfo,
} from "../lib/tauri";
import { useSkillCatalog } from "./useSkillCatalog";

/**
 * The FOUR composer catalogs for a Space (or `null` = user-level /
 * app-scope only) — skills, agents, MCP servers, and the `?` file
 * listing. Mirrors `useSkillCatalog`'s module-level promise cache (the
 * in-flight PROMISE is cached — the two-consumers-same-key in-flight
 * dedupe keeps working; a key change evicts the LEFT key; a key change
 * IMMEDIATELY serves the empty value until the new fetch resolves — a
 * stale Space's rows must never be offered; a failed fetch degrades to
 * the empty value with a `console.error`). Three module-level caches
 * (agents + mcp + files), keyed the same way
 * (`spacePath ?? "__global__"`).
 *
 * `null` key: `listAgentDefinitionsForSpace(null)` = user-level only
 * (the `cwd: None` command case); `listMcpServersEffective(null)` =
 * the desktop + pi-global layers (the `cwd: None` project-layer skip);
 * `listSpaceFiles(null)` = the empty listing (no Space, nothing to walk).
 */
const agentsCache = new Map<string, Promise<AgentDefinitionDto[]>>();
const mcpCache = new Map<string, Promise<McpServerInfo[]>>();
const filesCache = new Map<string, Promise<FileListDto>>();
/**
 * The out-of-Space Directory completion rows (ADR 0035), keyed by the
 * token's DIRECTORY PREFIX rather than by the Space — everything else is
 * the same machinery. NOT one of `useMentionCatalogs`' three — it belongs to
 * `useCompletionDir` below. Keying per directory (not per keystroke) is what
 * makes "one listing per directory" true: the engine returns the whole
 * UNFILTERED directory (Task 1) so the renderer filters it synchronously
 * and the token's non-directory tail never invalidates the entry.
 */
const completionCache = new Map<string, Promise<CompletionDirDto>>();

/**
 * The empty `?` listing — a MODULE-LEVEL stable identity, pinned by
 * `the_empty_files_value_is_one_module_constant_across_consumers` in
 * `useMentionCatalogs.test.ts` (mutation-checked: a fresh object literal at the
 * call site reddens it, and reddens NOTHING else — which is why the pin exists).
 * It must not be a fresh literal per render because it is the `empty` argument of
 * `useCatalogPayload`: the caller-side half of the render-loop hazard that hook's
 * deps comment describes. Stated exactly, because the hazard is conditional — the
 * loop needs BOTH a literal caller AND `empty` tracked in those deps, and today
 * neither half holds. Nobody has ever observed the loop; what HAS been observed is
 * a literal caller passing every test in the file, so identity is asserted here
 * rather than predicted.
 */
const EMPTY_FILE_LIST: FileListDto = { entries: [], truncated: false };

/** The empty row array the array-shaped catalogs degrade to (stable). */
const EMPTY_ROWS: never[] = [];

/**
 * The empty Directory completion payload — a MODULE-LEVEL stable identity
 * for the same reason `EMPTY_FILE_LIST` exists above (it is the `empty`
 * argument of `useCatalogPayload`), and pinned the same way: the identity
 * assertion in `the_null_dir_prefix_serves_the_stable_empty_dir` reddens when
 * this constant is replaced by a fresh literal at the call site
 * (mutation-checked). The loop itself is conditional — see `EMPTY_FILE_LIST`.
 */
const EMPTY_COMPLETION_DIR: CompletionDirDto = { entries: [], truncated: false };

/**
 * The shared freshness machinery for one catalog (the agents, mcp and
 * files rows). Replicates `useSkillCatalog`'s `{ key, rows }` state
 * pattern verbatim, generalised from a ROW ARRAY to any PAYLOAD (the
 * files row is `{ entries, truncated }`, not an array): a key change
 * immediately serves `empty` until the new fetch resolves (the state
 * tracks the key its payload belongs to); the LEFT key is evicted when
 * the effect re-runs for a different key (an unchanged key does NOT
 * evict — a remount reuses the settled cache entry); the cache stores
 * the IN-FLIGHT PROMISE (the two-consumers-same-key in-flight dedupe);
 * a failed fetch degrades to `empty` (logged, and the failed promise is
 * only evicted if it is still the cached entry — a test's
 * `clearMentionCatalogsCache()` + re-mount may have cached a FRESH one
 * under the same key).
 */
function useCatalogPayload<P>(
  cache: Map<string, Promise<P>>,
  spacePath: string | null,
  fetch: (spacePath: string | null) => Promise<P>,
  fetchName: string,
  empty: P,
): { payload: P; pending: boolean } {
  const key = spacePath ?? "__global__";
  // ONE state object, deliberately: `payload` and `settled` must move together,
  // because "the rows on screen" and "the fetch is over" are the same claim
  // about the same key. Two separate `useState`s could be observed in between
  // (a key change with an older key's payload on screen but a newer key's
  // settle flag), which is precisely the window `pending` is meant to describe.
  const [state, setState] = useState<{
    key: string;
    payload: P;
    settled: boolean;
  }>(() => ({ key, payload: empty, settled: false }));
  const prevKeyRef = useRef<string | null>(null);
  useEffect(() => {
    const prevKey = prevKeyRef.current;
    prevKeyRef.current = key;
    if (prevKey !== null && prevKey !== key) {
      cache.delete(prevKey); // stale on disk: re-fetch when we come back
    }
    let cancelled = false;
    let p = cache.get(key);
    if (!p) {
      p = fetch(spacePath)
        .catch((err) => {
          // A failed fetch degrades to the empty value (logged — the app's
          // convention for a non-critical background fetch failure).
          // A FAILED promise is NOT cached (delete it) so a later effect
          // retries — but only if THIS promise is still the cached entry
          // (a test's `clearMentionCatalogsCache()` + re-mount may have
          // cached a FRESH one under the same key; an unconditional
          // delete would evict it).
          console.error(`${fetchName} failed:`, err);
          if (cache.get(key) === p) cache.delete(key);
          return empty;
        });
      cache.set(key, p);
    }
    void p.then((r) => {
      // Stale-response guard: the Space changed while in flight.
      if (!cancelled) {
        // A REJECTION lands here too (the `.catch` above converted it to
        // `empty`), which is exactly what makes this the right place to mark a
        // settle: "settled" means "the fetch is over", never "the fetch
        // returned rows". A dead directory has NO rows ever, and a caller that
        // inferred pending-from-rows would hold Enter down there forever.
        setState({ key, payload: r, settled: true });
      }
    });
    // The deps below intentionally omit `cache`, `fetch` AND `empty`: all
    // three are stable BY CONSTRUCTION — `cache` is a module-level Map
    // (`agentsCache` / `mcpCache` / `filesCache` / `completionCache`), `fetch`
    // an imported module function, and `empty` a module constant (`EMPTY_ROWS`
    // / `EMPTY_FILE_LIST` / `EMPTY_COMPLETION_DIR`). None can change between
    // renders, so none may be tracked. `empty` is the one that would actually
    // bite IF it ever were tracked: were `empty` added to these deps AND a
    // caller passing a fresh literal (instead of the module constant), the
    // effect would re-fire on EVERY render, each run's `.then` writing a NEW
    // payload object — an infinite render loop. Read that sentence for what it
    // IS: a conditional about a configuration this app does not have. It needs
    // BOTH halves (tracked `empty` AND a literal caller) and neither holds, so
    // the loop has never been observed here — a caller swapping in a literal on
    // its own leaves this hook untouched (measured: the whole suite stayed
    // green). That is precisely why the two stable-identity constants are
    // pinned by IDENTITY assertions in `useMentionCatalogs.test.ts` rather than
    // left to this paragraph: the observable contract today is "the empty value
    // is one object", and that is what a test can actually see.
    // (`empty` must therefore never be tracked — that is the rule, not the
    // loop.)
    return () => {
      cancelled = true;
    };
  }, [spacePath, fetchName]);
  return {
    payload: state.key === key ? state.payload : empty,
    // THE INVARIANT, and it is load-bearing: `pending === true` ⟹ `payload` is
    // the EMPTY constant. Both halves read the same two fields of one state
    // object, so they can never disagree — which matters because `pending` is
    // only ever a reason to HOLD a keystroke while there is nothing to insert,
    // and a `pending` that could be true beside a screen full of rows would
    // swallow an insert the user can see. `state.payload` is the `empty` value
    // whenever `settled` is false (it is only ever written by the settle), so
    // the `state.key === key && !settled` branch cannot show rows either.
    pending: state.key === key ? !state.settled : true,
  };
}

/** An array-shaped catalog row (the agents / mcp lists). */
function useCatalogRow<T>(
  cache: Map<string, Promise<T[]>>,
  spacePath: string | null,
  fetch: (spacePath: string | null) => Promise<T[]>,
  fetchName: string,
): T[] {
  return useCatalogPayload<T[]>(cache, spacePath, fetch, fetchName, EMPTY_ROWS)
    .payload;
}
export function useMentionCatalogs(
  spacePath: string | null,
): {
  skills: SkillInfo[];
  agents: AgentDefinitionDto[];
  mcpServers: McpServerInfo[];
  files: FileListDto;
} {
  const skills = useSkillCatalog(spacePath);
  const agents = useCatalogRow(
    agentsCache,
    spacePath,
    listAgentDefinitionsForSpace,
    "listAgentDefinitionsForSpace",
  );
  const mcpServers = useCatalogRow(
    mcpCache,
    spacePath,
    listMcpServersEffective,
    "listMcpServersEffective",
  );
  const files = useCatalogPayload(
    filesCache,
    spacePath,
    listSpaceFiles,
    "listSpaceFiles",
    EMPTY_FILE_LIST,
  ).payload;
  return { skills, agents, mcpServers, files };
}

/**
 * The out-of-Space completion rows for ONE directory (ADR 0035). `null`
 * = no out-of-Space token open; it maps to the `__global__` cache key and
 * the command's `None` arm, which touches no filesystem at all.
 *
 * `null` is still a FETCH: the wrapper is called with `null` on that cache miss
 * (exactly as `listSpaceFiles(null)` is), so "no token costs nothing" is a
 * RUST-side guarantee, pinned there, and NOT something a hook test can show by
 * absence — an assertion here that the fetcher was not called could never go
 * green, and mutation-checked, a client-side short-circuit for `null` reddens
 * `the_null_dir_prefix_serves_the_stable_empty_dir` rather than passing quietly.
 *
 * ONE accepted consequence of the machinery being shared: the LEFT key is evicted
 * (the same rule the Space catalogs use, so a directory edited on disk mid-session
 * is re-listed), which also means re-entering a directory you walked away from
 * reads it again instead of serving the row set you left behind. That is the
 * freshness half of the trade — stale rows must never be insertable into a token
 * that no longer names their directory.
 *
 * THE OTHER HALF OF THE GAP (why the return shape is a pair and not the DTO).
 * Freshness is served by serving `empty` for a key that has not landed — which
 * leaves the picker with ZERO ROWS for one IPC round trip, and the row count is
 * what gates the Enter intercept in `ComposerRow` (ADR 0033's "a picker is never
 * a gate" is implemented as "no rows, so nothing to intercept"). So a blank
 * picker is not merely a STALENESS question, it is an INPUT question: while the
 * rows for the directory the user just clicked into are in flight, Enter would
 * send the half-typed `?/home/u/.config/htop/` instead of waiting for the row it
 * was about to be offered. `pending` is that signal: true ONLY while a fetch for
 * the CURRENT non-null `dirPrefix` has not settled, false for `null`, false for
 * a cache hit that has settled, false after a resolve AND false after a REJECT
 * (a dead directory is blank forever, so a flag derived from "rows arrived"
 * would swallow Enter there for good). It is derived from the fetch SETTLING —
 * never from a timer. Pinned clause by clause in `useMentionCatalogs.test.ts`
 * and end-to-end in `ChatStream.test.tsx`.
 *
 * The returned WRAPPER object is a fresh literal each render (React does not
 * care; nothing memoises on it), so the half that carries the identity promise
 * is `.dir`: it stays the module-level `EMPTY_COMPLETION_DIR` while empty, and
 * `ChatStream` uses `.dir` (never the wrapper) as its `useMemo` dep. That is why
 * the identity assertions in `useMentionCatalogs.test.ts` read through `.dir`.
 *
 * ADR 0035's own note is worth correcting when it is next read: it reasoned
 * about this window ONLY as a staleness question — "blank is better than stale
 * and insertable" — and the half that was MISSING is the input half, that
 * blankness is also what removes the Enter intercept. Both halves are true; the
 * fix keeps the blankness (it is still the right answer for stale rows) and
 * holds Enter still for the one round trip it takes to know which one this is.
 */
export function useCompletionDir(dirPrefix: string | null): {
  dir: CompletionDirDto;
  pending: boolean;
} {
  const { payload, pending: rawPending } = useCatalogPayload<CompletionDirDto>(
    completionCache,
    dirPrefix,
    listCompletionEntries,
    "listCompletionEntries",
    EMPTY_COMPLETION_DIR,
  );
  return {
    dir: payload,
    // `dirPrefix === null` is NOT a pending fetch even though the hook still
    // calls the fetcher with `null` on that cache miss (see above): with no
    // out-of-Space token open there is no descent in flight to wait for, and a
    // flag that counted that fetch would swallow Enter in an ordinary composer.
    pending: dirPrefix !== null && rawPending,
  };
}

/**
 * TEST-ONLY: clear the agents + mcp + files + completion module-level caches (the
 * `clearSkillCatalogCache` pattern from `useSkillCatalog` — the skills
 * cache itself is cleared by `clearSkillCatalogCache()`, which the
 * consuming test files' `beforeEach` already calls). Without the clear,
 * a key cached by an earlier test in the file serves WARM rows and the
 * gap-state / called-once assertions fail.
 */
export function clearMentionCatalogsCache(): void {
  agentsCache.clear();
  mcpCache.clear();
  filesCache.clear();
  completionCache.clear();
}
