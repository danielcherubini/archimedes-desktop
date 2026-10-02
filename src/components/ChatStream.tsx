import {
  useEffect,
  useCallback,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { ArrowUp, FolderIcon, MoreHorizontalIcon, PanelRightIcon, X } from "lucide-react";
import {
  cancelSession,
  closeSession,
  readClipboardImage,
  sendPrompt,
  setSessionConfigOption,
  type SkillInfo,
} from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { expandSkillMentions } from "../lib/skills";
import { groupConsecutiveFileWrites } from "../lib/toolGroups";
import {
  addImageAttachments,
  agentSupportsImages,
  readAttachmentAsBase64,
  releaseAttachment,
  type ChatComposerAttachment,
} from "../lib/chatAttachments";
import { shouldPreferSpreadsheetClipboardText } from "../lib/chatAttachmentMetadata";
import { useSessions, spaceViewFor, type SpaceView } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useInteractive } from "../store/interactive";
import { useStartNewConversation } from "../hooks/useStartNewConversation";
import { usePendingSubagentRequests } from "../hooks/usePendingSubagentRequests";
import { useSpinQuip } from "../hooks/useSpinQuip";
import { useSkillCatalog } from "../hooks/useSkillCatalog";
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
import SessionConfigSelect from "./SessionConfigSelect";
import MessageBubble from "./MessageBubble";
import ChangesGroupCard from "./ChangesGroupCard";
import PermissionPrompt from "./PermissionPrompt";
import AskQuestionCard from "./AskQuestionCard";
import FileSummaryCard from "./FileSummaryCard";
import SudoConfirmModal from "./SudoConfirmModal";
import SudoPasswordModal from "./SudoPasswordModal";

/**
 * The active skill token at the caret (the ONE shared helper — used by the
 * `onChange` re-computation, the keydown re-evaluation, and `selectSkill`):
 * the span between the nearest preceding whitespace (or start-of-line) and
 * the caret. Returns `{ remainder, start }` when the span starts with `$`
 * — `remainder` is the token's remainder AFTER the `$` (a bare `$` is `""`),
 * `start` is the `$`'s index in the string — else `null` (no active token:
 * the span doesn't start with `$`, or it contains a character that can't be
 * part of a skill name, e.g. uppercase — the regex `[a-z0-9-]*$` simply won't
 * reach the caret).
 */
