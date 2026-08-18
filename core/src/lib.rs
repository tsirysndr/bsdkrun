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
/// AI coding agents in disposable microVMs — the registry, the sandbox
/// lifecycle, and the shared skills store.
pub mod ai;
pub mod api;
pub mod cache;
pub mod cli;
pub mod commands;
pub mod console;
pub mod db;
/// Docker compatibility: a `docker:dind` microVM whose API is served on a host
/// unix socket, so the host's own `docker` CLI drives it.
pub mod docker;
pub mod domains;
pub mod elf;
pub mod fetch;
pub mod flavors;
pub mod host;
pub mod id;
/// The libkrun FFI. Its constants are always available — other modules pick a
/// kernel format with them — but the parts that call into the library are only
/// compiled when this crate can actually start a machine.
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
/// Solo5 unikernels (MirageOS). Unlike every other guest here, these run in a
/// separate process — the `solo5-hvt` tender — rather than through libkrun.
pub mod solo5;
#[cfg(target_os = "macos")]
pub mod store;
pub mod tty;
#[cfg(all(feature = "boot", feature = "tui"))]
pub mod tui;
#[cfg(feature = "ui")]
pub mod ui;
pub mod unikraft;
pub mod watchdog;

#[cfg(feature = "boot")]
use anyhow::Result;
#[cfg(feature = "boot")]
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
/// Only compiled with `boot`, since most of what it dispatches to starts a
/// machine. A caller that links this crate without it — the daemon — drives
/// [`api`] directly and hands anything boot-shaped to `bsdkrun-supervisor`.
#[cfg(feature = "boot")]
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
        #[cfg(feature = "solo5")]
        Command::Solo5(args) => commands::solo5::cmd_solo5(args),
        // The variant always exists (the daemon needs to *describe* the
        // command); only the implementation is feature-gated.
        #[cfg(not(feature = "solo5"))]
        Command::Solo5(_) => anyhow::bail!(
            "this build has no solo5 support: it was compiled without the `solo5` feature. \
             Rebuild with `--features solo5` (and the library/solo5 submodule checked out)."
        ),
        Command::Ps(args) => commands::machines::cmd_ps(args.all, args.json),
        Command::Images(args) => commands::images::cmd_images(args.json),
        Command::Image(args) => match args.cmd {
            ImageCmd::Rm(a) => commands::images::cmd_image_rm(&a.ids, a.force),
        },
        Command::Stop(args) => commands::machines::cmd_stop(&args.id),
        Command::Start(args) => commands::boot::cmd_start(&args.id),
        Command::Update(args) => commands::machines::cmd_update(&args.id, args.cpus, args.mem),
        Command::Rm(args) => commands::machines::cmd_rm(&args.ids, args.force),
        Command::Prune(a) => {
            commands::prune::cmd_prune(a.all, a.volumes, &a.only, a.force, a.dry_run, a.json)
        }
        Command::Agent(args) => match args.cmd {
            AgentCmd::Update(a) => commands::guest::cmd_agent_update(&a.id),
        },
        Command::Logs(args) => commands::guest::cmd_logs(&args.id, args.follow, args.boot),
        Command::Shell(args) => commands::guest::cmd_shell(&args.id),
        Command::Exec(args) => {
            commands::guest::cmd_exec(&args.id, &args.command, &args.env, args.tty)
        }
        Command::Cp(args) => commands::cp::cmd_cp(&args.src, &args.dst, args.recursive),
        Command::Cache(args) => match args.cmd {
            CacheCmd::Save(a) => {
                let (id, path) = split_target(&a.target)?;
                let path = path.ok_or_else(|| {
                    anyhow::anyhow!("`cache save` needs the directory to archive: ID:PATH")
                })?;
                commands::cache::cmd_save(id, path, &a.key, a.compression.parse()?, a.force, a.json)
            }
            CacheCmd::Restore(a) => {
                let (id, path) = split_target(&a.target)?;
                commands::cache::cmd_restore(id, path, &a.key, &a.restore_keys, a.json)
            }
            CacheCmd::Ls(a) => commands::cache::cmd_ls(a.json),
            CacheCmd::Rm(a) => commands::cache::cmd_rm(&a.keys, a.all),
        },
        Command::Doctor(args) => commands::doctor::cmd_doctor(args.json),
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
        Command::Ai(args) => match args.cmd {
            AiCmd::Agents(a) => commands::ai::cmd_agents(a.json),
            // Typed at a terminal, so it shares the cwd like the aliases do.
            AiCmd::Start(a) => commands::boot::cmd_ai_start(a.with_cli_cwd()),
            AiCmd::Ls(a) => commands::ai::cmd_sessions(a.json),
            AiCmd::Stop(a) => commands::ai::cmd_stop(&a.agent),
            AiCmd::Resume(a) => commands::boot::cmd_ai_resume(&a.machine, a.detach),
            AiCmd::Disk(a) => match a.cmd {
                AiDiskCmd::Ls(l) => commands::ai::cmd_disk_ls(l.json, l.watch),
                AiDiskCmd::Grow(g) => commands::ai::cmd_disk_grow(&g.disk, &g.size),
            },
            AiCmd::Rm(a) => commands::ai::cmd_rm(&a.agent, a.keep_home),
            AiCmd::ShellCommand(a) => commands::ai::cmd_shell_command(&a.agent, &a.machine),
            AiCmd::Upload(a) => commands::ai::cmd_upload(
                &a.what,
                &a.agent,
                a.dir.as_deref(),
                a.name.as_deref(),
                a.all,
                a.json,
            ),
            AiCmd::Receive(a) => {
                commands::ai::cmd_receive(&a.what, &a.agent, a.name.as_deref(), a.json)
            }
        },
        // The per-agent aliases: `bsdkrun claude` is `ai start claude --cwd`.
        Command::Claude(a) => commands::boot::cmd_ai_start(a.into_start("claude")),
        Command::Codex(a) => commands::boot::cmd_ai_start(a.into_start("codex")),
        Command::Gemini(a) => commands::boot::cmd_ai_start(a.into_start("gemini")),
        Command::Opencode(a) => commands::boot::cmd_ai_start(a.into_start("opencode")),
        Command::Crush(a) => commands::boot::cmd_ai_start(a.into_start("crush")),
        Command::Copilot(a) => commands::boot::cmd_ai_start(a.into_start("copilot")),
        Command::Kilo(a) => commands::boot::cmd_ai_start(a.into_start("kilo")),
        Command::Qwen(a) => commands::boot::cmd_ai_start(a.into_start("qwen")),
        Command::Docker(args) => match args.cmd {
            DockerCmd::Start(a) => commands::boot::cmd_docker_start(a),
            DockerCmd::Stop => commands::docker::cmd_stop(),
            DockerCmd::Status(a) => commands::docker::cmd_status(a.json),
            DockerCmd::Rm(a) => commands::docker::cmd_rm(a.force),
            DockerCmd::Ps(a) => commands::docker::cmd_ps(a.all, a.json),
            DockerCmd::Container(a) => commands::docker::cmd_container(&a.action, &a.ids),
            DockerCmd::Logs(a) => commands::docker::cmd_logs(&a.id, a.tail),
            DockerCmd::Disk(a) => commands::docker::cmd_disk(a.size.as_deref(), a.json),
            DockerCmd::Env => commands::docker::cmd_env(),
            DockerCmd::Shell => commands::docker::cmd_shell(),
            DockerCmd::Serve(a) => commands::docker::cmd_serve(a.port, &a.machine, &a.publish_bind),
        },
        Command::Snapshot(args) => match args.cmd {
            Some(SnapshotCmd::Ls(a)) => commands::snapshot::cmd_ls(a.machine.as_deref(), a.json),
            Some(SnapshotCmd::Rm(a)) => commands::snapshot::cmd_rm(&a.names),
            None => {
                let id = args.id.ok_or_else(|| {
                    anyhow::anyhow!(
                        "which machine? `bsdkrun snapshot <ID> [NAME]` (see `bsdkrun ps -a`)"
                    )
                })?;
                commands::snapshot::cmd_create(
                    &id,
                    args.name.as_deref(),
                    &args.description,
                    args.json,
                )
            }
        },
        Command::Snapshots(args) => commands::snapshot::cmd_ls(args.machine.as_deref(), args.json),
        Command::Branch(args) => commands::boot::cmd_branch(args),
        Command::Restore(args) => {
            commands::snapshot::cmd_restore(&args.id, &args.snapshot, args.force, !args.no_backup)
        }
        Command::Rollback(args) => {
            commands::snapshot::cmd_rollback(&args.id, args.force, !args.no_backup)
        }
        Command::Flavors(args) => commands::flavor::cmd_flavors(args.json),
        Command::Flavor(args) => match args.cmd {
            FlavorCmd::Run(a) => commands::boot::cmd_flavor_run(a),
            FlavorCmd::Add(a) => commands::flavor::cmd_flavor_add(a),
            FlavorCmd::Rm(a) => commands::flavor::cmd_flavor_rm(&a.names, a.force),
            FlavorCmd::Build(a) => {
                commands::boot::cmd_flavor_prebuild(&a.name, a.vm.cpus, a.vm.mem, a.force)
            }
            FlavorCmd::Dockerfiles(a) => commands::flavor::cmd_dockerfiles(&a.out, a.check),
            FlavorCmd::BuildInternal(a) => {
                commands::boot::cmd_flavor_build(&a.name, &a.key, a.vm.cpus, a.vm.mem)
            }
        },
        Command::Ui(args) => serve_ui(args),
        #[cfg(feature = "pack")]
        Command::Pack(args) => commands::pack::cmd_pack(&args.args),
        Command::Network(args) => match args.cmd {
            NetworkCmd::Create(a) => network::cmd_create(&a.name),
            NetworkCmd::Ls(a) => network::cmd_ls(a.json),
            NetworkCmd::Rm(a) => network::cmd_rm(&a.names, a.force),
            NetworkCmd::Connect(a) => network::cmd_connect(&a.machine, &a.network),
            NetworkCmd::Disconnect(a) => network::cmd_disconnect(&a.machine),
            NetworkCmd::Sync(a) => network::cmd_sync(&a.network),
        },
        Command::Domains(args) => match args.cmd {
            DomainsCmd::Enable(a) => commands::domains::cmd_enable(a),
            DomainsCmd::Disable(a) => commands::domains::cmd_disable(a.purge),
            DomainsCmd::Status(a) => commands::domains::cmd_status(a.json),
            DomainsCmd::Ls(a) => commands::domains::cmd_ls(a.json),
            DomainsCmd::Sync => commands::domains::cmd_sync(),
            DomainsCmd::Ca(a) => commands::domains::cmd_ca(a.pem),
            // The detached responder process: serve() never returns.
            DomainsCmd::ServeDns(a) => domains::dns::serve(a.port, &a.tld),
        },
        Command::Tui(args) => run_tui(args),
    }
}

