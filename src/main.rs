//! bsdkrun — a Firecracker-style microVM launcher for BSD guests on macOS,
//! built on libkrun (Hypervisor.framework).
//!
//! Two boot modes:
//!   * kernel   — direct kernel + cmdline, using libkrun's generated FDT
//!                (target: NetBSD evbarm / bare kernel+FDT boot)
//!   * firmware — a UEFI firmware image that boots a normal BSD disk via its
//!                EFI loader (target: FreeBSD / OpenBSD arm64)

mod elf;
mod fetch;
mod krun;
mod linux;
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

    /// Boot a microVM from a direct kernel image + optional root disk.
    Kernel(KernelArgs),

    /// Boot a microVM from a UEFI firmware image + root disk.
    Firmware(FirmwareArgs),

    /// Download a BSD arm64 image and prepare it for booting.
    Fetch(FetchArgs),

    /// List the arm64 builds available to fetch.
    Versions(VersionsArgs),

    /// Grow a disk image (the guest expands its root FS on next boot).
    Grow(GrowArgs),

    /// Run an OCI image (Docker Hub / any registry) as a Linux microVM.
    Linux(LinuxArgs),
}

#[derive(Parser)]
struct LinuxArgs {
    /// OCI image reference, e.g. `alpine`, `alpine:3.20`, `ghcr.io/owner/name:tag`.
    #[arg(value_name = "IMAGE")]
    image: String,

    /// Kernel to boot (default: a prebuilt aarch64 vmlinux, downloaded + cached).
    #[arg(long)]
    kernel: Option<PathBuf>,

    /// Share the extracted rootfs via virtio-fs instead of packing an initramfs.
    /// Requires a guest kernel built with CONFIG_FUSE_FS=y / virtio-fs.
    #[arg(long)]
    virtiofs: bool,

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
    #[arg(long)]
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
    net: NetConfig,

