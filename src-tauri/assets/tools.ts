// Desktop-provided override extension (Phase 2). Re-registers `ask` /
// `sudo_exec` / `manage_todo_list` with the SAME name + label +
// description + parameters schema as the suite's versions, but
// `execute()` = "send the params to the desktop over the bridge, await
// the result, return it (shaped, for `ask`)". The desktop owns the
// confirmation / execution / todo store; the agent is a thin delegate.
//
// SELF-GATED on the platform + the four bridge env vars (the exact
// condition the suite's bridge uses, packages/core/src/bridge/index.ts
// isBridgeMode — minus the mode/subagent checks, which are irrelevant
// here). Inert without them: the suite's original tools remain (no
// regression).
//
// Registration is DEFERRED (inside `session_start`): two extensions
// registering the same tool name at LOAD time is a process.exit(1)
// conflict (DefaultResourceLoader.addExtensionConflictDiagnostics flags
// same-name tools from different extension paths at load-finalization).
// A deferred registration is absent at load time → no conflict. The
// override still wins: CLI `-e` extensions load FIRST and the tool merge
// is first-wins (getAllRegisteredTools keeps the FIRST registration per
// name), and `registerTool` auto-calls runtime.refreshTools() (no manual
// refresh needed).
//
// NOT registered here: `subagent` + `list_agents` (out of scope —
// `subagent` is already desktop-executed; overriding it would break
// named-agent / parallel / async dispatch).
import { connect, type Socket } from "node:net";
import { randomUUID } from "node:crypto";

