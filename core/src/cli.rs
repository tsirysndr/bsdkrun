//! The command-line surface, as data.
//!
//! Every subcommand and its arguments live here rather than in the `bsdkrun`
//! binary, because the CLI is no longer the only thing that issues them: the
//! daemon builds these same structs and hands them to the same entry points.
//! Keeping one definition is the whole point — the daemon used to re-encode
//! this surface as argv strings, and a misplaced flag there was invisible until
//! it reached a real machine.
//!
//! Fields are public for that reason: a caller that is not clap still has to be
//! able to fill them in.

use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::styling::{Color, RgbColor, Style, Styles};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[cfg(feature = "boot")]
use crate::krun;
use crate::net::PortForward;
#[cfg(target_os = "macos")]
use crate::store;
use crate::{fetch, linux, osv};

// Accent palette (with matching muted + error tones) applied to clap's --help
// styling: electric teal for section headers/usage, violet for literals.
pub(crate) const TEAL: Color = Color::Rgb(RgbColor(0, 232, 198));
pub(crate) const VIOLET: Color = Color::Rgb(RgbColor(130, 100, 255));
pub(crate) const MUTED: Color = Color::Rgb(RgbColor(200, 210, 220));
pub(crate) const ERROR: Color = Color::Rgb(RgbColor(255, 100, 100));

/// clap help/usage colors: electric teal for the section headers
/// (Usage/Commands/Options), violet for literals (flags & subcommand names),
/// muted gray for placeholders.
pub fn cli_styles() -> Styles {
    Styles::styled()
        .header(Style::new().bold().fg_color(Some(TEAL)))
        .usage(Style::new().bold().fg_color(Some(TEAL)))
        .literal(Style::new().fg_color(Some(VIOLET)))
        .placeholder(Style::new().fg_color(Some(MUTED)))
        .valid(Style::new().fg_color(Some(VIOLET)))
        .error(Style::new().bold().fg_color(Some(ERROR)))
        .invalid(Style::new().bold().fg_color(Some(ERROR)))
}

