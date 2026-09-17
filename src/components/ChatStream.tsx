import { useEffect, useMemo, useRef, useState } from "react";
import { closeSession, listAgents, sendPrompt, startSession, type AgentEntryDto } from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { useSessions, spaceViewFor, type SpaceView } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useBridge } from "../store/bridge";
import MessageBubble from "./MessageBubble";
import PermissionPrompt from "./PermissionPrompt";
import AskQuestionCard from "./AskQuestionCard";
import SudoConfirmModal from "./SudoConfirmModal";
import SudoPasswordModal from "./SudoPasswordModal";
import TodoBoardPanel from "./TodoBoardPanel";

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
  const addSession = useSessions((s) => s.addSession);
  const addSpace = useSessions((s) => s.addSpace);
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

  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  // Registry for the "New conversation" `agentId` fallback (fetched
  // `useEffect`-style like the dialog; `firstAgentId` is the default).
  const [agents, setAgents] = useState<AgentEntryDto[] | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    listAgents().then(setAgents).catch(() => setAgents([]));
  }, []);

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
  // row) — rendered exactly as today (no space bar, no selector).
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

  // `New conversation` in this space: `agentId = live ?? storedMostRecent
  // ?? firstAgentId (registry default)`, `spacePath = view.path` (the
  // active session's `cwd` by the match above; the backend canonicalizes
  // and its one-live policy closes a displaced session; its
  // `session-closed (replaced)` event takes it to stored automatically).
  const storedMostRecent =
    view !== undefined
      ? historySessions.find(
          (s) => s.sessionId === view.storedSessionIds[0],
        )
      : undefined;
  const firstAgentId = agents?.[0]?.id ?? "";
  const newConversationAgentId =
    liveSession?.agentId ?? storedMostRecent?.agentId ?? firstAgentId;

  const startNewConversation = async () => {
    if (!view || activeSessionId === null || newConversationAgentId === "")
      return;
    setError(null);
    try {
      const info = await startSession(newConversationAgentId, view.path);
      // `addSession` switches the view to the new session automatically
      // (Task 5); the displaced conversation surfaces as stored via its
      // `replaced` close event. No manual view-switch call.
      addSession(info);
      addSpace(info.cwd);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

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
      // Both return paths wrap in a flex row for the `TodoBoardPanel`
      // right rail (M3): a single-path change would leave the rail
      // missing in one state.
      <div className="flex min-w-0 flex-1">
        <main className="flex flex-1 flex-col items-center justify-center text-neutral-500">
          <p className="text-lg">No active session</p>
          <p className="mt-1 text-sm">Open a space from the list on the left.</p>
        </main>
        <TodoBoardPanel sessionId={null} />
      </div>
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

  return (
    // Flex row: the stream (main) + the `TodoBoardPanel` right rail
    // (M3 — both return paths are wrapped; see the empty-state path above).
    <div className="flex min-w-0 flex-1">
      <main className="flex min-w-0 flex-1 flex-col">
      {view && (
        <div className="flex items-center justify-between gap-3 border-b border-neutral-800 px-4 py-2">
          <div className="flex min-w-0 items-center gap-2">
            <p className="truncate text-sm font-medium">{spaceTitle}</p>
            {(isLive || isHistoryOnly) && (
              <p className={`text-xs ${isLive ? "text-emerald-400" : "text-neutral-500"}`}>
                {isLive ? "live" : "stored"}
              </p>
            )}
          </div>
          <div className="flex flex-shrink-0 items-center gap-2">
            <select
              value={activeSessionId}
              onChange={(e) => openSession(e.target.value)}
              className="rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-xs outline-none focus:border-sky-600"
            >
              {options.map((opt) => (
                <option key={opt.id} value={opt.id}>
                  {opt.label}
                </option>
              ))}
            </select>
            {isLive && (
              <>
                <button
                  type="button"
                  onClick={() => void startNewConversation()}
                  disabled={newConversationAgentId === ""}
                  className="rounded-md border border-neutral-600 px-3 py-1 text-xs hover:bg-neutral-800 disabled:opacity-50"
                >
                  New conversation
                </button>
                <button
                  type="button"
                  onClick={() => void pause()}
                  className="rounded-md border border-neutral-600 px-3 py-1 text-xs hover:bg-neutral-800"
                >
                  Pause
                </button>
              </>
            )}
          </div>
        </div>
      )}
      {isHistoryOnly && (
        <div className="flex items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
          {canResume ? (
            <p className="text-xs text-neutral-400">
              {closeReasons[activeSessionId] === "replaced"
                ? "Paused — a conversation started in another space. Resume to reconnect."
                : "This session is stored. Resuming reconnects it to the agent."}
            </p>
          ) : (
            <p className="text-xs text-amber-400">
              History only — continuing starts a new session.
            </p>
          )}
          <div className="flex gap-2">
            {canResume && (
              <button
                type="button"
                onClick={() => void startNewConversation()}
                disabled={newConversationAgentId === ""}
                className="rounded-md border border-neutral-600 px-3 py-1 text-xs font-medium hover:bg-neutral-800 disabled:opacity-50"
              >
                New conversation
              </button>
            )}
            {canResume && (
              <button
                type="button"
                onClick={() => void resume()}
                disabled={resuming}
                className="rounded-md bg-sky-600 px-3 py-1 text-xs font-medium text-white hover:bg-sky-500 disabled:opacity-50"
              >
                {resuming ? "Resuming…" : "Resume"}
              </button>
            )}
          </div>
        </div>
      )}
      <div ref={scrollRef} className="flex-1 space-y-3 overflow-y-auto p-4">
        {messages.map((message, i) => {
          // The `AskQuestionCard` replaces the pending `ask` `ToolCallCard`
          // (correlated via `(source, toolCallId)`/`requestId` — a `main`
          // ask whose `toolCallId` matches this tool-call message).
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
        {inTurn && (
          <p className="text-xs text-neutral-500">Agent is working…</p>
        )}
        {!inTurn && stopReason && stopReason !== "end_turn" && (
          <p className="text-xs text-neutral-500">Turn ended: {stopReason}</p>
        )}
      </div>

      <div className="border-t border-neutral-800 p-3">
        {error && <p className="mb-2 text-xs text-red-400">{error}</p>}
        <div className="flex gap-2">
          <textarea
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
                ? "Send a prompt…"
                : canResume
                  ? "This space's conversation is paused — Resume to reconnect"
                  : "This session is closed / history only"
            }
            rows={2}
            disabled={!isLive || inTurn}
            className="flex-1 resize-none rounded-md border border-neutral-700 bg-neutral-900 px-3 py-2 text-sm outline-none focus:border-sky-600 disabled:opacity-50"
          />
          <button
            type="button"
            onClick={() => void send()}
            disabled={!isLive || inTurn || draft.trim() === ""}
            className="rounded-md bg-sky-600 px-4 py-2 text-sm font-medium text-white hover:bg-sky-500 disabled:opacity-50"
          >
            Send
          </button>
        </div>
      </div>
      </main>
      <TodoBoardPanel sessionId={activeSessionId} />
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
    </div>
  );
}
