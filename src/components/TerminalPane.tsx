import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useSessions } from "../store/sessions";

/**
 * xterm.js pane fed by `terminal-output` events (accumulated in the store
 * per terminal id). One pane per app; shows the most recent terminal of the
 * active session — the ACP terminal-output event carries only a terminalId,
 * so the latest terminal is the one the user just asked about.
 */
export default function TerminalPane() {
  const terminals = useSessions((s) => s.terminals);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontSize: 12,
      theme: {
        background: "#0a0a0a",
        foreground: "#e5e5e5",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    try {
      fit.fit();
    } catch {
      // container not measurable yet; the resize handler will retry
    }
    termRef.current = term;
    fitRef.current = fit;

    const onResize = () => {
      try {
        fit.fit();
      } catch {
        // ignore
      }
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, []);

  const terminalIds = Object.keys(terminals);
  const latestTerminalId = terminalIds.length > 0 ? terminalIds[terminalIds.length - 1] : null;

  useEffect(() => {
    const term = termRef.current;
    if (!term || !latestTerminalId) return;
    const data = terminals[latestTerminalId] ?? "";
    term.clear();
    term.write(data);
  }, [terminals, latestTerminalId]);

  return (
    <aside className="flex w-96 shrink-0 flex-col border-l border-neutral-800">
      <div className="border-b border-neutral-800 p-3 text-sm font-semibold uppercase tracking-wide text-neutral-400">
        Terminal
      </div>
      <div
        ref={containerRef}
        className="min-h-0 flex-1 bg-[#0a0a0a] p-1"
      />
      {latestTerminalId === null && (
        <p className="p-3 text-xs text-neutral-500">
          No terminal output yet. Terminals the agent creates appear here.
        </p>
      )}
    </aside>
  );
}
