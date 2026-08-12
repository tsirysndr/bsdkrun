//! The gRPC `Bsdkrun` service: proto messages mapped onto [`crate::ops`].
//!
//! This layer is deliberately thin. Everything that decides *what command to
//! run* lives in `ops`, shared with the GraphQL front end; what remains here is
//! translation between protobuf and domain types, plus the streaming plumbing
//! specific to gRPC.
//!
//! Machines launched through the daemon are always detached. The daemon
//! outlives any single RPC, so a foreground VM would have nowhere to live; the
//! boot RPCs therefore return a machine id and clients use `Logs` to watch it
//! boot and `Exec` to get a shell.

use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::ops::{self, Ops};
use crate::pb::bsdkrun_server::Bsdkrun;
use crate::pb::*;
use crate::pty::{PtySession, DEFAULT_COLS, DEFAULT_ROWS};

pub struct BsdkrunService {
    ops: Ops,
}

impl BsdkrunService {
    pub fn new(supervisor: crate::supervisor::Supervisor) -> Self {
        Self {
            ops: Ops::new(supervisor),
        }
    }

    /// Share one [`Ops`] with the GraphQL front end when both are served.
    pub fn from_ops(ops: Ops) -> Self {
        Self { ops }
    }
}

/// Boxed because an interactive RPC returns one of two concrete streams
/// depending on whether a terminal was requested.
type ChunkStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<OutputChunk, Status>> + Send>>;

// ---------------------------------------------------------------------------
// proto -> domain
// ---------------------------------------------------------------------------

fn bsd_os(os: i32) -> Result<ops::BsdOs, Status> {
    match BsdOs::try_from(os) {
        Ok(BsdOs::Freebsd) => Ok(ops::BsdOs::Freebsd),
        Ok(BsdOs::Netbsd) => Ok(ops::BsdOs::Netbsd),
        _ => Err(Status::invalid_argument(
            "os must be BSD_OS_FREEBSD or BSD_OS_NETBSD",
        )),
    }
}

fn net_opts(net: Option<NetConfig>) -> ops::NetOpts {
    match net {
        Some(n) => ops::NetOpts {
            no_net: n.no_net,
            ports: n.ports,
            mac: n.mac,
            network: n.network,
            name: n.name,
        },
        None => ops::NetOpts::default(),
    }
}

/// `VmConfig` uses 0 to mean "unset", so the CLI applies its own default.
fn vm_opts(vm: Option<VmConfig>) -> (Option<u32>, Option<u32>) {
    match vm {
        Some(v) => (Some(v.cpus), Some(v.mem)),
        None => (None, None),
    }
}

// ---------------------------------------------------------------------------
// domain -> proto
// ---------------------------------------------------------------------------

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
            ports: m
                .ports
                .into_iter()
                .map(|p| PortForward {
                    bind: p.bind.to_string(),
                    host: p.host as u32,
                    guest: p.guest as u32,
                })
                .collect(),
        }
    }
}

impl From<ops::Image> for Image {
    fn from(i: ops::Image) -> Self {
        Image {
            id: i.id,
            reference: i.reference,
            digest: i.digest,
            size: i.size,
            rootfs: i.rootfs,
            created_at: i.created_at,
        }
    }
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

impl From<ops::Network> for Network {
    fn from(n: ops::Network) -> Self {
        Network {
            name: n.name,
            subnet: n.subnet,
            gateway: n.gateway,
            members: n.members,
            running: n.running,
            up: n.up,
            created_at: n.created_at,
        }
    }
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

impl From<ops::Version> for Version {
    fn from(v: ops::Version) -> Self {
        Version {
            version: v.version,
            latest: v.latest,
        }
    }
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
        let i = self.ops.info().await?;
        Ok(Response::new(InfoResponse {
            daemon_version: i.daemon_version,
            // The proto still calls these the "cli" version and path. There is
            // no separate CLI any more: the daemon links the engine and reports
            // its own binary, so existing clients keep reading one field pair
            // that now cannot disagree with what actually runs.
            cli_version: i.engine_version,
            cli_path: i.exe_path,
            os: i.os,
            arch: i.arch,
        }))
    }

