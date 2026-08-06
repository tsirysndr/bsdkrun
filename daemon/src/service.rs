//! The `Bsdkrun` service: every RPC translated into a CLI invocation.
//!
//! Typed RPCs build an argv, run it, and (for list calls) parse the CLI's
//! `--json` output into proto messages. The generic [`Run`](BsdkrunService::run)
//! passthrough forwards an argv verbatim, which is what keeps the long tail
//! (probe/kernel/firmware/grow/store) and any newer subcommand reachable
//! without a proto change.
//!
//! Machines launched through the daemon are always detached. The daemon
//! outlives any single RPC, so a foreground VM would have nowhere to live; the
//! boot RPCs therefore return a machine id and clients use `Logs` to watch it
//! boot and `Exec` to get a shell.

use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::cli::Cli;
use crate::pb::bsdkrun_server::Bsdkrun;
use crate::pb::*;
use crate::pty::{PtySession, DEFAULT_COLS, DEFAULT_ROWS};

pub struct BsdkrunService {
    cli: Cli,
}

impl BsdkrunService {
    pub fn new(cli: Cli) -> Self {
        Self { cli }
    }
}

/// Boxed because an interactive RPC returns one of two concrete streams
/// depending on whether a terminal was requested.
type ChunkStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<OutputChunk, Status>> + Send>>;

// ---------------------------------------------------------------------------
// argv building
// ---------------------------------------------------------------------------

/// A small builder so each RPC reads as the command line it produces.
///
/// Flags are always emitted *before* positionals. clap accepts them
/// interspersed, but several subcommands take `trailing_var_arg` /
/// `last = true` arguments that swallow everything after the positional, so
/// keeping flags in front is the only ordering that is correct everywhere.
#[derive(Default)]
struct Argv(Vec<String>);

impl Argv {
    fn new(parts: &[&str]) -> Self {
        Self(parts.iter().map(|s| s.to_string()).collect())
    }

    fn arg(&mut self, v: impl Into<String>) -> &mut Self {
        self.0.push(v.into());
        self
    }

    fn args<I: IntoIterator<Item = S>, S: Into<String>>(&mut self, vs: I) -> &mut Self {
        self.0.extend(vs.into_iter().map(Into::into));
        self
    }

    fn flag(&mut self, name: &str, on: bool) -> &mut Self {
        if on {
            self.0.push(name.to_string());
        }
        self
    }

    fn opt<T: ToString>(&mut self, name: &str, v: &Option<T>) -> &mut Self {
        if let Some(v) = v {
            self.0.push(name.to_string());
            self.0.push(v.to_string());
        }
        self
    }

    /// A repeatable `--flag VALUE` option.
    fn each(&mut self, name: &str, vs: &[String]) -> &mut Self {
        for v in vs {
            self.0.push(name.to_string());
            self.0.push(v.clone());
        }
        self
    }

    /// `VmConfig` uses 0 to mean "unset", letting the CLI apply its own default
    /// rather than this daemon pinning one.
    fn vm(&mut self, vm: &Option<VmConfig>) -> &mut Self {
        if let Some(vm) = vm {
            if vm.cpus > 0 {
                self.arg("--cpus").arg(vm.cpus.to_string());
            }
            if vm.mem > 0 {
                self.arg("--mem").arg(vm.mem.to_string());
            }
        }
        self
    }

    fn net(&mut self, net: &Option<NetConfig>) -> &mut Self {
        if let Some(n) = net {
            self.flag("--no-net", n.no_net)
                .each("--port", &n.ports)
                .opt("--mac", &n.mac)
                .opt("--network", &n.network)
                .opt("--name", &n.name);
        }
        self
    }

    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.0)
    }
}

fn bsd_os(os: i32) -> Result<&'static str, Status> {
    match BsdOs::try_from(os) {
        Ok(BsdOs::Freebsd) => Ok("freebsd"),
        Ok(BsdOs::Netbsd) => Ok("netbsd"),
        _ => Err(Status::invalid_argument(
            "os must be BSD_OS_FREEBSD or BSD_OS_NETBSD",
        )),
    }
}