#[derive(Parser, Serialize, Deserialize)]
// `about` is spelled out rather than taken from the package description: this
// crate is the engine, and its description says so, but `--help` belongs to the
// product. `version` is this crate's, which is the one number there is — keep
// the `bsdkrun` package's version in step with it.
#[command(
    name = "bsdkrun",
    version,
    about = "A Firecracker-style microVM launcher for BSD and Linux (OCI) guests on macOS, built on libkrun (Hypervisor.framework)",
    styles = cli_styles()
)]
pub struct Cli {
    /// libkrun log verbosity (0=off .. 5=trace)
    #[arg(long, global = true, default_value_t = 1)]
    pub log_level: u32,

    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum Command {
    /// Check that libkrun links and a context/hvf can be initialized.
    Probe,

    /// Check that this host can run machines at all: `/dev/kvm` exists, this
    /// user can open it, and it speaks the expected KVM API. Exits non-zero
    /// when it can't, so scripts can gate on it.
    #[cfg(target_os = "linux")]
    Kvm(KvmArgs),

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

    /// Boot a Unikraft unikernel (built with `kraft build --plat fc`).
    Unikraft(UnikraftArgs),

    /// Boot a Nanos (NanoVMs) unikernel image (built with `ops build`).
    /// Linux/x86_64 boots it Firecracker-style (experimental); macOS/arm64
    /// needs an upstream Nanos fix — see examples/nanos-hello.
    Nanos(NanosArgs),

    /// Boot an OSv unikernel image (an OSv release loader, or one composed by
    /// `capstan`).
    Osv(OsvArgs),

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

    /// Manage the case-sensitive store that nix guests need (macOS only).
    #[cfg(target_os = "macos")]
    Store(StoreArgs),

    /// Snapshot a machine's current state into a named flavor (like `docker commit`).
    Commit(CommitArgs),

    /// List flavors: the built-in catalog + your saved snapshots.
    Flavors(FlavorsListArgs),

    /// Run / remove flavors (`flavor run <name>`, `flavor rm <name>`).
    Flavor(FlavorArgs),

    /// Manage global networks (a shared subnet + internal DNS) that machines join
    /// with `--network` to reach each other by IP and by name.
    Network(NetworkArgs),

    /// Serve the bundled web interface (a Docker-Desktop-style UI in a browser).
    ///
    /// Static assets only — the UI drives a `bsdkrund` GraphQL API, which it
    /// asks for on first run, so it can manage a daemon on any reachable host.
    Ui(UiArgs),

    /// Package a project into a bootable Unikraft unikernel: detect its
    /// language, build it with BuildKit, and generate a Kraftfile — like
    /// railpack, but for `kraft build` instead of an OCI image. Boot the
    /// result with `bsdkrun unikraft .`.
    ///
    /// A thin wrapper over a separate (embedded) tool, so every flag below
    /// `pack` belongs to it, not to this CLI's clap surface — run
    /// `bsdkrun pack --help` for the real list.
    #[cfg(feature = "pack")]
    Pack(PackArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct UiArgs {
    /// Address to serve the UI on.
    #[arg(long, default_value = "127.0.0.1:8088")]
    pub bind: std::net::SocketAddr,

    /// Do not open a browser.
    #[arg(long)]
    pub no_open: bool,
}

#[cfg(feature = "pack")]
#[derive(Parser, Serialize, Deserialize)]
// Without this, clap intercepts `-h`/`--help` itself and prints *this*
// struct's (empty) help instead of forwarding to the real one `bsdkrun-pack
// --help` prints — defeating the entire point of a passthrough subcommand.
#[command(disable_help_flag = true)]
pub struct PackArgs {
    /// Forwarded verbatim to the `bsdkrun-pack` binary (e.g. a project path,
    /// `--help`, `--target arm64`). Not parsed here — see
    /// `bsdkrun pack --help`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkArgs {
    #[command(subcommand)]
    pub cmd: NetworkCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum NetworkCmd {
    /// Create a network (starts its shared gvproxy switch).
    Create(NetworkCreateArgs),
    /// List networks and their members.
    Ls(NetworkLsArgs),
    /// Remove one or more networks (refuses running members unless `-f`).
    Rm(NetworkRmArgs),
    /// Connect a machine to a network (join/switch) — applies on next start.
    Connect(NetworkConnectArgs),
    /// Disconnect a machine from its network — applies on next start.
    Disconnect(NetworkDisconnectArgs),
    /// Refresh members' /etc/hosts with current membership (fixes name lookup).
    Sync(NetworkSyncArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkCreateArgs {
    /// Network name.
    #[arg(value_name = "NAME")]
    pub name: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkLsArgs {
    /// Emit JSON (for scripting / the desktop).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkRmArgs {
    /// Remove even with running members (they lose the network on next start).
    #[arg(short, long)]
    pub force: bool,

    /// Network name(s) to remove.
    #[arg(value_name = "NAME", required = true)]
    pub names: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkConnectArgs {
    /// Machine id or name.
    #[arg(value_name = "MACHINE")]
    pub machine: String,
    /// Network to join.
    #[arg(value_name = "NETWORK")]
    pub network: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkDisconnectArgs {
    /// Machine id or name.
    #[arg(value_name = "MACHINE")]
    pub machine: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct NetworkSyncArgs {
    /// Network whose members' /etc/hosts to refresh.
    #[arg(value_name = "NETWORK")]
    pub network: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct CommitArgs {
    /// machine id to snapshot (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Name for the new flavor.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Optional description.
    #[arg(short, long, default_value = "")]
    pub description: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorsListArgs {
    /// Emit the flavor list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorArgs {
    #[command(subcommand)]
    pub cmd: FlavorCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum FlavorCmd {
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

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorPrebuildArgs {
    /// Flavor name to build (a catalog or user flavor with provisioning).
    #[arg(value_name = "NAME")]
    pub name: String,

    #[command(flatten)]
    pub vm: VmConfig,

    /// Rebuild even if a cached build already exists.
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorAddArgs {
    /// Flavor name (letters, digits, '-', '_', '.').
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Base image: an OCI ref (e.g. `node:22`), or `freebsd` / `netbsd`.
    #[arg(long)]
    pub base: String,

    /// Grouping for the UI (language / service / web / ai / …).
    #[arg(long, default_value = "custom")]
    pub category: String,

    /// A short description.
    #[arg(long, default_value = "")]
    pub description: String,

    /// Default host↔guest port forward `HOST:GUEST` (repeatable). Prefix with
    /// a bind address (`BIND:HOST:GUEST`, e.g. `0.0.0.0:8080:80`) to make it
    /// reachable from the LAN instead of just localhost.
    #[arg(long = "port", value_name = "[BIND:]HOST:GUEST")]
    pub ports: Vec<String>,

    /// Default environment `K=V` (repeatable).
    #[arg(long = "env", value_name = "K=V")]
    pub env: Vec<String>,

    /// Nix package to install on an OCI base (repeatable).
    #[arg(long = "nix", value_name = "PKG")]
    pub nix: Vec<String>,

    /// Provisioning command run in the guest after boot (repeatable, in order).
    #[arg(long = "provision", value_name = "CMD")]
    pub provision: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorBuildArgs {
    /// Flavor name to build.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Cache key (the build volume to provision into).
    #[arg(long)]
    pub key: String,

    #[command(flatten)]
    pub vm: VmConfig,
}

#[derive(Parser, Default, Serialize, Deserialize)]
pub struct FlavorRunArgs {
    /// Flavor name (see `bsdkrun flavors`).
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Run detached in the background (like `docker run -d`).
    #[arg(short = 'd', long)]
    pub detach: bool,

    #[command(flatten)]
    pub vm: VmConfig,

    /// Extra host↔guest port forward (repeatable), on top of the flavor's
    /// defaults. Prefix with a bind address (`BIND:HOST:GUEST`, e.g.
    /// `0.0.0.0:8080:80`) to make it reachable from the LAN instead of just
    /// localhost.
    #[arg(long = "port", value_name = "[BIND:]HOST:GUEST")]
    pub ports: Vec<PortForward>,

    /// Persist to a named volume (Linux flavors).
    #[arg(short = 'v', long, value_name = "NAME")]
    pub volume: Option<String>,

    /// Clone a git repo into the guest after boot and `cd` into it on shell open.
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorRmArgs {
    /// Remove even if not tracked (best-effort delete of its data).
    #[arg(short, long)]
    pub force: bool,

    /// Flavor name(s) to remove.
    #[arg(value_name = "NAME", required = true)]
    pub names: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct VolumeArgs {
    #[command(subcommand)]
    pub cmd: VolumeCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum VolumeCmd {
    /// List persistent volumes.
    Ls(VolumeLsArgs),
    /// Remove one or more volumes (and their data).
    Rm(VolumeRmArgs),
}

/// macOS formats the boot volume case-insensitively, which collapses nix store
/// paths that differ only by case and breaks every nix guest. These commands
/// `bsdkrun kvm` — the KVM readiness check. macOS uses Hypervisor.framework
/// (gated by an entitlement, not a device node), so there is nothing for this
/// to inspect there and the subcommand is not compiled on macOS.
#[cfg(target_os = "linux")]
#[derive(Parser, Serialize, Deserialize)]
pub struct KvmArgs {
    /// Emit JSON (for scripting / the desktop).
    #[arg(long)]
    pub json: bool,
}

/// macOS formats the boot volume case-insensitively, which collapses nix store
/// paths that differ only by case and breaks every nix guest. These commands
/// manage the case-sensitive APFS sparsebundle that holds image rootfs trees
/// and named volumes instead. Linux hosts are already case-sensitive and need
/// none of this, so the subcommand is not compiled there.
#[cfg(target_os = "macos")]
#[derive(Parser, Serialize, Deserialize)]
pub struct StoreArgs {
    #[command(subcommand)]
    pub cmd: StoreCmd,
}

#[cfg(target_os = "macos")]
#[derive(Subcommand, Serialize, Deserialize)]
pub enum StoreCmd {
    /// Create the case-sensitive store and move existing volumes onto it.
    Init(StoreInitArgs),
    /// Show whether a store exists, is attached, and how much disk it uses.
    Status,
    /// Attach the store (done automatically, but useful after a manual detach).
    Attach,
    /// Detach the store. Machines using it must be stopped first.
    Detach(StoreDetachArgs),
    /// Delete the store and everything on it.
    Rm(StoreRmArgs),
}

#[cfg(target_os = "macos")]
#[derive(Parser, Serialize, Deserialize)]
pub struct StoreInitArgs {
    /// Capacity ceiling for the store, e.g. `200g`. Sparse — a fresh store of
    /// any size occupies only ~24 MB until images are pulled into it.
    #[arg(long, default_value = store::DEFAULT_SIZE)]
    pub size: String,
}

#[cfg(target_os = "macos")]
#[derive(Parser, Serialize, Deserialize)]
pub struct StoreDetachArgs {
    /// Detach even if files on the store are still open.
    #[arg(short, long)]
    pub force: bool,
}

#[cfg(target_os = "macos")]
#[derive(Parser, Serialize, Deserialize)]
pub struct StoreRmArgs {
    /// Required: deleting the store destroys every cached image and every
    /// named volume living on it.
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct VolumeLsArgs {
    /// Emit the volume list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct ImagesArgs {
    /// Emit the image list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct VolumeRmArgs {
    /// Remove even if a running machine is using the volume.
    #[arg(short, long)]
    pub force: bool,

    /// Volume name(s) to remove.
    #[arg(value_name = "NAME", required = true)]
    pub names: Vec<String>,
}

#[derive(Parser, Default, Serialize, Deserialize)]
pub struct ExecArgs {
    /// Allocate a pseudo-TTY (interactive; like `docker exec -it`).
    #[arg(short = 't', long)]
    pub tty: bool,

    /// Set an environment variable in the command (repeatable), e.g. `-e K=V`.
    #[arg(short = 'e', long = "env", value_name = "K=V")]
    pub env: Vec<String>,

    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Command and arguments to run inside the guest.
    #[arg(value_name = "COMMAND", required = true, trailing_var_arg = true)]
    pub command: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct TailscaleArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Action + arguments for the in-guest agent:
    /// `setup [--authkey K] [--hostname H]`, `status`, `install`,
    /// `start [--kernel-tun]`. Extra `setup` args pass through to `tailscale up`.
    #[arg(
        value_name = "ACTION",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub args: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct SshArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

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
    pub args: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct SystemdArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Action for the in-guest agent: `setup` (install + mark for next boot),
    /// `status`, `disable`. Boot on a volume (-v) so the change persists.
    #[arg(
        value_name = "ACTION",
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub args: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct PsArgs {
    /// Show all machines (default shows only running ones).
    #[arg(short, long)]
    pub all: bool,

    /// Emit the machine list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct IdArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub cmd: AgentCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum AgentCmd {
    /// Download + install the current agent inside a running guest, over its
    /// existing (possibly outdated) agent. The next exec/ssh/tailscale spawns
    /// the fresh binary.
    Update(IdArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct RmArgs {
    /// Remove even if the machine is still running (stops it first).
    #[arg(short, long)]
    pub force: bool,

    /// machine id(s) to remove (a unique prefix is enough).
    #[arg(value_name = "ID", required = true)]
    pub ids: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct UpdateArgs {
    /// machine id to update (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

    /// New number of vCPUs.
    #[arg(long)]
    pub cpus: Option<u8>,

    /// New guest RAM in MiB.
    #[arg(long)]
    pub mem: Option<u32>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct LogsArgs {
    /// machine id (a unique prefix is enough).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Follow the live console output.
    #[arg(short, long)]
    pub follow: bool,

    /// Show bsdkrun's own boot log (libkrun diagnostics + boot errors) instead of
    /// the guest console — useful when a machine dies before producing console
    /// output. This is what `--log-level` writes for a detached machine.
    #[arg(long)]
    pub boot: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct LinuxArgs {
    /// OCI image reference, e.g. `alpine`, `alpine:3.20`, `ghcr.io/owner/name:tag`.
    #[arg(value_name = "IMAGE")]
    pub image: String,

    /// Kernel to boot (a path to an ELF vmlinux or raw arm64 Image). Overrides
    /// `--kernel-version`.
    #[arg(long)]
    pub kernel: Option<PathBuf>,

    /// vmlinux-builder release to download + boot (ignored if `--kernel` is
    /// given). See https://github.com/tsirysndr/vmlinux-builder/releases.
    #[arg(long, default_value = linux::DEFAULT_KERNEL_VERSION)]
    pub kernel_version: String,

    /// Run the machine in the background and print its id (like `docker run -d`).
    /// Use `logs`/`shell`/`stop` to interact with it afterwards.
    #[arg(short = 'd', long)]
    pub detach: bool,

    /// Boot from an initramfs (the whole rootfs is loaded into RAM) instead of
    /// the default virtio-fs (which serves the rootfs from disk — no RAM-size
    /// limit). Use this if the guest kernel lacks CONFIG_VIRTIO_FS.
    #[arg(long)]
    pub initramfs: bool,

    /// Persist the guest's rootfs to a named volume that survives reboots (like a
    /// Docker volume). First use CoW-clones the image rootfs; reuse the same name
    /// to keep your changes. Requires virtio-fs (not `--initramfs`, which is RAM).
    #[arg(short = 'v', long, value_name = "NAME")]
    pub volume: Option<String>,

    /// Bind-mount a host directory into the guest over virtio-fs (repeatable),
    /// like `docker run -v`. Format: `HOST:GUEST[:ro]` (append `:ro` for
    /// read-only). Linux guests only.
    #[arg(long = "mount", value_name = "HOST:GUEST[:ro]")]
    pub mounts: Vec<String>,

    /// Override the image's entrypoint (like `docker run --entrypoint`).
    #[arg(long)]
    pub entrypoint: Option<String>,

    /// Set an environment variable in the guest (repeatable), e.g. `-e K=V`.
    #[arg(short = 'e', long = "env", value_name = "K=V")]
    pub env: Vec<String>,

    /// Guest console device the kernel should log to. libkrun's native console
    /// is the virtio-console `hvc0`; use `ttyS0` only with a kernel/setup that
    /// expects libkrun's explicit 8250 serial instead.
    #[arg(long, default_value = "hvc0")]
    pub console: String,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,

    /// Clone a git repo into the guest after boot and `cd` into it when you open
    /// a shell (e.g. `--repo https://github.com/owner/name`).
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,

    /// Command (and args) to run instead of the image's default Cmd.
    /// Everything after `--` is passed through.
    #[arg(last = true, value_name = "CMD")]
    pub command: Vec<String>,
}

impl LinuxArgs {
    /// virtio-fs is the default; `--initramfs` opts out of it.
    #[cfg(feature = "boot")]
    pub(crate) fn virtiofs(&self) -> bool {
        !self.initramfs
    }
}

#[derive(Parser, Serialize, Deserialize)]
pub struct GrowArgs {
    /// Path to the raw disk image to enlarge.
    #[arg(long)]
    pub disk: PathBuf,

    /// New size, e.g. 8G, 4096M (only enlarges — never shrinks).
    #[arg(long)]
    pub size: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FetchArgs {
    /// Guest OS to fetch.
    #[arg(long, value_enum, default_value = "freebsd")]
    pub os: fetch::Os,

    /// Version to download. FreeBSD: a release like 15.1 (default: latest).
    /// NetBSD: a release like 10.1, or `current` (default: current).
    #[arg(long)]
    pub version: Option<String>,

    /// Directory to link the (cache-backed) image into.
    #[arg(long, default_value = "images")]
    pub dir: PathBuf,

    /// Re-download even if the image is already cached.
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct VersionsArgs {
    /// Guest OS to list builds for.
    #[arg(long, value_enum, default_value = "freebsd")]
    pub os: fetch::Os,
}

#[derive(Clone, Copy, ValueEnum, Serialize, Deserialize)]
pub enum KernelFormat {
    Raw,
    Elf,
}

impl KernelFormat {
    #[cfg(feature = "boot")]
    pub(crate) fn to_krun(self) -> u32 {
        match self {
            KernelFormat::Raw => krun::KRUN_KERNEL_FORMAT_RAW,
            KernelFormat::Elf => krun::KRUN_KERNEL_FORMAT_ELF,
        }
    }
}

#[derive(Parser, Serialize, Deserialize)]
pub struct KernelArgs {
    /// Path to the guest kernel image.
    #[arg(long)]
    pub kernel: PathBuf,

    /// Kernel image format.
    #[arg(long, value_enum, default_value = "elf")]
    pub format: KernelFormat,

    /// Optional initramfs/initrd.
    #[arg(long)]
    pub initramfs: Option<PathBuf>,

    /// Kernel command line.
    #[arg(long, default_value = "")]
    pub cmdline: String,

    /// Root disk image (raw), attached as virtio-blk.
    #[arg(long)]
    pub disk: Option<PathBuf>,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    pub attach_disk: Vec<DiskSpec>,

    #[command(flatten)]
    pub run: RunConfig,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,
}

/// Options for the `nanos` command.
#[derive(Parser, Default, Serialize, Deserialize)]
pub struct NanosArgs {
    /// The Nanos image to boot: a path, or a bare name looked up in
    /// `~/.ops/images/` (what `ops build -i <name>` produces).
    #[arg(value_name = "IMAGE")]
    pub image: String,

    /// Nanos kernel to load (Linux hosts; default: the newest
    /// `~/.ops/<version>/kernel.img` ops has staged).
    #[arg(long)]
    pub kernel: Option<PathBuf>,

    /// Kernel command line.
    #[arg(long, default_value = "")]
    pub cmdline: String,

    /// UEFI firmware (macOS hosts; default: krunkit's KRUN_EFI, auto-located).
    #[arg(long)]
    pub firmware: Option<PathBuf>,

    /// Run the machine in the background and print its id (like `docker run
    /// -d`). Use `logs`/`stop` to interact with it afterwards — a unikernel
    /// has no shell, so `shell`/`exec` don't apply.
    #[arg(short = 'd', long)]
    pub detach: bool,

    /// Boot the image in place (writes persist to it) instead of the default
    /// per-machine copy-on-write clone.
    #[arg(long)]
    pub persist: bool,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,
}

/// Options for the `osv` command.
#[derive(Parser, Serialize, Deserialize)]
pub struct OsvArgs {
    /// The OSv image to boot: an aarch64 loader from an OSv release (e.g.
    /// `osv-loader-microvm.qemu.aarch64`), or an image composed by `capstan`.
    /// A composed image is both the kernel and the root disk.
    #[arg(value_name = "IMAGE")]
    pub image: PathBuf,

    /// OSv command line — the application to run and its arguments, e.g.
    /// `/hello.so`. This is what the guest is actually booted with (via the
    /// device tree on aarch64, the PVH start_info on x86_64), so it overrides
    /// the copy baked into the image. Defaults to that baked-in command line.
    #[arg(long, default_value = "")]
    pub cmdline: String,

    /// Root disk image (raw), attached as virtio-blk. Required on x86_64, where
    /// the loader ELF is kernel only; on aarch64 it overrides the filesystem
    /// carried inside a composed image.
    #[arg(long)]
    pub disk: Option<PathBuf>,

    /// Boot the kernel alone, without attaching a root disk. OSv will need
    /// `--nomount` in its command line to get anywhere.
    #[arg(long)]
    pub no_disk: bool,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    pub attach_disk: Vec<DiskSpec>,

    /// Interrupt controller to ask libkrun for. OSv only grew a GICv3 driver
    /// after v0.57.0, so its released aarch64 kernel needs `v2`; pass `v3` for
    /// a kernel built from OSv master.
    #[arg(long, default_value = "v2", value_name = "v2|v3")]
    pub gic: osv::Gic,

    /// Boot the disk in place (writes persist to it; only one machine at a
    /// time) instead of the default per-machine copy-on-write clone.
    #[arg(long)]
    pub persist: bool,

    /// Persist the guest's disk to a named volume that survives reboots.
    #[arg(short = 'v', long = "volume", value_name = "NAME")]
    pub volume: Option<String>,

    /// Run the machine in the background and print its id (like `docker run -d`).
    /// Use `logs`/`stop` to interact with it afterwards — a unikernel has no
    /// shell, so `shell`/`exec` don't apply.
    #[arg(short = 'd', long)]
    pub detach: bool,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,
}

/// Options for the `unikraft` command.
#[derive(Parser, Serialize, Deserialize)]
pub struct UnikraftArgs {
    /// The unikernel to boot: an image built by `kraft` (the raw
    /// `<name>_fc-<arch>` image, or the `.dbg` ELF beside it), or a project
    /// directory whose `.unikraft/build/` holds one.
    #[arg(value_name = "IMAGE|DIR", default_value = ".")]
    pub path: PathBuf,

    /// Kernel command line. Unikraft reads it from the device tree and hands it
    /// to the application as `argv` — the first word is `argv[0]`.
    #[arg(long, default_value = "")]
    pub cmdline: String,

    /// Optional initrd, for a unikernel built with an initrd-backed rootfs
    /// (`kraft build --rootfs`).
    #[arg(long)]
    pub initramfs: Option<PathBuf>,

    /// Share a host directory into the unikernel over virtio-fs, as
    /// HOST:GUEST (repeatable). The guest path must be absolute. Requires a
    /// unikernel built with CONFIG_LIBUKFS_VIRTIOFS and
    /// CONFIG_LIBPOSIX_VFS_FSTAB_USER — see examples/unikraft-volume.
    #[arg(long = "mount", value_name = "HOST:GUEST")]
    pub mount: Vec<String>,

    /// Run the machine in the background and print its id (like `docker run -d`).
    /// Use `logs`/`stop` to interact with it afterwards — a unikernel has no
    /// shell, so `shell`/`exec` don't apply.
    #[arg(short = 'd', long)]
    pub detach: bool,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FirmwareArgs {
    /// Path to the UEFI firmware image (e.g. edk2/AAVMF for aarch64).
    #[arg(long)]
    pub firmware: PathBuf,

    /// Root disk image (raw), attached as virtio-blk.
    #[arg(long)]
    pub disk: PathBuf,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    pub attach_disk: Vec<DiskSpec>,

    #[command(flatten)]
    pub run: RunConfig,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,
}

/// Machine lifecycle options shared by `firmware` and `kernel`.
#[derive(Args, Default, Serialize, Deserialize)]
pub struct RunConfig {
    /// Run the machine in the background and print its id (like `docker run -d`).
    /// Use `logs`/`shell`/`stop` to interact with it afterwards.
    #[arg(short = 'd', long)]
    pub detach: bool,

    /// Boot the disk in place (writes persist to it; only one machine at a time)
    /// instead of the default per-machine APFS copy-on-write clone.
    #[arg(long, conflicts_with = "volume")]
    pub persist: bool,

    /// Persist the guest's disk to a named volume that survives reboots (like a
    /// Docker volume). First use CoW-clones the base; reuse the same name to keep
    /// your changes. Stored under `<state>/volumes/<NAME>`.
    #[arg(short = 'v', long, value_name = "NAME")]
    pub volume: Option<String>,
}

/// Options for the `freebsd` / `netbsd` shortcut commands.
#[derive(Parser, Default, Serialize, Deserialize)]
pub struct BsdArgs {
    /// Version to run. FreeBSD: a release like 15.1 (default: latest).
    /// NetBSD: a release like 10.1, or `current` (default: current).
    #[arg(long)]
    pub version: Option<String>,

    /// UEFI firmware to boot with (default: krunkit's KRUN_EFI, auto-located).
    #[arg(long)]
    pub firmware: Option<PathBuf>,

    /// Re-download even if the image is already cached.
    #[arg(short, long)]
    pub force: bool,

    /// Additional disk to attach as virtio-blk (repeatable).
    /// Format: PATH[:ro] — append `:ro` for a read-only attachment.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    pub attach_disk: Vec<DiskSpec>,

    #[command(flatten)]
    pub run: RunConfig,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,

    /// Grow the guest's root disk to this size before boot (only enlarges),
    /// e.g. `8G`, `4096M`. The guest expands its root FS on first boot.
    #[arg(long, value_name = "SIZE")]
    pub disk_size: Option<String>,

    /// Stream the guest's boot console live while waiting for its agent (instead
    /// of the terse "waiting…" line), so you see the full BSD boot. The command
    /// output / shell follows once the agent is up.
    #[arg(long)]
    pub verbose: bool,

    /// Clone a git repo into the guest after boot and `cd` into it on shell open
    /// (installs git via pkg/pkgin/pkg_add if needed).
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,

    /// Command (and args) to run inside the guest via its agent once it's
    /// booted, like `bsdkrun linux`. Everything after `--` is passed through.
    /// Without `-d` this is one-shot: the guest boots, runs the command
    /// (streaming its output), then powers off, and bsdkrun exits with the
    /// command's status. With `-d` the machine is left running afterward.
    /// Needs networking (the agent) — incompatible with `--no-net`.
    #[arg(last = true, value_name = "CMD")]
    pub command: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct VmConfig {
    /// Number of vCPUs.
    #[arg(long, default_value_t = 1)]
    pub cpus: u8,

    /// Guest RAM in MiB.
    #[arg(long, default_value_t = 512)]
    pub mem: u32,
}

/// User-mode networking options (shared by `kernel` and `firmware`).
///
/// Networking is on by default: the guest gets a virtio-net NIC wired to
/// gvproxy, which NATs it out to the host's network (internet access via DHCP
/// on 192.168.127.0/24). Pass `--no-net` for an isolated guest.
#[derive(Args, Default, Serialize, Deserialize)]
pub struct NetConfig {
    /// Disable networking (boot the guest with no NIC).
    #[arg(long = "no-net")]
    pub no_net: bool,

    /// Forward a host TCP port to the guest: HOST:GUEST (repeatable).
    /// Example: `--port 2222:22` for SSH. Binds `127.0.0.1` by default;
    /// prefix a bind address to change that, e.g. `--port 0.0.0.0:8080:80`
    /// to make it reachable from the LAN instead of just localhost.
    #[arg(long = "port", value_name = "[BIND:]HOST:GUEST")]
    pub ports: Vec<PortForward>,

    /// MAC address for the guest NIC (default: a fixed locally-administered one).
    #[arg(long, value_name = "AA:BB:CC:DD:EE:FF")]
    pub mac: Option<String>,

    /// Join a global network so the machine shares a subnet with, and can reach
    /// (by IP + name), other members (`bsdkrun network create <name>` first).
    #[arg(long, value_name = "NAME")]
    pub network: Option<String>,

    /// Name for this machine (used as its DNS name on a `--network`, and shown in
    /// `ps`). Defaults to a generated Docker-style name.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
}

/// A disk to attach as virtio-blk, parsed from `PATH[:ro]`.
#[derive(Clone, Serialize, Deserialize)]
pub struct DiskSpec {
    pub path: PathBuf,
    pub read_only: bool,
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

    /// `HOST:GUEST` (binds `127.0.0.1`, the default), or `BIND:HOST:GUEST` to
    /// choose the host interface explicitly — e.g. `0.0.0.0:8080:80` to make
    /// the forward reachable from the LAN, not just localhost.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        let (bind, host, guest) = match parts.as_slice() {
            [host, guest] => (None, *host, *guest),
            [bind, host, guest] => (Some(*bind), *host, *guest),
            _ => return Err(format!("expected HOST:GUEST or BIND:HOST:GUEST, got {s:?}")),
        };
        let bind = match bind {
            Some(b) => b
                .parse::<std::net::IpAddr>()
                .map_err(|_| format!("invalid bind address {b:?} in {s:?}"))?,
            None => std::net::Ipv4Addr::LOCALHOST.into(),
        };
        let host = host
            .parse::<u16>()
            .map_err(|_| format!("invalid host port {host:?} in {s:?}"))?;
        let guest = guest
            .parse::<u16>()
            .map_err(|_| format!("invalid guest port {guest:?} in {s:?}"))?;
        Ok(PortForward { bind, host, guest })
    }
}

// ---------------------------------------------------------------------------
// defaults
// ---------------------------------------------------------------------------
//
// clap fills these in from `default_value` when it parses a command line. A
// caller that builds one of these structs directly — the daemon — gets nothing
// from clap, so the defaults are restated here and every such struct starts
// from `Default::default()`. Without this a daemon-booted machine would come up
// with an empty console device, no kernel version and 0 vCPUs, and the
// divergence would only show at boot.

impl Default for VmConfig {
    fn default() -> Self {
        Self { cpus: 1, mem: 512 }
    }
}

impl Default for LinuxArgs {
    fn default() -> Self {
        Self {
            image: String::new(),
            kernel: None,
            kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
            detach: false,
            initramfs: false,
            volume: None,
            mounts: vec![],
            entrypoint: None,
            env: vec![],
            console: "hvc0".to_string(),
            net: NetConfig::default(),
            vm: VmConfig::default(),
            repo: None,
            command: vec![],
        }
    }
}

impl Default for UnikraftArgs {
    fn default() -> Self {
        Self {
            path: PathBuf::from("."),
            cmdline: String::new(),
            initramfs: None,
            mount: vec![],
            detach: false,
            net: NetConfig::default(),
            vm: VmConfig::default(),
        }
    }
}

impl Default for OsvArgs {
    fn default() -> Self {
        Self {
            image: PathBuf::new(),
            cmdline: String::new(),
            disk: None,
            no_disk: false,
            attach_disk: vec![],
            gic: osv::Gic::default(),
            persist: false,
            volume: None,
            detach: false,
            net: NetConfig::default(),
            vm: VmConfig::default(),
        }
    }
}

impl Default for FetchArgs {
    fn default() -> Self {
        Self {
            os: fetch::Os::Freebsd,
            version: None,
            dir: PathBuf::from("images"),
            force: false,
        }
    }
}

impl Default for FlavorAddArgs {
    fn default() -> Self {
        Self {
            name: String::new(),
            base: String::new(),
            category: "custom".to_string(),
            description: String::new(),
            ports: vec![],
            env: vec![],
            nix: vec![],
            provision: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every boot struct a non-clap caller builds has to agree with what clap
    /// would have produced. These compare a bare command line against
    /// `Default::default()` field for field, so adding a `default_value` to an
    /// argument without restating it above fails here rather than at boot.
    fn json<T: Serialize>(v: &T) -> String {
        serde_json::to_string_pretty(v).unwrap()
    }

    fn parse(argv: &[&str]) -> Command {
        Cli::parse_from(argv).cmd
    }

    #[test]
    fn linux_defaults_match_clap() {
        let Command::Linux(parsed) = parse(&["bsdkrun", "linux", "alpine"]) else {
            panic!("not a linux command");
        };
        let built = LinuxArgs {
            image: "alpine".to_string(),
            ..Default::default()
        };
        assert_eq!(json(&parsed), json(&built));
    }

    #[test]
    fn bsd_defaults_match_clap() {
        let Command::Freebsd(parsed) = parse(&["bsdkrun", "freebsd"]) else {
            panic!("not a freebsd command");
        };
        assert_eq!(json(&parsed), json(&BsdArgs::default()));
    }

    #[test]
    fn unikraft_defaults_match_clap() {
        let Command::Unikraft(parsed) = parse(&["bsdkrun", "unikraft"]) else {
            panic!("not a unikraft command");
        };
        assert_eq!(json(&parsed), json(&UnikraftArgs::default()));
    }

    #[test]
    fn osv_defaults_match_clap() {
        let Command::Osv(parsed) = parse(&["bsdkrun", "osv", "loader.img"]) else {
            panic!("not an osv command");
        };
        let built = OsvArgs {
            image: PathBuf::from("loader.img"),
            ..Default::default()
        };
        assert_eq!(json(&parsed), json(&built));
    }

    #[test]
    fn nanos_defaults_match_clap() {
        let Command::Nanos(parsed) = parse(&["bsdkrun", "nanos", "prog"]) else {
            panic!("not a nanos command");
        };
        let built = NanosArgs {
            image: "prog".to_string(),
            ..Default::default()
        };
        assert_eq!(json(&parsed), json(&built));
    }

    #[test]
    fn flavor_run_defaults_match_clap() {
        let Command::Flavor(args) = parse(&["bsdkrun", "flavor", "run", "node"]) else {
            panic!("not a flavor command");
        };
        let FlavorCmd::Run(parsed) = args.cmd else {
            panic!("not a flavor run");
        };
        let built = FlavorRunArgs {
            name: "node".to_string(),
            ..Default::default()
        };
        assert_eq!(json(&parsed), json(&built));
    }

    #[test]
    fn flavor_add_defaults_match_clap() {
        let Command::Flavor(args) =
            parse(&["bsdkrun", "flavor", "add", "mine", "--base", "alpine"])
        else {
            panic!("not a flavor command");
        };
        let FlavorCmd::Add(parsed) = args.cmd else {
            panic!("not a flavor add");
        };
        let built = FlavorAddArgs {
            name: "mine".to_string(),
            base: "alpine".to_string(),
            ..Default::default()
        };
        assert_eq!(json(&parsed), json(&built));
    }

    /// The supervisor hands a whole command between processes as JSON, so a
    /// round trip has to be lossless.
    #[test]
    fn a_command_survives_a_json_round_trip() {
        let cmd = parse(&[
            "bsdkrun", "linux", "-d", "--cpus", "4", "--mem", "2048", "--port", "8080:80", "-e",
            "FOO=bar", "alpine", "--", "sh", "-c", "echo hi",
        ]);
        let wire = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&wire).unwrap();
        assert_eq!(wire, serde_json::to_string(&back).unwrap());

        let Command::Linux(a) = back else {
            panic!("round trip changed the command")
        };
        assert_eq!(a.vm.cpus, 4);
        assert_eq!(a.vm.mem, 2048);
        assert!(a.detach);
        assert_eq!(a.net.ports.len(), 1);
        assert_eq!(a.command, ["sh", "-c", "echo hi"]);
    }

    #[test]
    fn port_forward_defaults_to_loopback() {
        let pf: PortForward = "8080:80".parse().unwrap();
        assert_eq!(pf.bind, std::net::Ipv4Addr::LOCALHOST);
        assert_eq!(pf.host, 8080);
        assert_eq!(pf.guest, 80);
    }

    #[test]
    fn port_forward_accepts_an_explicit_bind_address() {
        let pf: PortForward = "0.0.0.0:8080:80".parse().unwrap();
        assert_eq!(pf.bind, std::net::Ipv4Addr::UNSPECIFIED);
        assert_eq!(pf.host, 8080);
        assert_eq!(pf.guest, 80);
    }

    #[test]
    fn port_forward_rejects_garbage() {
        assert!("8080".parse::<PortForward>().is_err());
        assert!("not-an-ip:8080:80".parse::<PortForward>().is_err());
        assert!("a:b:c:d".parse::<PortForward>().is_err());
    }
}
