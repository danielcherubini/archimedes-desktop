import { useEffect, useRef, useState } from "react";
import type { ReactElement, ReactNode } from "react";
import {
  ArrowLeftIcon,
  EyeIcon,
  EyeOffIcon,
  KeyIcon,
  PackageIcon,
  PaletteIcon,
  PencilIcon,
  PlayIcon,
  RefreshCwIcon,
  ServerIcon,
  Settings2Icon,
  TrashIcon,
} from "lucide-react";

import { applySettingsToDocument, MAX_FONT_PX, MIN_FONT_PX } from "@/lib/settings";
import {
  authMcpServer,
  getSettings,
  listAgents,
  listModels,
  saveSettings,
  testMcpServer,
  type AgentEntryDto,
  type AppSettings,
  type McpServerEntry,
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
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

type Section = "general" | "appearance" | "providers" | "mcp";

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

/** A labeled provider field (a small caption above a full-width `TextField`). */
function Field({ label, children }: { label: string; children: ReactNode }): ReactElement {
  return (
    <div>
      <div className="mb-1 text-ui-sm text-foreground-subtle">{label}</div>
      {children}
    </div>
  );
}

/**
 * One MCP server row (ADR 0019 — the desktop's OWN entries only): the name
 * + a type badge (HTTP / stdio) + a one-line summary (the url, or
 * `command` + args) + the on-demand Test status + edit / remove. (pi's
 * `mcp.json` entries stay in their own files — not listed here.)
 */
function McpRow({
  name,
  entry,
  onEdit,
  onRemove,
}: {
  name: string;
  entry: McpServerEntry;
  onEdit: () => void;
  onRemove: () => void;
}): ReactElement {
  const [testing, setTesting] = useState(false);
  const [authenticating, setAuthenticating] = useState(false);
  const [result, setResult] = useState<
    { count: number } | { error: string } | null
  >(null);
  const isHttp = entry.url !== undefined;
  const isOAuth = entry.auth !== undefined;
  const kind = isHttp ? "HTTP" : "stdio";
  const summary = isHttp
    ? entry.url
    : [entry.command ?? "", ...(entry.args ?? [])].join(" ").trim();

  const runTest = async () => {
    if (testing || authenticating) return;
    setTesting(true);
    setResult(null);
    try {
      const count = await testMcpServer(entry, name);
      setResult({ count });
    } catch (e) {
      setResult({ error: e instanceof Error ? e.message : String(e) });
    } finally {
      setTesting(false);
    }
  };

  const runAuth = async () => {
    if (testing || authenticating) return;
    setAuthenticating(true);
    setResult(null);
    try {
      await authMcpServer(name, entry);
      const count = await testMcpServer(entry, name);
      setResult({ count });
    } catch (e) {
      setResult({ error: e instanceof Error ? e.message : String(e) });
    } finally {
      setAuthenticating(false);
    }
  };

  const needsAuth =
    result !== null && "error" in result && result.error.includes("needs-auth");

  return (
    <div className="border-t border-border px-4 py-3 first:border-t-0">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-ui-base font-medium text-foreground">
              {name}
            </span>
            <SettingsBadge>{kind}</SettingsBadge>
            {isOAuth && <SettingsBadge>OAuth</SettingsBadge>}
          </div>
          <div className="truncate text-ui-sm text-foreground-subtle">{summary}</div>
        </div>
        {authenticating ? (
          <SettingsBadge>signing in…</SettingsBadge>
        ) : testing ? (
          <SettingsBadge>testing…</SettingsBadge>
        ) : result !== null ? (
          "count" in result ? (
            <SettingsBadge>{`${result.count} tools`}</SettingsBadge>
          ) : needsAuth ? (
            <SettingsBadge title={result.error}>needs auth</SettingsBadge>
          ) : (
            <SettingsBadge title={result.error}>error</SettingsBadge>
          )
        ) : null}
        {(isOAuth || needsAuth) && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`Authenticate MCP server ${name}`}
            title="Sign in with OAuth"
            disabled={testing || authenticating}
            onClick={() => void runAuth()}
          >
            <KeyIcon className="size-3.5" />
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={`Test MCP server ${name}`}
          disabled={testing || authenticating}
          onClick={() => void runTest()}
        >
          <PlayIcon className="size-3.5" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={`Edit MCP server ${name}`}
          onClick={onEdit}
        >
          <PencilIcon className="size-3.5" />
        </Button>
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={`Remove MCP server ${name}`}
            >
              <TrashIcon className="size-3.5" />
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Remove MCP server {name}?</AlertDialogTitle>
              <AlertDialogDescription>
                The server leaves the harness's `mcp` tool (its tools are no
                longer callable). Existing sessions are unaffected.
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
  );
}

/** The add/edit form draft (the `McpDialog`'s local state — one field per input). */
interface McpDraft {
  name: string;
  kind: "http" | "stdio";
  url: string;
  headers: string; // `KEY=VALUE` lines
  bearerTokenEnv: string;
  useOAuth: boolean;
  command: string;
  args: string; // one arg per line
  env: string; // `KEY=VALUE` lines
  cwd: string;
}

