import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { startSession } from "../lib/tauri";
import { useSessions } from "../store/sessions";

/**
 * Modal for starting a session: pick an agent id and a working directory
 * (directory picker via @tauri-apps/plugin-dialog).
 */
export default function NewSessionDialog({ onClose }: { onClose: () => void }) {
  const addSession = useSessions((s) => s.addSession);
  const [agentId, setAgentId] = useState("fake");
  const [cwd, setCwd] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pickDirectory = async () => {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === "string") setCwd(selected);
    } catch {
      // Outside Tauri (unit tests / plain browser) the plugin is unavailable;
      // the user can still type a path.
    }
  };

  const start = async () => {
    if (cwd.trim() === "") {
      setError("Choose a working directory first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const info = await startSession(agentId.trim(), cwd.trim());
      addSession(info);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-96 rounded-lg border border-neutral-700 bg-neutral-900 p-4">
        <h2 className="text-lg font-semibold">New session</h2>
        <label className="mt-4 block text-sm text-neutral-300">
          Agent id
          <input
            value={agentId}
            onChange={(e) => setAgentId(e.target.value)}
            className="mt-1 w-full rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm outline-none focus:border-sky-600"
          />
        </label>
        <label className="mt-3 block text-sm text-neutral-300">
          Working directory
          <div className="mt-1 flex gap-2">
            <input
              value={cwd}
              onChange={(e) => setCwd(e.target.value)}
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
            disabled={busy}
            className="rounded-md bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 disabled:opacity-50"
          >
            {busy ? "Starting…" : "Start"}
          </button>
        </div>
      </div>
    </div>
  );
}
