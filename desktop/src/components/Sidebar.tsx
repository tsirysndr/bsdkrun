import { useAtom } from "jotai";
import {
  IconServer2,
  IconStack2,
  IconDatabase,
  IconApps,
  IconSettings,
  IconKeyboard,
} from "@tabler/icons-react";
import { Tooltip } from "@heroui/react";
import {
  settingsOpenAtom,
  shortcutsOpenAtom,
  viewAtom,
} from "../state/atoms";
import { useFlavors, useImages, useMachines, useVolumes } from "../lib/queries";
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
];

export default function Sidebar() {
  const [view, setView] = useAtom(viewAtom);
  const { data: machines = [] } = useMachines();
  const { data: images = [] } = useImages();
  const { data: volumes = [] } = useVolumes();
  const { data: flavors = [] } = useFlavors();
  const [, setSettingsOpen] = useAtom(settingsOpenAtom);
  const [, setShortcutsOpen] = useAtom(shortcutsOpenAtom);

  const counts: Record<ViewKey, number> = {
    machines: machines.filter((m) => m.running).length,
    images: images.length,
    volumes: volumes.length,
    // Only badge user-created flavors (snapshots + flavors.toml), not the
    // static catalog — otherwise it'd always show a large constant.
    flavors: flavors.filter((f) => f.source !== "catalog").length,
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
      </div>
    </aside>
  );
}
