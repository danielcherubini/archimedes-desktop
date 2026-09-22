---
status: accepted
date: 2026-09-16
superseded-by:
---

# Subagent launch config via a per-dispatch PI_ACP_PI_COMMAND wrapper

A subagent session must start its pi with a per-dispatch configuration (system prompt, model, tools, thinking level, `--no-session`). The ACP adapter (`pi-acp`) has no CLI surface for any of that: it spawns `pi --mode rpc --no-themes` and only honors `PI_ACP_PI_COMMAND` (the pi executable) plus pass-through env. ACP `session/set_config_option` (which pi-acp implements) covers only model/mode *mid-session*. We decided: the desktop writes a **per-dispatch wrapper script** (in the per-spawn 0700 dir) that execs `pi <resolved flags> "$@"`, and spawns `pi-acp` with `PI_ACP_PI_COMMAND=<wrapper>`.

**Considered Options**

1. **Extend pi-acp with config flags** (e.g. `--system-prompt`/`--model` passthrough): couples the desktop to a pi-acp release; the adapter is a separate repo with its own iteration cadence.
2. **ACP `session/set_config_option` only**: no surface for system prompt / tools at session start; pi-acp's config options are model/mode only.
3. **Shared pi settings file** (`~/.pi/agent/settings.json` or `<project>/.pi/settings.json`): conflicts across sessions — the main session and subagent sessions share the file; per-dispatch overrides would race.
4. **Per-dispatch `PI_ACP_PI_COMMAND` wrapper (chosen)**: desktop-local, no cross-repo coupling, per-spawn isolation (the wrapper lives in the per-spawn 0700 dir and dies with the session). Verified in the installed pi-acp: the session spawn uses `getPiCommand(params.piCommand)`, passes `env: process.env` (bridge vars reach pi), and the best-effort `pi --version` startup probe uses the literal `pi` on PATH, so it is unaffected by the wrapper.

**Consequences**

- The desktop owns a small wrapper-script format (exec `pi` with flags, forwarding pi-acp's `--mode rpc --no-themes`); wrapper edge cases (flag ordering, quoting) are a verification item.
- Model changes mid-session can use ACP `session/set_config_option` (pi-acp supports it); the initial model is set in the wrapper.
- If pi-acp later gains native config flags, the wrapper can be retired — a one-site change in the spawn path.
