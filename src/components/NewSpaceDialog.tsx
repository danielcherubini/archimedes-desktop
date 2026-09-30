import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  getSettings,
  listAgents,
  spaceForPath,
  startSession,
  type AgentEntryDto,
  type AppSettings,
  type SpaceCheck,
} from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { useSessions } from "../store/sessions";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

/**
 * Modal for starting a space: pick an agent (registry-driven dropdown,
 * default = an explicit selection > `settings.defaultAgent` (when in the
 * registry) > the first registry entry) and a working folder (directory
 * picker via @tauri-apps/plugin-dialog, validated with `space_for_path`).
 * Starting IS the space: the session starts in the folder and the space
 * row is upserted by the response's canonical `cwd` — one flow, one action
 * (no empty spaces: a space is born when a conversation starts in it).
 */
export default function NewSpaceDialog({ onClose }: { onClose: () => void }) {
  const addSession = useSessions((s) => s.addSession);
  const addSpace = useSessions((s) => s.addSpace);
  const [agents, setAgents] = useState<AgentEntryDto[]>([]);
  const [agentsError, setAgentsError] = useState<string | null>(null);
  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [cwd, setCwd] = useState("");
  const [check, setCheck] = useState<SpaceCheck | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The settings (read ONCE on mount — the dialog is modal): the
  // default-agent precedence source. A `getSettings` failure → `null` (a
  // `.catch` — `getSettings` rejects in jsdom without the Tauri internals,
  // and an unhandled rejection would fail the vitest run).
  const [settings, setSettings] = useState<AppSettings | null>(null);

  // The default-agent precedence: an explicit selection > the settings'
  // `defaultAgent` (only when it is in the registry — an unknown id, the
  // agent was removed from `agents.json`, falls back) > `agents[0]` (on
  // the default registry that is `pi` — the built-in `archimedes` entry
  // is appended, never prepended).
  const effectiveAgentId =
    selectedAgentId ||
    (settings?.defaultAgent &&
      agents.some((a) => a.id === settings.defaultAgent)
      ? settings.defaultAgent
      : null) ||
    agents[0]?.id ||
    "";

  useEffect(() => {
    listAgents()
      .then((rows) => {
        setAgents(rows);
        setAgentsError(null);
      })
      .catch((err) => {
        setAgentsError(err instanceof Error ? err.message : String(err));
      });
    getSettings()
      .then(setSettings)
      .catch(() => null);
  }, []);

  /**
   * Edit OR Browse pick: a non-empty cwd is validated with `space_for_path`
   * (canonicalize + "is a space row already there"). Tauri rejects
   * `Err(String)` commands with a PLAIN string, so `e.message` alone would
   * be `undefined` — the `String(e)` fallback keeps the message visible
   * (same idiom as the Start error below). An empty cwd clears both.
   */
  const changeCwd = (next: string) => {
    setCwd(next);
    if (next === "") {
      setCheck(null);
      setFolderError(null);
      return;
    }
    void (async () => {
      try {
        setCheck(await spaceForPath(next));
        setFolderError(null);
      } catch (e) {
        setCheck(null);
        setFolderError(e instanceof Error ? e.message : String(e));
      }
    })();
  };

  const pickDirectory = async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === "string") changeCwd(selected);
    } catch {
      // Outside Tauri (unit tests / plain browser) the plugin is unavailable;
      // the user can still type a path.
    }
  };

  // The dynamic title's condition is the `spaceForPath(cwd)` check's
  // `isSpace` (the folder is an EXISTING space), NOT "cwd is set": a
  // fresh (unvalidated) folder renders the literal "New space".
  const existingSpace = check?.isSpace;
  const titleBase = basenameOfPath(cwd);
  const title = existingSpace
    ? `New conversation in ${titleBase !== "" ? titleBase : cwd}`
    : "New space";

  const start = async () => {
    if (effectiveAgentId === "" || cwd.trim() === "" || folderError !== null)
      return;
    setBusy(true);
    setError(null);
    try {
      const info = await startSession(effectiveAgentId, cwd);
      // `info.cwd` is the source of truth: the backend canonicalized it
      // (Task 3). The view switches to this session automatically (the
      // store's `addSession` always selects what was just started).
      addSession(info);
      addSpace(info.cwd);
      onClose();
    } catch (err) {
      // Keep the dialog open; `space_for_path` / `start_session` errors
      // (e.g. `no such folder`, `spawning failed`) reject as String.
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            Start a session in a folder — starting IS the space.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <label className="flex flex-col gap-1 text-ui-base">
            Agent
            {agentsError ? (
              <Input disabled value="" placeholder={agentsError} />
            ) : (
              <select
                value={effectiveAgentId}
                onChange={(e) => setSelectedAgentId(e.target.value)}
                className="h-7 w-full rounded-md border border-input-border bg-input px-2 text-ui-base text-foreground outline-none"
              >
                {selectedAgentId === "" && (
                  <option value="">Select an agent…</option>
                )}
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.name}
                  </option>
                ))}
              </select>
            )}
          </label>
          <label className="flex flex-col gap-1 text-ui-base">
            Folder
            <div className="flex gap-2">
              <Input
                value={cwd}
                onChange={(e) => changeCwd(e.target.value)}
                placeholder="/path/to/project"
                className="flex-1"
              />
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void pickDirectory()}
              >
                Browse…
              </Button>
            </div>
            {folderError && (
              <p className="text-ui-sm text-destructive">{folderError}</p>
            )}
          </label>
        </div>
        {error && <p className="text-ui-sm text-destructive">{error}</p>}
        <DialogFooter>
          <Button variant="destructive" onClick={onClose}>
            Cancel
          </Button>
          <Button
            onClick={() => void start()}
            disabled={
              busy ||
              effectiveAgentId === "" ||
              cwd.trim() === "" ||
              folderError !== null
            }
          >
            {busy ? "Starting…" : "Start"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
