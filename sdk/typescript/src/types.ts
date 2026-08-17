/** A host→guest TCP port forward, `{ bind: "127.0.0.1", host: 2222, guest: 22 }`. */
export interface PortForward {
  /**
   * Host interface the forward is bound to, e.g. `127.0.0.1` (default) or
   * `0.0.0.0`. Always present on {@link SandboxInfo.ports}; optional when
   * passed as input to {@link NetworkOptions.ports}.
   */
  bind?: string;
  host: number;
  guest: number;
}

/** Networking configuration shared by every guest kind. */
export interface NetworkOptions {
  /** Disable networking entirely (`--no-net`). Off by default. */
  disabled?: boolean;
  /**
   * Forward host TCP ports into the guest. Accepts `{host, guest}` objects or
   * `"HOST:GUEST"` strings.
   */
  ports?: Array<PortForward | string>;
  /** Override the guest NIC MAC address. */
  mac?: string;
  /**
   * Join a global network so the machine shares a subnet with, and can reach
   * (by IP and by name), other members. Create the network first with
   * {@link networks.create}. Its DNS name is the machine's `name`.
   */
  network?: string;
}

/** vCPU / RAM sizing, shared by every guest kind. */
export interface ResourceOptions {
  /** Number of vCPUs (default 1). */
  cpus?: number;
  /** Guest RAM in MiB (default 512). */
  mem?: number;
}

/** Options common to every `create` call. */
export interface BaseCreateOptions extends ResourceOptions {
  net?: NetworkOptions;
  /**
   * Machine name — used as its DNS name on a `net.network`, and shown in
   * {@link Sandbox.list}. Defaults to a generated Docker-style name.
   */
  name?: string;
  /** bsdkrun `--log-level` for the boot (0=off .. 5=trace). Default 1. */
  logLevel?: number;
}

/** Persistence options shared by disk-backed (BSD/firmware/kernel) guests. */
export interface DiskPersistenceOptions {
  /** Boot the disk in place (writes persist; one machine at a time). */
  persist?: boolean;
  /** Persist to a named CoW volume that survives reboots. */
  volume?: string;
  /** Extra disks to attach as virtio-blk. `"PATH"` or `"PATH:ro"`. */
  attachDisk?: string[];
}

/** Run an OCI image as a Linux microVM (`bsdkrun linux`). */
export interface LinuxCreateOptions extends BaseCreateOptions {
  os: "linux";
  /** OCI image reference, e.g. `alpine`, `ghcr.io/owner/name:tag`. */
  image: string;
  /** Path to a kernel (ELF vmlinux or raw arm64 Image). */
  kernel?: string;
  /** vmlinux-builder release to download + boot. */
  kernelVersion?: string;
  /** Boot from an initramfs (whole rootfs in RAM) instead of virtio-fs. */
  initramfs?: boolean;
  /** Persist the rootfs to a named volume that survives reboots. */
  volume?: string;
  /** Bind-mount host dirs into the guest: `"HOST:GUEST"` or `"HOST:GUEST:ro"`. */
  mounts?: string[];
  /** Extra disks to attach as virtio-blk. `"PATH"` or `"PATH:ro"`. */
  attachDisk?: string[];
  /** Override the image entrypoint. */
  entrypoint?: string;
  /**
   * Environment variables for the guest's entrypoint (`-e K=V`). These are
   * merged over the image's own config, so setting a key the image already
   * defines replaces it rather than adding a duplicate.
   */
  env?: Record<string, string>;
  /** Guest console device (default `hvc0`). */
  console?: string;
  /** Command (and args) to run instead of the image's default Cmd. */
  command?: string[];
}

/** Run FreeBSD (`bsdkrun freebsd`). */
export interface FreebsdCreateOptions
  extends BaseCreateOptions,
    DiskPersistenceOptions {
  os: "freebsd";
  /** Release like `14.3` (default: bsdkrun's bundled agent image). */
  version?: string;
  /** UEFI firmware to boot with (default: auto-located KRUN_EFI). */
  firmware?: string;
  /** Re-download even if cached. */
  force?: boolean;
}

/** Run NetBSD (`bsdkrun netbsd`). */
export interface NetbsdCreateOptions
  extends BaseCreateOptions,
    DiskPersistenceOptions {
  os: "netbsd";
  /** Release like `10.1`, or `current` (default). */
  version?: string;
  /** Re-download even if cached. */
  force?: boolean;
}

/** Boot a raw disk through its UEFI loader (`bsdkrun firmware`). */
export interface FirmwareCreateOptions
  extends BaseCreateOptions,
    DiskPersistenceOptions {
  os: "firmware";
  /** Path to the UEFI firmware image. */
  firmware: string;
  /** Root disk image (raw), attached as virtio-blk. */
  disk: string;
}

