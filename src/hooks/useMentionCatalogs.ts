import { useEffect, useRef, useState } from "react";
import {
  listAgentDefinitionsForSpace,
  listMcpServersEffective,
  type AgentDefinitionDto,
  type McpServerInfo,
  type SkillInfo,
} from "../lib/tauri";
import { useSkillCatalog } from "./useSkillCatalog";

/**
 * The three mention catalogs for a Space (or `null` = user-level /
 * app-scope only). Mirrors `useSkillCatalog`'s module-level promise
 * cache (the in-flight PROMISE is cached — the two-consumers-same-key
 * in-flight dedupe keeps working; a key change evicts the LEFT key; a
 * key change IMMEDIATELY serves `[]` until the new fetch resolves — a
 * stale Space's rows must never expand; a failed fetch degrades to `[]`
 * with a `console.error`). Two module-level caches (agents + mcp), keyed
 * the same way (`spacePath ?? "__global__"`).
 *
 * `null` key: `listAgentDefinitionsForSpace(null)` = user-level only
 * (the `cwd: None` command case); `listMcpServersEffective(null)` =
 * the desktop + pi-global layers (the `cwd: None` project-layer skip).
 */
const agentsCache = new Map<string, Promise<AgentDefinitionDto[]>>();
const mcpCache = new Map<string, Promise<McpServerInfo[]>>();

/**
 * The shared freshness machinery for one catalog (the agents and mcp
 * rows). Replicates `useSkillCatalog`'s `{ key, rows }` state pattern
 * verbatim: a key change immediately serves [] until the new fetch
 * resolves (the state tracks the key its rows belong to); the LEFT key
 * is evicted when the effect re-runs for a different key (an unchanged
 * key does NOT evict — a remount reuses the settled cache entry); the
 * cache stores the IN-FLIGHT PROMISE (the two-consumers-same-key
 * in-flight dedupe); a failed fetch degrades to [] (logged, and the
 * failed promise is only evicted if it is still the cached entry — a
 * test's `clearMentionCatalogsCache()` + re-mount may have cached a
 * FRESH one under the same key).
 */
function useCatalogRow<T>(
  cache: Map<string, Promise<T[]>>,
  spacePath: string | null,
  fetch: (spacePath: string | null) => Promise<T[]>,
  fetchName: string,
): T[] {
  const key = spacePath ?? "__global__";
  const [state, setState] = useState<{ key: string; rows: T[] }>(() => ({
    key,
    rows: [],
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
          // A failed fetch degrades to [] (logged — the app's convention
          // for a non-critical background fetch failure).
          // A FAILED promise is NOT cached (delete it) so a later effect
          // retries — but only if THIS promise is still the cached entry
          // (a test's `clearMentionCatalogsCache()` + re-mount may have
          // cached a FRESH one under the same key; an unconditional
          // delete would evict it).
          console.error(`${fetchName} failed:`, err);
          if (cache.get(key) === p) cache.delete(key);
          return [] as T[];
        });
      cache.set(key, p);
    }
    void p.then((r) => {
      // Stale-response guard: the Space changed while in flight.
      if (!cancelled) setState({ key, rows: r });
    });
    // The deps below intentionally omit `cache` and `fetch`: both are
    // module-level constants the callers pass (`agentsCache` / `mcpCache` +
    // the imported fetch fns), so they are stable BY CONSTRUCTION and can
    // never change between renders.
    return () => {
      cancelled = true;
    };
  }, [spacePath, fetchName]);
  return state.key === key ? state.rows : [];
}

export function useMentionCatalogs(
  spacePath: string | null,
): { skills: SkillInfo[]; agents: AgentDefinitionDto[]; mcpServers: McpServerInfo[] } {
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
  return { skills, agents, mcpServers };
}

/**
 * TEST-ONLY: clear the agents + mcp module-level caches (the
 * `clearSkillCatalogCache` pattern from `useSkillCatalog` — the skills
 * cache itself is cleared by `clearSkillCatalogCache()`, which the
 * consuming test files' `beforeEach` already calls). Without the clear,
 * a key cached by an earlier test in the file serves WARM rows and the
 * gap-state / called-once assertions fail.
 */
export function clearMentionCatalogsCache(): void {
  agentsCache.clear();
  mcpCache.clear();
}
