import { ArrowUp, Brain, Plus } from "lucide-react";
import type { RefObject } from "react";
import { Button } from "../ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "../ui/tooltip";
import SessionConfigSelect from "../SessionConfigSelect";
import AttachmentStrip from "./AttachmentStrip";
import ComposerMentions, { type MentionRow } from "./ComposerMentions";
import { activeMentionToken } from "../../lib/skills";
import type { ChatComposerAttachment } from "../../lib/chatAttachments";
import type { SessionConfigOption } from "../../lib/tauri";

/**
 * The composer: the outer rounded box (drop target), the three-prefix
 * mention picker (`$` skills / `#` MCP servers / `@` agents), the
 * staged-attachment strip, the controlled textarea (with the three-prefix
 * mention-token `onChange` / `onKeyDown` logic — `$` skills / `#` MCP servers /
 * `@` agents) and the bottom toolbar row (the
 * `+` attach button, the context-usage bar, the model / thinking selectors
 * and the Send button).
 *
 * Props-only by design — ALL state stays in `ChatStream` (the `draft`,
 * `picker` and `attachments` state, the `composerRef` the auto-grow and
 * skill-insert effects need, `send`, and the attachment handlers). The
 * `onChange` / `onKeyDown` closures live here VERBATIM (they are pure given
 * these props: `activeMentionToken` is a free function, the `draft` / `picker`
 * setters and the `send` / `selectMention` callbacks are props). Extracted
 * verbatim from `ChatStream` (identical DOM).
 */
