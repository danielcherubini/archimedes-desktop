import { useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";
import {
  ArrowLeftIcon,
  EyeIcon,
  EyeOffIcon,
  PackageIcon,
  PaletteIcon,
  RefreshCwIcon,
  Settings2Icon,
  TrashIcon,
} from "lucide-react";

import { applySettingsToDocument, MAX_FONT_PX, MIN_FONT_PX } from "@/lib/settings";
import {
  getSettings,
  listAgents,
  listModels,
  saveSettings,
  type AgentEntryDto,
  type AppSettings,
  type ModelDto,
  type ProviderConfig,
} from "@/lib/tauri";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  SettingsBadge,
  SettingsGroupCard,
  SettingsRow,
  SettingsSidebarButton,
} from "./primitives";

type Section = "general" | "appearance" | "providers";

/** A ZCode-style slug: lowercase alphanumeric + `-`. */
function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/**
 * The ZCode `FontSizeInput` pattern: a `w-28` number `Input` (12–20,
 * `px` suffix) with a local draft — commit on blur/Enter (clamped),
 * Escape cancels the draft.
 */
function FontSizeInput({
  value,
  min,
  max,
  ariaLabel,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  ariaLabel: string;
  onChange: (value: number) => void;
}): ReactElement {
  const [draft, setDraft] = useState(String(value));
  const commit = () => {
    const parsed = draft.trim() === "" ? Number.NaN : Number(draft);
    const next = Number.isFinite(parsed)
      ? Math.min(max, Math.max(min, Math.round(parsed)))
      : value;
    setDraft(String(next));
    if (next !== value) onChange(next);
  };
  return (
    <div className="relative w-28">
      <Input
        type="number"
        inputMode="numeric"
        min={min}
        max={max}
        step={1}
        value={draft}
        aria-label={ariaLabel}
        onChange={(event) => setDraft(event.currentTarget.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.currentTarget.blur();
          } else if (event.key === "Escape") {
            event.preventDefault();
            setDraft(String(value));
          }
        }}
        className="pr-8"
      />
      <span className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-ui-sm text-foreground-subtlest">
        px
      </span>
    </div>
  );
}

/**
 * A provider text field (immediate save): a local draft that commits on
 * blur/Enter ONLY when the value actually changed (a no-change blur saves
 * nothing and re-runs no discovery).
 */
