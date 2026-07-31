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

mod console;
mod db;
mod elf;
mod fetch;
mod id;
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

    /// List machines.
    Ps(PsArgs),

    /// List downloaded images.
    Images,

    /// Stop a running machine.
    Stop(IdArgs),

    /// Show a machine's console log.
    Logs(LogsArgs),

    /// Attach an interactive shell to a running (detached) machine.
    Shell(IdArgs),
}

#[derive(Parser)]
struct PsArgs {
    /// Show all machines (default shows only running ones).
    #[arg(short, long)]
    all: bool,
}

#[derive(Parser)]
struct IdArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,
}

#[derive(Parser)]
struct LogsArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    id: String,

    /// Follow the live console output.
    #[arg(short, long)]
    follow: bool,
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
        Command::Ps(args) => cmd_ps(args.all),
        Command::Images => cmd_images(),
        Command::Stop(args) => cmd_stop(&args.id),
        Command::Logs(args) => cmd_logs(&args.id, args.follow),
        Command::Shell(args) => cmd_shell(&args.id),
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

/// Shared boot tail: enter the guest, restore the terminal on the way out
/// (success or error), then tear down networking and exit with the guest's code.
/// Never returns normally.
fn finish(ctx: Ctx, gvproxy: Option<Gvproxy>) -> Result<()> {
    let result = ctx.start_enter();
    tty::restore_stdin_termios();
    let code = result.context("starting machine")?;
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
        db::record_disk(&disk.to_string_lossy());
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
        "booting machine"
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

    db::record_disk(&args.disk.to_string_lossy());
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
        "booting machine"
    );
    finish(ctx, gvproxy)
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

    let machine_id = id::short_id();
    let vdir =
        db::machine_dir(&machine_id).unwrap_or_else(|_| std::env::temp_dir().join(&machine_id));
    std::fs::create_dir_all(&vdir).ok();
    let command = ep.argv.join(" ");

    // Decide up front whether networking will come up, so the baked-in initramfs
    // `/init` + kernel `ip=` match what `setup_networking` will actually do.
    let net_up = !args.net.no_net && net::locate().is_ok();

    // A detached machine with no explicit command keeps a console shell alive
    // across exits (so `shell` behaves like a persistent VM you attach to);
    // otherwise the workload runs once and the machine powers off when it ends.
    let persistent = args.detach && args.command.is_empty();

    // Prepare the rootfs before forking so failures surface synchronously:
    // either an initramfs (default) or a cloned, writable virtio-fs root.
    let (initramfs, virtiofs_root) = if args.virtiofs() {
        let root = linux::prepare_virtiofs_root(&image.rootfs, &ep, net_up, persistent, &vdir)?;
        (None, Some(root))
    } else {
        linux::warn_initramfs_memory(&image.rootfs, args.vm.mem);
        (
            Some(linux::build_initramfs(&image.rootfs, &ep, net_up, persistent)?),
            None,
        )
    };

    if args.detach {
        return run_linux_detached(
            &args,
            &kernel,
            &ep,
            initramfs.as_deref(),
            virtiofs_root.as_deref(),
            net_up,
            &machine_id,
            &vdir,
        );
    }

    // Foreground: record, then boot attached to this terminal.
    db::record_machine(
        &machine_id,
        &args.image,
        &command,
        "running",
        Some(std::process::id() as i64),
        false,
        args.vm.cpus as i64,
        args.vm.mem as i64,
        &vdir.to_string_lossy(),
    );

    // Snapshot the terminal + arm signal cleanup before libkrun raws the TTY.
    // NOTE: no stderr-tee watchdog here — it redirects fd 2, which libkrun's
    // implicit virtio-console (hvc0) claims for the guest.
    tty::install();

    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    // `hvc*` uses libkrun's implicit virtio-console; `ttyS*` its explicit serial.
    if !args.console.starts_with("hvc") {
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
    }
    let gvproxy = configure_linux_ctx(
        &ctx,
        &args,
        &kernel,
        &ep,
        initramfs.as_deref(),
        virtiofs_root.as_deref(),
        net_up,
    )?;

    info!(
        id = %machine_id,
        cpus = args.vm.cpus,
        mem_mib = args.vm.mem,
        image = %args.image,
        mode = if args.virtiofs() { "virtiofs" } else { "initramfs" },
        "booting Linux machine"
    );
    finish_recording(ctx, gvproxy, machine_id)
}

