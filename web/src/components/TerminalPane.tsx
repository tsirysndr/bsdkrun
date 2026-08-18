import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { registerWebLinks } from "../lib/xtermLinks";
import { useUiTheme } from "../lib/theme";
import { api, onTermData, onTermExit } from "../lib/api";
import type { UnlistenFn } from "../lib/api";
import { HOST_MACHINE } from "../state/atoms";


/** An interactive PTY session (`bsdkrun exec -t <id> <cmd>`), streamed over
 *  Tauri events into an xterm.js instance. */
export default function TerminalPane({
  machineId,
  command = [],
}: {
  machineId: string;
  command?: string[];
}) {
  const ui = useUiTheme();

  // The live Terminal, for re-theming: xterm is a canvas, so CSS variables
  // never reach it and the theme has to be pushed in.
  const termInst = useRef<Terminal | null>(null);
  useEffect(() => {
    if (termInst.current) termInst.current.options.theme = ui.term;
  }, [ui]);
  const hostRef = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<"connecting" | "open" | "closed">(
    "connecting",
  );

  useEffect(() => {
    let disposed = false;
    let term: Terminal | null = null;
    let fit: FitAddon | null = null;
    let session = "";
    const unlisten: UnlistenFn[] = [];
    let ro: ResizeObserver | null = null;

    (async () => {
      // Wait for the bundled mono font so glyph metrics are correct.
      try {
        await (document as any).fonts?.load?.('15px "Agave"');
        await (document as any).fonts?.ready;
      } catch {
        /* ignore */
      }
      if (disposed || !hostRef.current) return;

      term = new Terminal({
        fontFamily: '"Agave", ui-monospace, SFMono-Regular, Menlo, monospace',
        fontSize: 15,
        lineHeight: 1.2,
        letterSpacing: 0.2,
        cursorBlink: true,
        cursorStyle: "bar",
        scrollback: 5000,
        theme: ui.term,
        allowProposedApi: true,
      });
      fit = new FitAddon();
      term.loadAddon(fit);
      // URLs a guest prints become clickable. `noopener` because the target is
      // whatever a guest process chose to print.
      registerWebLinks(term, (uri) => {
        window.open(uri, "_blank", "noopener,noreferrer");
      });
      term.open(hostRef.current);
      termInst.current = term;
      fit.fit();

      term.onData((data) => {
        if (session) api.termWrite(session, data).catch(() => {});
      });

      unlisten.push(
        await onTermData((p) => {
          if (p.session === session && term) {
            term.write(Uint8Array.from(p.bytes));
          }
        }),
      );
      unlisten.push(
        await onTermExit((p) => {
          if (p.session === session && term) {
            setStatus("closed");
            term.write(
              `\r\n\x1b[38;5;244m— session ended${p.code != null ? ` (exit ${p.code})` : ""} —\x1b[0m\r\n`,
            );
          }
        }),
      );

      const { rows, cols } = term;
      try {
        session =
          machineId === HOST_MACHINE
            ? await api.termOpenHost(rows, cols)
            : await api.termOpen(machineId, command, rows, cols);
        setStatus("open");
        term.focus();
      } catch (e) {
        setStatus("closed");
        term.write(`\x1b[38;5;203mFailed to open terminal: ${e}\x1b[0m\r\n`);
        return;
      }

      // Keep the PTY sized to the pane.
      ro = new ResizeObserver(() => {
        if (!fit || !term) return;
        try {
          fit.fit();
          if (session) api.termResize(session, term.rows, term.cols).catch(() => {});
        } catch {
          /* ignore */
        }
      });
      ro.observe(hostRef.current);
    })();

    return () => {
      disposed = true;
      ro?.disconnect();
      unlisten.forEach((u) => u());
      if (session) api.termClose(session).catch(() => {});
      term?.dispose();
    };
  }, [machineId, command]);

  return (
    <div className="relative h-full w-full">
      <div ref={hostRef} className="term-host p-2" />
      {status === "connecting" && (
        <div className="pointer-events-none absolute inset-0 grid place-items-center text-xs text-foreground-500">
          {machineId === HOST_MACHINE
            ? "Starting host shell…"
            : "Connecting to guest agent…"}
        </div>
      )}
    </div>
  );
}
