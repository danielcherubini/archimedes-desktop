import { useState, type ChangeEvent } from "react";
import { respondBridgeRequest } from "../lib/tauri";
import { useBridge } from "../store/bridge";

/**
 * Modal for a bridge `password` request (the `sudo_exec` credential gate):
 * displays the `command` + `reason` **verbatim** (so even a direct
 * `password` call — which skips the confirm gate — shows the user what the
 * password is for) plus a masked `•` field (the TUI `maskLine` behavior:
 * one `•` per char, the raw value never rendered). Enter confirms / Esc
 * cancels / Backspace deletes. Confirm → `respondBridgeRequest(…, {
 * password })`; Esc/empty → `{ password: "" }` (cancel); then
 * `removeRequest` (the password lives only in this modal's `useState` —
 * it is never stored in the bridge store).
 */
export default function SudoPasswordModal({
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
  // The raw value lives in state only — it is NEVER rendered (the input
  // shows the mask: one `•` per char) and never stored in the bridge
  // store.
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!request || request.method !== "password") return null;
  const params = request.params as { command?: string; reason?: string };
  const masked = "•".repeat(secret.length);

  const respond = async (cancelled: boolean) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    const password = cancelled ? "" : secret;
    try {
      await respondBridgeRequest(sessionId, requestId, { password });
      // The password was in `useState` only — removing the request
      // settles the card; it never persisted in store state.
      removeRequest(sessionId, requestId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  // The controlled-mask trick: the input's DOM value is the mask, so a
  // change is either the previous mask + typed chars (append) or the mask
  // minus its tail (Backspace). The raw value is derived, never rendered.
  const onChange = (e: ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    if (value.startsWith(masked)) {
      setSecret(secret + value.slice(masked.length));
    } else if (masked.startsWith(value)) {
      setSecret(secret.slice(0, value.length));
    }
    // Any other change (replacement / selection) is ignored.
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70">
      <div className="w-full max-w-md rounded-md border border-amber-600/50 bg-neutral-900 p-4">
        <p className="text-sm font-medium text-amber-200">
          Sudo password required
        </p>
        <pre className="mt-2 overflow-x-auto rounded bg-neutral-950 p-2 text-xs text-neutral-300">
          {params.command ?? ""}
        </pre>
        {params.reason && (
          <p className="mt-2 text-xs text-neutral-400">{params.reason}</p>
        )}
        <input
          autoFocus
          value={masked}
          onChange={onChange}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void respond(false);
            } else if (e.key === "Escape") {
              e.preventDefault();
              void respond(true);
            }
          }}
          placeholder="••••••••"
          aria-label="Password"
          className="mt-3 w-full rounded border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm tracking-widest outline-none focus:border-amber-500"
        />
        <p className="mt-1 text-xs text-neutral-500">
          Enter confirm · Esc cancel · Backspace deletes the last character
        </p>
        {error && <p className="mt-2 text-xs text-red-400">{error}</p>}
        <div className="mt-3 flex justify-end gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => void respond(true)}
            className="rounded bg-neutral-700 px-3 py-1 text-sm text-neutral-200 hover:bg-neutral-600 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void respond(false)}
            className="rounded bg-amber-600 px-3 py-1 text-sm text-black hover:bg-amber-500 disabled:opacity-50"
          >
            Confirm
          </button>
        </div>
      </div>
    </div>
  );
}