export default (pi: any) => {
  // Platform gate FIRST: the desktop's bridge LISTENER only exists on
  // Linux (`bridge.rs` `#[cfg(windows)]` / `#[cfg(target_os = "macos")]`
  // are no-op stubs). On Windows `bridge_spawn_setup` STILL sets the
  // four env vars (`bridge::available()` is true there), so the env check
  // alone would let the override register + win and then FAIL every
  // `execute()` at `connect()` — regressing `manage_todo_list`
  // (agent-local today). The platform check keeps the extension inert on
  // Windows/macOS (the suite's original tools remain — no regression).
  if (process.platform !== "linux") return;
  const bridgeEnv = process.env;
  const gated =
    bridgeEnv.PI_ARCHIMEDES_BRIDGE === "1" &&
    !!bridgeEnv.PI_ARCHIMEDES_BRIDGE_SOCKET &&
    !!bridgeEnv.PI_ARCHIMEDES_BRIDGE_SESSION &&
    !!bridgeEnv.PI_ARCHIMEDES_BRIDGE_SERVER_PID;
  if (!gated) return; // inert: the suite's original tools remain

  // TEST-ONLY SEAM: expose `bridgeRequest` for the vitest client tests
  // (`tests/tools_bridgeclient.test.mjs` drives the REAL node:net socket
  // client against a real Unix-socket server — the failure paths the
  // selfgate test can't reach). Inert in production: pi never reads
  // `pi.__test`, and the seam is attached ONLY after the platform + env
  // gates pass (an inert load exposes nothing). `bridgeRequest` stays
  // INLINE — pi loads this file standalone (embedded via `include_str!`),
  // so the client cannot live in a separate module.
  pi.__test = { bridgeRequest };

  // ── Bridge client ────────────────────────────────────────────────────
  // One FRESH connection per request (the bridge is one-connection-per-
  // message: the desktop answers on the SAME connection then closes it).
  // The agent process is a descendant of the desktop, so the peer-
  // verification passes. `source` is the literal "main" for the tool
  // contract (matches the suite, which also hardcodes `{source: "main"}`
  // — packages/todo/src/tool.ts); subagent todo COLUMNS on the board
  // remain unfed in Phase 2 (status quo).

  function bridgeRequest<T = any>(
    method: string,
    params: any,
    signal: AbortSignal | undefined,
    timeoutMs: number | null, // null = no timeout (matches the suite's channel.ts: undefined = 5 min default, null = none)
  ): Promise<T> {
    const socketPath = bridgeEnv.PI_ARCHIMEDES_BRIDGE_SOCKET!;
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      let conn: Socket | undefined; // declared BEFORE fail (a pre-aborted signal calls fail() before connect() — conn?.destroy() must not throw a ReferenceError; the suite handles pre-aborted signals, core/bridge/index.ts:101-104)
      let timer: NodeJS.Timeout | undefined;
      let onAbort: (() => void) | undefined;
      // All terminal paths funnel through here: idempotent behind the
      // `settled` guard, and it CLEANS UP the timer + the abort listener
      // (so a pre-aborted-signal reject or a timeout does not pin the
      // event loop or leak a stale `abort` listener on the turn's
      // long-lived AbortSignal — the suite removes its listener on
      // settle, core/bridge/index.ts:102-107). `timer` / `onAbort` are
      // declared (not yet assigned) above so the references are safe:
      // `fail` is only ever called after both are assigned.
      const fail = (e: Error) => {
        if (!settled) {
          settled = true;
          if (timer) clearTimeout(timer);
          if (onAbort && signal) signal.removeEventListener("abort", onAbort);
          conn?.destroy();
          reject(e);
        }
      };
      // Arm the timer ONLY when timeoutMs > 0 (a 0 / negative / null
      // value = no timer).
      if (timeoutMs !== null && timeoutMs > 0) {
        timer = setTimeout(() => fail(new Error("timeout")), timeoutMs);
      }
      // Wire the tool's AbortSignal → destroy the socket (the desktop sees
      // EOF → aborts in-flight execution).
      onAbort = () => fail(new Error("cancelled"));
      if (signal) {
        if (signal.aborted) {
          onAbort!();
          return;
        }
        signal.addEventListener("abort", onAbort!, { once: true });
      }
      conn = connect(socketPath);
      let buf = "";
      conn.on("data", (c: Buffer) => {
        buf += c.toString();
        const nl = buf.indexOf("\n");
        if (nl === -1) return;
        if (timer) clearTimeout(timer);
        if (signal) signal.removeEventListener("abort", onAbort);
        const line = buf.slice(0, nl);
        conn.destroy();
        let frame: any;
        try {
          frame = JSON.parse(line);
        } catch (e: any) {
          return fail(new Error(`bad bridge frame: ${e.message}`));
        } // try/catch — an uncaught throw in a socket handler crashes the agent
        if (frame.type !== "response") return fail(new Error("unexpected bridge frame"));
        if (frame.error !== undefined) {
          return fail(
            Object.assign(new Error(frame.error), { isError: true, raw: frame }),
          );
        } // error frame → a typed error via `fail` (which cleans up the timer + abort listener); the override maps it to an isError result
        settled = true;
        resolve(frame.result); // success path: `fail`-style cleanup already done above (timer + listener cleared before this line)
      });
      conn.on("error", (e) => fail(e));
      conn.on("close", () => fail(new Error("bridge connection closed"))); // a clean close with NO `error` event is terminal (the suite's channel.ts:19-21,189 — peer-verification rejection, an unparseable/oversized frame, or a desktop crash). REQUIRED for `sudo_exec` (`timeoutMs: null` = no timer): without it the promise never settles and the whole agent turn hangs until a manual abort.
      conn.on("connect", () => {
        conn.write(
          JSON.stringify({ v: 1, type: "request", id: randomUUID(), method, source: "main", params }) + "\n",
        );
      });
    });
  }

  // ── `ask` override ───────────────────────────────────────────────────
  // Schemas copied VERBATIM from the suite (packages/ask/src/tool.ts
  // AskParamsSchema + the ASK_TOOL_DESCRIPTION / label "Ask").
  const OptionItemSchema = {
    type: "object",
    properties: { label: { type: "string", description: "Display label" } },
    required: ["label"],
  };
  const QuestionItemSchema = {
    type: "object",
    properties: {
      id: { type: "string", description: "Question id (e.g. auth, cache, priority)" },
      question: { type: "string", description: "Question text" },
      description: {
        type: "string",
        description:
          "Optional context in Markdown/plain text. Rendered above options with wrapping (supports headings/lists/code blocks).",
      },
      options: {
        type: "array",
        description: "Available options. Do not include 'Other'.",
        items: OptionItemSchema,
        minItems: 1,
      },
      multi: { type: "boolean", description: "Allow multi-select" },
      recommended: {
        type: "number",
        description: "0-indexed recommended option. '(Recommended)' is shown automatically.",
      },
    },
    required: ["id", "question", "options"],
  };
  const AskParamsSchema = {
    type: "object",
    properties: {
      questions: { type: "array", description: "Questions to ask", items: QuestionItemSchema, minItems: 1 },
    },
    required: ["questions"],
  };
  const ASK_TOOL_DESCRIPTION = `
Ask the user for clarification when a choice materially affects the outcome.

- Use when multiple valid approaches have different trade-offs.
- Prefer 2-5 concise options.
- Use multi=true when multiple answers are valid.
- Use recommended=<index> (0-indexed) to mark the default option.
- Use description to provide Markdown/plain context (supports long explanations and structure diagrams).
- You can ask multiple related questions in one call using questions[].
- Do NOT include an 'Other' option; UI adds it automatically.
`.trim();

  // Session-text helpers — ported VERBATIM from the suite
  // (packages/ask/src/tool.ts:68-142 + buildAskSessionContent:181 +
  // responseToResults:192). The desktop returns the raw
  // AskResponsePayload {cancelled, results}; the override maps it to
  // {content, details} so the LLM sees the suite's text (not machine
  // JSON).
  function sanitizeForSessionText(value: string): string {
    return value
      .replace(/[\r\n\t]/g, " ")
      .replace(/[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]/g, "")
      .replace(/\s{2,}/g, " ")
      .trim();
  }

  function sanitizeMultilineForSessionText(value: string): string {
    return value
      .replace(/\r\n/g, "\n")
      .replace(/\r/g, "\n")
      .split("\n")
      .map((line) => sanitizeForSessionText(line))
      .join("\n")
      .trim();
  }

  function sanitizeOptionForSessionText(option: string): string {
    const sanitizedOption = sanitizeForSessionText(option);
    return sanitizedOption.length > 0 ? sanitizedOption : "(empty option)";
  }

  function toSessionSafeQuestionResult(result: any): any {
    const selectedOptions = result.selectedOptions
      .map((selectedOption: string) => sanitizeForSessionText(selectedOption))
      .filter((selectedOption: string) => selectedOption.length > 0);

    const rawDescription = result.description;
    const description = rawDescription == null ? undefined : sanitizeMultilineForSessionText(rawDescription);
    const rawCustomInput = result.customInput;
    const customInput = rawCustomInput == null ? undefined : sanitizeForSessionText(rawCustomInput);

    return {
      id: sanitizeForSessionText(result.id) || "(unknown)",
      question: sanitizeForSessionText(result.question) || "(empty question)",
      description: description && description.length > 0 ? description : undefined,
      options: result.options.map(sanitizeOptionForSessionText),
      multi: result.multi,
      selectedOptions,
      customInput: customInput && customInput.length > 0 ? customInput : undefined,
    };
  }

  function formatSelectionForSummary(result: any): string {
    const hasSelectedOptions = result.selectedOptions.length > 0;
    const hasCustomInput = Boolean(result.customInput);

    if (!hasSelectedOptions && !hasCustomInput) {
      return "(cancelled)";
    }

    if (hasSelectedOptions && hasCustomInput) {
      const selectedPart = result.multi
        ? `[${result.selectedOptions.join(", ")}]`
        : result.selectedOptions[0] ?? "";
      return `${selectedPart} + Other: "${result.customInput}"`;
    }

    if (hasCustomInput) {
      return `"${result.customInput}"`;
    }

    if (result.multi) {
      return `[${result.selectedOptions.join(", ")}]`;
    }

    return result.selectedOptions[0] ?? "";
  }

  function formatQuestionResult(result: any): string {
    return `${result.id}: ${formatSelectionForSummary(result)}`;
  }

  function formatQuestionContext(result: any, questionIndex: number): string {
    const lines: string[] = [`Question ${questionIndex + 1} (${result.id})`, `Prompt: ${result.question}`];

    if (result.description) {
      lines.push("Context:");
      for (const descriptionLine of result.description.split("\n")) {
        lines.push(`  ${descriptionLine}`);
      }
    }

    lines.push("Options:");
    lines.push(...result.options.map((option: string, optionIndex: number) => `  ${optionIndex + 1}. ${option}`));
    lines.push("Response:");

    const hasSelectedOptions = result.selectedOptions.length > 0;
    const hasCustomInput = Boolean(result.customInput);

    if (!hasSelectedOptions && !hasCustomInput) {
      lines.push("  Selected: (cancelled)");
      return lines.join("\n");
    }

    if (hasSelectedOptions) {
      const selectedText = result.multi
        ? `[${result.selectedOptions.join(", ")}]`
        : result.selectedOptions[0];
      lines.push(`  Selected: ${selectedText}`);
    }

    if (hasCustomInput) {
      if (!hasSelectedOptions) {
        lines.push(`  Selected: Other (type your own)`);
      }
      lines.push(`  Custom input: ${result.customInput}`);
    }

    return lines.join("\n");
  }

  function buildAskSessionContent(results: any[]): string {
    const safeResults = results.map(toSessionSafeQuestionResult);
    const summaryLines = safeResults.map(formatQuestionResult).join("\n");
    const contextBlocks = safeResults.map((result, index) => formatQuestionContext(result, index)).join("\n\n");
    return `User answers:\n${summaryLines}\n\nAnswer context:\n${contextBlocks}`;
  }

  /**
   * Build QuestionResult[] from an AskResponsePayload-shaped response
   * (ported from the suite's responseToResults, packages/ask/src/tool.ts:192).
   */
  function responseToResults(
    response: { cancelled: boolean; results: Array<{ id: string; selectedOptions: string[]; customInput?: string }> },
    questions: any[],
  ): any[] {
    return questions.map((q, i) => {
      const r = response.results[i];
      return {
        id: q.id,
        question: q.question,
        description: q.description && q.description.trim().length > 0 ? q.description : undefined,
        options: q.options.map((o: any) => o.label),
        multi: q.multi ?? false,
        selectedOptions: r?.selectedOptions ?? [],
        customInput: r?.customInput ?? undefined,
      };
    });
  }

  /**
   * Shape the desktop's raw AskResponsePayload into the tool result the LLM
   * sees (the suite's cancelled / answer shapes, packages/ask/src/tool.ts:260-263
   * + buildAskSessionContent).
   */
  function shapeAskResult(r: any, params: any) {
    const results = responseToResults(r, params.questions);
    if (r && r.cancelled && results.every((x: any) => x.selectedOptions.length === 0)) {
      return {
        content: [{ type: "text", text: "User cancelled the question." }],
        details: { results, customInput: undefined, description: undefined },
      };
    }
    return {
      content: [{ type: "text", text: buildAskSessionContent(results) }],
      details: { results, customInput: undefined, description: undefined },
    };
  }

  function registerAskOverride(pi: any) {
    pi.registerTool({
      name: "ask",
      label: "Ask",
      description: ASK_TOOL_DESCRIPTION,
      parameters: AskParamsSchema,
      async execute(_toolCallId: string, params: any, signal: AbortSignal | undefined, _onUpdate: undefined, _ctx: any) {
        // The desktop's generic ask path writes error:"cancelled" on
        // timeout / session close / EOF (bridge.rs — common paths), so a
        // rejection is mapped to the suite's clean cancelled shape (NOT a
        // thrown error). timeoutMs: 300_000 (5 min — the suite's channel
        // default; the desktop's waiter is 330 s, so the agent's 300 s
        // cancel deterministically wins with a 30 s margin; a 330_000
        // client timeout would RACE the desktop's own 330 s waiter).
        let r: any;
        try {
          r = await bridgeRequest("ask", params, signal, 300_000);
        } catch {
          r = { cancelled: true, results: params.questions.map((q: any) => ({ id: q.id, selectedOptions: [] })) };
        }
        return shapeAskResult(r, params);
      },
    });
  }

  // ── `sudo_exec` override ─────────────────────────────────────────────
  // Schemas copied VERBATIM from the suite (packages/sudo/src/tool.ts
  // SudoExecParamsSchema + the SUDO_EXEC_DESCRIPTION / label "sudo").
  const SudoExecParamsSchema = {
    type: "object",
    properties: {
      command: {
        type: "string",
        description:
          'Exact command and arguments to run with elevated privileges, e.g. "apt install ripgrep". Do NOT include a leading \'sudo\'. Executed directly via argv — no shell: avoid pipes, redirects, &&, env assignments, or quotes-as-syntax; pass multiple args space-separated, quote only literal args.',
      },
      reason: {
        type: "string",
        description: "Human-readable explanation of why this privileged command is needed, shown to the user before execution.",
      },
      timeoutMs: { type: "number", description: "Optional timeout in milliseconds (default from config)." },
    },
    required: ["command", "reason"],
  };
  const SUDO_EXEC_DESCRIPTION = `Run a command with elevated privileges using sudo.

- BEFORE any credential is requested, the exact command and its reason are shown to the user for confirmation.
- The password is entered only through a masked UI, passed to sudo via stdin, cached in memory for the session, and never exposed.
- Use this instead of sudo in bash — interactive sudo in bash is blocked.
- The command is executed as argv, NOT through a shell: no pipes, redirects, &&, ;, or env assignments; pass args space-separated, quote only literal args.`;

  function registerSudoOverride(pi: any) {
    pi.registerTool({
      name: "sudo_exec",
      label: "sudo",
      description: SUDO_EXEC_DESCRIPTION,
      parameters: SudoExecParamsSchema,
      async execute(_toolCallId: string, params: any, signal: AbortSignal | undefined, _onUpdate: undefined, _ctx: any) {
        // The desktop's sudo_exec handler is the single confirm and returns
        // the shaped {content, details} (with isError on failure) — the
        // override passes it through verbatim (the desktop owns the
        // shaping). timeoutMs: null = no client-side timeout (the desktop's
        // handler has its own 330 s cap; cancellation is the
        // signal / EOF).
        const r = await bridgeRequest("sudo_exec", params, signal, null);
        return r;
      },
    });
  }

  // ── `manage_todo_list` override ──────────────────────────────────────
  // Schemas copied VERBATIM from the suite (packages/todo/src/tool.ts
  // ManageTodoListParams + TOOL_DESCRIPTION / label "Todo List"), plus
  // prepareArguments (ported from packages/todo/src/prepare-args.ts — it
  // repairs malformed model args BEFORE schema validation; dropping it
  // regresses arg handling).
  const TodoItemSchema = {
    type: "object",
    properties: {
      content: {
        type: "string",
        description: 'Short imperative label for the task (3-10 words). Displayed in UI. Example: "Fix the auth middleware".',
      },
      status: {
        type: "string",
        enum: ["pending", "in_progress", "completed"],
        description:
          "pending: Not begun | in_progress: Currently working (multiple allowed for parallel work/subagents) | completed: Fully finished with no blockers",
      },
      description: {
        type: "string",
        description: "Optional detailed context: file paths, specific methods, or acceptance criteria.",
      },
    },
    required: ["content", "status"],
  };
  const ManageTodoListParams = {
    type: "object",
    properties: {
      operation: {
        type: "string",
        enum: ["write", "read"],
        description:
          "write: Replace entire todo list with new content. read: Retrieve current todo list. ALWAYS provide complete list when writing - partial updates not supported.",
      },
      todoList: {
        type: "array",
        description:
          "Complete array of all todo items (required for write operation, ignored for read). Must include ALL items - both existing and new.",
        items: TodoItemSchema,
      },
    },
    required: ["operation"],
  };
  const TODO_TOOL_DESCRIPTION = `Manage a structured todo list to track progress and plan tasks throughout your coding session. Use this tool VERY frequently to ensure task visibility and proper planning.

When to use this tool:
- Complex multi-step work requiring planning and tracking
- When user provides multiple tasks or requests (numbered, comma-separated)
- After receiving new instructions that require multiple steps
- BEFORE starting work on any todo (mark as in_progress)
- IMMEDIATELY after completing each todo (mark completed individually)
- When breaking down larger tasks into smaller actionable steps
- To give users visibility into your progress and planning

When NOT to use:
- Single, trivial tasks that can be completed in one step
- Purely conversational/informational requests
- When just reading files or performing simple searches

CRITICAL workflow:
1. Plan tasks by writing todo list with specific, actionable items
2. Mark todo(s) as in_progress before starting work
3. Complete the work for that specific todo
4. Mark that todo as completed IMMEDIATELY
5. Move to next todo and repeat

Todo item shape:
{"content": "Fix the auth middleware", "status": "pending", "description": "optional: file paths, acceptance criteria"}
- content: the short displayed label — do NOT assign ids (numbering is order)
- status: exactly one of pending | in_progress | completed

Todo states:
- pending: Todo not yet begun
- in_progress: Currently working (multiple allowed for parallel work/subagents)
- completed: Finished successfully

IMPORTANT: Mark todos completed as soon as they are done. Do not batch completions.
When all todos are completed, the list auto-clears after a brief delay.`;

  // prepareArguments — ported VERBATIM from packages/todo/src/prepare-args.ts
  // (repairs malformed model args BEFORE schema validation).
  type Record_ = Record<string, unknown>;

  const VALID_STATUSES: ReadonlySet<string> = new Set<string>([
    "pending",
    "in_progress",
    "completed",
  ]);

  const STATUS_ALIASES: ReadonlyMap<string, string> = new Map<string, string>([
    ["done", "completed"],
    ["complete", "completed"],
    ["finished", "completed"],
    ["closed", "completed"],
    ["success", "completed"],
    ["passed", "completed"],
    ["doing", "in_progress"],
    ["started", "in_progress"],
    ["working", "in_progress"],
    ["active", "in_progress"],
    ["wip", "in_progress"],
    ["ongoing", "in_progress"],
    ["inprogress", "in_progress"],
    ["pending", "pending"],
    ["todo", "pending"],
    ["planned", "pending"],
    ["untouched", "pending"],
    ["notstarted", "pending"],
    ["unstarted", "pending"],
    ["open", "pending"],
  ]);

  const CONTENT_FALLBACK_KEYS = ["title", "step", "task", "text", "name", "label", "activeForm"] as const;
  const DESCRIPTION_FALLBACK_KEYS = ["details", "notes", "summary", "context"] as const;

  const LAST_RESORT_SKIP_KEYS = new Set<string>([
    "content",
    ...(CONTENT_FALLBACK_KEYS as readonly string[]),
    "description",
    ...(DESCRIPTION_FALLBACK_KEYS as readonly string[]),
    "status",
    "id",
  ]);

  const DERIVED_CONTENT_MAX = 60;

  function isRecord(value: unknown): value is Record_ {
    return typeof value === "object" && value !== null && !Array.isArray(value);
  }

  function firstStringOf(record: Record_, keys: readonly string[]): string {
    for (const key of keys) {
      const value = record[key];
      if (typeof value === "string" && value.trim() !== "") return value;
    }
    return "";
  }

  function deriveTitleFromDescription(description: string): string {
    const flat = description.replace(/\s+/g, " ").trim();
    if (flat.length <= DERIVED_CONTENT_MAX) return flat;
    const cut = flat.slice(0, DERIVED_CONTENT_MAX);
    const lastSpace = cut.lastIndexOf(" ");
    const head = lastSpace > 0 ? cut.slice(0, lastSpace) : cut;
    return `${head.trimEnd()}…`;
  }

  function normalizeStatus(raw: unknown): string {
    if (typeof raw !== "string") {
      // Missing, null, number, … — no signal about intent.
      return "pending";
    }
    const s = raw.trim().toLowerCase();
    if (VALID_STATUSES.has(s)) return s;
    const collapsed = s.replace(/[\s_-]+/g, "");
    return STATUS_ALIASES.get(collapsed) ?? "pending";
  }

  function normalizeTodoItem(raw: unknown, _index: number): unknown {
    // Laziest form: ["write tests", "deploy"]
    if (typeof raw === "string") {
      const text = raw.trim();
      if (text === "") return raw;
      return { content: deriveTitleFromDescription(text), status: "pending" };
    }

    if (!isRecord(raw)) return raw; // null / number / nested array → schema error

    const description =
      (typeof raw.description === "string" ? raw.description.trim() : "") ||
      firstStringOf(raw, DESCRIPTION_FALLBACK_KEYS);
    let content =
      (typeof raw.content === "string" ? raw.content.trim() : "") ||
      firstStringOf(raw, CONTENT_FALLBACK_KEYS);
    if (content === "" && description !== "") {
      content = deriveTitleFromDescription(description);
    }
    // Last resort: any remaining non-empty string field that isn't a known
    // alias/status/id. This alone is the catch-all.
    if (content === "") {
      for (const [key, value] of Object.entries(raw)) {
        if (!LAST_RESORT_SKIP_KEYS.has(key) && typeof value === "string" && value.trim() !== "") {
          content = value;
          break;
        }
      }
    }

    // If both are empty this returns { content: "", status } — deliberately
    // keeps an empty (valid-string) field so validation reports the missing
    // `content` instead of the schema masking it.
    return {
      content,
      ...(description !== "" ? { description } : {}),
      status: normalizeStatus(raw.status),
    };
  }

  function looksLikeTodoItem(value: unknown): boolean {
    return (
      isRecord(value) &&
      ("content" in value ||
        "title" in value ||
        "step" in value ||
        "description" in value ||
        "status" in value)
    );
  }

  function normalizeTodoList(raw: unknown): { value: unknown; kept: boolean } {
    let value: unknown = raw;
    if (typeof raw === "string") value = parseStringifiedList(raw);

    if (value === null) {
      // Required-schema fields reject null; dropping the key yields the
      // clearer "todoList is required for write operation" error.
      return { value: undefined, kept: true };
    }

    if (value === undefined) return { value, kept: false };

    if (Array.isArray(value)) {
      return { value: value.map((item, i) => normalizeTodoItem(item, i)), kept: true };
    }

    // A single bare item instead of an array.
    if (looksLikeTodoItem(value)) {
      return { value: [normalizeTodoItem(value, 0)], kept: true };
    }

    return { value, kept: true }; // Unmendable — original validation error stands
  }

  // Repair the occasional mangled JSON: `"id": 1"` (an extra quote after
  // the number). The replacement is safe to apply blindly — that shape is
  // invalid JSON to begin with.
  // The long backslash sequence is spelled out as char codes for robustness.
  const ID_FIX_PATTERN = String.fromCharCode(40, 34, 105, 100, 34, 92, 115, 42, 58, 92, 115, 42, 92, 100, 43, 41, 92, 34);
  const ID_QUOTE_FIX = new RegExp(ID_FIX_PATTERN, "g");
  function repairStringifiedJson(s: string): string {
    return s.replace(ID_QUOTE_FIX, "$1");
  }

  function parseStringifiedList(raw: string): unknown {
    try {
      return JSON.parse(repairStringifiedJson(raw));
    } catch {
      return raw; // Unparseable — keep the string; schema error says "must be array"
    }
  }

  function normalizeOperation(raw: unknown, listPresent: boolean): "write" | "read" {
    if (raw === "write" || raw === "read") return raw;
    if (typeof raw === "string") {
      const lowered = raw.trim().toLowerCase();
      if (lowered === "write" || lowered === "read") return lowered;
    }
    // Model omitted/garbled the operation: a list implies a write.
    return listPresent ? "write" : "read";
  }

  /**
   * Repair raw `manage_todo_list` call arguments before schema validation.
   * Never throws, never mutates its input, and passes through anything it
   * cannot recover.
   */
  function prepareTodoArguments(args: unknown): any {
    if (!isRecord(args)) return args;

    const normalized =
      args.todoList === undefined ? undefined : normalizeTodoList(args.todoList);
    const present =
      normalized !== undefined && normalized.kept && normalized.value !== undefined;

    const out: Record_ = {
      operation: normalizeOperation(args.operation, present),
    };
    if (present) {
      out.todoList = (normalized as { value: unknown }).value;
    }
    return out;
  }

  function registerTodoOverride(pi: any) {
    pi.registerTool({
      name: "manage_todo_list",
      label: "Todo List",
      description: TODO_TOOL_DESCRIPTION,
      parameters: ManageTodoListParams,
      prepareArguments: prepareTodoArguments,
      async execute(_toolCallId: string, params: any, signal: AbortSignal | undefined, _onUpdate: undefined, _ctx: any) {
        // The desktop's todo_update handler returns the shaped
        // {content, details} — the override passes it through.
        const r = await bridgeRequest("todo_update", params, signal, 330_000);
        return r;
      },
    });
  }

  // DEFERRED registration: `registerTool` at LOAD time would conflict with
  // the suite's same-name tools (process.exit(1) — see the module doc).
  // `registerTool` adds the tool AND auto-calls runtime.refreshTools(), so
  // a deferred registration is picked up with no manual refresh.
  pi.on("session_start", () => {
    registerAskOverride(pi);
    registerSudoOverride(pi);
    registerTodoOverride(pi);
  });
};
