import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ArchiveIcon,
  ArchiveXIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  FolderIcon,
  FolderOpenIcon,
  MessageCirclePlusIcon,
  PauseIcon,
  PlusIcon,
  ShieldCheckIcon,
  ShieldIcon,
  SparklesIcon,
  Trash2Icon,
} from "lucide-react";
import { closeSession, type SessionInfo } from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import {
  useSessions,
  spaceViewFor,
  type Message,
  type SpaceView,
} from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useBridge } from "../store/bridge";
import { useStartNewConversation } from "../hooks/useStartNewConversation";
import { useSkillCatalog } from "../hooks/useSkillCatalog";
import NewSpaceDialog from "./NewSpaceDialog";
import SkillsDialog from "./SkillsDialog";
import DeleteSessionDialog from "./DeleteSessionDialog";
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

/**
 * A session's Space name (the `spaceViewFor` pattern): a session's
 * `cwd` IS the Space's `path` (both canonical-path keys) — the base
 * name of the matching space, or of the `cwd` itself when NO space
 * matches (a legacy session whose cwd is no longer a Space).
 */
function spaceNameFor(cwd: string, spaces: { path: string }[]): string {
  const match = spaces.find((s) => s.path === cwd);
  return match ? basenameOfPath(match.path) : basenameOfPath(cwd);
}

