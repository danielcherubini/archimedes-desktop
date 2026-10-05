import { useEffect, useState } from "react";
import { useSubagents } from "../store/subagents";
import { useSubagentSelection } from "../store/subagentSelection";
import { useInteractive } from "../store/interactive";
import { useSessions, type Message, type ToolCallUiStatus } from "../store/sessions";
import {
  cleanModelName,
  extractThinkingFromModel,
  formatCost,
  formatDuration,
  formatThinkingIndicator,
  formatTokens,
  getModelContextWindow,
  summarizeToolCall,
} from "../lib/toolOutput";
import { StatusIcon } from "./SubagentStatusIcon";
import { ToolCallCardHeader } from "./ToolCallCardHeader";

interface RawInputTaskItem {
  task?: string;
  agent?: string;
  model?: string;
  thinking?: string;
}

interface RawProgressItem {
  agent?: string;
  task?: string;
  status?: "running" | "completed" | "failed";
  currentTool?: string;
  currentToolArgs?: string;
  currentToolStartedAt?: number;
  toolCount?: number;
  inputTokens?: number;
  outputTokens?: number;
  tokens?: number;
  cost?: number;
  durationMs?: number;
  error?: string;
  model?: string;
  output?: string;
  recentOutput?: string[];
  toolCalls?: Array<{ name?: string; argsPreview?: string; error?: boolean } | string>;
  percent?: number;
  contextPercent?: number;
}

interface RawResultItem {
  agent?: string;
  task?: string;
  childSessionId?: string;
  exitCode?: number;
  usage?: {
    input?: number;
    output?: number;
    cacheRead?: number;
    cacheWrite?: number;
    cost?: number;
    turns?: number;
  };
  model?: string;
  finalOutput?: string;
  error?: string;
  progressSummary?: { toolCount?: number; tokens?: number; durationMs?: number };
}

function extractTargetInfo(rawInput: unknown, rawOutput: unknown) {
  const targetTasks: string[] = [];
  const targetSessionIds: string[] = [];
  const inputItems: RawInputTaskItem[] = [];

  if (typeof rawInput === "object" && rawInput !== null) {
    const input = rawInput as Record<string, unknown>;
    if (typeof input.task === "string" && input.task) {
      targetTasks.push(input.task);
      inputItems.push({
        task: input.task,
        agent: typeof input.agent === "string" ? input.agent : undefined,
        model: typeof input.model === "string" ? input.model : undefined,
        thinking: typeof input.thinking === "string" ? input.thinking : undefined,
      });
    }
    if (Array.isArray(input.tasks)) {
      for (const t of input.tasks) {
        if (typeof t === "string" && t) {
          targetTasks.push(t);
          inputItems.push({ task: t });
        } else if (typeof t === "object" && t !== null) {
          const obj = t as Record<string, unknown>;
          if (typeof obj.task === "string" && obj.task) {
            targetTasks.push(obj.task);
            inputItems.push({
              task: obj.task,
              agent: typeof obj.agent === "string" ? obj.agent : undefined,
              model: typeof obj.model === "string" ? obj.model : undefined,
              thinking: typeof obj.thinking === "string" ? obj.thinking : undefined,
            });
          }
        }
      }
    }
  }

  const output = rawOutput as Record<string, unknown> | undefined;
  const details = output?.details as Record<string, unknown> | undefined;
  const progressList: RawProgressItem[] = Array.isArray(details?.progress)
    ? (details!.progress as RawProgressItem[])
    : [];
  const resultsList: RawResultItem[] = Array.isArray(details?.results)
    ? (details!.results as RawResultItem[])
    : [];

  for (const p of progressList) {
    if (p && typeof p.task === "string" && p.task) {
      targetTasks.push(p.task);
    }
  }

  for (const r of resultsList) {
    if (r) {
      if (typeof r.task === "string" && r.task) targetTasks.push(r.task);
      if (typeof r.childSessionId === "string" && r.childSessionId) {
        targetSessionIds.push(r.childSessionId);
      }
    }
  }

  return {
    targetTasks: Array.from(new Set(targetTasks)),
    targetSessionIds: Array.from(new Set(targetSessionIds)),
    inputItems,
    progressList,
    resultsList,
    details,
  };
}

