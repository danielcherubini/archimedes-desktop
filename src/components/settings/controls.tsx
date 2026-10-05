import { useEffect, useState } from "react";
import type { ReactElement, ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { BrailleLoader } from "@/components/ui/braille-loader";
import { brailleLoaderVariants } from "@/lib/braille-loader";
import { cn } from "@/components/lib/utils";
import { humanizeVariant } from "./primitives";
/**
 * The ZCode `FontSizeInput` pattern: a `w-28` number `Input` (12–20,
 * `px` suffix) with a local draft — commit on blur/Enter (clamped),
 * Escape cancels the draft.
 */
export function FontSizeInput({
  value,
  min,
  max,
  ariaLabel,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  ariaLabel: string;
  onChange: (value: number) => void;
}): ReactElement {
  const [draft, setDraft] = useState(String(value));
  const commit = () => {
    const parsed = draft.trim() === "" ? Number.NaN : Number(draft);
    const next = Number.isFinite(parsed)
      ? Math.min(max, Math.max(min, Math.round(parsed)))
      : value;
    setDraft(String(next));
    if (next !== value) onChange(next);
  };
  return (
    <div className="relative w-28">
      <Input
        type="number"
        inputMode="numeric"
        min={min}
        max={max}
        step={1}
        value={draft}
        aria-label={ariaLabel}
        onChange={(event) => setDraft(event.currentTarget.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.currentTarget.blur();
          } else if (event.key === "Escape") {
            event.preventDefault();
            setDraft(String(value));
          }
        }}
        className="pr-8"
      />
      <span className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-ui-sm text-foreground-subtlest">
        px
      </span>
    </div>
  );
}

/**
 * The thinking-spinner picker (the `Appearance` section): a `4`-column
 * radiogroup grid of the FULL `braille-loader` gallery — every variant as
 * a LIVE `BrailleLoader` preview (an animated 2×N braille block) + its
 * humanized name. A cell picks its variant immediately (the page's
 * immediate-save pattern). The `aria-label` is the humanized name (it
 * wins over the cell's content for naming — the visible label stays a
 * plain visual duplicate).
 */
export function SpinnerStylePicker({
  value,
  onChange,
  ariaLabel,
}: {
  value: string;
  onChange: (variant: string) => void;
  ariaLabel?: string;
}): ReactElement {
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel ?? "Thinking spinner"}
      className="grid grid-cols-4 gap-1.5"
    >
      {brailleLoaderVariants.map((variant) => {
        const selected = value === variant;
        const name = humanizeVariant(variant);
        return (
          <button
            key={variant}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-label={name}
            onClick={() => onChange(variant)}
            className={cn(
              "flex flex-col items-center gap-1 rounded-md border px-2 py-1.5 transition-colors",
              selected
                ? "border-primary bg-primary/10"
                : "border-border hover:bg-muted",
            )}
          >
            <BrailleLoader
              variant={variant}
              speed="normal"
              fontSize={14}
              label={name}
            />
            <span className="text-[10px] leading-none text-foreground-subtle">
              {name}
            </span>
          </button>
        );
      })}
    </div>
  );
}

/**
 * A provider text field (immediate save): a local draft that commits on
 * blur/Enter ONLY when the value actually changed (a no-change blur saves
 * nothing and re-runs no discovery).
 */
export function TextField({
  value,
  placeholder,
  ariaLabel,
  className,
  type = "text",
  onCommit,
}: {
  value: string;
  placeholder?: string;
  ariaLabel: string;
  className?: string;
  type?: "text" | "password";
  onCommit: (value: string) => void;
}): ReactElement {
  const [draft, setDraft] = useState(value);
  // Re-sync when the committed value changes externally.
  useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  return (
    <Input
      type={type}
      value={draft}
      placeholder={placeholder}
      aria-label={ariaLabel}
      className={className}
      onChange={(event) => setDraft(event.currentTarget.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
    />
  );
}

/** A labeled provider field (a small caption above a full-width `TextField`). */
export function Field({ label, children }: { label: string; children: ReactNode }): ReactElement {
  return (
    <div>
      <div className="mb-1 text-ui-sm text-foreground-subtle">{label}</div>
      {children}
    </div>
  );
}
