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
    /// needs the patched Nanos kernel staged — see examples/nanos-hello.
    Nanos(NanosArgs),

    /// Boot an OSv unikernel image (an OSv release loader, or one composed by
    /// `capstan`).
    Osv(OsvArgs),

    /// Run a Solo5 unikernel — MirageOS and anything else built for the `hvt`
    /// target — with the embedded `solo5-hvt` tender.
    ///
    /// Aliased as `mirage`, since that is what most people are running.
    //
    // Not gated on the `solo5` feature (a doc comment here would land in
    // `--help`): like every boot command, the *definition* stays available so
    // a non-booting consumer (the daemon) can describe the command and hand it
    // to `bsdkrun-supervisor`. Only the implementation — the embedded tender —
    // is feature-gated.
    #[command(visible_alias = "mirage")]
    Solo5(Solo5Args),

    /// List machines.
    Ps(PsArgs),

    /// List downloaded images.
    Images(ImagesArgs),

    /// Manage images (`image rm <id>`).
    Image(ImageArgs),

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

    /// Copy files between the host and a running machine (like `docker cp`).
    Cp(CpArgs),

    /// Save and restore a guest directory under a key (host disk or S3).
    Cache(CacheArgs),

    /// Check that this host can run machines, and say what to fix if not.
    Doctor(DoctorArgs),

    /// Manage tailscale inside a running machine (install/start/status/setup).
    Tailscale(TailscaleArgs),

    /// Set up key-based SSH inside a running machine (setup/add-key/status).
    Ssh(SshArgs),

    /// Configure systemd as PID 1 in a Linux guest (setup/status/disable).
    Systemd(SystemdArgs),

    /// Manage persistent volumes (list / remove).
    Volume(VolumeArgs),

    /// Manage the case-sensitive Linux rootfs store (macOS only).
    #[cfg(target_os = "macos")]
    Store(StoreArgs),

    /// Snapshot a machine's current state into a named flavor (like `docker commit`).
    Commit(CommitArgs),

    /// Run a Docker engine in a microVM and serve its API on a host socket, so
    /// the local `docker` CLI drives it — a Docker Desktop replacement.
    Docker(DockerArgs),

    /// Run an AI coding agent in a sandbox VM (`ai agents` / `ai ls` / …).
    Ai(AiArgs),

    /// Claude Code in a sandbox, sharing this directory.
    Claude(AiRunArgs),
    /// OpenAI Codex in a sandbox, sharing this directory.
    Codex(AiRunArgs),
    /// Gemini CLI in a sandbox, sharing this directory.
    Gemini(AiRunArgs),
    /// OpenCode in a sandbox, sharing this directory.
    Opencode(AiRunArgs),
    /// Crush in a sandbox, sharing this directory.
    Crush(AiRunArgs),
    /// GitHub Copilot CLI in a sandbox, sharing this directory.
    Copilot(AiRunArgs),
    /// Kilo Code in a sandbox, sharing this directory.
    Kilo(AiRunArgs),
    /// Qwen Code in a sandbox, sharing this directory.
    Qwen(AiRunArgs),

    /// Save a machine's current disk state under a name (`snapshot <ID> [NAME]`),
    /// or manage saved ones (`snapshot ls` / `snapshot rm`).
    Snapshot(SnapshotArgs),

    /// List saved snapshots — all of them, or one machine's.
    Snapshots(SnapshotLsArgs),

    /// Boot a new machine from a snapshot — or from a machine, which is
    /// snapshotted first. Either way the original is untouched: a branch is a
    /// copy-on-write copy of a machine's state.
    Branch(BranchArgs),

    /// Put a machine's disk state back to one of its snapshots.
    Restore(RestoreArgs),

    /// Restore a machine to its most recent snapshot (`restore`, without having
    /// to name it).
    Rollback(RollbackArgs),

    /// List flavors: the built-in catalog + your saved snapshots.
    Flavors(FlavorsListArgs),

    /// Run / remove flavors (`flavor run <name>`, `flavor rm <name>`).
    Flavor(FlavorArgs),

    /// Manage global networks (a shared subnet + internal DNS) that machines join
    /// with `--network` to reach each other by IP and by name.
    Network(NetworkArgs),

    /// Local machine domains: serve every machine at https://<name>.<tld> on
    /// this host — a built-in DNS responder plus a Caddy reverse proxy with a
    /// locally-trusted CA.
    Domains(DomainsArgs),

    /// Open the interactive terminal dashboard (machines, images, volumes,
    /// networks — with live status, logs, and actions).
    Tui(TuiArgs),

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
pub struct DomainsArgs {
    #[command(subcommand)]
    pub cmd: DomainsCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum DomainsCmd {
    /// Turn on machine domains: start the DNS responder, wire the system
    /// resolver for the TLD, start Caddy, and trust its local CA. Idempotent —
    /// re-running repairs whatever is missing.
    Enable(DomainsEnableArgs),
    /// Stop the DNS responder and the proxy. `--purge` also removes the
    /// resolver wiring and untrusts the CA.
    Disable(DomainsDisableArgs),
    /// Show each component's health (DNS, resolver, Caddy, CA trust).
    Status(DomainsStatusArgs),
    /// List machine domains: NAME → URL → upstream port.
    Ls(DomainsLsArgs),
    /// Regenerate the proxy config from the machine list and reload Caddy.
    Sync,
    /// Print the local CA's root certificate path — point tools that don't read
    /// the system trust store at it (e.g. `http --verify "$(bsdkrun domains ca)"
    /// https://web.bsdk`, or `export REQUESTS_CA_BUNDLE=...`). `--pem` prints the
    /// certificate itself instead of the path.
    Ca(DomainsCaArgs),
    /// The detached DNS responder process (not for direct use).
    #[command(hide = true, name = "__serve-dns")]
    ServeDns(ServeDnsArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DomainsCaArgs {
    /// Print the PEM certificate to stdout instead of its path.
    #[arg(long)]
    pub pem: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DomainsEnableArgs {
    /// TLD for machine domains (machines become https://<name>.<tld>).
    #[arg(long, default_value = "bsdk")]
    pub tld: String,

    /// Host port Caddy serves HTTPS on.
    #[arg(long, default_value_t = 443)]
    pub https_port: u16,

    /// Host port Caddy serves the HTTP→HTTPS redirect on.
    #[arg(long, default_value_t = 80)]
    pub http_port: u16,

    /// Skip installing Caddy's root CA into the system trust store.
    #[arg(long)]
    pub no_trust: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DomainsDisableArgs {
    /// Also remove the resolver wiring (/etc/resolver file or resolved
    /// drop-in) and remove the CA from the trust store.
    #[arg(long)]
    pub purge: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DomainsStatusArgs {
    /// Emit the status as JSON (for scripting / the TUI).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DomainsLsArgs {
    /// Emit the domain list as a JSON array.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct ServeDnsArgs {
    /// UDP port to listen on (loopback only).
    #[arg(long)]
    pub port: u16,

    /// The zone to answer for.
    #[arg(long)]
    pub tld: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct TuiArgs {}

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
pub struct ImageArgs {
    #[command(subcommand)]
    pub cmd: ImageCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum ImageCmd {
    /// Remove dangling images (ones no machine uses) and their rootfs.
    Rm(ImageRmArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct ImageRmArgs {
    /// Image id, a unique prefix, or its reference.
    #[arg(value_name = "IMAGE", required = true)]
    pub ids: Vec<String>,

    /// Remove even when machines still reference it. They will not boot.
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiArgs {
    #[command(subcommand)]
    pub cmd: AiCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum AiCmd {
    /// List the agents and whether each one is installed.
    Agents(AiAgentsArgs),
    /// Start (or reuse) a sandbox and attach to the agent's TUI.
    Start(AiStartArgs),
    /// List agent sandboxes.
    Ls(AiLsArgs),
    /// Stop an agent's sandboxes. Its saved login survives.
    Stop(AiAgentArgs),
    /// Remove an agent's sandboxes and (unless `--keep-home`) its saved login.
    Rm(AiRmArgs),
    /// (internal) Print the argv that starts an agent's TUI, as JSON.
    ///
    /// The desktop app opens its terminal with this rather than rebuilding the
    /// wrapper (skills symlink, `cd`, `exec`) — the daemon's `aiShellCommand`
    /// query answers from the same function, so there is one definition.
    #[command(name = "__shell-command", hide = true)]
    ShellCommand(AiShellCommandArgs),
    /// Copy local files into a sandbox on a remote engine.
    ///
    /// Every other path here resolves on the machine running the engine, so
    /// driving a VPS leaves your skills, keys and project on the laptop where
    /// no sandbox can see them. This sends them across.
    Upload(AiUploadArgs),
    /// (internal) Receive an upload on the engine's host, as a tar on stdin.
    #[command(name = "__receive", hide = true)]
    Receive(AiReceiveArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiUploadArgs {
    /// What to send: `skills`, `ssh`, `git`, or `workspace` (the current
    /// directory). `git` is your `user.name` and `user.email`.
    #[arg(long, value_name = "KIND", default_value = "workspace")]
    pub what: String,

    /// The agent whose sandbox receives it. `ssh` lands in that agent's home
    /// volume; `skills` is shared by every agent regardless.
    #[arg(long, default_value = "claude")]
    pub agent: String,

    /// The directory to upload, for `--what workspace`. Defaults to the
    /// current directory.
    #[arg(value_name = "DIR")]
    pub dir: Option<String>,

    /// Name the uploaded workspace on the engine. Defaults to the directory's
    /// own name.
    #[arg(long)]
    pub name: Option<String>,

    /// Include build output (`node_modules`, `target`, `.venv`, …), which is
    /// skipped by default because it is large and regenerated anyway.
    #[arg(long)]
    pub all: bool,

    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiReceiveArgs {
    /// `skills`, `ssh`, `git` or `workspace`.
    #[arg(long, value_name = "KIND")]
    pub what: String,

    #[arg(long, default_value = "claude")]
    pub agent: String,

    /// The workspace name, for `--what workspace`.
    #[arg(long)]
    pub name: Option<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiShellCommandArgs {
    #[arg(value_name = "AGENT")]
    pub agent: String,

    /// The sandbox whose recorded workspace the command should `cd` into.
    #[arg(value_name = "MACHINE")]
    pub machine: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiAgentsArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiLsArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiAgentArgs {
    /// Agent id (`claude`, `codex`, `gemini`, …).
    #[arg(value_name = "AGENT")]
    pub agent: String,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiRmArgs {
    #[arg(value_name = "AGENT")]
    pub agent: String,

    /// Keep the volume holding the agent's login, so a later run doesn't
    /// have to authenticate again.
    #[arg(long)]
    pub keep_home: bool,
}

/// `bsdkrun claude` and friends: everything `ai start` takes, minus the agent
/// (the subcommand names it).
#[derive(Parser, Serialize, Deserialize)]
pub struct AiRunArgs {
    #[command(flatten)]
    pub vm: VmConfig,

    /// Share this directory instead of the current one. Mounted at the same
    /// path in the sandbox, so paths mean the same thing on both sides.
    #[arg(long, value_name = "PATH")]
    pub workspace: Option<String>,

    /// Share nothing — a sandbox that cannot see any of your files.
    #[arg(long, conflicts_with = "workspace")]
    pub no_workspace: bool,

    /// Boot a second sandbox instead of reusing the running one. The agent's
    /// saved login is shared between them.
    #[arg(long)]
    pub new: bool,

    /// Name this session, e.g. `--name refactor-auth`. Shown in `ai ls` and in
    /// the desktop's session switcher.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Group this session under a project. Defaults to the shared folder's
    /// name, so sessions on one codebase group together on their own.
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,

    /// Do not share the host's `~/.ssh`.
    ///
    /// It is shared read-only by default so `git push` works with the keys you
    /// already use. An agent can *read* a private key through it, so a sandbox
    /// you do not fully trust should opt out and clone over HTTPS.
    #[arg(long)]
    pub no_ssh: bool,

    /// Clone a git repository into the sandbox and start the agent in it.
    ///
    /// The clone happens *inside* the sandbox, so it needs no access to your
    /// filesystem — which makes it the natural way to give an agent a codebase
    /// when the engine is remote.
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,

    /// Start it in the background and print its id, instead of attaching.
    #[arg(short = 'd', long)]
    pub detach: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct AiStartArgs {
    /// Agent id (`claude`, `codex`, `gemini`, …).
    #[arg(value_name = "AGENT", default_value = "claude")]
    pub agent: String,

    #[command(flatten)]
    pub vm: VmConfig,

    /// Share this directory. Mounted at the same path in the sandbox.
    ///
    /// **Resolved on the engine's host.** Driving a remote `bsdkrund` (a VPS),
    /// this names a directory *there* — your laptop's filesystem is not
    /// reachable from it.
    #[arg(long, value_name = "PATH")]
    pub workspace: Option<String>,

    /// Share nothing — a sandbox that cannot see any of your files.
    ///
    /// Without it, a `bsdkrun ai start` typed at a terminal shares the current
    /// directory, exactly as `bsdkrun claude` does. A *daemon* builds this
    /// struct directly and leaves `cwd` false: its working directory is not
    /// the caller's, and silently sharing it would be a surprise at best.
    #[arg(long, conflicts_with = "workspace")]
    pub no_workspace: bool,

    /// Share the current directory. Set by the CLI (including the per-agent
    /// aliases); never by a daemon.
    #[arg(skip)]
    pub cwd: bool,

    /// Boot a second sandbox instead of reusing the running one.
    #[arg(long)]
    pub new: bool,

    /// Name this session, e.g. `--name refactor-auth`. Shown in `ai ls` and in
    /// the desktop's session switcher; also used for the machine's name.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Group this session under a project. Defaults to the shared folder's
    /// name, so sessions on one codebase group together on their own.
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,

    /// Do not share the host's `~/.ssh`.
    ///
    /// It is shared read-only by default so `git push` works with the keys you
    /// already use. An agent can *read* a private key through it, so a sandbox
    /// you do not fully trust should opt out and clone over HTTPS.
    #[arg(long)]
    pub no_ssh: bool,

    /// Clone a git repository into the sandbox and start the agent in it.
    ///
    /// The clone happens *inside* the sandbox, so it needs no access to your
    /// filesystem — which makes it the natural way to give an agent a codebase
    /// when the engine is remote.
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,

    /// Start it in the background and print its id, instead of attaching.
    #[arg(short = 'd', long)]
    pub detach: bool,
}

impl AiRunArgs {
    /// The `ai start` form of a per-agent alias. The alias shares the current
    /// directory unless told otherwise — that is the whole reason it exists.
    pub fn into_start(self, agent: &str) -> AiStartArgs {
        AiStartArgs {
            agent: agent.to_string(),
            vm: self.vm,
            cwd: !self.no_workspace && self.workspace.is_none(),
            workspace: self.workspace,
            no_workspace: self.no_workspace,
            new: self.new,
            name: self.name,
            project: self.project,
            repo: self.repo,
            no_ssh: self.no_ssh,
            detach: self.detach,
        }
    }
}

impl AiStartArgs {
    /// Resolve "share the current directory" for a CLI invocation: default on,
    /// off with `--no-workspace` or an explicit `--workspace`.
    pub fn with_cli_cwd(mut self) -> Self {
        self.cwd = !self.no_workspace && self.workspace.is_none();
        self
    }
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerArgs {
    #[command(subcommand)]
    pub cmd: DockerCmd,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum DockerCmd {
    /// Start (or resume) the Docker engine VM and wire up the host socket.
    Start(DockerStartArgs),
    /// Stop the engine. Images and containers stay on its disk.
    Stop,
    /// Show whether the engine is running, and how to reach it.
    Status(DockerStatusArgs),
    /// Remove the engine VM, its image store, and the docker context.
    Rm(DockerRmArgs),
    /// List containers (like `docker ps`).
    Ps(DockerPsArgs),
    /// Act on a container: start / stop / restart / kill / pause / unpause / rm.
    Container(DockerContainerArgs),
    /// Print a container's logs.
    Logs(DockerLogsArgs),
    /// Show or grow the image store's disk.
    Disk(DockerDiskArgs),
    /// Print the `DOCKER_HOST` line for a shell that isn't using a context.
    Env,
    /// Open a shell **in the engine VM** (not in a container).
    Shell,
    /// (internal) The detached socket proxy + port publisher.
    #[command(name = "__serve", hide = true)]
    Serve(DockerServeArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerStartArgs {
    #[command(flatten)]
    pub vm: VmConfig,

    /// Share another host directory into the VM, so `-v` can reach it.
    /// `PATH` mounts at the same path in the guest; `HOST:GUEST` picks one.
    #[arg(long = "mount", value_name = "PATH|HOST:GUEST")]
    pub mounts: Vec<String>,

    /// Do not share `$HOME` (the default). Containers then only see what
    /// `--mount` names.
    #[arg(long)]
    pub no_home: bool,

    /// Where a published container port is bound on the host: `mirror` (what
    /// the container asked for, Docker Desktop's behaviour) or a fixed address
    /// such as `127.0.0.1`.
    #[arg(long, value_name = "IP|mirror", default_value = "mirror")]
    pub publish_bind: String,

    /// Extra host↔guest forward for the VM itself (repeatable). Container
    /// ports are published automatically and need none of these.
    #[arg(long = "port", value_name = "[BIND:]HOST:GUEST")]
    pub ports: Vec<PortForward>,

    /// Give the image store a dedicated ext4 disk of this size (`60G`) instead
    /// of the host-backed rootfs. Sparse, so it costs what it holds; grow it
    /// later with `bsdkrun docker disk --size`. Only applies when the VM is
    /// created — an existing one keeps whatever it was made with.
    #[arg(long, value_name = "SIZE")]
    pub disk_size: Option<String>,

    /// Also point `/var/run/docker.sock` at this engine (asks for sudo), for
    /// tools that hardcode it.
    #[arg(long)]
    pub system_socket: bool,

    /// Do not create a `bsdkrun` docker context.
    #[arg(long)]
    pub no_context: bool,

    /// Create the context but leave the active one alone.
    #[arg(long)]
    pub no_activate: bool,

    /// How long to wait for dockerd to answer, in seconds.
    #[arg(long, default_value_t = 120)]
    pub timeout: u32,

    /// Print the resulting status as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerStatusArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerRmArgs {
    /// Remove it even if the engine is running (stops it first).
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerPsArgs {
    /// Include stopped containers.
    #[arg(short, long)]
    pub all: bool,

    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerContainerArgs {
    /// start | stop | restart | kill | pause | unpause | rm
    #[arg(value_name = "ACTION")]
    pub action: String,

    /// Container id(s) or name(s).
    #[arg(value_name = "CONTAINER", required = true)]
    pub ids: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerLogsArgs {
    /// Container id or name.
    #[arg(value_name = "CONTAINER")]
    pub id: String,

    /// How many trailing lines to show.
    #[arg(long, default_value_t = 200)]
    pub tail: u32,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerDiskArgs {
    /// Grow the image store to this size (e.g. `100G`). Growing only ever
    /// enlarges; the guest picks the new size up immediately when it is
    /// running, and on its next boot otherwise.
    #[arg(long, value_name = "SIZE")]
    pub size: Option<String>,

    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct DockerServeArgs {
    /// Host port forwarded to the guest's dockerd.
    #[arg(long)]
    pub port: u16,

    /// The engine VM's machine id (whose gvproxy publishes container ports).
    #[arg(long)]
    pub machine: String,

    #[arg(long, default_value = "mirror")]
    pub publish_bind: String,
}

/// `bsdkrun snapshot` — one verb with two shapes: a bare `snapshot <ID> [NAME]`
/// takes one, and `snapshot ls` / `snapshot rm` manage the saved ones.
///
/// `args_conflicts_with_subcommands` is what lets both live under one name: it
/// tells clap that `snapshot ls` is the subcommand, not a machine called "ls".
#[derive(Parser, Serialize, Deserialize)]
#[command(args_conflicts_with_subcommands = true)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub cmd: Option<SnapshotCmd>,

    /// Machine to snapshot (id, name, or a unique id prefix).
    #[arg(value_name = "ID")]
    pub id: Option<String>,

    /// Name for the snapshot. Defaults to `<machine>-<n>`.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Optional description.
    #[arg(short, long, default_value = "")]
    pub description: String,

    /// Print the snapshot as JSON (for scripting / the SDK).
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Serialize, Deserialize)]
pub enum SnapshotCmd {
    /// List saved snapshots (all, or one machine's).
    Ls(SnapshotLsArgs),
    /// Remove saved snapshots and their data.
    Rm(SnapshotRmArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct SnapshotLsArgs {
    /// Only this machine's snapshots (id, name, or unique id prefix).
    #[arg(value_name = "MACHINE")]
    pub machine: Option<String>,

    /// Emit the list as a JSON array (for scripting / the SDK).
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct SnapshotRmArgs {
    /// Snapshot name(s) or id(s) to remove.
    #[arg(value_name = "SNAPSHOT", required = true)]
    pub names: Vec<String>,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct BranchArgs {
    /// What to branch from: a snapshot (name, id, or unique id prefix), or a
    /// machine — which is snapshotted first, then branched.
    #[arg(value_name = "SNAPSHOT|MACHINE")]
    pub snapshot: String,

    /// Name for the new machine. Defaults to a generated one.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Run detached in the background (like `docker run -d`).
    #[arg(short = 'd', long)]
    pub detach: bool,

    /// vCPUs for the branch. Defaults to what the snapshot recorded.
    #[arg(long)]
    pub cpus: Option<u8>,

    /// Guest RAM in MiB. Defaults to what the snapshot recorded.
    #[arg(long)]
    pub mem: Option<u32>,

    /// Host↔guest port forward (repeatable). Given at least once, these
    /// *replace* the snapshot's recorded forwards — two machines cannot both
    /// hold the same host port.
    #[arg(long = "port", value_name = "[BIND:]HOST:GUEST")]
    pub ports: Vec<PortForward>,

    /// Do not forward any port (ignore the ones the snapshot recorded).
    #[arg(long, conflicts_with = "ports")]
    pub no_ports: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct RestoreArgs {
    /// Machine to restore (id, name, or unique id prefix).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Snapshot to restore it to (name, id, or unique id prefix).
    #[arg(value_name = "SNAPSHOT")]
    pub snapshot: String,

    /// Stop the machine first if it is running.
    #[arg(short, long)]
    pub force: bool,

    /// Skip the automatic snapshot of the state being replaced.
    #[arg(long)]
    pub no_backup: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct RollbackArgs {
    /// Machine to roll back (id, name, or unique id prefix).
    #[arg(value_name = "ID")]
    pub id: String,

    /// Stop the machine first if it is running.
    #[arg(short, long)]
    pub force: bool,

    /// Skip the automatic snapshot of the state being replaced.
    #[arg(long)]
    pub no_backup: bool,
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
    /// (internal) Write the generated Dockerfiles for every provisioned flavor.
    ///
    /// They are generated from the catalog rather than hand-written, so the
    /// images CI publishes cannot drift from what a local build would produce.
    #[command(name = "__dockerfiles", hide = true)]
    Dockerfiles(FlavorDockerfilesArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct FlavorDockerfilesArgs {
    /// Directory to write into (one `<flavor>/Dockerfile` per flavor).
    ///
    /// `flavors/`, not `images/`: the latter is where `bsdkrun fetch` drops
    /// multi-gigabyte guest disk images and is git-ignored repo-wide, so a
    /// tree generated there is never committed and CI finds nothing to build.
    #[arg(long, default_value = "flavors")]
    pub out: String,

    /// Fail instead of writing when what is on disk differs — the CI check
    /// that a flavor change was accompanied by a regenerated tree.
    #[arg(long)]
    pub check: bool,
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

/// macOS formats the boot volume case-insensitively, which collapses Linux
/// paths that differ only by case. These commands
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
/// manage the case-sensitive APFS sparsebundle that holds OCI rootfs trees and
/// named volumes instead. Linux hosts are already case-sensitive and need
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

/// `docker cp`-shaped: exactly one of SRC/DST carries an `ID:` prefix, and `-`
/// is the host's stdin (as SRC) or stdout (as DST).
#[derive(Parser, Default, Serialize, Deserialize)]
#[command(after_help = "\
EXAMPLES:
  bsdkrun cp ./main.py web:/app/main.py     copy a file into the machine
  bsdkrun cp web:/var/log/app.log ./        copy it back out
  bsdkrun cp -r ./src web:/app              copy a directory's contents (needs tar in the guest)
  cat a.txt | bsdkrun cp - web:/tmp/a       stream stdin in
  bsdkrun cp web:/tmp/a - | wc -c           stream stdout out")]
pub struct CpArgs {
    /// Copy a directory and its contents. `-r ./src ID:/app` leaves the guest's
    /// /app holding what ./src holds.
    #[arg(short = 'r', long)]
    pub recursive: bool,

    /// Source: `PATH`, `ID:PATH`, or `-` for stdin.
    #[arg(value_name = "SRC")]
    pub src: String,

    /// Destination: `PATH`, `ID:PATH`, or `-` for stdout.
    #[arg(value_name = "DST")]
    pub dst: String,
}

/// Save and restore guest directories under a key, so a rebuild can pick up
/// where the last one left off.
#[derive(Parser, Serialize, Deserialize)]
#[command(after_help = "\
EXAMPLES:
  bsdkrun cache save web:/root/.cargo --key cargo-$(shasum Cargo.lock | cut -c1-12)
  bsdkrun cache restore web --key cargo-abc123 --restore-keys cargo-
  bsdkrun cache ls
  bsdkrun cache rm cargo-abc123

Entries go to the host disk by default. Point them at S3 with
BSDKRUN_CACHE_BACKEND=s3 and BSDKRUN_CACHE_S3_BUCKET, or ~/.config/bsdkrun/cache.toml.")]
pub struct CacheArgs {
    #[command(subcommand)]
    pub cmd: CacheCmd,
}

#[derive(clap::Subcommand, Serialize, Deserialize)]
pub enum CacheCmd {
    /// Archive a guest directory and store it under a key.
    Save(CacheSaveArgs),
    /// Restore a stored tree into a machine.
    Restore(CacheRestoreArgs),
    /// List stored entries.
    Ls(CacheLsArgs),
    /// Remove stored entries.
    Rm(CacheRmArgs),
}

#[derive(Parser, Serialize, Deserialize)]
pub struct CacheSaveArgs {
    /// What to archive, as `ID:PATH` (the machine must be running).
    #[arg(value_name = "ID:PATH")]
    pub target: String,

    /// Key to store it under. Make it name the content — a lockfile hash is the
    /// usual choice — so a changed dependency set gets a different entry.
    #[arg(short = 'k', long)]
    pub key: String,

    /// Archive format: gzip (default), zstd, estargz, or none.
    #[arg(short = 'c', long, default_value = "gzip", value_name = "FORMAT")]
    pub compression: String,

    /// Replace an entry that already has this key.
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Print the result as JSON instead of a human summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct CacheRestoreArgs {
    /// Where to restore, as `ID` or `ID:PATH`. Without a path, the entry goes
    /// back to the directory it was saved from.
    #[arg(value_name = "ID[:PATH]")]
    pub target: String,

    /// Key to look for.
    #[arg(short = 'k', long)]
    pub key: String,

    /// Prefixes to fall back on when the key misses, most preferred first.
    /// Within a prefix the newest matching entry wins.
    #[arg(long, value_name = "PREFIX", num_args = 1..)]
    pub restore_keys: Vec<String>,

    /// Print the result as JSON. `restored` says whether anything was found,
    /// and `key` which entry a `--restore-keys` fallback landed on.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Default, Serialize, Deserialize)]
pub struct CacheLsArgs {
    /// Print the entries as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Serialize, Deserialize)]
pub struct CacheRmArgs {
    /// Remove every entry in the store.
    #[arg(long, conflicts_with = "keys")]
    pub all: bool,

    /// Key(s) to remove.
    #[arg(value_name = "KEY", required_unless_present = "all")]
    pub keys: Vec<String>,
}

#[derive(Parser, Default, Serialize, Deserialize)]
pub struct DoctorArgs {
    /// Print the report as JSON.
    #[arg(long)]
    pub json: bool,
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

    /// Attach a raw disk image as a virtio-blk block device (repeatable).
    /// Format: `PATH[:ro]` — append `:ro` for a read-only attachment. Unlike
    /// the virtio-fs rootfs, a block device gives the guest native-speed I/O
    /// (its own page cache, no per-file host round-trips) — format it inside
    /// the guest (e.g. `mkfs.ext4`) and mount it for I/O-heavy work like
    /// compiling. With the default virtio-fs root, the first attachment
    /// appears as `/dev/vda`. Re-attached automatically on `start`.
    #[arg(long = "attach-disk", value_name = "PATH[:ro]")]
    pub attach_disk: Vec<DiskSpec>,

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

/// Options for the `solo5` command.
///
/// Deliberately small. A Solo5 unikernel declares the devices it wants in its
/// own binary (the `MFT1` manifest note), so bsdkrun reads the network and
/// block device *names* from there rather than asking for them — see
/// [`crate::solo5`]. What is left is what only the host can know: how much
/// memory to give it, what to back a block device with, and which ports to
/// forward.
#[derive(Parser, Serialize, Deserialize)]
pub struct Solo5Args {
    /// The unikernel to run: a `.hvt` binary, or a project directory whose
    /// `dist/` holds one (where `mirage build` leaves it).
    #[arg(value_name = "UNIKERNEL|DIR", default_value = ".")]
    pub path: PathBuf,

    /// Back a declared block device with a file, as NAME=FILE (repeatable).
    /// The NAME= may be omitted when the unikernel declares exactly one.
    #[arg(long = "block", value_name = "NAME=FILE")]
    pub block: Vec<String>,

    /// Run a different `solo5-hvt` tender instead of the embedded one — for
    /// testing a tender build without rebuilding bsdkrun.
    #[arg(long, value_name = "PATH")]
    pub tender: Option<PathBuf>,

    /// Run in the background and print the machine id (like `docker run -d`).
    /// Use `logs`/`stop` afterwards — a unikernel has no shell, so
    /// `shell`/`exec` do not apply.
    #[arg(short = 'd', long)]
    pub detach: bool,

    #[command(flatten)]
    pub net: NetConfig,

    #[command(flatten)]
    pub vm: VmConfig,

    /// Arguments passed to the unikernel itself. Put them after `--`, since
    /// MirageOS options look like this CLI's own: `-- --ipv4=10.0.0.2/24`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
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
    /// Absolute guest path to mount it at, when the spec named one
    /// (`PATH:/data`). The generated init formats a blank disk, mounts it
    /// there, and grows the filesystem if the image has since been enlarged.
    #[serde(default)]
    pub mount: Option<String>,
}

impl FromStr for DiskSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Only treat a trailing `:ro`/`:rw` as a mode (paths may contain `:`).
        let (base, read_only) = match (s.strip_suffix(":ro"), s.strip_suffix(":rw")) {
            (Some(b), _) => (b, true),
            (_, Some(b)) => (b, false),
            _ => (s, false),
        };
        // `PATH:/guest/path` asks the guest to mount it there. Only an
        // *absolute* tail counts, so a host path that itself contains a colon
        // is still just a path.
        let (path, mount) = match base.rsplit_once(':') {
            Some((p, m)) if m.starts_with('/') && !p.is_empty() => {
                (p.to_string(), Some(m.to_string()))
            }
            _ => (base.to_string(), None),
        };
        Ok(DiskSpec {
            path: PathBuf::from(path),
            read_only,
            mount,
        })
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
            attach_disk: vec![],
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

impl Default for DockerStartArgs {
    fn default() -> Self {
        Self {
            vm: VmConfig::default(),
            mounts: vec![],
            no_home: false,
            publish_bind: "mirror".to_string(),
            ports: vec![],
            disk_size: None,
            system_socket: false,
            no_context: false,
            no_activate: false,
            timeout: 120,
            json: false,
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

    /// The daemon builds `DockerStartArgs` directly, so its `Default` has to
    /// agree with clap's — otherwise a daemon-started engine would wait a
    /// different timeout, or bind published ports somewhere else.
    #[test]
    fn docker_start_defaults_match_clap() {
        let Command::Docker(parsed) = parse(&["bsdkrun", "docker", "start"]) else {
            panic!("not a docker command");
        };
        let DockerCmd::Start(parsed) = parsed.cmd else {
            panic!("not a docker start");
        };
        assert_eq!(json(&parsed), json(&DockerStartArgs::default()));
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
