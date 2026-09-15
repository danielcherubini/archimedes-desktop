import { useEffect, useRef, useState } from "react";
import { sendPrompt } from "../lib/tauri";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import MessageBubble from "./MessageBubble";
import PermissionPrompt from "./PermissionPrompt";

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
      ? s.historySessions.find((x) => x.sessionId === s.activeSessionId)
      : undefined,
  );
  const inTurn = useSessions((s) => (s.activeSessionId ? !!s.inTurn[s.activeSessionId] : false));
  const stopReason = useSessions((s) => (s.activeSessionId ? s.stopReasons[s.activeSessionId] : undefined));
  const prompts =
    usePermissions((s) =>
      activeSessionId ? s.prompts[activeSessionId] : undefined,
    ) ?? [];
  const addUserMessage = useSessions((s) => s.addUserMessage);
  const beginTurn = useSessions((s) => s.beginTurn);
  const turnCompleted = useSessions((s) => s.turnCompleted);
  const resumeSession = useSessions((s) => s.resumeSession);

  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  // A stored (non-live) session: its transcript is read-only unless the
  // agent negotiated `loadSession`, in which case it can be resumed.
  const isLive = !!liveSession;
  const isHistoryOnly = !isLive && !!historySession;
  const canResume =
    isHistoryOnly && historySession?.capabilities.loadSession === true;

  // Auto-scroll to the bottom as new content streams in.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, prompts.length]);

  if (!activeSessionId) {
    return (
      <main className="flex flex-1 flex-col items-center justify-center text-neutral-500">
        <p className="text-lg">No active session</p>
        <p className="mt-1 text-sm">Start a session from the list on the left.</p>
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

  return (
    <main className="flex min-w-0 flex-1 flex-col">
      {isHistoryOnly && (
        <div className="flex items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
          {canResume ? (
            <p className="text-xs text-neutral-400">
              This session is stored. Resuming reconnects it to the agent.
            </p>
          ) : (
            <p className="text-xs text-amber-400">
              History only — continuing starts a new session.
            </p>
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
      )}
      <div ref={scrollRef} className="flex-1 space-y-3 overflow-y-auto p-4">
        {messages.map((message, i) => (
          <MessageBubble key={i} message={message} />
        ))}
        {prompts.map((prompt) => (
          <PermissionPrompt
            key={prompt.requestId}
            sessionId={activeSessionId}
            requestId={prompt.requestId}
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
            placeholder={isLive ? "Send a prompt…" : "This session is closed"}
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
  );
}
