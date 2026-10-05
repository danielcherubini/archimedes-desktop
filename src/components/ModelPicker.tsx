import { useState, type ReactNode } from "react";
import { ChevronDownIcon } from "lucide-react";
import { Button } from "./ui/button";
import { cn } from "./lib/utils";
import ModelPickerDialog, { type ModelPickerItem } from "./ModelPickerDialog";

/**
 * The MODEL PICKER — the one shared component every model picker uses
 * (the composer's model option + the settings' Default-model /
 * Subagents' per-agent pickers): a trigger (a `Button` in the
 * SelectTrigger's look — the current value, or the muted placeholder
 * when unset, + a chevron) opening the `ModelPickerDialog` (the
 * fuzzy-searched, alphabetical list). The dialog's open state is owned
 * HERE (the call site just supplies the data + the `onSelect` side
 * effect).
 *
 * `variant` is the trigger's look: `"ghost"` (the composer — the old
 * SelectTrigger's `ghost` variant) / `"outline"` (the settings — the
 * old SelectTrigger's `input` variant: a bordered `w-64` box).
 */
export default function ModelPicker({
  label,
  value,
  placeholder,
  items,
  onSelect,
  variant = "outline",
  triggerClassName,
  icon,
  disabled,
}: {
  label: string;
  value: string;
  placeholder?: string;
  items: ModelPickerItem[];
  onSelect: (value: string) => void;
  variant?: "ghost" | "outline";
  /** The trigger's size classes (the composer's content-sized
   *  `min-w-0 shrink max-w-72`; the settings pass none and get the
   *  `Button` default, content-sized). */
  triggerClassName?: string;
  /** An optional leading icon (the composer's bot icon). */
  icon?: ReactNode;
  /** Disable the trigger (the composer's `onSet`-pending state). */
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const unset = value === "";
  // The trigger's text is the selected ITEM's display name (the bare model
  // id) plus the provider as a muted suffix — NOT the raw `provider/id`
  // value: the provider would then be printed twice (prefix + name), and
  // the prefix eats the composer's narrow `max-w-48` trigger
  // (`wizards/Qwen/Qw…`). A value matching no item (a stale override, a
  // hand-set key) falls back to the raw value, unsuffixed.
  const selected = items.find((item) => item.value === value);
  const displayText = selected?.name ?? value;
  return (
    <>
      <Button
        variant={variant}
        size={variant === "ghost" ? "sm" : "default"}
        className={cn(
          "justify-start",
          // The settings' trigger mirrors the SelectTrigger's `input`
          // variant (a bordered input-look box); the ghost variant is
          // borderless.
          variant === "outline" &&
            "border-input-border bg-input hover:border-input-border-hover",
          triggerClassName,
        )}
        aria-label={label}
        disabled={disabled}
        onClick={() => setOpen(true)}
      >
        {icon}
        <span
          className={`min-w-0 flex-1 truncate text-left ${
            unset ? "text-foreground-subtlest" : ""
          }`}
        >
          {unset ? placeholder ?? "" : displayText}
          {!unset && selected?.provider !== undefined && (
            <span className="text-foreground-subtlest">
              {" · " + selected.provider}
            </span>
          )}
        </span>
        <ChevronDownIcon
          className="size-3.5 shrink-0 text-foreground-subtle"
          aria-hidden
        />
      </Button>
      <ModelPickerDialog
        open={open}
        onOpenChange={setOpen}
        items={items}
        onSelect={onSelect}
      />
    </>
  );
}
