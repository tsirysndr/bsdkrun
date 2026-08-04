//! bsdkrun — a Firecracker-style machine launcher for BSD and Linux guests on
//! macOS, built on libkrun (Hypervisor.framework).
//!
//! Boot modes:
//!   * kernel   — direct kernel + cmdline, using libkrun's generated FDT
//!                (target: NetBSD evbarm / bare kernel+FDT boot)
//!   * firmware — a UEFI firmware image that boots a normal BSD disk via its
//!                EFI loader (target: FreeBSD / NetBSD arm64)
//!   * linux    — run an OCI image (Docker Hub / any registry) as a Linux
//!                machine: fetch a kernel, extract the rootfs, boot it

mod agent;
mod console;
mod db;
mod elf;
mod fetch;
mod flavors;
mod host;
mod id;
mod krun;
mod linux;
mod names;
mod net;
mod network;
mod oci;
mod tty;
mod watchdog;

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::builder::styling::{Color, RgbColor, Style, Styles};
use clap::{Args, Parser, Subcommand, ValueEnum};
use tracing::info;
use tracing_subscriber::EnvFilter;

use krun::Ctx;
use net::{Gvproxy, PortForward};

// Accent palette (with matching muted + error tones) applied to clap's --help
// styling: electric teal for section headers/usage, violet for literals.
const TEAL: Color = Color::Rgb(RgbColor(0, 232, 198));
const VIOLET: Color = Color::Rgb(RgbColor(130, 100, 255));
const MUTED: Color = Color::Rgb(RgbColor(200, 210, 220));
const ERROR: Color = Color::Rgb(RgbColor(255, 100, 100));

/// clap help/usage colors: electric teal for the section headers
/// (Usage/Commands/Options), violet for literals (flags & subcommand names),
/// muted gray for placeholders.
fn cli_styles() -> Styles {
    Styles::styled()
        .header(Style::new().bold().fg_color(Some(TEAL)))
        .usage(Style::new().bold().fg_color(Some(TEAL)))
        .literal(Style::new().fg_color(Some(VIOLET)))
        .placeholder(Style::new().fg_color(Some(MUTED)))
        .valid(Style::new().fg_color(Some(VIOLET)))
        .error(Style::new().bold().fg_color(Some(ERROR)))
        .invalid(Style::new().bold().fg_color(Some(ERROR)))
}

#[derive(Parser)]
#[command(name = "bsdkrun", version, about, styles = cli_styles())]
struct Cli {
    /// libkrun log verbosity (0=off .. 5=trace)
    #[arg(long, global = true, default_value_t = 1)]
    log_level: u32,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that libkrun links and a context/hvf can be initialized.
    Probe,

    /// Boot a machine from a direct kernel image + optional root disk.
    Kernel(KernelArgs),

    /// Boot a machine from a UEFI firmware image + root disk.
    Firmware(FirmwareArgs),

    /// Download a BSD arm64 image and prepare it for booting.
    Fetch(FetchArgs),

    /// List the arm64 builds available to fetch.
    Versions(VersionsArgs),

    /// Grow a disk image (the guest expands its root FS on next boot).
    Grow(GrowArgs),

    /// Run an OCI image (Docker Hub / any registry) as a Linux machine.
    Linux(LinuxArgs),

    /// Run FreeBSD — EFI-firmware boot on macOS; PVH direct boot on Linux/amd64.
    Freebsd(BsdArgs),

    /// Run NetBSD — fetch + direct-kernel boot (no firmware needed).
    Netbsd(BsdArgs),

    /// List machines.
    Ps(PsArgs),

    /// List downloaded images.
    Images(ImagesArgs),

    /// Stop a running machine.
    Stop(IdArgs),

    /// Start (restart) an existing stopped machine in place — same id, same
    /// image/resources/volume. Re-boots detached, like `docker start`.
    Start(IdArgs),

    /// Update a machine's recorded vCPU / memory. Takes effect on the next
    /// `start` (libkrun fixes VM resources at boot).
    Update(UpdateArgs),

    /// Remove one or more machines (and their state). Refuses a running
    /// machine unless `-f`, which stops it first.
    Rm(RmArgs),

    /// Manage the in-guest agent (e.g. `agent update <id>` to refresh a stale
    /// baked-in agent so ssh/tailscale setup works).
    Agent(AgentArgs),

    /// Show a machine's console log.
    Logs(LogsArgs),

    /// Attach an interactive shell to a running (detached) machine.
    Shell(IdArgs),

    /// Run a command inside a running machine (via its guest agent).
    Exec(ExecArgs),

    /// Manage tailscale inside a running machine (install/start/status/setup).
    Tailscale(TailscaleArgs),

    /// Set up key-based SSH inside a running machine (setup/add-key/status).
    Ssh(SshArgs),

    /// Configure systemd as PID 1 in a Linux guest (setup/status/disable).
    Systemd(SystemdArgs),

    /// Manage persistent volumes (list / remove).
    Volume(VolumeArgs),

    /// Snapshot a machine's current state into a named flavor (like `docker commit`).
    Commit(CommitArgs),

    /// List flavors: the built-in catalog + your saved snapshots.
    Flavors(FlavorsListArgs),

    /// Run / remove flavors (`flavor run <name>`, `flavor rm <name>`).
    Flavor(FlavorArgs),

    /// Manage global networks (a shared subnet + internal DNS) that machines join
    /// with `--network` to reach each other by IP and by name.
    Network(NetworkArgs),
}

#[derive(Parser)]
struct NetworkArgs {
    #[command(subcommand)]
    cmd: NetworkCmd,
}

#[derive(Subcommand)]
enum NetworkCmd {
    /// Create a network (starts its shared gvproxy switch).
    Create(NetworkCreateArgs),
    /// List networks and their members.
    Ls(NetworkLsArgs),
    /// Remove one or more networks (refuses running members unless `-f`).
    Rm(NetworkRmArgs),
}

#[derive(Parser)]
struct NetworkCreateArgs {
    /// Network name.
    #[arg(value_name = "NAME")]
    name: String,
}

#[derive(Parser)]
struct NetworkLsArgs {
    /// Emit JSON (for scripting / the desktop).
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct NetworkRmArgs {
    /// Remove even with running members (they lose the network on next start).
    #[arg(short, long)]
    force: bool,

    /// Network name(s) to remove.
    #[arg(value_name = "NAME", required = true)]
    names: Vec<String>,
}

#[derive(Parser)]
struct CommitArgs {
    /// machine id to snapshot (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Name for the new flavor.
    #[arg(value_name = "NAME")]
    name: String,

    /// Optional description.
    #[arg(short, long, default_value = "")]
    description: String,
}

#[derive(Parser)]
struct FlavorsListArgs {
    /// Emit the flavor list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct FlavorArgs {
    #[command(subcommand)]
    cmd: FlavorCmd,
}

#[derive(Subcommand)]
enum FlavorCmd {
    /// Boot a new machine from a flavor (a catalog entry or a snapshot).
    Run(FlavorRunArgs),
    /// Define (or update) a custom flavor in your `flavors.toml`.
    Add(FlavorAddArgs),
    /// Remove a saved snapshot or user flavor (catalog flavors can't be removed).
    Rm(FlavorRmArgs),
    /// Pre-build a flavor's provisioned rootfs into the cache (so a later `run` is
    /// instant). Streams the provisioning output; a no-op if already cached.
    Build(FlavorPrebuildArgs),
    /// (internal) Provision a flavor into its build cache. Used by `run`/`build`.
    #[command(name = "__build", hide = true)]
    BuildInternal(FlavorBuildArgs),
}

#[derive(Parser)]
struct FlavorPrebuildArgs {
    /// Flavor name to build (a catalog or user flavor with provisioning).
    #[arg(value_name = "NAME")]
    name: String,

    #[command(flatten)]
    vm: VmConfig,

    /// Rebuild even if a cached build already exists.
    #[arg(long)]
    force: bool,
}

#[derive(Parser)]
struct FlavorAddArgs {
    /// Flavor name (letters, digits, '-', '_', '.').
    #[arg(value_name = "NAME")]
    name: String,

    /// Base image: an OCI ref (e.g. `node:22`), or `freebsd` / `netbsd`.
    #[arg(long)]
    base: String,

    /// Grouping for the UI (language / service / web / ai / …).
    #[arg(long, default_value = "custom")]
    category: String,

    /// A short description.
    #[arg(long, default_value = "")]
    description: String,

    /// Default host↔guest port forward `HOST:GUEST` (repeatable).
    #[arg(long = "port", value_name = "HOST:GUEST")]
    ports: Vec<String>,

    /// Default environment `K=V` (repeatable).
    #[arg(long = "env", value_name = "K=V")]
    env: Vec<String>,

    /// Nix package to install on an OCI base (repeatable).
    #[arg(long = "nix", value_name = "PKG")]
    nix: Vec<String>,

    /// Provisioning command run in the guest after boot (repeatable, in order).
    #[arg(long = "provision", value_name = "CMD")]
    provision: Vec<String>,
}

#[derive(Parser)]
struct FlavorBuildArgs {
    /// Flavor name to build.
    #[arg(value_name = "NAME")]
    name: String,

    /// Cache key (the build volume to provision into).
    #[arg(long)]
    key: String,

    #[command(flatten)]
    vm: VmConfig,
}

#[derive(Parser)]
struct FlavorRunArgs {
    /// Flavor name (see `bsdkrun flavors`).
    #[arg(value_name = "NAME")]
    name: String,

    /// Run detached in the background (like `docker run -d`).
    #[arg(short = 'd', long)]
    detach: bool,

    #[command(flatten)]
    vm: VmConfig,

    /// Extra host↔guest port forward (repeatable), on top of the flavor's defaults.
    #[arg(long = "port", value_name = "HOST:GUEST")]
    ports: Vec<PortForward>,

    /// Persist to a named volume (Linux flavors).
    #[arg(short = 'v', long, value_name = "NAME")]
    volume: Option<String>,

    /// Clone a git repo into the guest after boot and `cd` into it on shell open.
    #[arg(long, value_name = "URL")]
    repo: Option<String>,
}

#[derive(Parser)]
struct FlavorRmArgs {
    /// Remove even if not tracked (best-effort delete of its data).
    #[arg(short, long)]
    force: bool,

    /// Flavor name(s) to remove.
    #[arg(value_name = "NAME", required = true)]
    names: Vec<String>,
}

#[derive(Parser)]
struct VolumeArgs {
    #[command(subcommand)]
    cmd: VolumeCmd,
}

#[derive(Subcommand)]
enum VolumeCmd {
    /// List persistent volumes.
    Ls(VolumeLsArgs),
    /// Remove one or more volumes (and their data).
    Rm(VolumeRmArgs),
}

#[derive(Parser)]
struct VolumeLsArgs {
    /// Emit the volume list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct ImagesArgs {
    /// Emit the image list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct VolumeRmArgs {
    /// Remove even if a running machine is using the volume.
    #[arg(short, long)]
    force: bool,

    /// Volume name(s) to remove.
    #[arg(value_name = "NAME", required = true)]
    names: Vec<String>,
}

#[derive(Parser)]
struct ExecArgs {
    /// Allocate a pseudo-TTY (interactive; like `docker exec -it`).
    #[arg(short = 't', long)]
    tty: bool,

    /// Set an environment variable in the command (repeatable), e.g. `-e K=V`.
    #[arg(short = 'e', long = "env", value_name = "K=V")]
    env: Vec<String>,

    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Command and arguments to run inside the guest.
    #[arg(value_name = "COMMAND", required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Parser)]
struct TailscaleArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Action + arguments for the in-guest agent:
    /// `setup [--authkey K] [--hostname H]`, `status`, `install`,
    /// `start [--kernel-tun]`. Extra `setup` args pass through to `tailscale up`.
    #[arg(
        value_name = "ACTION",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    args: Vec<String>,
}

#[derive(Parser)]
struct SshArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Action + arguments for the in-guest agent:
    /// `setup [--user U] [--key K]...`, `add-key --key K...`, `status`.
    /// `--key` accepts a literal public key or a local `.pub` file path.
    /// With no `--key`, `setup`/`add-key` install your local `~/.ssh/id_*.pub`.
    #[arg(
        value_name = "ACTION",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    args: Vec<String>,
}

#[derive(Parser)]
struct SystemdArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Action for the in-guest agent: `setup` (install + mark for next boot),
    /// `status`, `disable`. Boot on a volume (-v) so the change persists.
    #[arg(
        value_name = "ACTION",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    args: Vec<String>,
}

#[derive(Parser)]
struct PsArgs {
    /// Show all machines (default shows only running ones).
    #[arg(short, long)]
    all: bool,

    /// Emit the machine list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct IdArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,
}

#[derive(Parser)]
struct AgentArgs {
    #[command(subcommand)]
    cmd: AgentCmd,
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Download + install the current agent inside a running guest, over its
    /// existing (possibly outdated) agent. The next exec/ssh/tailscale spawns
    /// the fresh binary.
    Update(IdArgs),
}

#[derive(Parser)]
struct RmArgs {
    /// Remove even if the machine is still running (stops it first).
    #[arg(short, long)]
    force: bool,

    /// machine id(s) to remove (a unique prefix is enough).
    #[arg(value_name = "ID", required = true)]
    ids: Vec<String>,
}

#[derive(Parser)]
struct UpdateArgs {
    /// machine id to update (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// New number of vCPUs.
    #[arg(long)]
    cpus: Option<u8>,

    /// New guest RAM in MiB.
    #[arg(long)]
    mem: Option<u32>,
}

#[derive(Parser)]
struct LogsArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Follow the live console output.
    #[arg(short, long)]
    follow: bool,

    /// Show bsdkrun's own boot log (libkrun diagnostics + boot errors) instead of
    /// the guest console — useful when a machine dies before producing console
    /// output. This is what `--log-level` writes for a detached machine.
    #[arg(long)]
    boot: bool,
}

#[derive(Parser)]
struct LinuxArgs {
    /// OCI image reference, e.g. `alpine`, `alpine:3.20`, `ghcr.io/owner/name:tag`.
    #[arg(value_name = "IMAGE")]
    image: String,

    /// Kernel to boot (a path to an ELF vmlinux or raw arm64 Image). Overrides
    /// `--kernel-version`.
    #[arg(long)]
    kernel: Option<PathBuf>,

    /// vmlinux-builder release to download + boot (ignored if `--kernel` is
    /// given). See https://github.com/tsirysndr/vmlinux-builder/releases.
    #[arg(long, default_value = linux::DEFAULT_KERNEL_VERSION)]
    kernel_version: String,

    /// Run the machine in the background and print its id (like `docker run -d`).
    /// Use `logs`/`shell`/`stop` to interact with it afterwards.
    #[arg(short = 'd', long)]
    detach: bool,

    /// Boot from an initramfs (the whole rootfs is loaded into RAM) instead of
    /// the default virtio-fs (which serves the rootfs from disk — no RAM-size
    /// limit). Use this if the guest kernel lacks CONFIG_VIRTIO_FS.
    #[arg(long)]
    initramfs: bool,

    /// Persist the guest's rootfs to a named volume that survives reboots (like a
    /// Docker volume). First use CoW-clones the image rootfs; reuse the same name
    /// to keep your changes. Requires virtio-fs (not `--initramfs`, which is RAM).
    #[arg(short = 'v', long, value_name = "NAME")]
    volume: Option<String>,

    /// Bind-mount a host directory into the guest over virtio-fs (repeatable),
    /// like `docker run -v`. Format: `HOST:GUEST[:ro]` (append `:ro` for
    /// read-only). Linux guests only.
    #[arg(long = "mount", value_name = "HOST:GUEST[:ro]")]
    mounts: Vec<String>,

    /// Override the image's entrypoint (like `docker run --entrypoint`).
    #[arg(long)]
    entrypoint: Option<String>,

    /// Set an environment variable in the guest (repeatable), e.g. `-e K=V`.
    #[arg(short = 'e', long = "env", value_name = "K=V")]
    env: Vec<String>,

    /// Guest console device the kernel should log to. libkrun's native console
    /// is the virtio-console `hvc0`; use `ttyS0` only with a kernel/setup that
    /// expects libkrun's explicit 8250 serial instead.
    #[arg(long, default_value = "hvc0")]
    console: String,

    #[command(flatten)]
    net: NetConfig,

    #[command(flatten)]
    vm: VmConfig,

    /// Clone a git repo into the guest after boot and `cd` into it when you open
    /// a shell (e.g. `--repo https://github.com/owner/name`).
    #[arg(long, value_name = "URL")]
    repo: Option<String>,

    /// Command (and args) to run instead of the image's default Cmd.
    /// Everything after `--` is passed through.
    #[arg(last = true, value_name = "CMD")]
    command: Vec<String>,
}

impl LinuxArgs {
    /// virtio-fs is the default; `--initramfs` opts out of it.
    fn virtiofs(&self) -> bool {
        !self.initramfs
    }
}

#[derive(Parser)]
struct GrowArgs {
    /// Path to the raw disk image to enlarge.
    #[arg(long)]
    disk: PathBuf,

    /// New size, e.g. 8G, 4096M (only enlarges — never shrinks).
    #[arg(long)]
    size: String,
}

