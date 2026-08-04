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
mod host;
mod id;
mod krun;
mod linux;
mod names;
mod net;
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

    /// Guest console device the kernel should log to. libkrun's native console
    /// is the virtio-console `hvc0`; use `ttyS0` only with a kernel/setup that
    /// expects libkrun's explicit 8250 serial instead.
    #[arg(long, default_value = "hvc0")]
    console: String,

    #[command(flatten)]
    net: NetConfig,

    #[command(flatten)]
    vm: VmConfig,

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
    run_machine(
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
    )
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
fn boot_freebsd(args: BsdArgs) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        boot_freebsd_efi(args)
    }
    #[cfg(target_os = "linux")]
    {
        boot_freebsd_pvh(args)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = args;
        anyhow::bail!("bsdkrun freebsd is only supported on macOS and Linux");
    }
}

/// macOS EFI-firmware boot: FreeBSD's `loader.efi` takes over from the ESP.
#[cfg(target_os = "macos")]
fn boot_freebsd_efi(args: BsdArgs) -> Result<()> {
    // No explicit command on a foreground boot → drop into an interactive shell.
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    // Default (no --version) to bsdkrun's bundled arm64 image, which has the guest
    // agent injected so `exec` works out of the box. An explicit --version (or a
    // non-arm64 host) fetches the official FreeBSD VM image from download.freebsd.org.
    let disk = match (host::Arch::current()?, &args.version) {
        (host::Arch::Aarch64, None) => fetch::fetch_freebsd_arm64_image(args.force)?,
        _ => {
            let cache = fetch::cache_dir()?;
            fetch::fetch(fetch::Os::Freebsd, args.version.clone(), &cache, args.force)?
        }
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
fn boot_freebsd_pvh(args: BsdArgs) -> Result<()> {
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
    std::env::set_var("KRUN_PVH", "1");
    std::env::set_var("KRUN_VIRTIO_MMIO_HINTS", "freebsd");

    let disk = fetch::fetch_freebsd_amd64_image(args.force)?;
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
    run_machine(
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
    )
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
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    let arch = host::Arch::current()?;

    // amd64 NetBSD is a PVH kernel (MICROVM). Tell libkrun to enter via the
    // PHYS32_ENTRY note instead of the Linux boot protocol. Harmless on a libkrun
    // without PVH support (the flag is simply ignored).
    if matches!(arch, host::Arch::X86_64) {
        std::env::set_var("KRUN_PVH", "1");
    }

    let (disk, kernel) = match arch {
        host::Arch::X86_64 => (
            fetch::fetch_netbsd_amd64_image(args.force)?,
            fetch::fetch_netbsd_amd64_kernel(args.force)?,
        ),
        host::Arch::Aarch64 => (
            fetch::fetch_netbsd_arm64_image(args.force)?,
            fetch::fetch_netbsd_kernel(args.version.clone(), args.force)?,
        ),
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
    run_machine(
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
    )
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

fn boot_linux(args: LinuxArgs) -> Result<()> {
    // Prepare everything that can fail before we fork / touch the hypervisor.
    let kernel = linux::ensure_kernel(args.kernel.clone(), &args.kernel_version)?;
    let image = oci::pull(&args.image)?;
    let ep = linux::resolve_entrypoint(&image.config, args.entrypoint.as_deref(), &args.command);
    info!(argv = ?ep.argv, "resolved entrypoint");

    // Persist the image (best-effort; DB problems never block booting).
    db::record_image(
        &args.image,
        &image.digest,
        image.size,
        &image.rootfs.to_string_lossy(),
    );

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
            &image.rootfs,
            &ep,
            net_up,
            persistent,
            voldir,
            &mounts,
        )?),
        (None, true) => LinuxRoot::Virtiofs(linux::prepare_virtiofs_root(
            &image.rootfs,
            &ep,
            net_up,
            persistent,
            &vdir,
            &mounts,
        )?),
        (None, false) => {
            linux::warn_initramfs_memory(&image.rootfs, args.vm.mem);
            LinuxRoot::Initramfs(linux::build_initramfs(
                &image.rootfs,
                &ep,
                net_up,
                persistent,
                &mounts,
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
    run_machine(
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
        &[],
        false,
        false,
        build,
    )
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
    let rows = db.list_volumes()?;
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
                if e.path().is_dir() && !tracked.contains(&name) {
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
            if e.path().is_dir() && !tracked.contains(&name) {
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
            console: "hvc0".to_string(),
            net,
            vm: vmcfg,
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
        // then drop the DB row. force_remove handles a read-only nix rootfs.
        host::force_remove_dir_all(&std::path::PathBuf::from(&vm.state_dir));
        db.delete_machine(&vm.id)?;
        println!("{}", vm.id);
    }
    Ok(())
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
fn interactive_shell_argv() -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "if command -v bash >/dev/null 2>&1; then exec bash; else exec /bin/sh; fi".to_string(),
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
