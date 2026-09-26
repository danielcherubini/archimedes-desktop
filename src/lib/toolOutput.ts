import { basenameOfPath } from "./paths";
import type { ToolCallUiStatus } from "../store/sessions";
import type { LucideIcon } from "lucide-react";
import {
  SquareTerminalIcon,
  FileTextIcon,
  FilePenIcon,
  PencilIcon,
  SearchIcon,
  ListIcon,
  GlobeIcon,
  DownloadIcon,
  MessageCircleQuestionIcon,
  ListTodoIcon,
  BotIcon,
  PlugIcon,
  WrenchIcon,
} from "lucide-react";

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
 * items joined, images counted, `details` as a fallback), a bare string
 * (as-is), or any other object (compact JSON). `undefined` when there is
 * nothing to show.
 *
 * `failed` marks a failed tool call: its failure reason lives in
 * `details`, so an empty text item does NOT mean "no output" for failed
 * calls — the `details` fallback still applies.
 */
export function normalizeToolOutput(
  rawOutput: unknown,
  failed = false,
): string | undefined {
  if (typeof rawOutput === "string") return rawOutput === "" ? undefined : rawOutput;
  if (typeof rawOutput !== "object" || rawOutput === null) return undefined;
  const result = rawOutput as Record<string, unknown>;
  if (Array.isArray(result.content)) {
    const parts: string[] = [];
    let hasText = false;
    let images = 0;
    for (const item of result.content as Array<Record<string, unknown>>) {
      if (item && item.type === "text" && typeof item.text === "string") {
        hasText = true;
        if (item.text !== "") parts.push(item.text);
      } else if (item && item.type === "image") images += 1;
    }
    let text = parts.join("\n");
    if (images > 0)
      text = (text ? text + "\n" : "") + `(+${images} image${images > 1 ? "s" : ""})`;
    if (text !== "") return text;
    // A text item that is present but empty (e.g. a successful command with
    // no stdout) means the tool produced no output — show "(no output)"
    // instead of metadata. Exception: a FAILED call puts its failure
    // reason in `details`, so fall back to it. Same for results with no
    // text items at all.
    if (hasText && !failed) return undefined;
  }
  if (result.details !== undefined) {
    const d = JSON.stringify(result.details);
    return d === "null" || d === "{}" ? undefined : d;
  }
  const whole = JSON.stringify(rawOutput);
  return whole === "{}" || whole === "null" ? undefined : whole;
}

const VERBS: Record<string, { completed: string; running: string }> = {
  bash: { completed: "Ran", running: "Running" },
  powershell: { completed: "Ran", running: "Running" },
  sudo_exec: { completed: "Ran", running: "Running" },
  read: { completed: "Read", running: "Reading" },
  write: { completed: "Wrote", running: "Writing" },
  edit: { completed: "Edited", running: "Editing" },
  grep: { completed: "Searched", running: "Searching" },
  find: { completed: "Searched", running: "Searching" },
  web_search: { completed: "Searched", running: "Searching" },
  ls: { completed: "Listed", running: "Listing" },
  fetch_content: { completed: "Fetched", running: "Fetching" },
  ask: { completed: "Asked", running: "Asking" },
  manage_todo_list: { completed: "Todos", running: "Updating todos" },
  subagent: { completed: "Delegated", running: "Delegating" },
  mcp: { completed: "MCP", running: "MCP" },
};

/**
 * The header verb for a tool call. Past tense when finished (the verb
 * itself is the "done" signal — no check icon), present tense while
 * `pending` (the caller adds the `animated-gradient-text` shimmer).
 * A `failed` call takes the past tense too — the red `Failed` word is
 * the failure signal, so do not "fix" this. `undefined` for unknown
 * tools (the caller falls back to the raw title).
 */
export function toolVerb(title: string, status: ToolCallUiStatus): string | undefined {
  const v = VERBS[title];
  if (!v) return undefined;
  return status === "pending" ? v.running : v.completed;
}