// ---------------------------------------------------------------------------
// JSON shapes emitted by the CLI's `--json` subcommands
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct JsonMachine {
    id: String,
    #[serde(default)]
    name: Option<String>,
    image: String,
    kind: String,
    command: String,
    status: String,
    running: bool,
    exit_code: Option<i64>,
    pid: Option<i64>,
    detached: bool,
    cpus: Option<i64>,
    mem: Option<i64>,
    volume: Option<String>,
    state_dir: Option<String>,
    created_at: Option<String>,
    finished_at: Option<String>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    net_ip: Option<String>,
}

#[derive(Deserialize)]
struct JsonImage {
    id: String,
    reference: String,
    digest: Option<String>,
    size: i64,
    rootfs: Option<String>,
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct JsonVolume {
    name: String,
    guest: Option<String>,
    base: Option<String>,
    path: Option<String>,
    /// Human-readable ("2.3 GiB"), or "-" when the size could not be measured.
    size: Option<String>,
    created_at: Option<String>,
    tracked: bool,
}

#[derive(Deserialize)]
struct JsonNetwork {
    name: String,
    subnet: String,
    gateway: String,
    members: i64,
    running: i64,
    up: bool,
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct JsonFlavor {
    name: String,
    source: String,
    kind: String,
    base: String,
    category: String,
    #[serde(default)]
    method: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    ports: Vec<String>,
    #[serde(default)]
    nix: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
}

/// Parse `versions --os <os>`, which prints indented `  <ver>  (latest)` rows.
/// NetBSD's recommended build is the literal `current` rather than a number.
fn parse_versions(out: &str) -> Vec<Version> {
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
// Service
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl Bsdkrun for BsdkrunService {
    type UpdateAgentStream = ChunkStream;
    type FetchStream = ChunkStream;
    type BuildFlavorStream = ChunkStream;
    type ExecStream = ChunkStream;
    type LogsStream = ChunkStream;
    type GuestToolCallStream = ChunkStream;
    type RunStream = ChunkStream;

    // -- daemon --------------------------------------------------------------

    async fn info(&self, _: Request<InfoRequest>) -> Result<Response<InfoResponse>, Status> {
        let res = self.cli.output(&["--version".to_string()]).await?;
        Ok(Response::new(InfoResponse {
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            cli_version: res.stdout.trim().to_string(),
            cli_path: self.cli.bin().display().to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }))
    }

    // -- machines ------------------------------------------------------------

    async fn list_machines(
        &self,
        req: Request<ListMachinesRequest>,
    ) -> Result<Response<ListMachinesResponse>, Status> {
        let mut argv = Argv::new(&["ps"]);
        argv.flag("-a", req.into_inner().all).arg("--json");
        let raw: Vec<JsonMachine> = self.cli.json(&argv.take()).await?;
        let machines = raw
            .into_iter()
            .map(|m| Machine {
                id: m.id,
                name: m.name,
                image: m.image,
                kind: m.kind,
                command: m.command,
                status: m.status,
                running: m.running,
                exit_code: m.exit_code,
                pid: m.pid,
                detached: m.detached,
                cpus: m.cpus,
                mem: m.mem,
                volume: m.volume,
                state_dir: m.state_dir,
                created_at: m.created_at,
                finished_at: m.finished_at,
                network: m.network,
                net_ip: m.net_ip,
            })
            .collect();
        Ok(Response::new(ListMachinesResponse { machines }))
    }

    async fn start(&self, req: Request<MachineRequest>) -> Result<Response<CommandResult>, Status> {
        let argv = Argv::new(&["start"]).arg(req.into_inner().id).take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn stop(&self, req: Request<MachineRequest>) -> Result<Response<CommandResult>, Status> {
        let argv = Argv::new(&["stop"]).arg(req.into_inner().id).take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn remove_machines(
        &self,
        req: Request<RemoveMachinesRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        if r.ids.is_empty() {
            return Err(Status::invalid_argument("ids must not be empty"));
        }
        let argv = Argv::new(&["rm"]).flag("-f", r.force).args(r.ids).take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn update_machine(
        &self,
        req: Request<UpdateMachineRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        let argv = Argv::new(&["update"])
            .opt("--cpus", &r.cpus)
            .opt("--mem", &r.mem)
            .arg(r.id)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn commit(&self, req: Request<CommitRequest>) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        let argv = Argv::new(&["commit"])
            .arg("-d")
            .arg(r.description)
            .arg(r.id)
            .arg(r.name)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn update_agent(
        &self,
        req: Request<MachineRequest>,
    ) -> Result<Response<Self::UpdateAgentStream>, Status> {
        let argv = Argv::new(&["agent", "update"])
            .arg(req.into_inner().id)
            .take();
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.cli.stream(&argv),
        ))))
    }

    // -- booting -------------------------------------------------------------

    async fn run_linux(
        &self,
        req: Request<RunLinuxRequest>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        if r.image.trim().is_empty() {
            return Err(Status::invalid_argument("image must not be empty"));
        }
        let mut argv = Argv::new(&["linux", "-d"]);
        argv.vm(&r.vm)
            .net(&r.net)
            .opt("-v", &r.volume)
            .each("--mount", &r.mounts)
            .each("-e", &r.env)
            .opt("--entrypoint", &r.entrypoint)
            .flag("--initramfs", r.initramfs)
            .opt("--kernel", &r.kernel)
            .opt("--kernel-version", &r.kernel_version)
            .opt("--console", &r.console)
            .opt("--repo", &r.repo)
            .arg(r.image);
        if !r.command.is_empty() {
            argv.arg("--").args(r.command);
        }
        let id = self.cli.detached(&argv.take()).await?;
        Ok(Response::new(RunResponse { id }))
    }

    async fn run_bsd(&self, req: Request<RunBsdRequest>) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let os = bsd_os(r.os)?;
        let mut argv = Argv::new(&[os, "-d"]);
        argv.vm(&r.vm)
            .net(&r.net)
            .opt("--version", &r.version)
            .opt("-v", &r.volume)
            .flag("--persist", r.persist)
            .flag("-f", r.force)
            .opt("--firmware", &r.firmware)
            .each("--attach-disk", &r.attach_disk)
            .opt("--disk-size", &r.disk_size)
            .opt("--repo", &r.repo);
        if !r.command.is_empty() {
            argv.arg("--").args(r.command);
        }
        let id = self.cli.detached(&argv.take()).await?;
        Ok(Response::new(RunResponse { id }))
    }

    async fn run_flavor(
        &self,
        req: Request<RunFlavorRequest>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        if r.name.trim().is_empty() {
            return Err(Status::invalid_argument("name must not be empty"));
        }
        let argv = Argv::new(&["flavor", "run", "-d"])
            .vm(&r.vm)
            .each("--port", &r.ports)
            .opt("-v", &r.volume)
            .opt("--repo", &r.repo)
            .arg(r.name)
            .take();
        let id = self.cli.detached(&argv).await?;
        Ok(Response::new(RunResponse { id }))
    }

    // -- images --------------------------------------------------------------

    async fn list_images(
        &self,
        _: Request<ListImagesRequest>,
    ) -> Result<Response<ListImagesResponse>, Status> {
        let raw: Vec<JsonImage> = self
            .cli
            .json(&["images".to_string(), "--json".to_string()])
            .await?;
        let images = raw
            .into_iter()
            .map(|i| Image {
                id: i.id,
                reference: i.reference,
                digest: i.digest,
                size: i.size,
                rootfs: i.rootfs,
                created_at: i.created_at,
            })
            .collect();
        Ok(Response::new(ListImagesResponse { images }))
    }

    async fn fetch(
        &self,
        req: Request<FetchRequest>,
    ) -> Result<Response<Self::FetchStream>, Status> {
        let r = req.into_inner();
        let argv = Argv::new(&["fetch", "--os"])
            .arg(bsd_os(r.os)?)
            .opt("--version", &r.version)
            .opt("--dir", &r.dir)
            .flag("-f", r.force)
            .take();
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.cli.stream(&argv),
        ))))
    }

    async fn list_versions(
        &self,
        req: Request<ListVersionsRequest>,
    ) -> Result<Response<ListVersionsResponse>, Status> {
        let argv = Argv::new(&["versions", "--os"])
            .arg(bsd_os(req.into_inner().os)?)
            .take();
        let res = self.cli.output(&argv).await?;
        if res.exit_code != 0 {
            return Err(Status::internal(format!(
                "`bsdkrun versions` failed ({}): {}",
                res.exit_code,
                res.stderr.trim()
            )));
        }
        Ok(Response::new(ListVersionsResponse {
            versions: parse_versions(&res.stdout),
        }))
    }

    // -- volumes -------------------------------------------------------------

    async fn list_volumes(
        &self,
        _: Request<ListVolumesRequest>,
    ) -> Result<Response<ListVolumesResponse>, Status> {
        let raw: Vec<JsonVolume> = self
            .cli
            .json(&["volume".to_string(), "ls".to_string(), "--json".to_string()])
            .await?;
        let volumes = raw
            .into_iter()
            .map(|v| Volume {
                name: v.name,
                guest: v.guest,
                base: v.base,
                path: v.path,
                // The CLI prints "-" for a size it could not determine; an
                // absent field says that more clearly than the placeholder.
                size: v.size.filter(|s| s != "-"),
                created_at: v.created_at,
                tracked: v.tracked,
            })
            .collect();
        Ok(Response::new(ListVolumesResponse { volumes }))
    }

    async fn remove_volumes(
        &self,
        req: Request<RemoveVolumesRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        if r.names.is_empty() {
            return Err(Status::invalid_argument("names must not be empty"));
        }
        let argv = Argv::new(&["volume", "rm"])
            .flag("-f", r.force)
            .args(r.names)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    // -- networks ------------------------------------------------------------

    async fn list_networks(
        &self,
        _: Request<ListNetworksRequest>,
    ) -> Result<Response<ListNetworksResponse>, Status> {
        let raw: Vec<JsonNetwork> = self
            .cli
            .json(&[
                "network".to_string(),
                "ls".to_string(),
                "--json".to_string(),
            ])
            .await?;
        let networks = raw
            .into_iter()
            .map(|n| Network {
                name: n.name,
                subnet: n.subnet,
                gateway: n.gateway,
                members: n.members,
                running: n.running,
                up: n.up,
                created_at: n.created_at,
            })
            .collect();
        Ok(Response::new(ListNetworksResponse { networks }))
    }

    async fn create_network(
        &self,
        req: Request<CreateNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let argv = Argv::new(&["network", "create"])
            .arg(req.into_inner().name)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn remove_networks(
        &self,
        req: Request<RemoveNetworksRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        if r.names.is_empty() {
            return Err(Status::invalid_argument("names must not be empty"));
        }
        let argv = Argv::new(&["network", "rm"])
            .flag("-f", r.force)
            .args(r.names)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn connect_network(
        &self,
        req: Request<ConnectNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        let argv = Argv::new(&["network", "connect"])
            .arg(r.machine)
            .arg(r.network)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn disconnect_network(
        &self,
        req: Request<DisconnectNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let argv = Argv::new(&["network", "disconnect"])
            .arg(req.into_inner().machine)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn sync_network(
        &self,
        req: Request<SyncNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let argv = Argv::new(&["network", "sync"])
            .arg(req.into_inner().network)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    // -- flavors -------------------------------------------------------------

    async fn list_flavors(
        &self,
        _: Request<ListFlavorsRequest>,
    ) -> Result<Response<ListFlavorsResponse>, Status> {
        let raw: Vec<JsonFlavor> = self
            .cli
            .json(&["flavors".to_string(), "--json".to_string()])
            .await?;
        let flavors = raw
            .into_iter()
            .map(|f| Flavor {
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
            })
            .collect();
        Ok(Response::new(ListFlavorsResponse { flavors }))
    }

    async fn add_flavor(
        &self,
        req: Request<AddFlavorRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        if r.name.trim().is_empty() || r.base.trim().is_empty() {
            return Err(Status::invalid_argument("name and base are required"));
        }
        let mut argv = Argv::new(&["flavor", "add"]);
        argv.arg("--base").arg(r.base);
        if !r.category.is_empty() {
            argv.arg("--category").arg(r.category);
        }
        if !r.description.is_empty() {
            argv.arg("--description").arg(r.description);
        }
        argv.each("--port", &r.ports)
            .each("--env", &r.env)
            .each("--nix", &r.nix)
            .each("--provision", &r.provision)
            .arg(r.name);
        Ok(Response::new(self.cli.output(&argv.take()).await?))
    }

    async fn remove_flavors(
        &self,
        req: Request<RemoveFlavorsRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        if r.names.is_empty() {
            return Err(Status::invalid_argument("names must not be empty"));
        }
        let argv = Argv::new(&["flavor", "rm"])
            .flag("-f", r.force)
            .args(r.names)
            .take();
        Ok(Response::new(self.cli.output(&argv).await?))
    }

    async fn build_flavor(
        &self,
        req: Request<BuildFlavorRequest>,
    ) -> Result<Response<Self::BuildFlavorStream>, Status> {
        let r = req.into_inner();
        let argv = Argv::new(&["flavor", "build"])
            .vm(&r.vm)
            .flag("--force", r.force)
            .arg(r.name)
            .take();
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.cli.stream(&argv),
        ))))
    }

    // -- guest interaction ---------------------------------------------------

    /// Interactive exec. See [`crate::pty`] for why a TTY session runs the CLI
    /// under a real pty instead of pipes.
    async fn exec(
        &self,
        req: Request<Streaming<ExecInput>>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let mut input = req.into_inner();
        let first = input
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("stream closed before `start`"))?;
        let start = match first.payload {
            Some(exec_input::Payload::Start(s)) => s,
            _ => {
                return Err(Status::invalid_argument(
                    "the first message on an Exec stream must be `start`",
                ))
            }
        };
        if start.id.trim().is_empty() {
            return Err(Status::invalid_argument("id must not be empty"));
        }

        // An empty command means "give me this machine's shell", which is only
        // meaningful on a terminal — so it implies a tty regardless of the flag.
        let (argv, tty) = if start.command.is_empty() {
            (Argv::new(&["shell"]).arg(start.id).take(), true)
        } else {
            let argv = Argv::new(&["exec"])
                .flag("-t", start.tty)
                .each("-e", &start.env)
                .arg(start.id)
                .args(start.command)
                .take();
            (argv, start.tty)
        };

        let stream = if tty {
            let (rows, cols) = start
                .size
                .map(|s| (s.rows as u16, s.cols as u16))
                .unwrap_or((DEFAULT_ROWS, DEFAULT_COLS));
            let (session, stream) =
                PtySession::spawn(self.cli.bin(), &argv, self.cli.env_path(), rows, cols)?;
            tokio::spawn(pump_exec_input(input, Session::Pty(session)));
            Box::pin(stream) as ChunkStream
        } else {
            let (session, rx) = self.cli.spawn_piped(&argv)?;
            tokio::spawn(pump_exec_input(input, Session::Piped(session)));
            Box::pin(ReceiverStream::new(rx)) as ChunkStream
        };
        Ok(Response::new(stream))
    }

    async fn logs(&self, req: Request<LogsRequest>) -> Result<Response<Self::LogsStream>, Status> {
        let r = req.into_inner();
        let argv = Argv::new(&["logs"])
            .flag("-f", r.follow)
            .flag("--boot", r.boot)
            .arg(r.id)
            .take();
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.cli.stream(&argv),
        ))))
    }

    async fn guest_tool_call(
        &self,
        req: Request<GuestToolRequest>,
    ) -> Result<Response<Self::GuestToolCallStream>, Status> {
        let r = req.into_inner();
        let tool = match GuestTool::try_from(r.tool) {
            Ok(GuestTool::Ssh) => "ssh",
            Ok(GuestTool::Tailscale) => "tailscale",
            Ok(GuestTool::Systemd) => "systemd",
            _ => return Err(Status::invalid_argument("unknown guest tool")),
        };
        if r.args.is_empty() {
            return Err(Status::invalid_argument(
                "args must name an action, e.g. [\"status\"]",
            ));
        }
        let argv = Argv::new(&[tool]).arg(r.id).args(r.args).take();
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.cli.stream(&argv),
        ))))
    }

    // -- everything else -----------------------------------------------------

    async fn run(
        &self,
        req: Request<Streaming<RunInput>>,
    ) -> Result<Response<Self::RunStream>, Status> {
        let mut input = req.into_inner();
        let first = input
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("stream closed before `start`"))?;
        let start = match first.payload {
            Some(run_input::Payload::Start(s)) => s,
            _ => {
                return Err(Status::invalid_argument(
                    "the first message on a Run stream must be `start`",
                ))
            }
        };
        if start.args.is_empty() {
            return Err(Status::invalid_argument("args must not be empty"));
        }

        let stream = if start.tty {
            let (rows, cols) = start
                .size
                .map(|s| (s.rows as u16, s.cols as u16))
                .unwrap_or((DEFAULT_ROWS, DEFAULT_COLS));
            let (session, stream) =
                PtySession::spawn(self.cli.bin(), &start.args, self.cli.env_path(), rows, cols)?;
            tokio::spawn(pump_run_input(input, Session::Pty(session)));
            Box::pin(stream) as ChunkStream
        } else {
            let (session, rx) = self.cli.spawn_piped(&start.args)?;
            tokio::spawn(pump_run_input(input, Session::Piped(session)));
            Box::pin(ReceiverStream::new(rx)) as ChunkStream
        };
        Ok(Response::new(stream))
    }
}

