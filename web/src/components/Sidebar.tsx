import { useAtom } from "jotai";
import {
  IconServer2,
  IconStack2,
  IconDatabase,
  IconApps,
  IconNetwork,
  IconSettings,
  IconKeyboard,
  IconCloud,
  IconLogout,
} from "@tabler/icons-react";
import { Tooltip } from "@heroui/react";
import {
  settingsOpenAtom,
  shortcutsOpenAtom,
  viewAtom,
} from "../state/atoms";
import {
  useFlavors,
  useImages,
  useMachines,
  useNetworks,
  useProbe,
  useVolumes,
} from "../lib/queries";
import { clearConnection, getConnection } from "../lib/connection";
import type { ViewKey } from "../lib/types";

const ITEMS: {
  key: ViewKey;
  label: string;
  icon: typeof IconServer2;
  hint: string;
}[] = [
  { key: "machines", label: "Machines", icon: IconServer2, hint: "⌘1" },
  { key: "images", label: "Images", icon: IconStack2, hint: "⌘2" },
  { key: "volumes", label: "Volumes", icon: IconDatabase, hint: "⌘3" },
  { key: "flavors", label: "Flavors", icon: IconApps, hint: "⌘4" },
  { key: "networks", label: "Networks", icon: IconNetwork, hint: "⌘5" },
];

export default function Sidebar() {
  const [view, setView] = useAtom(viewAtom);
  const { data: machines = [] } = useMachines();
  const { data: images = [] } = useImages();
  const { data: volumes = [] } = useVolumes();
  const { data: flavors = [] } = useFlavors();
  const { data: networks = [] } = useNetworks();
  const [, setSettingsOpen] = useAtom(settingsOpenAtom);
  const [, setShortcutsOpen] = useAtom(shortcutsOpenAtom);

  const counts: Record<ViewKey, number> = {
    machines: machines.filter((m) => m.running).length,
    images: images.length,
    volumes: volumes.length,
    // Only badge user-created flavors (snapshots + flavors.toml), not the
    // static catalog — otherwise it'd always show a large constant.
    flavors: flavors.filter((f) => f.source !== "catalog").length,
    networks: networks.length,
  };

  return (
    <aside className="flex w-52 shrink-0 flex-col border-r border-white/10 bg-content1/40 p-2">
      <nav className="flex flex-col gap-0.5">
        {ITEMS.map((it) => {
          const Icon = it.icon;
          const active = view === it.key;
          return (
            <button
              key={it.key}
              onClick={() => setView(it.key)}
              className={`group flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition ${
                active
                  ? "bg-primary/15 text-foreground shadow-[inset_0_0_0_1px] shadow-primary/25"
                  : "text-foreground-500 hover:bg-white/5 hover:text-foreground-300"
              }`}
            >
              <Icon
                size={18}
                className={active ? "text-primary" : "text-foreground-400"}
              />
              <span className="flex-1 text-left font-medium">{it.label}</span>
              {counts[it.key] > 0 && (
                <span
                  className={`rounded-full px-1.5 py-0.5 text-[11px] font-semibold tabular-nums ${
                    active
                      ? "bg-primary text-white"
                      : "bg-white/10 text-foreground-400"
                  }`}
                >
                  {counts[it.key]}
                </span>
              )}
            </button>
          );
        })}
      </nav>

      <div className="flex-1" />

      <div className="flex flex-col gap-0.5">
        <Tooltip content="Keyboard shortcuts (?)" placement="right">
          <button
            onClick={() => setShortcutsOpen(true)}
            className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-foreground-500 transition hover:bg-white/5 hover:text-foreground-300"
          >
            <IconKeyboard size={18} className="text-foreground-400" />
            <span className="font-medium">Shortcuts</span>
          </button>
        </Tooltip>
        <button
          onClick={() => setSettingsOpen(true)}
          className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-foreground-500 transition hover:bg-white/5 hover:text-foreground-300"
        >
          <IconSettings size={18} className="text-foreground-400" />
          <span className="font-medium">Settings</span>
        </button>
        <ConnectionBadge />
      </div>
    </aside>
  );
}

/**
 * Which daemon this browser is driving, pinned to the bottom of the sidebar.
 *
 * The web app is always remote, and the UI is identical whichever host it is
 * pointed at — so "which machine am I about to destroy a VM on" needs to be on
 * screen, not behind a Settings dialog.
 */
function ConnectionBadge() {
  const { data: probe } = useProbe();
  const [, setSettingsOpen] = useAtom(settingsOpenAtom);
  const conn = getConnection();

  const logout = () => {
    clearConnection();
    // A reload is the honest way to drop every cached query, live subscription
    // and open terminal belonging to the daemon we just left.
    window.location.reload();
  };

  return (
    <div className="mt-1 rounded-lg border border-white/10 bg-content2/40 px-2.5 py-2">
      <div className="flex items-center gap-2">
        <span
          className={`h-1.5 w-1.5 shrink-0 rounded-full ${
            probe?.ok ? "bg-emerald-400" : "bg-amber-400"
          }`}
          aria-hidden
        />
        <IconCloud size={14} className="shrink-0 text-foreground-400" />
        <Tooltip content={conn?.url ?? "not connected"} placement="right">
          <button
            onClick={() => setSettingsOpen(true)}
            className="min-w-0 flex-1 truncate text-left text-[11px] font-medium text-foreground-400 transition hover:text-foreground-200"
          >
            {hostOf(conn?.url ?? "")}
          </button>
        </Tooltip>
        <Tooltip content="Disconnect" placement="right">
          <button
            onClick={logout}
            aria-label="Disconnect from this daemon"
            className="shrink-0 rounded p-0.5 text-foreground-500 transition hover:bg-white/10 hover:text-danger"
          >
            <IconLogout size={14} />
          </button>
        </Tooltip>
      </div>
      {probe && !probe.ok && (
        <div className="mt-1 truncate text-[11px] text-amber-400" title={probe.message}>
          unreachable
        </div>
      )}
    </div>
  );
}

/** Just the host:port, so a long URL still fits the sidebar. */
function hostOf(url: string): string {
  try {
    const u = new URL(url);
    return u.port ? `${u.hostname}:${u.port}` : u.hostname;
  } catch {
    return url || "not connected";
  }
}

