import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronDownIcon,
  ChevronRightIcon,
  FolderIcon,
  FolderOpenIcon,
  MessageCirclePlusIcon,
  PauseIcon,
  PlusIcon,
} from "lucide-react";
import { closeSession } from "../lib/tauri";
import {
  useSessions,
  spaceViewFor,
  type Message,
  type SpaceView,
} from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useBridge } from "../store/bridge";
import { useStartNewConversation } from "../hooks/useStartNewConversation";
import NewSpaceDialog from "./NewSpaceDialog";
import { Spinner } from "./ui/spinner";
import { Kbd } from "./ui/kbd";

/**
 * The relative time of a session's last activity, derived ONLY from the
 * latest message's `at` when the transcript is loaded this boot
 * (`useSessions.messages[sessionId]`): `<60s` → `now`, `<60m` → `Nm`,
 * else `Nh`. `SessionInfo` has NO `createdAt` field (the store's ordering
 * note says so explicitly), so a stored session not opened this boot has
 * no loaded messages → `null` (the row renders an empty slot).
 */
function relativeTimeFor(
  messages: Message[] | undefined,
  now: number = Date.now(),
): string | null {
  const latest = messages?.[messages.length - 1];
  if (!latest) return null;
  const diff = now - latest.at;
  if (diff < 60_000) return "now";
  if (diff < 60 * 60_000) return `${Math.floor(diff / 60_000)}m`;
  return `${Math.floor(diff / (60 * 60_000))}h`;
}

/**
 * A session row's title: the first user message truncated to ~80 chars
 * (the same derivation as the center header), else the space's name.
 */
function titleFor(messages: Message[] | undefined, spaceName: string): string {
  const firstUser = messages?.find((m) => m.kind === "user");
  if (!firstUser || firstUser.text === "") return spaceName;
  return firstUser.text.length > 80 ? firstUser.text.slice(0, 80) : firstUser.text;
}

export default function SpacesList() {
  const spaces = useSessions((s) => s.spaces);
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const closeReasons = useSessions((s) => s.closeReasons);
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const openSession = useSessions((s) => s.openSession);
  const [dialogOpen, setDialogOpen] = useState(false);

  // Keep the `spaces` list order (`lastOpenedAt` desc from `list_spaces`):
  // groups follow the server's recent-first order.
  const views = useMemo(
    () => spaces.map((s) => spaceViewFor(s, sessions, historySessions, closeReasons)),
    [spaces, sessions, historySessions, closeReasons],
  );
  // The view owning `activeSessionId` (the same `view` `ChatStream`
  // computes): the active session's Space, matched by live or stored
  // membership. `undefined` when nothing is active.
  const activeView =
    activeSessionId === null
      ? undefined
      : views.find(
          (v) =>
            v.liveSessionId === activeSessionId ||
            v.storedSessionIds.includes(activeSessionId),
        );
  const newSession = useStartNewConversation(activeView);

  // `handleOpenSpace` is stable forever; `handleNewSession` is stable only
  // while `activeSessionId` and `newSession` are — but `useStartNewConversation`
  // returns a FRESH object every render, so the keydown effect below
  // re-subscribes on EVERY render. That is one cheap listener swap per
  // render, and the `e.target` guard makes the churn harmless.
  const handleNewSession = useCallback(() => {
    // No active session (no view): the button opens the Open Space dialog
    // instead (the hook itself no-ops on an undefined view).
    if (activeSessionId === null) {
      setDialogOpen(true);
      return;
    }
    void newSession.startNewConversation();
  }, [activeSessionId, newSession]);
  const handleOpenSpace = useCallback(() => setDialogOpen(true), []);

  // ⌘N / Ctrl+N → New Session, ⌘O / Ctrl+O → Open Space. Ignored while the
  // key is pressed in an input, textarea, editable element, or dialog (e.g.
  // the composer or NewSpaceDialog's fields) so native shortcuts keep
  // working there.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!e.metaKey && !e.ctrlKey) return;
      if (
        e.target instanceof HTMLElement &&
        e.target.closest("input, textarea, [contenteditable='true'], [role='dialog']")
      ) {
        return;
      }
      const key = e.key.toLowerCase();
      if (key === "n") {
        e.preventDefault();
        handleNewSession();
      } else if (key === "o") {
        e.preventDefault();
        handleOpenSpace();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handleNewSession, handleOpenSpace]);

  return (
    <aside className="flex w-[260px] shrink-0 flex-col bg-sidebar">
      <div className="flex flex-col gap-2 border-b border-border/50 p-3">
        <button
          type="button"
          onClick={handleNewSession}
          className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-ui-base hover:bg-surface-hover"
        >
          <MessageCirclePlusIcon className="size-4" />
          <span className="flex-1 text-left">New Session</span>
          <Kbd className="text-foreground-subtlest">⌘N</Kbd>
        </button>
        <button
          type="button"
          onClick={handleOpenSpace}
          className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-ui-base hover:bg-surface-hover"
        >
          <FolderOpenIcon className="size-4" />
          <span className="flex-1 text-left">Open Space</span>
          <Kbd className="text-foreground-subtlest">⌘O</Kbd>
        </button>
      </div>
      <p className="px-2.5 py-2 text-ui-base text-foreground-subtlest">
        Sessions
      </p>
      <div className="flex-1 overflow-y-auto">
        {views.length === 0 && (
          <p className="p-3 text-ui-sm text-foreground-subtlest">
            No spaces yet. Open a folder to get started.
          </p>
        )}
        {views.map((view) => (
          <SpaceGroup
            key={view.path}
            view={view}
            activeSessionId={activeSessionId}
            onOpen={openSession}
          />
        ))}
      </div>
      {dialogOpen && <NewSpaceDialog onClose={() => setDialogOpen(false)} />}
    </aside>
  );
}