function estimateTokensFromMessages(messages: Message[]): number {
  let totalChars = 0;
  for (const m of messages) {
    if (m.kind === "user" || m.kind === "agent-text" || m.kind === "agent-thought") {
      totalChars += m.text.length;
    } else if (m.kind === "tool-call") {
      if (typeof m.rawInput === "string") totalChars += m.rawInput.length;
      else if (typeof m.rawInput === "object" && m.rawInput !== null) {
        totalChars += JSON.stringify(m.rawInput).length;
      }
      if (typeof m.rawOutput === "string") totalChars += m.rawOutput.length;
      else if (typeof m.rawOutput === "object" && m.rawOutput !== null) {
        totalChars += JSON.stringify(m.rawOutput).length;
      }
    } else if (m.kind === "diff") {
      totalChars += m.patch.length;
    }
  }
  return Math.round(totalChars / 4);
}

interface ActivityInfo {
  glyph?: string;
  text: string;
  duration?: string;
  colorClass: string;
}

function getActivityDisplay(
  row: {
    status: "running" | "completed" | "failed";
    error?: string;
    progress?: RawProgressItem;
    result?: RawResultItem;
  },
  sessionMessages: Message[] | undefined,
  now: number,
): ActivityInfo | undefined {
  if (row.error) {
    return { glyph: "✗", text: row.error, colorClass: "text-destructive" };
  }
  if (row.status === "completed") {
    return { glyph: "✓", text: "Done", colorClass: "text-success" };
  }
  if (row.status === "failed") {
    return { glyph: "✗", text: "Failed", colorClass: "text-destructive" };
  }

  // 1. Check live subagent transcript messages if available
  if (sessionMessages && sessionMessages.length > 0) {
    // In-flight running tool call
    const inFlightTool = sessionMessages.find(
      (m): m is Extract<Message, { kind: "tool-call" }> =>
        m.kind === "tool-call" && (m.status === "pending" || (m.status as string) === "running"),
    );
    if (inFlightTool) {
      const summary = summarizeToolCall(inFlightTool.title, inFlightTool.rawInput);
      const dur = inFlightTool.at ? ` · ${formatDuration(now - inFlightTool.at)}` : "";
      return {
        glyph: "▸",
        text: `${inFlightTool.title}${summary ? `: ${summary}` : ""}`,
        duration: dur,
        colorClass: "text-foreground-subtle",
      };
    }

    // Most recent transcript action
    const lastMsg = sessionMessages[sessionMessages.length - 1];
    if (lastMsg) {
      if (lastMsg.kind === "tool-call") {
        const summary = summarizeToolCall(lastMsg.title, lastMsg.rawInput);
        const glyph = lastMsg.status === "failed" ? "✗" : "✓";
        return {
          glyph,
          text: `${lastMsg.title}${summary ? `: ${summary}` : ""}`,
          colorClass: lastMsg.status === "failed" ? "text-destructive" : "text-foreground-subtle",
        };
      }
      if (lastMsg.kind === "agent-thought") {
        const lines = lastMsg.text.split("\n").filter((l) => l.trim() !== "");
        if (lines.length > 0) {
          return {
            glyph: "▸",
            text: `[thinking] ${lines[lines.length - 1].trim()}`,
            colorClass: "text-foreground-subtlest",
          };
        }
      }
      if (lastMsg.kind === "agent-text") {
        const lines = lastMsg.text.split("\n").filter((l) => l.trim() !== "");
        if (lines.length > 0) {
          return {
            glyph: "▸",
            text: lines[lines.length - 1].trim(),
            colorClass: "text-foreground-subtlest",
          };
        }
      }
    }
  }

  // 2. Fall back to progress from details envelope
  const p = row.progress;
  if (p?.currentTool) {
    const args = p.currentToolArgs ? `: ${p.currentToolArgs}` : "";
    const dur = p.currentToolStartedAt
      ? ` · ${formatDuration(now - p.currentToolStartedAt)}`
      : "";
    return {
      glyph: "▸",
      text: `${p.currentTool}${args}`,
      duration: dur,
      colorClass: "text-foreground-subtle",
    };
  }
  if (p?.toolCalls && p.toolCalls.length > 0) {
    const last = p.toolCalls[p.toolCalls.length - 1];
    if (typeof last === "string") {
      return { glyph: "✓", text: last, colorClass: "text-foreground-subtle" };
    }
    if (last && typeof last === "object") {
      const glyph = last.error ? "✗" : "✓";
      const suffix = last.argsPreview ? `: ${last.argsPreview}` : "";
      return {
        glyph,
        text: `${last.name ?? "tool"}${suffix}`,
        colorClass: last.error ? "text-destructive" : "text-foreground-subtle",
      };
    }
  }
  if (p?.recentOutput && p.recentOutput.length > 0) {
    const lastLine = p.recentOutput[p.recentOutput.length - 1];
    if (lastLine) {
      return { glyph: "▸", text: lastLine, colorClass: "text-foreground-subtlest" };
    }
  }
  if (p?.output) {
    const lines = p.output.split("\n").filter((l) => l.trim() !== "");
    if (lines.length > 0) {
      return { glyph: "▸", text: lines[lines.length - 1], colorClass: "text-foreground-subtlest" };
    }
  }

  // 3. Fall back to result
  if (row.result) {
    const r = row.result;
    if (r.exitCode !== 0 && r.error) {
      return { glyph: "✗", text: r.error, colorClass: "text-destructive" };
    }
    if (r.finalOutput) {
      const lines = r.finalOutput.split("\n").filter((l) => l.trim() !== "");
      if (lines.length > 0) {
        return { text: lines[lines.length - 1], colorClass: "text-foreground-subtle" };
      }
    }
    return {
      glyph: r.exitCode === 0 ? "✓" : "✗",
      text: r.exitCode === 0 ? "Done" : "Failed",
      colorClass: r.exitCode === 0 ? "text-success" : "text-destructive",
    };
  }

  if (row.status === "running") {
    return { glyph: "▸", text: "Starting...", colorClass: "text-foreground-subtlest" };
  }

  return undefined;
}