    // -- machines ------------------------------------------------------------

    async fn list_machines(
        &self,
        req: Request<ListMachinesRequest>,
    ) -> Result<Response<ListMachinesResponse>, Status> {
        let machines = self.ops.list_machines(req.into_inner().all).await?;
        Ok(Response::new(ListMachinesResponse {
            machines: machines.into_iter().map(Into::into).collect(),
        }))
    }

    async fn start(&self, req: Request<MachineRequest>) -> Result<Response<CommandResult>, Status> {
        Ok(Response::new(self.ops.start(&req.into_inner().id).await?))
    }

    async fn stop(&self, req: Request<MachineRequest>) -> Result<Response<CommandResult>, Status> {
        Ok(Response::new(self.ops.stop(&req.into_inner().id).await?))
    }

    async fn remove_machines(
        &self,
        req: Request<RemoveMachinesRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.remove_machines(&r.ids, r.force).await?,
        ))
    }

    async fn update_machine(
        &self,
        req: Request<UpdateMachineRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.update_machine(&r.id, r.cpus, r.mem).await?,
        ))
    }

    async fn commit(&self, req: Request<CommitRequest>) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.commit(&r.id, &r.name, &r.description).await?,
        ))
    }

    async fn update_agent(
        &self,
        req: Request<MachineRequest>,
    ) -> Result<Response<Self::UpdateAgentStream>, Status> {
        let cmd = self.ops.update_agent_command(&req.into_inner().id);
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.ops.stream(&cmd)?,
        ))))
    }

    // -- booting -------------------------------------------------------------

    async fn run_linux(
        &self,
        req: Request<RunLinuxRequest>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunLinuxOpts {
            image: r.image,
            cpus,
            mem,
            net: net_opts(r.net),
            volume: r.volume,
            mounts: r.mounts,
            env: r.env,
            entrypoint: r.entrypoint,
            initramfs: r.initramfs,
            kernel: r.kernel,
            kernel_version: r.kernel_version,
            console: r.console,
            repo: r.repo,
            command: r.command,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_linux(&opts).await?,
        }))
    }

    async fn run_bsd(&self, req: Request<RunBsdRequest>) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunBsdOpts {
            os: bsd_os(r.os)?,
            version: r.version,
            cpus,
            mem,
            net: net_opts(r.net),
            volume: r.volume,
            persist: r.persist,
            force: r.force,
            firmware: r.firmware,
            attach_disk: r.attach_disk,
            disk_size: r.disk_size,
            repo: r.repo,
            command: r.command,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_bsd(&opts).await?,
        }))
    }

    async fn run_nanos(
        &self,
        req: Request<RunNanosRequest>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunNanosOpts {
            image: r.image,
            cpus,
            mem,
            net: net_opts(r.net),
            kernel: r.kernel,
            cmdline: r.cmdline,
            persist: r.persist,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_nanos(&opts).await?,
        }))
    }

    async fn run_unikraft(
        &self,
        req: Request<RunUnikraftRequest>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunUnikraftOpts {
            path: r.path,
            cpus,
            mem,
            net: net_opts(r.net),
            cmdline: r.cmdline,
            initramfs: r.initramfs,
            mounts: r.mounts,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_unikraft(&opts).await?,
        }))
    }

    async fn run_solo5(
        &self,
        req: Request<RunSolo5Request>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunSolo5Opts {
            path: r.path,
            cpus,
            mem,
            net: net_opts(r.net),
            block: r.block,
            args: r.args,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_solo5(&opts).await?,
        }))
    }

    async fn run_osv(&self, req: Request<RunOsvRequest>) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunOsvOpts {
            image: r.image,
            cpus,
            mem,
            net: net_opts(r.net),
            cmdline: r.cmdline,
            disk: r.disk,
            no_disk: r.no_disk,
            attach_disk: r.attach_disk,
            // UNSPECIFIED means "let the CLI decide", so it maps to no flag
            // rather than to a guessed version.
            gic: match OsvGic::try_from(r.gic).unwrap_or(OsvGic::Unspecified) {
                OsvGic::Unspecified => None,
                OsvGic::V2 => Some("v2".to_string()),
                OsvGic::V3 => Some("v3".to_string()),
            },
            persist: r.persist,
            volume: r.volume,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_osv(&opts).await?,
        }))
    }

    async fn run_flavor(
        &self,
        req: Request<RunFlavorRequest>,
    ) -> Result<Response<RunResponse>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let opts = ops::RunFlavorOpts {
            name: r.name,
            cpus,
            mem,
            ports: r.ports,
            volume: r.volume,
            repo: r.repo,
        };
        Ok(Response::new(RunResponse {
            id: self.ops.run_flavor(&opts).await?,
        }))
    }

    // -- images --------------------------------------------------------------

    async fn list_images(
        &self,
        _: Request<ListImagesRequest>,
    ) -> Result<Response<ListImagesResponse>, Status> {
        let images = self.ops.list_images().await?;
        Ok(Response::new(ListImagesResponse {
            images: images.into_iter().map(Into::into).collect(),
        }))
    }

    async fn fetch(
        &self,
        req: Request<FetchRequest>,
    ) -> Result<Response<Self::FetchStream>, Status> {
        let r = req.into_inner();
        let cmd = self
            .ops
            .fetch_command(bsd_os(r.os)?, &r.version, &r.dir, r.force);
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.ops.stream(&cmd)?,
        ))))
    }

    async fn list_versions(
        &self,
        req: Request<ListVersionsRequest>,
    ) -> Result<Response<ListVersionsResponse>, Status> {
        let versions = self.ops.list_versions(bsd_os(req.into_inner().os)?).await?;
        Ok(Response::new(ListVersionsResponse {
            versions: versions.into_iter().map(Into::into).collect(),
        }))
    }

    // -- volumes -------------------------------------------------------------

    async fn list_volumes(
        &self,
        _: Request<ListVolumesRequest>,
    ) -> Result<Response<ListVolumesResponse>, Status> {
        let volumes = self.ops.list_volumes().await?;
        Ok(Response::new(ListVolumesResponse {
            volumes: volumes.into_iter().map(Into::into).collect(),
        }))
    }

    async fn remove_volumes(
        &self,
        req: Request<RemoveVolumesRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.remove_volumes(&r.names, r.force).await?,
        ))
    }

    // -- networks ------------------------------------------------------------

    async fn list_networks(
        &self,
        _: Request<ListNetworksRequest>,
    ) -> Result<Response<ListNetworksResponse>, Status> {
        let networks = self.ops.list_networks().await?;
        Ok(Response::new(ListNetworksResponse {
            networks: networks.into_iter().map(Into::into).collect(),
        }))
    }

    async fn create_network(
        &self,
        req: Request<CreateNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        Ok(Response::new(
            self.ops.create_network(&req.into_inner().name).await?,
        ))
    }

    async fn remove_networks(
        &self,
        req: Request<RemoveNetworksRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.remove_networks(&r.names, r.force).await?,
        ))
    }

    async fn connect_network(
        &self,
        req: Request<ConnectNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.connect_network(&r.machine, &r.network).await?,
        ))
    }

    async fn disconnect_network(
        &self,
        req: Request<DisconnectNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        Ok(Response::new(
            self.ops
                .disconnect_network(&req.into_inner().machine)
                .await?,
        ))
    }

    async fn sync_network(
        &self,
        req: Request<SyncNetworkRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        Ok(Response::new(
            self.ops.sync_network(&req.into_inner().network).await?,
        ))
    }

    // -- flavors -------------------------------------------------------------

    async fn list_flavors(
        &self,
        _: Request<ListFlavorsRequest>,
    ) -> Result<Response<ListFlavorsResponse>, Status> {
        let flavors = self.ops.list_flavors().await?;
        Ok(Response::new(ListFlavorsResponse {
            flavors: flavors.into_iter().map(Into::into).collect(),
        }))
    }

    async fn add_flavor(
        &self,
        req: Request<AddFlavorRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        let opts = ops::AddFlavorOpts {
            name: r.name,
            base: r.base,
            category: r.category,
            description: r.description,
            ports: r.ports,
            env: r.env,
            nix: r.nix,
            provision: r.provision,
        };
        Ok(Response::new(self.ops.add_flavor(&opts).await?))
    }

    async fn remove_flavors(
        &self,
        req: Request<RemoveFlavorsRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let r = req.into_inner();
        Ok(Response::new(
            self.ops.remove_flavors(&r.names, r.force).await?,
        ))
    }

    async fn build_flavor(
        &self,
        req: Request<BuildFlavorRequest>,
    ) -> Result<Response<Self::BuildFlavorStream>, Status> {
        let r = req.into_inner();
        let (cpus, mem) = vm_opts(r.vm);
        let cmd = self.ops.build_flavor_command(&r.name, cpus, mem, r.force);
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.ops.stream(&cmd)?,
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

        let (rows, cols) = start
            .size
            .map(|s| (s.rows as u16, s.cols as u16))
            .unwrap_or((DEFAULT_ROWS, DEFAULT_COLS));
        // A terminal on a BSD guest needs TERM supplied; see Ops::interactive_env.
        let env = if start.tty || start.command.is_empty() {
            self.ops.interactive_env(&start.id, start.env).await
        } else {
            start.env
        };
        let (cmd, tty) = ops::ExecOpts {
            id: start.id,
            command: start.command,
            env,
            tty: start.tty,
        }
        .to_command();
        let sup = self.ops.supervisor();
        let argv = sup.argv(&cmd)?;

        let stream = if tty {
            let (session, stream) =
                PtySession::spawn(sup.require()?, &argv, sup.env_path(), rows, cols)?;
            tokio::spawn(pump_exec_input(input, Session::Pty(session)));
            Box::pin(stream) as ChunkStream
        } else {
            let (session, rx) = sup.spawn_piped(&argv)?;
            tokio::spawn(pump_exec_input(input, Session::Piped(session)));
            Box::pin(ReceiverStream::new(rx)) as ChunkStream
        };
        Ok(Response::new(stream))
    }

    async fn logs(&self, req: Request<LogsRequest>) -> Result<Response<Self::LogsStream>, Status> {
        let r = req.into_inner();
        let cmd = self.ops.logs_command(&r.id, r.follow, r.boot);
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.ops.stream(&cmd)?,
        ))))
    }

    async fn guest_tool_call(
        &self,
        req: Request<GuestToolRequest>,
    ) -> Result<Response<Self::GuestToolCallStream>, Status> {
        let r = req.into_inner();
        let tool = match GuestTool::try_from(r.tool) {
            Ok(GuestTool::Ssh) => ops::GuestTool::Ssh,
            Ok(GuestTool::Tailscale) => ops::GuestTool::Tailscale,
            Ok(GuestTool::Systemd) => ops::GuestTool::Systemd,
            _ => return Err(Status::invalid_argument("unknown guest tool")),
        };
        let cmd = self.ops.guest_tool_command(tool, &r.id, &r.args)?;
        Ok(Response::new(Box::pin(ReceiverStream::new(
            self.ops.stream(&cmd)?,
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
            let sup = self.ops.supervisor();
            let argv = sup.argv_raw(&start.args);
            let (session, stream) =
                PtySession::spawn(sup.require()?, &argv, sup.env_path(), rows, cols)?;
            tokio::spawn(pump_run_input(input, Session::Pty(session)));
            Box::pin(stream) as ChunkStream
        } else {
            let sup = self.ops.supervisor();
            let (session, rx) = sup.spawn_piped(&sup.argv_raw(&start.args))?;
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
    Piped(crate::supervisor::PipedSession),
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
