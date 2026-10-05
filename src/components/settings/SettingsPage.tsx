import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactElement } from "react";
import {
  ArrowLeftIcon,
  BotIcon,
  PackageIcon,
  PaletteIcon,
  ServerIcon,
  Settings2Icon,
} from "lucide-react";

import { applySettingsToDocument } from "@/lib/settings";
import {
  getSettings,
  listAgentDefinitions,
  listKnownProviders,
  listModels,
  listTools,
  saveSettings,
  type AgentDefinitionDto,
  type AppSettings,
  type McpServerEntry,
  type ModelDto,
  type ProviderConfig,
  type WireApi,
  type KnownProvider,
} from "@/lib/tauri";
import { Button } from "@/components/ui/button";
import { SettingsSidebarButton } from "./primitives";
import { useSettings } from "@/store/settings";
import GeneralSection from "./GeneralSection";
import AppearanceSection from "./AppearanceSection";
import ProviderSection from "./ProviderSection";
import SubagentSection from "./SubagentSection";
import McpSection, { type McpDraft } from "./McpSection";

type Section = "general" | "appearance" | "providers" | "subagents" | "mcp";

/** A ZCode-style slug: lowercase alphanumeric + `-`. */
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** A slug de-duped against the taken ids (a `-2` / `-3` suffix). */
function uniqueSlug(slug: string, taken: Set<string>): string {
  let id = slug;
  let n = 2;
  while (taken.has(id)) {
    id = `${slug}-${n}`;
    n += 1;
  }
  return id;
}

/**
 * Remap a settings-level model reference (the `defaultModel` composed key —
 * `"<providerId>/<modelId>"`) from a provider's old id to its new one (a
 * name commit re-identifies the provider — the reference must not orphan).
 * A `null` / non-matching key is untouched.
 */
function remapModelRef(
  key: string | null,
  oldId: string,
  newId: string,
): string | null {
  if (key === null || !key.startsWith(`${oldId}/`)) return key;
  return `${newId}${key.slice(oldId.length)}`;
}

/** Remap the `defaultThinkingLevels` keys (the same `"<providerId>/<modelId>"` composed keys). */
function remapModelRefs(
  refs: Record<string, string>,
  oldId: string,
  newId: string,
): Record<string, string> {
  const next: Record<string, string> = {};
  for (const [key, value] of Object.entries(refs)) {
    next[remapModelRef(key, oldId, newId) as string] = value;
  }
  return next;
}

/**
 * The settings page (the approved ZCode-parity design, spec §1/§4): a full
 * content-area view — a 268px section sidebar (back button + General /
 * Appearance / Providers / Subagents / MCP) + the active section's content. Immediate save:
 * a control change → `saveSettings` with the COMPLETE document (text
 * fields commit on blur/Enter; no save button, no dirty state).
 */
