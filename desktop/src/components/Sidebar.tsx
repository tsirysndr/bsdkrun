import { useAtom } from "jotai";
import {
  IconRocket,
  IconServer2,
  IconStack2,
  IconDatabase,
  IconApps,
  IconNetwork,
  IconCamera,
  IconBrandDocker,
  IconMoon,
  IconSettings,
  IconKeyboard,
  IconCloud,
  IconDeviceLaptop,
  IconLogout,
} from "@tabler/icons-react";
import { Tooltip } from "@heroui/react";
import {
  settingsOpenAtom,
  shortcutsOpenAtom,
  themeAtom,
  viewAtom,
} from "../state/atoms";
import {
  useFlavors,
  useImages,
  useMachines,
  useNetworks,
  useProbe,
  useSaveSettings,
  useSettings,
  useDockerContainers,
  useDockerStatus,
  useSnapshots,
  useVolumes,
} from "../lib/queries";
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
  { key: "containers", label: "Containers", icon: IconBrandDocker, hint: "⌘4" },
  { key: "snapshots", label: "Snapshots", icon: IconCamera, hint: "⌘5" },
  { key: "flavors", label: "Flavors", icon: IconApps, hint: "⌘6" },
  { key: "cicd", label: "CI/CD", icon: IconRocket, hint: "⌘8" },
  { key: "networks", label: "Networks", icon: IconNetwork, hint: "⌘9" },
];

export default function Sidebar() {
  const [view, setView] = useAtom(viewAtom);
  const { data: machines = [] } = useMachines();
  const { data: images = [] } = useImages();
  const { data: volumes = [] } = useVolumes();
  const { data: flavors = [] } = useFlavors();
  const { data: networks = [] } = useNetworks();
  const { data: snapshots = [] } = useSnapshots();
  const { data: dockerStatus } = useDockerStatus();
  // Only ask for containers when the engine is up — otherwise every poll is a
  // CLI call that fails.
  const { data: containers = [] } = useDockerContainers(
    true,
    !!dockerStatus?.running,
  );
  const [theme, setTheme] = useAtom(themeAtom);
  const [, setSettingsOpen] = useAtom(settingsOpenAtom);
  const [, setShortcutsOpen] = useAtom(shortcutsOpenAtom);

  const counts: Record<ViewKey, number> = {
    machines: machines.filter((m) => m.running).length,
    images: images.length,
    volumes: volumes.length,
    // Running containers, like the machines badge counts running machines.
    containers: containers.filter((c) => c.state === "running").length,
    snapshots: snapshots.length,
    // No badge: run history is client-side; a count would just be noise.
    cicd: 0,
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
                  : "text-foreground-500 hover:bg-default-100/70 hover:text-foreground-300"
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
                      : "bg-default-200/70 text-foreground-400"
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
        <Tooltip
          content={
            theme === "night-rider"
              ? "Theme: Night Rider — click for Classic Dark"
              : "Theme: Classic Dark — click for Night Rider"
          }
          placement="right"
        >
          <button
            onClick={() =>
              setTheme(theme === "night-rider" ? "dark" : "night-rider")
            }
            className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-foreground-500 transition hover:bg-default-100/70 hover:text-foreground-300"
          >
            <IconMoon size={18} className="text-foreground-400" />
            <span className="font-medium">Appearance</span>
          </button>
        </Tooltip>
        <Tooltip content="Keyboard shortcuts (?)" placement="right">
          <button
            onClick={() => setShortcutsOpen(true)}
            className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-foreground-500 transition hover:bg-default-100/70 hover:text-foreground-300"
          >
            <IconKeyboard size={18} className="text-foreground-400" />
            <span className="font-medium">Shortcuts</span>
          </button>
        </Tooltip>
        <button
          onClick={() => setSettingsOpen(true)}
          className="flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-foreground-500 transition hover:bg-default-100/70 hover:text-foreground-300"
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
 * Which engine we are driving, pinned to the bottom of the sidebar.
 *
 * Worth permanent screen space because the app looks identical whether the VMs
 * are on this machine or a server — and "which host am I about to destroy a
 * machine on" is not a question to answer by opening Settings.
 */
function ConnectionBadge() {
  const { data: settings } = useSettings();
  const { data: probe } = useProbe();
  const [, setSettingsOpen] = useAtom(settingsOpenAtom);
  const saveSettings = useSaveSettings();

  const targetValue = settings?.binary_path ?? "";
  const remote = looksLikeUrl(targetValue);
  const host = remote ? hostOf(targetValue) : "this machine";

  // Logging out of a remote means going back to a local bsdkrun, which is this
  // app's default state — not signing out of anything.
  const disconnect = () =>
    saveSettings.mutate({
      binaryPath: "",
      cachePath: settings?.cache_path ?? "",
      token: "",
    });

  return (
    <div className="mt-1 rounded-lg border border-white/10 bg-content2/40 px-2.5 py-2">
      <div className="flex items-center gap-2">
        <span
          className={`h-1.5 w-1.5 shrink-0 rounded-full ${
            probe?.ok ? "bg-emerald-400" : "bg-amber-400"
          }`}
          aria-hidden
        />
        {remote ? (
          <IconCloud size={14} className="shrink-0 text-foreground-400" />
        ) : (
          <IconDeviceLaptop size={14} className="shrink-0 text-foreground-400" />
        )}
        <Tooltip content={remote ? targetValue : probe?.binary || "local bsdkrun"} placement="right">
          <span className="min-w-0 flex-1 truncate text-[11px] font-medium text-foreground-400">
            {host}
          </span>
        </Tooltip>
        {remote && (
          <Tooltip content="Disconnect and go back to local" placement="right">
            <button
              onClick={disconnect}
              aria-label="Disconnect from the remote daemon"
              className="shrink-0 rounded p-0.5 text-foreground-500 transition hover:bg-default-200/70 hover:text-danger"
            >
              <IconLogout size={14} />
            </button>
          </Tooltip>
        )}
      </div>
      {!remote && (
        <button
          onClick={() => setSettingsOpen(true)}
          className="mt-1 text-[11px] text-foreground-600 transition hover:text-primary"
        >
          Connect to a remote host…
        </button>
      )}
    </div>
  );
}

/** Mirrors src-tauri/src/target.rs — only an explicit scheme is a URL. */
const looksLikeUrl = (s: string) => /^(grpc|grpcs|http|https):\/\/.+/i.test(s.trim());

/** Just the host:port, so a long URL still fits the sidebar. */
function hostOf(url: string): string {
  try {
    const u = new URL(url.trim().replace(/^grpcs:\/\//i, "https://").replace(/^grpc:\/\//i, "http://"));
    return u.port ? `${u.hostname}:${u.port}` : u.hostname;
  } catch {
    return url;
  }
}

