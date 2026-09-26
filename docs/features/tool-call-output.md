# Tool-call output display

Shipped 2026-07-09 (PR #5).

## Behavior

A tool-call card in the transcript shows what happened, in two places:

- **Header line** (single `h-8` row): status icon + tool name + a muted, truncated
  **summary** derived from the tool's `rawInput` — bash/sudo_exec/powershell → the
  `command`; read → `path` (+ `L5–54` line range when `offset`/`limit` present);
  write/edit → `path` (edit adds `· N edits`); grep/find → `pattern` (+ ` in {path}`);
  ls → `path` or `.`; web_search/fetch_content/ask/subagent/mcp → their primary arg;
  unknown tools → a compact JSON dump of the input (or nothing when empty).
- **Expanded body**: the tool's **output** — `rawOutput` normalized to display text
  (pi's `AgentToolResult` shape: `content[]` text items joined, images counted as
  `(+N images)`, `details` as a fallback — including the failure reason for a failed
  call with empty stdout), a bare string as-is, or compact JSON; scrollable
  (`max-h-80`), capped at 20k chars with a truncation note. `(no output)` only when
  there is genuinely nothing (empty text items count as nothing for a successful call).
  A `content`-derived diff (the ACP path) keeps priority when present.

## Data source (why no agent-side work was needed)

The Rust normalizer (`src-tauri/src/agent/session.rs` `normalize()`) has always emitted
both fields on the `tool_call` / `tool_call_update` frames it sends to the frontend:

- `rawInput` — the tool's arguments (`toolcall_start` / `toolcall_delta` /
  `toolcall_end`),
- `rawOutput` — the tool's result: **live partial** on `tool_execution_update`
  (the card shows it while the tool runs, for free) and final on
  `tool_execution_end` (with `status: completed|failed`).

Both are also **persisted** into the tool-call row (`persist_update`'s `merge_json`
shallow-merge), so reloads restore the summary + output. The frontend previously
dropped `rawOutput` (no field on `AcpSessionUpdate`) and rendered only the title —
the fix was frontend-only: plumb `rawOutput` through the store (same
`??`-merge rule as `rawInput`) and render it in `ToolCallCard`
(`src/lib/toolOutput.ts` holds the pure `summarizeToolCall` / `normalizeToolOutput`
helpers; normalization is deferred until the card is expanded).
