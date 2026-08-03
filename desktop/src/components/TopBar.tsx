import { Button, Kbd, Spinner, Tooltip } from "@heroui/react";
import { useAtom, useSetAtom } from "jotai";
import {
  IconPlus,
  IconRefresh,
  IconSearch,
  IconCircleFilled,
  IconLayoutSidebar,
  IconLayoutSidebarFilled,
} from "@tabler/icons-react";
import { paletteOpenAtom, runOpenAtom, sidebarVisibleAtom } from "../state/atoms";
import { useMachines, useProbe, useRefreshAll } from "../lib/queries";

export default function TopBar() {
  const { data: probe } = useProbe();
  const { isFetching } = useMachines();
  const setRunOpen = useSetAtom(runOpenAtom);
  const setPalette = useSetAtom(paletteOpenAtom);
  const [sidebar, setSidebar] = useAtom(sidebarVisibleAtom);
  const refreshAll = useRefreshAll();
  const loading = isFetching;

  const status = probe?.ok
    ? { color: "text-emerald-400", label: "Engine running" }
    : probe
      ? { color: "text-red-400", label: "Engine unavailable" }
      : { color: "text-foreground-400", label: "Checking…" };

  return (
    <header
      data-tauri-drag-region
      className="flex h-12 shrink-0 items-center gap-3 border-b border-white/10 bg-content1/70 pl-20 pr-3"
    >
      <Tooltip
        content={sidebar ? "Hide sidebar" : "Show sidebar"}
        placement="bottom"
      >
        <Button
          isIconOnly
          size="sm"
          variant="light"
          className="no-drag text-foreground-400 hover:text-foreground"
          onPress={() => setSidebar((s) => !s)}
        >
          {sidebar ? (
            <IconLayoutSidebarFilled size={18} />
          ) : (
            <IconLayoutSidebar size={18} />
          )}
        </Button>
      </Tooltip>

      <div data-tauri-drag-region className="flex items-center gap-2">
        <div className="pointer-events-none grid h-6 w-6 place-items-center rounded-md bg-gradient-to-br from-[#5f6bff] to-[#9b5cff] font-mono text-sm font-bold text-white shadow">
          &gt;
        </div>
        <span className="pointer-events-none text-sm font-semibold tracking-tight">
          bsdkrun
          <span className="text-foreground-400"> Desktop</span>
        </span>
      </div>

      <div data-tauri-drag-region className="flex-1" />

      <button
        onClick={() => setPalette(true)}
        className="no-drag group flex h-8 items-center gap-2 rounded-lg border border-white/10 bg-content2/60 px-2.5 text-xs text-foreground-500 transition hover:border-white/20 hover:text-foreground-300"
      >
        <IconSearch size={14} />
        <span>Search or run a command</span>
        <Kbd className="ml-1 bg-transparent text-foreground-400">/</Kbd>
      </button>

      <Tooltip content={probe?.message || status.label} placement="bottom">
        <div className="no-drag flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs">
          <IconCircleFilled size={9} className={status.color} />
          <span className="text-foreground-500">{status.label}</span>
        </div>
      </Tooltip>

      <Tooltip content="Refresh (R)" placement="bottom">
        <Button
          isIconOnly
          size="sm"
          variant="light"
          className="no-drag"
          onPress={() => refreshAll()}
        >
          {loading ? <Spinner size="sm" /> : <IconRefresh size={17} />}
        </Button>
      </Tooltip>

      <Button
        size="sm"
        color="primary"
        variant="shadow"
        className="no-drag font-medium"
        startContent={<IconPlus size={16} />}
        onPress={() => setRunOpen(true)}
      >
        Run
      </Button>
    </header>
  );
}
