//! Transport-agnostic operations: the engine's surface, as this daemon uses it.
//!
//! Both front ends — gRPC ([`crate::service`]) and GraphQL ([`crate::graphql`])
//! — go through here rather than talking to `bsdkrun-core` themselves, so the
//! two can't drift and each maps only its own wire types.
//!
//! This module used to build *command lines*. Its own header said argv
//! construction was where the daemon was most likely to be wrong, and that
//! three of the bugs found while building the gRPC side lived exactly here.
//! There is no argv any more: reads and mutations call the engine in-process,
//! and the operations that need their own process ([`crate::supervisor`]) hand
//! it a typed [`bsdkrun_core::cli::Command`] rather than a string of flags.
//!
//! Nothing here knows about protobuf, GraphQL or HTTP.

use bsdkrun_core::api;
use bsdkrun_core::cli::{
    BsdArgs, Command as CoreCommand, ExecArgs, FetchArgs, FlavorAddArgs, FlavorArgs, FlavorCmd,
    FlavorPrebuildArgs, FlavorRunArgs, IdArgs, LinuxArgs, LogsArgs, NanosArgs, NetConfig, OsvArgs,
    RunConfig, SshArgs, SystemdArgs, TailscaleArgs, UnikraftArgs, VmConfig,
};
use bsdkrun_core::net::PortForward;

use crate::pb::CommandResult;
use crate::supervisor::Supervisor;

/// The domain types are the engine's own — the shapes it already used for
/// `--json` — so nothing is re-declared or re-parsed here.
pub use bsdkrun_core::api::{Flavor, Image, Machine, Network, Version, Volume};

// ---------------------------------------------------------------------------
// option structs
// ---------------------------------------------------------------------------

/// The guest OSes with a dedicated subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsdOs {
    Freebsd,
    Netbsd,
}

impl BsdOs {
    pub fn as_str(self) -> &'static str {
        match self {
            BsdOs::Freebsd => "freebsd",
            BsdOs::Netbsd => "netbsd",
        }
    }

    fn fetch_os(self) -> bsdkrun_core::fetch::Os {
        match self {
            BsdOs::Freebsd => bsdkrun_core::fetch::Os::Freebsd,
            BsdOs::Netbsd => bsdkrun_core::fetch::Os::Netbsd,
        }
    }
}

/// User-mode networking, shared by every boot operation.
#[derive(Debug, Default, Clone)]
pub struct NetOpts {
    pub no_net: bool,
    /// Host->guest TCP forwards, each "HOST:GUEST".
    pub ports: Vec<String>,
    pub mac: Option<String>,
    pub network: Option<String>,
    pub name: Option<String>,
}

impl NetOpts {
    /// A malformed forward is dropped rather than failing the boot: the same
    /// leniency the wire had when these were passed through as strings.
    fn to_config(&self) -> NetConfig {
        NetConfig {
            no_net: self.no_net,
            ports: self.ports.iter().filter_map(|p| p.parse().ok()).collect(),
            mac: self.mac.clone(),
            network: self.network.clone(),
            name: self.name.clone(),
        }
    }
}

/// vCPU / memory, where 0 or absent means "whatever the engine defaults to"
/// rather than this daemon pinning a number of its own.
fn vm_config(cpus: Option<u32>, mem: Option<u32>) -> VmConfig {
    let d = VmConfig::default();
    VmConfig {
        cpus: cpus.filter(|c| *c > 0).map_or(d.cpus, |c| c.min(255) as u8),
        mem: mem.filter(|m| *m > 0).unwrap_or(d.mem),
    }
}

#[derive(Debug, Default, Clone)]
pub struct RunLinuxOpts {
    pub image: String,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: NetOpts,
    pub volume: Option<String>,
    pub mounts: Vec<String>,
    pub env: Vec<String>,
    pub entrypoint: Option<String>,
    pub initramfs: bool,
    pub kernel: Option<String>,
    pub kernel_version: Option<String>,
    pub console: Option<String>,
    pub repo: Option<String>,
    pub command: Vec<String>,
}