export default function SettingsPage({ onBack }: { onBack: () => void }): ReactElement {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [section, setSection] = useState<Section>("general");
  const [models, setModels] = useState<ModelDto[]>([]);
  // The known-providers catalog (ADR 0024 — the Providers section's picker
  // data source; `[]` = not loaded / the load failed).
  const [knownProviders, setKnownProviders] = useState<KnownProvider[]>([]);
  // The picker's selected template id ("" = nothing selected — the
  // placeholder; the Add button is disabled until one is picked).
  const [selectedKnown, setSelectedKnown] = useState("");
  // The user-level discovered agent definitions (ADR 0023 — the Subagents
  // section's data source; `null` until the first list lands).
  const [agentDefs, setAgentDefs] = useState<AgentDefinitionDto[] | null>(null);
  // The native harness's tool names (the enabled-tools checkbox list).
  const [tools, setTools] = useState<string[]>([]);
  // The MCP add/edit dialog draft (`null` = closed — a fresh draft per open
  // so a Cancel never leaks the draft into the next open).
  const [mcpDialog, setMcpDialog] = useState<McpDraft | null>(null);
  // The theme cleanup is HELD IN A REF and replaced on each apply —
  // discarding it would leak a `matchMedia` listener per control change
  // (`applySettingsToDocument` subscribes one for "system" themes).
  const themeCleanupRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    // The page's single source of truth: the settings document.
    getSettings()
      .then((settings) => {
        setSettings(settings);
        // Keep the app-wide store in sync (the working-indicator spinner
        // style reads it at runtime — the boot path seeds it too).
        useSettings.getState().setSettings(settings);
      })
      .catch(() => {});
    listModels()
      .then(setModels)
      .catch(() => {});
    void listAgentDefinitions()
      .then(setAgentDefs)
      .catch(() => setAgentDefs([]));
    listTools()
      .then(setTools)
      .catch(() => {});
    listKnownProviders()
      .then(setKnownProviders)
      .catch((error) => {
        console.error("loading the known-providers catalog failed", error);
        setKnownProviders([]);
      });
    return () => {
      themeCleanupRef.current?.();
    };
  }, []);

  /**
   * The Default-thinking-level select's options (the union of the
   * `thinkingLevels` arrays across the `listModels` results, deduped in
   * FIRST-SEEN order — each model's advertised order, consistent with the
   * in-session thinking-level picker; a `Set` preserves insertion order,
   * so no `.sort()`): a model's live `thinkingLevels` is the source of
   * truth — a level no model advertises is not offered.
   */
  const thinkingLevelUnion = useMemo(() => {
    const levels = new Set<string>();
    for (const model of models) {
      for (const level of model.thinkingLevels) levels.add(level);
    }
    return [...levels];
  }, [models]);

  /**
   * The stored `defaultThinkingLevel` when it is NON-NULL, NON-BLANK and NOT
   * in the union (a provider removed it / the level sets changed): offered
   * as its own raw option — otherwise Radix's `SelectValue` falls back to the
   * placeholder and the UI DISPLAYS "Model default" while sessions still
   * start with the stored level (a false representation of the saved state).
   * A blank stored value (a hand-edited file — the frontend never SAVES
   * `""`) is treated as absent: `""` is the `value` of the "Model default"
   * option, so offering it again would give Radix two items with the same
   * value.
   */
  const storedLevel = settings?.defaultThinkingLevel ?? null;
  const storedLevelOutsideUnion =
    storedLevel !== null &&
    storedLevel !== "" &&
    !thinkingLevelUnion.includes(storedLevel)
      ? storedLevel
      : null;

  /** The immediate-save pattern (one helper, used by every control). */
  const update = (patch: Partial<AppSettings>) => {
    if (settings === null) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    // The app-wide store (the working indicator reads `spinnerStyle` from
    // it — a settings change must take effect without a reload).
    useSettings.getState().setSettings(next);
    void saveSettings(next); // the complete document
    themeCleanupRef.current?.(); // drop the previous theme listener
    themeCleanupRef.current = applySettingsToDocument(next); // live theme/font
  };

  /**
   * A provider field commit (immediate save): `update` + re-run the changed
   * provider's discovery (Task 2's `force_refresh` bypasses its cache).
   * The id is derived from the name: a committed name (a non-blank slug)
   * that differs from the current id re-identifies the provider — the
   * settings-level model references (the `defaultModel` composed key + the
   * `defaultThinkingLevels` keys, both `"<providerId>/<modelId>"`) are
   * remapped old → new so they don't orphan (the in-memory discovery cache
   * entries for the old id are simply unused — the new id gets a fresh
   * discovery; a BLANK name keeps the current id).
   */
  const commitProviderField = (index: number, patch: Partial<ProviderConfig>) => {
    if (settings === null) return;
    const current = settings.providers[index];
    const next = { ...current, ...patch };
    const slug = slugify(next.name);
    let id = next.id;
    if (slug !== "") {
      const taken = new Set(
        settings.providers
          .map((p, i) => (i === index ? null : p.id))
          .filter((v): v is string => v !== null),
      );
      id = uniqueSlug(slug, taken);
    }
    const nextSettings: Partial<AppSettings> = {
      providers: settings.providers.map((p, i) =>
        i === index ? { ...next, id } : p,
      ),
    };
    if (id !== current.id) {
      nextSettings.defaultModel = remapModelRef(
        settings.defaultModel,
        current.id,
        id,
      );
      nextSettings.defaultThinkingLevels = remapModelRefs(
        settings.defaultThinkingLevels,
        current.id,
        id,
      );
      // (ADR 0023) `subagentModels` is `agent name → model key`: the model
      // refs are the VALUES (the agent names are the KEYS — `remapModelRefs`
      // remaps map KEYS and would be a no-op here). A non-matching value
      // stays untouched.
      const remappedSubagentModels: Record<string, string> = {};
      for (const [name, model] of Object.entries(settings.subagentModels)) {
        remappedSubagentModels[name] = remapModelRef(model, current.id, id) ?? model;
      }
      nextSettings.subagentModels = remappedSubagentModels;
    }
    update(nextSettings);
    void listModels(id).then(setModels).catch(() => {});
  };

  /**
   * Add provider: append an editable row. No template → an empty row with
   * a placeholder id (`provider-N` — a name commit derives the real id
   * from the name, de-duped with a `-2` / `-3` suffix). A template (the
   * known-providers picker, ADR 0024) → a pre-filled row (name / base URL
   * / wire / key URL; the key is ALWAYS empty — the user pastes it; the id
   * is the template name's slug, de-duped the same way).
   */
  const addProvider = (template?: {
    name?: string;
    baseUrl?: string;
    api?: WireApi;
    keyUrl?: string;
  }) => {
    if (settings === null) return;
    const name = template?.name ?? "";
    const slug = slugify(name);
    const base = slug !== "" ? slug : `provider-${settings.providers.length + 1}`;
    const existing = new Set(settings.providers.map((p) => p.id));
    update({
      providers: [
        ...settings.providers,
        {
          id: uniqueSlug(base, existing),
          name,
          baseUrl: template?.baseUrl ?? "",
          apiKey: "",
          api: template?.api ?? "openai-completions",
          keyUrl: template?.keyUrl ?? null,
        },
      ],
    });
  };

  const removeProvider = (id: string) => {
    if (settings === null) return;
    update({ providers: settings.providers.filter((p) => p.id !== id) });
  };

  /** Toggle one tool in the enabled-tools list (immediate save — the CHECKED
   * names are the saved list; `[]` = all tools). */
  const toggleTool = (name: string) => {
    if (settings === null) return;
    const current = settings.enabledTools;
    const next = current.includes(name)
      ? current.filter((n) => n !== name)
      : [...current, name];
    update({ enabledTools: next });
  };

  /** Save an MCP server (the dialog's Save — immediate save, the provider pattern). */
  const saveMcpServer = (name: string, entry: McpServerEntry) => {
    if (settings === null) return;
    update({ mcpServers: { ...settings.mcpServers, [name]: entry } });
    setMcpDialog(null);
  };

  const removeMcpServer = (name: string) => {
    if (settings === null) return;
    const mcpServers = { ...settings.mcpServers };
    delete mcpServers[name];
    update({ mcpServers });
  };

  const refreshProvider = (id: string): Promise<void> =>
    listModels(id).then(setModels).catch(() => {});

  const generalSection = (
    <GeneralSection
      settings={settings}
      models={models}
      tools={tools}
      thinkingLevelUnion={thinkingLevelUnion}
      storedLevelOutsideUnion={storedLevelOutsideUnion}
      onSave={update}
      onToggleTool={toggleTool}
    />
  );

  const appearanceSection = (
    <AppearanceSection settings={settings} onSave={update} />
  );

  const providersSection = (
    <ProviderSection
      settings={settings}
      models={models}
      knownProviders={knownProviders}
      selectedKnown={selectedKnown}
      onSelectKnown={setSelectedKnown}
      onCommitField={commitProviderField}
      onRefresh={refreshProvider}
      onRemove={removeProvider}
      onAdd={addProvider}
    />
  );

  const subagentsSection = (
    <SubagentSection
      settings={settings}
      agentDefs={agentDefs}
      models={models}
      onSave={update}
    />
  );

  const mcpSection = (
    <McpSection
      settings={settings}
      dialog={mcpDialog}
      setDialog={setMcpDialog}
      onSave={saveMcpServer}
      onRemove={removeMcpServer}
    />
  );

  return (
    <div className="grid h-full grid-cols-[268px_minmax(0,1fr)]">
      <aside className="min-w-0 border-r border-border">
        <div className="flex h-full flex-col">
          <div className="px-2 pb-3 pt-3">
            <Button
              type="button"
              variant="ghost"
              size="lg"
              aria-label="Back"
              className="w-[calc(100%-0.5rem)] justify-start gap-2 rounded-xl px-1.5 text-foreground-subtle hover:bg-surface-hover hover:text-foreground"
              onClick={onBack}
            >
              <ArrowLeftIcon className="size-4" />
              <span>Back</span>
            </Button>
          </div>
          <nav aria-label="Settings sections" className="flex-1 overflow-y-auto px-2 pb-3">
            <div className="space-y-1">
              <SettingsSidebarButton
                icon={Settings2Icon}
                label="General"
                active={section === "general"}
                onClick={() => setSection("general")}
              />
              <SettingsSidebarButton
                icon={PaletteIcon}
                label="Appearance"
                active={section === "appearance"}
                onClick={() => setSection("appearance")}
              />
              <SettingsSidebarButton
                icon={PackageIcon}
                label="Providers"
                active={section === "providers"}
                onClick={() => setSection("providers")}
              />
              <SettingsSidebarButton
                icon={BotIcon}
                label="Subagents"
                active={section === "subagents"}
                onClick={() => setSection("subagents")}
              />
              <SettingsSidebarButton
                icon={ServerIcon}
                label="MCP"
                active={section === "mcp"}
                onClick={() => setSection("mcp")}
              />
            </div>
          </nav>
        </div>
      </aside>
      <section className="min-h-0 overflow-y-auto">
        <div className="mx-auto w-full max-w-4xl px-4 pb-8 pt-4">
          {section === "general"
            ? generalSection
            : section === "appearance"
              ? appearanceSection
              : section === "providers"
                ? providersSection
                : section === "subagents"
                  ? subagentsSection
                  : mcpSection}
        </div>
      </section>
    </div>
  );
}
