import { useState } from "react";
import type { ReactElement } from "react";
import { KeyIcon, PencilIcon, PlayIcon, TrashIcon } from "lucide-react";
import {
  authMcpServer,
  testMcpServer,
  type AppSettings,
  type McpServerEntry,
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
import { SettingsBadge, SettingsGroupCard } from "./primitives";
import { Field } from "./controls";

/** The MCP section's props (every piece of state stays in `SettingsPage`). */
interface McpSectionProps {
  settings: AppSettings | null;
  /** The add/edit dialog draft (`null` = closed). */
  dialog: McpDraft | null;
  setDialog: (draft: McpDraft | null) => void;
  /** The dialog's Save (immediate save, the provider pattern). */
  onSave: (name: string, entry: McpServerEntry) => void;
  /** Drop one `mcpServers` entry (immediate save). */
  onRemove: (name: string) => void;
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
export interface McpDraft {
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

/** The MCP section: the desktop's own `mcpServers` list + the add/edit dialog. */
export default function McpSection({
  settings,
  dialog,
  setDialog,
  onSave,
  onRemove,
}: McpSectionProps): ReactElement | null {
  if (settings === null) return null;
  // (ADR 0019) The MCP section: the desktop's OWN `mcpServers` entries
  // (pi's `mcp.json` entries stay in their own files — not listed here).
  return (
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
              onEdit={() => setDialog(entryToDraft(name, entry))}
              onRemove={() => onRemove(name)}
            />
          ))
        )}
      </SettingsGroupCard>
      <Button
        onClick={() =>
          setDialog({
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
      {dialog !== null && (
        <McpDialog
          initial={dialog}
          onSave={onSave}
          onClose={() => setDialog(null)}
        />
      )}
    </div>
  );
}
