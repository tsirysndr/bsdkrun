//! The GraphQL API, for the web frontend.
//!
//! Served by the same daemon as gRPC and backed by the same [`crate::ops`], so
//! the two front ends cannot drift in what commands they actually run.
//!
//! Numbers are widened to `Float` where the CLI can produce values a GraphQL
//! `Int` cannot hold: `Int` is signed 32-bit, and an image size in bytes passes
//! that at 2 GiB. Sizes and timestamps are strings where the CLI already emits
//! text (a volume's "2.3 GiB", a unix timestamp), so nothing is reinterpreted
//! on the way through.
//!
//! Shell sessions are documented in [`crate::shell`] — a subscription cannot
//! carry input, so a terminal is assembled from a mutation, a subscription and
//! further mutations.

use std::sync::Arc;

use async_graphql::{
    Context, Enum, ErrorExtensions, InputObject, Object, SimpleObject, Subscription,
};
use base64::Engine as _;
use tokio_stream::{Stream, StreamExt};

use crate::ops::{self, OpError, Ops};
use crate::shell::{ShellEvent, ShellRegistry};

/// Shared state behind every resolver.
pub struct ApiContext {
    pub ops: Ops,
    pub shells: Arc<ShellRegistry>,
}

pub type BsdkrunSchema = async_graphql::Schema<Query, Mutation, Subscription_>;

pub fn schema(ops: Ops, shells: Arc<ShellRegistry>) -> BsdkrunSchema {
    async_graphql::Schema::build(Query, Mutation, Subscription_)
        .data(Arc::new(ApiContext { ops, shells }))
        .finish()
}

fn api<'a>(ctx: &Context<'a>) -> async_graphql::Result<&'a Arc<ApiContext>> {
    ctx.data::<Arc<ApiContext>>()
}

