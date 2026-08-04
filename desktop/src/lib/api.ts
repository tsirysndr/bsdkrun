import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Flavor,
  Image,
  Machine,
  Network,
  NewFlavor,
  ProbeResult,
  RunSpec,
  Settings,
  SystemStats,
  VersionEntry,
  Volume,
} from "./types";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (binaryPath: string, cachePath: string) =>
    invoke<Settings>("set_settings", { binaryPath, cachePath }),
  defaultCache: () => invoke<string>("default_cache"),

  probe: () => invoke<ProbeResult>("probe"),
  setTrayStatus: (ok: boolean, detail: string) =>
    invoke<void>("set_tray_status", { ok, detail }),

  listMachines: (all: boolean) => invoke<Machine[]>("list_machines", { all }),
  listImages: () => invoke<Image[]>("list_images"),
  listVolumes: () => invoke<Volume[]>("list_volumes"),
  listVersions: (os: string) => invoke<VersionEntry[]>("list_versions", { os }),
  listFlavors: () => invoke<Flavor[]>("list_flavors"),
  listNetworks: () => invoke<Network[]>("list_networks"),
  createNetwork: (name: string) => invoke<void>("create_network", { name }),
  removeNetwork: (name: string, force: boolean) =>
    invoke<void>("remove_network", { name, force }),
  syncNetwork: (name: string) => invoke<void>("sync_network", { name }),
  systemStats: () => invoke<SystemStats>("system_stats"),

  runFlavor: (name: string, ports: string[], volume: string | null) =>
    invoke<string>("run_flavor", { name, ports, volume }),
  launchFlavor: (
    launchId: string,
    name: string,
    ports: string[],
    volume: string | null,
    repo: string | null,
  ) => invoke<void>("launch_flavor", { launchId, name, ports, volume, repo }),
  buildFlavor: (launchId: string, name: string) =>
    invoke<void>("build_flavor", { launchId, name }),
  createFlavor: (spec: NewFlavor) => invoke<void>("create_flavor", { ...spec }),
  commitMachine: (id: string, name: string, description: string) =>
    invoke<string>("commit_machine", { id, name, description }),
  removeFlavor: (name: string, force: boolean) =>
    invoke<void>("remove_flavor", { name, force }),

  runMachine: (spec: RunSpec) => invoke<string>("run_machine", { spec }),
  launchMachine: (launchId: string, spec: RunSpec) =>
    invoke<void>("launch_machine", { launchId, spec }),
  updateMachine: (id: string, cpus: number, mem: number) =>
    invoke<void>("update_machine", { id, cpus, mem }),
  updateMachineNetwork: (id: string, network: string | null) =>
    invoke<void>("update_machine_network", { id, network }),
  stopMachine: (id: string) => invoke<void>("stop_machine", { id }),
  restartMachine: (id: string) => invoke<string>("restart_machine", { id }),
  removeMachine: (id: string, force: boolean) =>
    invoke<void>("remove_machine", { id, force }),
  removeVolume: (name: string, force: boolean) =>
    invoke<void>("remove_volume", { name, force }),

  machineLogs: (id: string, boot: boolean) =>
    invoke<string>("machine_logs", { id, boot }),
  sshAction: (id: string, args: string[]) =>
    invoke<string>("ssh_action", { id, args }),
  tailscaleAction: (id: string, args: string[]) =>
    invoke<string>("tailscale_action", { id, args }),
  updateAgent: (id: string) => invoke<string>("update_agent", { id }),
  startLogStream: (id: string) => invoke<void>("start_log_stream", { id }),
  stopLogStream: (id: string) => invoke<void>("stop_log_stream", { id }),

  termOpen: (id: string, command: string[], rows: number, cols: number) =>
    invoke<string>("term_open", { id, command, rows, cols }),
  termOpenHost: (rows: number, cols: number) =>
    invoke<string>("term_open_host", { rows, cols }),
  termWrite: (session: string, data: string) =>
    invoke<void>("term_write", { session, data }),
  termResize: (session: string, rows: number, cols: number) =>
    invoke<void>("term_resize", { session, rows, cols }),
  termClose: (session: string) => invoke<void>("term_close", { session }),
};

// ---- event payloads --------------------------------------------------------

export interface TermData {
  session: string;
  bytes: number[];
}
export interface TermExit {
  session: string;
  code: number | null;
}
export interface LogLine {
  id: string;
  line: string;
}
export interface LogEnd {
  id: string;
}
export interface FlavorLog {
  launch_id: string;
  line: string;
}
export interface FlavorDone {
  launch_id: string;
  id: string | null;
  error: string | null;
}

export const onTermData = (cb: (p: TermData) => void): Promise<UnlistenFn> =>
  listen<TermData>("term://data", (e) => cb(e.payload));
export const onTermExit = (cb: (p: TermExit) => void): Promise<UnlistenFn> =>
  listen<TermExit>("term://exit", (e) => cb(e.payload));
export const onLogLine = (cb: (p: LogLine) => void): Promise<UnlistenFn> =>
  listen<LogLine>("log://line", (e) => cb(e.payload));
export const onLogEnd = (cb: (p: LogEnd) => void): Promise<UnlistenFn> =>
  listen<LogEnd>("log://end", (e) => cb(e.payload));
export const onFlavorLog = (cb: (p: FlavorLog) => void): Promise<UnlistenFn> =>
  listen<FlavorLog>("flavor://log", (e) => cb(e.payload));
export const onFlavorDone = (cb: (p: FlavorDone) => void): Promise<UnlistenFn> =>
  listen<FlavorDone>("flavor://done", (e) => cb(e.payload));
export const onMenuAction = (cb: (action: string) => void): Promise<UnlistenFn> =>
  listen<string>("menu://action", (e) => cb(e.payload));
