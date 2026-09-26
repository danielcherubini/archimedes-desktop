import type { Message } from "../store/sessions";

export type ToolCallMessage = Extract<Message, { kind: "tool-call" }>;

export type RenderUnit =
  | { kind: "single"; message: Message }
  | { kind: "changes-group"; messages: ToolCallMessage[] };

const GROUP_TOOLS = new Set(["write", "edit"]);

/**
 * Fold a maximal run of ≥2 CONSECUTIVE `write`/`edit` tool-call
 * messages into one `changes-group` unit; everything else (a single
 * write/edit, or any run interrupted by another message kind —
 * `agent-text`, `agent-thought`, `user`, `diff`) passes through as
 * `single` units. Pure — re-derived per render, so live streaming
 * (a write joins the run mid-turn) and reloads are consistent.
 */
export function groupConsecutiveFileWrites(messages: Message[]): RenderUnit[] {
  const units: RenderUnit[] = [];
  let run: ToolCallMessage[] = [];
  const flush = () => {
    if (run.length >= 2) units.push({ kind: "changes-group", messages: run });
    else for (const m of run) units.push({ kind: "single", message: m });
    run = [];
  };
  for (const m of messages) {
    if (m.kind === "tool-call" && GROUP_TOOLS.has(m.title)) {
      run.push(m);
    } else {
      flush();
      units.push({ kind: "single", message: m });
    }
  }
  flush();
  return units;
}