#[derive(Parser)]
struct FetchArgs {
    /// Guest OS to fetch.
    #[arg(long, value_enum, default_value = "freebsd")]
    os: fetch::Os,

    /// Version to download. FreeBSD: a release like 15.1 (default: latest).
    /// NetBSD: a release like 10.1, or `current` (default: current).
    #[arg(long)]
    version: Option<String>,

    /// Directory to link the (cache-backed) image into.
    #[arg(long, default_value = "images")]
    dir: PathBuf,

    /// Re-download even if the image is already cached.
    #[arg(short, long)]
    force: bool,
}

#[derive(Parser)]
struct VersionsArgs {
    /// Guest OS to list builds for.
    #[arg(long, value_enum, default_value = "freebsd")]
    os: fetch::Os,
}

#[derive(Clone, Copy, ValueEnum)]
enum KernelFormat {
    Raw,
    Elf,
}

impl KernelFormat {
    fn to_krun(self) -> u32 {
        match self {
            KernelFormat::Raw => krun::KRUN_KERNEL_FORMAT_RAW,
            KernelFormat::Elf => krun::KRUN_KERNEL_FORMAT_ELF,
        }
    }
}

#[derive(Parser)]
struct KernelArgs {
    /// Path to the guest kernel image.
    #[arg(long)]
    kernel: PathBuf,

    /// Kernel image format.
    #[arg(long, value_enum, default_value = "elf")]
    format: KernelFormat,

    /// Optional initramfs/initrd.
    #[arg(long)]
    initramfs: Option<PathBuf>,

    /// Kernel command line.
    #[arg(long, default_value = "")]
    cmdline: String,

    /// Root disk image (raw), attached as virtio-blk.
    #[arg(long)]
    disk: Option<PathBuf>,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    attach_disk: Vec<DiskSpec>,

    #[command(flatten)]
    run: RunConfig,

    #[command(flatten)]
    net: NetConfig,

    #[command(flatten)]
    vm: VmConfig,
}

#[derive(Parser)]
struct FirmwareArgs {
    /// Path to the UEFI firmware image (e.g. edk2/AAVMF for aarch64).
    #[arg(long)]
    firmware: PathBuf,

    /// Root disk image (raw), attached as virtio-blk.
    #[arg(long)]
    disk: PathBuf,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    attach_disk: Vec<DiskSpec>,

    #[command(flatten)]
    run: RunConfig,

    #[command(flatten)]
    net: NetConfig,

    #[command(flatten)]
    vm: VmConfig,
}

/// Machine lifecycle options shared by `firmware` and `kernel`.
#[derive(Args)]
struct RunConfig {
    /// Run the machine in the background and print its id (like `docker run -d`).
    /// Use `logs`/`shell`/`stop` to interact with it afterwards.
    #[arg(short = 'd', long)]
    detach: bool,

    /// Boot the disk in place (writes persist to it; only one machine at a time)
    /// instead of the default per-machine APFS copy-on-write clone.
    #[arg(long, conflicts_with = "volume")]
    persist: bool,

    /// Persist the guest's disk to a named volume that survives reboots (like a
    /// Docker volume). First use CoW-clones the base; reuse the same name to keep
    /// your changes. Stored under `<state>/volumes/<NAME>`.
    #[arg(short = 'v', long, value_name = "NAME")]
    volume: Option<String>,
}

/// Options for the `freebsd` / `netbsd` shortcut commands.
#[derive(Parser)]
struct BsdArgs {
    /// Version to run. FreeBSD: a release like 15.1 (default: latest).
    /// NetBSD: a release like 10.1, or `current` (default: current).
    #[arg(long)]
    version: Option<String>,

    /// UEFI firmware to boot with (default: krunkit's KRUN_EFI, auto-located).
    #[arg(long)]
    firmware: Option<PathBuf>,

    /// Re-download even if the image is already cached.
    #[arg(short, long)]
    force: bool,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    attach_disk: Vec<DiskSpec>,

    #[command(flatten)]
    run: RunConfig,

    #[command(flatten)]
    net: NetConfig,

    #[command(flatten)]
    vm: VmConfig,

    /// Grow the guest's root disk to this size before boot (only enlarges),
    /// e.g. `8G`, `4096M`. The guest expands its root FS on first boot.
    #[arg(long, value_name = "SIZE")]
    disk_size: Option<String>,

    /// Stream the guest's boot console live while waiting for its agent (instead
    /// of the terse "waiting…" line), so you see the full BSD boot. The command
    /// output / shell follows once the agent is up.
    #[arg(long)]
    verbose: bool,

    /// Clone a git repo into the guest after boot and `cd` into it on shell open
    /// (installs git via pkg/pkgin/pkg_add if needed).
    #[arg(long, value_name = "URL")]
    repo: Option<String>,

    /// Command (and args) to run inside the guest via its agent once it's
    /// booted, like `bsdkrun linux`. Everything after `--` is passed through.
    /// Without `-d` this is one-shot: the guest boots, runs the command
    /// (streaming its output), then powers off, and bsdkrun exits with the
    /// command's status. With `-d` the machine is left running afterward.
    /// Needs networking (the agent) — incompatible with `--no-net`.
    #[arg(last = true, value_name = "CMD")]
    command: Vec<String>,
}

#[derive(Parser)]
struct VmConfig {
    /// Number of vCPUs.
    #[arg(long, default_value_t = 1)]
    cpus: u8,

    /// Guest RAM in MiB.
    #[arg(long, default_value_t = 512)]
    mem: u32,
}

/// User-mode networking options (shared by `kernel` and `firmware`).
///
/// Networking is on by default: the guest gets a virtio-net NIC wired to
/// gvproxy, which NATs it out to the host's network (internet access via DHCP
/// on 192.168.127.0/24). Pass `--no-net` for an isolated guest.
#[derive(Args)]
struct NetConfig {
    /// Disable networking (boot the guest with no NIC).
    #[arg(long = "no-net")]
    no_net: bool,

    /// Forward a host TCP port to the guest: HOST:GUEST (repeatable).
    /// Example: `--port 2222:22` for SSH.
    #[arg(long = "port", value_name = "HOST:GUEST")]
    ports: Vec<PortForward>,

    /// MAC address for the guest NIC (default: a fixed locally-administered one).
    #[arg(long, value_name = "AA:BB:CC:DD:EE:FF")]
    mac: Option<String>,

    /// Join a global network so the machine shares a subnet with, and can reach
    /// (by IP + name), other members (`bsdkrun network create <name>` first).
    #[arg(long, value_name = "NAME")]
    network: Option<String>,

    /// Name for this machine (used as its DNS name on a `--network`, and shown in
    /// `ps`). Defaults to a generated Docker-style name.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
}

/// A disk to attach as virtio-blk, parsed from `PATH[:ro]`.
#[derive(Clone)]
struct DiskSpec {
    path: PathBuf,
    read_only: bool,
}

impl FromStr for DiskSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Only treat a trailing `:ro`/`:rw` as a mode (paths may contain `:`).
        if let Some(base) = s.strip_suffix(":ro") {
            Ok(DiskSpec {
                path: PathBuf::from(base),
                read_only: true,
            })
        } else if let Some(base) = s.strip_suffix(":rw") {
            Ok(DiskSpec {
                path: PathBuf::from(base),
                read_only: false,
            })
        } else {
            Ok(DiskSpec {
                path: PathBuf::from(s),
                read_only: false,
            })
        }
    }
}

impl FromStr for PortForward {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (host, guest) = s
            .split_once(':')
            .ok_or_else(|| format!("expected HOST:GUEST, got {s:?}"))?;
        let host = host
            .parse::<u16>()
            .map_err(|_| format!("invalid host port {host:?} in {s:?}"))?;
        let guest = guest
            .parse::<u16>()
            .map_err(|_| format!("invalid guest port {guest:?} in {s:?}"))?;
        Ok(PortForward { host, guest })
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Our own diagnostics go through `tracing`, written to stderr so they never
    // mingle with the guest console on stdout. `--log-level` sets a sensible
    // default verbosity (matching libkrun's 0..5 scale); `RUST_LOG` overrides.
    let default_filter = match cli.log_level {
        0 => "warn",
        1..=3 => "info",
        4 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    // libkrun's own internal logging (separate from ours) also honours the flag.
    Ctx::set_log_level(cli.log_level).ok();

    match cli.cmd {
        Command::Probe => probe(),
        Command::Kernel(args) => boot_kernel(args),
        Command::Firmware(args) => boot_firmware(args),
        Command::Fetch(args) => {
            fetch::fetch(args.os, args.version, &args.dir, args.force).map(|_| ())
        }
        Command::Versions(args) => fetch::list_versions(args.os),
        Command::Grow(args) => fetch::grow(&args.disk, &args.size),
        Command::Linux(args) => boot_linux(args),
        Command::Freebsd(args) => boot_freebsd(args),
        Command::Netbsd(args) => boot_netbsd(args),
        Command::Ps(args) => cmd_ps(args.all, args.json),
        Command::Images(args) => cmd_images(args.json),
        Command::Stop(args) => cmd_stop(&args.id),
        Command::Start(args) => cmd_start(&args.id),
        Command::Update(args) => cmd_update(&args.id, args.cpus, args.mem),
        Command::Rm(args) => cmd_rm(&args.ids, args.force),
        Command::Agent(args) => match args.cmd {
            AgentCmd::Update(a) => cmd_agent_update(&a.id),
        },
        Command::Logs(args) => cmd_logs(&args.id, args.follow, args.boot),
        Command::Shell(args) => cmd_shell(&args.id),
        Command::Exec(args) => cmd_exec(&args.id, &args.command, &args.env, args.tty),
        Command::Tailscale(args) => cmd_tailscale(&args.id, &args.args),
        Command::Ssh(args) => cmd_ssh(&args.id, &args.args),
        Command::Systemd(args) => run_agent_cli(&args.id, "systemd", &args.args, &[]),
        Command::Volume(args) => match args.cmd {
            VolumeCmd::Ls(a) => cmd_volume_ls(a.json),
            VolumeCmd::Rm(a) => cmd_volume_rm(&a.names, a.force),
        },
        Command::Commit(args) => cmd_commit(&args.id, &args.name, &args.description),
        Command::Flavors(args) => cmd_flavors(args.json),
        Command::Flavor(args) => match args.cmd {
            FlavorCmd::Run(a) => cmd_flavor_run(a),
            FlavorCmd::Add(a) => cmd_flavor_add(a),
            FlavorCmd::Rm(a) => cmd_flavor_rm(&a.names, a.force),
            FlavorCmd::Build(a) => cmd_flavor_prebuild(&a.name, a.vm.cpus, a.vm.mem, a.force),
            FlavorCmd::BuildInternal(a) => cmd_flavor_build(&a.name, &a.key, a.vm.cpus, a.vm.mem),
        },
        Command::Network(args) => match args.cmd {
            NetworkCmd::Create(a) => network::cmd_create(&a.name),
            NetworkCmd::Ls(a) => network::cmd_ls(a.json),
            NetworkCmd::Rm(a) => network::cmd_rm(&a.names, a.force),
        },
    }
}

fn probe() -> Result<()> {
    let ctx = Ctx::new().context("creating libkrun context")?;
    ctx.set_vm_config(1, 256)
        .context("setting a trivial VM config")?;
    info!("libkrun linked and a context was created + configured (dropped without booting)");
    Ok(())
}

/// Attach any `--attach-disk` images after the root disk. Block ids are
/// `data0`, `data1`, … — libkrun only requires them to be unique.
fn attach_extra_disks(ctx: &Ctx, disks: &[DiskSpec]) -> Result<()> {
    for (i, disk) in disks.iter().enumerate() {
        let block_id = format!("data{i}");
        db::record_disk(&disk.path.to_string_lossy());
        ctx.add_disk(&block_id, &disk.path, disk.read_only)
            .with_context(|| format!("attaching disk {}", disk.path.display()))?;
        info!(
            id = %block_id,
            path = %disk.path.display(),
            read_only = disk.read_only,
            "attached disk"
        );
    }
    Ok(())
}

/// Bring up user-mode networking (on by default). Returns the live gvproxy
/// handle, which must outlive the VM (kept in scope until after `start_enter`).
///
/// If gvproxy isn't installed we degrade gracefully — the guest boots without a
/// NIC and we warn — *unless* the user explicitly asked for port forwards, in
/// which case the missing dependency is a hard error.
/// Bring up gvproxy networking. When `agent_dir` is given and networking is
/// available, also allocate a host port forwarded to the guest agent's TCP port
/// and record it under that machine's state dir (so `exec`/`shell` can reach it).
fn setup_networking_with_agent(
    ctx: &Ctx,
    cfg: &NetConfig,
    agent_dir: Option<&std::path::Path>,
) -> Result<Option<Gvproxy>> {
    if cfg.no_net {
        if !cfg.ports.is_empty() {
            anyhow::bail!("--port cannot be combined with --no-net");
        }
        info!("networking disabled (--no-net)");
        return Ok(None);
    }

    // Probe for gvproxy up front so a missing dependency degrades cleanly.
    if let Err(e) = net::locate() {
        if !cfg.ports.is_empty() {
            return Err(e).context("--port requires gvproxy");
        }
        tracing::warn!("networking disabled: {e:#}");
        return Ok(None);
    }

    let mac = match &cfg.mac {
        Some(s) => net::parse_mac(s).context("parsing --mac")?,
        None => net::DEFAULT_MAC,
    };

    // Shared/global network: if `BSDKRUN_NET_CONTROL` points at a running network
    // gvproxy, join THAT switch (via a per-member /connect bridge) instead of
    // spawning an isolated gvproxy — so members share a subnet and reach each
    // other by IP/name. Each member gets its own IP (`BSDKRUN_NET_IP`).
    if let Some(control) = std::env::var("BSDKRUN_NET_CONTROL")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let control = std::path::PathBuf::from(control);
        // A static (Linux) member knows its IP now; a DHCP (BSD) member gets it
        // after boot — its agent/port forwards + DNS are wired then.
        let ip = std::env::var("BSDKRUN_NET_IP")
            .ok()
            .filter(|s| !s.is_empty());
        // A per-member vfkit socket for the bridge (libkrun connects to it).
        let vfkit = match agent_dir {
            Some(dir) => dir.join("net-bridge.sock"),
            None => std::env::temp_dir().join(format!("bsdkrun-netbr-{}.sock", std::process::id())),
        };
        if let Some(ip) = &ip {
            if let Some(dir) = agent_dir {
                let host =
                    net::free_local_port().context("reserving a host port for the exec agent")?;
                net::expose_on_control(&control, host, ip, agent::GUEST_PORT)
                    .context("forwarding the agent port on the shared network")?;
                let _ = std::fs::write(agent::port_file(dir), host.to_string());
                info!(agent_port = host, %ip, "exec agent reachable via the shared network");
            }
            for pf in &cfg.ports {
                net::expose_on_control(&control, pf.host, ip, pf.guest)
                    .with_context(|| format!("forwarding host port {}", pf.host))?;
            }
        }
        // CRITICAL: every member on the shared switch needs a DISTINCT MAC (a
        // shared MAC makes gvproxy's CAM table route both members' traffic to one
        // port, breaking connectivity). `network::join` sets `BSDKRUN_NET_MAC`;
        // fall back to deriving it from the IP's last octet.
        let member_mac = if let Some(s) = std::env::var("BSDKRUN_NET_MAC")
            .ok()
            .filter(|s| !s.is_empty())
        {
            net::parse_mac(&s).unwrap_or(mac)
        } else if let Some(ip) = &ip {
            let last = ip
                .rsplit('.')
                .next()
                .and_then(|o| o.parse::<u8>().ok())
                .unwrap_or(2);
            [0x5a, 0x94, 0xef, 0xe4, 0x0c, last]
        } else {
            mac
        };
        net::start_network_bridge(&vfkit, &control).context("bridging into the shared network")?;
        ctx.add_net_gvproxy(&vfkit, member_mac)
            .context("attaching virtio-net to the shared network")?;
        info!("joined shared network");
        return Ok(None); // the network gvproxy is shared — we don't own it
    }

    // Forward a unique host port to the guest agent (for `exec`/`shell`) and
    // persist it, alongside any user-requested `--port` forwards.
    let mut ports = cfg.ports.clone();
    let agent_port = match agent_dir {
        Some(dir) => {
            let host =
                net::free_local_port().context("reserving a host port for the exec agent")?;
            ports.push(PortForward {
                host,
                guest: agent::GUEST_PORT,
            });
            let _ = std::fs::write(agent::port_file(dir), host.to_string());
            Some(host)
        }
        None => None,
    };

    let gvproxy = Gvproxy::spawn(&ports).context("starting gvproxy networking")?;
    ctx.add_net_gvproxy(&gvproxy.vfkit_socket, mac)
        .context("attaching virtio-net device")?;
    if let Some(p) = agent_port {
        info!(agent_port = p, "exec agent reachable via forwarded port");
    }
    Ok(Some(gvproxy))
}

