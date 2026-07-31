//! bsdkrun — a Firecracker-style microVM launcher for BSD guests on macOS,
//! built on libkrun (Hypervisor.framework).
//!
//! Two boot modes:
//!   * kernel   — direct kernel + cmdline, using libkrun's generated FDT
//!                (target: NetBSD evbarm / bare kernel+FDT boot)
//!   * firmware — a UEFI firmware image that boots a normal BSD disk via its
//!                EFI loader (target: FreeBSD / OpenBSD arm64)

mod krun;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use tracing::info;
use tracing_subscriber::EnvFilter;

use krun::Ctx;

#[derive(Parser)]
#[command(name = "bsdkrun", version, about)]
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
    }
}

fn probe() -> Result<()> {
    let ctx = Ctx::new().context("creating libkrun context")?;
    ctx.set_vm_config(1, 256)
        .context("setting a trivial VM config")?;
    info!("libkrun linked and a context was created + configured (dropped without booting)");
    Ok(())
}

fn boot_kernel(args: KernelArgs) -> Result<()> {
    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    ctx.attach_stdio_serial_console()
        .context("wiring guest serial console to stdio")?;

    if let Some(disk) = &args.disk {
        ctx.add_disk("root", disk, false)
            .with_context(|| format!("attaching root disk {}", disk.display()))?;
    }

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
    let code = ctx.start_enter().context("starting microVM")?;
    info!(code, "guest exited");
    std::process::exit(code);
}

fn boot_firmware(args: FirmwareArgs) -> Result<()> {
    let ctx = Ctx::new()?;
    ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
    ctx.attach_stdio_serial_console()
        .context("wiring guest serial console to stdio")?;

    ctx.add_disk("root", &args.disk, false)
        .with_context(|| format!("attaching root disk {}", args.disk.display()))?;

    ctx.set_firmware(&args.firmware)
        .context("configuring firmware")?;

    info!(
        cpus = args.vm.cpus,
        mem_mib = args.vm.mem,
        firmware = %args.firmware.display(),
        disk = %args.disk.display(),
        "booting microVM"
    );
    let code = ctx.start_enter().context("starting microVM")?;
    info!(code, "guest exited");
    std::process::exit(code);
}
