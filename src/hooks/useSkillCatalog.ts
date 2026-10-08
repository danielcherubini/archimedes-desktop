import { useEffect, useRef, useState } from "react";
import { listSkills, type SkillInfo } from "../lib/tauri";

/**
 * The skill catalog for a Space (or `null` = user-level skills only).
 *
 * Cached in a MODULE-LEVEL `Map<string, Promise<SkillInfo[]>>` keyed by
 * `spacePath ?? "__global__"` so the left pane AND the composer (two
 * components, same key) share ONE fetch. Re-fetched when `spacePath`
 * changes (the active Space changed) or on first mount (app start).
 * No loading state (the spec: discovery is a local disk walk — the
 * section simply re-renders when the result arrives). A failed fetch
 * degrades to `[]` (logged via `console.error` — the app's convention
 * for a non-critical background fetch failure).
 *
 * Freshness guarantees:
 * - A key change IMMEDIATELY serves `[]` until the new fetch resolves —
 *   the previous space's rows are never served in the gap (the state
 *   tracks the key its rows belong to), so `expandSkillMentions` cannot
 *   inject the wrong space's skill. (The composer now expands via the
 *   generalized `expandMentions`; the citation stands because
 *   `expandSkillMentions` is the retained `$`-only ORACLE for it, so the
 *   guarantee stated here holds for the path actually in use.)
 * - The LEFT key is evicted from the cache when the effect re-runs for a
 *   DIFFERENT key (tracked via a `useRef`), so a skill created/edited on
 *   disk mid-session is picked up on the next Space switch. An UNCHANGED
 *   key does NOT evict (a remount reuses the settled cache entry) and the
 *   NEW key is never evicted (the two-consumers-same-key in-flight dedupe
 *   keeps working — both mount in the same commit, so no key-change
 *   fires for them).
 *
 * The cache stores the IN-FLIGHT PROMISE, not just the resolved rows
 * (load-bearing: `App` mounts `SpacesList` AND `ChatStream`
 * simultaneously, both call `useSkillCatalog` with the same key in the
 * same commit — if the cache only populated on resolve, both effects
 * would see a cache miss and BOTH call `listSkills` → 2 IPC calls per
 * key).
 */
const cache = new Map<string, Promise<SkillInfo[]>>();

export function useSkillCatalog(spacePath: string | null): SkillInfo[] {
  const key = spacePath ?? "__global__";
  // Rows are tracked TOGETHER with the key they were fetched for: a key
  // change (the active Space changed) immediately serves [] until the new
  // fetch resolves — the previous space's rows must not be served in the
  // gap (sends there go UNEXPANDED, not expanded with the wrong space's
  // skills).
  const [state, setState] = useState<{ key: string; rows: SkillInfo[] }>(() => ({
    key,
    rows: [],
  }));
  // The key the previous effect run fetched for — used to evict the LEFT
  // key when the key changed (a skill created/edited on disk mid-session
  // is picked up on the next Space switch). Unchanged key → NO eviction
  // (a remount reuses the settled cache entry).
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
      p = listSkills(spacePath)
        .catch((err) => {
          // A failed fetch degrades to [] (logged — the app's convention
          // for a non-critical background fetch failure).
          // A FAILED promise is NOT cached (delete it) so a later effect
          // retries — but only if THIS promise is still the cached entry
          // (a test's `clearSkillCatalogCache()` + re-mount may have cached
          // a FRESH one under the same key; an unconditional delete would
          // evict it).
          console.error("listSkills failed:", err);
          if (cache.get(key) === p) cache.delete(key);
          return [] as SkillInfo[];
        });
      cache.set(key, p);
    }
    void p.then((r) => {
      // Stale-response guard: the Space changed while in flight.
      if (!cancelled) setState({ key, rows: r });
    });
    return () => {
      cancelled = true;
    };
  }, [spacePath]);
  return state.key === key ? state.rows : [];
}

/** Test-only: clear the module-level catalog cache (call in `beforeEach`). */
export function clearSkillCatalogCache(): void {
  cache.clear();
}
