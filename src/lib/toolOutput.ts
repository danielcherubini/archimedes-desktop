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
    case "mcp": {
      const tool = str("tool");
      const server = str("server");
      const search = str("search");
      const describe = str("describe");
      const connect = str("connect");
      const action = str("action");
      // The tool being called (most specific), optionally with the server
      // it is called on.
      if (tool) return server ? `${tool} (${server})` : tool;
      // A server-scoped action (list / auth / connect) → the server name.
      if (server) return server;
      // A cross-server action (search / describe / connect).
      if (search) return `search: ${search}`;
      if (describe) return `describe: ${describe}`;
      if (connect) return `connect ${connect}`;
      if (action) return action;
      return undefined; // the bare status call (mcp({})).
    }
    default: {
      const text = JSON.stringify(rawInput);
      return text === "{}" || text === "null" ? undefined : truncate(text, 80);
    }
  }
}

/**
 * A human duration for a millisecond span (`<60s` → `Ns`, `<60m` → `Nm`,
 * else `Nh`) — the live tool duration in the subagent activity line.
 */
export function formatDuration(ms: number): string {
  if (ms < 100) return "<0.1s";
  if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rem = s % 60;
  return `${m}m${rem > 0 ? `${rem}s` : ""}`;
}

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1).replace(/\.0$/, "")}k`;
  return String(n);
}

export function formatCost(cost: number): string {
  if (!cost || cost <= 0) return "";
  if (cost < 0.01) return "$" + cost.toFixed(4);
  return "$" + cost.toFixed(2);
}

export const THINKING_GLYPHS: Record<string, string> = {
  off: "○",
  minimal: "○",
  low: "◔",
  medium: "◑",
  high: "◕",
  xhigh: "●",
  max: "●",
};

export function formatThinkingIndicator(level?: string): string | undefined {
  if (!level) return undefined;
  const glyph = THINKING_GLYPHS[level.toLowerCase()] ?? "◑";
  return `${glyph} ${level}`;
}

export function cleanModelName(model?: string): string | undefined {
  if (!model) return undefined;
  const withoutThinking = model.includes(":") ? model.split(":")[0] : model;
  return withoutThinking;
}

export function extractThinkingFromModel(model?: string): string | undefined {
  if (!model || !model.includes(":")) return undefined;
  const parts = model.split(":");
  return parts[parts.length - 1];
}

/**
 * One subagent's ACTIVITY line (the pi-archimedes `buildActivityLine`
 * treatment, condensed): the failure reason, or `Done` / `Failed`, or
 * the current tool + args + live duration, or the LAST line of the
 * subagent's streamed output (the "what is it working on" line), or
 * `Starting...`. `undefined` when there is nothing to show.
 */
export function subagentActivityLine(
  e: Record<string, unknown>,
  now = Date.now(),
): string | undefined {
  if (typeof e.error === "string" && e.error !== "") return truncate(e.error, 80);
  const status = typeof e.status === "string" ? e.status : undefined;
  if (status === "completed") return "Done";
  if (status === "failed") return "Failed";
  if (status === "running") {
    const tool = typeof e.currentTool === "string" ? e.currentTool : undefined;
    if (tool) {
      const args = typeof e.currentToolArgs === "string" ? e.currentToolArgs : "";
      const startedAt =
        typeof e.currentToolStartedAt === "number" ? e.currentToolStartedAt : undefined;
      const duration =
        startedAt !== undefined ? ` · ${formatDuration(now - startedAt)}` : "";
      return (args !== "" ? `${tool}: ${truncate(args, 60)}` : tool) + duration;
    }
    if (Array.isArray(e.toolCalls) && e.toolCalls.length > 0) {
      const lastCall = e.toolCalls[e.toolCalls.length - 1] as
        | { name?: string; argsPreview?: string; error?: boolean }
        | string;
      if (typeof lastCall === "string") {
        return truncate(lastCall, 60);
      }
      if (lastCall && typeof lastCall.name === "string") {
        const glyph = lastCall.error ? "✗" : "✓";
        const suffix = lastCall.argsPreview ? `: ${truncate(lastCall.argsPreview, 60)}` : "";
        return `${glyph} ${lastCall.name}${suffix}`;
      }
    }
    // The last non-empty line of the subagent's streamed output (the
    // "what is it working on" line the user asked for).
    const recent = Array.isArray(e.recentOutput)
      ? (e.recentOutput as unknown[]).filter(
          (l): l is string => typeof l === "string" && l.trim() !== "",
        )
      : [];
    if (recent.length > 0) return truncate(recent[recent.length - 1] as string, 80);
    if (typeof e.output === "string" && e.output.trim() !== "") {
      const lines = (e.output as string)
        .split("\n")
        .filter((l) => l.trim() !== "");
      if (lines.length > 0) return truncate(lines[lines.length - 1], 80);
    }
    return "Starting...";
  }
  // The final `results` shape (with `exitCode` + `finalOutput`, no `status`):
  // the last line of the subagent's final output (the "what it worked on"
  // line the user asked for), or `Done` / `Failed` when there is none.
  const exitCode = typeof e.exitCode === "number" ? e.exitCode : undefined;
  if (exitCode !== undefined) {
    if (exitCode !== 0 && typeof e.error === "string" && e.error !== "") {
      return truncate(e.error, 80);
    }
    const final = typeof e.finalOutput === "string" ? e.finalOutput : "";
    const lines = final.split("\n").filter((l) => l.trim() !== "");
    if (lines.length > 0) return truncate(lines[lines.length - 1], 80);
    return exitCode === 0 ? "Done" : "Failed";
  }
  return undefined;
}

/**
 * The subagent tool's `details` (the progress envelope — NOT display text)
 * as a human-readable summary: one block per subagent — `<agent>: <task>`
 * + the activity line. Prefers the live `progress` (while any subagent is
 * still running) and falls back to the final `results`. `undefined` when
 * there is nothing to show.
 */
function summarizeSubagentDetails(details: unknown): string | undefined {
  if (typeof details !== "object" || details === null) return undefined;
  const d = details as Record<string, unknown>;
  const asEntries = (
    key: "progress" | "results",
  ): Array<Record<string, unknown>> => {
    const arr = d[key];
    if (!Array.isArray(arr)) return [];
    return (arr as unknown[]).filter(
      (x): x is Record<string, unknown> =>
        typeof x === "object" && x !== null,
    );
  };
  // Prefer the live progress while any subagent is still running; once
  // all are terminal, the final `results` carry the authoritative state
  // (the `progress` may be stale / misaligned after settle).
  const progress = asEntries("progress");
  const anyRunning = progress.some(
    (p) => p.status === "running",
  );
  const entries = anyRunning ? progress : (asEntries("results").length > 0 ? asEntries("results") : progress);
  if (entries.length === 0) return undefined;
  const blocks = entries.map((e) => {
    const agent = typeof e.agent === "string" ? e.agent : "subagent";
    const task = typeof e.task === "string" ? e.task : "";
    const header = task !== "" ? `${agent}: ${task}` : agent;
    const line = subagentActivityLine(e);
    return line !== undefined ? `${header}\n  ${line}` : header;
  });
  return blocks.join("\n\n");
}

/**
 * The subagent tool's `details` summary for ONE subagent (the `Subagents`
 * panel's transcript fallback): filters the `progress` / `results` entries
 * to the one matching `sessionId` (via `childSessionId` on `results`, or
 * the `task` on `progress` — the live entries carry no session id) and
 * renders it (the `summarizeSubagentDetails` treatment). `undefined` when
 * no entry matches (the caller falls back to the subagent's own stream).
 */
export function summarizeSubagentFor(
  details: unknown,
  sessionId: string,
  task: string,
): string | undefined {
  if (typeof details !== "object" || details === null) return undefined;
  const d = details as Record<string, unknown>;
  const pick = (
    key: "progress" | "results",
  ): Array<Record<string, unknown>> => {
    const arr = d[key];
    if (!Array.isArray(arr)) return [];
    return (arr as unknown[])
      .filter((x): x is Record<string, unknown> => typeof x === "object" && x !== null)
      .filter((x) => {
        // `results` carry the subagent's pi id; `progress` (live) do not —
        // match those on the `task` (the dispatch's task, unique per
        // subagent in practice).
        const childId = x.childSessionId;
        if (typeof childId === "string") return childId === sessionId;
        return typeof x.task === "string" && x.task === task;
      });
  };
  const filtered = { ...d, progress: pick("progress"), results: pick("results") };
  return summarizeSubagentDetails(filtered);
}

/**
 * The subagent tool's `details` ONE-LINE activity for a SINGLE subagent
 * (the nested `SubagentDelegatingCard`'s live preview — e.g. `read:
 * docs/foo.md · 12s`): filters the `progress` / `results` entries to the
 * one matching `sessionId` / `task` (the `summarizeSubagentFor` match),
 * applies the `summarizeSubagentDetails` preference ON THE FILTERED
 * arrays (prefer the live `progress` while any filtered `progress` entry
 * is still running — the SAME predicate, which checks only the
 * `progress` entries), and returns the `subagentActivityLine` of the
 * FIRST entry of the chosen array (the ONE-LINE activity, not the
 * multi-line `summarizeSubagentFor` block). `undefined` when no entry
 * matches (the caller renders no activity line) or when `details` is
 * not an object. Filtering FIRST matters for the mixed multi-subagent
 * case (one `subagent` tool call's `details` can carry multiple
 * subagents: with sub A finished while sub B runs, prefer-then-filter
 * would pick `progress` (B running) for A's row — a stale preview;
 * filter-first picks A's finished entry from `results`).
 */
export function subagentActivityFor(
  details: unknown,
  sessionId: string,
  task: string,
): string | undefined {
  if (typeof details !== "object" || details === null) return undefined;
  const d = details as Record<string, unknown>;
  const pick = (
    key: "progress" | "results",
  ): Array<Record<string, unknown>> => {
    const arr = d[key];
    if (!Array.isArray(arr)) return [];
    return (arr as unknown[])
      .filter((x): x is Record<string, unknown> => typeof x === "object" && x !== null)
      .filter((x) => {
        // `results` carry the subagent's pi id; `progress` (live) do not —
        // match those on the `task` (the dispatch's task, unique per
        // subagent in practice).
        const childId = x.childSessionId;
        if (typeof childId === "string") return childId === sessionId;
        return typeof x.task === "string" && x.task === task;
      });
  };
  // The `summarizeSubagentDetails` preference, applied to the FILTERED
  // arrays: prefer the live `progress` while any filtered `progress`
  // entry is still running (the same predicate — `progress` entries
  // only), else the final `results` if non-empty, else `progress`.
  const filteredProgress = pick("progress");
  const filteredResults = pick("results");
  const anyRunning = filteredProgress.some((p) => p.status === "running");
  const entries = anyRunning
    ? filteredProgress
    : (filteredResults.length > 0 ? filteredResults : filteredProgress);
  if (entries.length === 0) return undefined;
  return subagentActivityLine(entries[0]);
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
 *
 * `title` (the tool name) selects tool-specific `details` rendering: the
 * `subagent` `details` is a progress envelope (not display text) and is
 * rendered as a human-readable summary (agent + task + activity line)
 * instead of a raw JSON dump.
 */
export function normalizeToolOutput(
  rawOutput: unknown,
  failed = false,
  title?: string,
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
    // The `subagent` `details` is a progress envelope (not display text) —
    // render the human-readable summary; a `null` / empty summary falls
    // through to the JSON dump below (nothing to show yet).
    if (title === "subagent") {
      const summary = summarizeSubagentDetails(result.details);
      if (summary !== undefined) return summary;
    }
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
