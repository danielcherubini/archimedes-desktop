import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  listAgentDefinitionsForSpace,
  listMcpServersEffective,
  listSpaceFiles,
  listCompletionEntries,
  type AgentDefinitionDto,
  type CompletionDirDto,
  type CompletionEntryDto,
  type FileListDto,
  type McpServerInfo,
} from "../lib/tauri";
import {
  clearMentionCatalogsCache,
  useCompletionDir,
  useMentionCatalogs,
} from "./useMentionCatalogs";
import { clearSkillCatalogCache } from "./useSkillCatalog";

vi.mock("../lib/tauri", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("../lib/tauri");
  return {
    ...actual,
    listSkills: vi.fn().mockResolvedValue([
      {
        name: "alpha-skill",
        description: "Alpha skill",
        path: "/p/.agents/skills/alpha-skill/SKILL.md",
        dir: "/p/.agents/skills/alpha-skill",
        scope: "space",
        body: "B",
      },
    ]),
    listAgentDefinitionsForSpace: vi.fn().mockResolvedValue([
      {
        name: "alpha-agent",
        description: "Alpha agent",
        model: null,
        scope: "space",
      } satisfies AgentDefinitionDto,
    ]),
    listMcpServersEffective: vi.fn().mockResolvedValue([
      {
        name: "alpha-mcp",
        kind: "http",
        summary: "https://example.com/mcp",
      } satisfies McpServerInfo,
    ]),
    // Mocked because the files row fetches on EVERY mount: the real
    // wrapper would `invoke` under jsdom, reject, and add a third
    // `console.error` to the failed-fetch test's exact-count assertion.
    listSpaceFiles: vi.fn().mockResolvedValue({ entries: [], truncated: false }),
    // Mocked for the same reason as `listSpaceFiles`: the Directory
    // completion row fetches on EVERY mount, so the real wrapper would
    // `invoke` under jsdom, reject, and add an extra `console.error` to the
    // failed-fetch test's exact-count assertion.
    listCompletionEntries: vi.fn().mockResolvedValue({
      entries: [],
      truncated: false,
    }),
  };
});

const mockListAgentDefinitionsForSpace = vi.mocked(listAgentDefinitionsForSpace);
const mockListMcpServersEffective = vi.mocked(listMcpServersEffective);
const mockListSpaceFiles = vi.mocked(listSpaceFiles);
const mockListCompletionEntries = vi.mocked(listCompletionEntries);

/** The empty payload the files row degrades to. */
const EMPTY_FILES: FileListDto = { entries: [], truncated: false };
/** The empty payload the Directory completion row degrades to. */
const EMPTY_DIR: CompletionDirDto = { entries: [], truncated: false };

/** One Directory completion row (ADR 0035). */
function row(name: string): CompletionEntryDto {
  return {
    name,
    insert: `/tmp/${name}`,
    display: `/tmp/${name}`,
    isDir: false,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  // `clearAllMocks` does NOT drop implementations, so a per-test
  // `mockImplementation` below would leak into later tests. Re-arm the
  // file-listing default every test.
  mockListSpaceFiles.mockResolvedValue({ entries: [], truncated: false });
  mockListCompletionEntries.mockResolvedValue({ entries: [], truncated: false });
  clearSkillCatalogCache();
  clearMentionCatalogsCache();
});

