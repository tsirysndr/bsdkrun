// Mirrors the Rust structs returned by the Tauri commands (which in turn mirror
// bsdkrun's `--json` output).

export interface PortForward {
  bind: string;
  host: number;
  guest: number;
}

export interface Machine {
  id: string;
  name: string | null;
  image: string;
  kind: string; // "linux" | "freebsd" | "netbsd" | "firmware" | "kernel" | "unikraft" | "nanos"
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
  network: string | null;
  net_ip: string | null;
  ports: PortForward[];
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

export interface Network {
  name: string;
  subnet: string;
  gateway: string;
  members: number;
  running: number;
  up: boolean;
  created_at?: string | null;
}

// Payload for defining a custom flavor (`create_flavor` → `bsdkrun flavor add`).
export interface NewFlavor {
  name: string;
  base: string;
  category: string;
  description: string;
  ports: string[];
  env: string[];
  nix: string[];
  provision: string[];
}

export type FlavorSource = "catalog" | "user" | "snapshot";
export type FlavorMethod = "docker" | "nix" | "system" | "snapshot";

export interface Flavor {
  name: string;
  source: FlavorSource;
  kind: string; // "linux" | "freebsd" | "netbsd"
  base: string;
  category: string; // "language" | "runtime" | "service" | "web" | "ai" | "os" | "snapshot" | "custom"
  method: FlavorMethod;
  description: string;
  ports: string[];
  nix: string[];
  created_at?: string | null;
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
  /**
   * Where bsdkrun lives: a path to the binary on this machine, or a `bsdkrund`
   * gRPC URL (`grpc://host:50051`) to drive a remote host.
   */
  binary_path: string;
  cache_path: string;
  /** Access token, used only when `binary_path` is a daemon URL. */
  token: string;
}

// The Run dialog payload. Field names are snake_case to match the Rust
// `RunSpec` struct exactly (nested structs are not camelCase-converted).
export interface RunSpec {
  kind: "linux" | "freebsd" | "netbsd" | "unikraft" | "nanos" | "osv";
  image?: string | null;
  /** Unikraft only: a kraft project directory or a built unikernel image. */
  path?: string | null;
  /** Unikraft/Nanos/OSv: kernel cmdline. For OSv, the application to run. */
  cmdline?: string | null;
  /** Nanos only: kernel override (Linux hosts). */
  kernel?: string | null;
  /** Nanos/OSv: boot the image in place instead of a CoW clone. */
  persist?: boolean;
  /** OSv only: root disk (raw). Required on x86_64, where the loader ELF
   * carries no filesystem. */
  disk?: string | null;
  /** OSv only: interrupt controller, "v2" (default) or "v3". aarch64 only. */
  gic?: "v2" | "v3" | null;
  version?: string | null;
  cpus?: number | null;
  mem?: number | null;
  volume?: string | null;
  no_net: boolean;
  initramfs: boolean;
  entrypoint?: string | null;
  mounts: string[];
  ports: string[];
  attach_disks: string[];
  disk_size?: string | null;
  repo?: string | null;
  network?: string | null;
  name?: string | null;
  command: string[];
}

export type ViewKey = "machines" | "images" | "volumes" | "flavors" | "networks";