/**
 * The `subagent` tool's card (the "Delegating" card): the standard
 * `ToolCallCardHeader` + the subagents delegated by THIS tool call.
 * Displays a 3-line compact view per subagent:
 *   Line 1: <agentName>: <task>
 *   Line 2: <model> · <thinkingLevel> · <context% / tokens> · <stats>
 *   Line 3: <activity> (current tool + live duration, last tool, output, Done, Failed)
 * Clicking a row opens the subagent's dedicated transcript modal.
 */
export default function SubagentDelegatingCard({
  title,
  status,
  rawInput,
  rawOutput,
  sessionId,
}: {
  title: string;
  status: ToolCallUiStatus;
  rawInput?: unknown;
  rawOutput?: unknown;
  sessionId: string;
}) {
  const entries = useSubagents((s) => s.entries);
  const select = useSubagentSelection((s) => s.select);
  const allMessages = useSessions((s) => s.messages);
  const allInteractiveCost = useInteractive((s) => s.cost);
  const [open, setOpen] = useState(true);
  const [now, setNow] = useState(() => Date.now());

  const { targetTasks, targetSessionIds, inputItems, progressList, resultsList } =
    extractTargetInfo(rawInput, rawOutput);

  const allSessionEntries = Object.values(entries).filter(
    (e) => e.parentSessionId === sessionId,
  );

  const hasSpecificTargets = targetTasks.length > 0 || targetSessionIds.length > 0;

  const matchingStoreEntries = hasSpecificTargets
    ? allSessionEntries.filter(
        (e) =>
          targetSessionIds.includes(e.sessionId) ||
          targetTasks.includes(e.task),
      )
    : allSessionEntries;

  // Build the list of task keys to render
  const taskKeys: string[] = [];
  if (inputItems.length > 0) {
    for (const item of inputItems) {
      if (item.task && !taskKeys.includes(item.task)) taskKeys.push(item.task);
    }
  }
  for (const p of progressList) {
    if (p.task && !taskKeys.includes(p.task)) taskKeys.push(p.task);
  }
  for (const r of resultsList) {
    if (r.task && !taskKeys.includes(r.task)) taskKeys.push(r.task);
  }
  for (const e of matchingStoreEntries) {
    if (e.task && !taskKeys.includes(e.task)) taskKeys.push(e.task);
  }

  const rawInputObj =
    typeof rawInput === "object" && rawInput !== null
      ? (rawInput as Record<string, unknown>)
      : undefined;

  const rows =
    taskKeys.length > 0
      ? taskKeys.map((taskKey, idx) => {
          const storeEntry = matchingStoreEntries.find(
            (e) =>
              e.task === taskKey ||
              (e.sessionId &&
                resultsList.some(
                  (r) => r.task === taskKey && r.childSessionId === e.sessionId,
                )),
          );
          const progress = progressList.find((p) => p.task === taskKey);
          const result = resultsList.find(
            (r) =>
              r.task === taskKey ||
              (storeEntry && r.childSessionId === storeEntry.sessionId),
          );
          const rawInputItem = inputItems.find((i) => i.task === taskKey);

          const resolvedSessionId = storeEntry?.sessionId ?? result?.childSessionId;
          const agentCandidate =
            (progress?.agent && progress.agent.trim()) ||
            (result?.agent && result.agent.trim()) ||
            (storeEntry?.agentName && storeEntry.agentName.trim()) ||
            (rawInputItem?.agent && rawInputItem.agent.trim()) ||
            (typeof rawInputObj?.agent === "string" && rawInputObj.agent.trim());
          const agentName = agentCandidate || "subagent";

          const subStatus: "running" | "completed" | "failed" =
            result?.exitCode !== undefined
              ? (result.exitCode === 0 ? "completed" : "failed")
              : storeEntry?.status ??
                progress?.status ??
                (status === "completed" ? "completed" : "running");

          const model =
            progress?.model ??
            result?.model ??
            storeEntry?.model ??
            rawInputItem?.model ??
            (typeof rawInputObj?.model === "string" ? rawInputObj.model : undefined);

          const thinkingLevel =
            storeEntry?.thinkingLevel ??
            rawInputItem?.thinking ??
            (typeof rawInputObj?.thinking === "string"
              ? rawInputObj.thinking
              : undefined) ??
            extractThinkingFromModel(model);

          const interactiveCost = resolvedSessionId
            ? (allInteractiveCost[resolvedSessionId] as
                | {
                    inputTokens?: number;
                    outputTokens?: number;
                    tokens?: number;
                    cost?: number;
                    percent?: number;
                    contextPercent?: number;
                  }
                | undefined)
            : undefined;

          const sessionMessages = resolvedSessionId ? allMessages[resolvedSessionId] : undefined;

          const explicitTokens =
            progress?.tokens ??
            (interactiveCost &&
            (interactiveCost.inputTokens !== undefined || interactiveCost.outputTokens !== undefined || interactiveCost.tokens !== undefined)
              ? (interactiveCost.tokens ?? (interactiveCost.inputTokens ?? 0) + (interactiveCost.outputTokens ?? 0))
              : undefined) ??
            (result?.usage
              ? (result.usage.input ?? 0) + (result.usage.output ?? 0)
              : undefined) ??
            result?.progressSummary?.tokens ??
            (storeEntry?.metrics &&
            (storeEntry.metrics.inputTokens > 0 || storeEntry.metrics.outputTokens > 0)
              ? storeEntry.metrics.inputTokens + storeEntry.metrics.outputTokens
              : undefined);

          const estimatedTokens =
            sessionMessages && sessionMessages.length > 0
              ? estimateTokensFromMessages(sessionMessages)
              : undefined;

          const tokens =
            (explicitTokens && explicitTokens > 0 ? explicitTokens : undefined) ??
            estimatedTokens;

          const modelContextWindow = getModelContextWindow(model);
          const calculatedPct =
            tokens && modelContextWindow > 0 ? (tokens / modelContextWindow) * 100 : undefined;

          const contextPercent =
            progress?.percent ??
            progress?.contextPercent ??
            interactiveCost?.percent ??
            interactiveCost?.contextPercent ??
            calculatedPct;

          const cost =
            progress?.cost ??
            interactiveCost?.cost ??
            result?.usage?.cost ??
            storeEntry?.metrics?.cost;

          const sessionToolCount = sessionMessages
            ? sessionMessages.filter((m) => m.kind === "tool-call").length
            : undefined;
          const toolCount =
            (sessionToolCount && sessionToolCount > 0 ? sessionToolCount : undefined) ??
            progress?.toolCount ??
            result?.progressSummary?.toolCount;

          const isRunning = subStatus === "running";
          const durationMs = isRunning
            ? (storeEntry?.startedAt ? now - storeEntry.startedAt : progress?.durationMs)
            : (result?.progressSummary?.durationMs ??
              storeEntry?.metrics?.durationMs ??
              (storeEntry?.endedAt && storeEntry?.startedAt
                ? storeEntry.endedAt - storeEntry.startedAt
                : undefined));

          const error = storeEntry?.error ?? result?.error ?? progress?.error;

          return {
            key: resolvedSessionId ?? `${taskKey}-${idx}`,
            sessionId: resolvedSessionId,
            agentName,
            task: taskKey,
            status: subStatus,
            model: cleanModelName(model),
            thinkingLevel,
            tokens,
            contextPercent,
            cost,
            toolCount,
            durationMs,
            error,
            progress,
            result,
            sessionMessages,
          };
        })
      : matchingStoreEntries.map((storeEntry, idx) => {
          const progress = progressList.find(
            (p) => p.task === storeEntry.task,
          );
          const result = resultsList.find(
            (r) =>
              r.childSessionId === storeEntry.sessionId ||
              r.task === storeEntry.task,
          );
          const isRunning = storeEntry.status === "running";
          const durationMs = isRunning
            ? now - storeEntry.startedAt
            : (result?.progressSummary?.durationMs ??
              storeEntry.metrics?.durationMs ??
              (storeEntry.endedAt ? storeEntry.endedAt - storeEntry.startedAt : undefined));

          const sessionMessages = storeEntry.sessionId ? allMessages[storeEntry.sessionId] : undefined;
          const sessionToolCount = sessionMessages
            ? sessionMessages.filter((m) => m.kind === "tool-call").length
            : undefined;
          const toolCount =
            (sessionToolCount && sessionToolCount > 0 ? sessionToolCount : undefined) ??
            progress?.toolCount ??
            result?.progressSummary?.toolCount;

          const interactiveCost = storeEntry.sessionId
            ? (allInteractiveCost[storeEntry.sessionId] as
                | {
                    inputTokens?: number;
                    outputTokens?: number;
                    tokens?: number;
                    cost?: number;
                    percent?: number;
                    contextPercent?: number;
                  }
                | undefined)
            : undefined;

          const explicitTokens =
            progress?.tokens ??
            (interactiveCost &&
            (interactiveCost.inputTokens !== undefined || interactiveCost.outputTokens !== undefined || interactiveCost.tokens !== undefined)
              ? (interactiveCost.tokens ?? (interactiveCost.inputTokens ?? 0) + (interactiveCost.outputTokens ?? 0))
              : undefined) ??
            (result?.usage
              ? (result.usage.input ?? 0) + (result.usage.output ?? 0)
              : undefined) ??
            (storeEntry.metrics &&
            (storeEntry.metrics.inputTokens > 0 || storeEntry.metrics.outputTokens > 0)
              ? storeEntry.metrics.inputTokens + storeEntry.metrics.outputTokens
              : undefined);

          const estimatedTokens =
            sessionMessages && sessionMessages.length > 0
              ? estimateTokensFromMessages(sessionMessages)
              : undefined;

          const tokens =
            (explicitTokens && explicitTokens > 0 ? explicitTokens : undefined) ??
            estimatedTokens;

          const rawModel = storeEntry.model ?? progress?.model ?? result?.model;
          const modelContextWindow = getModelContextWindow(rawModel);
          const calculatedPct =
            tokens && modelContextWindow > 0 ? (tokens / modelContextWindow) * 100 : undefined;

          const contextPercent =
            progress?.percent ??
            progress?.contextPercent ??
            interactiveCost?.percent ??
            interactiveCost?.contextPercent ??
            calculatedPct;

          return {
            key: storeEntry.sessionId ?? `entry-${idx}`,
            sessionId: storeEntry.sessionId,
            agentName: storeEntry.agentName || "subagent",
            task: storeEntry.task,
            status: storeEntry.status,
            model: cleanModelName(rawModel),
            thinkingLevel:
              storeEntry.thinkingLevel ??
              extractThinkingFromModel(rawModel),
            tokens,
            contextPercent,
            cost: progress?.cost ?? interactiveCost?.cost ?? result?.usage?.cost ?? storeEntry.metrics?.cost,
            toolCount,
            durationMs,
            error: storeEntry.error ?? result?.error ?? progress?.error,
            progress,
            result,
            sessionMessages,
          };
        });

  const hasRunning =
    status === "pending" || rows.some((r) => r.status === "running");

  useEffect(() => {
    if (!hasRunning) return;
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [hasRunning]);

  return (
    <div className="flex w-full flex-col">
      <ToolCallCardHeader
        title={title}
        status={status}
        rawInput={rawInput}
        rawOutput={rawOutput}
        open={open}
        onToggle={() => setOpen((o) => !o)}
        files={[]}
        stat={undefined}
        range={undefined}
        isShell={false}
        command={undefined}
      />
      {open && rows.length > 0 && (
        <div className="mt-1 rounded-xl border border-border bg-panel px-4 py-3 divide-y divide-border/50">
          {rows.map((row) => {
            const activity = getActivityDisplay(row, row.sessionMessages, now);

            // Construct metadata parts for line 2
            const metadataParts: Array<{ text: string; className: string }> = [];
            if (row.model) {
              metadataParts.push({
                text: row.model,
                className: "text-foreground-subtle",
              });
            }
            if (row.thinkingLevel) {
              const formatted = formatThinkingIndicator(row.thinkingLevel);
              metadataParts.push({
                text: formatted ?? row.thinkingLevel,
                className: "text-accent",
              });
            }
            if (row.tokens !== undefined && row.tokens > 0) {
              const pctStr =
                row.contextPercent !== undefined && !Number.isNaN(row.contextPercent)
                  ? ` (${row.contextPercent >= 1 ? `${Math.round(row.contextPercent)}%` : `${row.contextPercent.toFixed(1)}%`})`
                  : "";
              metadataParts.push({
                text: `${formatTokens(row.tokens)} tok${pctStr}`,
                className: "text-foreground-subtle",
              });
            } else if (row.contextPercent !== undefined && !Number.isNaN(row.contextPercent)) {
              metadataParts.push({
                text: `${Math.round(row.contextPercent)}%`,
                className: "text-foreground-subtle",
              });
            }
            if (row.toolCount !== undefined && row.toolCount > 0) {
              metadataParts.push({
                text: `${row.toolCount} tool${row.toolCount !== 1 ? "s" : ""}`,
                className: "",
              });
            }
            if (row.durationMs !== undefined && row.durationMs > 0) {
              metadataParts.push({
                text: formatDuration(row.durationMs),
                className: "",
              });
            }
            if (row.cost !== undefined && row.cost > 0) {
              const formattedCost = formatCost(row.cost);
              if (formattedCost) {
                metadataParts.push({
                  text: formattedCost,
                  className: "text-foreground-subtle",
                });
              }
            }

            return (
              <button
                key={row.key}
                type="button"
                onClick={() => row.sessionId && select(row.sessionId)}
                aria-label={`Open ${row.agentName} transcript`}
                className="flex w-full items-start gap-2.5 rounded-lg px-2 py-2 text-left hover:bg-surface-hover focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <span className="mt-1">
                  <StatusIcon status={row.status} />
                </span>
                <div className="min-w-0 flex-1 flex flex-col gap-0.5">
                  {/* Line 1: Agent name + task preview */}
                  <div className="flex items-center gap-1.5 min-w-0">
                    <span className="shrink-0 font-medium text-ui-base text-foreground">
                      {row.agentName}
                    </span>
                    {row.task && (
                      <>
                        <span className="text-ui-base text-foreground-subtle">:</span>
                        <span className="min-w-0 truncate text-ui-base text-foreground-subtle">
                          {row.task}
                        </span>
                      </>
                    )}
                  </div>

                  {/* Line 2: Model · Thinking · Context/Tokens · Stats */}
                  {metadataParts.length > 0 && (
                    <div className="flex items-center gap-1.5 min-w-0 text-ui-xs text-foreground-subtlest font-mono truncate">
                      {metadataParts.map((part, i) => (
                        <span key={i} className="flex items-center gap-1.5 shrink-0">
                          {i > 0 && <span className="opacity-50">·</span>}
                          <span className={part.className}>{part.text}</span>
                        </span>
                      ))}
                    </div>
                  )}

                  {/* Line 3: Activity / Current Action */}
                  {activity && (
                    <p className={`truncate text-ui-xs font-mono ${activity.colorClass}`}>
                      {activity.glyph && <span className="mr-1">{activity.glyph}</span>}
                      <span>{activity.text}</span>
                      {activity.duration && <span className="opacity-75">{activity.duration}</span>}
                    </p>
                  )}
                </div>
              </button>
            );
          })}
        </div>
      )}
      {open && rows.length === 0 && (
        <p className="font-mono text-ui-base text-foreground-subtle">
          No subagents yet.
        </p>
      )}
    </div>
  );
}