describe("useMentionCatalogs", () => {
  it("two_simultaneous_consumers_share_one_fetch_per_catalog", async () => {
    // Two consumers with the same key mounted in the same commit (the
    // real app's `App` mounts two consumers this way): the in-flight
    // PROMISE is cached per catalog, so each catalog's fetch happens
    // ONCE (the skills row reuses `useSkillCatalog`'s own cache).
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    const b = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() =>
      expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(1),
    );
    await waitFor(() =>
      expect(mockListMcpServersEffective).toHaveBeenCalledTimes(1),
    );
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(a.result.current.mcpServers).toHaveLength(1));
    await waitFor(() => expect(b.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(b.result.current.mcpServers).toHaveLength(1));
    await waitFor(() => expect(a.result.current.skills).toHaveLength(1));
    await waitFor(() => expect(b.result.current.skills).toHaveLength(1));
    // The one-fetch property, pinned AFTER both consumers resolved: a
    // resolve-only cache would have both effects see a cache miss → 2
    // calls per wrapper.
    expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(1);
    expect(mockListMcpServersEffective).toHaveBeenCalledTimes(1);
    expect(mockListAgentDefinitionsForSpace).toHaveBeenLastCalledWith("/tmp/alpha");
    expect(mockListMcpServersEffective).toHaveBeenLastCalledWith("/tmp/alpha");
  });

  it("different_spaces_fetch_separately", async () => {
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    const b = renderHook(() => useMentionCatalogs("/tmp/beta"));
    await waitFor(() =>
      expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(2),
    );
    await waitFor(() =>
      expect(mockListMcpServersEffective).toHaveBeenCalledTimes(2),
    );
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(b.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(a.result.current.mcpServers).toHaveLength(1));
    await waitFor(() => expect(b.result.current.mcpServers).toHaveLength(1));
    expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(2);
    expect(mockListMcpServersEffective).toHaveBeenCalledTimes(2);
  });

  it("a_second_consumer_mounted_after_resolve_does_not_refetch", async () => {
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(a.result.current.mcpServers).toHaveLength(1));
    const b = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(b.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(b.result.current.mcpServers).toHaveLength(1));
    expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(1);
    expect(mockListMcpServersEffective).toHaveBeenCalledTimes(1);
  });

  it("a_key_change_serves_empty_until_the_new_fetch_resolves", async () => {
    // Freshness hole (1): when the active Space changes, the PREVIOUS
    // space's rows must not be served during the gap. Both the agents
    // and mcpServer rows must return [] until the new fetch resolves.
    let resolveBetaAgents!: (rows: AgentDefinitionDto[]) => void;
    let resolveBetaMcp!: (rows: McpServerInfo[]) => void;
    const betaAgentsPromise = new Promise<AgentDefinitionDto[]>(
      (resolve) => {
        resolveBetaAgents = resolve;
      },
    );
    const betaMcpPromise = new Promise<McpServerInfo[]>((resolve) => {
      resolveBetaMcp = resolve;
    });
    const alphaAgent: AgentDefinitionDto = {
      name: "alpha-agent",
      description: "Alpha agent",
      model: null,
      scope: "space",
    };
    const betaAgent: AgentDefinitionDto = {
      name: "beta-agent",
      description: "Beta agent",
      model: "gpt-4o",
      scope: "space",
    };
    const alphaMcp: McpServerInfo = {
      name: "alpha-mcp",
      kind: "http",
      summary: "https://alpha.example.com/mcp",
    };
    const betaMcp: McpServerInfo = {
      name: "beta-mcp",
      kind: "stdio",
      summary: "npx mcp-server",
    };
    mockListAgentDefinitionsForSpace.mockImplementation((p) =>
      p === "/tmp/alpha" ? Promise.resolve([alphaAgent]) : Promise.resolve(betaAgentsPromise),
    );
    mockListMcpServersEffective.mockImplementation((p) =>
      p === "/tmp/alpha" ? Promise.resolve([alphaMcp]) : Promise.resolve(betaMcpPromise),
    );
    let spacePath = "/tmp/alpha";
    const a = renderHook(() => useMentionCatalogs(spacePath));
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    expect(a.result.current.agents[0].name).toBe("alpha-agent");
    await waitFor(() => expect(a.result.current.mcpServers).toHaveLength(1));
    expect(a.result.current.mcpServers[0].name).toBe("alpha-mcp");
    // Switch the SAME consumer to space B. IMMEDIATELY (before the
    // deferred B fetch resolves) the hook must serve [] for BOTH rows —
    // NOT A's rows.
    spacePath = "/tmp/beta";
    a.rerender();
    expect(a.result.current.agents).toEqual([]);
    expect(a.result.current.mcpServers).toEqual([]);
    // Resolve B: B's rows appear.
    await act(async () => {
      resolveBetaAgents([betaAgent]);
      resolveBetaMcp([betaMcp]);
    });
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(a.result.current.mcpServers).toHaveLength(1));
    expect(a.result.current.agents[0].name).toBe("beta-agent");
    expect(a.result.current.mcpServers[0].name).toBe("beta-mcp");
    a.unmount();
  });

  it("switching_spaces_back_refetches_the_first_space", async () => {
    // Freshness hole (2): an agent/mcp config created/edited on disk
    // mid-session must be picked up on the next Space switch — the LEFT
    // key is evicted when the effect re-runs for a different key (both
    // catalogs).
    let spacePath = "/tmp/alpha";
    const a = renderHook(() => useMentionCatalogs(spacePath));
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(1);
    expect(mockListMcpServersEffective).toHaveBeenCalledTimes(1);
    // Leave A for B: B fetches (call 2 per catalog) and A is evicted
    // from BOTH caches.
    spacePath = "/tmp/beta";
    a.rerender();
    await waitFor(() =>
      expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(2),
    );
    await waitFor(() =>
      expect(mockListMcpServersEffective).toHaveBeenCalledTimes(2),
    );
    // Come back to A: the evicted entries are RE-FETCHED (call 3 with A's
    // path) instead of reusing A's stale cached promises.
    spacePath = "/tmp/alpha";
    a.rerender();
    await waitFor(() =>
      expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledTimes(3),
    );
    await waitFor(() =>
      expect(mockListMcpServersEffective).toHaveBeenCalledTimes(3),
    );
    expect(mockListAgentDefinitionsForSpace).toHaveBeenLastCalledWith("/tmp/alpha");
    expect(mockListMcpServersEffective).toHaveBeenLastCalledWith("/tmp/alpha");
    a.unmount();
  });

  it("a_failed_fetch_degrades_to_empty_and_a_remount_retries", async () => {
    // A failed fetch degrades to [] (logged via console.error) and the
    // failed promise is evicted from the cache, so a FRESH consumer with
    // the same key retries (call 2) instead of reusing the failed entry.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    let agentCalls = 0;
    let mcpCalls = 0;
    mockListAgentDefinitionsForSpace.mockImplementation(() => {
      agentCalls += 1;
      return agentCalls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve([
            {
              name: "alpha-agent",
              description: "Alpha agent",
              model: null,
              scope: "space",
            } satisfies AgentDefinitionDto,
          ]);
    });
    mockListMcpServersEffective.mockImplementation(() => {
      mcpCalls += 1;
      return mcpCalls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve([
            {
              name: "alpha-mcp",
              kind: "http",
              summary: "https://example.com/mcp",
            } satisfies McpServerInfo,
          ]);
    });
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    expect(a.result.current.agents).toEqual([]);
    expect(a.result.current.mcpServers).toEqual([]);
    await waitFor(() => expect(errSpy).toHaveBeenCalledTimes(2));
    const b = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(agentCalls).toBe(2));
    await waitFor(() => expect(mcpCalls).toBe(2));
    await waitFor(() => expect(b.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(b.result.current.mcpServers).toHaveLength(1));
    expect(b.result.current.agents[0].name).toBe("alpha-agent");
    expect(b.result.current.mcpServers[0].name).toBe("alpha-mcp");
    a.unmount();
    b.unmount();
    errSpy.mockRestore();
  });

  it("the_files_row_shares_one_fetch_between_two_consumers", async () => {
    // The 4th catalog (the `?` file listing) reuses the SAME in-flight
    // promise cache: two consumers of one key mean ONE `list_space_files`
    // IPC call.
    mockListSpaceFiles.mockResolvedValue({
      entries: ["README.md"],
      truncated: false,
    });
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    const b = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(mockListSpaceFiles).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(a.result.current.files.entries).toEqual(["README.md"]));
    await waitFor(() => expect(b.result.current.files.entries).toEqual(["README.md"]));
    // Pinned AFTER both resolved: a resolve-only cache would have both
    // effects see a miss → 2 calls.
    expect(mockListSpaceFiles).toHaveBeenCalledTimes(1);
    expect(mockListSpaceFiles).toHaveBeenLastCalledWith("/tmp/alpha");
    a.unmount();
    b.unmount();
  });

  it("a_key_change_serves_empty_files_until_the_new_listing_resolves_and_evicts_the_left_key", async () => {
    // Freshness hole for the files row: a stale Space's paths must never
    // be offered for insertion. The gap serves the EMPTY payload, and the
    // LEFT key is evicted (so coming back re-walks the directory).
    let resolveBeta!: (payload: FileListDto) => void;
    const betaPromise = new Promise<FileListDto>((resolve) => {
      resolveBeta = resolve;
    });
    mockListSpaceFiles.mockImplementation((p) =>
      p === "/tmp/alpha"
        ? Promise.resolve({ entries: ["alpha.md"], truncated: false })
        : betaPromise,
    );
    let spacePath = "/tmp/alpha";
    const a = renderHook(() => useMentionCatalogs(spacePath));
    await waitFor(() => expect(a.result.current.files.entries).toEqual(["alpha.md"]));
    expect(mockListSpaceFiles).toHaveBeenCalledTimes(1);
    // Leave A for B: the gap serves the empty payload, NOT A's entries.
    spacePath = "/tmp/beta";
    a.rerender();
    expect(a.result.current.files).toEqual(EMPTY_FILES);
    // B resolves: B's entries appear.
    await act(async () => {
      resolveBeta({ entries: ["beta.md"], truncated: true });
    });
    await waitFor(() => expect(a.result.current.files.entries).toEqual(["beta.md"]));
    expect(mockListSpaceFiles).toHaveBeenCalledTimes(2);
    // Come back to A: the LEFT key was evicted, so A is RE-walked (call 3)
    // rather than served from a stale cached promise.
    spacePath = "/tmp/alpha";
    a.rerender();
    await waitFor(() => expect(mockListSpaceFiles).toHaveBeenCalledTimes(3));
    expect(mockListSpaceFiles).toHaveBeenLastCalledWith("/tmp/alpha");
    a.unmount();
  });

  it("a_failed_file_listing_degrades_to_empty_and_a_remount_retries", async () => {
    // A failed walk (a Space on a disconnected drive) degrades to the
    // empty payload — no picker, Enter still sends — and the failed
    // promise is NOT left cached, so a fresh consumer retries.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    let fileCalls = 0;
    mockListSpaceFiles.mockImplementation(() => {
      fileCalls += 1;
      return fileCalls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve({ entries: ["alpha.md"], truncated: false });
    });
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    expect(a.result.current.files).toEqual(EMPTY_FILES);
    await waitFor(() => expect(errSpy).toHaveBeenCalledTimes(1));
    expect(errSpy.mock.calls[0][0]).toContain("listSpaceFiles");
    const b = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(fileCalls).toBe(2));
    await waitFor(() => expect(b.result.current.files.entries).toEqual(["alpha.md"]));
    a.unmount();
    b.unmount();
    errSpy.mockRestore();
  });

  it("the_empty_files_value_is_one_module_constant_across_consumers",
    async () => {
      // The symmetric pin to `the_null_dir_prefix_serves_the_stable_empty_dir`
      // below, and it exists because of a MEASUREMENT: replacing the module-level
      // `EMPTY_FILE_LIST` with a fresh object literal at the call site left every
      // test in this file (and then the whole frontend suite) GREEN. Stable
      // identity is therefore a contract of this catalog too, and it is now pinned
      // rather than asserted in a comment: two consumers waiting on DIFFERENT keys
      // must be handed the SAME empty object.
      //
      // Why anyone should care: `EMPTY_FILE_LIST` is the `empty` argument of
      // `useCatalogPayload`, i.e. the caller-side half of the render-loop hazard
      // that hook's deps comment describes, and a consumer that memoises on
      // `files` would re-run every render on a fresh literal. Both are invisible
      // to deep-equality assertions, which is exactly what a `toBe` is for.
      const { promise } = deferred<FileListDto>();
      mockListSpaceFiles.mockReturnValue(promise);
      const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
      const b = renderHook(() => useMentionCatalogs("/tmp/beta"));
      expect(a.result.current.files).toEqual(EMPTY_FILES);
      expect(b.result.current.files).toBe(a.result.current.files);
      a.unmount();
      b.unmount();
    });

  it("the_null_key_calls_the_file_listing_wrapper_with_null", async () => {
    const a = renderHook(() => useMentionCatalogs(null));
    await waitFor(() => expect(mockListSpaceFiles).toHaveBeenCalledWith(null));
    await waitFor(() => expect(a.result.current.files).toEqual(EMPTY_FILES));
    a.unmount();
  });

  it("a_warm_file_listing_keeps_its_truncated_flag", async () => {
    // The cache stores the whole PAYLOAD, not just the row array: a warm
    // read of a capped listing must still say `truncated` (the picker says
    // so instead of lying about completeness).
    mockListSpaceFiles.mockResolvedValue({
      entries: ["a.md", "b.md"],
      truncated: true,
    });
    const a = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(a.result.current.files.truncated).toBe(true));
    expect(a.result.current.files.entries).toEqual(["a.md", "b.md"]);
    const b = renderHook(() => useMentionCatalogs("/tmp/alpha"));
    await waitFor(() => expect(b.result.current.files.truncated).toBe(true));
    expect(b.result.current.files.entries).toEqual(["a.md", "b.md"]);
    expect(mockListSpaceFiles).toHaveBeenCalledTimes(1);
    a.unmount();
    b.unmount();
  });

  it("the_null_key_calls_both_wrappers_with_null", async () => {
    // The `null` key (user-level / app-scope only): both wrappers are
    // called with `null` (the `cwd: None` command case), not omitted or
    // defaulted.
    const a = renderHook(() => useMentionCatalogs(null));
    await waitFor(() =>
      expect(mockListAgentDefinitionsForSpace).toHaveBeenCalledWith(null),
    );
    await waitFor(() =>
      expect(mockListMcpServersEffective).toHaveBeenCalledWith(null),
    );
    await waitFor(() => expect(a.result.current.agents).toHaveLength(1));
    await waitFor(() => expect(a.result.current.mcpServers).toHaveLength(1));
    a.unmount();
  });
});

