import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listSkills, type SkillInfo } from "../lib/tauri";
import { clearSkillCatalogCache, useSkillCatalog } from "./useSkillCatalog";

vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    listSkills: vi.fn().mockResolvedValue([
      {
        name: "alpha",
        description: "Alpha skill",
        path: "/p/.agents/skills/alpha/SKILL.md",
        dir: "/p/.agents/skills/alpha",
        scope: "space",
        body: "B",
      } satisfies SkillInfo,
    ]),
  };
});

const mockedListSkills = vi.mocked(listSkills);

beforeEach(() => {
  vi.clearAllMocks();
  clearSkillCatalogCache();
});

describe("useSkillCatalog", () => {
  it("two_simultaneous_consumers_share_one_fetch", async () => {
    // Two consumers with the same key mounted in the same commit (the
    // real app's `App` mounts `SpacesList` AND `ChatStream` this way):
    // the in-flight PROMISE is cached, so the second effect reuses the
    // first consumer's fetch instead of issuing a second `listSkills`.
    const a = renderHook(() => useSkillCatalog("/tmp/alpha"));
    const b = renderHook(() => useSkillCatalog("/tmp/alpha"));
    await waitFor(() => expect(mockedListSkills).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    await waitFor(() => expect(b.result.current).toHaveLength(1));
    // The one-fetch property, pinned AFTER both consumers resolved: a
    // resolve-only cache would have both effects see a cache miss (both
    // effects run before the first resolve lands) → 2 calls.
    expect(mockedListSkills).toHaveBeenCalledTimes(1);
  });

  it("different_spaces_fetch_separately", async () => {
    const a = renderHook(() => useSkillCatalog("/tmp/alpha"));
    const b = renderHook(() => useSkillCatalog("/tmp/beta"));
    await waitFor(() => expect(mockedListSkills).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    await waitFor(() => expect(b.result.current).toHaveLength(1));
    expect(mockedListSkills).toHaveBeenCalledTimes(2);
  });

  it("a_second_consumer_mounted_after_resolve_does_not_refetch", async () => {
    const a = renderHook(() => useSkillCatalog("/tmp/alpha"));
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    // Consumer B mounts after the fetch settled: the SETTLED cached
    // promise is reused — the effect's `p.then` delivers rows on the
    // next microtask and `listSkills` is not called again.
    const b = renderHook(() => useSkillCatalog("/tmp/alpha"));
    await waitFor(() => expect(b.result.current).toHaveLength(1));
    expect(mockedListSkills).toHaveBeenCalledTimes(1);
  });

  it("a_key_change_serves_empty_until_the_new_fetch_resolves", async () => {
    // Freshness hole (1): when the active Space changes, the PREVIOUS
    // space's rows must not be served during the gap — the plan promises
    // sends in that gap go UNEXPANDED. The hook must return [] until the
    // new fetch resolves. (The mock is a stable `mockImplementation` —
    // NOT a once-queue — because `vi.clearAllMocks()` does not clear the
    // once-queue, so leftovers would poison the next test.)
    const alpha: SkillInfo = {
      name: "alpha",
      description: "Alpha skill",
      path: "/p/.agents/skills/alpha/SKILL.md",
      dir: "/p/.agents/skills/alpha",
      scope: "space",
      body: "B",
    };
    const beta: SkillInfo = {
      name: "beta",
      description: "Beta skill",
      path: "/q/.agents/skills/beta/SKILL.md",
      dir: "/q/.agents/skills/beta",
      scope: "space",
      body: "C",
    };
    let resolveBeta!: (rows: SkillInfo[]) => void;
    const betaPromise = new Promise<SkillInfo[]>((resolve) => {
      resolveBeta = resolve;
    });
    mockedListSkills.mockImplementation((p) =>
      p === "/tmp/alpha" ? Promise.resolve([alpha]) : Promise.resolve(betaPromise),
    );
    let spacePath = "/tmp/alpha";
    const a = renderHook(() => useSkillCatalog(spacePath));
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    expect(a.result.current[0].name).toBe("alpha");
    // Switch the SAME consumer to space B. IMMEDIATELY (before the
    // deferred B fetch resolves) the hook must serve [] — NOT A's rows.
    spacePath = "/tmp/beta";
    a.rerender();
    expect(a.result.current).toEqual([]);
    // Resolve B: B's rows appear.
    await act(async () => {
      resolveBeta([beta]);
    });
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    expect(a.result.current[0].name).toBe("beta");
    a.unmount();
  });

  it("switching_spaces_back_refetches_the_first_space", async () => {
    // Freshness hole (2): a skill created/edited on disk mid-session must
    // be picked up on the next Space switch — the LEFT key is evicted
    // when the effect re-runs for a different key.
    mockedListSkills.mockImplementation(() =>
      Promise.resolve([
        {
          name: "alpha",
          description: "Alpha skill",
          path: "/p/.agents/skills/alpha/SKILL.md",
          dir: "/p/.agents/skills/alpha",
          scope: "space",
          body: "B",
        } satisfies SkillInfo,
      ]),
    );
    let spacePath = "/tmp/alpha";
    const a = renderHook(() => useSkillCatalog(spacePath));
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    expect(mockedListSkills).toHaveBeenCalledTimes(1);
    // Leave A for B: B fetches (call 2) and A is evicted from the cache.
    spacePath = "/tmp/beta";
    a.rerender();
    await waitFor(() => expect(mockedListSkills).toHaveBeenCalledTimes(2));
    // Come back to A: the evicted entry is RE-FETCHED (call 3 with A's
    // path) instead of reusing A's stale cached promise.
    spacePath = "/tmp/alpha";
    a.rerender();
    await waitFor(() => expect(mockedListSkills).toHaveBeenCalledTimes(3));
    expect(mockedListSkills).toHaveBeenLastCalledWith("/tmp/alpha");
    await waitFor(() => expect(a.result.current).toHaveLength(1));
    expect(a.result.current[0].name).toBe("alpha");
    a.unmount();
  });

  it("a_failed_fetch_degrades_to_empty_and_a_remount_retries", async () => {
    // A failed fetch degrades to [] (logged via console.error) and the
    // failed promise is evicted from the cache, so a FRESH consumer with
    // the same key retries (call 2) instead of reusing the failed entry.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    let calls = 0;
    mockedListSkills.mockImplementation(() => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve([
            {
              name: "alpha",
              description: "Alpha skill",
              path: "/p/.agents/skills/alpha/SKILL.md",
              dir: "/p/.agents/skills/alpha",
              scope: "space",
              body: "B",
            } satisfies SkillInfo,
          ]);
    });
    const a = renderHook(() => useSkillCatalog("/tmp/alpha"));
    expect(a.result.current).toEqual([]);
    await waitFor(() => expect(errSpy).toHaveBeenCalledTimes(1));
    const b = renderHook(() => useSkillCatalog("/tmp/alpha"));
    await waitFor(() => expect(mockedListSkills).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(b.result.current).toHaveLength(1));
    expect(b.result.current[0].name).toBe("alpha");
    a.unmount();
    b.unmount();
    errSpy.mockRestore();
  });

  it("a_failed_fetch_does_not_evict_a_fresh_entry_planted_by_clearSkillCatalogCache", async () => {
    // Identity guard: the old failure's eviction must only delete the
    // cache entry if THIS promise is still the cached one — a fresh
    // consumer's new promise (planted after clearSkillCatalogCache) must
    // survive the old failure settling.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    let calls = 0;
    mockedListSkills.mockImplementation(() => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve([
            {
              name: "alpha",
              description: "Alpha skill",
              path: "/p/.agents/skills/alpha/SKILL.md",
              dir: "/p/.agents/skills/alpha",
              scope: "space",
              body: "B",
            } satisfies SkillInfo,
          ]);
    });
    const a = renderHook(() => useSkillCatalog("/tmp/alpha")); // call 1 (fails)
    clearSkillCatalogCache(); // wipe the in-flight (failing) entry
    const b = renderHook(() => useSkillCatalog("/tmp/alpha")); // call 2 (fresh, succeeds)
    await waitFor(() => expect(mockedListSkills).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(errSpy).toHaveBeenCalledTimes(1)); // the OLD failure settled
    // The fresh entry was NOT evicted: the fresh consumer's rows arrive.
    await waitFor(() => expect(b.result.current).toHaveLength(1));
    expect(b.result.current[0].name).toBe("alpha");
    expect(a.result.current).toEqual([]);
    expect(mockedListSkills).toHaveBeenCalledTimes(2);
    a.unmount();
    b.unmount();
    errSpy.mockRestore();
  });
});