/** Boot a kernel directly, no bootloader (`bsdkrun kernel`). */
export interface KernelCreateOptions
  extends BaseCreateOptions,
    DiskPersistenceOptions {
  os: "kernel";
  /** Path to the guest kernel image. */
  kernel: string;
  /** Kernel image format (default `elf`). */
  format?: "elf" | "raw";
  /** Optional initramfs/initrd. */
  initramfs?: string;
  /** Kernel command line. */
  cmdline?: string;
  /** Root disk image (raw), attached as virtio-blk. */
  disk?: string;
}

/**
 * Boot a Unikraft unikernel (`bsdkrun unikraft`).
 *
 * A unikernel is the application linked into the kernel: there is no disk and
 * no userland, so this extends neither {@link DiskPersistenceOptions} nor the
 * agent-backed options — `exec`, `shell` and `snapshot` do not apply to the
 * resulting sandbox. Use `logs` to read its output.
 */
export interface UnikraftCreateOptions extends BaseCreateOptions {
  os: "unikraft";
  /**
   * A `kraft` project directory (the image is found under its
   * `.unikraft/build/`) or a built unikernel image. Defaults to `"."`.
   */
  path?: string;
  /** Kernel command line; Unikraft hands it to the application as `argv`. */
  cmdline?: string;
  /** Optional initrd, for a unikernel built with an initrd-backed rootfs. */
  initramfs?: string;
  /**
   * Persistent volumes: host directories shared in over virtio-fs, each
   * `"HOST:GUEST"` with an absolute guest path. The one disk-shaped option a
   * unikernel does take — a share needs neither a disk nor an agent. Requires
   * a unikernel built for it; see `examples/unikraft-volume`.
   */
  mounts?: string[];
}

/**
 * Boot a Solo5 (MirageOS) unikernel (`bsdkrun solo5`).
 *
 * Runs under the `solo5-hvt` tender rather than libkrun. The unikernel
 * declares its own network and block devices in its `MFT1` manifest note, so
 * only what the host alone can know is asked for here. Like unikraft there is
 * no disk and no agent — `exec`, `shell` and `snapshot` do not apply; read
 * its output with `logs`. Always a single vCPU — `cpus` above 1 is warned
 * about and ignored.
 */
export interface Solo5CreateOptions extends BaseCreateOptions {
  os: "solo5";
  /**
   * A `.hvt` binary, or a project directory whose `dist/` holds one (where
   * `mirage build` leaves it). Defaults to `"."`.
   */
  path?: string;
  /**
   * Backing files for declared block devices, each `"NAME=FILE"`. The
   * `NAME=` may be omitted when the unikernel declares exactly one.
   */
  block?: string[];
  /**
   * Arguments passed to the unikernel itself, e.g. MirageOS's
   * `"--ipv4=10.0.0.2/24"`.
   */
  args?: string[];
}

/**
 * Boot a Nanos (NanoVMs) unikernel image (`bsdkrun nanos`).
 *
 * Like unikraft, there is no agent — `exec`, `shell` and `snapshot` do not
 * apply. Nanos does have a root disk, so `persist` is honored.
 */
export interface NanosCreateOptions extends BaseCreateOptions {
  os: "nanos";
  /** A path, or a bare name in `~/.ops/images` (what `ops build -i` makes). */
  image: string;
  /** Nanos kernel override (Linux hosts). */
  kernel?: string;
  /** Kernel command line. */
  cmdline?: string;
  /** Boot the image in place instead of a per-machine CoW clone. */
  persist?: boolean;
}

/**
 * Boot an OSv unikernel image (`bsdkrun osv`).
 *
 * Like the other unikernels there is no agent, so `exec`, `shell` and
 * `snapshot` do not apply — read its output with `logs`. OSv does have a root
 * filesystem, so `persist` is honored.
 *
 * The application is an ordinary Linux shared object that OSv `dlopen()`s and
 * calls `main()` on, so `cmdline` is the path to it inside the image plus its
 * arguments, e.g. `"/hello.so"`.
 */
export interface OsvCreateOptions extends BaseCreateOptions {
  os: "osv";
  /**
   * The image to boot: an aarch64 `loader.img` (a capstan-composed image is
   * both kernel and filesystem), or on x86_64 the loader ELF, which is kernel
   * only and needs {@link disk}.
   */
  image: string;
  /** The application to run and its arguments, e.g. `"/hello.so"`. */
  cmdline?: string;
  /**
   * Root disk (raw), attached as virtio-blk. Required on x86_64, where the
   * loader ELF carries no filesystem.
   */
  disk?: string;
  /**
   * Interrupt controller to ask libkrun for (aarch64 only). OSv only grew a
   * GICv3 driver after v0.57.0, so its released kernel needs `"v2"`, which is
   * the default; pass `"v3"` for a kernel built from OSv master.
   */
  gic?: "v2" | "v3";
  /** Boot the disk in place instead of a per-machine CoW clone. */
  persist?: boolean;
}

