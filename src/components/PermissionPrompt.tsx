import { useState } from "react";
import { respondPermission, type PermissionOutcome } from "../lib/tauri";
import { usePermissions } from "../store/permissions";

/**
 * Inline card shown when a `permission-request` event arrives for the active
 * session. Buttons answer the agent via the `respond_permission` command;
 * the prompt is removed locally on success (and auto-dismissed on
 * session-closed by the store).
 */
export default function PermissionPrompt({
  sessionId,
  requestId,
}: {
  sessionId: string;
  requestId: string;
}) {
  const prompt = usePermissions((state) =>
    (state.prompts[sessionId] ?? []).find((p) => p.requestId === requestId),
  );
  const removePrompt = usePermissions((state) => state.removePrompt);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!prompt) return null;

  const respond = async (outcome: PermissionOutcome) => {
    setBusy(true);
    setError(null);
    try {
      await respondPermission(sessionId, requestId, outcome);
      removePrompt(sessionId, requestId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="rounded-md border border-amber-600/50 bg-amber-950/40 px-3 py-2">
      <p className="text-sm text-amber-200">
        Agent requests permission: <strong>{prompt.toolTitle}</strong>
      </p>
      <div className="mt-2 flex flex-wrap gap-2">
        {prompt.options.map((option) => (
          <button
            key={option.optionId}
            type="button"
            disabled={busy}
            onClick={() => respond({ selected: { option_id: option.optionId } })}
            className="rounded bg-amber-600 px-3 py-1 text-sm text-black hover:bg-amber-500 disabled:opacity-50"
          >
            {option.name}
          </button>
        ))}
        <button
          type="button"
          disabled={busy}
          onClick={() => respond("cancelled")}
          className="rounded bg-neutral-700 px-3 py-1 text-sm text-neutral-200 hover:bg-neutral-600 disabled:opacity-50"
        >
          Cancel
        </button>
      </div>
      {error && <p className="mt-1 text-xs text-red-400">{error}</p>}
    </div>
  );
}
