import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { ArrowUp, FolderIcon, MoreHorizontalIcon, PanelRightIcon } from "lucide-react";
import { closeSession, sendPrompt } from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { useSessions, spaceViewFor, type SpaceView } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useBridge } from "../store/bridge";
import { useStartNewConversation } from "../hooks/useStartNewConversation";
import { useSpinQuip } from "../hooks/useSpinQuip";
import {
  getSidePaneCollapsed,
  setSidePaneCollapsed,
  subscribeSidePane,
} from "../lib/sidePaneState";
import { BrailleLoader } from "./ui/braille-loader";
import { Button } from "./ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import MessageBubble from "./MessageBubble";
import PermissionPrompt from "./PermissionPrompt";
import AskQuestionCard from "./AskQuestionCard";
import FileSummaryCard from "./FileSummaryCard";
import SudoConfirmModal from "./SudoConfirmModal";
import SudoPasswordModal from "./SudoPasswordModal";

export default function ChatStream() {
  const activeSessionId = useSessions((s) => s.activeSessionId);
  // Selectors must return stable references (no fresh `[]` fallbacks inside
  // the selector) or Zustand re-renders forever.
  const messages =
    useSessions((s) =>
      s.activeSessionId ? s.messages[s.activeSessionId] : undefined,
    ) ?? [];
  const liveSession = useSessions((s) =>
    s.activeSessionId
      ? s.sessions.find((x) => x.sessionId === s.activeSessionId)
      : undefined,
  );
  const historySession = useSessions((s) =>
    s.activeSessionId
      ? s.historySessions.find(
          (x) => x.sessionId === s.activeSessionId,
        )
      : undefined,
  );
  const spaces = useSessions((s) => s.spaces);
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const closeReasons = useSessions((s) => s.closeReasons);
  const openSession = useSessions((s) => s.openSession);
  const inTurn = useSessions((s) => (s.activeSessionId ? !!s.inTurn[s.activeSessionId] : false));
  const stopReason = useSessions((s) => (s.activeSessionId ? s.stopReasons[s.activeSessionId] : undefined));
  const prompts =
    usePermissions((s) =>
      activeSessionId ? s.prompts[activeSessionId] : undefined,
    ) ?? [];
  // Bridge surfaces for the ACTIVE session (B3): the store is keyed by the
  // ACP session id, which matches `activeSessionId`.
  const bridgeRequests =
    useBridge((s) =>
      activeSessionId ? s.requests[activeSessionId] : undefined,
    ) ?? [];
  const askRequests = bridgeRequests.filter((r) => r.method === "ask");
  const confirmRequests = bridgeRequests.filter((r) => r.method === "confirm");
  const passwordRequests = bridgeRequests.filter(
    (r) => r.method === "password",
  );
  // An `ask` request is ANCHORED when a `tool-call` message with its
  // `toolCallId` is in the stream (correlated by `(source, toolCallId)` —
  // only a `main` ask can anchor; the desktop has no ACP `tool_call` frame
  // for child tool calls). Anchored requests render IN PLACE of the
  // `ToolCallCard`; the rest stack in the stream (arrival order).
  const anchoredAskRequestIds = new Set<string>();
  for (const r of askRequests) {
    if (
      r.source === "main" &&
      r.toolCallId &&
      messages.some((m) => m.kind === "tool-call" && m.id === r.toolCallId)
    ) {
      anchoredAskRequestIds.add(r.requestId);
    }
  }
  const stackedAskRequests = askRequests.filter(
    (r) => !anchoredAskRequestIds.has(r.requestId),
  );
  const addUserMessage = useSessions((s) => s.addUserMessage);
  const beginTurn = useSessions((s) => s.beginTurn);
  const turnCompleted = useSessions((s) => s.turnCompleted);
  const resumeSession = useSessions((s) => s.resumeSession);

  // The bridge agent state for the active session — the working indicator
  // uses the `agentState` entry when present, else the `inTurn` fallback
  // (a bridge agent that hasn't pushed yet — its turn IS in flight).
  const agentState = useBridge((s) =>
    activeSessionId ? s.agentState[activeSessionId] : undefined,
  );
  const workingOrInTurn =
    agentState === "working" || (agentState === undefined && inTurn);
  // Called UNCONDITIONALLY at the top of the component body (the hook
  // contains `useState`/`useEffect` — invoking it inside the `working`
  // branch would be a conditional hook call and crash React when the
  // state flips).
  const quip = useSpinQuip(workingOrInTurn);
  // The shared side-pane collapsed flag (Task 4): the header toggle reads
  // it for its `aria-pressed` and sets it on click (no events, no store).
  const sidePaneCollapsed = useSyncExternalStore(
    subscribeSidePane,
    getSidePaneCollapsed,
  );

  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);

  // Auto-grow the composer textarea: reset to `auto`, then `scrollHeight`
  // (min 2 rows via `rows={2}`, max 6 rows via `max-h` + `overflow-y-auto`).
  useEffect(() => {
    const el = composerRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [draft]);

  // A stored (non-live) session: its transcript is read-only unless the
  // agent negotiated `loadSession`, in which case it can be resumed.
  const isLive = !!liveSession;
  const isHistoryOnly = !isLive && historySession !== undefined;
  const canResume =
    isHistoryOnly && historySession?.capabilities.loadSession === true;

  // The current space's `SpaceView` for `activeSessionId`: the space that
  // owns it — by its live session OR any of its stored sessions (NOT
  // `[0]`-only: the conversation selector below lets the user open
  // `#2`+ sessions, which are stored sessions of the space too, and the
  // space bar must stay rendered while they are open). `undefined` when
  // the active session belongs to no space (e.g. a legacy pre-Spaces DB
  // row) — rendered exactly as today (no space chip, no selector).
  const views = useMemo(
    () => spaces.map((s) => spaceViewFor(s, sessions, historySessions, closeReasons)),
    [spaces, sessions, historySessions, closeReasons],
  );
  const view: SpaceView | undefined =
    activeSessionId === null
      ? undefined
      : views.find(
          (v) =>
            v.liveSessionId === activeSessionId ||
            v.storedSessionIds.includes(activeSessionId),
        );

  // `New session` in this space (the extracted hook): `agentId =
  // live ?? storedMostRecent ?? firstAgentId (registry default)`,
  // `spacePath = view.path` (the active session's `cwd` by the match
  // above; the backend canonicalizes). With the one-live cap lifted
  // (ADR 0002) a new conversation does NOT displace a live one — they
  // coexist. `view === undefined` → the hook no-ops (the `New Session`
  // sidebar button routes that case to the Open Space dialog instead).
  const { startNewConversation, error: newConversationError } =
    useStartNewConversation(view);

  // `Pause`: close the live session. It moves to `historySessions` AND —
  // because a close keeps `activeSessionId` — the pane stays on that
  // conversation showing the stored/paused banner (NOT the `No active
  // session` empty state; re-selecting another space is how you leave).
  const pause = async () => {
    if (!activeSessionId) return;
    try {
      await closeSession(activeSessionId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  // Auto-scroll to the bottom as new content streams in.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, prompts.length, stackedAskRequests.length]);

  if (!activeSessionId) {
    return (
      <main className="m-1 flex min-w-0 flex-1 items-center justify-center rounded-xl bg-background-alt">
        <p className="text-ui-base text-foreground-subtle">
          No active session — open a Space from the list on the left
        </p>
      </main>
    );
  }

  const send = async () => {
    const text = draft.trim();
    if (!text || inTurn || !isLive) return;
    setDraft("");
    setError(null);
    addUserMessage(activeSessionId, text);
    beginTurn(activeSessionId);
    try {
      const stopReason = await sendPrompt(activeSessionId, text);
      turnCompleted(activeSessionId, stopReason);
    } catch (err) {
      turnCompleted(activeSessionId, "end_turn");
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const resume = async () => {
    if (!activeSessionId || resuming) return;
    setResuming(true);
    setError(null);
    try {
      await resumeSession(activeSessionId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setResuming(false);
    }
  };

  // Conversation selector options for the space: the live session first
  // (label `Live`), then stored sessions in order (`#1`, `#2`, …); when
  // the live session is absent the first stored gets the label `Latest`.
  const options: Array<{ id: string; label: string }> = [];
  if (view && view.liveSessionId !== null) {
    options.push({ id: view.liveSessionId, label: "Live" });
  }
  if (view) {
    view.storedSessionIds.forEach((id, i) => {
      options.push({ id, label: view.liveSessionId === null && i === 0 ? "Latest" : `#${i + 1}` });
    });
  }

  const spaceTitle = view
    ? view.title !== ""
      ? view.title
      : basenameOfPath(liveSession?.cwd ?? historySession?.cwd ?? "")
    : undefined;

  // The session title: the first `user` message truncated to ~80 chars
  // (the same derivation as the sidebar rows), else the Space name.
  const firstUserText = (() => {
    const m = messages.find((x) => x.kind === "user");
    return m && m.text !== "" ? m.text : undefined;
  })();
  const title =
    firstUserText !== undefined
      ? firstUserText.length > 80
        ? firstUserText.slice(0, 80)
        : firstUserText
      : spaceTitle ?? "";

  // The most recent turn (the messages after the last `user` message) and
  // its standalone `diff` messages — the SINGLE SOURCE OF TRUTH for the
  // file summary card: `applySessionUpdate`/`rowToMessages` emit BOTH a
  // `tool-call.diff = diffs[0]` AND standalone `diff` messages for the
  // same extracted diffs, so the `tool-call.diff` refs are deliberately
  // NOT used (summing both would count every file twice).
  const lastUserIndex = (() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].kind === "user") return i;
    }
    return -1;
  })();
  const turnDiffs =
    lastUserIndex === -1
      ? []
      : messages
          .slice(lastUserIndex + 1)
          .flatMap((m) =>
            m.kind === "diff" ? [{ path: m.path, patch: m.patch }] : [],
          );

  // A collapsed pane gives no other cue that a request is waiting —
  // the `bg-warning` dot on the toggle (the sidebar "Waiting" badge
  // covers only the row the user is looking at).
  const hasPendingRequest =
    prompts.length > 0 ||
    bridgeRequests.some(
      (r) =>
        r.method === "ask" || r.method === "confirm" || r.method === "password",
    );

  return (
    <main className="m-1 flex min-w-0 flex-1 flex-col rounded-xl bg-background-alt">
      {isHistoryOnly && (
        <div className="m-2 flex items-center justify-between gap-2 rounded-md bg-surface px-3 py-2 text-ui-sm">
          {canResume ? (
            <span className="text-foreground-subtle">
              This session is stored. Resuming reconnects it to the agent.
            </span>
          ) : (
            <span className="text-warning">
              History only — continuing starts a new session.
            </span>
          )}
          {canResume && (
            <Button size="xs" disabled={resuming} onClick={() => void resume()}>
              {resuming ? "Resuming…" : "Resume"}
            </Button>
          )}
        </div>
      )}
      <div className="flex h-12 items-center gap-2 border-b border-border/50 p-2">
        {title !== "" && (
          <span
            className="min-w-0 flex-1 truncate text-ui-base font-medium"
            style={{
              maskImage:
                "linear-gradient(to right, black calc(100% - 1.5rem), transparent)",
              WebkitMaskImage:
                "linear-gradient(to right, black calc(100% - 1.5rem), transparent)",
            }}
          >
            {title}
          </span>
        )}
        {view && (
          <span className="flex shrink-0 items-center gap-1 rounded-lg bg-surface px-2 py-0.5">
            <FolderIcon className="size-3.5" />
            <span className="text-ui-sm">{spaceTitle}</span>
          </span>
        )}
        {view && (
          <Select value={activeSessionId} onValueChange={openSession}>
            <SelectTrigger
              variant="ghost"
              size="sm"
              className="w-24"
              aria-label="Conversation"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {options.map((opt) => (
                <SelectItem key={opt.id} value={opt.id}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="Session actions">
              <MoreHorizontalIcon className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            {isLive && (
              <DropdownMenuItem onSelect={() => void pause()}>
                Pause
              </DropdownMenuItem>
            )}
            {canResume && (
              <DropdownMenuItem onSelect={() => void resume()}>
                Resume
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onSelect={() => void startNewConversation()}>
              New Session in this Space
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Toggle side pane"
          aria-pressed={!sidePaneCollapsed}
          onClick={() => setSidePaneCollapsed(!sidePaneCollapsed)}
          className="relative ml-auto"
        >
          <PanelRightIcon className="size-4" />
          {hasPendingRequest && (
            <span
              className="absolute top-0.5 right-0.5 size-1.5 rounded-full bg-warning"
              aria-hidden
            />
          )}
        </Button>
      </div>
      <div ref={scrollRef} className="flex-1 space-y-3 overflow-y-auto p-4">
        {messages.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <p className="text-ui-base text-foreground-subtlest">
              Send a prompt to start
            </p>
          </div>
        ) : (
          <>
            {messages.map((message, i) => {
              // The `AskQuestionCard` replaces the pending `ask`
              // `ToolCallCard` (correlated via `(source, toolCallId)`/
              // `requestId` — a `main` ask whose `toolCallId` matches this
              // tool-call message).
              if (message.kind === "tool-call") {
                const anchored = askRequests.find(
                  (r) => r.source === "main" && r.toolCallId === message.id,
                );
                if (anchored) {
                  return (
                    <AskQuestionCard
                      key={i}
                      sessionId={activeSessionId}
                      requestId={anchored.requestId}
                    />
                  );
                }
              }
              return <MessageBubble key={i} message={message} />;
            })}
            {prompts.map((prompt) => (
              <PermissionPrompt
                key={prompt.requestId}
                sessionId={activeSessionId}
                requestId={prompt.requestId}
              />
            ))}
            {/* Stacked bridge `ask` cards (one per pending request, arrival
                order — concurrent asks stack vertically in the stream). */}
            {stackedAskRequests.map((r) => (
              <AskQuestionCard
                key={r.requestId}
                sessionId={activeSessionId}
                requestId={r.requestId}
              />
            ))}
            {turnDiffs.length > 0 && <FileSummaryCard diffs={turnDiffs} />}
            {workingOrInTurn && (
              <div className="flex items-center gap-2">
                <BrailleLoader
                  variant="typing"
                  speed="normal"
                  fontSize={14}
                  label="Agent working"
                />
                <p className="text-ui-sm text-foreground-subtle">{quip}</p>
              </div>
            )}
            {agentState === "blocked" && (
              <p className="text-ui-sm text-foreground-subtle">
                Waiting for your input…
              </p>
            )}
            {!inTurn && stopReason && stopReason !== "end_turn" && (
              <p className="text-ui-sm text-foreground-subtlest">
                Turn ended: {stopReason}
              </p>
            )}
          </>
        )}
      </div>

      {(error ?? newConversationError) && (
        <p className="mb-2 px-3 text-ui-sm text-destructive">
          {error ?? newConversationError}
        </p>
      )}
      <div className="m-3 rounded-2xl border-input-border bg-input p-2 hover:border-input-border-hover focus-within:border-input-border-focused">
        <textarea
          ref={composerRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder={
            isLive
              ? workingOrInTurn
                ? "Agent is working…"
                : "Send a prompt…"
              : canResume
                ? "Paused — Resume to reconnect"
                : "This session is closed"
          }
          rows={2}
          disabled={!isLive || workingOrInTurn}
          className="max-h-32 resize-none overflow-y-auto bg-transparent text-ui-base outline-none placeholder:text-foreground-subtlest disabled:opacity-50"
        />
        <div className="mt-1 flex items-center justify-between">
          <span aria-hidden />
          <div className="flex items-center gap-2">
            <span className="text-ui-xs text-foreground-subtlest">
              {liveSession?.agentId ?? historySession?.agentId}
            </span>
            <Button
              size="icon"
              aria-label="Send"
              disabled={!isLive || inTurn || draft.trim() === ""}
              onClick={() => void send()}
              className="size-8 rounded-full bg-primary text-primary-foreground"
            >
              <ArrowUp className="size-4" />
            </Button>
          </div>
        </div>
      </div>
      {/* Bridge modals (rendered at the `ChatStream` root — `fixed`
          overlays, NOT inside the scroll region). */}
      {confirmRequests.map((r) => (
        <SudoConfirmModal
          key={r.requestId}
          sessionId={activeSessionId}
          requestId={r.requestId}
        />
      ))}
      {passwordRequests.map((r) => (
        <SudoPasswordModal
          key={r.requestId}
          sessionId={activeSessionId}
          requestId={r.requestId}
        />
      ))}
    </main>
  );
}