/** The full set of ways to boot a sandbox. Discriminated on `os`. */
export type CreateOptions =
  | LinuxCreateOptions
  | FreebsdCreateOptions
  | NetbsdCreateOptions
  | FirmwareCreateOptions
  | KernelCreateOptions
  | UnikraftCreateOptions
  | Solo5CreateOptions
  | NanosCreateOptions
  | OsvCreateOptions;

/** Kind of guest a sandbox is running. */
export type GuestKind =
  | "linux"
  | "freebsd"
  | "netbsd"
  | "firmware"
  | "kernel"
  | "unikraft"
  | "solo5"
  | "nanos"
  | "osv";

/** A machine as reported by `bsdkrun ps --json`. */
export interface SandboxInfo {
  id: string;
  /** Machine name (its DNS name on a network), or null if unnamed. */
  name: string | null;
  image: string;
  kind: GuestKind | string;
  command: string;
  status: "running" | "exited";
  running: boolean;
  exitCode: number | null;
  pid: number | null;
  detached: boolean;
  cpus: number;
  mem: number;
  volume: string | null;
  stateDir: string;
  /** Global network the machine belongs to, if any. */
  network: string | null;
  /** The machine's assigned IP on that network, if any. */
  netIp: string | null;
  /** Host↔guest TCP port forwards (`--port HOST:GUEST`), empty if none. */
  ports: PortForward[];
  /** Unix epoch seconds. */
  createdAt: number;
  /** Unix epoch seconds, or null while running. */
  finishedAt: number | null;
  /** The snapshot this machine was branched from, if any. */
  origin?: string | null;
}

/**
 * A machine snapshot: one machine's disk state, captured under a name.
 *
 * A copy-on-write clone rather than a memory image — the files the guest
 * wrote, not what it was executing. Boot a new machine from it with
 * `Client.branch`, or put it back with `Client.restore`.
 */
export interface SnapshotInfo {
  id: string;
  name: string;
  machineId: string;
  /** The machine's name when it was taken; empty if it had none. */
  machineName: string;
  /** `"linux"` | `"freebsd"` | `"netbsd"` | `"unikraft"`. */
  kind: string;
  image: string;
  path: string;
  /** The snapshot the source machine was itself branched from, if any. */
  parent: string | null;
  description: string;
  cpus: number;
  mem: number;
  ports: PortForward[];
  /** Human-readable, when measured. Taking a CoW clone costs nothing. */
  size: string | null;
  /** Unix epoch seconds. */
  createdAt: number;
}

/** An image as reported by `bsdkrun images --json`. */
export interface ImageInfo {
  id: string;
  reference: string;
  digest: string;
  size: number;
  rootfs: string;
  createdAt: number;
}

/** A volume as reported by `bsdkrun volume ls --json`. */
export interface VolumeInfo {
  name: string;
  guest: string | null;
  base: string | null;
  path: string;
  size: string;
  createdAt: number | null;
  tracked: boolean;
}

/** A global network as reported by `bsdkrun network ls --json`. */
export interface NetworkInfo {
  name: string;
  /** The shared subnet, e.g. `192.168.127.0/24`. */
  subnet: string;
  /** Gateway address, e.g. `192.168.127.1`. */
  gateway: string;
  /** Total members recorded. */
  members: number;
  /** Members currently running. */
  running: number;
  /** Whether the network's shared switch (gvproxy) is up. */
  up: boolean;
  /** Unix epoch seconds, or null. */
  createdAt: number | null;
}

// ---- remote Client (GraphQL) types -----------------------------------------
//
// Returned by `Client` (client.ts), the GraphQL counterpart to the local,
// CLI-backed `Sandbox` above. `Client` reuses `SandboxInfo` / `PortForward`
// as-is (see `fromGraphQLMachine` in sandbox.ts) since the GraphQL schema is
// already camelCase; these three are new, wire-shaped types with no local
// equivalent.
//
// `CommandResult` here is a plain `{exitCode, stdout, stderr}` record — the
// GraphQL `CommandResult` type (daemon/src/graphql.rs) has no `command`
// field, unlike the CLI-flavored `CommandResult` *class* exported from
// shell.ts (which wraps a `bsdkrun` invocation with `.text()`/`.json()`/
// `.throwIfFailed()`). The two intentionally share a name — same concept,
// different transport — so `index.ts` re-exports this one aliased to avoid
// a duplicate top-level export.

/** The outcome of a `Client` mutation that runs a daemon command to completion. */
export interface CommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

/** An open shell session, as returned by the `openShell` mutation. */
export interface ShellSessionInfo {
  id: string;
  machineId: string;
  finished: boolean;
  /** Output was dropped to stay under the session buffer cap. */
  truncated: boolean;
}

/** One frame of a shell session's (or `machineLogs` subscription's) output. */
export interface ShellOutput {
  /** Decoded bytes, or null when this frame only carries `exitCode`. */
  data: Uint8Array | null;
  /** Set exactly once, when the underlying process exits. */
  exitCode: number | null;
}
