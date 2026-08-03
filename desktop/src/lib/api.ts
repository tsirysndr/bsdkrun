import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Image,
  Machine,
  ProbeResult,
  RunSpec,
  Settings,
  VersionEntry,
  Volume,
} from "./types";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (binaryPath: string) =>
    invoke<Settings>("set_settings", { binaryPath }),

  probe: () => invoke<ProbeResult>("probe"),

  listMachines: (all: boolean) => invoke<Machine[]>("list_machines", { all }),
  listImages: () => invoke<Image[]>("list_images"),
  listVolumes: () => invoke<Volume[]>("list_volumes"),
  listVersions: (os: string) => invoke<VersionEntry[]>("list_versions", { os }),

  runMachine: (spec: RunSpec) => invoke<string>("run_machine", { spec }),
  stopMachine: (id: string) => invoke<void>("stop_machine", { id }),
  removeMachine: (id: string, force: boolean) =>
    invoke<void>("remove_machine", { id, force }),
  removeVolume: (name: string, force: boolean) =>
    invoke<void>("remove_volume", { name, force }),

  machineLogs: (id: string, boot: boolean) =>
    invoke<string>("machine_logs", { id, boot }),
  startLogStream: (id: string) => invoke<void>("start_log_stream", { id }),
  stopLogStream: (id: string) => invoke<void>("stop_log_stream", { id }),

  termOpen: (id: string, command: string[], rows: number, cols: number) =>
    invoke<string>("term_open", { id, command, rows, cols }),
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

export const onTermData = (cb: (p: TermData) => void): Promise<UnlistenFn> =>
  listen<TermData>("term://data", (e) => cb(e.payload));
export const onTermExit = (cb: (p: TermExit) => void): Promise<UnlistenFn> =>
  listen<TermExit>("term://exit", (e) => cb(e.payload));
export const onLogLine = (cb: (p: LogLine) => void): Promise<UnlistenFn> =>
  listen<LogLine>("log://line", (e) => cb(e.payload));
export const onLogEnd = (cb: (p: LogEnd) => void): Promise<UnlistenFn> =>
  listen<LogEnd>("log://end", (e) => cb(e.payload));
export const onMenuAction = (cb: (action: string) => void): Promise<UnlistenFn> =>
  listen<string>("menu://action", (e) => cb(e.payload));
