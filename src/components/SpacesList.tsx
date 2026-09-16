import { useMemo, useState } from "react";
import { useSessions, spaceViewFor } from "../store/sessions";
import NewSpaceDialog from "./NewSpaceDialog";

/**
 * Hover-tooltip copy for a space row, from its recorded close reason
 * (or the live marker when its live session is in flight).
 */
function tooltipForColumn(
  live: boolean,
  agentId: string,
  lastReason?: string,
): string {
  if (live) return `Live — ${agentId}`;
  switch (lastReason) {
    case "replaced":
      return "Paused — another space started a conversation";
    case "user":
      return "Paused";
    case "agent-exited":
      return "The agent exited";
    case "error":
      return "Ended in an error";
    default:
      // No recorded reason (normal on boot) → stored.
      return "Stored";
  }
}

export default function SpacesList() {
  const spaces = useSessions((s) => s.spaces);
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const closeReasons = useSessions((s) => s.closeReasons);
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const openSession = useSessions((s) => s.openSession);
  const removeSpace = useSessions((s) => s.removeSpace);
  const [dialogOpen, setDialogOpen] = useState(false);

  // Keep the `spaces` list order (`lastOpenedAt` desc from `list_spaces`):
  // view rows follow the server's recent-first order.
  const views = useMemo(
    () => spaces.map((s) => spaceViewFor(s, sessions, historySessions, closeReasons)),
    [spaces, sessions, historySessions, closeReasons],
  );

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r border-neutral-800">
      <div className="flex items-center justify-between border-b border-neutral-800 p-3">
        <h1 className="text-sm font-semibold uppercase tracking-wide text-neutral-400">
          Spaces
        </h1>
        <button
          type="button"
          onClick={() => setDialogOpen(true)}
          className="rounded-md bg-sky-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-sky-500"
        >
          + New space
        </button>
      </div>
      <div className="flex-1 overflow-y-auto">
        {spaces.length === 0 && (
          <p className="p-3 text-sm text-neutral-500">
            No spaces yet. Open a folder to get started.
          </p>
        )}
        <ul>
          {views.map((view) => (
            <SpaceRow
              key={view.path}
              view={view}
              liveSession={sessions.find(
                (s) => s.sessionId === view.liveSessionId,
              )}
              active={
                activeSessionId === view.liveSessionId ||
                (view.liveSessionId === null &&
                  view.storedSessionIds.includes(activeSessionId ?? ""))
              }
              onOpen={() => {
                const target =
                  view.liveSessionId ?? view.storedSessionIds[0] ?? null;
                if (target) openSession(target);
                // Guard: no session at all → the space was just created and
                // `start` hasn't completed yet; do nothing (transient).
              }}
              onRemove={() => void removeSpace(view.path)}
            />
          ))}
        </ul>
      </div>
      {dialogOpen && <NewSpaceDialog onClose={() => setDialogOpen(false)} />}
    </aside>
  );
}

function SpaceRow({
  view,
  liveSession,
  active,
  onOpen,
  onRemove,
}: {
  view: {
    path: string;
    title: string;
    liveSessionId: string | null;
    storedSessionIds: string[];
    lastReason?: string;
  };
  liveSession: { sessionId: string; agentId: string; cwd: string } | undefined;
  active: boolean;
  onOpen: () => void;
  onRemove: () => void;
}) {
  const live = view.liveSessionId !== null;
  const dotTitle = tooltipForColumn(
    live,
    liveSession?.agentId ?? "",
    view.lastReason,
  );
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
      <span
        title={dotTitle}
        className={`text-sm leading-5 ${live ? "text-emerald-400" : "text-neutral-600"}`}
      >
        {live ? "●" : "○"}
      </span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">
          {view.title !== "" ? view.title : view.path}
        </p>
        <p className="truncate text-xs text-neutral-500" title={view.path}>
          {view.path}
        </p>
      </div>
      {view.liveSessionId === null && (
        <button
          type="button"
          title="Forget this space (conversations stay stored)"
          aria-label={`Forget space ${view.path}`}
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          className="hidden rounded p-0.5 text-neutral-400 hover:text-red-400 group-hover:block"
        >
          ✕
        </button>
      )}
      </div>
    </li>
  );
}
