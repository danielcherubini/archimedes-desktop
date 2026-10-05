import { ArrowUp, Brain, Plus } from "lucide-react";
import type { RefObject } from "react";
import { Button } from "../ui/button";
import SessionConfigSelect from "../SessionConfigSelect";
import AttachmentStrip from "./AttachmentStrip";
import ComposerSkills from "./ComposerSkills";
import { activeSkillToken } from "../../lib/skills";
import type { ChatComposerAttachment } from "../../lib/chatAttachments";
import type { SessionConfigOption, SkillInfo } from "../../lib/tauri";

/**
 * The composer: the outer rounded box (drop target), the `$`-trigger skill
 * picker, the staged-attachment strip, the controlled textarea (with the
 * `$`-token `onChange` / `onKeyDown` logic) and the bottom toolbar row (the
 * `+` attach button, the context-usage bar, the model / thinking selectors
 * and the Send button).
 *
 * Props-only by design — ALL state stays in `ChatStream` (the `draft`,
 * `picker` and `attachments` state, the `composerRef` the auto-grow and
 * skill-insert effects need, `send`, and the attachment handlers). The
 * `onChange` / `onKeyDown` closures live here VERBATIM (they are pure given
 * these props: `activeSkillToken` is a free function, the `draft` / `picker`
 * setters and the `send` / `selectSkill` callbacks are props). Extracted
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
  selectSkill,
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
  picker: { query: string; index: number } | null;
  setPicker: (
    picker: { query: string; index: number } | null,
  ) => void;
  filtered: SkillInfo[];
  activeIndex: number;
  selectSkill: (skill: SkillInfo) => void;
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
  return (
    <div
      className="relative m-3 rounded-2xl border border-input-border bg-input p-3 transition-colors hover:border-input-border-hover focus-within:border-input-border-focused focus-within:bg-input-focused"
      onDrop={handleDrop}
    >
      <ComposerSkills
        open={picker !== null}
        filtered={filtered}
        activeIndex={activeIndex}
        onSelect={selectSkill}
      />
      <AttachmentStrip attachments={attachments} onRemove={removeAttachment} />
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
            <div
              role="progressbar"
              aria-label="Context used"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={contextPercent ?? 0}
              className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-foreground-subtlest"
            >
              <div
                className={`h-full rounded-full transition-[width] duration-500 ${contextRamp.fill}`}
                style={{ width: `${contextPercent ?? 0}%` }}
              />
            </div>
            {contextPercent !== undefined && contextUsage && (
              <span
                data-testid="context-usage"
                className={`shrink-0 text-ui-sm tabular-nums ${contextRamp.label}`}
                title={`${contextPercent}% of context used (${contextUsage.used.toLocaleString()} / ${contextUsage.window.toLocaleString()} tokens)`}
              >
                {contextPercent}%
              </span>
            )}
        </div>
        {/* ZCode's composer carries the config controls in its toolbar
            (left of the send button) — the header does not. ALWAYS
            rendered (the `isLive` gate is gone): a live session shows
            the live values, a stored session shows the POPULATED
            values (the store's kept entry, else the row's synthesized
            `configOptions` / persisted `contextUsage`) with DISABLED
            selectors (a stored session can't set config — a resume
            re-emits the fresh values). */}
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <SessionConfigSelect kind="model" option={modelOption ?? null} onSet={setConfigValue} disabled={!isLive} />
          <SessionConfigSelect kind="thinking" option={thinkingOption ?? null} onSet={setConfigValue} disabled={!isLive} />
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
