// Mirrors the Rust structs returned by the Tauri commands (which in turn mirror
// bsdkrun's `--json` output).

export interface Machine {
  id: string;
  image: string;
  kind: string; // "linux" | "freebsd" | "netbsd" | "firmware" | "kernel"
  command: string;
  status: string; // "running" | "exited"
  running: boolean;
  exit_code: number | null;
  pid: number | null;
  detached: boolean;
  cpus: number | null;
  mem: number | null;
  volume: string | null;
  state_dir: string | null;
  created_at: string | null;
  finished_at: string | null;
}

export interface Image {
  id: string;
  reference: string;
  digest: string | null;
  size: number;
  rootfs: string | null;
  created_at: string | null;
}

export interface Volume {
  name: string;
  guest: string | null;
  base: string | null;
  path: string | null;
  size: number | null;
  created_at: string | null;
  tracked: boolean;
}

export interface VersionEntry {
  version: string;
  latest: boolean;
}

export interface ProbeResult {
  ok: boolean;
  message: string;
  binary: string | null;
}

export interface SystemStats {
  cpu: number; // host CPU usage %
  mem_used: number; // bytes
  mem_total: number; // bytes
  vm_disk: number; // real bytes used by all microVMs
  vm_count: number;
}

export interface Settings {
  binary_path: string;
}

// The Run dialog payload. Field names are snake_case to match the Rust
// `RunSpec` struct exactly (nested structs are not camelCase-converted).
export interface RunSpec {
  kind: "linux" | "freebsd" | "netbsd";
  image?: string | null;
  version?: string | null;
  cpus?: number | null;
  mem?: number | null;
  volume?: string | null;
  no_net: boolean;
  initramfs: boolean;
  entrypoint?: string | null;
  mounts: string[];
  ports: string[];
  command: string[];
}

export type ViewKey = "machines" | "images" | "volumes";