    #[command(flatten)]
    vm: VmConfig,
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

/// Shared boot tail: enter the guest, restore the terminal on the way out
/// (success or error), then tear down networking and exit with the guest's code.
/// Never returns normally.
fn finish(ctx: Ctx, gvproxy: Option<Gvproxy>) -> Result<()> {
    let result = ctx.start_enter();
    tty::restore_stdin_termios();
    let code = result.context("starting microVM")?;
    info!(code, "guest exited");
    drop(gvproxy);
    std::process::exit(code);
}

/// Bring up user-mode networking (on by default). Returns the live gvproxy
/// handle, which must outlive the VM (kept in scope until after `start_enter`).
///
/// If gvproxy isn't installed we degrade gracefully — the guest boots without a
/// NIC and we warn — *unless* the user explicitly asked for port forwards, in
/// which case the missing dependency is a hard error.
fn setup_networking(ctx: &Ctx, cfg: &NetConfig) -> Result<Option<Gvproxy>> {
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
    let gvproxy = Gvproxy::spawn(&cfg.ports).context("starting gvproxy networking")?;
    ctx.add_net_gvproxy(&gvproxy.vfkit_socket, mac)
        .context("attaching virtio-net device")?;
    Ok(Some(gvproxy))
}

fn boot_kernel(args: KernelArgs) -> Result<()> {
    // Snapshot the terminal + arm signal cleanup before libkrun raws the TTY.
    tty::install();
    // Catch libkrun's HVF panic-hang on SMP shutdown and turn it into a clean
    // exit (see the watchdog module).
    watchdog::install();

    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    ctx.attach_stdio_serial_console()
        .context("wiring guest serial console to stdio")?;

    if let Some(disk) = &args.disk {
        ctx.add_disk("root", disk, false)
            .with_context(|| format!("attaching root disk {}", disk.display()))?;
    }
    attach_extra_disks(&ctx, &args.attach_disk)?;
    let gvproxy = setup_networking(&ctx, &args.net)?;

    ctx.set_kernel(
        &args.kernel,
        args.format.to_krun(),
        args.initramfs.as_deref(),
        &args.cmdline,
    )
    .context("configuring kernel")?;

    info!(
        cpus = args.vm.cpus,
        mem_mib = args.vm.mem,
        kernel = %args.kernel.display(),
        "booting microVM"
    );
    finish(ctx, gvproxy)
}

fn boot_firmware(args: FirmwareArgs) -> Result<()> {
    // Snapshot the terminal + arm signal cleanup before libkrun raws the TTY.
    tty::install();
    // Catch libkrun's HVF panic-hang on SMP shutdown and turn it into a clean
    // exit (see the watchdog module).
    watchdog::install();

    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    ctx.attach_stdio_serial_console()
        .context("wiring guest serial console to stdio")?;

    ctx.add_disk("root", &args.disk, false)
        .with_context(|| format!("attaching root disk {}", args.disk.display()))?;
    attach_extra_disks(&ctx, &args.attach_disk)?;
    let gvproxy = setup_networking(&ctx, &args.net)?;

    ctx.set_firmware(&args.firmware)
        .context("configuring firmware")?;

    info!(
        cpus = args.vm.cpus,
        mem_mib = args.vm.mem,
        firmware = %args.firmware.display(),
        disk = %args.disk.display(),
        "booting microVM"
    );
    finish(ctx, gvproxy)
}

fn boot_linux(args: LinuxArgs) -> Result<()> {
    // Prepare everything that can fail before we touch the terminal/hypervisor.
    let kernel = linux::ensure_kernel(args.kernel.clone())?;
    let image = oci::pull(&args.image)?;
    let ep = linux::resolve_entrypoint(&image.config, args.entrypoint.as_deref(), &args.command);
    info!(argv = ?ep.argv, "resolved entrypoint");

    // Snapshot the terminal + arm signal cleanup before libkrun raws the TTY.
    // NOTE: we deliberately do *not* install the stderr-tee watchdog here — it
    // redirects fd 2, which libkrun's implicit virtio-console (hvc0) claims for
    // the guest, breaking the console. The watchdog exists for the BSD SMP-
    // shutdown PSCI panic; Linux guests power off cleanly and don't need it.
    tty::install();

    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    // For a `ttyS*` console we wire libkrun's explicit 8250 serial (as the BSD
    // paths do); for `hvc*` we leave libkrun's implicit virtio-console in place
    // (that's the console libkrun natively gives Linux container guests).
    if !args.console.starts_with("hvc") {
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
    }

    let gvproxy = setup_networking(&ctx, &args.net)?;
    // Networking is only actually up if gvproxy started (graceful-degrade aware),
    // so the kernel `ip=`/DNS config is only added when there's really a NIC.
    let net_up = gvproxy.is_some();

    if args.virtiofs {
        // Directory rootfs over virtio-fs; libkrun's init runs the entrypoint.
        ctx.set_root(&image.rootfs)
            .context("configuring virtio-fs root (needs a CONFIG_FUSE_FS=y kernel)")?;
        if !ep.workdir.is_empty() {
            ctx.set_workdir(&ep.workdir)?;
        }
        let (exec, argv) = (ep.argv[0].clone(), ep.argv.clone());
        ctx.set_exec(&exec, &argv, &ep.env)?;
        ctx.set_kernel(
            &kernel,
            krun::KRUN_KERNEL_FORMAT_RAW,
            None,
            &format!("console={}", args.console),
        )
        .context("configuring kernel")?;
    } else {
        // Initramfs: pack the rootfs + generated /init and boot it from RAM.
        linux::warn_initramfs_memory(&image.rootfs, args.vm.mem);
        let initramfs = linux::build_initramfs(&image.rootfs, &ep, net_up)?;
        let cmdline = linux::kernel_cmdline(&args.console, net_up);
        ctx.set_kernel(
            &kernel,
            krun::KRUN_KERNEL_FORMAT_RAW,
            Some(&initramfs),
            &cmdline,
        )
        .context("configuring kernel")?;
    }

    info!(
        cpus = args.vm.cpus,
        mem_mib = args.vm.mem,
        image = %args.image,
        mode = if args.virtiofs { "virtiofs" } else { "initramfs" },
        "booting Linux microVM"
    );
    finish(ctx, gvproxy)
}
