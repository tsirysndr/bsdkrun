// Mirrors the Rust structs returned by the Tauri commands (which in turn mirror
// bsdkrun's `--json` output).

export interface Machine {
  id: string;
  name: string | null;
  image: string;
  kind: string; // "linux" | "freebsd" | "netbsd" | "firmware" | "kernel" | "unikraft" | "nanos" | "osv" | "solo5"
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
  /** The snapshot this machine was branched from, if any. */
  origin?: string | null;
}

export interface PortForward {
  bind: string;
  host: number;
  guest: number;
}

/**
 * A machine snapshot: one machine's disk state, captured under a name.
 *
 * Copy-on-write, not a memory image — the guest's files, not what it was
 * thinking. `branch` boots a new machine from it; `restore` puts it back.
 */
export interface Snapshot {
  id: string;
  name: string;
  machine_id: string;
  /** The machine's name when it was taken; empty if it had none. */
  machine_name: string;
  kind: string; // "linux" | "freebsd" | "netbsd" | "unikraft"
  image: string;
  path: string;
  /** The snapshot the source machine was itself branched from, if any. */
  parent?: string | null;
  description: string;
  cpus: number;
  mem: number;
  ports: PortForward[];
  /** Human-readable, when measured — a CoW clone costs nothing to take. */
  size?: string | null;
  created_at: string;
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
  /**
   * Human-readable, e.g. "2.3 GiB", or null when the CLI could not measure it.
   * `volume ls --json` reports this as text rather than a byte count.
   */
  size: string | null;
  created_at: string | null;
  tracked: boolean;
}

/** A coding agent bsdkrun can sandbox (`bsdkrun ai agents`). */
export interface AiAgent {
  /** Stable id — `claude`, `codex`, … Also the CLI alias. */
  id: string;
  label: string;
  /** The catalog flavor that installs it. */
  flavor: string;
  description: string;
  /**
   * Its flavor is provisioned, so a sandbox boots in a second. False means the
   * first launch installs a toolchain — minutes, streamed into the progress
   * modal rather than hidden behind a spinner.
   */
  installed: boolean;
  running: number;
}

/** One agent sandbox. It is a machine, so `logs`/`stop` work on `id`. */
export interface AiSession {
  id: string;
  name: string;
  agent: string;
  running: boolean;
  /** The directory shared into it, on the engine's host. */
  workspace?: string | null;
  /** A user-given name for the session. */
  label?: string | null;
  /** The project it groups under; defaults to the shared folder's name. */
  project?: string | null;
  created_at: string;
}

/**
 * The Docker engine VM's status (`bsdkrun docker status`).
 *
 * One `docker:dind` microVM whose API is served on a host unix socket, so the
 * host's own `docker` CLI drives it.
 */
export interface DockerStatus {
  running: boolean;
  machine_id?: string | null;
  machine_running: boolean;
  /** The unix socket the `docker` CLI talks to. */
  socket: string;
  socket_ready: boolean;
  api_port?: number | null;
  version?: string | null;
  containers?: number | null;
  images?: number | null;
  /** Host directories shared into the VM, each `HOST:GUEST`. */
  mounts: string[];
  /** A `bsdkrun` docker context exists / is the active one. */
  context: boolean;
  context_active: boolean;
  /** `/var/run/docker.sock` points at this engine. */
  system_socket: boolean;
  /** The dedicated image-store disk, when the VM has one. */
  disk?: string | null;
  /** Its size in bytes — sparse, so the cap rather than the usage. */
  disk_size?: number | null;
}

/** A container in the Docker engine VM (`bsdkrun docker ps`). */
export interface DockerContainer {
  id: string;
  name: string;
  image: string;
  command: string;
  /** "running" | "exited" | "created" | "paused" | … */
  state: string;
  /** Docker's human status, e.g. "Up 3 minutes". */
  status: string;
  /** Published forwards, each `HOST:GUEST/proto`. */
  ports: string[];
  /** Unix epoch seconds. */
  created: number;
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

// Which daemon this browser talks to. The desktop app configured a local
// binary and cache dir instead; a web build has to be told where its backend is.
export interface Settings {
  /** Full GraphQL endpoint URL. */
  url: string;
  token: string;
}

// The Run dialog payload. Field names are snake_case to match the Rust
// `RunSpec` struct exactly (nested structs are not camelCase-converted).
export interface RunSpec {
  kind: "linux" | "freebsd" | "netbsd" | "unikraft" | "nanos" | "osv" | "solo5";
  image?: string | null;
  /** Unikraft: a kraft project directory or a built unikernel image.
   * Solo5: a `.hvt` binary, or a project directory whose `dist/` holds one. */
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
  /** Solo5 only: backing files for declared block devices, `NAME=FILE`. */
  blocks: string[];
  /** Solo5 only: arguments passed to the unikernel itself. Separate from
   * `command`, which means "run via the guest agent" — Solo5 has no agent. */
  args: string[];
}

export type ViewKey =
  | "machines"
  | "images"
  | "volumes"
  | "containers"
  | "snapshots"
  | "flavors"
  | "networks";