fn boot_kernel(args: KernelArgs) -> Result<()> {
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    // CoW-clone the root disk per machine (unless --persist / --volume) so many
    // machines can boot the same base image concurrently without touching it.
    let volume = args.run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = match &args.disk {
        Some(d) => Some(prepare_bsd_disk(
            d,
            &vdir,
            args.run.persist,
            volume.as_deref(),
            None,
        )?),
        None => None,
    };
    let image = args
        .disk
        .as_deref()
        .map(basename)
        .unwrap_or_else(|| basename(&args.kernel));

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        if let Some(d) = &root_disk {
            if let Some(orig) = &args.disk {
                db::record_disk(&orig.to_string_lossy());
            }
            ctx.add_disk("root", d, false)
                .with_context(|| format!("attaching root disk {}", d.display()))?;
        }
        attach_extra_disks(&ctx, &args.attach_disk)?;
        // Forward a host port to the guest agent so a user-installed bsdkrun-agent
        // in the guest can serve `exec`/`shell` (idle until the guest runs it).
        let gvproxy = setup_networking_with_agent(&ctx, &args.net, Some(&vdir))?;
        ctx.set_kernel(
            &args.kernel,
            args.format.to_krun(),
            args.initramfs.as_deref(),
            &args.cmdline,
        )
        .context("configuring kernel")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.run.volume.as_deref(), &volume) {
        db::record_volume(name, "kernel", &image, &dir.to_string_lossy());
    }
    run_machine(
        &machine_id,
        &vdir,
        "kernel",
        &image,
        "",
        args.vm.cpus,
        args.vm.mem,
        args.run.detach,
        true, // BSD: use the SMP-shutdown watchdog on the foreground path
        args.run.volume.as_deref(),
        &[],
        false,
        false,
        build,
    )
}

fn boot_firmware(args: FirmwareArgs) -> Result<()> {
    firmware_machine(
        &args.firmware,
        &args.disk,
        &args.attach_disk,
        &args.run,
        &args.net,
        &args.vm,
        &[],
        false,
        false,
        None,
    )
}

/// Boot a machine via UEFI firmware + a root disk (shared by `firmware` and the
/// `freebsd`/`netbsd` shortcuts). The root disk is CoW-cloned per machine.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn firmware_machine(
    firmware: &std::path::Path,
    disk: &std::path::Path,
    attach: &[DiskSpec],
    run: &RunConfig,
    net: &NetConfig,
    vm: &VmConfig,
    exec_after: &[String],
    interactive: bool,
    verbose: bool,
    disk_size: Option<&str>,
) -> Result<()> {
    ensure_net_for_exec(net, exec_after)?;
    // BSD guests DHCP their IP (dhcp = true), so join before the ctx build.
    let joined = prepare_network(net, true)?;
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(disk);
    let volume = run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = prepare_bsd_disk(disk, &vdir, run.persist, volume.as_deref(), disk_size)?;

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(vm.cpus, vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&disk.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        attach_extra_disks(&ctx, attach)?;
        // Forward a host port to the guest agent so a user-installed bsdkrun-agent
        // in the guest can serve `exec`/`shell` (idle until the guest runs it).
        let gvproxy = setup_networking_with_agent(&ctx, net, Some(&vdir))?;
        ctx.set_firmware(firmware).context("configuring firmware")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (run.volume.as_deref(), &volume) {
        db::record_volume(name, "firmware", &image, &dir.to_string_lossy());
    }
    let result = run_machine(
        &machine_id,
        &vdir,
        "firmware",
        &image,
        &exec_after.join(" "),
        vm.cpus,
        vm.mem,
        run.detach,
        true,
        run.volume.as_deref(),
        exec_after,
        interactive,
        verbose,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &net.ports, &joined);
    result
}

/// A trailing command runs in the guest via its agent, which rides the guest
/// NIC — so it's incompatible with `--no-net`. Fail early with a clear message.
fn ensure_net_for_exec(net: &NetConfig, exec_after: &[String]) -> Result<()> {
    if !exec_after.is_empty() && net.no_net {
        anyhow::bail!(
            "running a command in the guest needs networking (the agent talks over the \
             guest NIC), but --no-net was given"
        );
    }
    Ok(())
}

/// The command a `freebsd`/`netbsd` boot should run in the guest.
///
/// The bundled BSD images are headless (no console getty), so a plain foreground
/// `bsdkrun freebsd` would boot to a console that just sits at the last rc line.
/// Instead, default it to an interactive shell over the agent — you drop into a
/// prompt and the VM powers off when you exit, like foreground `bsdkrun linux`.
/// This needs the agent (networking); `-d`, an explicit command, or `--no-net`
/// all opt out and keep their own behavior (background / that command / classic
/// console boot).
///
/// Returns `(command, interactive)`. `interactive` is true only for the shell we
/// synthesize, so it gets a PTY; an *explicit* command runs non-interactively
/// (like `docker run` without `-t`). That also sidesteps a guest-agent PTY drain
/// race that swallows a fast command's output when a tty is allocated.
fn bsd_exec_after(command: &[String], detach: bool, no_net: bool) -> (Vec<String>, bool) {
    if command.is_empty() && !detach && !no_net {
        (vec!["/bin/sh".to_string()], true)
    } else {
        (command.to_vec(), false)
    }
}

/// `freebsd` / `netbsd`: fetch the image if needed, auto-locate the firmware,
/// then boot it. How it boots depends on the host OS:
///
/// - **macOS** boots through FreeBSD's `loader.efi`, which needs libkrun's EDK2
///   firmware (the `libkrun-efi` flavor, macOS-only) — see [`boot_freebsd_efi`].
/// - **Linux/amd64** direct-boots the GENERIC kernel via **PVH** (no firmware),
///   like `netbsd` — see [`boot_freebsd_pvh`]. Needs the PVH libkrun fork.
/// If `--repo` was given and there's no explicit command, make the post-boot
/// command the repo clone (installs git + records the cwd marker).
fn bsd_inject_repo(args: &mut BsdArgs) {
    if args.command.is_empty() {
        if let Some(argv) = args.repo.as_deref().and_then(repo_clone_argv) {
            args.command = argv;
        }
    }
}

fn boot_freebsd(args: BsdArgs) -> Result<()> {
    boot_freebsd_disk(args, None)
}

/// Boot FreeBSD, optionally from a specific root disk (`disk_override`, used to
/// boot a committed snapshot) instead of the fetched/bundled base image.
fn boot_freebsd_disk(mut args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    bsd_inject_repo(&mut args);
    #[cfg(target_os = "macos")]
    {
        boot_freebsd_efi(args, disk_override)
    }
    #[cfg(target_os = "linux")]
    {
        boot_freebsd_pvh(args, disk_override)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (args, disk_override);
        anyhow::bail!("bsdkrun freebsd is only supported on macOS and Linux");
    }
}

/// macOS EFI-firmware boot: FreeBSD's `loader.efi` takes over from the ESP.
#[cfg(target_os = "macos")]
fn boot_freebsd_efi(args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    // No explicit command on a foreground boot → drop into an interactive shell.
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    // A snapshot boots from its saved disk; otherwise, default (no --version) to
    // bsdkrun's bundled arm64 image (agent injected so `exec` works), or fetch the
    // official FreeBSD VM image for an explicit --version / non-arm64 host.
    let disk = match disk_override {
        Some(d) => d,
        None => match (host::Arch::current()?, &args.version) {
            (host::Arch::Aarch64, None) => fetch::fetch_freebsd_arm64_image(args.force)?,
            _ => {
                let cache = fetch::cache_dir()?;
                fetch::fetch(fetch::Os::Freebsd, args.version.clone(), &cache, args.force)?
            }
        },
    };
    let firmware = match args.firmware {
        Some(f) => f,
        None => locate_krun_efi()?,
    };
    firmware_machine(
        &firmware,
        &disk,
        &args.attach_disk,
        &args.run,
        &args.net,
        &args.vm,
        &exec_after,
        interactive,
        args.verbose,
        args.disk_size.as_deref(),
    )
}

/// FreeBSD command line for the Linux/amd64 PVH boot (override with
/// `$BSDKRUN_FREEBSD_CMDLINE`). The bundled image is a bare makefs UFS on a
/// virtio-blk disk (`vtbd0`); `console=comconsole` routes the console to the
/// serial libkrun provides at COM1.
///
/// `hw.uart.console=io:0x3f8` is the key: it points FreeBSD's `uart(4)` low-level
/// console straight at libkrun's 16550 I/O port, so the console comes up at
/// `cninit` — the earliest console init — without needing the `device.hints` the
/// BOOTLOADER normally supplies (a PVH direct boot has no loader). We also pass
/// the equivalent `hint.uart.0.*` so `uart0` fully attaches as a device later.
/// virtio-mmio device hints are appended by libkrun itself (via
/// `KRUN_VIRTIO_MMIO_HINTS=freebsd`).
#[cfg(target_os = "linux")]
fn freebsd_cmdline() -> String {
    if let Ok(s) = std::env::var("BSDKRUN_FREEBSD_CMDLINE") {
        if !s.is_empty() {
            return s;
        }
    }
    // NB: FreeBSD can't derive its TSC frequency under libkrun on its own (no
    // KVM tsc-freq CPUID leaf by default, AMD has no Intel TSC leaf, and PVH's
    // xen_delay calibration path faults on a Xen pvclock KVM never sets up).
    // `machdep.tsc_freq` looks like the answer but is a runtime sysctl, not a
    // boot tunable, so it can't be set from here. The libkrun fork instead
    // synthesizes CPUID leaf 0x40000010 (tsc_freq_cpuid_vm) from KVM_GET_TSC_KHZ,
    // which FreeBSD reads before calibration — see configure_x86_64 in libkrun.
    "vfs.root.mountfrom=ufs:/dev/vtbd0 console=comconsole hw.uart.console=io:0x3f8 \
     hint.uart.0.at=isa hint.uart.0.port=0x3f8 hint.uart.0.flags=0x10 hint.uart.0.irq=4"
        .to_string()
}

/// Linux/amd64 PVH direct boot: enter the bundled FIRECRACKER kernel at its
/// `PHYS32_ENTRY`, no firmware. Needs a PVH-capable libkrun (the
/// tsirysndr/libkrun `feat/pvh-boot` fork) — stock libkrun boots x86_64 kernels
/// with the Linux protocol and would triple-fault immediately.
#[cfg(target_os = "linux")]
fn boot_freebsd_pvh(args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    let arch = host::Arch::current()?;
    if !matches!(arch, host::Arch::X86_64) {
        anyhow::bail!(
            "FreeBSD on Linux is only supported on amd64 (PVH direct boot) for now; \
             this host is {}.",
            arch.slug()
        );
    }

    // Tell libkrun to enter via the kernel's PHYS32_ENTRY note and to advertise
    // its virtio-mmio devices in FreeBSD's numbered-key cmdline form
    // (`virtio_mmio.device=`, `virtio_mmio.device_1=`, ...). FreeBSD can't read
    // the Linux form: Linux repeats the key, but FreeBSD's kernel environment
    // hides duplicate keys past the first.
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    let joined = prepare_network(&args.net, true)?; // FreeBSD DHCPs its IP
    std::env::set_var("KRUN_PVH", "1");
    std::env::set_var("KRUN_VIRTIO_MMIO_HINTS", "freebsd");

    let disk = match disk_override {
        Some(d) => d,
        None => fetch::fetch_freebsd_amd64_image(args.force)?,
    };
    let kernel = fetch::fetch_freebsd_amd64_kernel(args.force)?;

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(&disk);
    let volume = args.run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = prepare_bsd_disk(
        &disk,
        &vdir,
        args.run.persist,
        volume.as_deref(),
        args.disk_size.as_deref(),
    )?;

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&disk.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        attach_extra_disks(&ctx, &args.attach_disk)?;
        let gvproxy = setup_networking_with_agent(&ctx, &args.net, Some(&vdir))?;
        ctx.set_kernel(&kernel, linux::kernel_format(), None, &freebsd_cmdline())
            .context("configuring kernel")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.run.volume.as_deref(), &volume) {
        db::record_volume(name, "freebsd", &image, &dir.to_string_lossy());
    }
    let result = run_machine(
        &machine_id,
        &vdir,
        "freebsd",
        &image,
        &exec_after.join(" "),
        args.vm.cpus,
        args.vm.mem,
        args.run.detach,
        true,
        args.run.volume.as_deref(),
        &exec_after,
        interactive,
        args.verbose,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &args.net.ports, &joined);
    result
}

/// NetBSD kernel command line (override with `$BSDKRUN_NETBSD_CMDLINE`). The root
/// device is arch-specific: the arm64 image is GPT-partitioned, so its root FFS
/// is the wedge `dk1` (`dk0` is the EFI partition); the amd64 image is a bare
/// makefs FFS on a virtio-blk disk, which NetBSD roots as `ld0a`.
///
/// amd64 also needs `console=com`: in a PVH direct boot there's no NetBSD
/// bootloader to hand over bootinfo, and without it the kernel defaults its
/// console to VGA text ("pc") — which libkrun doesn't have — so all output
/// silently vanishes. `console=com` selects the serial console libkrun provides
/// (same as the upstream NetBSD-on-Firecracker setup).
fn netbsd_cmdline() -> String {
    if let Ok(s) = std::env::var("BSDKRUN_NETBSD_CMDLINE") {
        if !s.is_empty() {
            return s;
        }
    }
    match host::Arch::current() {
        Ok(host::Arch::X86_64) => "root=ld0a console=com".to_string(),
        _ => "root=dk1".to_string(),
    }
}

/// NetBSD boots via **direct kernel** — libkrun jumps straight into the kernel,
/// no EFI firmware needed (unlike FreeBSD). The disk + kernel are arch-specific:
///
/// - **arm64** uses bsdkrun's bundled evbarm image (the `gzimg` with the agent
///   injected) + the evbarm `GENERIC64` kernel from the NetBSD CDN (`root=dk1`).
/// - **amd64** PVH-direct-boots the bundled MICROVM kernel + FFS rootfs. Needs a
///   PVH-capable libkrun (the tsirysndr/libkrun `feat/pvh-boot` fork) — stock
///   libkrun boots x86_64 kernels with the Linux protocol and triple-faults.
///
/// `--version` applies only to the arm64 kernel; the images themselves are pinned
/// bundled assets.
fn boot_netbsd(args: BsdArgs) -> Result<()> {
    boot_netbsd_disk(args, None)
}

/// Boot NetBSD, optionally from a specific root disk (`disk_override`, used to
/// boot a committed snapshot) instead of the fetched bundled image.
fn boot_netbsd_disk(mut args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    bsd_inject_repo(&mut args);
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    let joined = prepare_network(&args.net, true)?; // NetBSD DHCPs its IP
    let arch = host::Arch::current()?;

    // amd64 NetBSD is a PVH kernel (MICROVM). Tell libkrun to enter via the
    // PHYS32_ENTRY note instead of the Linux boot protocol. Harmless on a libkrun
    // without PVH support (the flag is simply ignored).
    if matches!(arch, host::Arch::X86_64) {
        std::env::set_var("KRUN_PVH", "1");
    }

    // The kernel is always the bundled asset; a snapshot overrides the disk only.
    let kernel = match arch {
        host::Arch::X86_64 => fetch::fetch_netbsd_amd64_kernel(args.force)?,
        host::Arch::Aarch64 => fetch::fetch_netbsd_kernel(args.version.clone(), args.force)?,
    };
    let disk = match disk_override {
        Some(d) => d,
        None => match arch {
            host::Arch::X86_64 => fetch::fetch_netbsd_amd64_image(args.force)?,
            host::Arch::Aarch64 => fetch::fetch_netbsd_arm64_image(args.force)?,
        },
    };

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(&disk);
    let volume = args.run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = prepare_bsd_disk(
        &disk,
        &vdir,
        args.run.persist,
        volume.as_deref(),
        args.disk_size.as_deref(),
    )?;

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&disk.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        attach_extra_disks(&ctx, &args.attach_disk)?;
        let gvproxy = setup_networking_with_agent(&ctx, &args.net, Some(&vdir))?;
        ctx.set_kernel(&kernel, linux::kernel_format(), None, &netbsd_cmdline())
            .context("configuring kernel")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.run.volume.as_deref(), &volume) {
        db::record_volume(name, "netbsd", &image, &dir.to_string_lossy());
    }
    let result = run_machine(
        &machine_id,
        &vdir,
        "netbsd",
        &image,
        &exec_after.join(" "),
        args.vm.cpus,
        args.vm.mem,
        args.run.detach,
        true,
        args.run.volume.as_deref(),
        &exec_after,
        interactive,
        args.verbose,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &args.net.ports, &joined);
    result
}