/// Map a domain error onto a GraphQL error, keeping the distinction between
/// "you asked for something impossible" and "it ran and failed" in the
/// extensions so a client can tell them apart without parsing the message.
fn gql_err(e: OpError) -> async_graphql::Error {
    let code = match e {
        OpError::InvalidArgument(_) => "INVALID_ARGUMENT",
        OpError::Failed(_) => "FAILED",
    };
    async_graphql::Error::new(e.to_string()).extend_with(|_, ext| ext.set("code", code))
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

// ---------------------------------------------------------------------------
// types
// ---------------------------------------------------------------------------

#[derive(SimpleObject)]
pub struct Machine {
    pub id: String,
    pub name: Option<String>,
    pub image: String,
    pub kind: String,
    pub command: String,
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub pid: Option<f64>,
    pub detached: bool,
    pub cpus: Option<i32>,
    pub mem: Option<f64>,
    pub volume: Option<String>,
    pub state_dir: Option<String>,
    pub created_at: Option<String>,
    pub finished_at: Option<String>,
    pub network: Option<String>,
    pub net_ip: Option<String>,
}

impl From<ops::Machine> for Machine {
    fn from(m: ops::Machine) -> Self {
        Machine {
            id: m.id,
            name: m.name,
            image: m.image,
            kind: m.kind,
            command: m.command,
            status: m.status,
            running: m.running,
            exit_code: m.exit_code.map(|v| v as i32),
            pid: m.pid.map(|v| v as f64),
            detached: m.detached,
            cpus: m.cpus.map(|v| v as i32),
            mem: m.mem.map(|v| v as f64),
            volume: m.volume,
            state_dir: m.state_dir,
            created_at: m.created_at,
            finished_at: m.finished_at,
            network: m.network,
            net_ip: m.net_ip,
        }
    }
}

#[derive(SimpleObject)]
pub struct Image {
    pub id: String,
    pub reference: String,
    pub digest: Option<String>,
    /// Bytes. A `Float` because an image easily exceeds a 32-bit `Int`.
    pub size: f64,
    pub rootfs: Option<String>,
    pub created_at: Option<String>,
}

impl From<ops::Image> for Image {
    fn from(i: ops::Image) -> Self {
        Image {
            id: i.id,
            reference: i.reference,
            digest: i.digest,
            size: i.size as f64,
            rootfs: i.rootfs,
            created_at: i.created_at,
        }
    }
}

#[derive(SimpleObject)]
pub struct Volume {
    pub name: String,
    pub guest: Option<String>,
    pub base: Option<String>,
    pub path: Option<String>,
    /// Human-readable, e.g. "2.3 GiB". Absent when the CLI could not measure it.
    pub size: Option<String>,
    pub created_at: Option<String>,
    pub tracked: bool,
}

impl From<ops::Volume> for Volume {
    fn from(v: ops::Volume) -> Self {
        Volume {
            name: v.name,
            guest: v.guest,
            base: v.base,
            path: v.path,
            size: v.size,
            created_at: v.created_at,
            tracked: v.tracked,
        }
    }
}

#[derive(SimpleObject)]
pub struct Network {
    pub name: String,
    pub subnet: String,
    pub gateway: String,
    pub members: i32,
    pub running: i32,
    pub up: bool,
    pub created_at: Option<String>,
}

impl From<ops::Network> for Network {
    fn from(n: ops::Network) -> Self {
        Network {
            name: n.name,
            subnet: n.subnet,
            gateway: n.gateway,
            members: n.members as i32,
            running: n.running as i32,
            up: n.up,
            created_at: n.created_at,
        }
    }
}

#[derive(SimpleObject)]
pub struct Flavor {
    pub name: String,
    /// "catalog" | "user" | "snapshot".
    pub source: String,
    /// "linux" | "freebsd" | "netbsd".
    pub kind: String,
    pub base: String,
    pub category: String,
    pub method: String,
    pub description: String,
    pub ports: Vec<String>,
    pub nix: Vec<String>,
    pub created_at: Option<String>,
}

impl From<ops::Flavor> for Flavor {
    fn from(f: ops::Flavor) -> Self {
        Flavor {
            name: f.name,
            source: f.source,
            kind: f.kind,
            base: f.base,
            category: f.category,
            method: f.method,
            description: f.description,
            ports: f.ports,
            nix: f.nix,
            created_at: f.created_at,
        }
    }
}

#[derive(SimpleObject)]
pub struct Version {
    pub version: String,
    pub latest: bool,
}

#[derive(SimpleObject)]
pub struct Info {
    pub daemon_version: String,
    pub cli_version: String,
    pub cli_path: String,
    pub os: String,
    pub arch: String,
}

/// The outcome of a command that runs to completion.
///
/// A non-zero `exitCode` is reported rather than raised: for several CLI
/// subcommands (`ssh status`, `tailscale status`) it is a legitimate state the
/// UI should display, not a transport failure.
#[derive(SimpleObject)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl From<crate::pb::CommandResult> for CommandResult {
    fn from(r: crate::pb::CommandResult) -> Self {
        CommandResult {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
        }
    }
}

#[derive(SimpleObject)]
pub struct ShellSessionInfo {
    pub id: String,
    pub machine_id: String,
    pub finished: bool,
    /// Output was dropped to stay under the session buffer cap, so the
    /// terminal's scrollback has a hole in it.
    pub truncated: bool,
}

/// One frame of a shell session.
///
/// Exactly one of `dataBase64` / `exitCode` is set. Bytes are base64 because a
/// terminal emits arbitrary binary (escape sequences, partial UTF-8 across
/// chunk boundaries) that would not survive being typed as a GraphQL `String`.
#[derive(SimpleObject)]
pub struct ShellOutput {
    pub data_base64: Option<String>,
    pub exit_code: Option<i32>,
}

/// Host resources, for the status bar.
#[derive(SimpleObject)]
pub struct SystemStats {
    /// Host CPU usage, 0-100.
    pub cpu: f64,
    /// Bytes. `Float` because RAM and disk exceed a 32-bit `Int`.
    pub mem_used: f64,
    pub mem_total: f64,
    /// Real (CoW-aware) bytes used by all microVMs and volumes.
    pub vm_disk: f64,
    pub vm_count: i32,
}