function TextField({
  value,
  placeholder,
  ariaLabel,
  className,
  type = "text",
  onCommit,
}: {
  value: string;
  placeholder?: string;
  ariaLabel: string;
  className?: string;
  type?: "text" | "password";
  onCommit: (value: string) => void;
}): ReactElement {
  const [draft, setDraft] = useState(value);
  // Re-sync when the committed value changes externally.
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return (
    <Input
      type={type}
      value={draft}
      placeholder={placeholder}
      aria-label={ariaLabel}
      className={className}
      onChange={(event) => setDraft(event.currentTarget.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
    />
  );
}

/**
 * One provider row (the ZCode model-provider pattern): Name / Base URL /
 * API key (masked, an eye toggle) + the discovery status + a refresh + a
 * remove. The `id` is generated ONCE at add time and NEVER changes
 * afterwards (a later name commit must NOT regenerate it — `defaultModel`
 * references + the discovery cache entries would silently orphan).
 */
function ProviderRow({
  provider,
  modelCount,
  onCommitField,
  onRefresh,
  onRemove,
}: {
  provider: ProviderConfig;
  modelCount: number;
  onCommitField: (patch: Partial<ProviderConfig>) => void;
  onRefresh: () => Promise<void>;
  onRemove: () => void;
}): ReactElement {
  const [showKey, setShowKey] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const refresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    try {
      await onRefresh();
    } finally {
      setRefreshing(false);
    }
  };
  // The discovery status: `{n} models` when the provider's models are
  // present in the catalog, `unreachable` when it has 0, `checking…`
  // while a refresh is in flight.
  const status = refreshing
    ? "checking…"
    : modelCount > 0
      ? `${modelCount} models`
      : "unreachable";
  return (
    <SettingsRow
      label={provider.name || "Unnamed provider"}
      controlLayout="wide"
      control={
        <div className="flex items-center gap-2">
          <TextField
            value={provider.name}
            placeholder="Provider name"
            ariaLabel="Provider name"
            className="w-40"
            onCommit={(name) => onCommitField({ name })}
          />
          <TextField
            value={provider.baseUrl}
            placeholder="https://example.com/v1"
            ariaLabel="Base URL"
            className="w-56"
            onCommit={(baseUrl) => onCommitField({ baseUrl })}
          />
          <div className="relative w-40">
            <TextField
              value={provider.apiKey}
              placeholder="No key (local gateway)"
              ariaLabel="API key"
              type={showKey ? "text" : "password"}
              className="pr-8"
              onCommit={(apiKey) => onCommitField({ apiKey })}
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={showKey ? "Hide API key" : "Show API key"}
              className="absolute top-1/2 right-1.5 -translate-y-1/2"
              onClick={() => setShowKey((v) => !v)}
            >
              {showKey ? <EyeOffIcon className="size-3.5" /> : <EyeIcon className="size-3.5" />}
            </Button>
          </div>
          <SettingsBadge>{status}</SettingsBadge>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label="Refresh models"
            onClick={() => void refresh()}
          >
            <RefreshCwIcon className="size-3.5" />
          </Button>
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label="Remove provider"
              >
                <TrashIcon className="size-3.5" />
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>
                  Remove provider {provider.name || "Untitled"}?
                </AlertDialogTitle>
                <AlertDialogDescription>
                  Its {modelCount} models will leave the model picker.
                  Existing sessions are unaffected.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={onRemove}>Remove</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      }
    />
  );
}

/**
 * The settings page (the approved ZCode-parity design, spec §1/§4): a full
 * content-area view — a 268px section sidebar (back button + General /
 * Appearance / Providers) + the active section's content. Immediate save:
 * a control change → `saveSettings` with the COMPLETE document (text
 * fields commit on blur/Enter; no save button, no dirty state).
 */
export default function SettingsPage({ onBack }: { onBack: () => void }): ReactElement {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [section, setSection] = useState<Section>("general");
  const [agents, setAgents] = useState<AgentEntryDto[]>([]);
  const [models, setModels] = useState<ModelDto[]>([]);
  // The theme cleanup is HELD IN A REF and replaced on each apply —
  // discarding it would leak a `matchMedia` listener per control change
  // (`applySettingsToDocument` subscribes one for "system" themes).
  const themeCleanupRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    // The page's single source of truth: the settings document.
    getSettings()
      .then(setSettings)
      .catch(() => {});
    listAgents()
      .then(setAgents)
      .catch(() => {});
    listModels()
      .then(setModels)
      .catch(() => {});
    return () => {
      themeCleanupRef.current?.();
    };
  }, []);

  /** The immediate-save pattern (one helper, used by every control). */
  const update = (patch: Partial<AppSettings>) => {
    if (settings === null) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    void saveSettings(next); // the complete document
    themeCleanupRef.current?.(); // drop the previous theme listener
    themeCleanupRef.current = applySettingsToDocument(next); // live theme/font
  };

  /**
   * A provider field commit (immediate save): `update` + re-run the changed
   * provider's discovery (Task 2's `force_refresh` bypasses its cache).
   */
  const commitProviderField = (index: number, patch: Partial<ProviderConfig>) => {
    if (settings === null) return;
    const providers = settings.providers.map((p, i) =>
      i === index ? { ...p, ...patch } : p,
    );
    update({ providers });
    void listModels(settings.providers[index].id).then(setModels).catch(() => {});
  };

  /**
   * Add provider: append an empty editable row whose `id` is generated
   * ONCE at add time and NEVER changes afterwards (`slug(name)` when a
   * name is already known at add time, else `provider-N`; de-duped with a
   * `-2` / `-3` suffix).
   */
  const addProvider = (name = "") => {
    if (settings === null) return;
    const slug = slugify(name);
    const base = slug !== "" ? slug : `provider-${settings.providers.length + 1}`;
    const existing = new Set(settings.providers.map((p) => p.id));
    let id = base;
    let n = 2;
    while (existing.has(id)) {
      id = `${base}-${n}`;
      n += 1;
    }
    update({
      providers: [...settings.providers, { id, name: "", baseUrl: "", apiKey: "" }],
    });
  };

  const removeProvider = (id: string) => {
    if (settings === null) return;
    update({ providers: settings.providers.filter((p) => p.id !== id) });
  };

  const refreshProvider = (id: string): Promise<void> =>
    listModels(id).then(setModels).catch(() => {});

  const generalSection = settings === null ? null : (
    <SettingsGroupCard>
      <SettingsRow
        label="Default agent"
        description="The agent new sessions start with"
        control={
          <Select
            value={settings.defaultAgent ?? ""}
            onValueChange={(value) =>
              update({ defaultAgent: value === "" ? null : value })
            }
          >
            <SelectTrigger aria-label="Default agent" className="w-48">
              <SelectValue placeholder="Select an agent…" />
            </SelectTrigger>
            <SelectContent>
              {agents.map((agent) => (
                <SelectItem key={agent.id} value={agent.id}>
                  {agent.name}
                </SelectItem>
              ))}
              <SelectItem value="">Default (first in the list)</SelectItem>
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Trust new Spaces by default"
        description="New Spaces start trusted (skip permission prompts for bash/edit/write); existing Spaces are unaffected"
        control={
          <Switch
            checked={settings.defaultTrustNewSpaces}
            aria-label="Trust new Spaces by default"
            onCheckedChange={(checked) =>
              update({ defaultTrustNewSpaces: checked })
            }
          />
        }
      />
      <SettingsRow
        label="Default model"
        description="The model new sessions start with (both native and external sessions)"
        control={
          <Select
            value={settings.defaultModel ?? ""}
            onValueChange={(value) =>
              update({ defaultModel: value === "" ? null : value })
            }
          >
            <SelectTrigger aria-label="Default model" className="w-64">
              <SelectValue placeholder="System default" />
            </SelectTrigger>
            <SelectContent>
              {models.map((model) => (
                <SelectItem
                  key={`${model.provider}/${model.id}`}
                  value={`${model.provider}/${model.id}`}
                >
                  {model.provider}/{model.id}
                </SelectItem>
              ))}
              <SelectItem value="">System default</SelectItem>
            </SelectContent>
          </Select>
        }
      />
    </SettingsGroupCard>
  );

  const appearanceSection = settings === null ? null : (
    <SettingsGroupCard>
      <SettingsRow
        label="Theme"
        control={
          <Select
            value={settings.theme}
            onValueChange={(value) =>
              update({ theme: value as AppSettings["theme"] })
            }
          >
            <SelectTrigger aria-label="Theme" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">System (follow the OS)</SelectItem>
              <SelectItem value="dark">Dark</SelectItem>
              <SelectItem value="light">Light</SelectItem>
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Font size"
        description="The UI base size (12–20px)"
        control={
          <FontSizeInput
            value={settings.font.sizePx}
            min={MIN_FONT_PX}
            max={MAX_FONT_PX}
            ariaLabel="Font size"
            onChange={(sizePx) =>
              update({ font: { ...settings.font, sizePx } })
            }
          />
        }
      />
      <SettingsRow
        label="UI font"
        control={
          <Select
            value={settings.font.uiFamily ?? ""}
            onValueChange={(value) =>
              update({
                font: {
                  ...settings.font,
                  uiFamily: value === "" ? null : value,
                },
              })
            }
          >
            <SelectTrigger aria-label="UI font" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="">System (default)</SelectItem>
              <SelectItem value="ui-serif, Georgia, serif">Serif</SelectItem>
              <SelectItem value="ui-monospace, SFMono-Regular, Menlo, monospace">
                Monospace
              </SelectItem>
            </SelectContent>
          </Select>
        }
      />
      <SettingsRow
        label="Code font"
        control={
          <Select
            value={settings.font.codeFamily ?? ""}
            onValueChange={(value) =>
              update({
                font: {
                  ...settings.font,
                  codeFamily: value === "" ? null : value,
                },
              })
            }
          >
            <SelectTrigger aria-label="Code font" className="w-48">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="">System (default)</SelectItem>
              <SelectItem value="JetBrains Mono">JetBrains Mono</SelectItem>
              <SelectItem value="Fira Code">Fira Code</SelectItem>
              <SelectItem value="Cascadia Code">Cascadia Code</SelectItem>
              <SelectItem value="Menlo">Menlo</SelectItem>
              <SelectItem value="Consolas">Consolas</SelectItem>
            </SelectContent>
          </Select>
        }
      />
    </SettingsGroupCard>
  );

  const providersSection = settings === null ? null : (
    <div className="space-y-3">
      <SettingsGroupCard>
        {settings.providers.length === 0 ? (
          <div className="px-4 py-3 text-ui-base text-foreground-subtle">
            No providers yet — add one to connect a model provider.
          </div>
        ) : (
          settings.providers.map((provider, index) => (
            <ProviderRow
              key={provider.id}
              provider={provider}
              modelCount={models.filter((m) => m.provider === provider.id).length}
              onCommitField={(patch) => commitProviderField(index, patch)}
              onRefresh={() => refreshProvider(provider.id)}
              onRemove={() => removeProvider(provider.id)}
            />
          ))
        )}
      </SettingsGroupCard>
      <Button onClick={() => addProvider()}>Add provider</Button>
    </div>
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
              : providersSection}
        </div>
      </section>
    </div>
  );
}
