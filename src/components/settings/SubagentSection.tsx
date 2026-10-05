import type { ReactElement } from "react";
import { TrashIcon } from "lucide-react";
import type { AgentDefinitionDto, AppSettings, ModelDto } from "@/lib/tauri";
import ModelPicker from "@/components/ModelPicker";
import { modelItemsFromCatalog } from "@/components/ModelPickerDialog";
import { Button } from "@/components/ui/button";
import { SettingsGroupCard, SettingsRow } from "./primitives";

/**
 * ASCII-only case fold (mirrors the backend's `eq_ignore_ascii_case` — a
 * Unicode `toLowerCase()` would fold non-ASCII pairs the backend ignores).
 */
function foldAscii(s: string): string {
  return s.replace(/[A-Z]/g, (c) => c.toLowerCase());
}

/** The Subagents section's props (every piece of state stays in `SettingsPage`). */
interface SubagentSectionProps {
  settings: AppSettings | null;
  /** The user-level discovered definitions (`null` until the first list lands). */
  agentDefs: AgentDefinitionDto[] | null;
  models: ModelDto[];
  /** The immediate-save pattern. */
  onSave: (patch: Partial<AppSettings>) => void;
}

/** The Subagents section: one row per discovered agent + its model override. */
export default function SubagentSection({
  settings,
  agentDefs,
  models,
  onSave,
}: SubagentSectionProps): ReactElement | null {
  if (settings === null) return null;
  // (ADR 0023) Orphaned override entries (a key matching no discovered
  // agent — its file was removed/renamed): the ONLY cleanup path is the
  // row's remove button. Gated on `agentDefs !== null`: while the
  // definitions are still loading, `agentDefs` is `null` — treating "not
  // loaded yet" as "no discovered agents" would flash every override as a
  // stale orphan with a live remove button before the list lands.
  const orphanedSubagentModels =
    agentDefs === null
      ? []
      : Object.entries(settings.subagentModels).filter(
          ([name]) =>
            !agentDefs.some((d) => foldAscii(d.name) === foldAscii(name)),
        );

  // (ADR 0023) The Subagents section: one row per discovered user-level
  // agent (name + description + the file's model — read-only) + a model
  // `Select` defaulting to "File value (no override)" (immediate save: a
  // pick sets `subagentModels[name]`, "File value" deletes the key) + the
  // orphaned entries as muted rows with a remove button.
  return (
    <SettingsGroupCard>
      {agentDefs?.map((def) => {
        // (ADR 0023 review) The backend resolves the `subagentModels` keys
        // case-insensitively (`eq_ignore_ascii_case` — a hand-edited
        // mixed-case key must still match), so the row's stored override is
        // looked up the SAME way — an exact-case lookup would make a
        // hand-edited `"Scout"` for agent `scout` invisible in the UI (the
        // Select would show the "File value (no override)" placeholder — a
        // false claim about the saved state).
        const storedEntry = Object.entries(settings.subagentModels).find(
          ([key]) => foldAscii(key) === foldAscii(def.name),
        );
        const storedOverride = storedEntry?.[1];
        // A stored override whose model key is no longer in the catalog
        // (a renamed/removed provider) matches no `SelectItem` — Radix
        // would fall back to the placeholder and misrepresent the saved
        // state, so it is offered as its own (disabled) option (the
        // `storedLevelOutsideUnion` pattern).
        const staleOverride =
          storedOverride !== undefined &&
          !models.some((m) => `${m.provider}/${m.id}` === storedOverride)
            ? storedOverride
            : null;
        return (
          <SettingsRow
            key={def.name}
            label={def.name}
            description={`${def.description || "No description"} · file: ${def.model ?? "— (inherits parent model)"}`}
            control={
              // The SHARED `ModelPicker` (ADR 0023): the catalog's models
              // (the shared derivation — the row name is the BARE model
              // id; the provider cue is the display name)
              // + the "File value (no override)" row (picking it DELETES
              // the key) + a stale stored override as its own DISABLED
              // row (a key no longer in the catalog — the
              // `storedLevelOutsideUnion` pattern).
              <ModelPicker
                label={`Subagent model for ${def.name}`}
                value={storedOverride ?? ""}
                placeholder="File value (no override)"
                items={[
                  ...modelItemsFromCatalog(
                    models.map((model) => `${model.provider}/${model.id}`),
                    settings.providers,
                  ),
                  { value: "", name: "File value (no override)" },
                  ...(staleOverride !== null
                    ? [
                        { value: staleOverride, name: staleOverride, disabled: true },
                      ]
                    : []),
                ]}
                onSelect={(value) => {
                  const subagentModels = { ...settings.subagentModels };
                  // (ADR 0023 review) Drop EVERY key that is a case-variant
                  // of this agent's name (not just the exact `def.name`
                  // key) — leaving a twin (`"Scout"` alongside `scout`)
                  // would make the backend's case-insensitive `.find` pick
                  // between them in nondeterministic HashMap iteration order.
                  const foldedName = foldAscii(def.name);
                  for (const key of Object.keys(subagentModels)) {
                    if (foldAscii(key) === foldedName) delete subagentModels[key];
                  }
                  if (value !== "") subagentModels[def.name] = value;
                  onSave({ subagentModels });
                }}
              />
            }
          />
        );
      })}
      {orphanedSubagentModels.map(([name]) => (
        <SettingsRow
          key={name}
          label={<span className="text-foreground-subtle">{name}</span>}
          description="No longer discovered (stale override)"
          control={
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={`Remove subagent override ${name}`}
              onClick={() => {
                const subagentModels = { ...settings.subagentModels };
                delete subagentModels[name];
                onSave({ subagentModels });
              }}
            >
              <TrashIcon className="size-3.5" />
            </Button>
          }
        />
      ))}
    </SettingsGroupCard>
  );
}
