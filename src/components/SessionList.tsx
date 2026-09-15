import { useState } from "react";
import { closeSession } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import NewSessionDialog from "./NewSessionDialog";

export default function SessionList() {
  const sessions = useSessions((s) => s.sessions);
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const setActiveSession = useSessions((s) => s.setActiveSession);
  const [dialogOpen, setDialogOpen] = useState(false);

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-neutral-800">
      <div className="flex items-center justify-between border-b border-neutral-800 p-3">
        <h1 className="text-sm font-semibold uppercase tracking-wide text-neutral-400">
          Sessions
        </h1>
        <button
          type="button"
          onClick={() => setDialogOpen(true)}
          className="rounded-md bg-sky-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-500"
        >
          + New session
        </button>
      </div>
      <ul className="flex-1 overflow-y-auto">
        {sessions.length === 0 && (
          <li className="p-3 text-sm text-neutral-500">No sessions yet.</li>
        )}
        {sessions.map((session) => (
          <li key={session.sessionId}>
            <div
              role="button"
              tabIndex={0}
              onClick={() => setActiveSession(session.sessionId)}
              onKeyDown={(e) => {
                if (e.key === "Enter") setActiveSession(session.sessionId);
              }}
              className={`group flex cursor-pointer items-start gap-2 px-3 py-2.5 ${
                activeSessionId === session.sessionId
                  ? "bg-neutral-800"
                  : "hover:bg-neutral-900"
              }`}
            >
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{session.agentId}</p>
                <p className="truncate text-xs text-neutral-500">{session.cwd}</p>
              </div>
              <button
                type="button"
                title="Close session"
                aria-label={`Close session ${session.sessionId}`}
                onClick={(e) => {
                  e.stopPropagation();
                  void closeSession(session.sessionId);
                }}
                className="hidden rounded p-0.5 text-neutral-400 hover:text-red-400 group-hover:block"
              >
                ✕
              </button>
            </div>
          </li>
        ))}
      </ul>
      {dialogOpen && <NewSessionDialog onClose={() => setDialogOpen(false)} />}
    </aside>
  );
}
