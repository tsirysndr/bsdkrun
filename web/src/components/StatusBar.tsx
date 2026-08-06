import { IconCpu, IconDatabase, IconServer2, IconTerminal2 } from "@tabler/icons-react";
import { Tooltip } from "@heroui/react";
import { useSetAtom } from "jotai";
import { useMachines, useSystemStats } from "../lib/queries";
import { openHostTerminalAtom } from "../state/atoms";
import { humanSize } from "../lib/format";

function Meter({ pct }: { pct: number }) {
  const p = Math.max(0, Math.min(100, pct));
  const color =
    p > 85 ? "bg-red-400" : p > 60 ? "bg-amber-400" : "bg-emerald-400";
  return (
    <span className="inline-block h-1.5 w-14 overflow-hidden rounded-full bg-white/10">
      <span className={`block h-full ${color}`} style={{ width: `${p}%` }} />
    </span>
  );
}

/** Docker-Desktop-style bottom status bar: host CPU / RAM (live) and the real
 *  on-disk footprint of all microVMs. */
export default function StatusBar() {
  const { data: stats } = useSystemStats();
  const { data: machines = [] } = useMachines();
  const openHostTerminal = useSetAtom(openHostTerminalAtom);
  const running = machines.filter((m) => m.running).length;
  const memPct =
    stats && stats.mem_total ? (stats.mem_used / stats.mem_total) * 100 : 0;

  // Electric load tint for the CPU/RAM segments — like a neovim statusline.
  const loadTint = (p: number) =>
    p > 85
      ? "bg-red-500/20 text-red-300 shadow-[inset_0_0_0_1px] shadow-red-500/30"
      : p > 60
        ? "bg-amber-500/20 text-amber-300 shadow-[inset_0_0_0_1px] shadow-amber-500/30"
        : "bg-emerald-500/15 text-emerald-300 shadow-[inset_0_0_0_1px] shadow-emerald-500/25";

  return (
    <footer className="flex h-7 shrink-0 items-center gap-2 border-t border-white/10 bg-content1/70 pl-0 pr-3 text-[11px] text-foreground-500">
      {/* neovim-style "mode" block: filled, electric */}
      <span className="flex h-full items-center gap-1.5 bg-gradient-to-r from-[#d6249f] via-[#e0429b] to-[#fd5db0] px-3 font-semibold text-white shadow-[0_0_12px] shadow-pink-500/40">
        <IconServer2 size={13} />
        <span className="tabular-nums">{running}</span> running
      </span>
      <span className="flex items-center gap-1.5 rounded bg-white/5 px-2 py-0.5 tabular-nums">
        {machines.length} machines
      </span>

      <span className="ml-auto flex items-center gap-2">
        <span
          className={`flex items-center gap-1.5 rounded px-2 py-0.5 ${loadTint(stats?.cpu ?? 0)}`}
        >
          <IconCpu size={13} />
          CPU
          <span className="tabular-nums">
            {stats ? `${stats.cpu.toFixed(0)}%` : "–"}
          </span>
          <Meter pct={stats?.cpu ?? 0} />
        </span>

        <span
          className={`flex items-center gap-1.5 rounded px-2 py-0.5 ${loadTint(memPct)}`}
        >
          RAM
          <span className="tabular-nums">
            {stats
              ? `${humanSize(stats.mem_used)} / ${humanSize(stats.mem_total)}`
              : "–"}
          </span>
          <Meter pct={memPct} />
        </span>

        <span className="flex items-center gap-1.5 rounded bg-sky-500/15 px-2 py-0.5 text-sky-300 shadow-[inset_0_0_0_1px] shadow-sky-500/25">
          <IconDatabase size={13} />
          disk
          <span className="tabular-nums">
            {stats ? humanSize(stats.vm_disk) : "–"}
          </span>
        </span>

        <Tooltip content="Open a host terminal" placement="top">
          <button
            onClick={() => openHostTerminal()}
            className="flex items-center rounded bg-violet-500/15 p-1 text-violet-300 shadow-[inset_0_0_0_1px] shadow-violet-500/25 transition hover:bg-violet-500/25 hover:text-violet-200"
            aria-label="Open a host terminal"
          >
            <IconTerminal2 size={14} />
          </button>
        </Tooltip>
      </span>
    </footer>
  );
}