const TOOL_ICONS: Record<string, LucideIcon> = {
  bash: SquareTerminalIcon,
  powershell: SquareTerminalIcon,
  sudo_exec: SquareTerminalIcon,
  read: FileTextIcon,
  write: FilePenIcon,
  edit: PencilIcon,
  grep: SearchIcon,
  find: SearchIcon,
  web_search: GlobeIcon,
  ls: ListIcon,
  fetch_content: DownloadIcon,
  ask: MessageCircleQuestionIcon,
  manage_todo_list: ListTodoIcon,
  subagent: BotIcon,
  mcp: PlugIcon,
};

/** The header icon for a tool call (always static — no spinner). */
export function toolIcon(title: string): LucideIcon {
  return TOOL_ICONS[title] ?? WrenchIcon;
}

export interface ToolFileSummary {
  path: string;
  fileName: string;
}

/**
 * The file a tool call touches (the header chip). `read` / `write` /
 * `edit` carry `path`; every other tool → `[]`. `fileName` is the
 * basename (the full path is the chip's `title` tooltip).
 */
export function fileSummaries(title: string, rawInput: unknown): ToolFileSummary[] {
  if (title !== "read" && title !== "write" && title !== "edit") return [];
  if (typeof rawInput !== "object" || rawInput === null) return [];
  const path = (rawInput as Record<string, unknown>).path;
  if (typeof path !== "string" || path === "") return [];
  const fileName = basenameOfPath(path);
  return [{ path, fileName: fileName !== "" ? fileName : path }];
}

/**
 * Line-count change stat — `edit` ONLY (ZCode's `getChangeStat`
 * semantics): sum over `edits[]` of `lineCount(newText)` /
 * `lineCount(oldText)`. `undefined` for other tools, missing/empty
 * `edits`, or when both totals are 0.
 */
export function editChangeStat(
  title: string,
  rawInput: unknown,
): { added: number; removed: number } | undefined {
  if (title !== "edit") return undefined;
  if (typeof rawInput !== "object" || rawInput === null) return undefined;
  const edits = (rawInput as Record<string, unknown>).edits;
  if (!Array.isArray(edits)) return undefined;
  const countLines = (s: string): number => (s === "" ? 0 : s.split("\n").length);
  let added = 0;
  let removed = 0;
  for (const e of edits as Array<Record<string, unknown>>) {
    if (typeof e.newText === "string") added += countLines(e.newText);
    if (typeof e.oldText === "string") removed += countLines(e.oldText);
  }
  if (added === 0 && removed === 0) return undefined;
  return { added, removed };
}

/**
 * The line range of a `read` with `offset` + `limit` (1-based, inclusive
 * end — same arithmetic as `summarizeToolCall`'s read case):
 * `L{offset}–{offset + limit - 1}`. `undefined` when either is missing or
 * not a number.
 */
export function readLineRange(rawInput: unknown): string | undefined {
  if (typeof rawInput !== "object" || rawInput === null) return undefined;
  const input = rawInput as Record<string, unknown>;
  const offset = typeof input.offset === "number" ? input.offset : undefined;
  const limit = typeof input.limit === "number" ? input.limit : undefined;
  if (offset === undefined || limit === undefined) return undefined;
  return `L${offset}–${offset + limit - 1}`;
}

/**
 * The failure reason for a failed tool call (the failure tooltip):
 * `details.error` wins, then the first non-empty `content` text item,
 * then a bare-string result as-is. `undefined` when nothing usable.
 */
export function failureText(rawOutput: unknown): string | undefined {
  if (typeof rawOutput === "string")
    return rawOutput.trim() === "" ? undefined : rawOutput;
  if (typeof rawOutput !== "object" || rawOutput === null) return undefined;
  const result = rawOutput as Record<string, unknown>;
  const details = result.details;
  if (typeof details === "object" && details !== null) {
    const err = (details as Record<string, unknown>).error;
    if (typeof err === "string" && err.trim() !== "") return err;
  }
  if (Array.isArray(result.content)) {
    for (const item of result.content as Array<Record<string, unknown>>) {
      if (
        item &&
        item.type === "text" &&
        typeof item.text === "string" &&
        item.text.trim() !== ""
      )
        return item.text;
    }
  }
  return undefined;
}
