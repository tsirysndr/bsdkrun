//! Transport-agnostic operations: the CLI's surface expressed as plain Rust.
//!
//! Both front ends — gRPC ([`crate::service`]) and GraphQL ([`crate::graphql`])
//! — go through here rather than building command lines themselves. That
//! matters because argv construction is where this daemon is most likely to be
//! wrong: a misplaced flag is invisible until it reaches a real machine, and
//! three of the bugs found while building the gRPC side lived exactly here.
//! Duplicating it per transport would double that risk and let the two drift.
//!
//! Nothing in this module knows about protobuf, GraphQL or HTTP. Domain types
//! deserialize straight from the CLI's `--json` output; each front end maps
//! them into its own wire types.

use serde::Deserialize;

use crate::cli::Cli;
use crate::pb::CommandResult;

// ---------------------------------------------------------------------------
// argv building
// ---------------------------------------------------------------------------

/// A small builder so each operation reads as the command line it produces.
///
/// Flags are always emitted *before* positionals. clap accepts them
/// interspersed, but several subcommands take `trailing_var_arg` /
/// `last = true` arguments that swallow everything after the positional, so
/// keeping flags in front is the only ordering that is correct everywhere.
#[derive(Default)]
pub struct Argv(Vec<String>);

impl Argv {
    pub fn new(parts: &[&str]) -> Self {
        Self(parts.iter().map(|s| s.to_string()).collect())
    }

    pub fn arg(&mut self, v: impl Into<String>) -> &mut Self {
        self.0.push(v.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(&mut self, vs: I) -> &mut Self {
        self.0.extend(vs.into_iter().map(Into::into));
        self
    }

    pub fn flag(&mut self, name: &str, on: bool) -> &mut Self {
        if on {
            self.0.push(name.to_string());
        }
        self
    }

    pub fn opt<T: ToString>(&mut self, name: &str, v: &Option<T>) -> &mut Self {
        if let Some(v) = v {
            self.0.push(name.to_string());
            self.0.push(v.to_string());
        }
        self
    }

    /// A repeatable `--flag VALUE` option.
    pub fn each(&mut self, name: &str, vs: &[String]) -> &mut Self {
        for v in vs {
            self.0.push(name.to_string());
            self.0.push(v.clone());
        }
        self
    }

    /// vCPU / memory sizing. `None` leaves the flag off so the CLI applies its
    /// own default rather than this daemon pinning one.
    pub fn vm(&mut self, cpus: Option<u32>, mem: Option<u32>) -> &mut Self {
        self.opt("--cpus", &cpus.filter(|c| *c > 0))
            .opt("--mem", &mem.filter(|m| *m > 0))
    }

    pub fn net(&mut self, net: &NetOpts) -> &mut Self {
        self.flag("--no-net", net.no_net)
            .each("--port", &net.ports)
            .opt("--mac", &net.mac)
            .opt("--network", &net.network)
            .opt("--name", &net.name)
    }

    pub fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.0)
    }
}

// ---------------------------------------------------------------------------
// option structs
// ---------------------------------------------------------------------------