/// One step of a launch: a progress line, or the terminal event carrying the
/// new machine's id (or the error that stopped it).
///
/// Booting can take minutes — an image pull, provisioning, a BSD boot — so a
/// plain mutation would leave the UI with nothing to show. This streams the
/// same output a terminal user would watch scroll past.
#[derive(SimpleObject)]
pub struct LaunchEvent {
    pub line: Option<String>,
    /// Set exactly once, on success, when the machine id is known.
    pub machine_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum BsdOs {
    Freebsd,
    Netbsd,
}

impl From<BsdOs> for ops::BsdOs {
    fn from(o: BsdOs) -> Self {
        match o {
            BsdOs::Freebsd => ops::BsdOs::Freebsd,
            BsdOs::Netbsd => ops::BsdOs::Netbsd,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum GuestTool {
    Ssh,
    Tailscale,
    Systemd,
}

impl From<GuestTool> for ops::GuestTool {
    fn from(t: GuestTool) -> Self {
        match t {
            GuestTool::Ssh => ops::GuestTool::Ssh,
            GuestTool::Tailscale => ops::GuestTool::Tailscale,
            GuestTool::Systemd => ops::GuestTool::Systemd,
        }
    }
}

// ---------------------------------------------------------------------------
// inputs
// ---------------------------------------------------------------------------

#[derive(InputObject, Default)]
pub struct NetInput {
    #[graphql(default)]
    pub no_net: bool,
    /// Host->guest TCP forwards, each "HOST:GUEST".
    #[graphql(default)]
    pub ports: Vec<String>,
    pub mac: Option<String>,
    pub network: Option<String>,
    pub name: Option<String>,
}

impl From<NetInput> for ops::NetOpts {
    fn from(n: NetInput) -> Self {
        ops::NetOpts {
            no_net: n.no_net,
            ports: n.ports,
            mac: n.mac,
            network: n.network,
            name: n.name,
        }
    }
}

#[derive(InputObject)]
pub struct RunLinuxInput {
    pub image: String,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: Option<NetInput>,
    pub volume: Option<String>,
    #[graphql(default)]
    pub mounts: Vec<String>,
    #[graphql(default)]
    pub env: Vec<String>,
    pub entrypoint: Option<String>,
    #[graphql(default)]
    pub initramfs: bool,
    pub kernel: Option<String>,
    pub kernel_version: Option<String>,
    pub console: Option<String>,
    pub repo: Option<String>,
    #[graphql(default)]
    pub command: Vec<String>,
}

#[derive(InputObject)]
pub struct RunBsdInput {
    pub os: BsdOs,
    pub version: Option<String>,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: Option<NetInput>,
    pub volume: Option<String>,
    #[graphql(default)]
    pub persist: bool,
    #[graphql(default)]
    pub force: bool,
    pub firmware: Option<String>,
    #[graphql(default)]
    pub attach_disk: Vec<String>,
    pub disk_size: Option<String>,
    pub repo: Option<String>,
    #[graphql(default)]
    pub command: Vec<String>,
}

/// A unikernel has no disk and no agent, so this carries none of the
/// volume/persist/repo/command fields the other guests take.
#[derive(InputObject)]
pub struct RunUnikraftInput {
    /// A `kraft` project directory or a built unikernel image. Defaults to ".".
    pub path: Option<String>,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    pub net: Option<NetInput>,
    /// Kernel command line; Unikraft hands it to the application as argv.
    pub cmdline: Option<String>,
    pub initramfs: Option<String>,
    /// Persistent volumes over virtio-fs, each "HOST:GUEST" with an absolute
    /// guest path. Needs a unikernel built for it (examples/unikraft-volume).
    #[graphql(default)]
    pub mounts: Vec<String>,
}

#[derive(InputObject)]
pub struct RunFlavorInput {
    pub name: String,
    pub cpus: Option<u32>,
    pub mem: Option<u32>,
    #[graphql(default)]
    pub ports: Vec<String>,
    pub volume: Option<String>,
    pub repo: Option<String>,
}

#[derive(InputObject)]
pub struct AddFlavorInput {
    pub name: String,
    pub base: String,
    #[graphql(default)]
    pub category: String,
    #[graphql(default)]
    pub description: String,
    #[graphql(default)]
    pub ports: Vec<String>,
    #[graphql(default)]
    pub env: Vec<String>,
    #[graphql(default)]
    pub nix: Vec<String>,
    #[graphql(default)]
    pub provision: Vec<String>,
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

pub struct Query;

#[Object]
impl Query {
    /// The daemon and the CLI it drives.
    async fn info(&self, ctx: &Context<'_>) -> async_graphql::Result<Info> {
        let i = api(ctx)?.ops.info().await.map_err(gql_err)?;
        Ok(Info {
            daemon_version: i.daemon_version,
            cli_version: i.cli_version,
            cli_path: i.cli_path,
            os: i.os,
            arch: i.arch,
        })
    }

    /// Machines. `all` includes stopped ones, like `ps -a`.
    async fn machines(
        &self,
        ctx: &Context<'_>,
        #[graphql(default)] all: bool,
    ) -> async_graphql::Result<Vec<Machine>> {
        Ok(api(ctx)?
            .ops
            .list_machines(all)
            .await
            .map_err(gql_err)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// A single machine by id or name, or null if there is no such machine.
    async fn machine(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<Option<Machine>> {
        let machines = api(ctx)?.ops.list_machines(true).await.map_err(gql_err)?;
        Ok(machines
            .into_iter()
            // Ids may be given as a unique prefix, exactly as on the CLI.
            .find(|m| m.id == id || m.name.as_deref() == Some(&id) || m.id.starts_with(&id))
            .map(Into::into))
    }

    async fn images(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Image>> {
        Ok(api(ctx)?
            .ops
            .list_images()
            .await
            .map_err(gql_err)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn volumes(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Volume>> {
        Ok(api(ctx)?
            .ops
            .list_volumes()
            .await
            .map_err(gql_err)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn networks(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Network>> {
        Ok(api(ctx)?
            .ops
            .list_networks()
            .await
            .map_err(gql_err)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn flavors(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<Flavor>> {
        Ok(api(ctx)?
            .ops
            .list_flavors()
            .await
            .map_err(gql_err)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Downloadable builds for a guest OS.
    async fn versions(&self, ctx: &Context<'_>, os: BsdOs) -> async_graphql::Result<Vec<Version>> {
        Ok(api(ctx)?
            .ops
            .list_versions(os.into())
            .await
            .map_err(gql_err)?
            .into_iter()
            .map(|v| Version {
                version: v.version,
                latest: v.latest,
            })
            .collect())
    }

    /// Host CPU, RAM and microVM disk usage.
    async fn system_stats(&self, ctx: &Context<'_>) -> async_graphql::Result<SystemStats> {
        let s = api(ctx)?.ops.system_stats().await.map_err(gql_err)?;
        Ok(SystemStats {
            cpu: s.cpu as f64,
            mem_used: s.mem_used as f64,
            mem_total: s.mem_total as f64,
            vm_disk: s.vm_disk as f64,
            vm_count: s.vm_count as i32,
        })
    }

    /// A machine's console log as a single string. Use the `machineLogs`
    /// subscription to follow it live instead.
    async fn machine_logs(
        &self,
        ctx: &Context<'_>,
        id: String,
        #[graphql(default)] boot: bool,
    ) -> async_graphql::Result<String> {
        api(ctx)?.ops.machine_logs(&id, boot).await.map_err(gql_err)
    }

    /// Currently open shell sessions.
    async fn shell_sessions(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<ShellSessionInfo>> {
        Ok(api(ctx)?
            .shells
            .list()
            .into_iter()
            .map(|s| ShellSessionInfo {
                id: s.id.clone(),
                machine_id: s.machine_id.clone(),
                finished: s.is_finished(),
                truncated: s.truncated(),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Mutation
// ---------------------------------------------------------------------------

pub struct Mutation;

#[Object]
impl Mutation {
    // -- machine lifecycle ---------------------------------------------------

    async fn start_machine(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?.ops.start(&id).await.map_err(gql_err)?.into())
    }

    async fn stop_machine(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?.ops.stop(&id).await.map_err(gql_err)?.into())
    }

    async fn remove_machines(
        &self,
        ctx: &Context<'_>,
        ids: Vec<String>,
        #[graphql(default)] force: bool,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .remove_machines(&ids, force)
            .await
            .map_err(gql_err)?
            .into())
    }

    /// Change a machine's recorded vCPU / memory. Takes effect on next start,
    /// since libkrun fixes VM resources at boot.
    async fn update_machine(
        &self,
        ctx: &Context<'_>,
        id: String,
        cpus: Option<u32>,
        mem: Option<u32>,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .update_machine(&id, cpus, mem)
            .await
            .map_err(gql_err)?
            .into())
    }

    /// Snapshot a machine into a named flavor, like `docker commit`.
    async fn commit_machine(
        &self,
        ctx: &Context<'_>,
        id: String,
        name: String,
        #[graphql(default)] description: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .commit(&id, &name, &description)
            .await
            .map_err(gql_err)?
            .into())
    }

    // -- booting -------------------------------------------------------------
    //
    // Always detached, returning the new machine id: the daemon outlives any
    // one request, so a foreground VM would have nowhere to live.

    async fn run_linux(
        &self,
        ctx: &Context<'_>,
        input: RunLinuxInput,
    ) -> async_graphql::Result<String> {
        let opts = ops::RunLinuxOpts {
            image: input.image,
            cpus: input.cpus,
            mem: input.mem,
            net: input.net.map(Into::into).unwrap_or_default(),
            volume: input.volume,
            mounts: input.mounts,
            env: input.env,
            entrypoint: input.entrypoint,
            initramfs: input.initramfs,
            kernel: input.kernel,
            kernel_version: input.kernel_version,
            console: input.console,
            repo: input.repo,
            command: input.command,
        };
        api(ctx)?.ops.run_linux(&opts).await.map_err(gql_err)
    }

    async fn run_bsd(
        &self,
        ctx: &Context<'_>,
        input: RunBsdInput,
    ) -> async_graphql::Result<String> {
        let opts = ops::RunBsdOpts {
            os: input.os.into(),
            version: input.version,
            cpus: input.cpus,
            mem: input.mem,
            net: input.net.map(Into::into).unwrap_or_default(),
            volume: input.volume,
            persist: input.persist,
            force: input.force,
            firmware: input.firmware,
            attach_disk: input.attach_disk,
            disk_size: input.disk_size,
            repo: input.repo,
            command: input.command,
        };
        api(ctx)?.ops.run_bsd(&opts).await.map_err(gql_err)
    }

    async fn run_unikraft(
        &self,
        ctx: &Context<'_>,
        input: RunUnikraftInput,
    ) -> async_graphql::Result<String> {
        let opts = ops::RunUnikraftOpts {
            path: input.path,
            cpus: input.cpus,
            mem: input.mem,
            net: input.net.map(Into::into).unwrap_or_default(),
            cmdline: input.cmdline,
            initramfs: input.initramfs,
            mounts: input.mounts,
        };
        api(ctx)?.ops.run_unikraft(&opts).await.map_err(gql_err)
    }

    async fn run_flavor(
        &self,
        ctx: &Context<'_>,
        input: RunFlavorInput,
    ) -> async_graphql::Result<String> {
        let opts = ops::RunFlavorOpts {
            name: input.name,
            cpus: input.cpus,
            mem: input.mem,
            ports: input.ports,
            volume: input.volume,
            repo: input.repo,
        };
        api(ctx)?.ops.run_flavor(&opts).await.map_err(gql_err)
    }

    // -- volumes / networks / flavors ----------------------------------------

    async fn remove_volumes(
        &self,
        ctx: &Context<'_>,
        names: Vec<String>,
        #[graphql(default)] force: bool,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .remove_volumes(&names, force)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn create_network(
        &self,
        ctx: &Context<'_>,
        name: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .create_network(&name)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn remove_networks(
        &self,
        ctx: &Context<'_>,
        names: Vec<String>,
        #[graphql(default)] force: bool,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .remove_networks(&names, force)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn connect_network(
        &self,
        ctx: &Context<'_>,
        machine: String,
        network: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .connect_network(&machine, &network)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn disconnect_network(
        &self,
        ctx: &Context<'_>,
        machine: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .disconnect_network(&machine)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn sync_network(
        &self,
        ctx: &Context<'_>,
        network: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .sync_network(&network)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn add_flavor(
        &self,
        ctx: &Context<'_>,
        input: AddFlavorInput,
    ) -> async_graphql::Result<CommandResult> {
        let opts = ops::AddFlavorOpts {
            name: input.name,
            base: input.base,
            category: input.category,
            description: input.description,
            ports: input.ports,
            env: input.env,
            nix: input.nix,
            provision: input.provision,
        };
        Ok(api(ctx)?
            .ops
            .add_flavor(&opts)
            .await
            .map_err(gql_err)?
            .into())
    }

    async fn remove_flavors(
        &self,
        ctx: &Context<'_>,
        names: Vec<String>,
        #[graphql(default)] force: bool,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .remove_flavors(&names, force)
            .await
            .map_err(gql_err)?
            .into())
    }

    // -- guest tools ---------------------------------------------------------

    /// Run an in-guest agent action, e.g. `ssh` with ["setup"] or `tailscale`
    /// with ["status"]. A non-zero `exitCode` is a state to display, not a
    /// failure — "not installed" and "not running" both arrive that way.
    async fn guest_tool(
        &self,
        ctx: &Context<'_>,
        id: String,
        tool: GuestTool,
        args: Vec<String>,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .guest_tool(tool.into(), &id, &args)
            .await
            .map_err(gql_err)?
            .into())
    }

    /// Install the current agent inside a running guest, over a stale one.
    async fn update_agent(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<CommandResult> {
        Ok(api(ctx)?
            .ops
            .update_agent(&id)
            .await
            .map_err(gql_err)?
            .into())
    }

    // -- shell sessions ------------------------------------------------------

    /// Open an interactive session on a machine and return its id.
    ///
    /// Output is buffered from this moment, so nothing written before the
    /// client subscribes is lost. With no `command`, this opens the machine's
    /// shell.
    async fn open_shell(
        &self,
        ctx: &Context<'_>,
        machine_id: String,
        #[graphql(default)] command: Vec<String>,
        #[graphql(default)] env: Vec<String>,
        #[graphql(default = 24)] rows: u32,
        #[graphql(default = 80)] cols: u32,
    ) -> async_graphql::Result<ShellSessionInfo> {
        let api = api(ctx)?;
        let env = api.ops.interactive_env(&machine_id, env).await;
        let session = api
            .shells
            .open(
                &api.ops,
                &machine_id,
                command,
                env,
                rows as u16,
                cols as u16,
            )
            .map_err(gql_err)?;
        Ok(ShellSessionInfo {
            id: session.id.clone(),
            machine_id: session.machine_id.clone(),
            finished: false,
            truncated: false,
        })
    }

    /// Send keystrokes. `dataBase64` so arbitrary bytes (control characters,
    /// escape sequences) survive the trip.
    async fn send_shell_input(
        &self,
        ctx: &Context<'_>,
        session_id: String,
        data_base64: String,
    ) -> async_graphql::Result<bool> {
        let bytes = b64().decode(data_base64.as_bytes()).map_err(|e| {
            async_graphql::Error::new(format!("dataBase64 is not valid base64: {e}"))
        })?;
        api(ctx)?
            .shells
            .get(&session_id)
            .map_err(gql_err)?
            .write(bytes);
        Ok(true)
    }

    /// Apply a terminal resize, so full-screen programs in the guest redraw.
    async fn resize_shell(
        &self,
        ctx: &Context<'_>,
        session_id: String,
        rows: u32,
        cols: u32,
    ) -> async_graphql::Result<bool> {
        api(ctx)?
            .shells
            .get(&session_id)
            .map_err(gql_err)?
            .resize(rows as u16, cols as u16);
        Ok(true)
    }

    /// Close a session and kill its command. Idempotent, so tearing down a
    /// terminal never has to care whether the shell already exited.
    async fn close_shell(
        &self,
        ctx: &Context<'_>,
        session_id: String,
    ) -> async_graphql::Result<bool> {
        api(ctx)?.shells.close(&session_id).map_err(gql_err)?;
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// Named with a trailing underscore because `Subscription` is the derive macro.
pub struct Subscription_;

#[Subscription]
impl Subscription_ {
    /// A shell session's output: everything buffered since it opened, then
    /// live output, ending with `exitCode`.
    async fn shell_output(
        &self,
        ctx: &Context<'_>,
        session_id: String,
    ) -> async_graphql::Result<impl Stream<Item = ShellOutput>> {
        let session = api(ctx)?.shells.get(&session_id).map_err(gql_err)?;
        Ok(session.subscribe().map(|event| match event {
            ShellEvent::Output(bytes) => ShellOutput {
                data_base64: Some(b64().encode(bytes)),
                exit_code: None,
            },
            ShellEvent::Exit(code) => ShellOutput {
                data_base64: None,
                exit_code: Some(code),
            },
        }))
    }

    /// A machine's console log, as `bsdkrun logs -f` produces it. Ends when the
    /// command exits; with `follow` that is when the machine stops.
    async fn machine_logs(
        &self,
        ctx: &Context<'_>,
        id: String,
        #[graphql(default = true)] follow: bool,
        #[graphql(default)] boot: bool,
    ) -> async_graphql::Result<impl Stream<Item = ShellOutput>> {
        let api = api(ctx)?;
        let argv = api.ops.logs_argv(&id, follow, boot);
        let rx = api.ops.cli().stream(&argv);
        Ok(
            tokio_stream::wrappers::ReceiverStream::new(rx).filter_map(|chunk| {
                let chunk = chunk.ok()?;
                match chunk.payload {
                    Some(crate::pb::output_chunk::Payload::Stdout(b))
                    | Some(crate::pb::output_chunk::Payload::Stderr(b)) => Some(ShellOutput {
                        data_base64: Some(b64().encode(b)),
                        exit_code: None,
                    }),
                    Some(crate::pb::output_chunk::Payload::ExitCode(c)) => Some(ShellOutput {
                        data_base64: None,
                        exit_code: Some(c),
                    }),
                    None => None,
                }
            }),
        )
    }

    /// Boot a Linux machine, streaming progress until its id is known.
    async fn launch_linux(
        &self,
        ctx: &Context<'_>,
        input: RunLinuxInput,
    ) -> async_graphql::Result<impl Stream<Item = LaunchEvent>> {
        let api = api(ctx)?;
        let opts = ops::RunLinuxOpts {
            image: input.image,
            cpus: input.cpus,
            mem: input.mem,
            net: input.net.map(Into::into).unwrap_or_default(),
            volume: input.volume,
            mounts: input.mounts,
            env: input.env,
            entrypoint: input.entrypoint,
            initramfs: input.initramfs,
            kernel: input.kernel,
            kernel_version: input.kernel_version,
            console: input.console,
            repo: input.repo,
            command: input.command,
        };
        Ok(launch_stream(api.ops.clone(), opts.to_argv()))
    }

    /// Boot a FreeBSD/NetBSD machine, streaming progress until its id is known.
    async fn launch_bsd(
        &self,
        ctx: &Context<'_>,
        input: RunBsdInput,
    ) -> async_graphql::Result<impl Stream<Item = LaunchEvent>> {
        let api = api(ctx)?;
        let opts = ops::RunBsdOpts {
            os: input.os.into(),
            version: input.version,
            cpus: input.cpus,
            mem: input.mem,
            net: input.net.map(Into::into).unwrap_or_default(),
            volume: input.volume,
            persist: input.persist,
            force: input.force,
            firmware: input.firmware,
            attach_disk: input.attach_disk,
            disk_size: input.disk_size,
            repo: input.repo,
            command: input.command,
        };
        Ok(launch_stream(api.ops.clone(), opts.to_argv()))
    }

    /// Boot a Unikraft unikernel, streaming progress until its id is known.
    async fn launch_unikraft(
        &self,
        ctx: &Context<'_>,
        input: RunUnikraftInput,
    ) -> async_graphql::Result<impl Stream<Item = LaunchEvent>> {
        let api = api(ctx)?;
        let opts = ops::RunUnikraftOpts {
            path: input.path,
            cpus: input.cpus,
            mem: input.mem,
            net: input.net.map(Into::into).unwrap_or_default(),
            cmdline: input.cmdline,
            initramfs: input.initramfs,
            mounts: input.mounts,
        };
        Ok(launch_stream(api.ops.clone(), opts.to_argv()))
    }

    /// Boot a flavor, streaming provisioning output until its id is known.
    async fn launch_flavor(
        &self,
        ctx: &Context<'_>,
        input: RunFlavorInput,
    ) -> async_graphql::Result<impl Stream<Item = LaunchEvent>> {
        let api = api(ctx)?;
        let opts = ops::RunFlavorOpts {
            name: input.name,
            cpus: input.cpus,
            mem: input.mem,
            ports: input.ports,
            volume: input.volume,
            repo: input.repo,
        };
        Ok(launch_stream(api.ops.clone(), opts.to_argv()))
    }

    /// Pre-build a flavor's provisioned rootfs, streaming the build log. No
    /// machine is started, so no `machineId` is ever emitted.
    async fn build_flavor(
        &self,
        ctx: &Context<'_>,
        name: String,
        cpus: Option<u32>,
        mem: Option<u32>,
        #[graphql(default)] force: bool,
    ) -> async_graphql::Result<impl Stream<Item = LaunchEvent>> {
        let api = api(ctx)?;
        let argv = api.ops.build_flavor_argv(&name, cpus, mem, force);
        Ok(launch_stream(api.ops.clone(), argv))
    }

    /// Poll-backed machine list, so a dashboard can stay current without the
    /// frontend running its own timer.
    ///
    /// The CLI has no change feed — machine state lives in a SQLite file that
    /// `ps` reads — so this genuinely polls. `intervalMs` is clamped to a floor
    /// because each tick forks a CLI process.
    async fn machines_changed(
        &self,
        ctx: &Context<'_>,
        #[graphql(default)] all: bool,
        #[graphql(default = 2000)] interval_ms: u32,
    ) -> async_graphql::Result<impl Stream<Item = Vec<Machine>>> {
        let ops = api(ctx)?.ops.clone();
        let period = std::time::Duration::from_millis(interval_ms.max(500) as u64);
        Ok(async_stream::stream! {
            let mut ticker = tokio::time::interval(period);
            loop {
                ticker.tick().await;
                if let Ok(machines) = ops.list_machines(all).await {
                    yield machines.into_iter().map(Into::into).collect::<Vec<_>>();
                }
            }
        })
    }
}

/// Run a boot command and turn its output into [`LaunchEvent`]s.
///
/// The CLI writes progress to stderr and the new machine's id to stdout, so the
/// two streams are classified separately. An id is a bare 12-hex-digit line —
/// the same shape the desktop app looks for — and anything else on stdout is
/// just more progress.
fn launch_stream(ops: Ops, argv: Vec<String>) -> impl Stream<Item = LaunchEvent> {
    async_stream::stream! {
        let mut rx = ops.cli().stream(&argv);
        let mut machine_id: Option<String> = None;
        let mut stdout = String::new();
        let mut stderr = String::new();

        while let Some(chunk) = rx.recv().await {
            let Ok(chunk) = chunk else { continue };
            match chunk.payload {
                Some(crate::pb::output_chunk::Payload::Stdout(b)) => {
                    stdout.push_str(&String::from_utf8_lossy(&b));
                    while let Some(nl) = stdout.find('\n') {
                        let line: String = stdout.drain(..=nl).collect();
                        let t = line.trim().to_string();
                        if t.is_empty() {
                            continue;
                        }
                        if machine_id.is_none() && is_machine_id(&t) {
                            machine_id = Some(t);
                        } else {
                            yield LaunchEvent { line: Some(t), machine_id: None, error: None };
                        }
                    }
                }
                Some(crate::pb::output_chunk::Payload::Stderr(b)) => {
                    stderr.push_str(&String::from_utf8_lossy(&b));
                    while let Some(nl) = stderr.find('\n') {
                        let line: String = stderr.drain(..=nl).collect();
                        let t = line.trim_end().to_string();
                        if !t.trim().is_empty() {
                            yield LaunchEvent { line: Some(t), machine_id: None, error: None };
                        }
                    }
                }
                Some(crate::pb::output_chunk::Payload::ExitCode(code)) => {
                    // Flush whatever was still buffered without a trailing newline.
                    for rest in [stdout.trim(), stderr.trim()] {
                        if !rest.is_empty() {
                            yield LaunchEvent {
                                line: Some(rest.to_string()),
                                machine_id: None,
                                error: None,
                            };
                        }
                    }
                    yield if code == 0 {
                        LaunchEvent { line: None, machine_id: machine_id.clone(), error: None }
                    } else {
                        LaunchEvent {
                            line: None,
                            machine_id: None,
                            error: Some(format!("bsdkrun exited with status {code}")),
                        }
                    };
                    return;
                }
                None => {}
            }
        }
    }
}

/// A machine id as the CLI prints it: exactly 12 hex digits on a line of its own.
fn is_machine_id(line: &str) -> bool {
    line.len() == 12 && line.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_ids_are_told_apart_from_progress_lines() {
        assert!(is_machine_id("fab8f81e4f91"));
        assert!(is_machine_id("000000000000"));
        // Progress output must never be mistaken for an id.
        assert!(!is_machine_id("pulling alpine:3.20"));
        assert!(!is_machine_id("fab8f81e4f9")); // 11
        assert!(!is_machine_id("fab8f81e4f912")); // 13
        assert!(!is_machine_id("zzzzzzzzzzzz"));
        assert!(!is_machine_id(""));
    }
}
