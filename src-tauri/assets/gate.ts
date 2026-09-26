// Desktop-owned tool gate. Inert unless PI_ARCHIMEDES_GATE=1 (the desktop
// is the only spawner that sets it). Gated tools prompt the user via the
// extension-UI subprotocol; a rejection BLOCKS the tool (the LLM sees the
// reason as an error result).
export default (pi: any) => {
  if (process.env.PI_ARCHIMEDES_GATE !== "1") return;
  // sudo_exec is NOT gated here: in a desktop spawn the tools.ts override replaces the suite's sudo_exec, and the desktop's sudo_exec handler is the single confirm (a gate confirm + a desktop confirm = double). bash/edit/write stay gated (built-in, NOT overridden in Phase 2).
  const GATED = new Set(["bash", "edit", "write"]);
  const TIMEOUT_MS = 300_000; // matches the desktop's 300 s permission waiter
  pi.on("tool_call", async (event: any, ctx: any) => {
    if (!GATED.has(event.toolName)) return; // undefined = proceed
    const detail = JSON.stringify(event.input ?? {}).slice(0, 500);
    const ok = await ctx.ui.confirm(
      `${event.toolName}: allow this tool call?`,
      detail,
      { timeout: TIMEOUT_MS },
    );
    if (!ok) return { block: true, reason: "User rejected the tool call." };
  });
};