/** A pending promise plus its resolver (a fetch that has NOT settled). */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("useCompletionDir", () => {
  it("the_null_dir_prefix_serves_the_stable_empty_dir", async () => {
    // `null` = no out-of-Space token open. The fetch is left PENDING so the
    // hook stays in its empty state and the assertion is about the EMPTY
    // VALUE and its IDENTITY — never "`listCompletionEntries` was not
    // called": the hook calls the wrapper on every cache miss, exactly as
    // `listSpaceFiles(null)` is called for a Space-less window.
    const { promise } = deferred<CompletionDirDto>();
    mockListCompletionEntries.mockReturnValue(promise);
    const a = renderHook(() => useCompletionDir(null));
    // The payload now rides in `.dir` beside the `pending` flag (ADR 0035's
    // in-flight-descent gap), so the IDENTITY assertion reads through `.dir`.
    // The guarantee itself is unchanged and still pinned here: the empty value
    // is the module-level `EMPTY_COMPLETION_DIR`, and a fresh literal at the
    // call site reddens THIS assertion and nothing else.
    expect(a.result.current.dir).toEqual(EMPTY_DIR);
    // Stable identity: the empty value is a MODULE constant, so a second
    // consumer on a DIFFERENT pending key gets the SAME object. A fresh
    // literal per render would be a different object every render, and a
    // consumer that memoised on it would loop.
    const b = renderHook(() => useCompletionDir("/tmp/alpha"));
    expect(b.result.current.dir).toBe(a.result.current.dir);
    // The `null` key is PASSED THROUGH to the wrapper (no client-side
    // short-circuit): the "no filesystem access for `None`" guarantee is
    // Rust-side, pinned in `src-tauri/src/commands/files.rs`.
    await waitFor(() =>
      expect(mockListCompletionEntries).toHaveBeenCalledWith(null),
    );
    expect(a.result.current.dir).toBe(b.result.current.dir);
    a.unmount();
    b.unmount();
  });

  it("the_same_dir_prefix_is_listed_once", async () => {
    // The cache dedupes: a re-render with the SAME `dirPrefix`, and a second
    // consumer of the SAME directory, each cost ZERO extra IPC calls. This is
    // what makes one Directory completion per directory rather than one per
    // keystroke — the engine returns the whole unfiltered directory (Task 1)
    // and the renderer filters it synchronously.
    //
    // What this test does NOT prove: that TYPING inside one directory does
    // not refetch. Here `dirPrefix` is a hook ARGUMENT, so that assertion
    // would only say "the same input gives the same output"; the property
    // lives in `ChatStream`'s DERIVATION of `dirPrefix` from the token, and
    // is pinned there.
    mockListCompletionEntries.mockResolvedValue({
      entries: [row("a.md")],
      truncated: false,
    });
    const a = renderHook(() => useCompletionDir("/tmp/alpha"));
    await waitFor(() => expect(a.result.current.dir.entries).toHaveLength(1));
    a.rerender();
    const b = renderHook(() => useCompletionDir("/tmp/alpha"));
    await waitFor(() => expect(b.result.current.dir.entries).toHaveLength(1));
    expect(mockListCompletionEntries).toHaveBeenCalledTimes(1);
    expect(mockListCompletionEntries).toHaveBeenLastCalledWith("/tmp/alpha");
    a.unmount();
    b.unmount();
  });

  it("a_dir_prefix_change_serves_empty_until_the_new_dir_resolves_and_evicts_the_left_dir", async () => {
    // Freshness hole: the rows of the directory you LEFT must never be
    // offered for insertion into the token you are now typing. The gap
    // serves the EMPTY payload, and the LEFT key is evicted — a directory
    // created/renamed on disk mid-session is re-listed when you come back.
    const beta = deferred<CompletionDirDto>();
    mockListCompletionEntries.mockImplementation((p) =>
      p === "/tmp/alpha"
        ? Promise.resolve({ entries: [row("alpha.md")], truncated: false })
        : beta.promise,
    );
    let dirPrefix = "/tmp/alpha";
    const a = renderHook(() => useCompletionDir(dirPrefix));
    await waitFor(() =>
      expect(a.result.current.dir.entries).toEqual([row("alpha.md")]),
    );
    expect(mockListCompletionEntries).toHaveBeenCalledTimes(1);
    // Leave A for B: the gap serves the EMPTY payload, NOT A's rows.
    dirPrefix = "/tmp/beta";
    a.rerender();
    expect(a.result.current.dir).toEqual(EMPTY_DIR);
    // B resolves: B's rows appear (with the cap flag intact).
    await act(async () => {
      beta.resolve({ entries: [row("beta.md")], truncated: true });
    });
    await waitFor(() =>
      expect(a.result.current.dir.entries).toEqual([row("beta.md")]),
    );
    expect(a.result.current.dir.truncated).toBe(true);
    // Come back to A: the LEFT key was evicted, so A is RE-LISTED (call 3:
    // A, then B, then A again) instead of served from a stale cached promise.
    dirPrefix = "/tmp/alpha";
    a.rerender();
    await waitFor(() => expect(mockListCompletionEntries).toHaveBeenCalledTimes(3));
    expect(mockListCompletionEntries).toHaveBeenLastCalledWith("/tmp/alpha");
    a.unmount();
  });

  it("a_failed_completion_listing_degrades_to_the_stable_empty_dir_and_a_remount_retries", async () => {
    // A directory that cannot be read (deleted mid-session, a permission
    // refusal, a drive gone) degrades to the empty payload — no picker,
    // Enter still sends — and the failed promise is NOT left cached, so a
    // fresh consumer retries.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    let calls = 0;
    mockListCompletionEntries.mockImplementation(() => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new Error("boom"))
        : Promise.resolve({ entries: [row("a.md")], truncated: false });
    });
    const a = renderHook(() => useCompletionDir("/tmp/alpha"));
    // A SECOND consumer of the same key shares the one in-flight (failed)
    // promise, so the failure is logged ONCE.
    const shared = renderHook(() => useCompletionDir("/tmp/alpha"));
    expect(a.result.current.dir).toEqual(EMPTY_DIR);
    await waitFor(() => expect(errSpy).toHaveBeenCalledTimes(1));
    expect(errSpy.mock.calls[0][0]).toContain("listCompletionEntries");
    // The degraded value is the SAME module constant for both consumers
    // (identity, not just deep-equality).
    expect(shared.result.current.dir).toBe(a.result.current.dir);
    // A fresh consumer RETRIES (the failed promise was evicted).
    const b = renderHook(() => useCompletionDir("/tmp/alpha"));
    await waitFor(() => expect(b.result.current.dir.entries).toHaveLength(1));
    expect(calls).toBe(2);
    a.unmount();
    shared.unmount();
    b.unmount();
    errSpy.mockRestore();
  });

  it("a_late_response_for_a_left_dir_is_not_applied_even_after_coming_back", async () => {
    // The stale-response guard, pinned where it can actually be SEEN. The
    // token goes A → B → A while the first A listing is still in flight; A's
    // key was evicted, so coming back re-lists A (a SECOND in-flight IPC).
    // When A's STALE response finally lands we are on key A again, so a
    // response that was applied instead of dropped would be VISIBLE here —
    // which is why the assertion has to be made in this position, not while
    // B is the current key (the render-side key check would hide it).
    const alpha1 = deferred<CompletionDirDto>();
    const alpha2 = deferred<CompletionDirDto>();
    const beta = deferred<CompletionDirDto>();
    let alphaCalls = 0;
    mockListCompletionEntries.mockImplementation((p) => {
      if (p !== "/tmp/alpha") return beta.promise;
      alphaCalls += 1;
      return alphaCalls === 1 ? alpha1.promise : alpha2.promise;
    });
    let dirPrefix = "/tmp/alpha";
    const a = renderHook(() => useCompletionDir(dirPrefix));
    dirPrefix = "/tmp/beta";
    a.rerender();
    await act(async () => {
      beta.resolve({ entries: [row("beta.md")], truncated: false });
    });
    await waitFor(() =>
      expect(a.result.current.dir.entries).toEqual([row("beta.md")]),
    );
    // Back to A: re-listed (the left key was evicted), and the gap is empty.
    dirPrefix = "/tmp/alpha";
    a.rerender();
    await waitFor(() => expect(alphaCalls).toBe(2));
    expect(a.result.current.dir).toEqual(EMPTY_DIR);
    // A's STALE first response lands while key A is current again: dropped.
    await act(async () => {
      alpha1.resolve({ entries: [row("ghost.md")], truncated: false });
    });
    expect(a.result.current.dir).toEqual(EMPTY_DIR);
    expect(a.result.current.dir.entries).not.toContainEqual(row("ghost.md"));
    // The CURRENT listing lands: its rows appear.
    await act(async () => {
      alpha2.resolve({ entries: [row("alpha-fresh.md")], truncated: false });
    });
    await waitFor(() =>
      expect(a.result.current.dir.entries).toEqual([row("alpha-fresh.md")]),
    );
    a.unmount();
  });
});

