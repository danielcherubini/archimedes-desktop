import { useState } from "react";
import { respondBridgeRequest } from "../lib/tauri";
import { useBridge } from "../store/bridge";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Button } from "./ui/button";

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
    <Dialog open onOpenChange={() => {}}>
      <DialogContent showCloseButton={false} className="max-w-md">
        <DialogHeader>
          <DialogTitle>Run this command with sudo?</DialogTitle>
        </DialogHeader>
        <pre className="overflow-x-auto rounded-md bg-surface p-2 font-mono text-ui-sm">
          {params.command ?? ""}
        </pre>
        {params.reason && (
          <p className="text-ui-sm text-foreground-subtle">{params.reason}</p>
        )}
        {error && <p className="text-ui-sm text-destructive">{error}</p>}
        <DialogFooter>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => void respond(false)}
          >
            Cancel
          </Button>
          <Button disabled={busy} onClick={() => void respond(true)}>
            Run
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