/// Locate libkrun's EDK2 firmware (`KRUN_EFI`), keeping a copy in bsdkrun's own
/// cache dir (not the current directory). Overridden by `$BSDKRUN_FIRMWARE` (and
/// by the `--firmware` flag before this is called). macOS only — the EFI
/// firmware ships with libkrun-efi, which is macOS-only.
#[cfg(target_os = "macos")]
fn locate_krun_efi() -> Result<PathBuf> {
    if let Some(f) = std::env::var_os("BSDKRUN_FIRMWARE") {
        let p = PathBuf::from(f);
        if !p.exists() {
            anyhow::bail!("BSDKRUN_FIRMWARE={} does not exist", p.display());
        }
        return Ok(p);
    }
    // Keep a copy under bsdkrun's cache ($BSDKRUN_CACHE / ~/.cache/bsdkrun) so it
    // works from any directory and survives a krunkit upgrade.
    let cache = fetch::cache_dir()?;
    let cached = cache.join("KRUN_EFI.fd");
    if cached.exists() {
        return Ok(cached);
    }
    let src = find_krunkit_firmware()?;
    std::fs::create_dir_all(&cache).ok();
    // Copy-on-write clone from krunkit's copy (`cp -c` — clonefile on APFS),
    // falling back to a plain copy.
    if fetch::run(
        std::process::Command::new("cp")
            .arg("-c")
            .arg(&src)
            .arg(&cached),
        "cp (clone firmware)",
    )
    .is_err()
    {
        let _ = std::fs::remove_file(&cached);
        std::fs::copy(&src, &cached).with_context(|| {
            format!("copying firmware {} -> {}", src.display(), cached.display())
        })?;
    }
    info!(path = %cached.display(), "cached KRUN_EFI firmware");
    Ok(cached)
}

/// Find krunkit's `KRUN_EFI.silent.fd` in its Homebrew install. macOS only —
/// krunkit (and the EFI firmware) exist only there.
#[cfg(target_os = "macos")]
fn find_krunkit_firmware() -> Result<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    if let Ok(out) = std::process::Command::new("brew")
        .args(["--prefix", "krunkit"])
        .output()
    {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                prefixes.push(PathBuf::from(p));
            }
        }
    }
    for p in ["/opt/homebrew/opt/krunkit", "/opt/homebrew", "/usr/local"] {
        prefixes.push(PathBuf::from(p));
    }
    for base in prefixes {
        let fw = base.join("share/krunkit/KRUN_EFI.silent.fd");
        if fw.exists() {
            return Ok(fw);
        }
    }
    anyhow::bail!(
        "could not find the KRUN_EFI firmware. Install it with `brew install krunkit`, \
         or pass --firmware /path/to/KRUN_EFI.fd (or set BSDKRUN_FIRMWARE)."
    )
}

/// Per-machine state dir (`<state>/machines/<id>`), falling back to a temp dir.
fn machine_dir_or_tmp(id: &str) -> std::path::PathBuf {
    let dir = db::machine_dir(id).unwrap_or_else(|_| std::env::temp_dir().join(id));
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// The last path component, for display.
fn basename(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Prepare a BSD machine's root disk. With `volume`, the disk lives at a stable
/// path under `<state>/volumes` and is reused across runs (changes persist);
/// with `persist`, the base disk is booted in place; otherwise it's cloned into
/// `vdir` fresh each boot. Clones use an APFS copy-on-write clone (`cp -c` —
/// instant, no extra disk until the guest writes) so the base stays pristine.
fn prepare_bsd_disk(
    disk: &std::path::Path,
    vdir: &std::path::Path,
    persist: bool,
    volume: Option<&std::path::Path>,
    disk_size: Option<&str>,
) -> Result<PathBuf> {
    // Grow a freshly-cloned disk to `disk_size` (only enlarges); the guest
    // expands its root FS on boot. Never applied to a `persist` base (in place)
    // so the pristine base image is left untouched.
    let grow = |dst: &std::path::Path| -> Result<()> {
        if let Some(size) = disk_size {
            fetch::grow(dst, size)
                .with_context(|| format!("growing disk {} to {size}", dst.display()))?;
        }
        Ok(())
    };
    let ext = disk.extension().and_then(|e| e.to_str()).unwrap_or("img");
    if let Some(voldir) = volume {
        std::fs::create_dir_all(voldir)
            .with_context(|| format!("creating volume dir {}", voldir.display()))?;
        let dst = voldir.join(format!("root.{ext}"));
        if dst.exists() {
            info!(path = %dst.display(), "reusing persistent volume disk");
            return Ok(dst);
        }
        info!(path = %dst.display(), "creating persistent volume (CoW clone of base)");
        clone_cow_file(disk, &dst)?;
        grow(&dst)?;
        return Ok(dst);
    }
    if persist {
        return Ok(disk.to_path_buf());
    }
    let dst = vdir.join(format!("root.{ext}"));
    let _ = std::fs::remove_file(&dst);
    clone_cow_file(disk, &dst)?;
    grow(&dst)?;
    Ok(dst)
}

/// Parse a `--mount HOST:GUEST[:ro]` spec into a bind mount. The host directory
/// must exist (it's canonicalized to an absolute path); the guest path must be
/// absolute.
fn parse_mount(spec: &str) -> Result<linux::BindMount> {
    let parts: Vec<&str> = spec.split(':').collect();
    let (host, guest, ro) = match parts.as_slice() {
        [h, g] => (*h, *g, false),
        [h, g, "ro"] => (*h, *g, true),
        [h, g, "rw"] => (*h, *g, false),
        _ => anyhow::bail!("invalid --mount {spec:?} — expected HOST:GUEST[:ro]"),
    };
    if host.is_empty() || guest.is_empty() {
        anyhow::bail!("invalid --mount {spec:?} — empty HOST or GUEST");
    }
    if !guest.starts_with('/') {
        anyhow::bail!("invalid --mount {spec:?} — GUEST path must be absolute");
    }
    let host = std::fs::canonicalize(host)
        .with_context(|| format!("--mount host directory {host:?} does not exist"))?;
    if !host.is_dir() {
        anyhow::bail!("--mount host path {} is not a directory", host.display());
    }
    Ok(linux::BindMount {
        host,
        guest: guest.to_string(),
        ro,
    })
}

/// Resolve a `--volume NAME` to its directory under `<state>/volumes`, rejecting
/// names that could escape it.
fn volume_dir(name: &str) -> Result<PathBuf> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!("invalid volume name {name:?} — use letters, digits, '-', '_' or '.'");
    }
    Ok(db::volumes_dir()?.join(name))
}

/// Copy `src` to `dst` as a copy-on-write clone where the filesystem supports it
/// (APFS on macOS, reflink on Linux), falling back to a plain copy.
fn clone_cow_file(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    host::cow_copy(src, dst, false)
}

/// A friendly machine name: a pending restart override if set, else a fresh
/// unique `adjective_scientist` (falling back to a non-checked random name only
/// if the DB can't be opened).
fn machine_name() -> String {
    names::take_override().unwrap_or_else(|| {
        db::Db::open()
            .map(|d| d.generate_name())
            .unwrap_or_else(|_| names::random_name())
    })
}

/// Run a machine either in the foreground (records + attaches to this terminal)
/// or detached (`detach`). `watchdog` installs the BSD SMP-shutdown watchdog on
/// the foreground path. `build` creates + configures the libkrun context.
#[allow(clippy::too_many_arguments)]
fn run_machine(
    machine_id: &str,
    vdir: &std::path::Path,
    kind: &str,
    image: &str,
    command: &str,
    cpus: u8,
    mem: u32,
    detach: bool,
    watchdog: bool,
    volume: Option<&str>,
    exec_after: &[String],
    interactive: bool,
    verbose: bool,
    build: impl FnOnce() -> Result<(Ctx, Option<Gvproxy>)>,
) -> Result<()> {
    // A trailing command runs in the guest via its agent once it's up, which
    // needs the VM in the background — so a one-shot command boots detached too
    // (but doesn't announce the id and powers the VM off when the command ends).
    if detach || !exec_after.is_empty() {
        return run_detached(
            machine_id,
            vdir,
            kind,
            image,
            command,
            cpus,
            mem,
            volume,
            exec_after,
            detach,
            interactive,
            verbose,
            build,
        );
    }
    db::record_machine(
        machine_id,
        &machine_name(),
        image,
        kind,
        command,
        "running",
        Some(std::process::id() as i64),
        false,
        cpus as i64,
        mem as i64,
        &vdir.to_string_lossy(),
        volume,
    );
    tty::install();
    if watchdog {
        watchdog::install();
    }
    let (ctx, gvproxy) = build()?;
    info!(id = %machine_id, kind, image, "booting machine");
    finish_recording(ctx, gvproxy, machine_id.to_string())
}

/// Single-quote a string for safe interpolation into a `/bin/sh` command.
fn shell_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A destination directory name for a cloned repo: the URL's basename without a
/// `.git` suffix, restricted to filesystem-safe characters.
fn repo_dir_name(url: &str) -> String {
    let base = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo");
    let base = base.strip_suffix(".git").unwrap_or(base);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if cleaned.is_empty() {
        "repo".to_string()
    } else {
        cleaned
    }
}

/// The guest command that clones `repo` into `$HOME` after boot and records its
/// path in `/etc/bsdkrun-cwd`, so opening a shell drops you inside it. Best-effort
/// `git` install if the base image lacks it. `None` for an empty URL.
fn repo_clone_argv(repo: &str) -> Option<Vec<String>> {
    let repo = repo.trim();
    if repo.is_empty() {
        return None;
    }
    let dest = repo_dir_name(repo);
    let q = shell_squote(repo);
    let script = format!(
        "set -e; export PATH=\"/usr/local/bin:/usr/local/sbin:/usr/pkg/bin:$PATH\"; \
         H=\"${{HOME:-/root}}\"; case \"$H\" in \"\"|/) H=/root;; esac; \
         mkdir -p \"$H\" 2>/dev/null || true; \
         has() {{ command -v \"$1\" >/dev/null 2>&1; }}; \
         if ! has git; then \
           echo '==> installing git'; \
           (has apt-get && apt-get update && apt-get install -y git) || \
           (has apk && apk add --no-cache git) || \
           (has dnf && dnf install -y git) || \
           (has microdnf && microdnf install -y git) || \
           (has yum && yum install -y git) || \
           (has pacman && pacman -Sy --noconfirm git) || \
           (has zypper && zypper --non-interactive install git) || \
           (has pkg && ASSUME_ALWAYS_YES=yes pkg install -y git) || \
           (has pkgin && pkgin -y install git) || \
           (has pkg_add && {{ A=$(uname -p 2>/dev/null); [ \"$A\" = x86_64 ] && A=amd64; \
            R=$(uname -r 2>/dev/null | cut -d. -f1).0; \
            PKG_PATH=\"https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/$A/$R/All/\" \
              pkg_add git; }}) || \
           (has nix-env && nix-env -iA nixpkgs.git) || \
           (has nix && nix profile install nixpkgs#git) || true; \
         fi; \
         has git || {{ echo 'error: could not install git in the guest'; exit 1; }}; \
         echo '==> cloning {dest}'; \
         git clone --depth=1 {q} \"$H/{dest}\"; \
         printf '%s\\n' \"$H/{dest}\" > /etc/bsdkrun-cwd 2>/dev/null || true; \
         echo '==> cloned into '\"$H/{dest}\"",
    );
    Some(vec!["sh".to_string(), "-lc".to_string(), script])
}

fn boot_linux(args: LinuxArgs) -> Result<()> {
    let repo = args
        .repo
        .as_deref()
        .and_then(repo_clone_argv)
        .unwrap_or_default();
    boot_linux_from(args, None, &repo)
}

/// Boot a Linux/OCI machine.
///
/// `rootfs_override` clones the rootfs from that path instead of the pulled
/// image's cache — used to boot a *snapshot*/built flavor (its saved rootfs)
/// while still reading the base image's config for the entrypoint.
///
/// `provision` is a command run in the guest *after* boot, via its agent (the
/// `exec_after` hook) — used to install a flavor's packages/tools. A non-empty
/// `provision` implies the machine boots in the background so the parent can
/// wait for the agent and run it (see [`run_machine`]).
/// If a machine is joining a `--network` (or was given a `--name`), resolve its
/// name (recorded via the name override so `ps`/DNS agree) and join the network —
/// which sets `BSDKRUN_NET_*` for [`setup_networking_with_agent`]. Returns the
/// (network, member) to record as membership once the machine row exists.
/// `dhcp` = true for BSD guests (they DHCP their IP); false for Linux (static
/// kernel IP). Returns `(network, member, dhcp)` to finalize once booted.
fn prepare_network(net: &NetConfig, dhcp: bool) -> Result<Option<(String, String, bool)>> {
    if net.network.is_none() && net.name.is_none() {
        return Ok(None);
    }
    let member = match &net.name {
        Some(n) => n.clone(),
        None => db::Db::open()
            .map(|d| d.generate_name())
            .unwrap_or_else(|_| names::random_name()),
    };
    names::set_override(&member);
    if let Some(network) = &net.network {
        network::join(network, &member, dhcp)?;
        return Ok(Some((network.clone(), member, dhcp)));
    }
    Ok(None)
}

/// Finalize network membership after boot: a Linux (static) member just records
/// its allocated IP; a BSD (dhcp) member discovers its leased IP and wires up its
/// agent forward + DNS. No-op when not on a network.
fn finalize_network(
    machine_id: &str,
    agent_dir: Option<&std::path::Path>,
    ports: &[PortForward],
    joined: &Option<(String, String, bool)>,
) {
    if let Some((network, member, dhcp)) = joined {
        if *dhcp {
            if let Some(dir) = agent_dir {
                if let Err(e) = network::finalize_dhcp(network, member, machine_id, dir, ports) {
                    tracing::warn!("network finalize failed: {e:#}");
                }
            }
        } else if let Ok(db) = db::Db::open() {
            let ip = std::env::var("BSDKRUN_NET_IP").unwrap_or_default();
            let _ = db.set_machine_network(machine_id, network, &ip);
        }
    }
}

fn boot_linux_from(
    args: LinuxArgs,
    rootfs_override: Option<PathBuf>,
    provision: &[String],
) -> Result<()> {
    // Prepare everything that can fail before we fork / touch the hypervisor.
    let kernel = linux::ensure_kernel(args.kernel.clone(), &args.kernel_version)?;
    let image = oci::pull(&args.image)?;
    let mut ep =
        linux::resolve_entrypoint(&image.config, args.entrypoint.as_deref(), &args.command);
    // `-e K=V` (and flavor defaults) override the image's env in the guest.
    for kv in &args.env {
        let key = kv.split('=').next().unwrap_or("");
        ep.env.retain(|e| e.split('=').next() != Some(key));
        ep.env.push(kv.clone());
    }
    info!(argv = ?ep.argv, "resolved entrypoint");
    // Clone source: a snapshot's saved rootfs, else the pulled image's rootfs.
    let rootfs_src = rootfs_override.as_deref().unwrap_or(&image.rootfs);

    // Persist the image (best-effort; DB problems never block booting).
    db::record_image(
        &args.image,
        &image.digest,
        image.size,
        &image.rootfs.to_string_lossy(),
    );

    // Join a global network (allocate IP + register DNS + set BSDKRUN_NET_*)
    // before we build the ctx, so `setup_networking_with_agent` bridges in.
    // Linux uses a static kernel IP (dhcp = false).
    let joined = prepare_network(&args.net, false)?;

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let command = ep.argv.join(" ");

    // A named volume makes the rootfs persist across runs. It needs virtio-fs
    // (an initramfs is a RAM disk — nothing to persist).
    if args.volume.is_some() && args.initramfs {
        anyhow::bail!("--volume needs virtio-fs; it can't persist an --initramfs (RAM) rootfs");
    }
    let volume = args.volume.as_deref().map(volume_dir).transpose()?;

    // Host directory bind mounts (`--mount HOST:GUEST[:ro]`), shared via virtio-fs.
    let mounts: Vec<linux::BindMount> = args
        .mounts
        .iter()
        .map(|s| parse_mount(s))
        .collect::<Result<_>>()?;

    // Decide up front whether networking will come up, so the baked-in initramfs
    // `/init` + kernel `ip=` match what `setup_networking` will actually do.
    let net_up = !args.net.no_net && net::locate().is_ok();

    // A detached machine with no explicit command keeps a console shell alive
    // across exits (so `shell` behaves like a persistent VM you attach to);
    // otherwise the workload runs once and the machine powers off when it ends.
    let persistent = args.detach && args.command.is_empty();

    // Prepare the rootfs before forking so failures surface synchronously.
    // A volume is a persistent, writable virtio-fs root (CoW-cloned on first use);
    // otherwise it's a per-machine virtio-fs clone, or an initramfs (--initramfs).
    let root_mode = match (&volume, args.virtiofs()) {
        (Some(voldir), _) => LinuxRoot::Virtiofs(linux::prepare_volume_root(
            rootfs_src, &ep, net_up, persistent, voldir, &mounts,
        )?),
        (None, true) => LinuxRoot::Virtiofs(linux::prepare_virtiofs_root(
            rootfs_src, &ep, net_up, persistent, &vdir, &mounts,
        )?),
        (None, false) => {
            linux::warn_initramfs_memory(rootfs_src, args.vm.mem);
            LinuxRoot::Initramfs(linux::build_initramfs(
                rootfs_src, &ep, net_up, persistent, &mounts,
            )?)
        }
    };

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        // `hvc*` uses libkrun's implicit virtio-console; `ttyS*` its explicit serial.
        if !args.console.starts_with("hvc") {
            ctx.attach_stdio_serial_console()
                .context("wiring guest serial console to stdio")?;
        }
        let gvproxy =
            configure_linux_ctx(&ctx, &args, &kernel, &root_mode, &mounts, net_up, &vdir)?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.volume.as_deref(), &volume) {
        db::record_volume(name, "linux", &args.image, &dir.to_string_lossy());
    }
    // Linux never uses the SMP-shutdown watchdog: it redirects fd 2, which
    // libkrun's implicit virtio-console (hvc0) claims for the guest.
    let result = run_machine(
        &machine_id,
        &vdir,
        "linux",
        &args.image,
        &command,
        args.vm.cpus,
        args.vm.mem,
        args.detach,
        false,
        args.volume.as_deref(),
        provision,
        false,
        false,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &args.net.ports, &joined);
    result
}

