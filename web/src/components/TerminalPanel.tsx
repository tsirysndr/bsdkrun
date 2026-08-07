import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { Button, Tooltip } from "@heroui/react";
import {
  IconTerminal2,
  IconArrowsMaximize,
  IconArrowsMinimize,
  IconX,
} from "@tabler/icons-react";
import {
  activeTerminalAtom,
  closeTerminalAtom,
  HOST_MACHINE,
  terminalCollapsedAtom,
  terminalFullscreenAtom,
  terminalHeightAtom,
  terminalTabsAtom,
} from "../state/atoms";
import { useMachines } from "../lib/queries";
import { kindColor, shortId } from "../lib/format";
import TerminalPane from "./TerminalPane";

const EMPTY: string[] = [];
const MIN_H = 140;

/**
 * A bottom-docked terminal with closable tabs (IDE-style). Each open adds a new
 * tab; sessions stay alive across tab switches (all panes stay mounted, stacked
 * with only the active one visible). Can expand to fullscreen.
 */
export default function TerminalPanel() {
  const tabs = useAtomValue(terminalTabsAtom);
  const [active, setActive] = useAtom(activeTerminalAtom);
  const [fullscreen, setFullscreen] = useAtom(terminalFullscreenAtom);
  const [height, setHeight] = useAtom(terminalHeightAtom);
  const collapsed = useAtomValue(terminalCollapsedAtom);
  const closeTab = useSetAtom(closeTerminalAtom);
  const { data: machines = [] } = useMachines();

  if (tabs.length === 0) return null;
  const activeId =
    active && tabs.some((t) => t.id === active)
      ? active
      : tabs[tabs.length - 1].id;

  // Drag the top edge to resize the docked panel.
  const onResizeStart = (e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = height;
    const maxH = window.innerHeight - 160;
    const onMove = (ev: MouseEvent) => {
      const next = startH + (startY - ev.clientY);
      setHeight(Math.max(MIN_H, Math.min(maxH, next)));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
    };
    document.body.style.userSelect = "none";
    document.body.style.cursor = "ns-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div
      style={fullscreen ? undefined : { height }}
      className={`${collapsed ? "hidden " : ""}${
        fullscreen
          ? "absolute inset-0 z-30 flex flex-col bg-[#0a0d13]"
          : "relative flex shrink-0 flex-col border-t border-white/10 bg-[#0a0d13]"
      }`}
    >
      {!fullscreen && (
        <div
          onMouseDown={onResizeStart}
          className="group absolute inset-x-0 -top-1 z-10 h-2 cursor-ns-resize"
        >
          <div className="mx-auto mt-[3px] h-0.5 w-10 rounded-full bg-white/15 transition group-hover:bg-primary/60" />
        </div>
      )}

      {/* Tab bar */}
      <div className="flex items-center gap-1 border-b border-white/10 bg-content1/70 px-2 py-1">
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {tabs.map((t) => {
            const isHost = t.machineId === HOST_MACHINE;
            const m = isHost
              ? undefined
              : machines.find((x) => x.id === t.machineId);
            const kc = m ? kindColor(m.kind, m.image) : null;
            // The machine's own name, not its image: several machines commonly
            // run the same image, and a row of identical "alpine" tabs tells you
            // nothing about which shell you are typing into. The generated name
            // is unique per machine and is what `ps` and the lists show.
            const label = isHost ? "Host" : m?.name || shortId(t.machineId);
            // Guest OS spelled out beside it. The dot already encodes it by
            // colour, which is only legible once you know the scheme.
            const os = isHost ? null : kc?.label ?? null;
            const tooltip = isHost
              ? "Host shell"
              : [m?.name, os, m?.image, shortId(t.machineId)]
                  .filter(Boolean)
                  .join(" · ");
            const isActive = t.id === activeId;
            return (
              <div
                key={t.id}
                title={tooltip}
                onClick={() => setActive(t.id)}
                onAuxClick={(e) => {
                  if (e.button === 1) closeTab(t.id); // middle-click closes
                }}
                className={`group flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-xs transition ${
                  isActive
                    ? "bg-white/10 text-foreground"
                    : "text-foreground-500 hover:bg-white/5"
                }`}
              >
                {kc ? (
                  <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${kc.dot}`} />
                ) : (
                  <IconTerminal2 size={13} className="shrink-0" />
                )}
                <span className="max-w-[140px] truncate">{label}</span>
                {os && (
                  <span className="shrink-0 text-[10px] font-medium uppercase tracking-wide text-foreground-600">
                    {os}
                  </span>
                )}
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(t.id);
                  }}
                  className="ml-0.5 rounded p-0.5 text-foreground-500 opacity-0 transition hover:bg-white/10 hover:text-foreground group-hover:opacity-100"
                  aria-label="Close terminal tab"
                >
                  <IconX size={12} />
                </button>
              </div>
            );
          })}
        </div>

        <Tooltip
          content={fullscreen ? "Exit fullscreen" : "Fullscreen"}
          placement="bottom"
        >
          <Button
            isIconOnly
            size="sm"
            variant="light"
            onPress={() => setFullscreen((f) => !f)}
          >
            {fullscreen ? (
              <IconArrowsMinimize size={16} />
            ) : (
              <IconArrowsMaximize size={16} />
            )}
          </Button>
        </Tooltip>
      </div>

      {/* Bodies — all mounted, stacked; only the active one is visible. */}
      <div className="relative min-h-0 flex-1 overflow-hidden">
        {tabs.map((t) => {
          const isHost = t.machineId === HOST_MACHINE;
          const m = isHost
            ? undefined
            : machines.find((x) => x.id === t.machineId);
          const isActive = t.id === activeId;
          return (
            <div
              key={t.id}
              className={`absolute inset-0 ${
                isActive
                  ? "z-10 opacity-100"
                  : "pointer-events-none z-0 opacity-0"
              }`}
            >
              {isHost || m?.running ? (
                <TerminalPane machineId={t.machineId} command={EMPTY} />
              ) : (
                <div className="grid h-full place-items-center px-6 text-center text-xs text-foreground-500">
                  This machine isn't running. Start it to open a terminal.
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
