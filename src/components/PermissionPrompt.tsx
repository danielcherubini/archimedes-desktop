import { useState } from "react";
import { respondPermission, type PermissionOutcome } from "../lib/tauri";
import { usePermissions } from "../store/permissions";
import { Button } from "./ui/button";

/**
 * Inline card shown when a `permission-request` event arrives for the active
 * session (the green confirmation treatment). The header strip carries the
 * tool name — there is NO tool detail or diff preview to render (`{
 * requestId, toolTitle, options }` is all the store holds). The footer has
 * one `button` per agent-defined option (first = primary, the rest =
 * outline) + a Cancel. Buttons answer the agent via the
 * `respond_permission` command; the prompt is removed locally on success
 * (and auto-dismissed on session-closed by the store).
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
    <div className="overflow-hidden rounded-xl border border-border">
      <div className="bg-interaction-confirmation-surface px-3 py-2">
        <p className="text-ui-base font-medium text-interaction-confirmation-foreground">
          {prompt.toolTitle}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2 p-3">
        {prompt.options.map((option, i) => (
          <Button
            key={option.optionId}
            variant={i === 0 ? "default" : "outline"}
            size="sm"
            disabled={busy}
            onClick={() => respond({ selected: { option_id: option.optionId } })}
          >
            {option.name}
          </Button>
        ))}
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={() => respond("cancelled")}
        >
          Cancel
        </Button>
      </div>
      {error && (
        <p className="px-3 pb-2 text-ui-sm text-destructive">{error}</p>
      )}
    </div>
  );
}
