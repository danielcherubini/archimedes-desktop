import { useState } from "react";
import { closeSession } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import NewSessionDialog from "./NewSessionDialog";

export default function SessionList() {
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const openSession = useSessions((s) => s.openSession);
  const deleteSession = useSessions((s) => s.deleteSession);
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
      <div className="flex-1 overflow-y-auto">
        {sessions.length === 0 && historySessions.length === 0 && (
          <p className="p-3 text-sm text-neutral-500">No sessions yet.</p>
        )}
        {sessions.length > 0 && (
          <>
            <p className="px-3 pt-2 text-[11px] font-semibold uppercase tracking-wide text-neutral-600">
              Live
            </p>
            <ul>
              {sessions.map((session) => (
                <SessionItem
                  key={session.sessionId}
                  session={session}
                  active={activeSessionId === session.sessionId}
                  onOpen={() => openSession(session.sessionId)}
                  onClose={() => void closeSession(session.sessionId)}
                />
              ))}
            </ul>
          </>
        )}
        {historySessions.length > 0 && (
          <>
            <p className="px-3 pt-2 text-[11px] font-semibold uppercase tracking-wide text-neutral-600">
              History
            </p>
            <ul>
              {historySessions.map((session) => (
                <SessionItem
                  key={session.sessionId}
                  session={session}
                  active={activeSessionId === session.sessionId}
                  onOpen={() => openSession(session.sessionId)}
                  onClose={() => void deleteSession(session.sessionId)}
                />
              ))}
            </ul>
          </>
        )}
      </div>
      {dialogOpen && <NewSessionDialog onClose={() => setDialogOpen(false)} />}
    </aside>
  );
}

function SessionItem({
  session,
  active,
  onOpen,
  onClose,
}: {
  session: { sessionId: string; agentId: string; cwd: string };
  active: boolean;
  onOpen: () => void;
  onClose: () => void;
}) {
  return (
    <li>
      <div
        role="button"
        tabIndex={0}
        onClick={onOpen}
        onKeyDown={(e) => {
          if (e.key === "Enter") onOpen();
        }}
        className={`group flex cursor-pointer items-start gap-2 px-3 py-2.5 ${
          active ? "bg-neutral-800" : "hover:bg-neutral-900"
        }`}
      >
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{session.agentId}</p>
          <p className="truncate text-xs text-neutral-500">{session.cwd}</p>
        </div>
        <button
          type="button"
          title="Close / delete session"
          aria-label={`Close session ${session.sessionId}`}
          onClick={(e) => {
            e.stopPropagation();
            onClose();
          }}
          className="hidden rounded p-0.5 text-neutral-400 hover:text-red-400 group-hover:block"
        >
          ✕
        </button>
      </div>
    </li>
  );
}
