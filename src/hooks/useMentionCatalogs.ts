import { useEffect, useRef, useState } from "react";
import {
  listAgentDefinitionsForSpace,
  listMcpServersEffective,
  listSpaceFiles,
  type AgentDefinitionDto,
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
 * The empty `?` listing — a MODULE-LEVEL stable identity, because it is
 * the `empty` argument of `useCatalogPayload` and MUST NOT be a fresh
 * literal per render (see the effect's deps comment).
 */
const EMPTY_FILE_LIST: FileListDto = { entries: [], truncated: false };

/** The empty row array the array-shaped catalogs degrade to (stable). */
const EMPTY_ROWS: never[] = [];

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
): P {
  const key = spacePath ?? "__global__";
  const [state, setState] = useState<{ key: string; payload: P }>(() => ({
    key,
    payload: empty,
  }));
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
      if (!cancelled) setState({ key, payload: r });
    });
    // The deps below intentionally omit `cache`, `fetch` AND `empty`: all
    // three are stable BY CONSTRUCTION — `cache` is a module-level Map
    // (`agentsCache` / `mcpCache` / `filesCache`), `fetch` an imported
    // module function, and `empty` a module constant (`EMPTY_ROWS` /
    // `EMPTY_FILE_LIST`). None can change between renders, so none may be
    // tracked. `empty` is the one that would actually bite IF it ever were
    // tracked: were `empty` added to these deps AND a caller passing a fresh
    // literal (instead of the module constant), the effect would re-fire on
    // EVERY render, each run's `.then` writing a NEW payload object — an
    // infinite render loop. It is only a loop under BOTH conditions (the
    // tracked-`empty` one AND the caller-passes-a-literal one); today neither
    // holds, but a caller that swapped in a literal would satisfy the
    // CALLER-SIDE condition, and `empty` being tracked is what would turn that
    // into a loop — which is exactly why `empty` must never be tracked.
    // (The loop is also why `EMPTY_FILE_LIST` exists as a stable identity.)
    return () => {
      cancelled = true;
    };
  }, [spacePath, fetchName]);
  return state.key === key ? state.payload : empty;
}

/** An array-shaped catalog row (the agents / mcp lists). */
function useCatalogRow<T>(
  cache: Map<string, Promise<T[]>>,
  spacePath: string | null,
  fetch: (spacePath: string | null) => Promise<T[]>,
  fetchName: string,
): T[] {
  return useCatalogPayload<T[]>(cache, spacePath, fetch, fetchName, EMPTY_ROWS);
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
  );
  return { skills, agents, mcpServers, files };
}

/**
 * TEST-ONLY: clear the agents + mcp + files module-level caches (the
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
}
