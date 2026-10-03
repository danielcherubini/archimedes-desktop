import { memo, useState, useEffect } from "react";
import { Bot, ChevronDownIcon } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  SelectGroup,
  SelectLabel,
} from "../components/ui/select";
import ModelPicker from "./ModelPicker";
import { modelItemsFromCatalog } from "./ModelPickerDialog";
import { useSettings } from "../store/settings";
import { SessionConfigOption } from "../lib/tauri";
import { THINKING_GLYPHS } from "../lib/toolOutput";
import { Button } from "./ui/button";

type OnSet = (optionId: string, value: string) => Promise<void>;

/**
 * The props: a REAL option (the `kind` is optional — it falls back to the
 * option's `category` / `id`) OR a `null` option (a closed session's config
 * is dropped — the `kind` is REQUIRED then, since there is no option to
 * derive it from). The `null` case renders a disabled stub (the control's
 * identity, no data — the config re-emits on resume).
 */
type SessionConfigSelectProps =
  | { option: SessionConfigOption; onSet: OnSet; kind?: "model" | "thinking"; disabled?: boolean }
  | { option: null; kind: "model" | "thinking"; onSet: OnSet; disabled?: boolean };

/**
 * The thinking-level indicator (the pi-archimedes footer's
 * `thinkingLevelIcons` + `thinkingLevelColors` ramp, ported to the design
 * system): the GLYPH's fill is the level's magnitude (○ off/minimal,
 * ◔ low 25%, ◑ medium 50%, ◕ high 75%, ● xhigh/max full — the app's own
 * `THINKING_GLYPHS`, the same source), the HUE is the level's identity
 * (gray → blue → indigo → purple → pink → red). An empty / unknown level
 * is the open circle in the dim color (the reference's `off` state).
 */
function thinkingLevelIndicator(
  level: string,
): { glyph: string; className: string } {
  const l = level.toLowerCase();
  // `THINKING_GLYPHS` covers every known level (off/minimal → ○); the empty
  // value (no level set — the model default) is the open circle too, and an
  // unknown level falls back to the mid glyph (the `formatThinkingIndicator`
  // fallback).
  const glyph = l === "" ? "○" : THINKING_GLYPHS[l] ?? "◑";
  const className =
    l === "low"
      ? "text-blue-500"
      : l === "medium"
        ? "text-indigo-500"
        : l === "high"
          ? "text-purple-500"
          : l === "xhigh"
            ? "text-pink-500"
            : l === "max"
              ? "text-red-500"
              : "text-foreground-subtle";
  return { glyph, className };
}

