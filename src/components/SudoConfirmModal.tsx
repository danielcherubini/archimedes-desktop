import { useState } from "react";
import { respondBridgeRequest } from "../lib/tauri";
import { useBridge } from "../store/bridge";

/**
 * Modal for a bridge `confirm` request (the `sudo_exec` confirm gate):
 * shows the `command` + `reason` verbatim with Run/Cancel. Run →
 * `respondBridgeRequest(…, { confirmed: true })`; Cancel →
 * `{ confirmed: false }`; then `removeRequest` (the request is settled
 * and removed outright — there is no `markAnswered` step).
 */
export default function SudoConfirmModal({
  sessionId,
  requestId,
}: {
  sessionId: string;
  requestId: string;
}) {
  const request = useBridge((state) =>
    (state.requests[sessionId] ?? []).find((r) => r.requestId === requestId),
  );
  const removeRequest = useBridge((state) => state.removeRequest);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!request || request.method !== "confirm") return null;
  const params = request.params as { command?: string; reason?: string };

  const respond = async (confirmed: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await respondBridgeRequest(sessionId, requestId, { confirmed });
      removeRequest(sessionId, requestId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70">
      <div className="w-full max-w-md rounded-md border border-amber-600/50 bg-neutral-900 p-4">
        <p className="text-sm font-medium text-amber-200">
          Run this command with sudo?
        </p>
        <pre className="mt-2 overflow-x-auto rounded bg-neutral-950 p-2 text-xs text-neutral-300">
          {params.command ?? ""}
        </pre>
        {params.reason && (
          <p className="mt-2 text-xs text-neutral-400">{params.reason}</p>
        )}
        {error && <p className="mt-2 text-xs text-red-400">{error}</p>}
        <div className="mt-3 flex justify-end gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => void respond(false)}
            className="rounded bg-neutral-700 px-3 py-1 text-sm text-neutral-200 hover:bg-neutral-600 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void respond(true)}
            className="rounded bg-amber-600 px-3 py-1 text-sm text-black hover:bg-amber-500 disabled:opacity-50"
          >
            Run
          </button>
        </div>
      </div>
    </div>
  );
}
