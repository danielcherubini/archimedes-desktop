import {
  useEffect,
  useCallback,
  useMemo,
  useRef,
  useState,
} from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  cancelSession,
  readClipboardImage,
  readFileBytes,
  sendPrompt,
  setSessionConfigOption,
} from "../lib/tauri";
import { basenameOfPath } from "../lib/paths";
import { activeMentionToken, expandMentions } from "../lib/skills";
import { groupConsecutiveFileWrites } from "../lib/toolGroups";
import {
  addImageAttachments,
  agentSupportsImages,
  readAttachmentAsBase64,
  releaseAttachment,
  type ChatComposerAttachment,
} from "../lib/chatAttachments";
import { inferAttachmentMimeType, shouldPreferSpreadsheetClipboardText } from "../lib/chatAttachmentMetadata";
import { useSessions } from "../store/sessions";
import { usePermissions } from "../store/permissions";
import { useInteractive } from "../store/interactive";
import { useSettings } from "../store/settings";
import { usePendingSubagentRequests } from "../hooks/usePendingSubagentRequests";
import { useSpinQuip } from "../hooks/useSpinQuip";
import { useMentionCatalogs } from "../hooks/useMentionCatalogs";
import { normalizeVariant } from "../lib/braille-loader";
import SessionStalledBanner from "./SessionStalledBanner";
import MessageList from "./chat/MessageList";
import ComposerRow from "./chat/ComposerRow";
import ComposerModals, { WorkingIndicator } from "./chat/ComposerModals";
import type { MentionRow } from "./chat/ComposerMentions";

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
  // The working-indicator spinner style (the settings' `spinnerStyle` — a
  // `braille-loader` variant name): `null` / absent = the `typing` default.
  // A hand-edited settings file may carry an unknown name: `normalizeVariant`
  // falls back to the loader's `breathe` default (never an invalid variant).
  const spinnerStyle = normalizeVariant(
    useSettings((s) => s.settings?.spinnerStyle ?? "typing"),
  );
  /**
   * (ADR 0031's follow-up) Whether the `#` trigger names an MCP server. The
   * fail-safe default is OFF — `settings` is `null` until the first load
   * lands, and `#` is the prefix that collides with pasted developer text
   * (`#include`, `#123`), so an unknown state must behave like "off", never
   * like "on".
   *
   * THE GATE IS THE CATALOG, NOT A FLAG PARAMETER. Both the picker and the
   * send-path expansion are ALREADY catalog-gated (a token expands only if the
   * catalog has that exact name — the load-bearing invariant of ADR 0031), so
   * feeding the mention code an EMPTY MCP catalog switches `#` off completely
   * with NO new code path: `expandMentions` finds no match and returns the
   * text BYTE-IDENTICAL (its verbatim invariant), and `filtered` yields no
   * rows, so the picker renders nothing and every keyboard branch (gated on
   * `filtered.length > 0`) stays inert — Enter still sends, no keys are
   * swallowed. The `$` / `@` catalogs are untouched.
   *
   * The DISPLAY side is deliberately NOT gated: `MessageBubble` splits the
   * persisted text with `splitMentionBlocks`, so a message sent while the
   * trigger was ON keeps rendering its `<mcp>` chip after the toggle goes OFF
   * (the record is unchanged, and the content-based `mergeDedupeKey` resume
   * merge stays byte-stable).
   *
   * The `useMentionCatalogs` FETCH below is NOT gated either — it is a cheap
   * config read, and gating it would make a live toggle require a remount
   * before the rows appear.
   */
  const mcpMentionsEnabled = useSettings(
    (s) => s.settings?.mcpMentionsEnabled ?? false,
  );
  // A `blocked` agent is mid-turn AWAITING a request response
  // (`inTurn` is true) — the SEND must stay disabled for the whole wait
  // (pre-branch, `main`'s composer locked on `inTurn`): the harness is
  // one-turn-at-a-time (a prompt while a turn is in flight is REJECTED,
  // not queued — `send_prompt`'s atomic `pending_turn` claim), so sending
  // concurrently would fail + clobber the turn bookkeeping. The TEXTAREA
  // is always editable (you can draft while the agent works — `send()`'
  // `composerLocked` guard no-ops Enter, the draft is kept for when the
  // turn ends). (`agentState === "blocked"` also covers a blocked state
  // without an in-flight turn, where `inTurn` alone would not lock.)
  const composerLocked = workingOrInTurn || agentState === "blocked";
  // Called UNCONDITIONALLY at the top of the component body (the hook
  // contains `useState`/`useEffect` — invoking it inside the `working`
  // branch would be a conditional hook call and crash React when the
  // state flips).
  const quip = useSpinQuip(workingOrInTurn);
  // The session's config options (model / thinking-level selectors):
  // the store's map FIRST (the freshest — the live frames; a session
  // closed in this app session keeps its last known entry — the close
  // no longer drops it), falling back to the session row's
  // `configOptions` (the Rust `list_sessions` shape — synthesized from
  // the stored `model` / `thinkingLevel`): a STORED session's bottom bar
  // is POPULATED (the selectors render disabled — a stored session can't
  // set config; a resume re-emits the fresh values).
  const configOptions = useSessions((s) => {
    const id = s.activeSessionId;
    if (!id) return null;
    return (
      s.configOptions[id] ??
      s.sessions.find((x) => x.sessionId === id)?.configOptions ??
      s.historySessions.find((x) => x.sessionId === id)?.configOptions ??
      s.archivedSessions.find((x) => x.sessionId === id)?.configOptions ??
      null
    );
  });
  // The session's context usage (the context-percentage display — from
  // the harness's `context_usage_update` frames; the store's map first,
  // falling back to the session row's `contextUsage` — a STORED
  // session's last known usage, persisted on every frame): `undefined`
  // until the first frame, so a fresh session shows no percentage.
  const contextUsage = useSessions((s) => {
    const id = s.activeSessionId;
    if (!id) return undefined;
    return (
      s.contextUsage[id] ??
      s.sessions.find((x) => x.sessionId === id)?.contextUsage ??
      s.historySessions.find((x) => x.sessionId === id)?.contextUsage ??
      s.archivedSessions.find((x) => x.sessionId === id)?.contextUsage
    );
  });
  const contextPercent =
    contextUsage && contextUsage.window > 0
      ? Math.min(100, Math.round((contextUsage.used / contextUsage.window) * 100))
      : undefined;
  // The context bar's color ramp (the traffic-light scheme — the label
  // follows the fill's color): green at 0–49%, caution at 50–69%, warning
  // at 70–89%, the destructive red at 90–100%. Context pressure IS a status
  // ramp, so every band rides a semantic token (no raw hue to stand beside a
  // Palette's own yellow); `caution` is the band `success` and `warning` have
  // no room for.
  const contextRamp =
    contextPercent === undefined
      ? { fill: "bg-success", label: "text-success" }
      : contextPercent < 50
        ? { fill: "bg-success", label: "text-success" }
        : contextPercent < 70
          ? { fill: "bg-caution", label: "text-caution" }
          : contextPercent < 90
            ? { fill: "bg-warning", label: "text-warning" }
            : { fill: "bg-destructive", label: "text-destructive" };
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

  // --- The three-prefix mention picker (`$` skills / `#` MCP servers /
  // `@` agents) + the send-path expansion. ---
  // ALL of the hooks below live in the UNCONDITIONAL top block (before the
  // `!activeSessionId` early return further down) — placing any of them after
  // the early return would change the hook count across the session/no-session
  // transition and crash React (the same bug the `usePendingSubagentRequests`
  // comment above warns about). Plain (non-hook) functions like `selectMention`
  // are placement-flexible (the file's own comment says so for the attachment
  // handlers) and live with the other handlers below.
  // The Space path is the active session's `cwd` (CONTEXT.md: a Session's
  // `cwd` IS the Space's folder) — derivation UNCHANGED from the `$`-only
  // picker: live first, then stored (the same derivation as Task 4's
  // `SpacesList`). `useMentionCatalogs` caches per Space, so the composer and
  // the left pane share ONE skills fetch (same key — the skills row reuses
  // `useSkillCatalog` verbatim).
  const spacePath = liveSession?.cwd ?? historySession?.cwd ?? null;
  // The MCP catalog FETCH is deliberately NOT gated on `mcpMentionsEnabled`
  // (a cheap config read; gating it would make a live toggle need a remount).
  const { skills, agents, mcpServers } = useMentionCatalogs(spacePath);
  // The `$`/`#`/`@`-trigger picker state (the active token (incl. its prefix)
  // + the highlighted row).
  const [picker, setPicker] = useState<{
    prefix: "$" | "#" | "@";
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
  const filtered = useMemo(() => {
    // ONE picker, filtered to the ACTIVE prefix's catalog only (a `$`/`#`/`@`
    // never mixes in one open list): map the matching catalog to `MentionRow`
    // (skills/agents carry `description`, the MCP servers carry their one-line
    // `summary`).
    const prefix = picker?.prefix ?? "$";
    // The `#` gate: with the trigger off, the MCP catalog is EMPTY, so `#`
    // yields no rows → the picker renders nothing and the keyboard branches
    // (gated on `filtered.length > 0`) never intercept a key — Enter still
    // sends. `mcpMentionsEnabled` is a DEP below, so flipping the setting
    // re-derives the rows LIVE (no remount, and the catalog FETCH is not
    // gated — see the `mcpMentionsEnabled` note above).
    const mcpCatalog = mcpMentionsEnabled ? mcpServers : [];
    const rows: MentionRow[] =
      prefix === "#"
        ? mcpCatalog.map((s) => ({
            key: s.name,
            prefix: "#",
            name: s.name,
            description: s.summary,
          }))
        : prefix === "@"
          ? agents.map((a) => ({
              key: a.name,
              prefix: "@",
              name: a.name,
              description: a.description,
            }))
          : skills.map((s) => ({
              key: s.name,
              prefix,
              name: s.name,
              description: s.description,
            }));
    return rows.filter((r) =>
      r.name.toLowerCase().includes((picker?.query ?? "").toLowerCase()),
    );
  }, [skills, agents, mcpServers, mcpMentionsEnabled, picker]);
  // The highlighted row index, DERIVED (not clamped in place): the raw
  // `picker.index` can go stale (a Space switch refetches `skills` while the
  // picker is open with a non-zero `index`), and `filtered[staleIndex]` would
  // be `undefined` → a `selectMention(undefined)` crash on Enter (the rows are
  // `MentionRow`s). `Math.max(0, …)` is belt-and-braces: the picker UI and the keyboard branch are both guarded
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
      // Close the mention picker: the insert comes from OUTSIDE the
      // picker (a SkillsDialog row — the v1.1 flow), so a picker open at
      // insert time is stale (it would linger rendered until the next
      // keydown, and a token active at the caret would be spliced INTO).
      // The dialog inserts a `$`-prefixed SKILL token, so the picker this
      // closes is the skills one (`$` is the skill prefix).
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
  // The composer's SEND is enabled for a live session OR a RESUMABLE
  // stored session (a non-resumable stored session can't receive a prompt
  // — no agent process to deliver to, and the harness's `send_prompt`
  // rejects an unknown session): a resumable stored session auto-resumes
  // on send. The TEXTAREA is always editable regardless (the draft is
  // kept — it just can't be SENT while `composerEnabled` is false).
  const composerEnabled = isLive || canResume;

  // The session's `SpaceView` + the `New Session in this Space` action
  // + the side-pane toggle moved to `SpaceTabs` (the top tab bar — the
  // old header row is gone: the session title lives in the side pane
  // now, and the `Live` conversation selector is replaced by the tabs).
  // Auto-scroll to the bottom as new content streams in.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, prompts.length, stackedAskRequests.length]);

  if (!activeSessionId) {
    return (
      <main className="m-1 flex min-w-0 min-h-0 flex-1 items-center justify-center rounded-xl bg-chat">
        <p className="text-ui-base text-foreground-subtle">
          No active session — open a Space from the tabs above
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
      // EXPANSION: expand `$` / `@` / `#` mentions into their block forms
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
      // The `#` gate (see the `mcpMentionsEnabled` note above): an EMPTY MCP
      // catalog means a `#` token matches nothing, so the text stays
      // BYTE-IDENTICAL — the existing verbatim path, not a special case.
      const text = expandMentions(rawText, {
        skills,
        agents,
        mcpServers: mcpMentionsEnabled ? mcpServers : [],
      });
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

  // The conversation selector + the session title + the space chip moved
  // out of the header: the Spaces are the TOP TABS (`SpaceTabs` — one
  // tab per space, the active one selected) and the session title lives
  // in the side pane (its top section).

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

  // The `+` button (the native file picker): the dialog returns PATHS (a
  // webview `File` is not available for a picked file — no `fs` plugin),
  // so the bytes are read via `read_file_bytes` (the Rust command — the
  // image-extension allowlist + the 10 MiB cap re-validated server-side),
  // rebuilt into `File`s (the MIME inferred from the extension), and
  // staged through the SAME `stageFiles` path as a paste / drop (the
  // capability gate + the attachment caps apply identically).
  const handleAttachClick = async () => {
    if (!composerEnabled || composerLocked || !imageCapable) return;
    let raw: string | string[] | null;
    try {
      raw = await openFileDialog({
        multiple: true,
        // The image allowlist (the SAME set as the backend command — the
        // dialog filters, the command re-validates, `stageFiles` re-validates
        // the MIME a third time).
        filters: [
          {
            name: "Images",
            extensions: ["png", "jpg", "jpeg", "gif", "webp"],
          },
        ],
      });
    } catch {
      return; // the dialog is unavailable (e.g. a non-Tauri context)
    }
    if (raw == null) return; // cancelled
    const paths = Array.isArray(raw) ? raw : [raw];
    const files: File[] = [];
    let readError: string | null = null;
    for (const path of paths) {
      try {
        const bytes = await readFileBytes(path);
        if (!bytes || bytes.length === 0) continue; // not an image (the backend allowlist)
        files.push(
          new File([new Uint8Array(bytes)], basenameOfPath(path), {
            type: inferAttachmentMimeType(path),
          }),
        );
      } catch (e) {
        // A read failure (e.g. the file over the 10 MiB cap — the command
        // rejects): keep the readable selections staged, surface the error
        // only when NOTHING was staged.
        readError = e instanceof Error ? e.message : String(e);
      }
    }
    if (files.length > 0) {
      stageFiles(files);
    } else if (readError) {
      setError(readError);
    }
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

  // Select a mention from the `$`/`#`/`@`-trigger picker: replace the active
  // token (the `$`/`#`/`@`-prefixed span at the caret — the shared
  // `activeMentionToken` helper) with `<prefix><name> ` (LOWER-cased — the
  // case policy: a picker-selected mention ALWAYS expands on send, and the
  // mention regex is lowercase-only; the expansion keeps the catalog name
  // verbatim in the block's `name` attribute). The `requestAnimationFrame`
  // re-focus + caret-set is REQUIRED: `setDraft` re-renders the controlled
  // textarea, which would otherwise drop focus/caret.
  const selectMention = (row: MentionRow) => {
    if (!picker) return;
    const el = composerRef.current;
    const caret = el?.selectionStart ?? draft.length;
    const token = activeMentionToken(draft, caret);
    if (!token) return;
    const inserted = `${row.prefix}${row.name.toLowerCase()} `;
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
    // THE CENTER COLUMN IS THREE ISLANDS, NOT ONE (the composer used to live
    // INSIDE the `main`). Chrome (`#191a21`) frames each island, and the gap
    // between islands IS the separation: two surfaces of the SAME fill read as
    // distinct when they are SIBLINGS with chrome between them, and read as ONE
    // surface when one is nested in the other. That distinction is why the
    // composer is a sibling here — as a child of a `bg-chat` main it
    // would have had to be a different colour to be visible at all, and as a
    // sibling it wears its own page-plane fill (`#21222c`) while the transcript
    // beside it is `#282a36`, and the two still read as distinct islands.
    // `rounded-xl` + `m-1` match the transcript and the side
    // pane exactly, so the three islands are one visual family.
    //
    // The `min-h-0` on the `main` is still LOAD-BEARING (the classic flexbox
    // `min-height: auto` trap): it is a `flex-1` item of the center column (a
    // `flex-col`) with `overflow: visible`, so its automatic minimum size is the
    // CONTENT's height — without `min-h-0` a long transcript grows the `main`
    // past the column and the inner `overflow-y-auto` div (owned by
    // `MessageList`, NOT the composer) never scrolls. Moving the composer out
    // does not relax this: the transcript is still the tall child.
    <>
      <main className="m-1 flex min-w-0 min-h-0 flex-1 flex-col rounded-xl bg-chat">
      {/* The old header row (session title + space chip + `Live`
          conversation selector + `...` menu + side-pane toggle) is gone:
          the Spaces are the top TABS (`SpaceTabs`, above this component —
          the `...` menu + the toggle live on that row) and the session
          title lives in the side pane (its top section). */}
      {/* The stalled-session banner (a Worker crash — the `session-stalled`
          event set the store's `stalled` entry): above the transcript, below
          the (removed) header. `null` when the session is not stalled. */}
      {activeSessionId && <SessionStalledBanner sessionId={activeSessionId} />}
      <MessageList
        scrollRef={scrollRef}
        units={units}
        messageCount={messages.length}
        activeSessionId={activeSessionId}
        inTurn={inTurn}
        askRequests={askRequests}
        prompts={prompts}
        stackedAskRequests={stackedAskRequests}
        hasPendingRequest={hasPendingRequest}
        workingOrInTurn={workingOrInTurn}
        agentState={agentState}
        turnDiffs={turnDiffs}
        stopReason={stopReason}
      />

      {error && (
        <p className="mb-2 px-3 text-ui-sm text-destructive">
          {error}
        </p>
      )}
      <WorkingIndicator
        agentState={agentState}
        workingOrInTurn={workingOrInTurn}
        spinnerStyle={spinnerStyle}
        quip={quip}
      />
      </main>
      {/* The composer island (see the column comment): a SIBLING of the
          transcript, not its child. It stays LAST in DOM order, so tab order
          and the "composer is at the bottom" reading order are unchanged. */}
      <ComposerRow
        composerRef={composerRef}
        draft={draft}
        setDraft={setDraft}
        picker={picker}
        setPicker={setPicker}
        filtered={filtered}
        activeIndex={activeIndex}
        selectMention={selectMention}
        attachments={attachments}
        removeAttachment={removeAttachment}
        handlePaste={handlePaste}
        handleDrop={handleDrop}
        handleAttachClick={handleAttachClick}
        send={send}
        composerEnabled={composerEnabled}
        composerLocked={composerLocked}
        imageCapable={imageCapable}
        hasImages={hasImages}
        contextPercent={contextPercent}
        contextRamp={contextRamp}
        contextUsage={contextUsage}
        modelOption={modelOption}
        thinkingOption={thinkingOption}
        setConfigValue={setConfigValue}
        isLive={isLive}
      />
      {/* Interactive modals (rendered at the `ChatStream` root — `fixed`
          overlays, NOT inside the scroll region). */}
      <ComposerModals
        activeSessionId={activeSessionId}
        confirmRequests={confirmRequests}
        passwordRequests={passwordRequests}
      />
    </>
  );
}