/// `bsdkrun tui`, when this build has the dashboard compiled in.
#[cfg(all(feature = "boot", feature = "tui"))]
fn run_tui(args: cli::TuiArgs) -> Result<()> {
    commands::tui::cmd_tui(args)
}

#[cfg(all(feature = "boot", not(feature = "tui")))]
fn run_tui(_args: cli::TuiArgs) -> Result<()> {
    anyhow::bail!("this build has no TUI compiled in")
}

/// `bsdkrun ui`, when this build has the SPA compiled in.
///
/// Reachable only from [`dispatch`], hence the `boot` gate alongside the `ui`
/// one: a build that cannot start a machine has no dispatch to reach it from.
///
/// The bundle is behind a feature so the daemon — which serves its own API and
/// has no use for a second web server — does not link one.
#[cfg(all(feature = "boot", feature = "ui"))]
fn serve_ui(args: cli::UiArgs) -> Result<()> {
    ui::serve_ui(args.bind, !args.no_open)
}

#[cfg(all(feature = "boot", not(feature = "ui")))]
fn serve_ui(_args: cli::UiArgs) -> Result<()> {
    anyhow::bail!("this build has no web UI compiled in")
}

/// Split a `cache` target into `(id, path)`. The path is optional so
/// `cache restore web` can mean "back where it came from".
#[cfg(feature = "boot")]
fn split_target(target: &str) -> Result<(&str, Option<&str>)> {
    match target.split_once(':') {
        Some((id, path)) if !id.is_empty() && !path.is_empty() => Ok((id, Some(path))),
        Some(_) => anyhow::bail!("expected ID:PATH, got {target:?}"),
        None => Ok((target, None)),
    }
}
