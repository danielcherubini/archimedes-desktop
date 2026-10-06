import { useState } from "react";
import type { ReactElement } from "react";
import { EyeIcon, EyeOffIcon, RefreshCwIcon, TrashIcon } from "lucide-react";
import type {
  AppSettings,
  KnownProvider,
  ModelDto,
  ProviderConfig,
  WireApi,
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  SettingsBadge,
  SettingsGroupCard,
  closedSelectValue,
} from "./primitives";
import { Field, TextField } from "./controls";

/** The Providers section's props (every piece of state stays in `SettingsPage`). */
interface ProviderSectionProps {
  settings: AppSettings | null;
  models: ModelDto[];
  /** The known-providers catalog (ADR 0024; `[]` = not loaded / failed). */
  knownProviders: KnownProvider[];
  /** The picker's selected template id (`""` = nothing selected). */
  selectedKnown: string;
  onSelectKnown: (id: string) => void;
  /** A provider field commit (immediate save + re-run its discovery). */
  onCommitField: (index: number, patch: Partial<ProviderConfig>) => void;
  onRefresh: (id: string) => Promise<void>;
  onRemove: (id: string) => void;
  onAdd: (template?: {
    name?: string;
    baseUrl?: string;
    api?: WireApi;
    keyUrl?: string;
  }) => void;
}
/** The endpoint APIs a provider row can speak (ADR 0024 + 0026: the wire the harness speaks, or a discovery mode — `litellm` routes discovery to `GET /model/info` on the openai-completions wire). */
const API_VALUES: WireApi[] = [
  "openai-completions",
  "anthropic-messages",
  "openai-responses",
  "litellm",
];

/** The api's human label (the provider row's `api` select + the picker's option labels). */
const API_LABELS: Record<WireApi, string> = {
  "anthropic-messages": "Anthropic",
  "openai-responses": "OpenAI Responses",
  litellm: "LiteLLM",
  "openai-completions": "OpenAI-compatible",
};

function apiLabel(api: WireApi): string {
  return API_LABELS[api];
}
/**
 * One provider row: a labeled field grid (Name / Base URL / API key
 * (masked, an eye toggle) / the discovery status + a refresh + a remove;
 * a third row: the wire `api` select + a "Get key" link when the row has
 * a `keyUrl`) — the `SettingsRow`'s label+control shape can't host three
 * text fields (they overflow the fixed 280px control column and get
 * clipped by the card's `overflow-hidden`), so the row is a full-width
 * `border-t` block (the `SettingsRow`'s `px-4 py-3` / `first:border-t-0`
 * pattern).
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
    <div className="border-t border-border px-4 py-3 first:border-t-0">
      <div className="grid grid-cols-1 gap-x-4 gap-y-3 sm:grid-cols-2">
        <Field label="Name">
          <TextField
            value={provider.name}
            placeholder="Provider name"
            ariaLabel="Provider name"
            className="w-full"
            onCommit={(name) => onCommitField({ name })}
          />
        </Field>
        <Field label="Base URL">
          <TextField
            value={provider.baseUrl}
            placeholder="https://example.com/v1"
            ariaLabel="Base URL"
            className="w-full"
            onCommit={(baseUrl) => onCommitField({ baseUrl })}
          />
        </Field>
        <Field label="API key">
          <div className="flex items-center gap-2">
            <div className="relative flex-1">
              <TextField
                value={provider.apiKey}
                placeholder="No key (local gateway)"
                ariaLabel="API key"
                type={showKey ? "text" : "password"}
                className="w-full pr-8"
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
            {provider.keyUrl ? (
              <a
                href={provider.keyUrl}
                target="_blank"
                rel="noreferrer"
                className="text-ui-base text-foreground-subtle hover:underline"
              >
                Get key
              </a>
            ) : null}
          </div>
        </Field>
        <Field label="API">
          <Select
            // A stored `api` outside the wire list (a hand-edited file) leaves
            // BOTH consumers on the OpenAI-compatible wire — `WireApi::parse`
            // falls back to it for discovery and for `build_provider` — so that
            // is the label the trigger shows instead of going blank (Radix
            // renders no label for a value with no matching item). Nothing is
            // re-saved for being unreadable.
            value={closedSelectValue(provider.api, API_VALUES, "openai-completions")}
            onValueChange={(value) => onCommitField({ api: value as WireApi })}
          >
            <SelectTrigger aria-label="API" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {API_VALUES.map((api) => (
                <SelectItem key={api} value={api}>
                  {apiLabel(api)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <div className="flex items-center justify-end gap-2">
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
      </div>
    </div>
  );
}

/** The Providers section: the provider rows + the known-providers picker. */
export default function ProviderSection({
  settings,
  models,
  knownProviders,
  selectedKnown,
  onSelectKnown,
  onCommitField,
  onRefresh,
  onRemove,
  onAdd,
}: ProviderSectionProps): ReactElement | null {
  if (settings === null) return null;
  return (
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
              onCommitField={(patch) => onCommitField(index, patch)}
              onRefresh={() => onRefresh(provider.id)}
              onRemove={() => onRemove(provider.id)}
            />
          ))
        )}
      </SettingsGroupCard>
      <div className="flex items-center gap-2">
        {/* (ADR 0024) The known-providers picker: a `Select` of the 20
            catalog templates (the label is `name — apiLabel(api)`) + an
            Add button that appends the template as a pre-filled row
            (the user's list stays the sole model source). */}
        <Select value={selectedKnown} onValueChange={onSelectKnown}>
          <SelectTrigger aria-label="Known provider" className="w-64">
            <SelectValue placeholder="Add a known provider…" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="" disabled>
              Add a known provider…
            </SelectItem>
            {knownProviders.map((template) => (
              <SelectItem key={template.id} value={template.id}>
                {`${template.name} — ${apiLabel(template.api)}`}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          disabled={selectedKnown === ""}
          onClick={() => {
            const template = knownProviders.find(
              (t) => t.id === selectedKnown,
            );
            if (template) {
              onAdd({
                name: template.name,
                baseUrl: template.baseUrl,
                api: template.api,
                keyUrl: template.keyUrl,
              });
            }
            onSelectKnown("");
          }}
        >
          Add
        </Button>
        <Button onClick={() => onAdd()}>Add provider</Button>
      </div>
    </div>
  );
}
