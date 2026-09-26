import { useState } from "react";
import { CheckIcon, LoaderIcon, XIcon } from "lucide-react";
import type { DiffRef, ToolCallUiStatus } from "../store/sessions";
import DiffBlock from "./DiffBlock";

/** Truncate to `max` chars, appending an ellipsis when cut. */
function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max) + "…" : s;
}

/**
 * A one-line summary of a tool call's input (the header line): the
 * command / file path / pattern, per the approved design table.
 * Unknown tools fall back to a compact JSON dump of the input;
 * `undefined` when there is nothing worth showing.
 */
export function summarizeToolCall(title: string, rawInput: unknown): string | undefined {
  if (typeof rawInput !== "object" || rawInput === null) return undefined;
  const input = rawInput as Record<string, unknown>;
  const str = (k: string): string | undefined =>
    typeof input[k] === "string" ? (input[k] as string) : undefined;
  switch (title) {
    case "bash":
    case "sudo_exec":
    case "powershell":
      return str("command");
    case "read": {
      const path = str("path");
      if (!path) return undefined;
      const offset = typeof input.offset === "number" ? input.offset : undefined;
      const limit = typeof input.limit === "number" ? input.limit : undefined;
      // pi's `read` offset is 1-based; the end line is inclusive.
      if (offset !== undefined && limit !== undefined)
        return `${path} (L${offset}–${offset + limit - 1})`;
      return path;
    }
    case "write":
      return str("path");
    case "edit": {
      const path = str("path");
      if (!path) return undefined;
      const n = Array.isArray(input.edits) ? (input.edits as unknown[]).length : 0;
      return n > 1 ? `${path} (${n} edits)` : path;
    }
    case "grep":
    case "find": {
      const pattern = str("pattern");
      if (!pattern) return undefined;
      const path = str("path");
      return path ? `${pattern} in ${path}` : pattern;
    }
    case "ls":
      return str("path") ?? ".";
    case "web_search":
      return str("query");
    case "fetch_content":
      return str("url");
    case "ask": {
      const questions = input.questions;
      const first =
        Array.isArray(questions) && questions.length > 0 ? questions[0] : undefined;
      const q =
        first && typeof first === "object"
          ? (first as Record<string, unknown>).question
          : undefined;
      return typeof q === "string" ? truncate(q, 60) : undefined;
    }
    case "manage_todo_list":
      return str("operation");
    case "subagent": {
      const task = str("task");
      if (task) return truncate(task, 60);
      const tasks = input.tasks;
      return Array.isArray(tasks) ? `${tasks.length} tasks` : undefined;
    }
    case "mcp":
      return str("tool");
    default: {
      const text = JSON.stringify(rawInput);
      return text === "{}" || text === "null" ? undefined : truncate(text, 80);
    }
  }
}

/**
 * Normalize a tool result to display text. Accepts pi's `AgentToolResult`
 * shape (`{ content: (TextContent | ImageContent)[], details? }` — text
 * items joined, images counted, `details` as a no-text fallback), a bare
 * string (as-is), or any other object (compact JSON). `undefined` when
 * there is nothing to show.
 */
export function normalizeToolOutput(rawOutput: unknown): string | undefined {
  if (typeof rawOutput === "string") return rawOutput === "" ? undefined : rawOutput;
  if (typeof rawOutput !== "object" || rawOutput === null) return undefined;
  const result = rawOutput as Record<string, unknown>;
  if (Array.isArray(result.content)) {
    const parts: string[] = [];
    let images = 0;
    for (const item of result.content as Array<Record<string, unknown>>) {
      if (item && item.type === "text" && typeof item.text === "string")
        parts.push(item.text);
      else if (item && item.type === "image") images += 1;
    }
    let text = parts.join("\n");
    if (images > 0)
      text = (text ? text + "\n" : "") + `(+${images} image${images > 1 ? "s" : ""})`;
    if (text !== "") return text;
  }
  if (result.details !== undefined) {
    const d = JSON.stringify(result.details);
    return d === "null" || d === "{}" ? undefined : d;
  }
  const whole = JSON.stringify(rawOutput);
  return whole === "{}" || whole === "null" ? undefined : whole;
}

/**
 * One `h-8` tool row: a status-aware icon (pending → spinner, completed →
 * check, failed → ✕) + the title + a muted one-line summary derived from
 * `rawInput` (the command / file path / pattern, per the design table).
 * Expandable body: the extracted diff via `DiffBlock` when present, else
 * the normalized `rawOutput` text (scrollable, capped at 20k chars), else a
 * muted "(no output)" line.
 */
export default function ToolCallCard({
  title,
  status,
  diff,
  rawInput,
  rawOutput,
}: {
  title: string;
  status: ToolCallUiStatus;
  diff?: DiffRef;
  rawInput?: unknown;
  rawOutput?: unknown;
}) {
  const [open, setOpen] = useState(false);
  const summary = summarizeToolCall(title, rawInput);
  const output = normalizeToolOutput(rawOutput);

  return (
    <div className="w-full">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex h-8 w-full items-center gap-2 rounded-lg px-2 text-left hover:bg-surface-hover"
      >
        {status === "pending" ? (
          <LoaderIcon className="size-4 shrink-0 animate-spin text-foreground-subtle" />
        ) : status === "completed" ? (
          <CheckIcon className="size-4 shrink-0 text-foreground-subtle" />
        ) : (
          <XIcon className="size-4 shrink-0 text-destructive" />
        )}
        <span
          className={`min-w-0 truncate text-ui-base ${
            status === "failed"
              ? "text-destructive"
              : status === "completed"
                ? "text-foreground-subtle"
                : ""
          }`}
        >
          {title}
        </span>
        {summary && (
          <span className="min-w-0 truncate text-ui-sm text-foreground-subtlest">
            {summary}
          </span>
        )}
      </button>
      {open && (
        <div className="mt-1 rounded-md bg-surface p-2">
          {diff ? (
            <DiffBlock path={diff.path} patch={diff.patch} />
          ) : output !== undefined ? (
            <div>
              <pre className="font-mono max-h-80 overflow-auto whitespace-pre-wrap text-ui-sm">
                {output.length > 20000 ? output.slice(0, 20000) + "…" : output}
              </pre>
              {output.length > 20000 && (
                <p className="text-ui-sm text-foreground-subtlest">
                  … (truncated — {output.length} chars total)
                </p>
              )}
            </div>
          ) : (
            <p className="text-ui-sm text-foreground-subtlest">(no output)</p>
          )}
        </div>
      )}
    </div>
  );
}
