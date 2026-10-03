import type { ReactElement, ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

import { cn } from "@/components/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/**
 * A settings section card: a rounded-xl card hosting rows (the ZCode
 * `SettingsGroupCard` port).
 */
export function SettingsGroupCard({ children }: { children: ReactNode }): ReactElement {
  return (
    <Card className="overflow-hidden rounded-xl border border-border bg-card py-0 shadow-none">
      <CardContent className="space-y-0 px-0">{children}</CardContent>
    </Card>
  );
}

/**
 * One settings row: label + description left, control right (the ZCode
 * `SettingsRow` port — `grid-cols-[minmax(0,1fr)_192px]`, `border-t` rows,
 * `first:border-t-0`, `px-4 py-3`). `controlLayout="wide"` widens the
 * control column (`sm:grid-cols-[minmax(0,1fr)_280px]`) and moves `detail`
 * into it.
 */
export function SettingsRow({
  label,
  description,
  control,
  detail,
  controlLayout = "default",
}: {
  label: ReactNode;
  description?: ReactNode;
  control: ReactNode;
  detail?: ReactNode;
  controlLayout?: "default" | "wide";
}): ReactElement {
  return (
    <div className="border-t border-border px-4 py-3 first:border-t-0">
      <div
        className={cn(
          "grid items-center gap-4",
          controlLayout === "wide"
            ? "grid-cols-1 sm:grid-cols-[minmax(0,1fr)_280px]"
            : "grid-cols-[minmax(0,1fr)_192px]",
        )}
      >
        <div className="min-w-0">
          <div className="text-ui-base font-medium text-foreground">{label}</div>
          {description ? (
            <div className="mt-1 text-ui-base leading-6 text-foreground-subtle">
              {description}
            </div>
          ) : null}
        </div>
        <div className="flex w-full flex-nowrap items-center justify-end gap-2">
          {controlLayout === "wide" ? detail : null}
          {control}
        </div>
      </div>
      {detail && controlLayout !== "wide" ? <div className="mt-3">{detail}</div> : null}
    </div>
  );
}

/** A subtle inline badge (the ZCode `SettingsBadge` port; an optional
 * `title` for the error tooltips). */
export function SettingsBadge({
  children,
  title,
}: {
  children: ReactNode;
  title?: string;
}): ReactElement {
  return (
    <span
      title={title}
      className="rounded-md bg-surface px-2.5 py-1 text-ui-base font-medium text-foreground-subtle"
    >
      {children}
    </span>
  );
}

/**
 * A section-nav button (the ZCode `SettingsSidebarButton` port — `h-8
 * w-full rounded-xl px-2.5`, icon + label; active = `bg-surface-hover
 * text-foreground`, inactive = `text-foreground-subtle
 * hover:bg-surface-hover hover:text-foreground`). The `ControlHintTooltip`
 * is the app's `ui/tooltip` (a local `TooltipProvider` — the app has no
 * global provider).
 */
export function SettingsSidebarButton({
  icon: Icon,
  label,
  active,
  onClick,
}: {
  icon: LucideIcon;
  label: string;
  active?: boolean;
  onClick?: () => void;
}): ReactElement {
  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label={label}
            onClick={onClick}
            className={cn(
              "flex h-8 w-full items-center gap-2 rounded-xl px-2.5 text-left transition-colors",
              active
                ? "bg-surface-hover text-foreground"
                : "text-foreground-subtle hover:bg-surface-hover hover:text-foreground",
            )}
          >
            <span className="flex size-4 shrink-0 items-center justify-center text-current">
              <Icon className="size-4 text-foreground" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="truncate text-ui-base text-foreground">{label}</span>
            </span>
          </button>
        </TooltipTrigger>
        <TooltipContent side="right" align="center">
          {label}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

/**
 * Humanize a braille variant name for display (`wave-rows` → `Wave rows`):
 * the spinner picker's cell labels + accessible names (the reference
 * settings UI's `capitalize` port, over the hyphen-split words).
 */
export function humanizeVariant(variant: string): string {
  return variant
    .split("-")
    .map((word) => word[0]?.toUpperCase() + word.slice(1))
    .join(" ");
}