/// How a Linux guest's root filesystem is provided.
enum LinuxRoot {
    /// Whole rootfs loaded into RAM (`--initramfs`).
    Initramfs(PathBuf),
    /// Copy-on-write virtio-fs clone. Ephemeral per-machine by default, or a
    /// persistent named volume (`-v NAME`) whose clone lives in the volume dir.
    Virtiofs(PathBuf),
}

/// Configure networking + rootfs + kernel on `ctx` (shared by the foreground and
/// detached paths; the caller wires the console first). Returns the gvproxy.
#[allow(clippy::too_many_arguments)]
fn configure_linux_ctx(
    ctx: &Ctx,
    args: &LinuxArgs,
    kernel: &std::path::Path,
    root: &LinuxRoot,
    mounts: &[linux::BindMount],
    net_up: bool,
    vdir: &std::path::Path,
) -> Result<Option<Gvproxy>> {
    let gvproxy = setup_networking_with_agent(ctx, &args.net, Some(vdir))?;
    // Share each `--mount` host directory over virtio-fs; the generated init
    // mounts them by matching tag.
    for (i, m) in mounts.iter().enumerate() {
        ctx.add_virtiofs(&linux::mount_tag(i), &m.host)
            .with_context(|| format!("sharing --mount host dir {}", m.host.display()))?;
    }
    // We boot our own init in every virtio-fs mode (not libkrun's init.krun,
    // which is for the bundled libkrunfw kernel).
    match root {
        LinuxRoot::Virtiofs(dir) => {
            ctx.set_root(dir)
                .context("configuring virtio-fs root (needs a CONFIG_VIRTIO_FS=y kernel)")?;
            let cmdline = linux::virtiofs_cmdline(&args.console, net_up);
            ctx.set_kernel(kernel, linux::kernel_format(), None, &cmdline)
                .context("configuring kernel")?;
        }
        LinuxRoot::Initramfs(img) => {
            let cmdline = linux::kernel_cmdline(&args.console, net_up);
            ctx.set_kernel(kernel, linux::kernel_format(), Some(img), &cmdline)
                .context("configuring kernel")?;
        }
    }
    Ok(gvproxy)
}

/// Like `finish`, but records the guest's exit code in the state DB first.
fn finish_recording(ctx: Ctx, gvproxy: Option<Gvproxy>, machine_id: String) -> Result<()> {
    let result = ctx.start_enter();
    tty::restore_stdin_termios();
    let code = result.context("starting machine")?;
    info!(code, "guest exited");
    db::update_machine_status(&machine_id, "exited", Some(code as i64));
    drop(gvproxy);
    std::process::exit(code);
}

/// Run a machine in the background (any guest type): fork, record the child in
/// the state DB, print its id, and detach the child — wiring the guest console
/// to a per-machine PTY broker (console.log + console.sock). `build` (run in the
/// child, after the console is wired) creates + configures the libkrun context.
#[allow(clippy::too_many_arguments)]
fn run_detached(
    machine_id: &str,
    vdir: &std::path::Path,
    kind: &str,
    image: &str,
    command: &str,
    cpus: u8,
    mem: u32,
    volume: Option<&str>,
    exec_after: &[String],
    keep_running: bool,
    interactive: bool,
    verbose: bool,
    build: impl FnOnce() -> Result<(Ctx, Option<Gvproxy>)>,
) -> Result<()> {
    use std::io::Write;
    // Flush any pending output (e.g. the pull progress) before forking.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        anyhow::bail!("fork failed: {}", std::io::Error::last_os_error());
    }
    if pid > 0 {
        // Parent: record the running machine and print its id, like `docker -d`.
        db::record_machine(
            machine_id,
            &machine_name(),
            image,
            kind,
            command,
            "running",
            Some(pid as i64),
            true,
            cpus as i64,
            mem as i64,
            &vdir.to_string_lossy(),
            volume,
        );
        // No trailing command: classic `-d`, just announce the id.
        if exec_after.is_empty() {
            println!("{machine_id}");
            return Ok(());
        }
        // Command mode: the VM boots in the child (above); here in the parent we
        // wait for its agent, run the command against it, then (for a one-shot)
        // power the VM off. `-d` keeps it running and prints the id first.
        if keep_running {
            println!("{machine_id}");
        }
        return run_guest_command(
            machine_id,
            kind,
            exec_after,
            keep_running,
            interactive,
            verbose,
            pid as libc::pid_t,
        );
    }

    // Child: detach from the terminal/session, wire the console, boot.
    unsafe { libc::setsid() };
    // Send bsdkrun's own logs (fd 2) to a file in the machine dir.
    if let Ok(logf) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(vdir.join("bsdkrun.log"))
    {
        use std::os::fd::AsRawFd;
        unsafe { libc::dup2(logf.as_raw_fd(), 2) };
        std::mem::forget(logf); // fd 2 now owns it
    }

    let result = (|| -> Result<i32> {
        // Wire the guest console (fd 0/1) to the broker's PTY; a thread fans it
        // out to console.log + console.sock (see the console module).
        let console_fd = console::setup_detached(vdir)?;
        unsafe {
            libc::dup2(console_fd, 0);
            libc::dup2(console_fd, 1);
        }
        // Signal cleanup so `stop` (SIGTERM) tears down gvproxy.
        tty::install();
        let (ctx, gvproxy) = build()?;
        info!(id = %machine_id, kind, "detached machine booting");
        let code = ctx.start_enter().context("starting machine")?;
        db::update_machine_status(machine_id, "exited", Some(code as i64));
        drop(gvproxy);
        Ok(code)
    })();

    let code = match result {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("detached machine failed: {e:#}");
            db::update_machine_status(machine_id, "exited", Some(1));
            1
        }
    };
    unsafe { libc::_exit(code) };
}

/// How long to wait for a freshly-booted guest's agent before giving up on a
/// trailing command. A BSD guest takes tens of seconds to reach multiuser.
const GUEST_AGENT_BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Poll a booting machine's guest agent until it answers (or we time out / the
/// machine dies), returning the forwarded host port to `exec` against.
///
/// A BSD guest takes ~15-20s to reach the agent. When `console` is given
/// (`--verbose`), stream the guest's boot console to **stdout** while we wait —
/// so CI (and the curious) see the full boot, and e2e can assert on it.
/// Otherwise show a terse "still booting" counter on a terminal so the wait
/// doesn't look like a hang.
/// Boot milestones timestamped from the guest console by [`wait_for_agent`], each
/// measured from launch. `None` means the anchor line never appeared: NetBSD
/// direct-boots with no firmware/loader, so its `kernel_entry` ≈ `first_output`;
/// a very fast boot may reach the agent before a milestone is scanned.
#[derive(Default)]
struct BootMarks {
    /// First byte on the console — end of the hypervisor/firmware dead time.
    first_output: Option<std::time::Duration>,
    /// Kernel takes over (`---<<BOOT>>---` / NetBSD `booting ...`) — past firmware+loader.
    kernel_entry: Option<std::time::Duration>,
    /// Root filesystem mount / fsck — the kernel→userland handoff, start of rc.
    root_mount: Option<std::time::Duration>,
}

