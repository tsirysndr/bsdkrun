import { useEffect } from "react";
import { Button, Kbd, Spinner, Tooltip } from "@heroui/react";
import { useAtom, useSetAtom } from "jotai";
import {
  IconPlus,
  IconRefresh,
  IconSearch,
  IconCircleFilled,
  IconLayoutSidebar,
  IconLayoutSidebarFilled,
  IconLayoutSidebarRight,
  IconLayoutSidebarRightFilled,
  IconLayoutBottombar,
  IconLayoutBottombarFilled,
  IconTerminal2,
} from "@tabler/icons-react";
import { useAtomValue } from "jotai";
import {
  cliModalOpenAtom,
  openHostTerminalAtom,
  paletteOpenAtom,
  runOpenAtom,
  agentPanelOpenAtom,
  sidebarVisibleAtom,
  terminalCollapsedAtom,
  terminalTabsAtom,
} from "../state/atoms";
import { useMachines, useProbe, useRefreshAll } from "../lib/queries";
import { api } from "../lib/api";

export default function TopBar() {
  const { data: probe } = useProbe();
  const { isFetching } = useMachines();
  const setRunOpen = useSetAtom(runOpenAtom);
  const setPalette = useSetAtom(paletteOpenAtom);
  const [sidebar, setSidebar] = useAtom(sidebarVisibleAtom);
  const [agentPanel, setAgentPanel] = useAtom(agentPanelOpenAtom);
  const [termCollapsed, setTermCollapsed] = useAtom(terminalCollapsedAtom);
  const termTabs = useAtomValue(terminalTabsAtom);
  const openHostTerminal = useSetAtom(openHostTerminalAtom);
  const setCliOpen = useSetAtom(cliModalOpenAtom);
  const refreshAll = useRefreshAll();
  const loading = isFetching;

  // Mirror the engine status into the menu-bar tray line.
  useEffect(() => {
    if (probe) {
      api.setTrayStatus(probe.ok, probe.ok ? "" : probe.message || "").catch(
        () => {},
      );
    }
  }, [probe?.ok, probe?.message]);

  // No tabs → open a host terminal; otherwise hide/show the panel.
  const panelHidden = termTabs.length === 0 || termCollapsed;
  const toggleTerminal = () => {
    if (termTabs.length === 0) openHostTerminal();
    else setTermCollapsed((c) => !c);
  };

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
      <div className="flex items-center gap-0.5">
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

        <Tooltip
          content={
            termTabs.length === 0
              ? "Open a host terminal"
              : termCollapsed
                ? "Show terminal panel"
                : "Hide terminal panel"
          }
          placement="bottom"
        >
          <Button
            isIconOnly
            size="sm"
            variant="light"
            className="no-drag text-foreground-400 hover:text-foreground"
            onPress={toggleTerminal}
          >
            {panelHidden ? (
              <IconLayoutBottombar size={18} />
            ) : (
              <IconLayoutBottombarFilled size={18} />
            )}
          </Button>
        </Tooltip>

        <Tooltip
          content={
            agentPanel ? "Hide the AI agent panel (⌘J)" : "AI agent panel (⌘J)"
          }
          placement="bottom"
        >
          <Button
            isIconOnly
            size="sm"
            variant="light"
            className="no-drag text-foreground-400 hover:text-foreground"
            onPress={() => setAgentPanel((v) => !v)}
          >
            {agentPanel ? (
              <IconLayoutSidebarRightFilled size={18} />
            ) : (
              <IconLayoutSidebarRight size={18} />
            )}
          </Button>
        </Tooltip>
      </div>

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

      <Tooltip content="How to install the bsdkrun CLI" placement="bottom">
        <button
          onClick={() => setCliOpen(true)}
          className="no-drag flex h-8 items-center gap-1.5 rounded-lg border border-white/10 bg-content2/60 px-2.5 text-xs font-medium text-foreground-400 transition hover:border-white/20 hover:text-foreground-200"
        >
          <IconTerminal2 size={14} />
          CLI
        </button>
      </Tooltip>

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