export default function ComposerRow({
  composerRef,
  draft,
  setDraft,
  picker,
  setPicker,
  filtered,
  activeIndex,
  selectMention,
  attachments,
  removeAttachment,
  handlePaste,
  handleDrop,
  handleAttachClick,
  send,
  composerEnabled,
  composerLocked,
  imageCapable,
  hasImages,
  contextPercent,
  contextRamp,
  contextUsage,
  modelOption,
  thinkingOption,
  setConfigValue,
  isLive,
}: {
  composerRef: RefObject<HTMLTextAreaElement | null>;
  draft: string;
  setDraft: (value: string) => void;
  picker: { prefix: "$" | "#" | "@"; query: string; index: number } | null;
  setPicker: (
    picker: { prefix: "$" | "#" | "@"; query: string; index: number } | null,
  ) => void;
  filtered: MentionRow[];
  activeIndex: number;
  selectMention: (row: MentionRow) => void;
  attachments: ChatComposerAttachment[];
  removeAttachment: (id: string) => void;
  handlePaste: (e: React.ClipboardEvent<HTMLTextAreaElement>) => void;
  handleDrop: (e: React.DragEvent) => void;
  handleAttachClick: () => Promise<void>;
  send: () => Promise<void>;
  composerEnabled: boolean;
  composerLocked: boolean;
  imageCapable: boolean;
  hasImages: boolean;
  contextPercent: number | undefined;
  contextRamp: { fill: string; label: string };
  contextUsage: { used: number; window: number } | undefined;
  modelOption: SessionConfigOption | undefined;
  thinkingOption: SessionConfigOption | undefined;
  setConfigValue: (optionId: string, value: string) => Promise<void>;
  isLive: boolean;
}) {
  // The token counts as TEXT (the number is what the bar shows; these are
  // what the assistive tech and the tooltip carry). `undefined` until the
  // first usage frame — the same gate as the label itself.
  const countsText =
    contextUsage && contextPercent !== undefined
      ? `${contextUsage.used.toLocaleString()} of ${contextUsage.window.toLocaleString()} tokens (${contextPercent}%)`
      : undefined;
  const countsLabel = countsText ? `Context used: ${countsText}` : undefined;
  return (
    <div
      data-testid="composer-island"
      // `shrink-0` is LOAD-BEARING. The island is now a flex ITEM of the center
      // column (it used to be a child of the `main`). Flex shrink is weighted by
      // flex-basis, and the `main` has `flex-1` (basis 0) while this has
      // `basis: auto` — so under pressure the `main` cannot shrink at all and
      // the composer absorbs the entire shortfall (a long transcript squashes
      // the input to nothing instead of scrolling).
      className="relative m-1 shrink-0 rounded-xl bg-composer p-3"
      onDrop={handleDrop}
    >
      <ComposerMentions
        open={picker !== null}
        filtered={filtered}
        activeIndex={activeIndex}
        onSelect={selectMention}
      />
      <AttachmentStrip attachments={attachments} onRemove={removeAttachment} />
      <textarea
        ref={composerRef}
        value={draft}
        onChange={(e) => {
          // The `$`/`#`/`@`-trigger: a bare prefix (empty token remainder)
          // opens the picker with the FULL list for that prefix, any
          // non-token span closes it.
          const token = activeMentionToken(
            e.target.value,
            e.target.selectionStart ?? e.target.value.length,
          );
          setDraft(e.target.value);
          setPicker(
            token
              ? { prefix: token.prefix, query: token.remainder, index: 0 }
              : null,
          );
        }}
        onPaste={handlePaste}
        onKeyDown={(e) => {
          // Re-evaluate the active token at KEYDOWN time (the caret is on
          // the event's target): ArrowLeft/Right, Home/End, and a mouse
          // click move the caret WITHOUT `onChange`, so the `picker` state
          // (only recomputed in `onChange`) can be STALE — the caret may
          // no longer be on the token.
          const el = e.currentTarget;
          const token = activeMentionToken(
            el.value,
            el.selectionStart ?? el.value.length,
          );
          if (picker && !token) {
            // The caret left the token: CLOSE the picker instead of
            // `selectMention`'s silent early-return — a stale picker must
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
              selectMention(filtered[activeIndex]!);
              return;
            }
            if (e.key === "Tab" && !e.shiftKey) {
              e.preventDefault();
              selectMention(filtered[activeIndex]!);
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
        // The SINGLE static placeholder (the state if/else is gone — the
        // working indicator carries the working state, and a closed
        // session's send auto-resumes it, so "follow-up changes" is
        // accurate there too).
        placeholder="Ask for follow-up changes"
        rows={2}
        className="max-h-32 w-full resize-none overflow-y-auto bg-transparent text-ui-base outline-none placeholder:text-foreground-subtlest"
      />
      <div className="mt-1 flex items-center gap-2">
        {/* The `+` attach button (the native file picker — the same
            staging path as a paste / drop; disabled while the composer
            is locked or the agent doesn't advertise image support,
            fail-closed). */}
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Attach files"
          disabled={!composerEnabled || composerLocked || !imageCapable}
          title={
            imageCapable
              ? "Attach an image (file picker)"
              : "The agent does not support images"
          }
          onClick={() => void handleAttachClick()}
        >
          <Plus className="size-4" />
        </Button>
        {/* The context bar (the dynamic percentage — a progress bar
            spanning from the `+` button to the model selector, the
            reference UI's `🧠 [====] 61%` look). ALWAYS rendered (the
            `isLive` gate is gone): a live session's fill appears as the
            session grows, the label waits for the first frame, and a
            closed session (the context is dropped on close — re-emitted
            on resume) shows the empty 0% track. */}
        <div
            className="flex min-w-0 flex-1 items-center gap-2"
            data-testid="context-usage-bar"
          >
            <Brain
              className="size-3.5 shrink-0 text-foreground-subtle"
              aria-hidden
              data-testid="context-bar-icon"
            />
            {/* The counts ride the progressbar as its TEXT alternative
                (`aria-valuetext`) — a bare percentage is what the bar
                draws, but the numbers are the useful part and a
                progressbar exposes only its value. */}
            {/* The track is its OWN token (`--color-context-track`), not the
                subtlest-TEXT token it used to borrow. The bar's whole meaning is
                the band FILL (green → caution → warning → red) and the track is
                the only thing separating that fill from the unfilled remainder;
                under Dracula the borrowed Comment hue measured 3.43 / 4.21 /
                2.76 / 1.73 against the four fills this component emits, so the
                red band was almost invisible on the bar it was drawn on. Every
                palette but Dracula aliases the new token straight back to
                `foreground-subtlest`, so zai renders EXACTLY as it always has.
                The per-palette ratios are pinned in the context-bar gate in
                `paletteCompleteness.test.ts`. */}
            <div
              role="progressbar"
              aria-label="Context used"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={contextPercent ?? 0}
              aria-valuetext={countsText}
              className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-context-track"
            >
              <div
                className={`h-full rounded-full transition-[width] duration-500 ${contextRamp.fill}`}
                style={{ width: `${contextPercent ?? 0}%` }}
              />
            </div>
            {contextPercent !== undefined && contextUsage && (
              <TooltipProvider>
                <Tooltip>
                  {/* The trigger is a BUTTON, not a `<span>`: Radix opens
                      the tooltip on focus, so the counts are
                      keyboard-reachable only if the trigger is a tab stop;
                      and they ride in the accessible NAME, so they are
                      heard without opening the tooltip at all. Visuals are
                      unchanged — the reset classes keep the bare-number
                      look. */}
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      data-testid="context-usage"
                      aria-label={countsLabel}
                      className={`shrink-0 cursor-help appearance-none rounded-sm border-0 bg-transparent p-0 text-ui-sm tabular-nums focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring ${contextRamp.label}`}
                    >
                      {contextPercent}%
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="top" align="end">
                    {contextUsage.used.toLocaleString()} /{" "}
                    {contextUsage.window.toLocaleString()} tokens ·{" "}
                    {contextPercent}% of context
                  </TooltipContent>
                </Tooltip>
              </TooltipProvider>
            )}
        </div>
        {/* ZCode's composer carries the config controls in its toolbar
            (left of the send button) — the header does not. ALWAYS
            rendered (the `isLive` gate is gone): a live session shows
            the live values, a stored session shows the POPULATED
            values (the store's kept entry, else the row's synthesized
            `configOptions` / persisted `contextUsage`) with DISABLED
            selectors (a stored session can't set config — a resume
            re-emits the fresh values).
            The MODEL + THINKING pair is a GROUP with a tighter inner gap
            (`gap-1`): they are one cluster of session config, and the
            roomier `gap-2` now only separates the cluster from Send. */}
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <div
            className="flex min-w-0 items-center gap-1"
            data-testid="composer-config-controls"
          >
            <SessionConfigSelect kind="model" option={modelOption ?? null} onSet={setConfigValue} disabled={!isLive} />
            <SessionConfigSelect kind="thinking" option={thinkingOption ?? null} onSet={setConfigValue} disabled={!isLive} />
          </div>
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
  );
}