function activeSkillToken(
  value: string,
  caret: number,
): { remainder: string; start: number } | null {
  const before = value.slice(0, caret);
  const m = before.match(/(^|\s)(\$[a-z0-9-]*)$/);
  if (!m) return null;
  return { remainder: m[2]!.slice(1), start: (m.index ?? 0) + m[1]!.length };
}

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
  // Searches BOTH stored lists: a session living ONLY in
  // `archivedSessions` (opened from the Archived section) must still resolve
  // here — otherwise it renders no banner, a disabled composer ("This session
  // is closed"), and an unreachable `send()` auto-resume. One selector
  // returning a `??` of two finds (stable-reference rule preserved).
  const historySession = useSessions((s) =>
    s.activeSessionId
      ? (s.historySessions.find((x) => x.sessionId === s.activeSessionId) ??
          s.archivedSessions.find(
            (x) => x.sessionId === s.activeSessionId,
          ))
      : undefined,
  );
  const spaces = useSessions((s) => s.spaces);
  const sessions = useSessions((s) => s.sessions);
  const historySessions = useSessions((s) => s.historySessions);
  const archivedSessions = useSessions((s) => s.archivedSessions);
  const closeReasons = useSessions((s) => s.closeReasons);
  const openSession = useSessions((s) => s.openSession);
  const inTurn = useSessions((s) => (s.activeSessionId ? !!s.inTurn[s.activeSessionId] : false));
  const stopReason = useSessions((s) => (s.activeSessionId ? s.stopReasons[s.activeSessionId] : undefined));
  const prompts =
    usePermissions((s) =>
      activeSessionId ? s.prompts[activeSessionId] : undefined,
    ) ?? [];
  // Interactive surfaces for the ACTIVE session (B3): the store is keyed by the
  // ACP session id, which matches `activeSessionId`.
  const interactiveRequests =
    useInteractive((s) =>
      activeSessionId ? s.requests[activeSessionId] : undefined,
    ) ?? [];
  const askRequests = interactiveRequests.filter((r) => r.method === "ask");
  const confirmRequests = interactiveRequests.filter((r) => r.method === "confirm");
  const passwordRequests = interactiveRequests.filter(
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

  // The interactive agent state for the active session. `inTurn` is the
  // ground truth for "a turn is in flight" (set by `beginTurn`, cleared
  // by `turnCompleted`) — a STALE `idle` from the previous turn must
  // not unlock the composer or hide the working indicator, so `inTurn`
  // wins alongside `working` (the store does not reset `agentState` on
  // `turnCompleted` — a store follow-up; an agent that hasn't
  // pushed yet is covered the same way: its turn IS in flight).
  const agentState = useInteractive((s) =>
    activeSessionId ? s.agentState[activeSessionId] : undefined,
  );
  const workingOrInTurn = agentState === "working" || inTurn;
  // A `blocked` agent is mid-turn AWAITING a request response
  // (`inTurn` is true) — the composer must stay locked for the whole
  // wait (pre-branch, `main`'s composer locked on `inTurn`): sending
  // concurrently would double-send and clobber the turn bookkeeping.
  // (`agentState === "blocked"` also covers a blocked state without an
  // in-flight turn, where `inTurn` alone would not lock.)
  const composerLocked = workingOrInTurn || agentState === "blocked";
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
  const configOptions = useSessions(
    (s) => s.configOptions[s.activeSessionId ?? ""] ?? null,
  );
  const findOption = (category: string, id: string) =>
    configOptions?.find(
      (o) =>
        (o.category === category || o.id === id) &&
        o.type === "select" &&
        (o.options?.length ?? 0) > 0,
    );
  const modelOption = findOption("model", "model");
  const thinkingOption = findOption("thought_level", "thought_level");
  const applyConfigOptions = useSessions((s) => s.applyConfigOptions);
  // ONE stable callback for both config selects (the consumer-side half of
  // the SessionConfigSelect memo fix): a per-option closure minted per
  // render would re-render the ~600 mounted model-catalog SelectItems on
  // EVERY composer keystroke (~140ms each — see SessionConfigSelect.tsx).
  // `applyConfigOptions` is a zustand action — a stable reference, so this
  // callback's identity only changes when the active session changes.
  const setConfigValue = useCallback(
    async (optionId: string, value: string) => {
      if (!activeSessionId) return;
      const options = await setSessionConfigOption(
        activeSessionId,
        optionId,
        value,
      );
      applyConfigOptions(activeSessionId, options);
    },
    [activeSessionId, applyConfigOptions],
  );

  // Pending subagent requests (the toggle dot) — called UNCONDITIONALLY,
  // BEFORE the `!activeSessionId` early return below: this hook contains
  // `useSyncExternalStore` (via zustand), so calling it only on the
  // full-frame path would change the hook count when the active session
  // appears/disappears and crash React ("Rendered more hooks than during
  // the previous render" → white page).
  const pendingSubagentRequests = usePendingSubagentRequests();

  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  // Image attachments staged in the composer (CONTEXT.md: Attachment).
  // ALL of the attachment hooks live HERE, before the `!activeSessionId`
  // early return below — a hook called only on the full-frame path would
  // change the hook count when the active session appears/disappears and
  // crash React (the same bug the `usePendingSubagentRequests` comment
  // above warns about).
  const [attachments, setAttachments] = useState<ChatComposerAttachment[]>([]);
  const attachmentsRef = useRef(attachments);
  // Re-sync every render: the handlers read the CURRENT list from the ref
  // (a render closure would be stale for fast successive events — e.g.
  // pasting 8 files then a 9th in separate dispatches without a re-render
  // in between). Also read by the unmount-cleanup effect below (the effect
  // intentionally runs once, so the ref is the only live view it has).
  attachmentsRef.current = attachments;
  // Task 5's double-send guard (a hook — must live here, not in `send`):
  // `beginTurn` runs AFTER an `await` (the base64 read), so the composer is
  // not locked until then and a second Enter/click during the read would
  // otherwise double-send.
  const sendingRef = useRef(false);
  // Re-sync every render (the same pattern as `attachmentsRef` above): `send()`
  // is async and reads the attachment via a FileReader `await` BEFORE
  // `beginTurn`, so the composer is unlocked during the read. If the session
  // switches during the read, the closure's `activeSessionId` is stale — this
  // ref is the LIVE value `send()` checks after the read and aborts on a
  // mismatch (no send, no draft wipe, no turn).
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;
  // The LIVE draft, re-synced every render (the same pattern as `attachmentsRef`
  // above): `send()` captures `draft` at click time and the composer is not
  // locked until `beginTurn`, so the user can edit the draft during the
  // `resume()`/FileReader awaits. The ref is the live value `send()` checks
  // after the read and aborts on a mismatch (no stale send, no draft wipe).
  const draftRef = useRef(draft);
  draftRef.current = draft;
  // Capability gate (FAIL-CLOSED): the feature is inert unless the agent
  // advertises `promptCapabilities.image === true`.
  const imageCapable = agentSupportsImages(
    liveSession?.capabilities ?? historySession?.capabilities,
  );
  // Unmount cleanup: revoke the staged object URLs exactly once, on
  // unmount ONLY — revoking on every state change would leak/break live
  // previews (a revoked URL can no longer render the `img`).
  useEffect(
    () => () => {
      attachmentsRef.current.forEach(releaseAttachment);
    },
    [],
  );
  // Esc stops inference: while a turn is in flight, Escape sends
  // `session/cancel` (the agent resolves the open prompt with
  // `stopReason: "cancelled"`, which completes `sendPrompt` and unlocks the
  // composer). The textarea is DISABLED during a turn (the composer is
  // locked), so the listener is global — and active only while `inTurn`,
  // so Esc never hijacks a keypress outside a turn.
  useEffect(() => {
    if (!inTurn || !activeSessionId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        void cancelSession(activeSessionId);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [inTurn, activeSessionId]);
  // Session-change guard: `ChatStream` is one long-lived component (no
  // `key`), so `attachments` would otherwise survive a switch to another
  // session — images staged in session A (image-capable) could be sent to
  // session B even if B's agent doesn't advertise `promptCapabilities.image`
  // (breaking the fail-closed guarantee). Clear + release on switch.
  const prevSessionIdRef = useRef(activeSessionId);
  useEffect(() => {
    if (prevSessionIdRef.current === activeSessionId) return;
    prevSessionIdRef.current = activeSessionId;
    attachmentsRef.current.forEach(releaseAttachment);
    attachmentsRef.current = [];
    setAttachments([]);
  }, [activeSessionId]);
  // Window-level drop backstop: with `dragDropEnabled: false` (tauri.conf.json)
  // the webview receives native file drops — and the webview DEFAULT for a
  // file drop is to NAVIGATE to the file (replacing the app). A file dropped
  // anywhere OUTSIDE the composer must be swallowed: `preventDefault` on
  // every `dragover` makes the whole page a valid drop target, and this
  // backstop (the composer's own `onDrop` fires first during bubbling) stops
  // the navigation. (This also means NO composer-level `onDragOver` is
  // needed — the window listener covers the composer's dragovers too.)
  useEffect(() => {
    const prevent = (e: Event) => e.preventDefault();
    window.addEventListener("dragover", prevent);
    window.addEventListener("drop", prevent);
    return () => {
      window.removeEventListener("dragover", prevent);
      window.removeEventListener("drop", prevent);
    };
  }, []);

  // Auto-grow the composer textarea: reset to `auto`, then the
  // border-box height — `scrollHeight` is the CONTENT height, but
  // `height` (border-box) must also cover the border (`offsetHeight -
  // clientHeight` is the border thickness); without it a ~2px deficit
  // leaves a scrollbar flicker near `max-h-32`. Re-applied on window
  // resize (a stale height would otherwise linger until the next
  // keystroke).
  useEffect(() => {
    const el = composerRef.current;
    if (!el) return;
    const apply = () => {
      el.style.height = "auto";
      el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [draft]);

  // --- Skills (Task 5): the `$`-trigger picker + the send-path expansion. ---
  // ALL of the hooks below live in the UNCONDITIONAL top block (before the
  // `!activeSessionId` early return further down) — placing any of them after
  // the early return would change the hook count across the session/no-session
  // transition and crash React (the same bug the `usePendingSubagentRequests`
  // comment above warns about). Plain (non-hook) functions like `selectSkill`
  // are placement-flexible (the file's own comment says so for the attachment
  // handlers) and live with the other handlers below.
  // The Space path is the active session's `cwd` (CONTEXT.md: a Session's
  // `cwd` IS the Space's folder) — live first, then stored (the same
  // derivation as Task 4's `SpacesList`). `useSkillCatalog` caches per Space,
  // so the composer and the left pane share ONE fetch (same key).
  const spacePath = liveSession?.cwd ?? historySession?.cwd ?? null;
  const skills = useSkillCatalog(spacePath);
  // The `$`-trigger picker state (the active token + the highlighted row).
  const [picker, setPicker] = useState<{
    query: string;
    index: number;
  } | null>(null);
  // The catalog rows matching the active token (case-insensitive substring on
  // the NAME — v1: name only, not description). `picker?.query ?? ""` yields
  // the full list while the picker is null (harmless — the picker UI is gated
  // on `picker && filtered.length > 0`). NULL-SAFE: the `picker` state is
  // `{…} | null` and this `useMemo` lives in the unconditional top block, so a
  // bare `picker.query` would be a TS18047 compile error under `strict: true`
  // (and a `picker!` "fix" would crash at render whenever the picker is
  // closed, i.e. nearly every render).
  const filtered = useMemo(
    () =>
      skills.filter((s) =>
        s.name.toLowerCase().includes((picker?.query ?? "").toLowerCase()),
      ),
    [skills, picker],
  );
  // The highlighted row index, DERIVED (not clamped in place): the raw
  // `picker.index` can go stale (a Space switch refetches `skills` while the
  // picker is open with a non-zero `index`), and `filtered[staleIndex]` would
  // be `undefined` → a `selectSkill(undefined)` crash on Enter. `Math.max(0, …)`
  // is belt-and-braces: the picker UI and the keyboard branch are both guarded
  // by `filtered.length > 0`, so `filtered.length - 1` is ≥ 0 there.
  const activeIndex = picker
    ? Math.min(picker.index, Math.max(0, filtered.length - 1))
    : 0;
  // The `archimedes:insert-skill` listener (Task 4's left-pane rows dispatch
  // it): insert `$name ` at the caret and re-focus. Read through refs (the
  // `draftRef` mirror already exists) so the effect runs once and always sees
  // the LIVE draft (a render closure would be stale for fast events).
  useEffect(() => {
    const onInsertSkill = (e: Event) => {
      // Close the `$`-trigger picker: the insert comes from OUTSIDE the
      // picker (a SkillsDialog row — the v1.1 flow), so a picker open at
      // insert time is stale (it would linger rendered until the next
      // keydown, and a token active at the caret would be spliced INTO).
      setPicker(null);
      const text = (e as CustomEvent<string>).detail;
      if (typeof text !== "string") return;
      const el = composerRef.current;
      const draft = draftRef.current; // the LIVE value (the ref mirror above)
      if (!el) {
        setDraft(draft + text);
        return;
      }
      const start = el.selectionStart ?? draft.length;
      const end = el.selectionEnd ?? start;
      const next = draft.slice(0, start) + text + draft.slice(end);
      setDraft(next);
      requestAnimationFrame(() => {
        el.focus();
        const pos = start + text.length;
        el.setSelectionRange(pos, pos);
      });
    };
    window.addEventListener("archimedes:insert-skill", onInsertSkill);
    return () => window.removeEventListener("archimedes:insert-skill", onInsertSkill);
  }, []);

  // A stored (non-live) session: its transcript is read-only unless the
  // agent negotiated `loadSession`, in which case it can be resumed.
  const isLive = !!liveSession;
  const isHistoryOnly = !isLive && historySession !== undefined;
  const canResume =
    isHistoryOnly && historySession?.capabilities.loadSession === true;
  // The composer is usable for a live session OR a RESUMABLE stored session
  // (a non-resumable stored session stays fully disabled):
  // a resumable stored session auto-resumes on send.
  const composerEnabled = isLive || canResume;

  // The current space's `SpaceView` for `activeSessionId`: the space that
  // owns it — by its live session OR any of its stored sessions (NOT
  // `[0]`-only: the conversation selector below lets the user open
  // `#2`+ sessions, which are stored sessions of the space too, and the
  // space bar must stay rendered while they are open) OR any of its
  // ARCHIVED sessions (a space whose only sessions are archived still
  // resolves its `view` — the space bar + conversation selector stay
  // rendered). `undefined` when the active session belongs to no space
  // (e.g. a legacy pre-Spaces DB row) — rendered exactly as today (no space
  // chip, no selector).
  const views = useMemo(
    () =>
      spaces.map((s) =>
        spaceViewFor(s, sessions, historySessions, closeReasons, archivedSessions),
      ),
    [spaces, sessions, historySessions, closeReasons, archivedSessions],
  );
  const view: SpaceView | undefined =
    activeSessionId === null
      ? undefined
      : views.find(
          (v) =>
            v.liveSessionId === activeSessionId ||
            v.storedSessionIds.includes(activeSessionId) ||
            v.archivedSessionIds.includes(activeSessionId),
        );

  // `New session` in this space (the extracted hook): `spacePath =
  // view.path` (the active session's `cwd` by the match above; the
  // backend canonicalizes) — a native session (one harness — no agent
  // to choose). With the one-live cap lifted, a new
  // conversation does NOT displace a live one — they coexist.
  // `view === undefined` → the hook no-ops (the `New Session`
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

  // `hasImages`: a staged attachment AND the capability — the last line of
  // the fail-closed defense. Staged attachments are already cleared on
  // session switch (Task 4), but a same-session capability regression must
  // not ship images to an agent that can't take them. With it, an
  // image-ONLY send while `!imageCapable` is blocked (an empty prompt +
  // unsent images is meaningless), while a text-only send simply doesn't
  // attach images.
  // `let`: re-derived after `resume()` below — the FRESH agent's
  // capabilities (not the saved ones) decide whether images ship.
  let hasImages = attachments.length > 0 && imageCapable;

  const send = async () => {
    const rawText = draft.trim();
    if ((!rawText && !hasImages) || composerLocked) return;
    if (sendingRef.current) return; // guard: see the ref's comment above
    sendingRef.current = true;
    try {
      setError(null);
      // Auto-resume a stored (non-live) session before sending. A non-resumable
      // stored session can't be sent to; a failed resume aborts (error already set).
      if (!isLive) {
        if (!canResume) return;
        if (!(await resume())) return;
        // The resume reconnected to a FRESH agent: the SAVED capabilities
        // that gated `hasImages` above are stale. Re-derive from the live
        // session's capabilities (the store's `resumeSession` just upserted
        // the resume result): if the fresh agent doesn't advertise
        // `promptCapabilities.image`, proceed text-only (the same
        // fail-closed treatment — an image-ONLY send is blocked, a text
        // send simply doesn't attach the staged images); if it DOES
        // advertise images, include them.
        const fresh = useSessions
          .getState()
          .sessions.find((x) => x.sessionId === activeSessionId);
        hasImages =
          attachments.length > 0 && agentSupportsImages(fresh?.capabilities);
        if (!hasImages && rawText === "") return;
      }
      let images:
        | { id: string; name: string; mimeType: string; sizeBytes: number; data: string }[]
        | undefined;
      if (hasImages) {
        try {
          const read = await Promise.all(
            attachments.map(async (att) => ({
              id: att.id,
              name: att.filename,
              mimeType: att.mimeType,
              sizeBytes: att.sizeBytes,
              data: await readAttachmentAsBase64(att),
            })),
          );
          // Reconcile against the LIVE list: the composer isn't locked until
          // beginTurn, so a thumbnail removed during the read must not be sent.
          const liveIds = new Set(attachmentsRef.current.map((a) => a.id));
          images = read.filter((i) => liveIds.has(i.id));
        } catch {
          setError("Failed to read an attached image");
          return; // attachments stay staged; `sendingRef` resets in `finally`
        }
      }
      // Session switched during the read (the composer isn't locked until
      // beginTurn): the closure's `activeSessionId` is stale — abort without
      // sending, without wiping the draft, without completing a turn.
      if (activeSessionIdRef.current !== activeSessionId) return;
      // Draft edited during the read (the composer isn't locked until
      // `beginTurn`): sending the stale text and wiping the new draft is a
      // silent data loss — abort without sending, without wiping the draft,
      // without completing a turn (the user simply sends again). The guard
      // compares the RAW text (expansion is re-derived from the FRESH draft
      // below, so comparing expanded texts would be wrong).
      if (draftRef.current.trim() !== rawText) return;
      // EXPANSION: expand `$name` mentions into pi-format `<skill>` blocks
      // BEFORE both `addUserMessage` and `sendPrompt` (the REFINEMENT — the
      // live bubble, the persisted record, and the agent's input must all
      // carry the SAME text, so the content-based dedupe key in
      // `mergeDedupeKey` matches across a resume reload). Declared HERE
      // (immediately after the stale-draft guard, BEFORE the images-empty
      // block below which still references `text`) — the placement is
      // load-bearing (a TDZ trap: a later declaration would make the
      // unchanged `if (!text)` a "used before its declaration" error;
      // semantically it's safe — expansion maps `""`→`""`, so `!text` is
      // identical to `!rawText`).
      const text = expandSkillMentions(rawText, skills);
      // Every staged image was removed during the read (the composer isn't
      // locked until `beginTurn`, so a thumbnail can be removed during the
      // read): with no text there's nothing meaningful left to send; with
      // text, normalize to `undefined` so the 2-arg `sendPrompt` path is taken
      // (an empty `[]` is truthy and would take the 3-arg path, sending an
      // empty images array).
      if (images !== undefined && images.length === 0) {
        if (!text) return;
        images = undefined;
      }
      // `sentIds` is derived from what was ACTUALLY sent (the reconciled
      // `images`), not the stale closure list: on success, release/clear ONLY
      // these. Anything staged after the read — e.g. a drop during the
      // FileReader `await`, or attachments staged in ANOTHER session while
      // this turn was running — must survive.
      const sentIds = new Set(images?.map((i) => i.id) ?? []);
      // `ImageRef` has no `id` field — strip it before use.
      const imageRefs = images?.map(({ id, ...ref }) => ref); // ImageRef[] | undefined
      setDraft("");
      addUserMessage(activeSessionId, text, imageRefs); // EXPANDED
      beginTurn(activeSessionId);
      // Pass the third arg ONLY when there are images: a text-only send (and
      // an all-images-removed send, normalized above) calls
      // `sendPrompt(id, text)` — the pre-existing 2-arg call the existing tests
      // assert (`toHaveBeenCalledWith("s1", "hello")`; vitest compares arg
      // arrays by length, so an explicit `undefined` third arg would break it).
      const stopReason = imageRefs
        ? await sendPrompt(activeSessionId, text, imageRefs) // EXPANDED
        : await sendPrompt(activeSessionId, text); // EXPANDED
      // Release/clear ONLY the sent attachments, OUTSIDE the state updater
      // (updaters must be pure — StrictMode runs them twice).
      const still = attachmentsRef.current.filter((a) => !sentIds.has(a.id));
      attachmentsRef.current
        .filter((a) => sentIds.has(a.id))
        .forEach(releaseAttachment);
      attachmentsRef.current = still;
      setAttachments(still);
      turnCompleted(activeSessionId, stopReason);
    } catch (err) {
      turnCompleted(activeSessionId, "end_turn");
      // Tauri IPC errors are plain objects (`{ kind, message }`), not `Error`
      // instances — `String(err)` would show `[object Object]` (a pre-existing
      // gap the new `InvalidPrompt` validation error would hit; fix it here).
      const msg =
        err instanceof Error
          ? err.message
          : err && typeof err === "object" && "message" in err
            ? String((err as { message: unknown }).message)
            : String(err);
      setError(msg);
      // Attachments deliberately stay staged (a failed send keeps them — a
      // re-pasted image is costly, a re-typed draft is not). The draft is
      // lost on failure (pre-existing behavior, unchanged).
    } finally {
      sendingRef.current = false;
    }
  };

  const resume = async (): Promise<boolean> => {
    if (!activeSessionId || resuming) return false;
    setResuming(true);
    setError(null);
    try {
      await resumeSession(activeSessionId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setResuming(false);
    }
    return true;
  };

  // Conversation selector options for the space: the live session first
  // (label `Live`), then stored sessions in order (`#1`, `#2`, …); when
  // the live session is absent the first stored gets the label `Latest`.
  // An ARCHIVED session is NOT in `storedSessionIds` (the Space group
  // renders stored sessions only) — but the active session must still have
  // a matching item, or the `Select` renders an empty trigger + empty
  // dropdown (zero items with a `value` matching nothing).
  const options: Array<{ id: string; label: string }> = [];
  if (view && view.liveSessionId !== null) {
    options.push({ id: view.liveSessionId, label: "Live" });
  }
  if (view) {
    view.storedSessionIds.forEach((id, i) => {
      options.push({ id, label: view.liveSessionId === null && i === 0 ? "Latest" : `#${i + 1}` });
    });
    // Only add the fallback when the active session is NOT already
    // represented above: `spaceViewFor` does NOT filter live ids
    // out of `archivedSessionIds` (it's view-membership only), so a
    // STICKY-archived session (resumed, now live — the id is in BOTH
    // `sessions` and `archivedSessions`) would otherwise be pushed TWICE
    // (once as `Live`, once as `Archived` — two `SelectItem`s with the
    // same `key`/`value`). Label it `Live` when it IS live (in `sessions`
    // — a multi-live space where a second, more recent live session is
    // `view.liveSessionId`); `Archived` only when it isn't.
    if (
      activeSessionId !== view.liveSessionId &&
      !view.storedSessionIds.includes(activeSessionId) &&
      view.archivedSessionIds.includes(activeSessionId)
    ) {
      options.push({
        id: activeSessionId,
        label: sessions.some((s) => s.sessionId === activeSessionId) ? "Live" : "Archived",
      });
    }
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

  // The `bg-warning` dot on the toggle — the cue for the COLLAPSED-PANE
  // case (the `SidePane` "Waiting" pill is clipped by the frame's
  // `width: 0` + `overflow: hidden` when collapsed): it counts the
  // ACTIVE session's requests PLUS pending subagent requests (subagent
  // session ids are never active, so the active-session count alone
  // would strand a pending subagent request until the interactive timeout).
  const hasPendingRequest =
    prompts.length > 0 ||
    interactiveRequests.some(
      (r) =>
        r.method === "ask" || r.method === "confirm" || r.method === "password",
    ) ||
    pendingSubagentRequests > 0;
  // The "Send a prompt to start" hint is hidden when a pending request
  // (permission prompt or interactive `ask`/`confirm`/`password` — the cards
  // render in the stream regardless of the transcript's length) or a
  // working/blocked line is present — otherwise the card would be
  // swallowed by the hint.

  // --- Image-attachment handlers (plain functions, NOT hooks — their
  // placement is flexible; kept with the other handlers). ---
  const stageFiles = (files: File[]) => {
    // Read the CURRENT list from the ref — a render closure would be stale
    // for fast successive events (e.g. pasting 8 files then a 9th in
    // separate dispatches without a re-render in between).
    const { attachments: next, rejected } = addImageAttachments(
      attachmentsRef.current,
      files,
    );
    if (next !== attachmentsRef.current) {
      attachmentsRef.current = next;
      setAttachments(next);
    }
    // A successful stage also CLEARS a stale rejection line (null when
    // nothing was rejected) — otherwise a rejection stays visible after the
    // user successfully stages a different image.
    setError(rejected.length > 0 ? rejected.join("; ") : null);
  };

  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    if (!imageCapable) return; // fall through: default text paste
    // Pasted images live in `clipboardData.items` (a `ClipboardItem` pulled via
    // `getAsFile()`); `clipboardData.files` is EMPTY in real webviews
    // (WebKit/WebKitGTK, WebView2/Chromium), so derive the file list from
    // `items` instead — keep only `kind === "file"` items, drop null results.
    const files: File[] = [];
    for (const item of e.clipboardData.items) {
      if (item.kind === "file") {
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    const text = e.clipboardData.getData("text/plain");
    const html = e.clipboardData.getData("text/html");
    // A spreadsheet paste carries TSV (or Excel HTML) alongside the
    // synthetic image — the TEXT wins, so the default paste is untouched.
    if (
      files.length > 0 &&
      !shouldPreferSpreadsheetClipboardText(text, html)
    ) {
      e.preventDefault();
      e.stopPropagation();
      stageFiles(files);
      return;
    }
    // WebKitGTK quirk: a pasted image produces a `paste` event with NO file
    // items and NO text (the DataTransfer is empty — verified on
    // webkit2gtk-4.1 2.52.5 on Wayland; the image IS inserted into a
    // contenteditable, just not exposed on the event). When the event has
    // no content at all, the likely cause is a clipboard image WebKit can't
    // surface — read it from the system clipboard (Rust/arboard) instead.
    // A text-only paste is a text paste: `getData("text/plain")` is
    // non-empty, so the fallback is skipped (no stale image staged).
    if (files.length === 0 && !text) {
      // The read can straddle a session switch (it is a slow IPC round
      // trip): capture the active session at PASTE time and abort if it
      // changed — without the guard, `stageFiles` would target the NEW
      // active session (staging session A's pasted image in session B's
      // composer; the same cross-session leak `send()`'s
      // `activeSessionIdRef` guard prevents for sends).
      const pastedInSession = activeSessionIdRef.current;
      void readClipboardImage()
        .then((bytes) => {
          if (!bytes) return; // no image on the clipboard — nothing to stage
          if (activeSessionIdRef.current !== pastedInSession) return; // switched
          const file = new File([new Uint8Array(bytes)], "screenshot.png", {
            type: "image/png",
          });
          stageFiles([file]);
        })
        .catch(() => {
          // Silent: clipboard access failed (or no image) — an empty-clipboard
          // paste is not an error condition.
        });
    }
  };

  const handleDrop = (e: React.DragEvent) => {
    if (composerLocked || !imageCapable) return;
    e.preventDefault();
    stageFiles(Array.from(e.dataTransfer.files));
  };

  const removeAttachment = (id: string) => {
    // Revoke OUTSIDE the state updater (updaters must be pure — StrictMode
    // runs them twice; a double revoke is harmless but the wrong pattern).
    const target = attachmentsRef.current.find((a) => a.id === id);
    if (target) releaseAttachment(target);
    const next = attachmentsRef.current.filter((a) => a.id !== id);
    attachmentsRef.current = next;
    setAttachments(next);
  };

  // Select a skill from the `$`-trigger picker: replace the active token
  // (the `$`-prefixed span at the caret — the shared `activeSkillToken`
  // helper) with `$<name> ` (LOWERcased — the case policy: a picker-selected
  // skill ALWAYS expands on send, and the mention regex is lowercase-only;
  // the expansion keeps the frontmatter name verbatim in the block's `name`
  // attribute). The `requestAnimationFrame` re-focus + caret-set is REQUIRED:
  // `setDraft` re-renders the controlled textarea, which would otherwise drop
  // focus/caret.
  const selectSkill = (skill: SkillInfo) => {
    if (!picker) return;
    const el = composerRef.current;
    const caret = el?.selectionStart ?? draft.length;
    const token = activeSkillToken(draft, caret);
    if (!token) return;
    const inserted = `$${skill.name.toLowerCase()} `;
    const next = draft.slice(0, token.start) + inserted + draft.slice(caret);
    setPicker(null);
    setDraft(next);
    requestAnimationFrame(() => {
      const el = composerRef.current;
      if (!el) return;
      el.focus();
      const pos = token.start + inserted.length;
      el.setSelectionRange(pos, pos);
    });
  };

  // The transcript is grouped ONCE per render (not inside the map): a
  // maximal run of consecutive `write`/`edit` tool calls folds into a
  // single `Changes` card; everything else renders as before.
  const units = groupConsecutiveFileWrites(messages);

  return (
    <main className="m-1 flex min-w-0 flex-1 flex-col rounded-xl bg-background-alt">
      {isHistoryOnly && (
        <div className="m-2 flex items-center justify-between gap-2 rounded-md bg-surface px-3 py-2 text-ui-sm">
          {canResume ? (
            // Informational ONLY (the manual Resume button is gone — the
            // first `send()` auto-resumes the session before sending).
            <span className="text-foreground-subtle">
              This session is stored. Sending a message resumes it.
            </span>
          ) : (
            <span className="text-warning">
              History only — continuing starts a new session.
            </span>
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
        {/* The request cards and the working/blocked/stop-reason lines
            render UNCONDITIONALLY (regardless of the transcript's
            length — a native agent that asks at session start, or the
            window between `openSession` and `loadHistory` hydration,
            must not have its request swallowed by the hint). */}
        {messages.length === 0 &&
          !hasPendingRequest &&
          !workingOrInTurn &&
          agentState !== "blocked" && (
            <div className="flex h-full items-center justify-center">
              <p className="text-ui-base text-foreground-subtlest">
                Send a prompt to start
              </p>
            </div>
          )}
        {units.map((unit, i) => {
          if (unit.kind === "changes-group") {
            return (
              <ChangesGroupCard
                key={`${activeSessionId}:${i}`}
                messages={unit.messages}
              />
            );
          }
          const message = unit.message;
          // The key is session-scoped: switching sessions must not reuse the
          // previous session's component at the same index (a `Reasoning`
          // would otherwise carry over expanded state, duration, and timers).
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
                  key={`${activeSessionId}:${i}`}
                  sessionId={activeSessionId}
                  requestId={anchored.requestId}
                />
              );
            }
          }
          return (
            <MessageBubble
              key={`${activeSessionId}:${i}`}
              message={message}
              isStreaming={
                message.kind === "agent-thought" &&
                inTurn &&
                i === units.length - 1
              }
              sessionId={activeSessionId}
            />
          );
        })}
        {prompts.map((prompt) => (
          <PermissionPrompt
            key={prompt.requestId}
            sessionId={activeSessionId}
            requestId={prompt.requestId}
          />
        ))}
        {/* Stacked interactive `ask` cards (one per pending request, arrival
            order — concurrent asks stack vertically in the stream). */}
        {stackedAskRequests.map((r) => (
          <AskQuestionCard
            key={r.requestId}
            sessionId={activeSessionId}
            requestId={r.requestId}
          />
        ))}
        {turnDiffs.length > 0 && <FileSummaryCard diffs={turnDiffs} />}
        {/* `blocked` takes precedence: a blocked agent that is also
            mid-turn (`inTurn`) shows the waiting line, NOT the braille
            loader (both would render otherwise). */}
        {agentState === "blocked" ? (
          <p className="text-ui-sm text-foreground-subtle">
            Waiting for your input…
          </p>
        ) : (
          workingOrInTurn && (
            <div className="flex items-center gap-2">
              <BrailleLoader
                variant="typing"
                speed="normal"
                fontSize={14}
                label="Agent working"
              />
              <p className="text-ui-sm text-foreground-subtle">{quip}</p>
            </div>
          )
        )}
        {!inTurn && stopReason && stopReason !== "end_turn" && (
          <p className="text-ui-sm text-foreground-subtlest">
            Turn ended: {stopReason}
          </p>
        )}
      </div>

      {(error ?? newConversationError) && (
        <p className="mb-2 px-3 text-ui-sm text-destructive">
          {error ?? newConversationError}
        </p>
      )}
      <div
        className="relative m-3 rounded-2xl border border-input-border bg-input p-3 transition-colors hover:border-input-border-hover focus-within:border-input-border-focused focus-within:bg-input-focused"
        onDrop={handleDrop}
      >
        {picker && filtered.length > 0 && (
          <div
            data-testid="skill-picker"
            className="absolute left-3 right-3 -top-2 z-10 -translate-y-full rounded-lg border border-input-border bg-input p-1 shadow-lg"
          >
            {filtered.map((s, i) => (
              <button
                key={s.name}
                type="button"
                onMouseDown={(e) => {
                  e.preventDefault();
                  selectSkill(s);
                }}
                className={`flex w-full flex-col gap-0.5 rounded-md px-2 py-1 text-left ${i === activeIndex ? "bg-surface-hover" : ""}`}
              >
                <span className="text-ui-base">{s.name}</span>
                {s.description !== "" && (
                  <span
                    className="truncate text-ui-sm text-foreground-subtle"
                    title={s.description}
                  >
                    {s.description}
                  </span>
                )}
              </button>
            ))}
          </div>
        )}
        {attachments.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-2">
            {attachments.map((att) => (
              <div
                key={att.id}
                className="group relative size-12 overflow-hidden rounded-lg border border-input-border bg-input"
              >
                <img
                  src={att.objectUrl}
                  alt={att.filename}
                  className="size-full object-cover"
                />
                <button
                  type="button"
                  aria-label="Remove image attachment"
                  onClick={() => removeAttachment(att.id)}
                  className="absolute right-0.5 top-0.5 size-4 rounded-full bg-input p-0 opacity-0 transition-opacity group-hover:opacity-100"
                >
                  <X className="size-3 text-foreground" />
                </button>
              </div>
            ))}
          </div>
        )}
        <textarea
          ref={composerRef}
          value={draft}
          onChange={(e) => {
            // The `$`-trigger: a bare `$` (empty token remainder) opens the
            // picker with the FULL list, any non-`$` span closes it.
            const token = activeSkillToken(
              e.target.value,
              e.target.selectionStart ?? e.target.value.length,
            );
            setDraft(e.target.value);
            setPicker(token ? { query: token.remainder, index: 0 } : null);
          }}
          onPaste={handlePaste}
          onKeyDown={(e) => {
            // Re-evaluate the active token at KEYDOWN time (the caret is on
            // the event's target): ArrowLeft/Right, Home/End, and a mouse
            // click move the caret WITHOUT `onChange`, so the `picker` state
            // (only recomputed in `onChange`) can be STALE — the caret may
            // no longer be on the token.
            const el = e.currentTarget;
            const token = activeSkillToken(
              el.value,
              el.selectionStart ?? el.value.length,
            );
            if (picker && !token) {
              // The caret left the token: CLOSE the picker instead of
              // `selectSkill`'s silent early-return — a stale picker must
              // never swallow keys (Enter sends, arrows move the caret, Tab
              // falls through to the textarea default).
              setPicker(null);
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
              return;
            }
            // The picker is open AND the token is active at the caret: ↑/↓
            // move the highlight (with wrap), Enter/Tab select the highlighted
            // row (ZCode's `MentionPlugin` registers `KEY_TAB_COMMAND` →
            // `selectOption(selectedIndex)` — the same handler Enter uses),
            // Escape closes. `Shift+Enter` (a newline) and `Shift+Tab`
            // (move focus BACKWARD — intercepting it would be a
            // keyboard/a11y trap) fall through to the textarea default
            // (NOT a selection, NOT swallowed). The index used here is
            // `activeIndex` (the derivation above — NOT the raw
            // `picker.index`, which can be stale against a changed
            // `filtered`).
            if (picker && token && filtered.length > 0) {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setPicker({
                  ...picker,
                  index: (activeIndex + 1) % filtered.length,
                });
                return;
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                setPicker({
                  ...picker,
                  index: (activeIndex + filtered.length - 1) % filtered.length,
                });
                return;
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                selectSkill(filtered[activeIndex]!);
                return;
              }
              if (e.key === "Tab" && !e.shiftKey) {
                e.preventDefault();
                selectSkill(filtered[activeIndex]!);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setPicker(null);
                return;
              }
            }
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder={
            isLive
              ? composerLocked
                ? "Agent is working…"
                : messages.length === 0
                  ? imageCapable
                    ? "Ask anything — or paste an image…"
                    : "Ask anything…"
                  : "Ask for follow-up changes"
              : canResume
                ? "Resume this session to send"
                : "This session is closed"
          }
          rows={2}
          disabled={!composerEnabled || composerLocked}
          className="max-h-32 w-full resize-none overflow-y-auto bg-transparent text-ui-base outline-none placeholder:text-foreground-subtlest disabled:opacity-50"
        />
        <div className="mt-1 flex items-center justify-end gap-2">
          {/* ZCode's composer carries the config controls in its toolbar
              (left of the send button) — the header does not. */}
          <div className="flex flex-wrap items-center gap-2">
            {isLive && modelOption && (
              <SessionConfigSelect option={modelOption} onSet={setConfigValue} />
            )}
            {isLive && thinkingOption && (
              <SessionConfigSelect option={thinkingOption} onSet={setConfigValue} />
            )}
            <Button
              size="icon-md"
              aria-label="Send"
              disabled={
                !composerEnabled ||
                composerLocked ||
                (draft.trim() === "" && !hasImages)
              }
              onClick={() => void send()}
              className="bg-primary text-primary-foreground"
            >
              <ArrowUp className="size-4" />
            </Button>
          </div>
        </div>
      </div>
      {/* Interactive modals (rendered at the `ChatStream` root — `fixed`
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
