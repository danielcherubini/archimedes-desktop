// Desktop-owned tool gate. Inert unless PI_ARCHIMEDES_GATE=1 (the desktop
// is the only spawner that sets it). Gated tools prompt the user via the
// extension-UI subprotocol; a rejection BLOCKS the tool (the LLM sees the
// reason as an error result).
export default (pi: any) => {
  if (process.env.PI_ARCHIMEDES_GATE !== "1") return;
  const GATED = new Set(["bash", "edit", "write", "sudo_exec"]);
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