impl RunLinuxOpts {
    /// `detach` is unconditional: the daemon outlives any single request, so a
    /// foreground VM would have nowhere to live.
    ///
    /// Every field the caller did not set comes from `LinuxArgs::default()`,
    /// which restates clap's own defaults — so a machine booted through the
    /// daemon is configured exactly as `bsdkrun linux` would configure it.
    pub fn to_command(&self) -> CoreCommand {
        let d = LinuxArgs::default();
        CoreCommand::Linux(LinuxArgs {
            image: self.image.clone(),
            kernel: self.kernel.as_ref().map(Into::into),
            kernel_version: self.kernel_version.clone().unwrap_or(d.kernel_version),
            detach: true,
            initramfs: self.initramfs,
            volume: self.volume.clone(),
            mounts: self.mounts.clone(),
            entrypoint: self.entrypoint.clone(),
            env: self.env.clone(),
            console: self.console.clone().unwrap_or(d.console),
            net: self.net.to_config(),
            vm: vm_config(self.cpus, self.mem),
            repo: self.repo.clone(),
            command: self.command.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct RunBsdOpts {
    pub os: BsdOs,
    pub version: Option<String>,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: NetOpts,
    pub volume: Option<String>,
    pub persist: bool,
    pub force: bool,
    pub firmware: Option<String>,
    pub attach_disk: Vec<String>,
    pub disk_size: Option<String>,
    pub repo: Option<String>,
    pub command: Vec<String>,
}

impl RunBsdOpts {
    pub fn to_command(&self) -> CoreCommand {
        let args = BsdArgs {
            version: self.version.clone(),
            firmware: self.firmware.as_ref().map(Into::into),
            force: self.force,
            attach_disk: self
                .attach_disk
                .iter()
                .filter_map(|d| d.parse().ok())
                .collect(),
            run: RunConfig {
                detach: true,
                persist: self.persist,
                volume: self.volume.clone(),
            },
            net: self.net.to_config(),
            vm: vm_config(self.cpus, self.mem),
            disk_size: self.disk_size.clone(),
            verbose: false,
            repo: self.repo.clone(),
            command: self.command.clone(),
        };
        match self.os {
            BsdOs::Freebsd => CoreCommand::Freebsd(args),
            BsdOs::Netbsd => CoreCommand::Netbsd(args),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct RunNanosOpts {
    /// A path, or a bare name in ~/.ops/images.
    pub image: String,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: NetOpts,
    pub kernel: Option<String>,
    pub cmdline: Option<String>,
    pub persist: bool,
}

impl RunNanosOpts {
    pub fn to_command(&self) -> CoreCommand {
        // Like unikraft: no volume/repo/command — a unikernel has no agent to
        // run anything through. Nanos does have a root disk, so `persist`
        // (in-place boot) is the one disk option it takes.
        CoreCommand::Nanos(NanosArgs {
            image: self.image.clone(),
            kernel: self.kernel.as_ref().map(Into::into),
            cmdline: self.cmdline.clone().unwrap_or_default(),
            firmware: None,
            detach: true,
            persist: self.persist,
            net: self.net.to_config(),
            vm: vm_config(self.cpus, self.mem),
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct RunUnikraftOpts {
    /// A `kraft` project directory or a built unikernel image. Empty = ".".
    pub path: Option<String>,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: NetOpts,
    pub cmdline: Option<String>,
    pub initramfs: Option<String>,
    /// Host directories shared in over virtio-fs, each "HOST:GUEST".
    pub mounts: Vec<String>,
}

impl RunUnikraftOpts {
    pub fn to_command(&self) -> CoreCommand {
        // No volume/persist/repo/command: a unikernel has no disk to persist
        // and no agent to run anything through. `mount` is the exception — it
        // shares a host directory over virtio-fs, which needs neither.
        let d = UnikraftArgs::default();
        CoreCommand::Unikraft(UnikraftArgs {
            path: self.path.as_ref().map_or(d.path, Into::into),
            cmdline: self.cmdline.clone().unwrap_or_default(),
            initramfs: self.initramfs.as_ref().map(Into::into),
            mount: self.mounts.clone(),
            detach: true,
            net: self.net.to_config(),
            vm: vm_config(self.cpus, self.mem),
        })
    }
}

/// OSv: like nanos there is no agent (no exec/shell/snapshot), but it does
/// have a root filesystem, so the disk options apply.
#[derive(Debug, Default, Clone)]
pub struct RunOsvOpts {
    /// An aarch64 loader.img, or on x86_64 the loader ELF plus a `disk`.
    pub image: String,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: NetOpts,
    /// The application to run and its arguments, e.g. "/hello.so".
    pub cmdline: Option<String>,
    pub disk: Option<String>,
    pub no_disk: bool,
    pub attach_disk: Vec<String>,
    /// "v2" or "v3"; None takes the engine's default (v2).
    pub gic: Option<String>,
    pub persist: bool,
    pub volume: Option<String>,
}

impl RunOsvOpts {
    pub fn to_command(&self) -> CoreCommand {
        use bsdkrun_core::osv::Gic;
        CoreCommand::Osv(OsvArgs {
            image: self.image.clone().into(),
            cmdline: self.cmdline.clone().unwrap_or_default(),
            disk: self.disk.as_ref().map(Into::into),
            no_disk: self.no_disk,
            attach_disk: self
                .attach_disk
                .iter()
                .filter_map(|d| d.parse().ok())
                .collect(),
            // Anything but an explicit "v3" keeps the default, which is what an
            // unset field meant when this was a command line.
            gic: match self.gic.as_deref() {
                Some("v3") => Gic::V3,
                _ => Gic::default(),
            },
            persist: self.persist,
            volume: self.volume.clone(),
            detach: true,
            net: self.net.to_config(),
            vm: vm_config(self.cpus, self.mem),
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct RunFlavorOpts {
    pub name: String,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub ports: Vec<String>,
    pub volume: Option<String>,
    pub repo: Option<String>,
}

impl RunFlavorOpts {
    pub fn to_command(&self) -> CoreCommand {
        CoreCommand::Flavor(FlavorArgs {
            cmd: FlavorCmd::Run(FlavorRunArgs {
                name: self.name.clone(),
                detach: true,
                vm: vm_config(self.cpus, self.mem),
                ports: self
                    .ports
                    .iter()
                    .filter_map(|p| p.parse::<PortForward>().ok())
                    .collect(),
                volume: self.volume.clone(),
                repo: self.repo.clone(),
            }),
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct AddFlavorOpts {
    pub name: String,
    pub base: String,
    pub category: String,
    pub description: String,
    pub ports: Vec<String>,
    pub env: Vec<String>,
    pub nix: Vec<String>,
    pub provision: Vec<String>,
}

impl AddFlavorOpts {
    fn to_args(&self) -> FlavorAddArgs {
        let d = FlavorAddArgs::default();
        FlavorAddArgs {
            name: self.name.clone(),
            base: self.base.clone(),
            category: if self.category.is_empty() {
                d.category
            } else {
                self.category.clone()
            },
            description: self.description.clone(),
            ports: self.ports.clone(),
            env: self.env.clone(),
            nix: self.nix.clone(),
            provision: self.provision.clone(),
        }
    }
}

/// Whether a guest kind is a BSD rather than Linux, and the `TERM` such a
/// guest needs for an interactive session. Both are the engine's own rules.
pub use bsdkrun_core::api::{interactive_term, is_bsd};

/// An `exec` invocation. An empty `command` means "open this machine's shell",
/// which only makes sense on a terminal and therefore implies a tty.
#[derive(Debug, Default, Clone)]
pub struct ExecOpts {
    pub id: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    pub tty: bool,
}

impl ExecOpts {
    /// Returns the command and whether a terminal is required.
    pub fn to_command(&self) -> (CoreCommand, bool) {
        if self.command.is_empty() {
            (
                CoreCommand::Shell(IdArgs {
                    id: self.id.clone(),
                }),
                true,
            )
        } else {
            (
                CoreCommand::Exec(ExecArgs {
                    tty: self.tty,
                    env: self.env.clone(),
                    id: self.id.clone(),
                    command: self.command.clone(),
                }),
                self.tty,
            )
        }
    }
}

/// `ssh` / `tailscale` / `systemd` share a shape: a machine plus a verb and its
/// arguments, handed to the in-guest agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestTool {
    Ssh,
    Tailscale,
    Systemd,
}

impl GuestTool {
    pub fn as_str(self) -> &'static str {
        match self {
            GuestTool::Ssh => "ssh",
            GuestTool::Tailscale => "tailscale",
            GuestTool::Systemd => "systemd",
        }
    }
}

// ---------------------------------------------------------------------------
// results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Info {
    pub daemon_version: String,
    /// The engine this daemon links. There is no second binary to disagree with
    /// it, which is the point of the field.
    pub engine_version: String,
    pub exe_path: String,
    pub os: String,
    pub arch: String,
}

/// An error from an operation, kept transport-neutral so each front end can
/// render it in its own idiom (a gRPC `Status`, a GraphQL error).
#[derive(Debug)]
pub enum OpError {
    /// The caller asked for something impossible; nothing was run.
    InvalidArgument(String),
    /// The operation ran and failed, or could not be run at all.
    Failed(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::InvalidArgument(m) | OpError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for OpError {}

impl From<tonic::Status> for OpError {
    fn from(s: tonic::Status) -> Self {
        match s.code() {
            tonic::Code::InvalidArgument => OpError::InvalidArgument(s.message().to_string()),
            _ => OpError::Failed(s.message().to_string()),
        }
    }
}

impl From<OpError> for tonic::Status {
    fn from(e: OpError) -> Self {
        match e {
            OpError::InvalidArgument(m) => tonic::Status::invalid_argument(m),
            OpError::Failed(m) => tonic::Status::internal(m),
        }
    }
}

pub type OpResult<T> = Result<T, OpError>;

fn require_non_empty(what: &str, items: &[String]) -> OpResult<()> {
    if items.is_empty() {
        // Refused rather than run: a bare `rm -f` with no ids could be read far
        // more broadly than the caller intended.
        return Err(OpError::InvalidArgument(format!(
            "{what} must not be empty"
        )));
    }
    Ok(())
}

/// Run a blocking engine call on a thread with no tokio runtime attached.
///
/// Not `spawn_blocking`: a blocking-pool thread still carries the runtime
/// context, and the engine's database handle builds a runtime of its own and
/// `block_on`s it — which panics with "Cannot start a runtime from within a
/// runtime" the moment any context is present. A plain OS thread has none, so
/// the engine behaves exactly as it does under the CLI.
async fn blocking<T, F>(what: &'static str, f: F) -> OpResult<T>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name(format!("bsdkrun-{what}").replace(' ', "-"))
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| OpError::Failed(format!("starting a thread for {what}: {e}")))?;
    match rx.await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(OpError::Failed(format!("{e:#}"))),
        // The sender is only dropped without a value if the thread panicked.
        Err(_) => Err(OpError::Failed(format!("{what} panicked"))),
    }
}

/// A mutation's outcome as the wire's `CommandResult`.
///
/// There is no subprocess to have written anything any more, so the message the
/// engine returns — the same line the CLI prints — becomes stdout, and a
/// failure becomes a non-zero exit with the error chain on stderr. Clients that
/// display `stdout` keep working unchanged.
fn as_result(outcome: OpResult<String>) -> OpResult<CommandResult> {
    match outcome {
        Ok(stdout) => Ok(CommandResult {
            exit_code: 0,
            stdout: if stdout.ends_with('\n') {
                stdout
            } else {
                format!("{stdout}\n")
            },
            stderr: String::new(),
        }),
        // An invalid argument is the caller's mistake and stays an error; a
        // failure *while running* is reported like a non-zero exit was, since
        // several callers treat that as a state to display rather than a fault.
        Err(OpError::InvalidArgument(m)) => Err(OpError::InvalidArgument(m)),
        Err(OpError::Failed(m)) => Ok(CommandResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("{m}\n"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------------

/// Every operation the daemon exposes, independent of how it was requested.
#[derive(Clone, Debug)]
pub struct Ops {
    supervisor: Supervisor,
}

impl Ops {
    pub fn new(supervisor: Supervisor) -> Self {
        Self { supervisor }
    }

    pub fn supervisor(&self) -> &Supervisor {
        &self.supervisor
    }

    // -- daemon --------------------------------------------------------------

    pub async fn info(&self) -> OpResult<Info> {
        Ok(Info {
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            engine_version: bsdkrun_core::VERSION.to_string(),
            exe_path: self
                .supervisor
                .exe()
                .map(|p| p.display().to_string())
                // Reported rather than hidden: a UI showing this is exactly
                // where an operator would look for why a boot failed.
                .unwrap_or_else(|| format!("{} (not found)", crate::supervisor::SUPERVISOR_BIN)),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        })
    }

    // -- machines ------------------------------------------------------------

    pub async fn list_machines(&self, all: bool) -> OpResult<Vec<Machine>> {
        blocking("listing machines", move || api::list_machines(all)).await
    }

    /// Start a stopped machine. Booting, so it goes through a supervisor
    /// process; the returned id is the machine's own, unchanged.
    pub async fn start(&self, id: &str) -> OpResult<CommandResult> {
        let cmd = CoreCommand::Start(IdArgs { id: id.to_string() });
        as_result(self.supervisor.detached(&cmd).await.map_err(OpError::from))
    }

    pub async fn stop(&self, id: &str) -> OpResult<CommandResult> {
        let id = id.to_string();
        as_result(blocking("stopping a machine", move || api::stop(&id)).await)
    }

    pub async fn remove_machines(&self, ids: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("ids", ids)?;
        let ids = ids.to_vec();
        as_result(
            blocking("removing machines", move || {
                let mut removed = Vec::new();
                for id in &ids {
                    removed.push(api::remove_machine(id, force)?);
                }
                Ok(removed.join("\n"))
            })
            .await,
        )
    }

    pub async fn update_machine(
        &self,
        id: &str,
        cpus: Option<u32>,
        mem: Option<u32>,
    ) -> OpResult<CommandResult> {
        let id = id.to_string();
        let cpus = cpus.map(|c| c.min(255) as u8);
        as_result(blocking("updating a machine", move || api::update(&id, cpus, mem)).await)
    }

    pub async fn commit(&self, id: &str, name: &str, description: &str) -> OpResult<CommandResult> {
        let (id, name, description) = (id.to_string(), name.to_string(), description.to_string());
        as_result(
            blocking("committing a machine", move || {
                api::commit(&id, &name, &description)
            })
            .await,
        )
    }

    // -- booting -------------------------------------------------------------
    //
    // Each of these returns the new machine's id. They run in a supervisor
    // process because a detached boot forks and the child becomes the VM — see
    // [`crate::supervisor`].

    pub async fn run_linux(&self, opts: &RunLinuxOpts) -> OpResult<String> {
        if opts.image.trim().is_empty() {
            return Err(OpError::InvalidArgument("image must not be empty".into()));
        }
        Ok(self.supervisor.detached(&opts.to_command()).await?)
    }

    pub async fn run_bsd(&self, opts: &RunBsdOpts) -> OpResult<String> {
        Ok(self.supervisor.detached(&opts.to_command()).await?)
    }

    pub async fn run_unikraft(&self, opts: &RunUnikraftOpts) -> OpResult<String> {
        Ok(self.supervisor.detached(&opts.to_command()).await?)
    }

    pub async fn run_nanos(&self, opts: &RunNanosOpts) -> OpResult<String> {
        if opts.image.trim().is_empty() {
            return Err(OpError::InvalidArgument("image must not be empty".into()));
        }
        Ok(self.supervisor.detached(&opts.to_command()).await?)
    }

    pub async fn run_osv(&self, opts: &RunOsvOpts) -> OpResult<String> {
        if opts.image.trim().is_empty() {
            return Err(OpError::InvalidArgument("image must not be empty".into()));
        }
        Ok(self.supervisor.detached(&opts.to_command()).await?)
    }

    pub async fn run_flavor(&self, opts: &RunFlavorOpts) -> OpResult<String> {
        if opts.name.trim().is_empty() {
            return Err(OpError::InvalidArgument("name must not be empty".into()));
        }
        Ok(self.supervisor.detached(&opts.to_command()).await?)
    }

    // -- images --------------------------------------------------------------

    pub async fn list_images(&self) -> OpResult<Vec<Image>> {
        blocking("listing images", api::list_images).await
    }

    pub async fn list_versions(&self, os: BsdOs) -> OpResult<Vec<Version>> {
        let os = os.fetch_os();
        blocking("listing versions", move || Ok(api::list_versions(os))).await
    }

    // -- volumes -------------------------------------------------------------

    pub async fn list_volumes(&self) -> OpResult<Vec<Volume>> {
        blocking("listing volumes", api::list_volumes).await
    }

    pub async fn remove_volumes(&self, names: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("names", names)?;
        let names = names.to_vec();
        as_result(each(names, move |n| api::remove_volume(&n, force)).await)
    }

    // -- networks ------------------------------------------------------------

    pub async fn list_networks(&self) -> OpResult<Vec<Network>> {
        blocking("listing networks", api::list_networks).await
    }

    pub async fn create_network(&self, name: &str) -> OpResult<CommandResult> {
        let name = name.to_string();
        as_result(blocking("creating a network", move || api::create_network(&name)).await)
    }

    pub async fn remove_networks(&self, names: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("names", names)?;
        let names = names.to_vec();
        as_result(each(names, move |n| api::remove_network(&n, force)).await)
    }

    pub async fn connect_network(&self, machine: &str, network: &str) -> OpResult<CommandResult> {
        let (machine, network) = (machine.to_string(), network.to_string());
        as_result(
            blocking("connecting a machine to a network", move || {
                api::connect_network(&machine, &network)
            })
            .await,
        )
    }

    pub async fn disconnect_network(&self, machine: &str) -> OpResult<CommandResult> {
        let machine = machine.to_string();
        as_result(
            blocking("disconnecting a machine", move || {
                api::disconnect_network(&machine)
            })
            .await,
        )
    }

    pub async fn sync_network(&self, network: &str) -> OpResult<CommandResult> {
        let network = network.to_string();
        as_result(blocking("syncing a network", move || api::sync_network(&network)).await)
    }

    // -- flavors -------------------------------------------------------------

    pub async fn list_flavors(&self) -> OpResult<Vec<Flavor>> {
        blocking("listing flavors", api::list_flavors).await
    }

    pub async fn add_flavor(&self, opts: &AddFlavorOpts) -> OpResult<CommandResult> {
        if opts.name.trim().is_empty() || opts.base.trim().is_empty() {
            return Err(OpError::InvalidArgument(
                "name and base are required".into(),
            ));
        }
        let args = opts.to_args();
        as_result(blocking("adding a flavor", move || api::add_flavor(args)).await)
    }

    pub async fn remove_flavors(&self, names: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("names", names)?;
        // `force` has never meant anything for a flavor — a catalog entry is
        // refused either way — but it stays on the wire.
        let _ = force;
        let names = names.to_vec();
        as_result(each(names, |n| api::remove_flavor(&n)).await)
    }

    /// Environment for an interactive session, with `TERM` supplied when the
    /// guest needs one.
    ///
    /// Every interactive path must go through this. A BSD guest boots with no
    /// usable TERM and an explicit `exec` injects none — by design, since a
    /// non-interactive exec should run verbatim — so a terminal opened that way
    /// comes up `dumb` unless the caller supplies it. A caller that set TERM
    /// itself always wins.
    pub async fn interactive_env(&self, id: &str, env: Vec<String>) -> Vec<String> {
        let mut env = env;
        if env.iter().any(|e| e.starts_with("TERM=")) {
            return env;
        }
        if let Some(term) = self
            .machine_kind(id)
            .await
            .as_deref()
            .and_then(interactive_term)
        {
            env.push(term);
        }
        env
    }

    /// A machine's guest kind ("linux" | "freebsd" | "netbsd" | …), or None if
    /// there is no such machine.
    ///
    /// Ids may be a unique prefix, exactly as on the CLI.
    pub async fn machine_kind(&self, id: &str) -> Option<String> {
        let id = id.to_string();
        blocking("finding a machine", move || api::find_machine(&id))
            .await
            .ok()
            .flatten()
            .map(|m| m.kind)
    }

    // -- guest tools ---------------------------------------------------------

    /// Run an in-guest agent action (`ssh`/`tailscale`/`systemd`) and collect
    /// its output. A non-zero exit is returned rather than raised: for these
    /// commands "not installed" / "not running" is a legitimate state the UI
    /// should display, not a transport failure.
    pub async fn guest_tool(
        &self,
        tool: GuestTool,
        id: &str,
        args: &[String],
    ) -> OpResult<CommandResult> {
        let cmd = self.guest_tool_command(tool, id, args)?;
        Ok(self.supervisor.output(&self.supervisor.argv(&cmd)?).await?)
    }

    pub async fn update_agent(&self, id: &str) -> OpResult<CommandResult> {
        let cmd = self.update_agent_command(id);
        Ok(self.supervisor.output(&self.supervisor.argv(&cmd)?).await?)
    }

    /// A machine's console log as a single string, for a non-following read.
    pub async fn machine_logs(&self, id: &str, boot: bool) -> OpResult<String> {
        let cmd = self.logs_command(id, false, boot);
        let res = self.supervisor.output(&self.supervisor.argv(&cmd)?).await?;
        // Console output goes to stdout and the engine's own boot log to
        // stderr, so which one carries the content depends on `boot`.
        Ok(if res.stdout.trim().is_empty() {
            res.stderr
        } else {
            res.stdout
        })
    }

    // -- host ----------------------------------------------------------------

    /// Host CPU/RAM and the real on-disk size of every microVM.
    ///
    /// Off the async runtime: it walks the whole state directory.
    pub async fn system_stats(&self) -> OpResult<crate::system::SystemStats> {
        tokio::task::spawn_blocking(crate::system::sample)
            .await
            .map_err(|e| OpError::Failed(format!("sampling host stats: {e}")))
    }

    /// Run a command in a supervisor process and stream its output.
    ///
    /// The one streaming primitive both front ends use; neither builds a
    /// process itself.
    pub fn stream(
        &self,
        cmd: &CoreCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<crate::pb::OutputChunk, tonic::Status>>, OpError>
    {
        let argv = self.supervisor.argv(cmd)?;
        Ok(self.supervisor.stream(&argv)?)
    }

    // -- commands for the streaming operations --------------------------------
    //
    // These return a command rather than running it: the caller decides how to
    // surface a stream (a gRPC response stream, a GraphQL subscription).

    pub fn logs_command(&self, id: &str, follow: bool, boot: bool) -> CoreCommand {
        CoreCommand::Logs(LogsArgs {
            id: id.to_string(),
            follow,
            boot,
        })
    }

    pub fn update_agent_command(&self, id: &str) -> CoreCommand {
        CoreCommand::Agent(bsdkrun_core::cli::AgentArgs {
            cmd: bsdkrun_core::cli::AgentCmd::Update(IdArgs { id: id.to_string() }),
        })
    }

    pub fn fetch_command(
        &self,
        os: BsdOs,
        version: &Option<String>,
        dir: &Option<String>,
        force: bool,
    ) -> CoreCommand {
        let d = FetchArgs::default();
        CoreCommand::Fetch(FetchArgs {
            os: os.fetch_os(),
            version: version.clone(),
            dir: dir.clone().map_or(d.dir, Into::into),
            force,
        })
    }

    pub fn build_flavor_command(
        &self,
        name: &str,
        cpus: Option<u32>,
        mem: Option<u32>,
        force: bool,
    ) -> CoreCommand {
        CoreCommand::Flavor(FlavorArgs {
            cmd: FlavorCmd::Build(FlavorPrebuildArgs {
                name: name.to_string(),
                vm: vm_config(cpus, mem),
                force,
            }),
        })
    }

    pub fn guest_tool_command(
        &self,
        tool: GuestTool,
        id: &str,
        args: &[String],
    ) -> OpResult<CoreCommand> {
        if args.is_empty() {
            return Err(OpError::InvalidArgument(
                "args must name an action, e.g. [\"status\"]".into(),
            ));
        }
        let (id, args) = (id.to_string(), args.to_vec());
        Ok(match tool {
            GuestTool::Ssh => CoreCommand::Ssh(SshArgs { id, args }),
            GuestTool::Tailscale => CoreCommand::Tailscale(TailscaleArgs { id, args }),
            GuestTool::Systemd => CoreCommand::Systemd(SystemdArgs { id, args }),
        })
    }
}

/// Apply an operation to each name, collecting the results into one report.
///
/// The first failure stops the run, matching what the CLI does when it is given
/// several names and one of them cannot be removed.
async fn each<F>(names: Vec<String>, f: F) -> OpResult<String>
where
    F: Fn(String) -> anyhow::Result<String> + Send + 'static,
{
    blocking("applying an operation to each name", move || {
        let mut done = Vec::new();
        for n in names {
            done.push(f(n)?);
        }
        Ok(done.join("\n"))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single most valuable property of building typed commands rather than
    /// argv: a field the caller leaves alone gets the engine's own default,
    /// not an empty string or a zero.
    #[test]
    fn an_unset_option_takes_the_engines_default() {
        let CoreCommand::Linux(a) = RunLinuxOpts {
            image: "alpine".into(),
            ..Default::default()
        }
        .to_command() else {
            panic!("not a linux command");
        };
        let d = LinuxArgs::default();
        assert_eq!(a.console, d.console);
        assert_eq!(a.kernel_version, d.kernel_version);
        assert_eq!(a.vm.cpus, d.vm.cpus);
        assert_eq!(a.vm.mem, d.vm.mem);
        // Always detached: the daemon outlives the request.
        assert!(a.detach);
    }

    #[test]
    fn zero_resources_mean_the_default_rather_than_zero() {
        let d = VmConfig::default();
        assert_eq!(vm_config(Some(0), Some(0)).cpus, d.cpus);
        assert_eq!(vm_config(None, None).mem, d.mem);
        assert_eq!(vm_config(Some(4), Some(2048)).cpus, 4);
        assert_eq!(vm_config(Some(4), Some(2048)).mem, 2048);
        // More vCPUs than the field can hold saturates instead of wrapping.
        assert_eq!(vm_config(Some(9000), None).cpus, 255);
    }

    #[test]
    fn net_opts_become_a_parsed_config() {
        let cfg = NetOpts {
            no_net: false,
            ports: vec!["2222:22".into(), "not-a-port".into(), "8080:80".into()],
            mac: None,
            network: Some("dev".into()),
            name: Some("web".into()),
        }
        .to_config();
        // The malformed one is dropped; the good ones survive intact.
        assert_eq!(cfg.ports.len(), 2);
        assert_eq!(cfg.ports[0].host, 2222);
        assert_eq!(cfg.ports[0].guest, 22);
        assert_eq!(cfg.ports[1].host, 8080);
        assert_eq!(cfg.network.as_deref(), Some("dev"));
    }

    #[test]
    fn an_empty_exec_command_opens_a_shell_and_forces_a_tty() {
        let (cmd, tty) = ExecOpts {
            id: "abc".into(),
            command: vec![],
            env: vec![],
            tty: false,
        }
        .to_command();
        assert!(tty, "a shell is only meaningful on a terminal");
        assert!(matches!(cmd, CoreCommand::Shell(a) if a.id == "abc"));
    }

    #[test]
    fn a_bsd_run_targets_the_requested_os() {
        let opts = RunBsdOpts {
            os: BsdOs::Netbsd,
            version: Some("current".into()),
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            volume: None,
            persist: true,
            force: false,
            firmware: None,
            attach_disk: vec!["/tmp/a.raw:ro".into()],
            disk_size: None,
            repo: None,
            command: vec![],
        };
        let CoreCommand::Netbsd(a) = opts.to_command() else {
            panic!("not a netbsd command");
        };
        assert!(a.run.detach && a.run.persist);
        assert_eq!(a.version.as_deref(), Some("current"));
        assert_eq!(a.attach_disk.len(), 1);
        assert!(a.attach_disk[0].read_only);
    }

    #[test]
    fn a_failed_mutation_reads_as_a_non_zero_exit() {
        let res = as_result(Err(OpError::Failed("no such machine".into()))).unwrap();
        assert_eq!(res.exit_code, 1);
        assert!(res.stderr.contains("no such machine"));

        let ok = as_result(Ok("abc123".into())).unwrap();
        assert_eq!(ok.exit_code, 0);
        assert_eq!(ok.stdout, "abc123\n");
    }
}
