//! bsdkrun — a Firecracker-style machine launcher for BSD and Linux guests on
//! macOS, built on libkrun (Hypervisor.framework).
//!
//! This crate is the engine. It owns every boot mode, the machine database, the
//! image store, networking, volumes and the in-guest agent; the `bsdkrun` binary
//! is a clap front end over it, and `bsdkrund` links it directly rather than
//! shelling out to that binary.
//!
//! Boot modes:
//!   * kernel   — direct kernel + cmdline, using libkrun's generated FDT
//!     (target: NetBSD evbarm / bare kernel+FDT boot)
//!   * firmware — a UEFI firmware image that boots a normal BSD disk via its
//!     EFI loader (target: FreeBSD / NetBSD arm64)
//!   * linux    — run an OCI image (Docker Hub / any registry) as a Linux
//!     machine: fetch a kernel, extract the rootfs, boot it
//!
//! Two layers sit on top of those modules:
//!   * [`commands`] — the subcommands as the CLI runs them, printing as they go.
//!   * [`cli`] — the command surface as data, so a caller that is not a terminal
//!     can build the same requests.

pub mod agent;
pub mod api;
pub mod cli;
pub mod commands;
pub mod console;
pub mod db;
pub mod elf;
pub mod fetch;
pub mod flavors;
pub mod host;
pub mod id;
pub mod krun;
pub mod linux;
pub mod names;
pub mod nanos;
pub mod net;
pub mod network;
pub mod oci;
pub mod osv;
/// The case-sensitive store only exists to work around case-insensitive APFS;
/// Linux filesystems are case-sensitive already, so nix guests work there
/// out of the box and the module is not compiled.
#[cfg(target_os = "macos")]
pub mod store;
pub mod tty;
#[cfg(feature = "ui")]
pub mod ui;
pub mod unikraft;
pub mod watchdog;

use anyhow::Result;

use cli::Command;

/// This engine's version, which is also the version the CLI and the daemon
/// report — there is only one implementation to report on.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prepare the process to run machines.
///
/// Both front ends call this before anything else, because both get it wrong in
/// the same ways otherwise: an unraised fd limit surfaces inside the guest as
/// "Too many open files", and an unattached store turns `<cache>/store` into an
/// empty directory on the case-insensitive boot volume, so images extract into
/// the wrong place.
pub fn init_host() {
    // Before anything opens a file: virtio-fs is passthrough, so every file the
    // guest holds open burns an fd in this process. launchd hands GUI-launched
    // processes (the desktop app) a 256-fd soft limit, which a guest running
    // `nix` blows through.
    host::raise_fd_limit();

    // A sparsebundle does not survive a reboot attached, so re-attach it before
    // any path is resolved.
    #[cfg(target_os = "macos")]
    store::auto_attach();
}