function SessionConfigSelect({ option, kind, onSet, disabled }: SessionConfigSelectProps) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The base disabled state (the call site — a STORED session's selector
  // is populated but read-only: no agent process to deliver
  // `set_config_option` to) OR the in-flight `onSet` (one change at a
  // time). Both disable the trigger.
  const isDisabled = disabled || pending;
  // The settings document — the model picker's provider display names
  // (the `tama` key → the configured provider's `name` `Tama`; the raw
  // key when the provider is unconfigured). `null` (not loaded yet) →
  // the raw keys.
  const settings = useSettings((s) => s.settings);

  useEffect(() => {
    if (error) {
      const timer = setTimeout(() => setError(null), 5000);
      return () => clearTimeout(timer);
    }
  }, [error]);

  const value = typeof option?.currentValue === "string" ? option.currentValue : "";
  // The two controls' indicators (the reference UI's placement): the model
  // picker carries the bot icon (the reference's robot glyph — the web
  // equivalent of the Nerd Font codepoint), the thinking-level picker
  // carries the level's fill-ramp glyph (the pi-archimedes
  // `thinkingLevelIcons` ramp — the fill is the magnitude, the hue the
  // level's identity). The `kind` prop (the call site knows which option
  // it looked up) wins over the option's own `category` / `id` (the
  // fallback — a `null` option has none to derive from).
  const isThinking =
    kind === "thinking" ||
    option?.category === "thought_level" ||
    option?.id === "thought_level";
  const isModel =
    kind === "model" || option?.category === "model" || option?.id === "model";
  // A `null` option (a closed session's config is dropped — re-emitted on
  // resume): render a DISABLED stub (the control's identity, no data).
  if (option === null) {
    if (isModel) {
      return (
        <div className="relative">
          <ModelPicker
            label="Model"
            value=""
            placeholder="Model"
            items={[]}
            onSelect={() => {}}
            variant="ghost"
            triggerClassName="max-w-48"
            disabled
            icon={
              <Bot
                className="size-3.5 text-foreground-subtle"
                aria-hidden
                data-testid="model-icon"
              />
            }
          />
        </div>
      );
    }
    return (
      <div className="relative">
        <Button
          variant="ghost"
          size="sm"
          className="max-w-48 justify-start"
          aria-label="Thinking"
          disabled
        >
          <span
            className="text-ui-sm text-foreground-subtle"
            aria-hidden
            data-testid="thinking-level-icon"
          >
            ○
          </span>
          <span className="min-w-0 flex-1 truncate text-left text-foreground-subtlest">
            Thinking
          </span>
          <ChevronDownIcon
            className="size-3.5 shrink-0 text-foreground-subtle"
            aria-hidden
          />
        </Button>
      </div>
    );
  }
  const thinkingIndicator = thinkingLevelIndicator(value);
  // The model picker's items (the SHARED derivation — `ModelPicker` is
  // the one component every model picker uses): the config option's
  // options flattened to their VALUES (the Rust
  // `synthesize_catalog_config_options` shape — flat `{ value:
  // "provider/id" }` entries; grouped entries flattened too) → the row
  // name is the FULL `provider/id` value + the provider's display name
  // as the muted-grey inline cue (the `Qwen/Qwen3.8-27B (Tama)` shape).
  const modelItems = modelItemsFromCatalog(
    (option.options ?? []).flatMap((opt) =>
      "options" in opt ? opt.options.map((sub) => sub.value) : [opt.value],
    ),
    settings?.providers ?? [],
  );

  const handleValueChange = async (val: string) => {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await onSet(option.id, val);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  };

  // The MODEL option: the shared `ModelPicker` (a ghost trigger — the
  // bot icon + the current value + a chevron — opening the model picker
  // dialog; the catalog is too long for a Radix dropdown). The
  // thinking-level option stays a compact Radix Select (a handful of
  // options).
  if (isModel) {
    return (
      <div className="relative">
        <ModelPicker
          label={option.name}
          value={value}
          items={modelItems}
          onSelect={(val) => void handleValueChange(val)}
          variant="ghost"
          triggerClassName="max-w-48"
          disabled={isDisabled}
          icon={
            <Bot
              className="size-3.5 text-foreground-subtle"
              aria-hidden
              data-testid="model-icon"
            />
          }
        />
        {error && (
          <span className="text-ui-sm text-destructive absolute left-0 top-full mt-1 z-10 rounded bg-background px-1" role="alert">
            {error}
          </span>
        )}
      </div>
    );
  }

  // The THINKING-LEVEL option: the compact Radix Select (unchanged).
  return (
    <div className="relative">
      <Select value={value} onValueChange={handleValueChange}>
        <SelectTrigger
          variant="ghost"
          size="sm"
          className="max-w-48"
          disabled={isDisabled}
          aria-label={option.name}
        >
          {/* The model option never reaches this branch (it returns the
              dialog trigger above) — only the thinking-level icon (or
              none) renders here. */}
          {isThinking && (
            <span
              className={`text-ui-sm ${thinkingIndicator.className}`}
              aria-hidden
              data-testid="thinking-level-icon"
            >
              {thinkingIndicator.glyph}
            </span>
          )}
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {option.options?.map((opt) => {
            if ("options" in opt) {
              return (
                <SelectGroup key={opt.name}>
                  <SelectLabel>{opt.name}</SelectLabel>
                  {opt.options.map((subOpt) => (
                    <SelectItem key={subOpt.value} value={subOpt.value}>
                      {subOpt.name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              );
            }
            return (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.name}
              </SelectItem>
            );
          })}
        </SelectContent>
      </Select>
      {error && (
        <span className="text-ui-sm text-destructive absolute left-0 top-full mt-1 z-10 rounded bg-background px-1" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

/**
 * MEMO IS THE FIX (perf regression): the composer's `draft` state re-renders
 * `ChatStream` on every keystroke; without this memo (and a stable `onSet`),
 * every keystroke re-rendered the FULL model catalog mounted inside the
 * selects (~600 items × ~8 fibers ≈ 140ms/keystroke — typing lagged a full
 * minute behind). With stable `option`/`onSet` references the subtree is
 * skipped entirely; a new `option` object (config applied) still re-renders.
 */
export default memo(SessionConfigSelect);