/// Background console watcher that timestamps [`BootMarks`] off the main
/// agent-poll loop (which blocks on `agent::ping` each iteration). Milestone
/// times are milliseconds-from-launch in atomics (0 = not seen yet); dropping the
/// scanner signals its thread to stop.
struct BootScanner {
    first: std::sync::Arc<std::sync::atomic::AtomicU64>,
    kernel: std::sync::Arc<std::sync::atomic::AtomicU64>,
    root: std::sync::Arc<std::sync::atomic::AtomicU64>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl BootScanner {
    fn spawn(log: std::path::PathBuf, start: std::time::Instant) -> Self {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;
        let s = BootScanner {
            first: Arc::new(AtomicU64::new(0)),
            kernel: Arc::new(AtomicU64::new(0)),
            root: Arc::new(AtomicU64::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let (first, kernel, root, stop) = (
            s.first.clone(),
            s.kernel.clone(),
            s.root.clone(),
            s.stop.clone(),
        );
        std::thread::spawn(move || {
            // `.max(1)` so a milestone at t≈0 never reads back as "not seen".
            let now = || (start.elapsed().as_millis() as u64).max(1);
            while !stop.load(Ordering::Relaxed) {
                if let Ok(bytes) = std::fs::read(&log) {
                    if first.load(Ordering::Relaxed) == 0 && !bytes.is_empty() {
                        first.store(now(), Ordering::Relaxed);
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    // Kernel takes over: FreeBSD `---<<BOOT>>---`, NetBSD `booting ...`.
                    if kernel.load(Ordering::Relaxed) == 0
                        && ["---<<BOOT>>---", "booting ..."]
                            .iter()
                            .any(|p| text.contains(p))
                    {
                        kernel.store(now(), Ordering::Relaxed);
                    }
                    // Root mount / fsck — the kernel→userland (rc) handoff, and the
                    // last milestone, so the thread can stop once it appears.
                    if root.load(Ordering::Relaxed) == 0
                        && ["Trying to mount root", "Starting root file system check"]
                            .iter()
                            .any(|p| text.contains(p))
                    {
                        root.store(now(), Ordering::Relaxed);
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        s
    }

    fn marks(&self) -> BootMarks {
        use std::sync::atomic::Ordering;
        let d = |a: &std::sync::atomic::AtomicU64| {
            let v = a.load(Ordering::Relaxed);
            (v != 0).then(|| std::time::Duration::from_millis(v))
        };
        BootMarks {
            first_output: d(&self.first),
            kernel_entry: d(&self.kernel),
            root_mount: d(&self.root),
        }
    }
}

impl Drop for BootScanner {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Report the boot timing gathered by [`wait_for_agent`]: one structured `info!`
/// line (shown by default) plus, when `BSDKRUN_BOOT_TIMING` is set, a per-phase
/// breakdown to stderr (firmware/loader → kernel probe → userland rc), computed
/// as deltas between whichever milestones were reached.
fn log_boot_timing(
    id: &str,
    kind: &str,
    total: std::time::Duration,
    marks: &BootMarks,
    show: bool,
) {
    let ms = |d: Option<std::time::Duration>| d.map(|x| x.as_millis() as u64);
    tracing::info!(
        total_ms = total.as_millis() as u64,
        to_first_output_ms = ms(marks.first_output),
        to_kernel_entry_ms = ms(marks.kernel_entry),
        to_root_mount_ms = ms(marks.root_mount),
        "boot timing (launch → agent-ready)"
    );
    if !show {
        return;
    }
    // Ordered checkpoints actually reached, ending at agent-ready; print the gap
    // between each consecutive pair (launch is the implicit zero).
    let mut pts: Vec<(&str, std::time::Duration)> = Vec::new();
    if let Some(t) = marks.first_output {
        pts.push(("first console output", t));
    }
    if let Some(t) = marks.kernel_entry {
        pts.push(("kernel entry", t));
    }
    if let Some(t) = marks.root_mount {
        pts.push(("root mount (rc start)", t));
    }
    pts.push(("agent ready", total));

    let s = |d: std::time::Duration| format!("{:>6.2}s", d.as_secs_f64());
    eprintln!("\x1b[36m[boot timing]\x1b[0m {kind} {id}");
    let mut prev_label = "launch";
    let mut prev = std::time::Duration::ZERO;
    for (label, at) in pts {
        let seg = format!("{prev_label} → {label}");
        eprintln!("  {:<45} {}", seg, s(at.saturating_sub(prev)));
        prev_label = label;
        prev = at;
    }
    eprintln!("  {:<45} {}", "total (launch → agent ready)", s(total));
}

fn wait_for_agent(id: &str, kind: &str, console: Option<&std::path::Path>) -> Result<u16> {
    use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
    let counter = console.is_none() && std::io::stderr().is_terminal();
    // A little braille spinner for the counter — cycles each poll.
    const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let mut frame = 0usize;
    let start = std::time::Instant::now();
    let deadline = start + GUEST_AGENT_BOOT_TIMEOUT;
    // Boot timing: timestamp boot milestones from the guest console to split the
    // wait into firmware/loader, kernel device probe, and userland rc. `start` is
    // captured just after the fork, so it tracks the real launch → usable-agent
    // wall clock; the console log is resolved independently of `--verbose` so the
    // milestones are caught even on a quiet boot. Report at `info!` (shown by
    // default) plus a phase breakdown to stderr when BSDKRUN_BOOT_TIMING is set.
    let boot_timing = std::env::var_os("BSDKRUN_BOOT_TIMING").is_some();
    let timing_log = db::Db::open()
        .ok()
        .and_then(|db| db.find_machine(id).ok())
        .map(|vm| std::path::PathBuf::from(vm.state_dir).join("console.log"));
    // Scan the console on a background thread, not in this loop: `agent::ping`
    // below blocks up to its read timeout each iteration, which would starve the
    // scan and collapse every milestone onto one late sample. The scanner drops
    // (and stops) when `wait_for_agent` returns on any path.
    let scanner = timing_log
        .as_ref()
        .map(|p| BootScanner::spawn(p.clone(), start));
    // Verbose: tail the console log, remembering how far we've streamed.
    let mut tail: Option<std::fs::File> = None;
    let mut off: u64 = 0;
    let mut drain_console = || {
        let Some(path) = console else { return };
        if tail.is_none() {
            tail = std::fs::File::open(path).ok();
        }
        if let Some(f) = tail.as_mut() {
            if f.seek(SeekFrom::Start(off)).is_ok() {
                let mut buf = Vec::new();
                if let Ok(n) = f.read_to_end(&mut buf) {
                    if n > 0 {
                        let out = std::io::stdout();
                        let mut h = out.lock();
                        let _ = h.write_all(&buf);
                        let _ = h.flush();
                        off += n as u64;
                    }
                }
            }
        }
    };
    // Animate/stream at a smooth ~12fps, but only actually poll the agent (a real
    // round-trip) about twice a second.
    let tick = std::time::Duration::from_millis(if counter { 80 } else { 250 });
    let mut last_poll = start - std::time::Duration::from_secs(1); // poll immediately
    loop {
        let now = std::time::Instant::now();
        if now.duration_since(last_poll) >= std::time::Duration::from_millis(500) {
            last_poll = now;
            // agent_target() also confirms the machine's pid is alive; ping() does
            // a real agent round-trip, so it's only true once the agent is up.
            if let Ok((_vm, port)) = agent_target(id) {
                if agent::ping(port) {
                    drain_console(); // flush any final boot output
                    if counter {
                        eprint!("\r\x1b[K"); // clear the progress line
                        let _ = std::io::stderr().flush();
                    }
                    let marks = scanner.as_ref().map(|s| s.marks()).unwrap_or_default();
                    log_boot_timing(id, kind, start.elapsed(), &marks, boot_timing);
                    return Ok(port);
                }
            }
            // Fail fast if the guest died during boot rather than wait out the timeout.
            if let Ok(db) = db::Db::open() {
                if let Ok(vm) = db.find_machine(id) {
                    if vm.status == "exited" || !vm.pid.map(db::pid_alive).unwrap_or(false) {
                        drain_console();
                        if counter {
                            eprint!("\r\x1b[K");
                        }
                        anyhow::bail!(
                            "machine {id} exited before its agent came up — see `bsdkrun logs {id}`"
                        );
                    }
                }
            }
        }
        if now >= deadline {
            anyhow::bail!(
                "timed out after {}s waiting for the guest agent on machine {id} \
                 (still booting? try `bsdkrun logs {id}`)",
                GUEST_AGENT_BOOT_TIMEOUT.as_secs()
            );
        }
        drain_console();
        if counter {
            eprint!(
                "\r\x1b[K\x1b[36m{}\x1b[0m booting {kind} microVM — waiting for the guest agent ({}s)…",
                SPIN[frame % SPIN.len()],
                start.elapsed().as_secs()
            );
            let _ = std::io::stderr().flush();
            frame += 1;
        }
        std::thread::sleep(tick);
    }
}

/// Parent-side of a BSD boot with a trailing command: wait for the guest agent,
/// run the command against it (streaming stdio), and — unless `keep_running`
/// (`-d`) — power the VM off afterward. Exits with the command's status.
fn run_guest_command(
    id: &str,
    kind: &str,
    argv: &[String],
    keep_running: bool,
    interactive: bool,
    verbose: bool,
    child_pid: libc::pid_t,
) -> Result<()> {
    use std::io::IsTerminal;
    // `--verbose`: stream the guest's boot console (its state_dir/console.log)
    // while we wait for the agent.
    let console = verbose
        .then(|| db::Db::open().ok().and_then(|db| db.find_machine(id).ok()))
        .flatten()
        .map(|vm| std::path::PathBuf::from(vm.state_dir).join("console.log"));
    let port = wait_for_agent(id, kind, console.as_deref())?;
    // Only the synthesized shell gets a PTY, and only when both ends are a real
    // terminal. Explicit commands run non-interactively so their output isn't
    // lost to the guest agent's PTY drain race on a fast exit.
    let tty = interactive && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    // The synthesized interactive shell (`interactive`) prefers bash and sets a
    // sane TERM on BSD; an explicit command runs verbatim with no injected env.
    let (argv, env): (Vec<String>, Vec<String>) = if interactive {
        (interactive_shell_argv(), interactive_shell_env(kind))
    } else {
        (argv.to_vec(), Vec::new())
    };
    let code = agent::exec(port, &argv, &env, tty).map_err(|e| agent_error(kind, e))?;
    if !keep_running {
        // One-shot: tear the VM down (SIGTERM -> the child's cleanup + poweroff).
        unsafe { libc::kill(child_pid, libc::SIGTERM) };
        db::update_machine_status(id, "exited", Some(128 + libc::SIGTERM as i64));
    }
    std::process::exit(code);
}

// ---- management subcommands -------------------------------------------------

/// Truncate a string to `n` display chars, adding an ellipsis if cut.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
fn cmd_ps(all: bool, json: bool) -> Result<()> {
    let db = db::Db::open()?;
    let machines = db.list_machines()?;
    if json {
        let mut out = Vec::new();
        for m in machines {
            let running = m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false);
            if m.status == "running" && !running {
                db.set_machine_status(&m.id, "exited", m.exit_code).ok();
            }
            if !all && !running {
                continue;
            }
            out.push(serde_json::json!({
                "id": m.id,
                "name": m.name,
                "image": m.image,
                "kind": m.kind,
                "command": m.command,
                "status": if running { "running" } else { "exited" },
                "running": running,
                "exit_code": m.exit_code,
                "pid": m.pid,
                "detached": m.detached,
                "cpus": m.cpus,
                "mem": m.mem,
                "volume": m.volume,
                "state_dir": m.state_dir,
                "created_at": m.created_at,
                "finished_at": m.finished_at,
            }));
        }
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    println!(
        "{:<14}  {:<20}  {:<22}  {:<24}  {:<15}  {}",
        "ID", "NAME", "IMAGE", "STATUS", "CREATED", "COMMAND"
    );
    for m in machines {
        // Reconcile: a "running" row whose process is gone is really exited.
        let running = m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false);
        if m.status == "running" && !running {
            db.set_machine_status(&m.id, "exited", m.exit_code).ok();
        }
        if !all && !running {
            continue;
        }
        // Docker-style STATUS: "Up 5 minutes" / "Exited (0) 3 minutes ago".
        let status = if running {
            format!("Up {}", db::human_duration_since(&m.created_at))
        } else {
            let dur = m.finished_at.as_deref().map(db::human_duration_since);
            match (m.exit_code, dur) {
                (Some(c), Some(d)) => format!("Exited ({c}) {d} ago"),
                (Some(c), None) => format!("Exited ({c})"),
                (None, Some(d)) => format!("Exited {d} ago"),
                (None, None) => "Exited".to_string(),
            }
        };
        println!(
            "{:<14}  {:<20}  {:<22}  {:<24}  {:<15}  {}",
            m.id,
            truncate(m.name.as_deref().unwrap_or("-"), 20),
            truncate(&m.image, 22),
            status,
            format!("{} ago", db::human_duration_since(&m.created_at)),
            truncate(&m.command, 40)
        );
    }
    Ok(())
}

/// Record any BSD disk images sitting in the cache that aren't in the DB yet, so
/// `images` lists them (they may predate the DB, or be from another checkout).
fn reconcile_bsd_images() {
    let Ok(cache) = fetch::cache_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&cache) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Fetched images are named `<os>-<version>.<ext>` (freebsd → raw, netbsd → img).
        let is_bsd = (name.starts_with("freebsd-") && name.ends_with(".raw"))
            || (name.starts_with("netbsd-") && name.ends_with(".img"));
        if !is_bsd {
            continue;
        }
        let path = entry.path();
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        let reference = name.trim_end_matches(".raw").trim_end_matches(".img");
        db::record_image(
            reference,
            &format!("file:{}", path.display()),
            size,
            &path.to_string_lossy(),
        );
    }
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
fn cmd_images(json: bool) -> Result<()> {
    reconcile_bsd_images();
    let db = db::Db::open()?;
    let images = db.list_images()?;
    if json {
        let out: Vec<_> = images
            .iter()
            .map(|im| {
                serde_json::json!({
                    "id": im.id,
                    "reference": im.reference,
                    "digest": im.digest,
                    "size": im.size,
                    "rootfs": im.rootfs,
                    "created_at": im.created_at,
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    println!(
        "{:<14}  {:<32}  {:<10}  {}",
        "ID", "REFERENCE", "SIZE", "CREATED"
    );
    for im in images {
        println!(
            "{:<14}  {:<32}  {:<10}  {}",
            im.id,
            truncate(&im.reference, 32),
            oci::human_size(im.size.max(0) as u64),
            db::age(&im.created_at)
        );
    }
    Ok(())
}

fn cmd_volume_ls(json: bool) -> Result<()> {
    let db = db::Db::open()?;
    // Hide reserved flavor-build volumes (the provisioning cache layers).
    let rows: Vec<_> = db
        .list_volumes()?
        .into_iter()
        .filter(|v| !v.name.starts_with(FLAVOR_BUILD_PREFIX))
        .collect();
    let tracked: std::collections::HashSet<String> = rows.iter().map(|v| v.name.clone()).collect();
    if json {
        let mut out = Vec::new();
        for v in &rows {
            out.push(serde_json::json!({
                "name": v.name,
                "guest": v.kind,
                "base": v.base,
                "path": v.path,
                "size": volume_size(&v.path),
                "created_at": v.created_at,
                "tracked": true,
            }));
        }
        if let Ok(entries) = std::fs::read_dir(db::volumes_dir()?) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir()
                    && !tracked.contains(&name)
                    && !name.starts_with(FLAVOR_BUILD_PREFIX)
                {
                    out.push(serde_json::json!({
                        "name": name,
                        "guest": serde_json::Value::Null,
                        "base": serde_json::Value::Null,
                        "path": e.path().to_string_lossy(),
                        "size": volume_size(&e.path().to_string_lossy()),
                        "created_at": serde_json::Value::Null,
                        "tracked": false,
                    }));
                }
            }
        }
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    println!(
        "{:<20}  {:<9}  {:<28}  {:<10}  {}",
        "NAME", "GUEST", "BASE", "SIZE", "CREATED"
    );
    for v in &rows {
        println!(
            "{:<20}  {:<9}  {:<28}  {:<10}  {}",
            truncate(&v.name, 20),
            v.kind,
            truncate(&v.base, 28),
            volume_size(&v.path),
            db::age(&v.created_at),
        );
    }
    // Also surface any on-disk volume dirs not tracked in the DB (e.g. created
    // before volumes were recorded).
    if let Ok(entries) = std::fs::read_dir(db::volumes_dir()?) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir()
                && !tracked.contains(&name)
                && !name.starts_with(FLAVOR_BUILD_PREFIX)
            {
                println!(
                    "{:<20}  {:<9}  {:<28}  {:<10}  {}",
                    truncate(&name, 20),
                    "-",
                    "-",
                    volume_size(&e.path().to_string_lossy()),
                    "-",
                );
            }
        }
    }
    Ok(())
}

/// On-disk size of a volume via `du -sk` (counts allocated blocks, so CoW-shared
/// data isn't double-counted); "-" if it can't be determined.
fn volume_size(path: &str) -> String {
    let out = std::process::Command::new("du")
        .args(["-sk", path])
        .output()
        .ok();
    out.filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .map(|kb| oci::human_size(kb * 1024))
        .unwrap_or_else(|| "-".to_string())
}

fn cmd_volume_rm(names: &[String], force: bool) -> Result<()> {
    let db = db::Db::open()?;
    // Volumes currently attached to a running machine.
    let in_use: std::collections::HashSet<String> = db
        .list_machines()?
        .into_iter()
        .filter(|m| m.pid.map(db::pid_alive).unwrap_or(false))
        .filter_map(|m| m.volume)
        .collect();

    let mut failed = false;
    for name in names {
        let row = db.find_volume(name)?;
        let dir = match &row {
            Some(r) => PathBuf::from(&r.path),
            None => db::volumes_dir()?.join(name),
        };
        if row.is_none() && !dir.exists() {
            eprintln!("Error: no such volume: {name}");
            failed = true;
            continue;
        }
        if in_use.contains(name) && !force {
            eprintln!("Error: volume {name:?} is in use by a running machine (use --force)");
            failed = true;
            continue;
        }
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                eprintln!("Error: removing {}: {e}", dir.display());
                failed = true;
                continue;
            }
        }
        db.remove_volume(name).ok();
        println!("{name}");
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_stop(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    match vm.pid {
        Some(pid) if db::pid_alive(pid) => {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            // The process exits 128+SIGTERM on our signal handler; record that so
            // `ps` shows a Docker-style "Exited (143)".
            let code = vm.exit_code.or(Some(128 + libc::SIGTERM as i64));
            db.set_machine_status(&vm.id, "exited", code).ok();
            println!("{}", vm.id);
            Ok(())
        }
        _ => {
            db.set_machine_status(&vm.id, "exited", vm.exit_code).ok();
            println!("{} is not running", vm.id);
            Ok(())
        }
    }
}

/// `bsdkrun update <id> [--cpus N] [--mem M]` — change a machine's recorded
/// vCPU / memory. libkrun fixes VM resources at boot, so the new values apply on
/// the next `start` (an in-place restart), not to a currently-running guest.
fn cmd_update(id: &str, cpus: Option<u8>, mem: Option<u32>) -> Result<()> {
    if cpus.is_none() && mem.is_none() {
        anyhow::bail!("nothing to update — pass --cpus and/or --mem");
    }
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let new_cpus = cpus.map(i64::from).unwrap_or(vm.cpus).clamp(1, 255);
    let new_mem = mem.map(i64::from).unwrap_or(vm.mem).max(64);
    db.set_machine_resources(&vm.id, new_cpus, new_mem)?;
    let running = vm.pid.map(db::pid_alive).unwrap_or(false) && vm.status == "running";
    if running {
        info!(
            id = %vm.id,
            "resources updated ({new_cpus} vCPU / {new_mem} MiB) — restart the machine to apply"
        );
    } else {
        info!(id = %vm.id, "resources updated ({new_cpus} vCPU / {new_mem} MiB)");
    }
    println!("{}", vm.id);
    Ok(())
}

/// Restart a stopped machine *in place*: re-boot the recorded image / resources
/// / volume under the SAME id (like `docker start`), rather than minting a new
/// machine. Detached. The guest OS is inferred from the recorded image ref.
fn cmd_start(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;

    // Already up? Nothing to do.
    if vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false) {
        println!("{}", vm.id);
        return Ok(());
    }

    let cpus = vm.cpus.clamp(1, 255) as u8;
    let mem = vm.mem.max(64) as u32;
    let volume = vm.volume.clone();

    // Clear the stale per-machine state so the re-boot starts from a fresh
    // clone. The DB row is left in place (the boot re-records it via INSERT OR
    // REPLACE), so it flips exited→running rather than vanishing from `ps`. A
    // named volume lives elsewhere and is reused, so its changes persist.
    // NB: don't wipe the whole state dir here — that means an `rm -rf` of the old
    // read-only nix rootfs, which is slow enough to make Play "spin forever". The
    // boot path (prepare_linux_root) renames the stale rootfs aside and GC's it in
    // the background, so the restart returns promptly. Old sockets/logs/port files
    // are simply overwritten on the new boot.

    // The next boot picks up this id + name instead of generating fresh ones.
    id::set_override(&vm.id);
    if let Some(name) = &vm.name {
        names::set_override(name);
    }

    let net = NetConfig {
        no_net: false,
        ports: vec![],
        mac: None,
        network: None,
        name: None,
    };
    let vmcfg = VmConfig { cpus, mem };

    let reference = vm.image.to_lowercase();
    let is_freebsd = vm.kind == "firmware" || reference.starts_with("freebsd");
    let is_netbsd = vm.kind == "kernel" || reference.starts_with("netbsd");

    if vm.kind == "linux" {
        boot_linux(LinuxArgs {
            image: vm.image.clone(),
            kernel: None,
            kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
            detach: true,
            initramfs: false,
            volume,
            mounts: vec![],
            entrypoint: None,
            env: vec![],
            console: "hvc0".to_string(),
            net,
            vm: vmcfg,
            repo: None,
            command: vec![], // persistent restart — keep a console shell alive
        })
    } else if is_freebsd || is_netbsd {
        let args = BsdArgs {
            version: None, // bundled image (as originally booted)
            firmware: None,
            force: false,
            attach_disk: vec![],
            disk_size: None,
            run: RunConfig {
                detach: true,
                persist: false,
                volume,
            },
            net,
            vm: vmcfg,
            verbose: false,
            repo: None,
            command: vec![],
        };
        if is_freebsd {
            boot_freebsd(args)
        } else {
            boot_netbsd(args)
        }
    } else {
        // Put the id back for any future attempt and report clearly.
        anyhow::bail!(
            "don't know how to restart a {:?} machine ({}); start it with `run` instead",
            vm.kind,
            vm.image
        );
    }
}

fn cmd_rm(ids: &[String], force: bool) -> Result<()> {
    let db = db::Db::open()?;
    for id in ids {
        let vm = db.find_machine(id)?;
        let running = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
        if running {
            if !force {
                anyhow::bail!("machine {} is running (stop it first, or use -f)", vm.id);
            }
            if let Some(pid) = vm.pid {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }
        }
        // Remove the per-machine state dir (console log, sockets, CoW disk clone),
        // then drop the DB row. Rename-aside + background GC so a huge read-only
        // nix rootfs doesn't make `rm` (and the desktop's delete) hang.
        host::remove_dir_all_detached(&std::path::PathBuf::from(&vm.state_dir));
        db.delete_machine(&vm.id)?;
        println!("{}", vm.id);
    }
    Ok(())
}

/// A default `LinuxArgs` for launching a flavor: image + resources + ports/env,
/// detached-with-no-command (persistent console) so the environment stays up.
#[allow(clippy::too_many_arguments)]
fn flavor_linux_args(
    image: String,
    detach: bool,
    cpus: u8,
    mem: u32,
    volume: Option<String>,
    ports: Vec<PortForward>,
    env: Vec<String>,
) -> LinuxArgs {
    LinuxArgs {
        image,
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach,
        initramfs: false,
        volume,
        mounts: vec![],
        entrypoint: None,
        env,
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig { cpus, mem },
        repo: None,
        command: vec![],
    }
}

/// Validate a flavor/snapshot name (used as a directory + DB key).
fn valid_flavor_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!("invalid flavor name {name:?} — use letters, digits, '-', '_' or '.'");
    }
    if flavors::find(name).is_some() {
        anyhow::bail!("{name:?} is a built-in catalog flavor name — pick another");
    }
    Ok(())
}

/// `bsdkrun commit <id> <name>` — snapshot a machine's current rootfs/disk into a
/// named flavor (a CoW clone), like `docker commit`. Boot it with `flavor run`.
/// Prepare a running guest for a consistent snapshot, via its agent.
///
/// - **Linux** (virtio-fs rootfs backed by host files): a `sync` flushes the
///   guest's page cache through to those host files — enough, since there's no
///   in-guest block filesystem to tear.
/// - **BSD** (a raw UFS disk): a mounted UFS is always flagged *dirty*, and a
///   running guest keeps writing, so cloning it yields a *torn* image that boots
///   into an `fsck` which discards recent changes — even a `sync` first isn't
///   enough (verified). The only reliable way to a clean, bootable image is a
///   **clean shutdown**, which unmounts UFS. So we power the guest off, wait for
///   it to exit, and snapshot the quiescent disk. The machine is left **stopped**
///   (start it again to resume).
///
/// Best-effort: a machine that's already stopped, or one without an agent, is
/// snapshotted as-is.
fn quiesce_guest_for_snapshot(vm: &db::MachineRow) {
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        return;
    }
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let Some(port) = agent::read_port(&vdir) else {
        tracing::warn!(
            id = %vm.id,
            "no guest agent to quiesce before snapshot — it may miss recent writes"
        );
        return;
    };

    if vm.kind == "linux" {
        let argv = ["sh".to_string(), "-c".to_string(), "sync; sync".to_string()];
        let _ = agent::exec(port, &argv, &[], false);
        info!(id = %vm.id, "flushed guest to disk before snapshot");
        return;
    }

    // BSD: clean poweroff so UFS is unmounted and the image is consistent.
    info!(id = %vm.id, "powering off guest for a consistent BSD snapshot…");
    let argv = [
        "sh".to_string(),
        "-c".to_string(),
        "sync; nohup shutdown -p now >/dev/null 2>&1 & sleep 1".to_string(),
    ];
    let _ = agent::exec(port, &argv, &[], false);

    // Wait for the VM process to exit (its clean unmount is then done).
    if let Some(pid) = vm.pid {
        for _ in 0..80 {
            if !db::pid_alive(pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
    if let Ok(db) = db::Db::open() {
        let _ = db.set_machine_status(&vm.id, "exited", Some(0));
    }
    info!(
        id = %vm.id,
        "guest powered off — snapshotting its clean disk (machine left stopped; `start` to resume)"
    );
}

/// Force a file's dirty pages all the way to permanent storage, so a subsequent
/// APFS `clonefile` (which shares on-disk *extents*) captures the latest writes.
/// libkrun's virtio-blk writes land in the host page cache; a plain `sync(2)`
/// only *schedules* the flush, so we use `F_FULLFSYNC`, which macOS guarantees
/// hits the disk. Opening a second read fd is enough — fsync flushes the whole
/// inode's dirty pages, including the ones libkrun wrote through its own fd.
#[cfg(target_os = "macos")]
fn fullsync_file(path: &std::path::Path) {
    use std::os::unix::io::AsRawFd;
    if let Ok(f) = std::fs::File::open(path) {
        // SAFETY: fcntl(F_FULLFSYNC) on a valid fd is always safe.
        unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn fullsync_file(path: &std::path::Path) {
    use std::os::unix::io::AsRawFd;
    if let Ok(f) = std::fs::File::open(path) {
        // SAFETY: fsync on a valid fd is always safe.
        unsafe {
            libc::fsync(f.as_raw_fd());
        }
    }
}

fn cmd_commit(id: &str, name: &str, description: &str) -> Result<()> {
    valid_flavor_name(name)?;
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let dir = db::flavors_dir()?.join(name);
    if dir.exists() || db.find_flavor(name)?.is_some() {
        anyhow::bail!("flavor {name:?} already exists (remove it first)");
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating flavor dir {}", dir.display()))?;

    // Make the guest's disk/rootfs consistent before cloning: flush a Linux
    // guest, or cleanly power off a BSD guest (a live UFS can't be cloned
    // consistently). See [`quiesce_guest_for_snapshot`].
    quiesce_guest_for_snapshot(&vm);

    let mdir = std::path::PathBuf::from(&vm.state_dir);
    if vm.kind == "linux" {
        // The machine's writable rootfs (per-machine clone, or its volume).
        let src = match &vm.volume {
            Some(v) => volume_dir(v)?.join("rootfs"),
            None => mdir.join("rootfs"),
        };
        if !src.exists() {
            let _ = std::fs::remove_dir_all(&dir);
            anyhow::bail!("machine {} has no rootfs to snapshot", vm.id);
        }
        // Flush the guest's just-synced writes (virtio-fs passes them through to
        // these host files) to disk before the extent-level clone.
        let _ = std::process::Command::new("sync").status();
        host::cow_copy(&src, &dir.join("rootfs"), true)?;
    } else {
        // BSD: snapshot the raw disk (`root.<ext>`), from the volume or state dir.
        let base = match &vm.volume {
            Some(v) => volume_dir(v)?,
            None => mdir.clone(),
        };
        let disk = ["root.raw", "root.img"]
            .iter()
            .map(|f| base.join(f))
            .find(|p| p.exists())
            .ok_or_else(|| {
                let _ = std::fs::remove_dir_all(&dir);
                anyhow::anyhow!("machine {} has no disk to snapshot", vm.id)
            })?;
        // Flush the guest's just-synced writes from the host page cache to disk
        // so `clonefile` (extent-level) captures them — without this the snapshot
        // silently misses recent changes even though the file *reads* current.
        fullsync_file(&disk);
        let ext = disk.extension().and_then(|e| e.to_str()).unwrap_or("img");
        host::cow_copy(&disk, &dir.join(format!("disk.{ext}")), false)?;
    }

    db.upsert_flavor(
        name,
        // Store the guest-OS label (freebsd/netbsd/linux), not the internal boot
        // mode (firmware/kernel), so the UI labels it right and `run` knows how
        // to boot it.
        guest_os_kind(&vm.kind, &vm.image),
        &vm.image,
        &dir.to_string_lossy(),
        description,
    )?;
    println!("{name}");
    Ok(())
}

/// `bsdkrun flavors` — list saved snapshots + the built-in catalog.
#[allow(clippy::print_literal)]
fn cmd_flavors(json: bool) -> Result<()> {
    let db = db::Db::open()?;
    let snapshots = db.list_flavors().unwrap_or_default();
    let user = flavors::user_flavors();
    if json {
        let mut out = Vec::new();
        for f in &snapshots {
            out.push(serde_json::json!({
                // Normalize legacy boot-mode kinds (firmware/kernel) for display.
                "name": f.name, "source": "snapshot", "kind": guest_os_kind(&f.kind, &f.base),
                "base": f.base, "category": "snapshot", "method": "snapshot",
                "description": f.description, "created_at": f.created_at,
            }));
        }
        for c in flavors::catalog() {
            out.push(serde_json::json!({
                "name": c.name, "source": "catalog", "kind": c.kind(),
                "base": c.image(), "category": c.category, "method": c.method(),
                "description": c.description, "ports": c.ports, "nix": c.nix,
            }));
        }
        for u in &user {
            out.push(serde_json::json!({
                "name": u.name, "source": "user", "kind": u.kind(),
                "base": u.base, "category": u.category, "method": u.method(),
                "description": u.description, "ports": u.ports, "nix": u.nix,
            }));
        }
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    if !snapshots.is_empty() {
        println!("Your snapshots:");
        for f in &snapshots {
            println!("  {:<18}  {:<9}  {}", f.name, f.kind, f.description);
        }
        println!();
    }
    if !user.is_empty() {
        println!("Your flavors (flavors.toml):");
        println!(
            "  {:<14}  {:<9}  {:<8}  {}",
            "NAME", "CATEGORY", "METHOD", "DESCRIPTION"
        );
        for u in &user {
            println!(
                "  {:<14}  {:<9}  {:<8}  {}",
                u.name,
                u.category,
                u.method(),
                u.description
            );
        }
        println!();
    }
    println!("Catalog:");
    println!(
        "  {:<14}  {:<9}  {:<8}  {}",
        "NAME", "CATEGORY", "METHOD", "DESCRIPTION"
    );
    for c in flavors::catalog() {
        println!(
            "  {:<14}  {:<9}  {:<8}  {}",
            c.name,
            c.category,
            c.method(),
            c.description
        );
    }
    Ok(())
}

/// `bsdkrun flavor rm <name>...` — remove saved snapshot flavors.
fn cmd_flavor_rm(names: &[String], _force: bool) -> Result<()> {
    let db = db::Db::open()?;
    let mut failed = false;
    for name in names {
        if flavors::find(name).is_some() {
            eprintln!("Error: {name:?} is a built-in catalog flavor (can't remove)");
            failed = true;
            continue;
        }
        match db.find_flavor(name)? {
            Some(f) => {
                // Rename-aside + background GC so a large (nix) snapshot rootfs
                // doesn't make the delete hang, same as `rm`.
                host::remove_dir_all_detached(&std::path::PathBuf::from(&f.path));
                db.remove_flavor(name).ok();
                println!("{name}");
            }
            None => {
                // Not a snapshot — try a user (flavors.toml) flavor.
                match flavors::remove_user_flavor(name) {
                    Ok(true) => println!("{name}"),
                    Ok(false) => {
                        eprintln!("Error: no such flavor: {name}");
                        failed = true;
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        failed = true;
                    }
                }
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// `bsdkrun flavor add <name> --base <ref> …` — define a custom flavor in the
/// writable `flavors.toml`.
fn cmd_flavor_add(a: FlavorAddArgs) -> Result<()> {
    let ok = !a.name.is_empty()
        && a.name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!(
            "invalid flavor name {:?} — use letters, digits, '-', '_' or '.'",
            a.name
        );
    }
    if a.base.trim().is_empty() {
        anyhow::bail!("--base is required (an OCI image ref, or `freebsd`/`netbsd`)");
    }
    let flavor = flavors::UserFlavor {
        name: a.name.clone(),
        category: if a.category.is_empty() {
            "custom".into()
        } else {
            a.category
        },
        description: a.description,
        base: a.base,
        ports: a.ports,
        env: a.env,
        nix: a.nix,
        provision: a.provision,
    };
    let path = flavors::upsert_user_flavor(flavor)?;
    info!(flavor = %a.name, file = %path.display(), "saved flavor");
    println!("{}", a.name);
    Ok(())
}

/// A resolved Linux flavor: the base image plus its defaults and provisioning
/// steps, from either the built-in catalog or a user `flavors.toml`.
struct LinuxFlavorSpec {
    image: String,
    env: Vec<String>,
    ports: Vec<String>,
    nix: Vec<String>,
    provision: Vec<String>,
}

/// Resolve a Linux flavor (catalog or user) by name. Returns `None` for a BSD
/// flavor or an unknown name.
fn resolve_linux_flavor(name: &str) -> Option<LinuxFlavorSpec> {
    if let Some(c) = flavors::find(name) {
        if c.kind() != "linux" {
            return None;
        }
        return Some(LinuxFlavorSpec {
            image: c.image().to_string(),
            env: c.env.iter().map(|s| s.to_string()).collect(),
            ports: c.ports.iter().map(|s| s.to_string()).collect(),
            nix: c.nix.iter().map(|s| s.to_string()).collect(),
            provision: c.provision.iter().map(|s| s.to_string()).collect(),
        });
    }
    let u = flavors::find_user(name)?;
    if u.kind() != "linux" {
        return None;
    }
    Some(LinuxFlavorSpec {
        image: u.base.clone(),
        env: u.env,
        ports: u.ports,
        nix: u.nix,
        provision: u.provision,
    })
}

/// A stable short cache key for a flavor build, derived from the base image ref
/// and the exact provisioning steps — like a Docker build cache keyed by its
/// instructions. Any change to the base or a step yields a new key (cache miss).
fn flavor_build_key(image: &str, nix: &[String], provision: &[String]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    image.hash(&mut h);
    0xFFu8.hash(&mut h);
    for n in nix {
        n.hash(&mut h);
        0x01u8.hash(&mut h);
    }
    0xFEu8.hash(&mut h);
    for p in provision {
        p.hash(&mut h);
        0x02u8.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// The reserved volume that holds a built flavor's provisioned rootfs (the
/// "cache layer"). Hidden from `volume ls`.
pub const FLAVOR_BUILD_PREFIX: &str = "bsdkrun-build-";
fn flavor_build_volume(key: &str) -> String {
    format!("{FLAVOR_BUILD_PREFIX}{key}")
}

/// A short filesystem-safe label for a flavor name (for guest `echo`s).
fn safe_label(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

/// Build the single guest command that provisions a flavor: Nix package installs
/// (via the Determinate Systems installer on the OCI base) then the flavor's
/// shell steps, in order, under a login shell. `None` ⇒ nothing to provision.
fn flavor_provision_argv(name: &str, nix: &[String], provision: &[String]) -> Option<Vec<String>> {
    if nix.is_empty() && provision.is_empty() {
        return None;
    }
    let label = safe_label(name);
    let mut lines: Vec<String> = vec![format!("echo '==> provisioning {label}'")];
    if !nix.is_empty() {
        lines.push("echo '==> installing Nix (Determinate Systems)'".into());
        lines.push(
            "command -v nix >/dev/null 2>&1 || curl --proto '=https' --tlsv1.2 -sSf -L \
             https://install.determinate.systems/nix | sh -s -- install linux \
             --no-confirm --init none"
                .into(),
        );
        lines.push(
            ". /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh 2>/dev/null || \
             export PATH=/nix/var/nix/profiles/default/bin:$PATH"
                .into(),
        );
        let pkgs = nix
            .iter()
            .map(|p| format!("nixpkgs#{p}"))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!("nix profile install {pkgs}"));
    }
    for step in provision {
        lines.push(step.clone());
    }
    lines.push(format!("echo '==> {label} ready'"));
    Some(vec!["sh".into(), "-lc".into(), lines.join("\n")])
}

/// Ensure a flavor's provisioned rootfs is built and cached, returning its path.
/// Cache hit → returns immediately (instant, no re-provisioning). Miss → runs
/// the provisioning build in a child `bsdkrun` process (streaming its progress)
/// and records the result so every later launch just clones it.
fn ensure_flavor_built(spec: &LinuxFlavorSpec, name: &str, cpus: u8, mem: u32) -> Result<PathBuf> {
    let key = flavor_build_key(&spec.image, &spec.nix, &spec.provision);
    let vol = flavor_build_volume(&key);
    let voldir = volume_dir(&vol)?;
    let rootfs = voldir.join("rootfs");
    let marker = voldir.join(".provisioned");
    if marker.exists() && rootfs.exists() {
        info!(flavor = name, key = %key, "using cached flavor build");
        return Ok(rootfs);
    }
    // Cache miss: build in a CHILD process. Provisioning ends in `process::exit`
    // (see `run_guest_command`), so it must not run in this process — we need to
    // survive it to clone + boot the real machine afterwards.
    info!(flavor = name, key = %key, "building flavor (first launch)…");
    host::force_remove_dir_all(&voldir); // clear any half-built remnant
    let exe = std::env::current_exe().context("locating bsdkrun for the flavor build")?;
    let status = std::process::Command::new(exe)
        .args([
            "flavor",
            "__build",
            name,
            "--key",
            &key,
            "--cpus",
            &cpus.to_string(),
            "--mem",
            &mem.to_string(),
        ])
        .status()
        .context("spawning the flavor build")?;
    if !status.success() {
        host::force_remove_dir_all(&voldir);
        anyhow::bail!("provisioning {name} failed (see the output above)");
    }
    if !rootfs.exists() {
        host::force_remove_dir_all(&voldir);
        anyhow::bail!("the flavor build produced no rootfs for {name}");
    }
    std::fs::write(&marker, key.as_bytes()).ok();
    Ok(rootfs)
}

/// Hidden `bsdkrun flavor __build` — the child that provisions a flavor into its
/// build volume, then powers the builder off. Not for direct use.
fn cmd_flavor_build(name: &str, key: &str, cpus: u8, mem: u32) -> Result<()> {
    let spec = resolve_linux_flavor(name)
        .ok_or_else(|| anyhow::anyhow!("no such Linux flavor to build: {name}"))?;
    let argv = flavor_provision_argv(name, &spec.nix, &spec.provision)
        .ok_or_else(|| anyhow::anyhow!("{name} has nothing to provision"))?;
    let vol = flavor_build_volume(key);

    // Boot a builder whose root is the persistent build volume. A trivial
    // keep-alive is the main process so the VM (and its agent) stay up on any
    // base image while provisioning runs; `run_machine` powers it off when the
    // provisioning command finishes (detach=false ⇒ keep_running=false).
    let largs = LinuxArgs {
        image: spec.image.clone(),
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach: false,
        initramfs: false,
        volume: Some(vol),
        mounts: vec![],
        entrypoint: None,
        env: spec.env.clone(),
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports: vec![],
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig { cpus, mem },
        repo: None,
        command: vec![
            "sh".into(),
            "-c".into(),
            "while :; do sleep 86400; done".into(),
        ],
    };
    boot_linux_from(largs, None, &argv)
}

/// `bsdkrun flavor build <name>` — pre-build a flavor's provisioned rootfs into
/// the cache so a later `run` is instant. Streams provisioning output.
fn cmd_flavor_prebuild(name: &str, cpus: u8, mem: u32, force: bool) -> Result<()> {
    let Some(spec) = resolve_linux_flavor(name) else {
        anyhow::bail!("no such Linux flavor to build: {name} (see `bsdkrun flavors`)");
    };
    if spec.nix.is_empty() && spec.provision.is_empty() {
        println!("{name}: nothing to build (no provisioning steps)");
        return Ok(());
    }
    if force {
        // Drop the cached build so it's rebuilt from scratch.
        let key = flavor_build_key(&spec.image, &spec.nix, &spec.provision);
        if let Ok(dir) = volume_dir(&flavor_build_volume(&key)) {
            host::force_remove_dir_all(&dir);
        }
    }
    let built = ensure_flavor_built(&spec, name, cpus, mem)?;
    info!(flavor = name, rootfs = %built.display(), "flavor built");
    println!("{name}");
    Ok(())
}

/// `bsdkrun flavor run <name>` — boot a machine from a catalog/user flavor or a
/// saved snapshot. Provisioned flavors are built once (cached) then cloned.
fn cmd_flavor_run(args: FlavorRunArgs) -> Result<()> {
    let db = db::Db::open()?;

    // Optional `--repo` clones a repo into the machine after boot (cd on shell).
    let repo_argv = args
        .repo
        .as_deref()
        .and_then(repo_clone_argv)
        .unwrap_or_default();

    // A saved snapshot (from `commit`) wins over any catalog/user name.
    if let Some(f) = db.find_flavor(&args.name)? {
        // Normalize (old snapshots stored the boot mode: firmware/kernel).
        let osk = guest_os_kind(&f.kind, &f.base);
        if osk == "linux" {
            let rootfs = std::path::PathBuf::from(&f.path).join("rootfs");
            if !rootfs.exists() {
                anyhow::bail!("snapshot {:?} is missing its rootfs data", f.name);
            }
            let largs = flavor_linux_args(
                f.base.clone(),
                args.detach,
                args.vm.cpus,
                args.vm.mem,
                args.volume,
                args.ports,
                vec![],
            );
            return boot_linux_from(largs, Some(rootfs), &repo_argv);
        }

        // BSD snapshot: boot from its saved root disk (`disk.raw` / `disk.img`).
        let disk = ["disk.raw", "disk.img"]
            .iter()
            .map(|n| std::path::PathBuf::from(&f.path).join(n))
            .find(|p| p.exists())
            .ok_or_else(|| anyhow::anyhow!("snapshot {:?} is missing its disk data", f.name))?;
        let bargs = BsdArgs {
            version: None,
            firmware: None,
            force: false,
            attach_disk: vec![],
            disk_size: None,
            run: RunConfig {
                detach: args.detach,
                persist: false,
                volume: args.volume,
            },
            net: NetConfig {
                no_net: false,
                ports: args.ports,
                mac: None,
                network: None,
                name: None,
            },
            vm: VmConfig {
                cpus: args.vm.cpus,
                mem: args.vm.mem,
            },
            verbose: false,
            repo: None,
            command: repo_argv,
        };
        return if osk == "netbsd" {
            boot_netbsd_disk(bargs, Some(disk))
        } else {
            boot_freebsd_disk(bargs, Some(disk))
        };
    }

    // A Linux flavor (catalog or user): build-once-then-clone.
    if let Some(spec) = resolve_linux_flavor(&args.name) {
        let mut ports: Vec<PortForward> = args.ports;
        for p in &spec.ports {
            if let Ok(pf) = p.parse::<PortForward>() {
                ports.push(pf);
            }
        }
        let largs = flavor_linux_args(
            spec.image.clone(),
            args.detach,
            args.vm.cpus,
            args.vm.mem,
            args.volume,
            ports,
            spec.env.clone(),
        );
        // Provisioned flavors boot from a cached, pre-provisioned rootfs; plain
        // ones boot the base image directly.
        let has_provisioning = !spec.nix.is_empty() || !spec.provision.is_empty();
        if has_provisioning {
            let built = ensure_flavor_built(&spec, &args.name, args.vm.cpus, args.vm.mem)?;
            return boot_linux_from(largs, Some(built), &repo_argv);
        }
        return boot_linux_from(largs, None, &repo_argv);
    }

    // A BSD catalog flavor (no provisioning/cache — boots the bundled image).
    let Some(c) = flavors::find(&args.name) else {
        anyhow::bail!("no such flavor: {} (see `bsdkrun flavors`)", args.name);
    };
    let mut ports: Vec<PortForward> = args.ports;
    for p in c.ports {
        if let Ok(pf) = p.parse::<PortForward>() {
            ports.push(pf);
        }
    }
    let bargs = BsdArgs {
        version: None,
        firmware: None,
        force: false,
        attach_disk: vec![],
        disk_size: None,
        run: RunConfig {
            detach: args.detach,
            persist: false,
            volume: args.volume,
        },
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig {
            cpus: args.vm.cpus,
            mem: args.vm.mem,
        },
        verbose: false,
        repo: None,
        // On BSD the post-boot command IS the repo clone (if any).
        command: repo_argv,
    };
    match c.base {
        flavors::Base::Freebsd => boot_freebsd(bargs),
        flavors::Base::Netbsd => boot_netbsd(bargs),
        flavors::Base::Oci(_) => unreachable!("OCI flavors handled above"),
    }
}

fn cmd_logs(id: &str, follow: bool, boot: bool) -> Result<()> {
    use std::io::Write;
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let console_log = vdir.join("console.log");
    let boot_log = vdir.join("bsdkrun.log");

    // `--boot`: bsdkrun/libkrun's own log (fd 2 of the detached child) — the boot
    // diagnostics and any error that killed the machine before it reached console.
    if boot {
        match std::fs::read(&boot_log) {
            Ok(data) => {
                std::io::stdout().write_all(&data).ok();
                std::io::stdout().flush().ok();
                return Ok(());
            }
            Err(_) => anyhow::bail!(
                "no boot log for {} (only detached machines, run with -d, have one)",
                vm.id
            ),
        }
    }

    let console_data = std::fs::read(&console_log).unwrap_or_default();
    if !console_data.is_empty() {
        std::io::stdout().write_all(&console_data).ok();
        std::io::stdout().flush().ok();
    } else if !follow {
        // No guest console output — the machine may have died during boot. Fall
        // back to the boot log so the failure is actually visible (this is what
        // bit NetBSD-under-libkrun: an empty console but a real error in the log).
        if let Ok(boot_data) = std::fs::read(&boot_log) {
            if !boot_data.is_empty() {
                eprintln!(
                    "── no guest console output; showing boot log ({}) ──",
                    boot_log.display()
                );
                std::io::stdout().write_all(&boot_data).ok();
                std::io::stdout().flush().ok();
                return Ok(());
            }
        }
        anyhow::bail!(
            "no console log for {} (only detached machines, run with -d, have one)",
            vm.id
        );
    }
    if follow {
        console::follow(&vdir)?;
    }
    Ok(())
}

fn cmd_shell(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        anyhow::bail!("machine {} is not running", vm.id);
    }
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    // Prefer the guest agent (a fresh interactive shell over TCP). Fall back to
    // the persistent-console attach for machines booted without an agent port.
    if let Some(port) = agent::read_port(&vdir) {
        let argv = interactive_shell_argv();
        let env = interactive_shell_env(&vm.kind);
        let code = agent::exec(port, &argv, &env, true).map_err(|e| agent_error(&vm.kind, e))?;
        std::process::exit(code);
    }
    if !vm.detached {
        anyhow::bail!("`shell` attaches to a detached machine — start it with `-d`");
    }
    console::attach_interactive(&vdir)
}

/// argv that opens an interactive shell inside a guest, preferring `bash` when
/// it's on the guest's PATH and falling back to `/bin/sh` otherwise. The choice
/// is made in the guest (only it knows its PATH) via a `/bin/sh -c` wrapper that
/// `exec`s the winner so it inherits the agent's PTY. `/bin/sh` exists on
/// Alpine/busybox Linux images and on FreeBSD/NetBSD.
/// Shell snippet: on NetBSD, set `PKG_PATH` to the pkgsrc binary-package CDN for
/// this release so `pkg_add` (and thus `pkg_add pkgin`) works — an unset
/// `PKG_PATH` is exactly why `pkg_add pkgin` fails on a fresh guest. A `-current`
/// release (e.g. `11.99.7`) has no packages of its own, so use the matching
/// `<major>.0` stable branch (`11.0`), which pkg_add installs with a harmless
/// "different platform" warning. `x86_64` maps to the pkgsrc port `amd64`.
const NETBSD_PKG_PATH_SETUP: &str = "if [ -z \"$PKG_PATH\" ] && [ \"$(uname 2>/dev/null)\" = NetBSD ]; then __a=$(uname -p 2>/dev/null); [ \"$__a\" = x86_64 ] && __a=amd64; export PKG_PATH=\"https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/$__a/$(uname -r 2>/dev/null | cut -d. -f1).0/All/\"; fi;";

fn interactive_shell_argv() -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        // Add /usr/local/bin (FreeBSD `pkg`) and /usr/pkg/bin (NetBSD pkgsrc) to
        // PATH; on NetBSD, point `pkg_add` at the pkgsrc CDN for this release
        // (`-current` like 11.99.x uses the matching `<major>.0` branch) so
        // `pkg_add pkgin` works; `cd` into a cloned repo; then hand off to a shell.
        format!(
            "export PATH=\"/usr/local/bin:/usr/local/sbin:/usr/pkg/bin:/usr/pkg/sbin:$PATH\"; \
             [ \"$(uname 2>/dev/null)\" = Linux ] || export TERM=xterm; \
             {NETBSD_PKG_PATH_SETUP} \
             cd \"$(cat /etc/bsdkrun-cwd 2>/dev/null)\" 2>/dev/null; \
             if command -v bash >/dev/null 2>&1; then exec bash; else exec /bin/sh; fi"
        ),
    ]
}

/// Extra environment for an interactive shell over the agent. FreeBSD/NetBSD
/// guests boot with no `TERM` set on the agent PTY, which leaves line editing
/// and full-screen tools broken; `xterm` is in both guests' terminfo and gives
/// color plus the usual key sequences. Linux images set their own `TERM`, so
/// leave them alone.
fn interactive_shell_env(kind: &str) -> Vec<String> {
    if is_bsd(kind) {
        vec!["TERM=xterm".to_string()]
    } else {
        Vec::new()
    }
}

/// Whether a machine is a non-Linux guest (where we can't auto-inject the agent
/// — the user installs and starts `bsdkrun-agent` in the guest themselves).
fn is_bsd(kind: &str) -> bool {
    kind != "linux"
}

/// Normalize a machine's internal boot-mode kind (`firmware`/`kernel`/`linux`) to
/// a guest-OS label (`freebsd`/`netbsd`/`linux`) — for display and for deciding
/// how to boot a committed snapshot.
fn guest_os_kind(kind: &str, image: &str) -> &'static str {
    let img = image.to_ascii_lowercase();
    if kind == "freebsd" || kind == "firmware" || img.starts_with("freebsd") {
        "freebsd"
    } else if kind == "netbsd" || kind == "kernel" || img.starts_with("netbsd") {
        "netbsd"
    } else {
        "linux"
    }
}

/// Add a guest-specific hint to an agent connection/exec failure.
fn agent_error(kind: &str, e: anyhow::Error) -> anyhow::Error {
    if is_bsd(kind) {
        // Guest arch == host arch under KVM/HVF.
        let arch = host::Arch::current().unwrap_or(host::Arch::Aarch64);
        anyhow::anyhow!(
            "{e}\n\nBSD guests don't run the exec agent automatically. Download the agent for \
             your guest from the bsdkrun GitHub release:\n  \
             FreeBSD: {}\n  \
             NetBSD:  {}\n\
             then copy it into the running microVM and start it (it listens on TCP port {}): \
             `./bsdkrun-agent &`. bsdkrun forwards a host port to it automatically.",
            agent::asset_url(host::GuestOs::Freebsd, arch),
            agent::asset_url(host::GuestOs::Netbsd, arch),
            agent::GUEST_PORT,
        )
    } else {
        e
    }
}

/// Resolve a running machine and its exec-agent port (shared by exec/tailscale).
fn agent_target(id: &str) -> Result<(db::MachineRow, u16)> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        anyhow::bail!("machine {} is not running", vm.id);
    }
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let port = agent::read_port(&vdir).ok_or_else(|| {
        anyhow::anyhow!(
            "machine {} has no exec agent port — it was booted with networking disabled \
             (--no-net), which the agent needs",
            vm.id
        )
    })?;
    Ok((vm, port))
}

/// Run a command inside a running machine via its guest agent.
fn cmd_exec(id: &str, command: &[String], env: &[String], tty: bool) -> Result<()> {
    let (vm, port) = agent_target(id)?;
    let code = agent::exec(port, command, env, tty).map_err(|e| agent_error(&vm.kind, e))?;
    std::process::exit(code);
}

/// Refresh the in-guest agent binary to the current release. Some bundled BSD
/// images bake in an agent that predates the `ssh`/`tailscale` subcommands, so
/// invoking those makes the old agent try to `listen` again (Address already in
/// use). The running daemon can still `exec`, so we have it download the current
/// binary (via the guest's own fetch tool) and replace the file in place — the
/// next ssh/tailscale/exec then spawns the fresh binary. No restart needed.
fn cmd_agent_update(id: &str) -> Result<()> {
    let (vm, port) = agent_target(id)?;
    let arch = host::Arch::current()?;
    let reference = vm.image.to_lowercase();

    // (guest OS, install path, download command incl. its output flag).
    let (os, dest, fetch_cmd) = if vm.kind == "linux" {
        (host::GuestOs::Linux, "/sbin/bsdkrun-agent", "curl -fL -o")
    } else if vm.kind == "kernel" || reference.starts_with("netbsd") {
        (
            host::GuestOs::Netbsd,
            "/usr/local/sbin/bsdkrun-agent",
            "ftp -o",
        )
    } else {
        (
            host::GuestOs::Freebsd,
            "/usr/local/sbin/bsdkrun-agent",
            "fetch -o",
        )
    };
    let url = agent::asset_url(os, arch);

    // Download to a temp file, chmod, then atomically move over the old binary.
    let script = format!(
        "set -e; tmp=/tmp/bsdkrun-agent.new; {fetch_cmd} \"$tmp\" '{url}'; \
         chmod +x \"$tmp\"; mv \"$tmp\" '{dest}'; \
         echo \"updated {dest}\"; echo \"from {url}\"",
    );
    eprintln!("updating agent in {} from {url}", vm.id);
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];
    let code = agent::exec(port, &argv, &[], false).map_err(|e| agent_error(&vm.kind, e))?;
    std::process::exit(code);
}

/// Run one of the agent's built-in CLI families (`tailscale`, `ssh`) inside
/// the guest, trying the known agent locations (Linux OCI guests carry it at
/// /sbin, the bundled BSD images at /usr/local/sbin). Exit code 127 means
/// "couldn't spawn" — i.e. wrong path — so try the next candidate.
fn run_agent_cli(id: &str, family: &str, args: &[String], env: &[String]) -> Result<()> {
    let (vm, port) = agent_target(id)?;

    let candidates: &[&str] = if vm.kind == "linux" {
        &["/sbin/bsdkrun-agent", "/usr/local/sbin/bsdkrun-agent"]
    } else {
        &["/usr/local/sbin/bsdkrun-agent", "/sbin/bsdkrun-agent"]
    };
    let mut code = 127;
    for cand in candidates {
        let mut argv: Vec<String> = vec![cand.to_string(), family.to_string()];
        argv.extend(args.iter().cloned());
        code = agent::exec(port, &argv, env, false).map_err(|e| agent_error(&vm.kind, e))?;
        if code != 127 {
            break;
        }
    }
    if code == 127 {
        anyhow::bail!(
            "bsdkrun-agent binary not found inside the guest (tried {}) — the image \
             predates the {family}-capable agent; rebuild/refetch it",
            candidates.join(", ")
        );
    }
    std::process::exit(code);
}

/// `bsdkrun tailscale <id> <action...>` — see the agent's tailscale module.
fn cmd_tailscale(id: &str, args: &[String]) -> Result<()> {
    // Forward the host's TS_AUTHKEY so `bsdkrun tailscale <id> setup` works
    // without pasting the key on the command line (shell history, ps, ...).
    let mut env: Vec<String> = Vec::new();
    if let Ok(k) = std::env::var("TS_AUTHKEY") {
        if !k.is_empty() {
            env.push(format!("TS_AUTHKEY={k}"));
        }
    }
    run_agent_cli(id, "tailscale", args, &env)
}

/// `bsdkrun ssh <id> <action...>` — key-based SSH via the agent's ssh module.
///
/// Host-side sugar on top of the raw agent CLI:
/// - a `--key` value that names a local file is replaced by its contents, so
///   `--key ~/.ssh/id_ed25519.pub` Just Works;
/// - when `setup`/`add-key` is called with no `--key` at all, the local
///   `~/.ssh/id_*.pub` keys are collected and forwarded via $BSDKRUN_SSH_KEYS
///   — the one-liner path: `bsdkrun ssh <id> setup`.
fn cmd_ssh(id: &str, args: &[String]) -> Result<()> {
    let mut out_args: Vec<String> = Vec::with_capacity(args.len());
    let mut have_key = false;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if a == "--key" {
            have_key = true;
            out_args.push(a.clone());
            if let Some(v) = it.next() {
                let p = std::path::Path::new(v);
                if p.is_file() {
                    let k = std::fs::read_to_string(p)
                        .with_context(|| format!("reading key file {v}"))?;
                    out_args.push(k.trim().to_string());
                } else {
                    out_args.push(v.clone());
                }
            }
        } else {
            out_args.push(a.clone());
        }
    }

    let mut env: Vec<String> = Vec::new();
    let action = args.first().map(String::as_str);
    if !have_key && matches!(action, Some("setup") | Some("add-key")) {
        let keys = local_public_keys();
        if !keys.is_empty() {
            println!(
                "using {} local public key(s) from ~/.ssh (pass --key to override)",
                keys.len()
            );
            env.push(format!("BSDKRUN_SSH_KEYS={}", keys.join("\n")));
        }
    }
    run_agent_cli(id, "ssh", &out_args, &env)
}

/// The user's `~/.ssh/id_*.pub` keys, one per line, comments and all.
fn local_public_keys() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let dir = std::path::Path::new(&home).join(".ssh");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("id_") && name.ends_with(".pub") {
            if let Ok(s) = std::fs::read_to_string(e.path()) {
                let s = s.trim();
                if !s.is_empty() {
                    keys.push(s.to_string());
                }
            }
        }
    }
    keys.sort();
    keys
}