// ---------------------------------------------------------------------------
// input pumps
// ---------------------------------------------------------------------------

/// Either kind of interactive session, so the two bidi RPCs share one pump.
enum Session {
    Pty(PtySession),
    Piped(crate::cli::PipedSession),
}

impl Session {
    fn write(&self, data: Vec<u8>) {
        match self {
            Session::Pty(s) => s.write(data),
            Session::Piped(s) => s.write(data),
        }
    }

    fn eof(&self) {
        match self {
            Session::Pty(s) => s.eof(),
            Session::Piped(s) => s.eof(),
        }
    }

    /// Resizes only mean something on a terminal; a piped session ignores them.
    fn resize(&self, rows: u16, cols: u16) {
        if let Session::Pty(s) = self {
            s.resize(rows, cols);
        }
    }

    /// The client half-closed its request stream: it will send no more input.
    ///
    /// For a pipe that is a genuine EOF, and closing it is what lets a guest
    /// command reading stdin finish. For a terminal it is not: EOF there means
    /// injecting Ctrl-D, which would kill an interactive shell belonging to a
    /// client that merely had nothing to type yet. Clients that really mean
    /// end-of-input on a tty send `stdin_eof` explicitly.
    fn input_closed(&self) {
        if let Session::Piped(s) = self {
            s.eof();
        }
    }
}

