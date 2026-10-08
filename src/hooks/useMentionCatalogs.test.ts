import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  listAgentDefinitionsForSpace,
  listMcpServersEffective,
  type AgentDefinitionDto,
  type McpServerInfo,
} from "../lib/tauri";
import {
  clearMentionCatalogsCache,
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
  };
});

const mockListAgentDefinitionsForSpace = vi.mocked(listAgentDefinitionsForSpace);
const mockListMcpServersEffective = vi.mocked(listMcpServersEffective);

beforeEach(() => {
  vi.clearAllMocks();
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
