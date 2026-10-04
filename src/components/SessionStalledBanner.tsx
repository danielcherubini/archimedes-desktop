import { useState } from "react";
import { X } from "lucide-react";
import { openPath } from "@tauri-apps/plugin-opener";
import { useSessions } from "../store/sessions";
import { Button } from "./ui/button";

/**
 * The stalled-session banner (ADR 0025 Task 6 — the user-visible half of
 * the Worker architecture): shown when the session's state is `stalled`
 * (a Worker crash — the `session-stalled` event set the store's
 * `stalled` entry). The spec's §4 wording: "Session stopped
 * unexpectedly at <time> — last turn incomplete."
 *
 * - `[Resume]` calls the store's `resumeSession` (the `resume_session`
 *   command — the Supervisor re-spawns the Worker; a success clears the
 *   `stalled` entry, which hides the banner).
 * - The secondary line (a clickable file path — the `tauri-plugin-opener`
 *   `openPath`) shows the crash log when `crashLog` is non-null.
 * - Dismissible (a LOCAL `useState` — the session stays stalled in the
 *   store; the banner reappears on re-open, when the component remounts
 *   with a fresh dismiss state).
 */
export default function SessionStalledBanner({
  sessionId,
}: {
  sessionId: string;
}) {
  const stalled = useSessions((s) => s.stalled[sessionId]);
  const resumeSession = useSessions((s) => s.resumeSession);
  const [dismissed, setDismissed] = useState(false);
  if (!stalled || dismissed) return null;
  return (
    <div
      role="alert"
      data-testid="stalled-banner"
      className="flex items-start gap-3 rounded-lg border border-warning bg-warning/10 px-3 py-2"
    >
      <div className="min-w-0 flex-1">
        <p className="text-ui-base text-warning">
          Session stopped unexpectedly at {new Date(stalled.at).toLocaleString()} — last turn incomplete.
        </p>
        {stalled.crashLog && (
          <button
            type="button"
            data-testid="stalled-crash-log"
            onClick={() => void openPath(stalled.crashLog!)}
            className="mt-0.5 block max-w-full truncate font-mono text-ui-sm text-foreground-subtle underline-offset-2 hover:underline"
            title={stalled.crashLog}
          >
            {stalled.crashLog}
          </button>
        )}
      </div>
      <Button
        variant="warning"
        size="sm"
        onClick={() => void resumeSession(sessionId)}
      >
        Resume
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Dismiss stalled banner"
        onClick={() => setDismissed(true)}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}
