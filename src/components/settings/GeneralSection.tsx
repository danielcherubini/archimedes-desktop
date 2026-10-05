import type { ReactElement } from "react";
import type { AppSettings, ModelDto } from "@/lib/tauri";
import ModelPicker from "@/components/ModelPicker";
import { modelItemsFromCatalog } from "@/components/ModelPickerDialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { SettingsGroupCard, SettingsRow } from "./primitives";

/** The General section's props (every piece of state stays in `SettingsPage`). */
interface GeneralSectionProps {
  settings: AppSettings | null;
  models: ModelDto[];
  /** The native harness's tool names (the enabled-tools checkbox list). */
  tools: string[];
  /** The union of the `listModels` results' advertised thinking levels. */
  thinkingLevelUnion: string[];
  /** A stored default level the union no longer advertises (`null` = none). */
  storedLevelOutsideUnion: string | null;
  /** The immediate-save pattern. */
  onSave: (patch: Partial<AppSettings>) => void;
  /** Toggle one tool in the enabled-tools list (immediate save). */
  onToggleTool: (name: string) => void;
}

/** The General section: trust default / default model / thinking level / tools. */
export default function GeneralSection({
  settings,
  models,
  tools,
  thinkingLevelUnion,
  storedLevelOutsideUnion,
  onSave,
  onToggleTool,
}: GeneralSectionProps): ReactElement | null {
  if (settings === null) return null;
  return (
    <SettingsGroupCard>
      <SettingsRow
        label="Trust new Spaces by default"
        description="New Spaces start trusted (skip permission prompts for bash/edit/write); existing Spaces are unaffected"
        control={
          <Switch
            checked={settings.defaultTrustNewSpaces}
            aria-label="Trust new Spaces by default"
            onCheckedChange={(checked) =>
              onSave({ defaultTrustNewSpaces: checked })
            }
          />
        }
      />
      <SettingsRow
        label="Default model"
        description="The model new sessions start with (the system default when unset)"
        control={
          // The SHARED `ModelPicker` (the one component every model picker
          // uses — a trigger opening the fuzzy-searched, alphabetical
          // dialog; the catalog is too long for a Radix dropdown). The
          // items are the SHARED derivation (the row name is the BARE
          // model id — the provider prefix is dropped; the provider cue is
          // the provider's display name — `tama` → `Tama` from the
          // configured providers) + the "System default" row (value `""`
          // → `defaultModel: null`).
          <ModelPicker
            label="Default model"
            value={settings.defaultModel ?? ""}
            placeholder="System default"
            items={[
              ...modelItemsFromCatalog(
                models.map((model) => `${model.provider}/${model.id}`),
                settings.providers,
              ),
              { value: "", name: "System default" },
            ]}
            onSelect={(value) =>
              onSave({ defaultModel: value === "" ? null : value })
            }
          />
        }
      />
      <SettingsRow
        label="Default thinking level"
        description="The thinking level new sessions start with (the model's own default when unset)"
        control={
          <Select
            value={settings.defaultThinkingLevel ?? ""}
            onValueChange={(value) =>
              onSave({ defaultThinkingLevel: value === "" ? null : value })
            }
          >
            <SelectTrigger aria-label="Default thinking level" className="w-48">
              <SelectValue placeholder="Model default" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="">Model default</SelectItem>
              {thinkingLevelUnion.map((level) => (
                <SelectItem key={level} value={level}>
                  {level}
                </SelectItem>
              ))}
              {storedLevelOutsideUnion !== null && (
                <SelectItem value={storedLevelOutsideUnion}>
                  {storedLevelOutsideUnion}
                </SelectItem>
              )}
            </SelectContent>
          </Select>
        }
      />
      {/* The enabled-tools list (the harness-level tool filter — `[]` = all
          tools: "none checked" is a valid, meaningful state, it means all). */}
      <div className="border-t border-border px-4 py-3">
        <div className="text-ui-base font-medium text-foreground">
          Enabled tools
        </div>
        <div className="mt-1 text-ui-base leading-6 text-foreground-subtle">
          The harness tools new sessions start with — empty = all tools
        </div>
        {tools.length === 0 ? (
          <div className="mt-2 text-ui-sm text-foreground-subtlest">
            Loading…
          </div>
        ) : (
          <div className="mt-2 flex flex-wrap gap-x-4 gap-y-2">
            {tools.map((name) => (
              <label
                key={name}
                className="flex items-center gap-2 text-ui-base text-foreground"
              >
                <input
                  type="checkbox"
                  checked={settings.enabledTools.includes(name)}
                  aria-label={`Enable tool ${name}`}
                  onChange={() => onToggleTool(name)}
                />
                {name}
              </label>
            ))}
          </div>
        )}
      </div>
    </SettingsGroupCard>
  );
}