export default function SpacesList() {
  const spaces = useSessions((s) => s.spaces);
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const archivedSessions = useSessions((s) => s.archivedSessions);
  const closeReasons = useSessions((s) => s.closeReasons);
  const activeSessionId = useSessions((s) => s.activeSessionId);
  const openSession = useSessions((s) => s.openSession);
  const deleteSession = useSessions((s) => s.deleteSession);
  const [dialogOpen, setDialogOpen] = useState(false);
  // The Skills modal (the left pane has NO skill list — ZCode parity: the
  // skills UI is a searchable modal, opened from the third button in the
  // row). The catalog fetch lives in `useSkillCatalog` below (the modal
  // receives the rows as props — same hook key as the composer → one
  // shared fetch).
  const [skillsOpen, setSkillsOpen] = useState(false);

  // Keep the `spaces` list order (`lastOpenedAt` desc from `list_spaces`):
  // groups follow the server's recent-first order.
  const views = useMemo(
    () =>
      spaces.map((s) =>
        spaceViewFor(s, sessions, historySessions, closeReasons, archivedSessions),
      ),
    [spaces, sessions, historySessions, closeReasons, archivedSessions],
  );
  // The view owning `activeSessionId` (the same `view` `ChatStream`
  // computes): the active session's Space, matched by live, stored, OR
  // archived membership (`archivedSessionIds` is view-membership-only —
  // an archived session is never rendered in the Space group, so a
  // space whose only sessions are archived still resolves its `view`;
  // ⌘N / New Session while an archived session is active routes to
  // THAT space, not the Open Space dialog). `undefined` when nothing is
  // active.
  const activeView =
    activeSessionId === null
      ? undefined
      : views.find(
          (v) =>
            v.liveSessionId === activeSessionId ||
            v.storedSessionIds.includes(activeSessionId) ||
            v.archivedSessionIds.includes(activeSessionId),
        );
  const newSession = useStartNewConversation(activeView);

  // The active Space's path for the skill catalog: a Session's `cwd` IS
  // the Space's folder (CONTEXT.md). Live sessions first, then stored,
  // then ARCHIVED (an archived-only active session must still resolve
  // its Space — otherwise the catalog silently degrades to user-level
  // skills); `null` when nothing is active.
  const activeSession = sessions.find((s) => s.sessionId === activeSessionId);
  const activeHistory = activeSession
    ? undefined
    : historySessions.find((s) => s.sessionId === activeSessionId) ??
      archivedSessions.find((s) => s.sessionId === activeSessionId);
  const activeSpacePath = activeSession?.cwd ?? activeHistory?.cwd ?? null;
  const skills = useSkillCatalog(activeSpacePath);

  // The session awaiting delete confirmation (the ONLY destructive
  // action in the sidebar — archive/unarchive are reversible and have
  // no confirm). `null` = no dialog. (The selector is unconditional and
  // returns stable references — `undefined` or the transcript array —
  // so Zustand does not re-render in a loop.)
  const [deleteTarget, setDeleteTarget] = useState<SessionInfo | null>(null);
  const deleteTargetMessages = useSessions(
    (s) => (deleteTarget ? s.messages[deleteTarget.sessionId] : undefined),
  );
  const deleteTargetTitle = deleteTarget
    ? titleFor(
        deleteTargetMessages,
        spaceNameFor(deleteTarget.cwd, spaces),
      )
    : "";

  // `handleOpenSpace` is stable forever; `handleNewSession` is stable only
  // while `activeSessionId` and `newSession` are — but `useStartNewConversation`
  // returns a FRESH object every render, so the keydown effect below
  // re-subscribes on EVERY render. That is one cheap listener swap per
  // render, and the `e.target` guard makes the churn harmless.
  const handleNewSession = useCallback(() => {
    // No matching Space view — either no active session, or an active
    // session that belongs to NO current Space (e.g. a legacy session
    // whose cwd is no longer a Space): open the Open Space dialog
    // instead (the hook itself no-ops on an undefined view).
    if (activeView === undefined) {
      setDialogOpen(true);
      return;
    }
    void newSession.startNewConversation();
  }, [activeView, newSession]);
  const handleOpenSpace = useCallback(() => setDialogOpen(true), []);

  // ⌘N / Ctrl+N → New Session, ⌘O / Ctrl+O → Open Space. Ignored while the
  // key is pressed in an input, textarea, editable element, or dialog (e.g.
  // the composer or NewSpaceDialog's fields) so native shortcuts keep
  // working there. The `contenteditable` match is spec-exact: per HTML,
  // `contenteditable=""` and a bare `contenteditable` BOTH mean editable,
  // so the guard matches the attribute's PRESENCE except `contenteditable="false"`.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!e.metaKey && !e.ctrlKey) return;
      if (
        e.target instanceof HTMLElement &&
        e.target.closest(
          "input, textarea, [contenteditable]:not([contenteditable='false']), [role='dialog']",
        )
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
        <button
          type="button"
          onClick={() => setSkillsOpen(true)}
          className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-ui-base hover:bg-surface-hover"
        >
          <SparklesIcon className="size-4" />
          <span className="flex-1 text-left">Skills</span>
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
        <ArchivedSection
          activeSessionId={activeSessionId}
          onOpen={openSession}
          onDelete={setDeleteTarget}
        />
      </div>
      {dialogOpen && <NewSpaceDialog onClose={() => setDialogOpen(false)} />}
      {skillsOpen && (
        <SkillsDialog skills={skills} onClose={() => setSkillsOpen(false)} />
      )}
      {deleteTarget && (
        <DeleteSessionDialog
          title={deleteTargetTitle}
          onConfirm={() => {
            // The store's `deleteSession` removes the id from ALL lists
            // (live / stored / archived + messages + activeSessionId). The
            // dialog closes on success AND on failure (a stuck-open dialog
            // is worse); a failure is logged (no unhandled rejection).
            deleteSession(deleteTarget.sessionId)
              .then(() => setDeleteTarget(null))
              .catch((err) => {
                console.error("Failed to delete session:", err);
                setDeleteTarget(null);
              });
          }}
          onClose={() => setDeleteTarget(null)}
        />
      )}
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
  const setSpaceTrusted = useSessions((s) => s.setSpaceTrusted);
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
          aria-label={view.trusted ? `Stop trusting ${name}` : `Trust ${name}`}
          title={
            view.trusted
              ? "Stop trusting this Space"
              : "Trust this Space — skip permission prompts for bash/edit/write"
          }
          onClick={() => setSpaceTrusted(view.path, !view.trusted)}
          className="rounded-md size-6 hover:bg-surface-hover"
        >
          {view.trusted ? (
            <ShieldCheckIcon className="size-4 text-success" />
          ) : (
            <ShieldIcon className="size-4 text-foreground-subtlest" />
          )}
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
 * password shows no "Waiting" cue anywhere else) — swapped on row hover
 * for a Pause button (live sessions, the existing `closeSession` path) or
 * an Archive button (stored sessions, reversible — no confirm).
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
  const archiveSession = useSessions((s) => s.archiveSession);
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
        // Stored row: the time/pill slot swaps for an Archive hover
        // button (the same pattern as the live row's Pause — archive is
        // reversible, so it needs no confirm). The slot still renders
        // when NOT hovering.
        <>
          <span className="group-hover:hidden">{rightSlot}</span>
          <button
            type="button"
            title="Archive"
            aria-label={`Archive ${title}`}
            onClick={(e) => {
              e.stopPropagation();
              void archiveSession(sessionId).catch((err) => {
                console.error("Failed to archive session:", err);
              });
            }}
            className="hidden size-6 items-center justify-center rounded-md group-hover:flex hover:bg-surface-hover"
          >
            <ArchiveIcon className="size-4" />
          </button>
        </>
      )}
    </div>
  );
}

/**
 * The flat, cross-space Archived section (ZCode's
 * `WorkspaceArchivedTasksFlatSection` model adapted to Archimedes' row
 * style): rendered AFTER the Space groups, scrolling with the list.
 * COLLAPSED by default; the header (icon + "Archived" + count + chevron)
 * toggles it. The sticky live ids are view-filtered out — a resumed
 * archived session renders in its Space group (live), not here.
 */