/// Run one subcommand, exactly as `bsdkrun` would.
///
/// The daemon uses this for the handful of operations that are genuinely a
/// command line — the passthrough RPC, and the boot supervisor it re-execs —
/// while everything else goes through [`api`] instead.
pub fn dispatch(cmd: Command) -> Result<()> {
    use cli::*;

    match cmd {
        Command::Probe => commands::probe::probe(),
        #[cfg(target_os = "linux")]
        Command::Kvm(args) => commands::probe::cmd_kvm(args.json),
        Command::Kernel(args) => commands::boot::boot_kernel(args),
        Command::Firmware(args) => commands::boot::boot_firmware(args),
        Command::Fetch(args) => {
            fetch::fetch(args.os, args.version, &args.dir, args.force).map(|_| ())
        }
        Command::Versions(args) => fetch::list_versions(args.os),
        Command::Grow(args) => fetch::grow(&args.disk, &args.size),
        Command::Linux(args) => commands::boot::boot_linux(args),
        Command::Freebsd(args) => commands::boot::boot_freebsd(args),
        Command::Netbsd(args) => commands::boot::boot_netbsd(args),
        Command::Unikraft(args) => commands::boot::boot_unikraft(args),
        Command::Nanos(args) => commands::boot::boot_nanos(args),
        Command::Osv(args) => commands::boot::boot_osv(args),
        Command::Ps(args) => commands::machines::cmd_ps(args.all, args.json),
        Command::Images(args) => commands::images::cmd_images(args.json),
        Command::Stop(args) => commands::machines::cmd_stop(&args.id),
        Command::Start(args) => commands::machines::cmd_start(&args.id),
        Command::Update(args) => commands::machines::cmd_update(&args.id, args.cpus, args.mem),
        Command::Rm(args) => commands::machines::cmd_rm(&args.ids, args.force),
        Command::Agent(args) => match args.cmd {
            AgentCmd::Update(a) => commands::guest::cmd_agent_update(&a.id),
        },
        Command::Logs(args) => commands::guest::cmd_logs(&args.id, args.follow, args.boot),
        Command::Shell(args) => commands::guest::cmd_shell(&args.id),
        Command::Exec(args) => {
            commands::guest::cmd_exec(&args.id, &args.command, &args.env, args.tty)
        }
        Command::Tailscale(args) => commands::guest::cmd_tailscale(&args.id, &args.args),
        Command::Ssh(args) => commands::guest::cmd_ssh(&args.id, &args.args),
        Command::Systemd(args) => {
            commands::guest::run_agent_cli(&args.id, "systemd", &args.args, &[])
        }
        Command::Volume(args) => match args.cmd {
            VolumeCmd::Ls(a) => commands::volumes::cmd_volume_ls(a.json),
            VolumeCmd::Rm(a) => commands::volumes::cmd_volume_rm(&a.names, a.force),
        },
        #[cfg(target_os = "macos")]
        Command::Store(args) => match args.cmd {
            StoreCmd::Init(a) => commands::store::cmd_store_init(&a.size),
            StoreCmd::Status => commands::store::cmd_store_status(),
            StoreCmd::Attach => commands::store::cmd_store_attach(),
            StoreCmd::Detach(a) => commands::store::cmd_store_detach(a.force),
            StoreCmd::Rm(a) => commands::store::cmd_store_rm(a.force),
        },
        Command::Commit(args) => {
            commands::machines::cmd_commit(&args.id, &args.name, &args.description)
        }
        Command::Flavors(args) => commands::flavor::cmd_flavors(args.json),
        Command::Flavor(args) => match args.cmd {
            FlavorCmd::Run(a) => commands::flavor::cmd_flavor_run(a),
            FlavorCmd::Add(a) => commands::flavor::cmd_flavor_add(a),
            FlavorCmd::Rm(a) => commands::flavor::cmd_flavor_rm(&a.names, a.force),
            FlavorCmd::Build(a) => {
                commands::flavor::cmd_flavor_prebuild(&a.name, a.vm.cpus, a.vm.mem, a.force)
            }
            FlavorCmd::BuildInternal(a) => {
                commands::flavor::cmd_flavor_build(&a.name, &a.key, a.vm.cpus, a.vm.mem)
            }
        },
        Command::Ui(args) => serve_ui(args),
        Command::Network(args) => match args.cmd {
            NetworkCmd::Create(a) => network::cmd_create(&a.name),
            NetworkCmd::Ls(a) => network::cmd_ls(a.json),
            NetworkCmd::Rm(a) => network::cmd_rm(&a.names, a.force),
            NetworkCmd::Connect(a) => network::cmd_connect(&a.machine, &a.network),
            NetworkCmd::Disconnect(a) => network::cmd_disconnect(&a.machine),
            NetworkCmd::Sync(a) => network::cmd_sync(&a.network),
        },
    }
}

/// `bsdkrun ui`, when this build has the SPA compiled in.
///
/// The bundle is behind a feature so the daemon — which serves its own API and
/// has no use for a second web server — does not link one.
#[cfg(feature = "ui")]
fn serve_ui(args: cli::UiArgs) -> Result<()> {
    ui::serve_ui(args.bind, !args.no_open)
}

#[cfg(not(feature = "ui"))]
fn serve_ui(_args: cli::UiArgs) -> Result<()> {
    anyhow::bail!("this build has no web UI compiled in")
}