/**
 * One collapsible Space group: folder icon + base name, a `+` hover-action
 * (new Session in this Space — the same hook, pinned to this view) and a
 * chevron toggling the group's visibility in local `useState` (default
 * open).
 */
function SpaceGroup({
  view,
  activeSessionId,
  onOpen,
}: {
  view: SpaceView;
  activeSessionId: string | null;
  onOpen: (sessionId: string) => void;
}) {
  const [open, setOpen] = useState(true);
  const newSession = useStartNewConversation(view);
  const name = view.title !== "" ? view.title : view.path;
  return (
    <div className="px-2.5 py-1">
      <div className="group flex items-center gap-1">
        <FolderIcon className="size-4 shrink-0 text-foreground-subtlest" />
        <span className="flex-1 truncate text-ui-base text-foreground-subtlest group-hover:text-foreground-subtle">
          {name}
        </span>
        <button
          type="button"
          aria-label={`New session in ${name}`}
          onClick={() => void newSession.startNewConversation()}
          className="rounded-md size-6 hover:bg-surface-hover"
        >
          <PlusIcon className="size-4" />
        </button>
        <button
          type="button"
          aria-label={open ? `Collapse ${name}` : `Expand ${name}`}
          onClick={() => setOpen(!open)}
          className="rounded-md size-6 hover:bg-surface-hover"
        >
          {open ? (
            <ChevronDownIcon className="size-4" />
          ) : (
            <ChevronRightIcon className="size-4" />
          )}
        </button>
      </div>
      {open && (
        <div className="mt-1 flex flex-col gap-0.5">
          {view.liveSessionId !== null && (
            <SessionRow
              sessionId={view.liveSessionId}
              spaceName={name}
              active={activeSessionId === view.liveSessionId}
              onOpen={onOpen}
            />
          )}
          {view.storedSessionIds.map((id) => (
            <SessionRow
              key={id}
              sessionId={id}
              spaceName={name}
              active={activeSessionId === id}
              onOpen={onOpen}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One session row: leading 16px slot (live + in-turn → the circular
 * `LoaderIcon` spinner, else an empty placeholder), the title with a
 * gradient-fade mask (applied to the text ITSELF — background-agnostic, so
 * it works over `bg-sidebar` / `bg-selected` / `bg-surface-hover` alike;
 * the text is NOT `truncate`-ellipsized, it overflows under the fade), and
 * the right slot: the relative time of last activity, the green "Waiting"
 * attention pill (a pending permission prompt OR a pending bridge
 * `ask`/`confirm`/`password` request — `password` included: a pending sudo
 * password shows no "Waiting" cue anywhere else), or — on row hover for a
 * live session — a Pause button (the existing `closeSession` path).
 */
function SessionRow({
  sessionId,
  spaceName,
  active,
  onOpen,
}: {
  sessionId: string;
  spaceName: string;
  active: boolean;
  onOpen: (sessionId: string) => void;
}) {
  const messages = useSessions((s) => s.messages[sessionId]);
  const isLive = useSessions((s) => s.sessions.some((x) => x.sessionId === sessionId));
  const inTurn = useSessions((s) => !!s.inTurn[sessionId]);
  // Selectors return stable references (no fresh `[]` fallbacks INSIDE the
  // selector) or Zustand re-renders forever.
  const prompts = usePermissions((s) => s.prompts[sessionId]) ?? [];
  const bridgeRequests = useBridge((s) => s.requests[sessionId]) ?? [];

  const waiting =
    prompts.length > 0 ||
    bridgeRequests.some(
      (r) => r.method === "ask" || r.method === "confirm" || r.method === "password",
    );
  const time = relativeTimeFor(messages);
  const title = titleFor(messages, spaceName);

  const rightSlot = waiting ? (
    <span className="rounded-full bg-success/14 px-2 text-ui-sm font-medium text-success">
      Waiting
    </span>
  ) : time !== null ? (
    <span className="text-ui-sm text-foreground-subtle">{time}</span>
  ) : (
    // Empty slot — keeps alignment when there is no loaded transcript.
    <span className="size-4" />
  );

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onOpen(sessionId)}
      onKeyDown={(e) => {
        if (e.key === "Enter") onOpen(sessionId);
      }}
      className={`group flex cursor-pointer items-center gap-2 rounded-lg pl-2.5 pr-1 py-1 ${
        active ? "bg-selected" : "hover:bg-surface-hover"
      }`}
    >
      {isLive && inTurn ? (
        <Spinner className="size-4 shrink-0 text-foreground-subtle" />
      ) : (
        <span className="size-4 shrink-0" />
      )}
      <span
        className="min-w-0 flex-1 overflow-hidden whitespace-nowrap text-ui-base text-foreground"
        style={{
          maskImage:
            "linear-gradient(to right, black calc(100% - 1.5rem), transparent)",
          WebkitMaskImage:
            "linear-gradient(to right, black calc(100% - 1.5rem), transparent)",
        }}
      >
        {title}
      </span>
      {isLive ? (
        <>
          <span className="group-hover:hidden">{rightSlot}</span>
          <button
            type="button"
            title="Pause"
            aria-label={`Pause ${title}`}
            onClick={(e) => {
              e.stopPropagation();
              void closeSession(sessionId).catch((err) => {
                // `close_session` failures are logged (this path touches no
                // error state).
                console.error("Failed to pause session:", err);
              });
            }}
            className="hidden size-6 items-center justify-center rounded-md group-hover:flex hover:bg-surface-hover"
          >
            <PauseIcon className="size-4" />
          </button>
        </>
      ) : (
        rightSlot
      )}
    </div>
  );
}