/// Configure networking + rootfs + kernel on `ctx` (shared by the foreground and
/// detached paths; the caller wires the console first). Returns the gvproxy.
fn configure_linux_ctx(
    ctx: &Ctx,
    args: &LinuxArgs,
    kernel: &std::path::Path,
    _ep: &linux::Entrypoint,
    initramfs: Option<&std::path::Path>,
    virtiofs_root: Option<&std::path::Path>,
    net_up: bool,
) -> Result<Option<Gvproxy>> {
    let gvproxy = setup_networking(ctx, &args.net)?;
    if args.virtiofs() {
        // Share the per-machine (cloned) rootfs over virtio-fs and boot our own
        // init from it — we don't use libkrun's init.krun (that's for the bundled
        // libkrunfw kernel), so no set_exec/set_workdir here (our init handles it).
        let root = virtiofs_root.expect("virtio-fs root prepared for --virtiofs boot");
        ctx.set_root(root)
            .context("configuring virtio-fs root (needs a CONFIG_VIRTIO_FS=y kernel)")?;
        let cmdline = linux::virtiofs_cmdline(&args.console, net_up);
        ctx.set_kernel(kernel, krun::KRUN_KERNEL_FORMAT_RAW, None, &cmdline)
            .context("configuring kernel")?;
    } else {
        let initramfs = initramfs.expect("initramfs built for non-virtiofs boot");
        let cmdline = linux::kernel_cmdline(&args.console, net_up);
        ctx.set_kernel(kernel, krun::KRUN_KERNEL_FORMAT_RAW, Some(initramfs), &cmdline)
            .context("configuring kernel")?;
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

/// Run a Linux machine in the background: fork, record the child in the state
/// DB, print its id, and detach the child (console → per-machine socket + log).
#[allow(clippy::too_many_arguments)]
fn run_linux_detached(
    args: &LinuxArgs,
    kernel: &std::path::Path,
    ep: &linux::Entrypoint,
    initramfs: Option<&std::path::Path>,
    virtiofs_root: Option<&std::path::Path>,
    net_up: bool,
    machine_id: &str,
    vdir: &std::path::Path,
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
        // Parent: record the running VM and print its id, like `docker run -d`.
        db::record_machine(
            machine_id,
            &args.image,
            &ep.argv.join(" "),
            "running",
            Some(pid as i64),
            true,
            args.vm.cpus as i64,
            args.vm.mem as i64,
            &vdir.to_string_lossy(),
        );
        println!("{machine_id}");
        return Ok(());
    }

    // Child: detach from the terminal/session and boot the VM.
    unsafe { libc::setsid() };

    // Send bsdkrun's own logs (fd 2) to a file in the VM dir.
    if let Ok(logf) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(vdir.join("bsdkrun.log"))
    {
        use std::os::fd::AsRawFd;
        unsafe { libc::dup2(logf.as_raw_fd(), 2) };
        std::mem::forget(logf); // fd 2 now owns it
    }

    let code = match run_detached_child(
        args,
        kernel,
        ep,
        initramfs,
        virtiofs_root,
        net_up,
        machine_id,
        vdir,
    ) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("detached machine failed: {e:#}");
            db::update_machine_status(machine_id, "exited", Some(1));
            1
        }
    };
    unsafe { libc::_exit(code) };
}

/// The detached child's boot: wire the guest console to the broker socket, set
/// up the VM, and run it. Returns the guest's exit code.
#[allow(clippy::too_many_arguments)]
fn run_detached_child(
    args: &LinuxArgs,
    kernel: &std::path::Path,
    ep: &linux::Entrypoint,
    initramfs: Option<&std::path::Path>,
    virtiofs_root: Option<&std::path::Path>,
    net_up: bool,
    machine_id: &str,
    vdir: &std::path::Path,
) -> Result<i32> {
    use std::os::fd::AsRawFd;

    // Wire the guest console (fd 0/1) to the broker socketpair; a thread fans it
    // out to console.log + console.sock (see the console module).
    let console_fd = console::setup_detached(vdir)?;
    unsafe {
        libc::dup2(console_fd, 0);
        libc::dup2(console_fd, 1);
    }

    // Signal cleanup so `stop` (SIGTERM) tears down gvproxy. No terminal to
    // restore (fd 0 is a socket), so tty::install's termios save is a no-op.
    tty::install();

    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    let _ = console_fd.as_raw_fd(); // (fd kept alive by dup2 above)
    let gvproxy =
        configure_linux_ctx(&ctx, args, kernel, ep, initramfs, virtiofs_root, net_up)?;

    info!(id = %machine_id, image = %args.image, "detached Linux machine booting");
    let code = ctx.start_enter().context("starting machine")?;
    db::update_machine_status(machine_id, "exited", Some(code as i64));
    drop(gvproxy);
    Ok(code)
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
fn cmd_ps(all: bool) -> Result<()> {
    let db = db::Db::open()?;
    let machines = db.list_machines()?;
    println!(
        "{:<14}  {:<22}  {:<12}  {:<10}  {}",
        "ID", "IMAGE", "STATUS", "CREATED", "COMMAND"
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
        let status = if running {
            "running".to_string()
        } else {
            match m.exit_code {
                Some(c) => format!("exited ({c})"),
                None => "exited".to_string(),
            }
        };
        println!(
            "{:<14}  {:<22}  {:<12}  {:<10}  {}",
            m.id,
            truncate(&m.image, 22),
            status,
            db::age(&m.created_at),
            truncate(&m.command, 40)
        );
    }
    Ok(())
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
fn cmd_images() -> Result<()> {
    let db = db::Db::open()?;
    let images = db.list_images()?;
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

fn cmd_stop(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    match vm.pid {
        Some(pid) if db::pid_alive(pid) => {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            db.set_machine_status(&vm.id, "exited", vm.exit_code).ok();
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

fn cmd_logs(id: &str, follow: bool) -> Result<()> {
    use std::io::Write;
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let log = vdir.join("console.log");
    if let Ok(data) = std::fs::read(&log) {
        std::io::stdout().write_all(&data).ok();
        std::io::stdout().flush().ok();
    } else if !follow {
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
    if !vm.detached {
        anyhow::bail!("`shell` attaches to a detached machine — start it with `-d`");
    }
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        anyhow::bail!("machine {} is not running", vm.id);
    }
    console::attach_interactive(&std::path::PathBuf::from(&vm.state_dir))
}