/// Drain the client's half of the stream into the session.
///
/// Ending this task must not end the session: a non-interactive client sends
/// its `start` message and immediately half-closes, which would otherwise cut
/// the command off before it produced a single byte.
async fn pump_exec_input(mut input: Streaming<ExecInput>, session: Session) {
    while let Ok(Some(msg)) = input.message().await {
        match msg.payload {
            Some(exec_input::Payload::Stdin(b)) => session.write(b),
            Some(exec_input::Payload::Resize(r)) => session.resize(r.rows as u16, r.cols as u16),
            Some(exec_input::Payload::StdinEof(true)) => session.eof(),
            // A second `start` is a client bug; ignoring it is friendlier than
            // tearing down a working session.
            _ => {}
        }
    }
    session.input_closed();
}

async fn pump_run_input(mut input: Streaming<RunInput>, session: Session) {
    while let Ok(Some(msg)) = input.message().await {
        match msg.payload {
            Some(run_input::Payload::Stdin(b)) => session.write(b),
            Some(run_input::Payload::Resize(r)) => session.resize(r.rows as u16, r.cols as u16),
            Some(run_input::Payload::StdinEof(true)) => session.eof(),
            _ => {}
        }
    }
    session.input_closed();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_puts_flags_before_positionals() {
        let argv = Argv::new(&["linux", "-d"])
            .vm(&Some(VmConfig { cpus: 2, mem: 1024 }))
            .each("-e", &["A=1".into()])
            .arg("alpine")
            .take();
        assert_eq!(
            argv,
            vec!["linux", "-d", "--cpus", "2", "--mem", "1024", "-e", "A=1", "alpine"]
        );
    }

    #[test]
    fn vm_zero_means_let_the_cli_decide() {
        let argv = Argv::new(&["linux"])
            .vm(&Some(VmConfig { cpus: 0, mem: 0 }))
            .take();
        assert_eq!(argv, vec!["linux"]);
    }

    #[test]
    fn net_config_expands_every_field() {
        let argv = Argv::new(&["freebsd"])
            .net(&Some(NetConfig {
                no_net: false,
                ports: vec!["2222:22".into(), "8080:80".into()],
                mac: None,
                network: Some("dev".into()),
                name: Some("web".into()),
            }))
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
    fn versions_parses_numeric_and_current_rows() {
        let out = "Available FreeBSD builds:\n  15.1  (latest)\n  14.3\n";
        let v = parse_versions(out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].version, "15.1");
        assert!(v[0].latest);
        assert_eq!(v[1].version, "14.3");
        assert!(!v[1].latest);

        let netbsd = "Available NetBSD builds:\n  current\n  10.1\n";
        let v = parse_versions(netbsd);
        assert_eq!(v[0].version, "current");
        assert!(v[0].latest);
    }
}
