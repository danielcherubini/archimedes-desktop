# Archimedes Desktop

A cross-platform (Windows/macOS/Linux) desktop app, built with Tauri 2, that runs coding agents in-process — the desktop's own Rust runtime is the only **Agent harness** (a **Native session** — ADR 0022).

## Language

**Client**:
The Archimedes Desktop application itself — the **Client** role: it is the **Supervisor** (worker lifecycle + sole SQLite writer + the response side of the permission/interactive gates — ADR 0025), renders the conversation, and provides the settings/skills/MCP surfaces.
_Avoid_: App, frontend, IDE, desktop (too generic)

**Agent**:
The conversation *partner* — the thing that produces the assistant's responses. Always embodied in the desktop's own **Agent harness** (a **Native session** — ADR 0022), which runs in a **Worker process** (ADR 0025) — never an external agent process. _Generalized 2026-10-06: a native session has no subprocess; the partner is embodied in the desktop's own runtime. 2026-10-02: the external (pi) embodiment is removed — the harness is the desktop's own runtime, full stop. 2026-10-04: the harness runs in a Worker process (ADR 0025) — still the desktop's own binary, never an external agent._
_Avoid_: Subagent, worker, bot, assistant. (Note: in the pi-archimedes project "Agent" means a subagent configuration — different meaning, different project. "Subagent session" is the desktop's term for a desktop-spawned delegated session — see its entry below.)

**Agent harness**:
The machinery that runs an agent conversation end-to-end — the model loop (model call → tool dispatch → retry/compaction) + the tool registry + session persistence + provider integration. A harness is a *runtime*, not a model: the desktop's own Rust runtime is the **only** harness (ADR 0022 — the external pi harness embodiment is removed), running in a **Worker process** (ADR 0025). The harness owns the conversation's *control flow*; the model only produces tokens, the tools only act on the world.
_Avoid_: Agent (that's the conversation *partner*; a harness *runs* it), loop, brain, runtime (too generic)

**Space**:
A single on-disk folder the Client can open — the workspace in which a conversation and its file access happen. Identified by the folder's canonical path, not a user-supplied name; the display label is the folder's base name. v1: one active conversation per Space — its most recent live **Session** (multiple live Sessions may coexist app-wide: the one-live policy was lifted 2026-09-22); stored conversations of a Space survive.
_Avoid_: Project, workspace, folder, directory, environment

**Session**:
One live conversation, backed by the desktop's **Agent harness** running in a **Worker process** (a **Native session** — ADR 0022/0025; the external embodiment is removed). The unit of lifecycle, history, and permission state. A Session lives inside one **Space**: its `cwd` (and fs sandbox root) is the Space's folder; a Space's active conversation is its most recent Session. _Generalized 2026-10-06: a native session has no external subprocess; embodiment is a harness, not an agent process. 2026-10-04: the harness runs in a Worker process (ADR 0025) — the session's embodiment is a Worker, and the Client is its Supervisor._
_Avoid_: Conversation, chat, thread, run

**Native session**:
A **Session** — the desktop's own Rust runtime is the **only** **Agent harness** (ADR 0022): no external agent process. Runs in a **Worker process** (ADR 0025) driven by the desktop's own AgentLoop, which calls the model directly (an OpenAI-compatible provider) and executes tools in the Worker (Rust executors); the **Supervisor** persists the transcript to SQLite from the event stream (the Worker has no DB).
_Avoid_: In-process session, local session, built-in session

**Worker process**:
One child OS process (the desktop's own binary, self-exec `archimedes --worker` — ADR 0025) that runs ONE session's **Agent harness** — the `AgentLoop` + tool executors + provider client — speaking a stdio JSONL protocol to its **Supervisor**. Worker lifetime = session lifetime (a main session's Worker is reaped at session end / app quit; a subagent's after `agent_settled`). The Worker has no DB (its `SessionStore` is a no-op; the conversation is in-memory) and never reads `settings.json` (the resolved provider config arrives in the protocol envelope). A Worker crash degrades to a **Stalled session** or a `SubagentOutcome::Failed` tool error — never an app death.
_Avoid_: Child process, agent process, harness process

**Supervisor**:
The Tauri app process (the **Client**) in the Worker-process architecture (ADR 0025): React UI + WorkerManager (spawn / reap / crash-detect / abort-all) + sole SQLite writer (persists transcripts from the Worker event stream) + provider catalog + the response side of the permission/interactive gates (relays the user's answers back over the protocol).
_Avoid_: Main process, parent, daemon

**Stalled session**:
A **Session** whose **Worker process** died (a crash, or a spawn failure) — the transcript is intact (persisted as events arrived), the last turn is incomplete (no `agent_settled` = incomplete), and the session offers a **Resume** action (a fresh Worker + the Supervisor-rehydrated transcript). A view/lifecycle state, distinct from **Archived** (a user action) and from a running session.
_Avoid_: Dead session, crashed session, orphaned session

**Archived session**:
A stored **Session** hidden from its **Space** group by an explicit user action (the `archived` flag on the `sessions` row) — listed in the sidebar's **Archived** section instead of its Space group. Archiving is a *view* property, not a lifecycle one: the transcript stays in the desktop's storage, the session is still openable and resumable (first send resumes it, as for any stored session), and the flag is **sticky** — it survives resume/pause and changes only via explicit archive/unarchive. Delete is offered only from the Archived section (ADR 0016).
_Avoid_: Paused session, closed session, hidden session

**Subagent session**:
A **Session** the desktop spawns to run a task delegated by the main agent — a **Worker process** running a native **AgentLoop** child (the parent's model/tools minus `subagent` and `list_agents`, plus optional `launch` overrides; its `agentName` resolves against discovered **Agent definitions** — ADR 0020; an unknown/omitted name is a config-less label-only dispatch). Unlike a **Session**, it is not a user-facing conversation — it exists to complete the delegated task, and its progress renders in the Client through the same event pipeline as a Session (its transcript is persisted as a hidden ephemeral row — ADR 0025). The subagent's interactive tools (ask, sudo_exec) run on the Worker's **interactive** channel, so their prompts go directly to the Client (the Supervisor's relay) without relaying through the main agent. _Generalized 2026-10-01: a native-native subagent has no pi process and no bridge. 2026-10-02: the bridge-mode embodiment is removed (ADR 0022). 2026-10-04: a subagent session is a Worker process (ADR 0025) — ephemeral, no resume; a crash degrades to a `SubagentOutcome::Failed` tool error._
_Avoid_: Worker, delegated task, child session, background session

**Agent definition**:
A user-authored markdown file (flat, in a standard agents dir) with YAML frontmatter (`name`, `description`, `model`, `thinking`, `tools`) + a system-prompt body — discovered by the desktop like a **Skill** (space-level `.agents/agents` + `.pi/agents` walked to the repo root; user-level `~/.agents/agents` + `~/.pi/agent/agents`). Selectable in a **Subagent session** via the `subagent` tool's `agentName` (layered under explicit params — ADR 0020); advertised by the `list_agents` tool. Distinct from **Agent** (the conversation partner) — an Agent definition is a user-authored prompt + config, not a runtime.
_Avoid_: Agent config, agent preset, subagent preset

**Subagent model override**:
A per-agent model override in the app's settings (`settings.json` `subagentModels`: agent name → model key) — layered between the explicit `subagent` tool `model` param and the **Agent definition**'s frontmatter (explicit > override > frontmatter > parent model — ADR 0023); a stale override degrades to the frontmatter layer (never fails the dispatch); the agent's markdown file is never modified.
_Avoid_: Agent config, model preset, subagent default model

**Provider**:
A user-managed model endpoint in the app's settings (`settings.json` `providers`: `{ id, name, baseUrl, apiKey, api, keyUrl? }`) — the desktop's SOLE model source (ADR 0014): each provider's models are discovered live from its endpoint, and its `api` field selects the endpoint FLAVOR — the wire the harness speaks AND the discovery mode (ADR 0024 + ADR 0026): `openai-completions` (default — `GET /models` discovery, OpenAI chat-completions wire) / `anthropic-messages` (Anthropic `GET /models`, Anthropic Messages wire) / `openai-responses` (OpenAI `GET /models`, Responses wire) / `litellm` (LiteLLM `GET {base}/model/info` discovery, OpenAI chat-completions wire — the `litellm` value is a discovery mode, NOT a fourth wire). The base model catalog is empty; a provider that discovers 0 models still shadows the base for its id (a no-op with a single source).
_Avoid_: Connection, model source, LLM endpoint

**Thinking level** (a LiteLLM `reasoning_effort` level):
One entry of a LiteLLM-discovered model's `thinking_levels` (e.g. `none` / `low` / `medium` / `xhigh` — the model's own vocabulary, surfaced VERBATIM from `model_info.reasoning_effort_levels`, ADR 0026). Distinct from the desktop's own thinking-level vocabulary (`off` / `low` / `high` / `xhigh`): a `litellm` provider's selector shows the proxy's words and sends the chosen level onto the wire as `reasoning_effort` AS-IS (no `off`↔`none` normalization). The per-model remembered level (`defaultThinkingLevels`, keyed `provider/model`) already handles the cross-vocabulary case.
_Avoid_: reasoning effort (the wire param name, not the UI term), thinking budget (the Anthropic mapping)

**Known provider**:
A built-in provider template in the desktop's known-providers catalog (a Rust constant seeded from the ZCode builtin provider catalog, ADR 0024) — `name` + `base URL` + `wire API` + `key-management URL`, offered in the Settings' Providers section as a picker. Picking a template adds a PRE-FILLED **Provider** row (empty API key — the user pastes the key); the template itself is not a provider (it stores no key and discovers nothing). The user's provider list remains the sole model source.
_Avoid_: Built-in provider (that would read as a model source), provider preset, provider template (the user-facing term is "known provider")

**Permission prompt**:
The Client's UI response to the native harness's in-process **permission gate** (the `permission-request` event before every mutating tool — ADR 0010) — the user approves or denies a tool call.
_Avoid_: Approval dialog, consent prompt, confirm

**Trusted Space**:
A Space flagged as trusted — its Sessions (including Subagent sessions) skip the **Permission prompt** for the gated tools (`bash`/`edit`/`write`), which the Client auto-approves. `sudo_exec` (its own confirm + password modal) and `ask` (a `select` request, not a confirm) are unaffected. Trust is stored per-Space, defaults to off, and is enforced in the native harness's in-process permission gate (ADR 0010).
_Avoid_: Auto-approve mode, trust mode, yolo mode

**Config option**:
A per-session selector the harness re-synthesizes in-process from the `ModelCatalog` (the model / thinking-level selectors — the `synthesize_catalog_config_options` shape) and the Client updates via the `set_session_config_option` command. The harness is the source of truth: the Client does not persist config options; a resume re-synthesizes fresh state from the stored model + the catalog.
_Avoid_: Model list, model picker, settings, preferences

**Thinking block**:
The collapsible UI unit that shows the agent's streamed internal reasoning (the harness's streamed thinking events) — one per contiguous thinking run, collapsed by default with a live one-line summary while streaming. In the transcript data model it is a message of kind `agent-thought`.
_Avoid_: Thinking tokens (reads as a token-count statistic), reasoning block (ZCode's term; the Client's UI says "Thinking…"/"Thought")

**Attachment**:
An image staged in the composer (via clipboard paste or drag-and-drop) that is sent with the next prompt. Previewed as a removable thumbnail in a strip above the composer's textarea; persisted inline in the user message's payload.
_Avoid_: File, image, media, clip, paperclip

**Theme**:
The light/dark **mode** of the Client's UI — the persisted `theme` setting (`system` / `dark` / `light`), resolved to an `AppTheme` (`zai-light` / `zai-dark`) by `src/lib/theme.ts`, where `system` follows the OS scheme. A Theme selects *how light or dark the app is*; it says nothing about which hues the app uses — that is the **Palette**. The two are separate settings (ADR 0027).
_Avoid_: Color scheme, skin, appearance (the Settings *section* that hosts both controls), style

**Palette**:
The Client's **color scheme** — the persisted `palette` setting (`zai` / `dracula`), applied as a `.theme-<name>` class on `<html>` over the shared `.dark` token block. A Palette selects *which hues* the app wears; it is orthogonal to **Theme** (the light/dark mode), except that `dracula` has no light reading and therefore PINS the app dark, ignoring the mode — `system` included (ADR 0027). The syntax-highlight theme is DERIVED from the palette (Shiki `dracula` / `github-dark`), never a third setting.
_Avoid_: Theme (that is the mode), skin, color scheme (ambiguous between the two axes)

**Identity color**:
A color token whose hue encodes *what kind of thing this is* rather than decoration — the file-type descriptors (`--color-file-*`), git status (`--color-git-*`), the **Thinking block**/tool/assistant row hues (`--color-trajectory-*`), usage-chart and context-breakdown scales, and the role chips (file/skill/command/session/plugin nodes). Because the hue IS the signal, an Identity color is never re-hued by a **Palette**: `.theme-dracula` inherits these from `.dark` (deriving them from Dracula's 8 hues would make a `.py` chip indistinguishable from a `.json` one). A Palette-owned structural token, by contrast, is always restated per palette — see ADR 0027's completeness consequence.
_Avoid_: Accent (a Palette choice), brand color, semantic color (the status hues — success/destructive/warning — ARE overridden per palette)

**Skill**:
A directory containing a `SKILL.md` — YAML frontmatter (`name`, `description`) + a markdown body (instructions, optionally referencing bundled files) — discovered from the standard locations (the user home `~/.agents/skills` + `~/.pi/agent/skills`; the Space/project's `.agents/skills` + `.pi/skills` walked up to the repository root). The **Agent harness** advertises each skill's metadata (name, description, location) in the system prompt and loads the full instructions on demand (progressive disclosure). In the Client, a skill is explicitly invoked with a `$name` composer mention; the Client expands the mention into the skill's full content on send, before the message is recorded (ADR 0013).
_Avoid_: Plugin, command, prompt template
