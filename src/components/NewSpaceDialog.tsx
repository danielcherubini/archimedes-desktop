import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  listAgents,
  spaceForPath,
  startSession,
  type AgentEntryDto,
  type SpaceCheck,
} from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { useSessions } from "../store/sessions";

/**
 * Modal for starting a space: pick an agent (registry-driven dropdown,
 * first entry as default) and a working folder (directory picker via
 * @tauri-apps/plugin-dialog, validated with `space_for_path`). Starting
 * IS the space: the session starts in the folder and the space row is
 * upserted by the response's canonical `cwd` — one flow, one action
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

  // The first registry entry IS the default (on the default registry that
  // is `pi`) — replaces the broken free-text `"fake"` default.
  const effectiveAgentId = selectedAgentId || agents[0]?.id || "";

  useEffect(() => {
    listAgents()
      .then((rows) => {
        setAgents(rows);
        setAgentsError(null);
      })
      .catch((err) => {
        setAgentsError(err instanceof Error ? err.message : String(err));
      });
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
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-96 rounded-lg border border-neutral-700 bg-neutral-900 p-4">
        <h2 className="text-lg font-semibold">{title}</h2>
        <label className="mt-4 block text-sm text-neutral-300">
          Agent
          {agentsError ? (
            <input
              disabled
              value=""
              placeholder={agentsError}
              className="mt-1 w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm outline-none"
            />
          ) : (
            <select
              value={effectiveAgentId}
              onChange={(e) => setSelectedAgentId(e.target.value)}
              className="mt-1 w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm outline-none focus:border-sky-600"
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
        <label className="mt-3 block text-sm text-neutral-300">
          Folder
          <div className="mt-1 flex gap-2">
            <input
              value={cwd}
              onChange={(e) => changeCwd(e.target.value)}
              placeholder="/path/to/project"
              className="flex-1 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm outline-none focus:border-sky-600"
            />
            <button
              type="button"
              onClick={() => void pickDirectory()}
              className="rounded-md border border-neutral-600 px-3 py-1.5 text-sm hover:bg-neutral-800"
            >
              Browse…
            </button>
          </div>
          {folderError && (
            <p className="mt-1 text-xs text-red-400">{folderError}</p>
          )}
        </label>
        {error && <p className="mt-3 text-xs text-red-400">{error}</p>}
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-neutral-600 px-3 py-1.5 text-sm hover:bg-neutral-800"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => void start()}
            disabled={
              busy ||
              effectiveAgentId === "" ||
              cwd.trim() === "" ||
              folderError !== null
            }
            className="rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:opacity-50"
          >
            {busy ? "Starting…" : "Start"}
          </button>
        </div>
      </div>
    </div>
  );
}