/// The guest OSes with a dedicated CLI subcommand.
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
    /// `-d` is unconditional: the daemon outlives any single request, so a
    /// foreground VM would have nowhere to live.
    pub fn to_argv(&self) -> Vec<String> {
        let mut argv = Argv::new(&["linux", "-d"]);
        argv.vm(self.cpus, self.mem)
            .net(&self.net)
            .opt("-v", &self.volume)
            .each("--mount", &self.mounts)
            .each("-e", &self.env)
            .opt("--entrypoint", &self.entrypoint)
            .flag("--initramfs", self.initramfs)
            .opt("--kernel", &self.kernel)
            .opt("--kernel-version", &self.kernel_version)
            .opt("--console", &self.console)
            .opt("--repo", &self.repo)
            .arg(self.image.clone());
        if !self.command.is_empty() {
            argv.arg("--").args(self.command.clone());
        }
        argv.take()
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
    pub fn to_argv(&self) -> Vec<String> {
        let mut argv = Argv::new(&[self.os.as_str(), "-d"]);
        argv.vm(self.cpus, self.mem)
            .net(&self.net)
            .opt("--version", &self.version)
            .opt("-v", &self.volume)
            .flag("--persist", self.persist)
            .flag("-f", self.force)
            .opt("--firmware", &self.firmware)
            .each("--attach-disk", &self.attach_disk)
            .opt("--disk-size", &self.disk_size)
            .opt("--repo", &self.repo);
        if !self.command.is_empty() {
            argv.arg("--").args(self.command.clone());
        }
        argv.take()
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
    pub fn to_argv(&self) -> Vec<String> {
        // Like unikraft: no -v/--repo/command — a unikernel has no agent to
        // run anything through. Nanos does have a root disk, so --persist
        // (in-place boot) is the one disk option it takes.
        Argv::new(&["nanos", "-d"])
            .vm(self.cpus, self.mem)
            .net(&self.net)
            .opt("--kernel", &self.kernel)
            .opt("--cmdline", &self.cmdline)
            .flag("--persist", self.persist)
            .arg(self.image.clone())
            .take()
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
    pub fn to_argv(&self) -> Vec<String> {
        // No -v/--persist/--repo/command: a unikernel has no disk to persist
        // and no agent to run anything through. `--mount` is the exception —
        // it shares a host directory over virtio-fs, which needs neither.
        Argv::new(&["unikraft", "-d"])
            .vm(self.cpus, self.mem)
            .net(&self.net)
            .opt("--cmdline", &self.cmdline)
            .opt("--initramfs", &self.initramfs)
            .each("--mount", &self.mounts)
            .arg(self.path.clone().unwrap_or_else(|| ".".into()))
            .take()
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
    pub fn to_argv(&self) -> Vec<String> {
        Argv::new(&["flavor", "run", "-d"])
            .vm(self.cpus, self.mem)
            .each("--port", &self.ports)
            .opt("-v", &self.volume)
            .opt("--repo", &self.repo)
            .arg(self.name.clone())
            .take()
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
    pub fn to_argv(&self) -> Vec<String> {
        let mut argv = Argv::new(&["flavor", "add"]);
        argv.arg("--base").arg(self.base.clone());
        if !self.category.is_empty() {
            argv.arg("--category").arg(self.category.clone());
        }
        if !self.description.is_empty() {
            argv.arg("--description").arg(self.description.clone());
        }
        argv.each("--port", &self.ports)
            .each("--env", &self.env)
            .each("--nix", &self.nix)
            .each("--provision", &self.provision)
            .arg(self.name.clone());
        argv.take()
    }
}

/// Whether a guest kind is a BSD rather than Linux.
///
/// Mirrors the CLI's own rule: anything that is not `linux` is a BSD guest.
/// The boot-mode kinds (`firmware`, `kernel`) are BSD too.
pub fn is_bsd(kind: &str) -> bool {
    kind != "linux"
}

/// `TERM` for an interactive session on a guest of this kind.
///
/// FreeBSD and NetBSD guests boot with no `TERM` on the agent's pty, which
/// leaves the shell in `dumb` mode — no line editing, no colour, no key
/// sequences. `xterm` is in both guests' terminfo. Linux images set their own,
/// so they are left alone.
///
/// The CLI injects this for `shell`, but an explicit `exec` runs verbatim with
/// no injected env by design — so anything driving `exec` for an interactive
/// terminal has to supply it, as the desktop app does locally.
pub fn interactive_term(kind: &str) -> Option<String> {
    is_bsd(kind).then(|| "TERM=xterm".to_string())
}

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
    /// Returns the argv and whether a terminal is required.
    pub fn to_argv(&self) -> (Vec<String>, bool) {
        if self.command.is_empty() {
            (Argv::new(&["shell"]).arg(self.id.clone()).take(), true)
        } else {
            let argv = Argv::new(&["exec"])
                .flag("-t", self.tty)
                .each("-e", &self.env)
                .arg(self.id.clone())
                .args(self.command.clone())
                .take();
            (argv, self.tty)
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
// domain types — the shapes the CLI's `--json` subcommands emit
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Machine {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub image: String,
    pub kind: String,
    pub command: String,
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i64>,
    pub pid: Option<i64>,
    pub detached: bool,
    pub cpus: Option<i64>,
    pub mem: Option<i64>,
    pub volume: Option<String>,
    pub state_dir: Option<String>,
    pub created_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub net_ip: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    pub id: String,
    pub reference: String,
    pub digest: Option<String>,
    pub size: i64,
    pub rootfs: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Volume {
    pub name: String,
    pub guest: Option<String>,
    pub base: Option<String>,
    pub path: Option<String>,
    /// Human-readable ("2.3 GiB"). The CLI reports volume size as text, and
    /// writes "-" when it cannot be measured — normalised away by [`Ops`].
    pub size: Option<String>,
    pub created_at: Option<String>,
    pub tracked: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Network {
    pub name: String,
    pub subnet: String,
    pub gateway: String,
    pub members: i64,
    pub running: i64,
    pub up: bool,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Flavor {
    pub name: String,
    /// "catalog" | "user" | "snapshot".
    pub source: String,
    /// "linux" | "freebsd" | "netbsd".
    pub kind: String,
    pub base: String,
    pub category: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub nix: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Version {
    pub version: String,
    pub latest: bool,
}

#[derive(Debug, Clone)]
pub struct Info {
    pub daemon_version: String,
    pub cli_version: String,
    pub cli_path: String,
    pub os: String,
    pub arch: String,
}

/// Parse `versions --os <os>`, which prints indented `  <ver>  (latest)` rows.
/// NetBSD's recommended build is the literal `current` rather than a number.
pub fn parse_versions(out: &str) -> Vec<Version> {
    let mut v = Vec::new();
    for line in out.lines() {
        let t = line.trim();
        let first = t.split_whitespace().next().unwrap_or("");
        let is_version = first.chars().next().is_some_and(|c| c.is_ascii_digit());
        let is_current = first == "current";
        if !is_version && !is_current {
            continue;
        }
        v.push(Version {
            version: first.to_string(),
            latest: t.contains("(latest)") || is_current,
        });
    }
    v
}

// ---------------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------------

/// An error from an operation, kept transport-neutral so each front end can
/// render it in its own idiom (a gRPC `Status`, a GraphQL error).
#[derive(Debug)]
pub enum OpError {
    /// The caller asked for something impossible; no command was run.
    InvalidArgument(String),
    /// The command ran and failed, or could not be run at all.
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

/// Every operation the daemon exposes, independent of how it was requested.
#[derive(Clone, Debug)]
pub struct Ops {
    cli: Cli,
}

impl Ops {
    pub fn new(cli: Cli) -> Self {
        Self { cli }
    }

    pub fn cli(&self) -> &Cli {
        &self.cli
    }

    // -- daemon --------------------------------------------------------------

    pub async fn info(&self) -> OpResult<Info> {
        let res = self.cli.output(&["--version".to_string()]).await?;
        Ok(Info {
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            cli_version: res.stdout.trim().to_string(),
            cli_path: self.cli.bin().display().to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        })
    }

    // -- machines ------------------------------------------------------------

    pub async fn list_machines(&self, all: bool) -> OpResult<Vec<Machine>> {
        let argv = Argv::new(&["ps"]).flag("-a", all).arg("--json").take();
        Ok(self.cli.json(&argv).await?)
    }

    pub async fn start(&self, id: &str) -> OpResult<CommandResult> {
        Ok(self
            .cli
            .output(&Argv::new(&["start"]).arg(id).take())
            .await?)
    }

    pub async fn stop(&self, id: &str) -> OpResult<CommandResult> {
        Ok(self
            .cli
            .output(&Argv::new(&["stop"]).arg(id).take())
            .await?)
    }

    pub async fn remove_machines(&self, ids: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("ids", ids)?;
        let argv = Argv::new(&["rm"])
            .flag("-f", force)
            .args(ids.to_vec())
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn update_machine(
        &self,
        id: &str,
        cpus: Option<u32>,
        mem: Option<u32>,
    ) -> OpResult<CommandResult> {
        let argv = Argv::new(&["update"])
            .opt("--cpus", &cpus)
            .opt("--mem", &mem)
            .arg(id)
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn commit(&self, id: &str, name: &str, description: &str) -> OpResult<CommandResult> {
        let argv = Argv::new(&["commit"])
            .arg("-d")
            .arg(description)
            .arg(id)
            .arg(name)
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    // -- booting -------------------------------------------------------------

    pub async fn run_linux(&self, opts: &RunLinuxOpts) -> OpResult<String> {
        if opts.image.trim().is_empty() {
            return Err(OpError::InvalidArgument("image must not be empty".into()));
        }
        Ok(self.cli.detached(&opts.to_argv()).await?)
    }

    pub async fn run_bsd(&self, opts: &RunBsdOpts) -> OpResult<String> {
        Ok(self.cli.detached(&opts.to_argv()).await?)
    }

    pub async fn run_unikraft(&self, opts: &RunUnikraftOpts) -> OpResult<String> {
        Ok(self.cli.detached(&opts.to_argv()).await?)
    }

    pub async fn run_nanos(&self, opts: &RunNanosOpts) -> OpResult<String> {
        if opts.image.trim().is_empty() {
            return Err(OpError::InvalidArgument("image must not be empty".into()));
        }
        Ok(self.cli.detached(&opts.to_argv()).await?)
    }

    pub async fn run_flavor(&self, opts: &RunFlavorOpts) -> OpResult<String> {
        if opts.name.trim().is_empty() {
            return Err(OpError::InvalidArgument("name must not be empty".into()));
        }
        Ok(self.cli.detached(&opts.to_argv()).await?)
    }

    // -- images --------------------------------------------------------------

    pub async fn list_images(&self) -> OpResult<Vec<Image>> {
        let argv = Argv::new(&["images", "--json"]).take();
        Ok(self.cli.json(&argv).await?)
    }

    pub async fn list_versions(&self, os: BsdOs) -> OpResult<Vec<Version>> {
        let argv = Argv::new(&["versions", "--os"]).arg(os.as_str()).take();
        let res = self.cli.output(&argv).await?;
        if res.exit_code != 0 {
            return Err(OpError::Failed(format!(
                "`bsdkrun versions` failed ({}): {}",
                res.exit_code,
                res.stderr.trim()
            )));
        }
        Ok(parse_versions(&res.stdout))
    }

    // -- volumes -------------------------------------------------------------

    pub async fn list_volumes(&self) -> OpResult<Vec<Volume>> {
        let argv = Argv::new(&["volume", "ls", "--json"]).take();
        let mut volumes: Vec<Volume> = self.cli.json(&argv).await?;
        for v in &mut volumes {
            // The CLI prints "-" for a size it could not determine; an absent
            // field says that more clearly than the placeholder.
            v.size = v.size.take().filter(|s| s != "-");
        }
        Ok(volumes)
    }

    pub async fn remove_volumes(&self, names: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("names", names)?;
        let argv = Argv::new(&["volume", "rm"])
            .flag("-f", force)
            .args(names.to_vec())
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    // -- networks ------------------------------------------------------------

    pub async fn list_networks(&self) -> OpResult<Vec<Network>> {
        let argv = Argv::new(&["network", "ls", "--json"]).take();
        Ok(self.cli.json(&argv).await?)
    }

    pub async fn create_network(&self, name: &str) -> OpResult<CommandResult> {
        let argv = Argv::new(&["network", "create"]).arg(name).take();
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn remove_networks(&self, names: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("names", names)?;
        let argv = Argv::new(&["network", "rm"])
            .flag("-f", force)
            .args(names.to_vec())
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn connect_network(&self, machine: &str, network: &str) -> OpResult<CommandResult> {
        let argv = Argv::new(&["network", "connect"])
            .arg(machine)
            .arg(network)
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn disconnect_network(&self, machine: &str) -> OpResult<CommandResult> {
        let argv = Argv::new(&["network", "disconnect"]).arg(machine).take();
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn sync_network(&self, network: &str) -> OpResult<CommandResult> {
        let argv = Argv::new(&["network", "sync"]).arg(network).take();
        Ok(self.cli.output(&argv).await?)
    }

    // -- flavors -------------------------------------------------------------

    pub async fn list_flavors(&self) -> OpResult<Vec<Flavor>> {
        let argv = Argv::new(&["flavors", "--json"]).take();
        Ok(self.cli.json(&argv).await?)
    }

    pub async fn add_flavor(&self, opts: &AddFlavorOpts) -> OpResult<CommandResult> {
        if opts.name.trim().is_empty() || opts.base.trim().is_empty() {
            return Err(OpError::InvalidArgument(
                "name and base are required".into(),
            ));
        }
        Ok(self.cli.output(&opts.to_argv()).await?)
    }

    pub async fn remove_flavors(&self, names: &[String], force: bool) -> OpResult<CommandResult> {
        require_non_empty("names", names)?;
        let argv = Argv::new(&["flavor", "rm"])
            .flag("-f", force)
            .args(names.to_vec())
            .take();
        Ok(self.cli.output(&argv).await?)
    }

    /// Environment for an interactive session, with `TERM` supplied when the
    /// guest needs one.
    ///
    /// Every interactive path must go through this. A BSD guest boots with no
    /// usable TERM and the CLI injects none for an explicit `exec` — by design,
    /// since a non-interactive exec should run verbatim — so a terminal opened
    /// that way comes up `dumb` unless the caller supplies it. A caller that set
    /// TERM itself always wins.
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
        let machines = self.list_machines(true).await.ok()?;
        machines
            .into_iter()
            .find(|m| m.id == id || m.name.as_deref() == Some(id) || m.id.starts_with(id))
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
        let argv = self.guest_tool_argv(tool, id, args)?;
        Ok(self.cli.output(&argv).await?)
    }

    pub async fn update_agent(&self, id: &str) -> OpResult<CommandResult> {
        Ok(self.cli.output(&self.update_agent_argv(id)).await?)
    }

    /// A machine's console log as a single string, for a non-following read.
    pub async fn machine_logs(&self, id: &str, boot: bool) -> OpResult<String> {
        let res = self.cli.output(&self.logs_argv(id, false, boot)).await?;
        // Console output goes to stdout and bsdkrun's own boot log to stderr,
        // so which one carries the content depends on `boot`.
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

    // -- argv for the streaming operations ------------------------------------
    //
    // These return a command line rather than running it: the caller decides
    // how to surface a stream (a gRPC response stream, a GraphQL subscription).

    pub fn logs_argv(&self, id: &str, follow: bool, boot: bool) -> Vec<String> {
        Argv::new(&["logs"])
            .flag("-f", follow)
            .flag("--boot", boot)
            .arg(id)
            .take()
    }

    pub fn update_agent_argv(&self, id: &str) -> Vec<String> {
        Argv::new(&["agent", "update"]).arg(id).take()
    }

    pub fn fetch_argv(
        &self,
        os: BsdOs,
        version: &Option<String>,
        dir: &Option<String>,
        force: bool,
    ) -> Vec<String> {
        Argv::new(&["fetch", "--os"])
            .arg(os.as_str())
            .opt("--version", version)
            .opt("--dir", dir)
            .flag("-f", force)
            .take()
    }

    pub fn build_flavor_argv(
        &self,
        name: &str,
        cpus: Option<u32>,
        mem: Option<u32>,
        force: bool,
    ) -> Vec<String> {
        Argv::new(&["flavor", "build"])
            .vm(cpus, mem)
            .flag("--force", force)
            .arg(name)
            .take()
    }

    pub fn guest_tool_argv(
        &self,
        tool: GuestTool,
        id: &str,
        args: &[String],
    ) -> OpResult<Vec<String>> {
        if args.is_empty() {
            return Err(OpError::InvalidArgument(
                "args must name an action, e.g. [\"status\"]".into(),
            ));
        }
        Ok(Argv::new(&[tool.as_str()])
            .arg(id)
            .args(args.to_vec())
            .take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_puts_flags_before_positionals() {
        let argv = Argv::new(&["linux", "-d"])
            .vm(Some(2), Some(1024))
            .each("-e", &["A=1".into()])
            .arg("alpine")
            .take();
        assert_eq!(
            argv,
            vec!["linux", "-d", "--cpus", "2", "--mem", "1024", "-e", "A=1", "alpine"]
        );
    }

    #[test]
    fn vm_zero_or_none_means_let_the_cli_decide() {
        assert_eq!(Argv::new(&["linux"]).vm(Some(0), Some(0)).take(), ["linux"]);
        assert_eq!(Argv::new(&["linux"]).vm(None, None).take(), ["linux"]);
    }

    #[test]
    fn net_opts_expand_every_field() {
        let argv = Argv::new(&["freebsd"])
            .net(&NetOpts {
                no_net: false,
                ports: vec!["2222:22".into(), "8080:80".into()],
                mac: None,
                network: Some("dev".into()),
                name: Some("web".into()),
            })
            .take();
        assert_eq!(
            argv,
            vec![
                "freebsd",
                "--port",
                "2222:22",
                "--port",
                "8080:80",
                "--network",
                "dev",
                "--name",
                "web"
            ]
        );
    }

    #[test]
    fn run_linux_is_detached_and_puts_the_command_last() {
        let opts = RunLinuxOpts {
            image: "alpine:3.20".into(),
            cpus: Some(4),
            mem: Some(2048),
            volume: Some("data".into()),
            env: vec!["A=1".into()],
            command: vec!["sh".into(), "-c".into(), "echo hi".into()],
            ..Default::default()
        };
        assert_eq!(
            opts.to_argv(),
            vec![
                "linux",
                "-d",
                "--cpus",
                "4",
                "--mem",
                "2048",
                "-v",
                "data",
                "-e",
                "A=1",
                "alpine:3.20",
                "--",
                "sh",
                "-c",
                "echo hi",
            ]
        );
    }

    #[test]
    fn exec_with_no_command_becomes_a_shell_and_forces_a_tty() {
        let (argv, tty) = ExecOpts {
            id: "abc".into(),
            tty: false, // deliberately false: a shell implies a tty regardless
            ..Default::default()
        }
        .to_argv();
        assert_eq!(argv, ["shell", "abc"]);
        assert!(tty);
    }

    #[test]
    fn exec_puts_flags_before_the_id() {
        let (argv, tty) = ExecOpts {
            id: "abc".into(),
            command: vec!["ls".into(), "-l".into()],
            env: vec!["K=V".into()],
            tty: true,
        }
        .to_argv();
        assert_eq!(argv, ["exec", "-t", "-e", "K=V", "abc", "ls", "-l"]);
        assert!(tty);
    }

    #[test]
    fn versions_parses_numeric_and_current_rows() {
        let v = parse_versions("Available FreeBSD builds:\n  15.1  (latest)\n  14.3\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].version, "15.1");
        assert!(v[0].latest);
        assert!(!v[1].latest);

        let v = parse_versions("Available NetBSD builds:\n  current\n  10.1\n");
        assert_eq!(v[0].version, "current");
        assert!(v[0].latest);
    }

    #[test]
    fn empty_id_lists_are_refused_before_running_anything() {
        assert!(matches!(
            require_non_empty("ids", &[]),
            Err(OpError::InvalidArgument(_))
        ));
        assert!(require_non_empty("ids", &["a".to_string()]).is_ok());
    }
}