/** The `mcpServers` entry → draft (the form's pre-fill for the EDIT case). */
function entryToDraft(name: string, entry: McpServerEntry): McpDraft {
  const mapToLines = (m?: Record<string, string>): string =>
    m ? Object.entries(m).map(([k, v]) => `${k}=${v}`).join("\n") : "";
  return {
    name,
    kind: entry.url !== undefined ? "http" : "stdio",
    url: entry.url ?? "",
    headers: mapToLines(entry.headers),
    bearerTokenEnv: entry.bearerTokenEnv ?? "",
    useOAuth: entry.auth !== undefined,
    command: entry.command ?? "",
    args: (entry.args ?? []).join("\n"),
    env: mapToLines(entry.env),
    cwd: entry.cwd ?? "",
  };
}

/** `KEY=VALUE` lines → a string map (`undefined` when empty; a line without
 * a `=` is skipped — the form is best-effort, not a linter). */
function linesToMap(lines: string): Record<string, string> | undefined {
  const out: Record<string, string> = {};
  for (const line of lines.split("\n")) {
    const idx = line.indexOf("=");
    if (idx > 0) out[line.slice(0, idx).trim()] = line.slice(idx + 1).trim();
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

/** draft → the `mcpServers` entry (the empty fields are OMITTED — a clean entry). */
function draftToEntry(d: McpDraft): McpServerEntry {
  const entry: McpServerEntry = {};
  if (d.kind === "http") {
    if (d.url.trim() !== "") entry.url = d.url.trim();
    const headers = linesToMap(d.headers);
    if (headers) entry.headers = headers;
    if (d.bearerTokenEnv.trim() !== "")
      entry.bearerTokenEnv = d.bearerTokenEnv.trim();
    if (d.useOAuth) entry.auth = "oauth";
  } else {
    if (d.command.trim() !== "") entry.command = d.command.trim();
    const args = d.args.split("\n").map((l) => l.trim()).filter((l) => l !== "");
    if (args.length > 0) entry.args = args;
    const env = linesToMap(d.env);
    if (env) entry.env = env;
    if (d.cwd.trim() !== "") entry.cwd = d.cwd.trim();
  }
  return entry;
}

/**
 * The add/edit form (ADR 0019 — a structured form, no raw JSON): Name + a
 * kind toggle (HTTP / stdio) + the kind's fields. Save = the immediate
 * `saveSettings` (the provider pattern); the entry is pi's shape VERBATIM.
 */
function McpDialog({
  initial,
  onSave,
  onClose,
}: {
  initial: McpDraft;
  onSave: (name: string, entry: McpServerEntry) => void;
  onClose: () => void;
}): ReactElement {
  const [draft, setDraft] = useState(initial);
  const set = (patch: Partial<McpDraft>) =>
    setDraft((d) => ({ ...d, ...patch }));
  const valid =
    draft.name.trim() !== "" &&
    (draft.kind === "http"
      ? draft.url.trim() !== ""
      : draft.command.trim() !== "");
  return (
    <Dialog open onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {initial.name === ""
              ? "Add MCP server"
              : `Edit MCP server ${initial.name}`}
          </DialogTitle>
          <DialogDescription>
            Stored in the desktop's settings.json (pi's entry shape — it
            copy-pastes to a mcp.json). Merged with pi's mcp.json files at
            global precedence (project &gt; desktop &gt; pi-global).
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <Field label="Name">
            <Input
              value={draft.name}
              placeholder="Server name"
              aria-label="Server name"
              onChange={(e) => set({ name: e.currentTarget.value })}
            />
          </Field>
          <Field label="Type">
            <Select
              value={draft.kind}
              onValueChange={(v) => set({ kind: v as McpDraft["kind"] })}
            >
              <SelectTrigger aria-label="Type" className="w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="http">HTTP (streamable)</SelectItem>
                <SelectItem value="stdio">stdio (a spawned process)</SelectItem>
              </SelectContent>
            </Select>
          </Field>
          {draft.kind === "http" ? (
            <>
              <Field label="URL">
                <Input
                  value={draft.url}
                  placeholder="https://example.com/mcp"
                  aria-label="URL"
                  onChange={(e) => set({ url: e.currentTarget.value })}
                />
              </Field>
              <Field label="Headers (one KEY=VALUE per line)">
                <Textarea
                  value={draft.headers}
                  placeholder="Authorization=Bearer $TOKEN"
                  aria-label="Headers"
                  rows={2}
                  onChange={(e) => set({ headers: e.currentTarget.value })}
                />
              </Field>
              <Field label="Bearer token env var">
                <Input
                  value={draft.bearerTokenEnv}
                  placeholder="MY_TOKEN"
                  aria-label="Bearer token env var"
                  onChange={(e) => set({ bearerTokenEnv: e.currentTarget.value })}
                />
              </Field>
              <div>
                <div className="mb-1 text-ui-sm text-foreground-subtle">
                  OAuth (interactive flow)
                </div>
                <Switch
                  checked={draft.useOAuth}
                  aria-label="OAuth (interactive flow)"
                  onCheckedChange={(c) => set({ useOAuth: c })}
                />
              </div>
            </>
          ) : (
            <>
              <Field label="Command">
                <Input
                  value={draft.command}
                  placeholder="npx"
                  aria-label="Command"
                  onChange={(e) => set({ command: e.currentTarget.value })}
                />
              </Field>
              <Field label="Args (one per line)">
                <Textarea
                  value={draft.args}
                  placeholder={"-y\nexample-mcp"}
                  aria-label="Args"
                  rows={2}
                  onChange={(e) => set({ args: e.currentTarget.value })}
                />
              </Field>
              <Field label="Env (one KEY=VALUE per line)">
                <Textarea
                  value={draft.env}
                  placeholder="KEY=value"
                  aria-label="Env"
                  rows={2}
                  onChange={(e) => set({ env: e.currentTarget.value })}
                />
              </Field>
              <Field label="Working dir">
                <Input
                  value={draft.cwd}
                  placeholder="/home/user/project"
                  aria-label="Working dir"
                  onChange={(e) => set({ cwd: e.currentTarget.value })}
                />
              </Field>
            </>
          )}
        </div>
        <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button
          disabled={!valid}
          onClick={() => onSave(draft.name.trim(), draftToEntry(draft))}
        >
          Save
        </Button>
      </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * One provider row: a labeled 2×2 grid (Name / Base URL / API key (masked,
 * an eye toggle) / the discovery status + a refresh + a remove) — the
 * `SettingsRow`'s label+control shape can't host three text fields (they
 * overflow the fixed 280px control column and get clipped by the card's
 * `overflow-hidden`), so the row is a full-width `border-t` block (the
 * `SettingsRow`'s `px-4 py-3` / `first:border-t-0` pattern).
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
          <div className="relative">
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

/**
 * The settings page (the approved ZCode-parity design, spec §1/§4): a full
 * content-area view — a 268px section sidebar (back button + General /
 * Appearance / Providers / MCP) + the active section's content. Immediate save:
 * a control change → `saveSettings` with the COMPLETE document (text
 * fields commit on blur/Enter; no save button, no dirty state).
 */
export default function SettingsPage({ onBack }: { onBack: () => void }): ReactElement {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [section, setSection] = useState<Section>("general");
  const [agents, setAgents] = useState<AgentEntryDto[]>([]);
  const [models, setModels] = useState<ModelDto[]>([]);
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
    }
    update(nextSettings);
    void listModels(id).then(setModels).catch(() => {});
  };

  /**
   * Add provider: append an empty editable row with a placeholder id
   * (`provider-N` — a name commit derives the real id from the name,
   * de-duped with a `-2` / `-3` suffix).
   */
  const addProvider = (name = "") => {
    if (settings === null) return;
    const slug = slugify(name);
    const base = slug !== "" ? slug : `provider-${settings.providers.length + 1}`;
    const existing = new Set(settings.providers.map((p) => p.id));
    update({
      providers: [
        ...settings.providers,
        { id: uniqueSlug(base, existing), name: "", baseUrl: "", apiKey: "" },
      ],
    });
  };

  const removeProvider = (id: string) => {
    if (settings === null) return;
    update({ providers: settings.providers.filter((p) => p.id !== id) });
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

  // (ADR 0019) The MCP section: the desktop's OWN `mcpServers` entries
  // (pi's `mcp.json` entries stay in their own files — not listed here).
  const mcpSection = settings === null ? null : (
    <div className="space-y-3">
      <SettingsGroupCard>
        {Object.keys(settings.mcpServers).length === 0 ? (
          <div className="px-4 py-3 text-ui-base text-foreground-subtle">
            No MCP servers yet — add one to connect an MCP server.
          </div>
        ) : (
          Object.entries(settings.mcpServers).map(([name, entry]) => (
            <McpRow
              key={name}
              name={name}
              entry={entry}
              onEdit={() => setMcpDialog(entryToDraft(name, entry))}
              onRemove={() => removeMcpServer(name)}
            />
          ))
        )}
      </SettingsGroupCard>
      <Button
        onClick={() =>
          setMcpDialog({
            name: "",
            kind: "http",
            url: "",
            headers: "",
            bearerTokenEnv: "",
            useOAuth: false,
            command: "",
            args: "",
            env: "",
            cwd: "",
          })
        }
      >
        Add server
      </Button>
      {mcpDialog !== null && (
        <McpDialog
          initial={mcpDialog}
          onSave={saveMcpServer}
          onClose={() => setMcpDialog(null)}
        />
      )}
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
                : mcpSection}
        </div>
      </section>
    </div>
  );
}
