/** A host→guest TCP port forward, `{ host: 2222, guest: 22 }`. */
export interface PortForward {
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
  /** Override the image entrypoint. */
  entrypoint?: string;
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
}

/** The full set of ways to boot a sandbox. Discriminated on `os`. */
export type CreateOptions =
  | LinuxCreateOptions
  | FreebsdCreateOptions
  | NetbsdCreateOptions
  | FirmwareCreateOptions
  | KernelCreateOptions
  | UnikraftCreateOptions;

/** Kind of guest a sandbox is running. */
export type GuestKind =
  | "linux"
  | "freebsd"
  | "netbsd"
  | "firmware"
  | "kernel"
  | "unikraft";

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
  /** Unix epoch seconds. */
  createdAt: number;
  /** Unix epoch seconds, or null while running. */
  finishedAt: number | null;
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