// --- The `pending` signal (ADR 0035, the in-flight-descent Enter gap). ------
//
// Selecting a DIRECTORY row moves `dirPrefix` to a directory whose rows are not
// here yet, so the picker has ZERO rows for one IPC round trip — and the row
// count is what gates the Enter intercept in `ComposerRow`. `pending` is the
// piece the composer needs to hold Enter still during exactly that window, and
// NOTHING else: ADR 0033's rule that a picker is never a gate survives
// untouched everywhere the flag is false. Each test below pins ONE clause of
// the flag's contract, because the clause a future change drops is exactly the
// one that turns Enter back into a send of a half-typed path (or into a key
// that never works again).

describe("useCompletionDir — the pending signal", () => {
  it("pending is TRUE while the fetch for the current dirPrefix is in flight and FALSE once it settles",
    async () => {
      const { promise, resolve } = deferred<CompletionDirDto>();
      mockListCompletionEntries.mockReturnValue(promise);
      const a = renderHook(() => useCompletionDir("/tmp/alpha"));
      // In flight, and no rows to insert: this is the window in which Enter must
      // not be allowed to deliver `?/tmp/alpha` to the model.
      expect(a.result.current.pending).toBe(true);
      expect(a.result.current.dir.entries).toEqual([]);
      await act(async () => {
        resolve({ entries: [row("a.md")], truncated: false });
      });
      await waitFor(() =>
        expect(a.result.current.dir.entries).toEqual([row("a.md")]),
      );
      // Settled: the flag is what lets the NEXT Enter insert the row.
      expect(a.result.current.pending).toBe(false);
      a.unmount();
    });

  it("pending is FALSE for a null dirPrefix even while the null-key fetch is in flight",
    async () => {
      // `null` = no out-of-Space token open, so there is no descent in flight to
      // wait for — and the in-flight FETCH this hook still issues for the
      // `__global__` key must NOT make the flag true, or every ordinary Enter in
      // a fresh composer would be swallowed by a fetch the user never asked for.
      const { promise } = deferred<CompletionDirDto>();
      mockListCompletionEntries.mockReturnValue(promise);
      const a = renderHook(() => useCompletionDir(null));
      expect(a.result.current.pending).toBe(false);
      // Still false while that fetch sits unsettled (it never settles here on
      // purpose: a flag that keys off "any fetch in flight" is red on this line).
      await act(async () => {
        await Promise.resolve();
      });
      expect(a.result.current.pending).toBe(false);
      await waitFor(() =>
        expect(mockListCompletionEntries).toHaveBeenCalledWith(null),
      );
      expect(a.result.current.pending).toBe(false);
      a.unmount();
    });

  it("pending is FALSE for rows served from the cache, which costs no second fetch",
    async () => {
      // The rows are already on disk in the cache, so nothing is in flight and
      // the flag must not claim otherwise.
      mockListCompletionEntries.mockResolvedValue({
        entries: [row("a.md")],
        truncated: false,
      });
      const a = renderHook(() => useCompletionDir("/tmp/alpha"));
      await waitFor(() =>
        expect(a.result.current.dir.entries).toEqual([row("a.md")]),
      );
      expect(a.result.current.pending).toBe(false);
      a.unmount();
      // A FRESH consumer of the SAME key: no IPC, and the flag is false as soon
      // as the cached payload is applied (the settle is a microtask, not a round
      // trip, and a keydown cannot land inside it — effects flush first).
      const b = renderHook(() => useCompletionDir("/tmp/alpha"));
      await waitFor(() =>
        expect(b.result.current.dir.entries).toEqual([row("a.md")]),
      );
      expect(b.result.current.pending).toBe(false);
      expect(mockListCompletionEntries).toHaveBeenCalledTimes(1);
      b.unmount();
    });

  it("pending is FALSE after a REJECTED fetch — a dead directory must not swallow Enter forever",
    async () => {
      // THE clause most likely to be got wrong. The hook degrades a failed fetch
      // to the empty payload AND evicts the failed promise, so the picker is
      // permanently blank for an unreadable directory. A flag derived from "the
      // rows have not arrived" would be TRUE forever there, and Enter would be
      // dead in a directory the user clicked into and cannot leave except by
      // deleting the draft. Rejection is a SETTLEMENT.
      const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
      mockListCompletionEntries.mockRejectedValue(new Error("unreadable dir"));
      const a = renderHook(() => useCompletionDir("/tmp/dead"));
      expect(a.result.current.pending).toBe(true);
      await waitFor(() => expect(errSpy).toHaveBeenCalledTimes(1));
      await act(async () => {
        await Promise.resolve();
      });
      expect(a.result.current.pending).toBe(false);
      expect(a.result.current.dir).toEqual(EMPTY_DIR);
      // And it STAYS false across renders of the same dead key (an entry that
      // was evicted on failure must not read as "fetch pending").
      a.rerender();
      expect(a.result.current.pending).toBe(false);
      a.unmount();
      errSpy.mockRestore();
    });

  it("pending is TRUE again when a DIFFERENT dirPrefix starts a fresh fetch",
    async () => {
      // The flag is per CURRENT key, not "has anything ever settled": coming
      // back to a directory re-lists it (the left key is evicted), and that new
      // round trip is a new gap in which Enter must not send.
      const alpha = deferred<CompletionDirDto>();
      const beta = deferred<CompletionDirDto>();
      mockListCompletionEntries.mockImplementation((p) =>
        p === "/tmp/alpha" ? alpha.promise : beta.promise,
      );
      let dirPrefix: string | null = "/tmp/alpha";
      const a = renderHook(() => useCompletionDir(dirPrefix));
      await act(async () => {
        alpha.resolve({ entries: [row("a.md")], truncated: false });
      });
      await waitFor(() => expect(a.result.current.pending).toBe(false));
      dirPrefix = "/tmp/beta";
      a.rerender();
      expect(a.result.current.pending).toBe(true);
      await act(async () => {
        beta.resolve({ entries: [row("b.md")], truncated: false });
      });
      await waitFor(() => expect(a.result.current.pending).toBe(false));
      a.unmount();
    });
});
