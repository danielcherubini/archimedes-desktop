import {
  CircleAlert,
  CheckCircle2,
  LoaderCircle,
} from "lucide-react";
import type { SubagentEntry } from "../store/subagents";

export const STATUS_CHIP_STYLES: Record<SubagentEntry["status"], string> = {
  running: "text-warning",
  completed: "text-success",
  failed: "text-destructive",
};

/**
 * The directory row's status icon (ZCode's `StatusIcon` treatment):
 * running = a spinning loader, completed = a check circle, failed = an
 * alert circle.
 */
export function StatusIcon({ status }: { status: SubagentEntry["status"] }) {
  const className = "size-4 shrink-0";
  if (status === "running") {
    return <LoaderCircle className={`${className} animate-spin text-warning`} />;
  }
  if (status === "completed") {
    return <CheckCircle2 className={`${className} text-success`} />;
  }
  return <CircleAlert className={`${className} text-destructive`} />;
}
