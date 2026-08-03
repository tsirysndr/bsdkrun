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

  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-white/10 bg-content1/60 px-4 text-[11px] text-foreground-500">
      <span className="flex items-center gap-1.5">
        <IconServer2 size={13} />
        <span className="tabular-nums">{running}</span> running ·{" "}
        <span className="tabular-nums">{machines.length}</span> machines
      </span>

      <span className="ml-auto flex items-center gap-5">
        <span className="flex items-center gap-1.5">
          <IconCpu size={13} />
          CPU
          <span className="tabular-nums text-foreground-400">
            {stats ? `${stats.cpu.toFixed(0)}%` : "–"}
          </span>
          <Meter pct={stats?.cpu ?? 0} />
        </span>

        <span className="flex items-center gap-1.5">
          RAM
          <span className="tabular-nums text-foreground-400">
            {stats
              ? `${humanSize(stats.mem_used)} / ${humanSize(stats.mem_total)}`
              : "–"}
          </span>
          <Meter pct={memPct} />
        </span>

        <span className="flex items-center gap-1.5">
          <IconDatabase size={13} />
          microVM disk
          <span className="tabular-nums text-foreground-400">
            {stats ? humanSize(stats.vm_disk) : "–"}
          </span>
        </span>

        <Tooltip content="Open a host terminal" placement="top">
          <button
            onClick={() => openHostTerminal()}
            className="flex items-center rounded p-1 text-foreground-400 transition hover:bg-white/10 hover:text-foreground"
            aria-label="Open a host terminal"
          >
            <IconTerminal2 size={15} />
          </button>
        </Tooltip>
      </span>
    </footer>
  );
}