function ArchivedSection({
  activeSessionId,
  onOpen,
  onDelete,
}: {
  activeSessionId: string | null;
  onOpen: (sessionId: string) => void;
  onDelete: (session: SessionInfo) => void;
}) {
  const [open, setOpen] = useState(false);
  const archived = useSessions((s) => s.archivedSessions);
  const sessions = useSessions((s) => s.sessions);
  const visible = archived.filter(
    (s) => !sessions.some((l) => l.sessionId === s.sessionId),
  );
  return (
    <div className="px-2.5 py-1">
      <div
        role="button"
        tabIndex={0}
        onClick={() => setOpen(!open)}
        onKeyDown={(e) => {
          if (e.key === "Enter") setOpen(!open);
        }}
        className="group flex cursor-pointer items-center gap-1 rounded-lg px-1 hover:bg-surface-hover"
      >
        <ArchiveIcon className="size-4 shrink-0 text-foreground-subtlest" />
        <span className="flex-1 truncate text-ui-base text-foreground-subtlest group-hover:text-foreground-subtle">
          Archived
        </span>
        <span className="text-ui-sm text-foreground-subtlest">
          {visible.length}
        </span>
        {open ? (
          <ChevronDownIcon className="size-4 shrink-0 text-foreground-subtlest" />
        ) : (
          <ChevronRightIcon className="size-4 shrink-0 text-foreground-subtlest" />
        )}
      </div>
      {open && (
        <div className="mt-1 flex flex-col gap-0.5">
          {visible.length === 0 ? (
            <p className="px-1 py-1 text-ui-sm text-foreground-subtlest">
              No archived sessions.
            </p>
          ) : (
            visible.map((session) => (
              <ArchivedSessionRow
                key={session.sessionId}
                session={session}
                active={activeSessionId === session.sessionId}
                onOpen={onOpen}
                onDelete={onDelete}
              />
            ))
          )}
        </div>
      )}
    </div>
  );
}

/**
 * One archived-session row (the `SessionRow` visual pattern): 16px
 * leading slot (empty — archived sessions are NEVER live, so no
 * spinner), the title + a subtle `· {spaceName}` suffix ONLY when the
 * title is message-derived (skipped when the title IS the space name —
 * avoid "alpha · alpha"), and the relative time (empty slot when the
 * transcript was not loaded this boot). Hover: Unarchive (`ArchiveX`,
 * reversible — no confirm) + Delete (`Trash2`, the only destructive
 * action — confirm dialog). Click opens the transcript (identical to a
 * stored row — the first send resumes it).
 */
function ArchivedSessionRow({
  session,
  active,
  onOpen,
  onDelete,
}: {
  session: SessionInfo;
  active: boolean;
  onOpen: (sessionId: string) => void;
  onDelete: (session: SessionInfo) => void;
}) {
  const messages = useSessions((s) => s.messages[session.sessionId]);
  const spaces = useSessions((s) => s.spaces);
  const unarchiveSession = useSessions((s) => s.unarchiveSession);
  const spaceName = spaceNameFor(session.cwd, spaces);
  const title = titleFor(messages, spaceName);
  const time = relativeTimeFor(messages);
  const showSuffix = title !== spaceName;
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onOpen(session.sessionId)}
      onKeyDown={(e) => {
        if (e.key === "Enter") onOpen(session.sessionId);
      }}
      className={`group flex cursor-pointer items-center gap-2 rounded-lg pl-2.5 pr-1 py-1 ${
        active ? "bg-selected" : "hover:bg-surface-hover"
      }`}
    >
      <span className="size-4 shrink-0" />
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
        {showSuffix && (
          <span className="text-foreground-subtlest"> · {spaceName}</span>
        )}
      </span>
      <span className="group-hover:hidden">
        {time !== null ? (
          <span className="text-ui-sm text-foreground-subtle">{time}</span>
        ) : (
          // Empty slot — keeps alignment when there is no loaded transcript.
          <span className="size-4" />
        )}
      </span>
      <button
        type="button"
        title="Unarchive"
        aria-label={`Unarchive ${title}`}
        onClick={(e) => {
          e.stopPropagation();
          void unarchiveSession(session.sessionId).catch((err) => {
            console.error("Failed to unarchive session:", err);
          });
        }}
        className="hidden size-6 items-center justify-center rounded-md group-hover:flex hover:bg-surface-hover"
      >
        <ArchiveXIcon className="size-4" />
      </button>
      <button
        type="button"
        title="Delete"
        aria-label={`Delete ${title}`}
        onClick={(e) => {
          e.stopPropagation();
          onDelete(session);
        }}
        className="hidden size-6 items-center justify-center rounded-md group-hover:flex hover:bg-surface-hover"
      >
        <Trash2Icon className="size-4" />
      </button>
    </div>
  );
}
