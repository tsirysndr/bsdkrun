//! A client for a remote `bsdkrund` daemon's GraphQL API.
//!
//! [`crate::Sandbox`] talks to a *local* `bsdkrun` binary by shelling out to
//! it. [`Client`] is the network sibling: it drives the exact same operations
//! against a daemon over HTTP (queries/mutations) and one shared
//! `graphql-transport-ws` socket (subscriptions — exec output, live shells,
//! log follow), so a program can target either a machine with the CLI
//! installed or a remote host running `bsdkrund` with the same calls.
//!
//! The GraphQL documents below are deliberately minimal string literals
//! rather than a generated client: this SDK has no code-generation step, and
//! the schema is small and stable enough (`daemon/src/graphql.rs`) that
//! hand-typed queries stay easy to keep in sync.

use std::sync::{mpsc, Arc, Mutex};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::args::{strvec, NetOpts};
use crate::error::{Error, Result};
use crate::transport::{http_request, normalize_url, ws_url, WsTransport, TOKEN_ENV, URL_ENV};
use crate::types::{
    CommandResult, DockerContainer, DockerStatus, RemoteExecResult, SandboxInfo, ShellSessionInfo,
    SnapshotInfo,
};

// ---------------------------------------------------------------------------
// GraphQL documents
// ---------------------------------------------------------------------------

const MACHINE_FIELDS: &str = "id name image kind command status running exitCode pid detached \
     cpus mem volume stateDir createdAt finishedAt network netIp origin \
     ports { bind host guest }";
const SNAPSHOT_FIELDS: &str = "id name machineId machineName kind image path parent description \
     cpus mem size createdAt ports { bind host guest }";
const CMD_RESULT_FIELDS: &str = "exitCode stdout stderr";
const SESSION_FIELDS: &str = "id machineId finished truncated";

fn list_query() -> String {
    format!("query($all: Boolean!) {{ machines(all: $all) {{ {MACHINE_FIELDS} }} }}")
}
fn get_query() -> String {
    format!("query($id: String!) {{ machine(id: $id) {{ {MACHINE_FIELDS} }} }}")
}
const LOGS_QUERY: &str =
    "query($id: String!, $boot: Boolean!) { machineLogs(id: $id, boot: $boot) }";

fn stop_mutation() -> String {
    format!("mutation($id: String!) {{ stopMachine(id: $id) {{ {CMD_RESULT_FIELDS} }} }}")
}
fn start_mutation() -> String {
    format!("mutation($id: String!) {{ startMachine(id: $id) {{ {CMD_RESULT_FIELDS} }} }}")
}
fn remove_mutation() -> String {
    format!(
        "mutation($ids: [String!]!, $force: Boolean!) {{ \
         removeMachines(ids: $ids, force: $force) {{ {CMD_RESULT_FIELDS} }} }}"
    )
}
fn update_mutation() -> String {
    format!(
        "mutation($id: String!, $cpus: Int, $mem: Int) {{ \
         updateMachine(id: $id, cpus: $cpus, mem: $mem) {{ {CMD_RESULT_FIELDS} }} }}"
    )
}
fn commit_mutation() -> String {
    format!(
        "mutation($id: String!, $name: String!, $description: String!) {{ \
         commitMachine(id: $id, name: $name, description: $description) {{ {CMD_RESULT_FIELDS} }} }}"
    )
}

const DOCKER_STATUS_FIELDS: &str =
    "running machineId machineRunning socket socketReady apiPort version \
     containers images mounts disk diskSize";
const DOCKER_CONTAINER_FIELDS: &str = "id name image command state status ports created";

fn docker_status_query() -> String {
    format!("{{ dockerStatus {{ {DOCKER_STATUS_FIELDS} }} }}")
}
fn docker_containers_query() -> String {
    format!(
        "query($all: Boolean!) {{ dockerContainers(all: $all) \
         {{ {DOCKER_CONTAINER_FIELDS} }} }}"
    )
}
const DOCKER_LOGS_QUERY: &str =
    "query($id: String!, $tail: Int!) { dockerContainerLogs(id: $id, tail: $tail) }";
fn docker_start_mutation() -> String {
    format!(
        "mutation($input: DockerStartInput!) {{ dockerStart(input: $input) \
         {{ {DOCKER_STATUS_FIELDS} }} }}"
    )
}
fn docker_stop_mutation() -> String {
    format!("mutation {{ dockerStop {{ {CMD_RESULT_FIELDS} }} }}")
}
fn docker_container_mutation() -> String {
    format!(
        "mutation($action: String!, $ids: [String!]!) {{ \
         dockerContainer(action: $action, ids: $ids) {{ {CMD_RESULT_FIELDS} }} }}"
    )
}

fn snapshots_query() -> String {
    format!("query($machine: String) {{ snapshots(machine: $machine) {{ {SNAPSHOT_FIELDS} }} }}")
}
fn snapshot_mutation() -> String {
    format!(
        "mutation($id: String!, $name: String, $description: String!) {{ \
         snapshotMachine(id: $id, name: $name, description: $description) \
         {{ {SNAPSHOT_FIELDS} }} }}"
    )
}
fn remove_snapshots_mutation() -> String {
    format!(
        "mutation($names: [String!]!) {{ \
         removeSnapshots(names: $names) {{ {CMD_RESULT_FIELDS} }} }}"
    )
}
fn restore_mutation() -> String {
    format!(
        "mutation($id: String!, $snapshot: String!, $force: Boolean!, $backup: Boolean!) {{ \
         restoreMachine(id: $id, snapshot: $snapshot, force: $force, backup: $backup) \
         {{ {CMD_RESULT_FIELDS} }} }}"
    )
}
fn rollback_mutation() -> String {
    format!(
        "mutation($id: String!, $force: Boolean!, $backup: Boolean!) {{ \
         rollbackMachine(id: $id, force: $force, backup: $backup) {{ {CMD_RESULT_FIELDS} }} }}"
    )
}
const BRANCH_MUTATION: &str = "mutation($input: BranchInput!) { branchSnapshot(input: $input) }";

const RUN_LINUX_MUTATION: &str = "mutation($input: RunLinuxInput!) { runLinux(input: $input) }";
const RUN_BSD_MUTATION: &str = "mutation($input: RunBsdInput!) { runBsd(input: $input) }";
const RUN_NANOS_MUTATION: &str = "mutation($input: RunNanosInput!) { runNanos(input: $input) }";
const RUN_UNIKRAFT_MUTATION: &str =
    "mutation($input: RunUnikraftInput!) { runUnikraft(input: $input) }";
const RUN_SOLO5_MUTATION: &str = "mutation($input: RunSolo5Input!) { runSolo5(input: $input) }";
const RUN_OSV_MUTATION: &str = "mutation($input: RunOsvInput!) { runOsv(input: $input) }";
const RUN_FLAVOR_MUTATION: &str = "mutation($input: RunFlavorInput!) { runFlavor(input: $input) }";

const MACHINE_LOGS_SUBSCRIPTION: &str =
    "subscription($id: String!, $follow: Boolean!, $boot: Boolean!) { \
     machineLogs(id: $id, follow: $follow, boot: $boot) { dataBase64 exitCode } }";

fn open_shell_mutation() -> String {
    format!(
        "mutation($machineId: String!, $command: [String!]!, $env: [String!]!, \
         $rows: Int!, $cols: Int!) {{ \
         openShell(machineId: $machineId, command: $command, env: $env, \
         rows: $rows, cols: $cols) {{ {SESSION_FIELDS} }} }}"
    )
}
const SHELL_OUTPUT_SUBSCRIPTION: &str = "subscription($sessionId: String!) { \
     shellOutput(sessionId: $sessionId) { dataBase64 exitCode } }";
const SEND_INPUT_MUTATION: &str = "mutation($sessionId: String!, $dataBase64: String!) { \
     sendShellInput(sessionId: $sessionId, dataBase64: $dataBase64) }";
const RESIZE_MUTATION: &str = "mutation($sessionId: String!, $rows: Int!, $cols: Int!) { \
     resizeShell(sessionId: $sessionId, rows: $rows, cols: $cols) }";
const CLOSE_MUTATION: &str = "mutation($sessionId: String!) { closeShell(sessionId: $sessionId) }";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

struct ClientInner {
    url: String,
    token: String,
    /// The one lazily opened WS transport every subscription shares. It drops
    /// its socket when the last subscription ends and reconnects on the next,
    /// so the `Arc` never needs replacing.
    ws: Mutex<Option<Arc<WsTransport>>>,
}

/// A client for a remote `bsdkrund`'s GraphQL API.
///
/// Queries and mutations go over HTTP; subscriptions (used internally by
/// [`Client::exec`], [`Client::shell`] and [`Client::follow_logs`]) share one
/// lazily opened `graphql-transport-ws` socket per client, torn down once the
/// last subscription ends. Cloning is cheap and shares that socket.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("url", &self.inner.url)
            .finish()
    }
}

impl Client {
    /// Build a client from a daemon URL and its bearer token.
    ///
    /// A URL configured without a token is refused rather than silently
    /// making an unauthenticated request — the daemon has no anonymous tier.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Result<Client> {
        let url = normalize_url(&url.into());
        if url.is_empty() {
            return Err(Error::InvalidInput("the daemon URL is empty".into()));
        }
        let token = token.into().trim().to_string();
        if token.is_empty() {
            return Err(Error::InvalidInput(
                "a daemon URL without a token is a configuration error; pass the bearer token"
                    .into(),
            ));
        }
        Ok(Client {
            inner: Arc::new(ClientInner {
                url,
                token,
                ws: Mutex::new(None),
            }),
        })
    }

    /// Build a client from `BSDKRUN_URL` / `BSDKRUN_TOKEN`.
    ///
    /// Errors if `BSDKRUN_URL` is unset (nothing to connect to), or if it is
    /// set but `BSDKRUN_TOKEN` is not — a host configured without a token is
    /// a configuration error, never a silent fall-back to an unauthenticated
    /// request.
    pub fn from_env() -> Result<Client> {
        let url = std::env::var(URL_ENV)
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() {
            return Err(Error::InvalidInput(format!(
                "{URL_ENV} is not set; nothing to connect to"
            )));
        }
        let token = std::env::var(TOKEN_ENV)
            .unwrap_or_default()
            .trim()
            .to_string();
        if token.is_empty() {
            return Err(Error::InvalidInput(format!(
                "{URL_ENV} is set but {TOKEN_ENV} is not"
            )));
        }
        Client::new(url, token)
    }

    /// The normalized GraphQL endpoint URL.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    // -- transport (escape hatch) ------------------------------------------

    /// Run a raw query or mutation and return its `data`.
    pub fn request(&self, query: &str, variables: Value) -> Result<Value> {
        http_request(&self.inner.url, &self.inner.token, query, &variables)
    }

    /// Start a raw subscription; each `next` payload's `data` goes to
    /// `on_next`. Returns a [`Subscription`] handle to end it with.
    pub fn subscribe(
        &self,
        query: &str,
        variables: Value,
        on_next: impl FnMut(Value) + Send + 'static,
    ) -> Result<Subscription> {
        self.subscribe_with(query, variables, on_next, |_| {}, || {})
    }

    /// [`Client::subscribe`] with error/completion callbacks.
    pub fn subscribe_with(
        &self,
        query: &str,
        variables: Value,
        on_next: impl FnMut(Value) + Send + 'static,
        on_error: impl FnMut(Error) + Send + 'static,
        on_complete: impl FnMut() + Send + 'static,
    ) -> Result<Subscription> {
        let transport = self.ws();
        let id = transport.subscribe(
            query,
            variables,
            Box::new(on_next),
            Box::new(on_error),
            Box::new(on_complete),
        )?;
        Ok(Subscription { transport, id })
    }

    fn ws(&self) -> Arc<WsTransport> {
        let mut guard = self.inner.ws.lock().unwrap();
        guard
            .get_or_insert_with(|| {
                Arc::new(WsTransport::new(
                    ws_url(&self.inner.url),
                    self.inner.token.clone(),
                ))
            })
            .clone()
    }

    // -- lifecycle / listing -----------------------------------------------

    /// List machines. `all` includes exited ones.
    pub fn list(&self, all: bool) -> Result<Vec<SandboxInfo>> {
        let data = self.request(&list_query(), json!({"all": all}))?;
        Ok(data
            .get("machines")
            .and_then(Value::as_array)
            .map(|machines| machines.iter().map(SandboxInfo::from_graphql).collect())
            .unwrap_or_default())
    }

    /// Fetch one machine by id (a unique prefix) or name, or `None`.
    pub fn get(&self, id: &str) -> Result<Option<SandboxInfo>> {
        let data = self.request(&get_query(), json!({"id": id}))?;
        Ok(data
            .get("machine")
            .filter(|m| !m.is_null())
            .map(SandboxInfo::from_graphql))
    }

    pub fn stop(&self, id: &str) -> Result<CommandResult> {
        let data = self.request(&stop_mutation(), json!({"id": id}))?;
        Ok(CommandResult::from_graphql(&data["stopMachine"]))
    }

    pub fn start(&self, id: &str) -> Result<CommandResult> {
        let data = self.request(&start_mutation(), json!({"id": id}))?;
        Ok(CommandResult::from_graphql(&data["startMachine"]))
    }

    pub fn remove<S: AsRef<str>>(&self, ids: &[S], force: bool) -> Result<CommandResult> {
        let ids: Vec<&str> = ids.iter().map(AsRef::as_ref).collect();
        let data = self.request(&remove_mutation(), json!({"ids": ids, "force": force}))?;
        Ok(CommandResult::from_graphql(&data["removeMachines"]))
    }

    /// Change a machine's recorded vCPU / memory; applies on its next start.
    pub fn update(&self, id: &str, cpus: Option<u32>, mem: Option<u32>) -> Result<CommandResult> {
        let data = self.request(
            &update_mutation(),
            json!({"id": id, "cpus": cpus, "mem": mem}),
        )?;
        Ok(CommandResult::from_graphql(&data["updateMachine"]))
    }

    /// Snapshot a machine into a named flavor, like `docker commit`.
    pub fn commit(&self, id: &str, name: &str, description: &str) -> Result<CommandResult> {
        let data = self.request(
            &commit_mutation(),
            json!({"id": id, "name": name, "description": description}),
        )?;
        Ok(CommandResult::from_graphql(&data["commitMachine"]))
    }

    // -- docker -------------------------------------------------------------
    //
    // bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
    // socket, so these drive the same engine the host's `docker` CLI does.

    /// Is the Docker engine up, and where is its socket?
    pub fn docker_status(&self) -> Result<DockerStatus> {
        let data = self.request(&docker_status_query(), json!({}))?;
        Ok(DockerStatus::from_graphql(&data["dockerStatus"]))
    }

    /// Containers in the engine. `all = false` lists only running ones.
    pub fn docker_containers(&self, all: bool) -> Result<Vec<DockerContainer>> {
        let data = self.request(&docker_containers_query(), json!({ "all": all }))?;
        Ok(data
            .get("dockerContainers")
            .and_then(Value::as_array)
            .map(|rows| rows.iter().map(DockerContainer::from_graphql).collect())
            .unwrap_or_default())
    }

    /// Start (or resume) the engine — see [`DockerStartBuilder`].
    ///
    /// Idempotent: the VM has a fixed name, so this resumes the existing one
    /// rather than creating a second.
    pub fn docker_start(&self) -> DockerStartBuilder {
        DockerStartBuilder {
            client: self.clone(),
            cpus: None,
            mem: None,
            mounts: Vec::new(),
            no_home: false,
            publish_bind: None,
            disk_size: None,
        }
    }

    /// Stop the engine. Images and containers stay on its disk.
    pub fn docker_stop(&self) -> Result<CommandResult> {
        let data = self.request(&docker_stop_mutation(), json!({}))?;
        Ok(CommandResult::from_graphql(&data["dockerStop"]))
    }

    /// Act on containers: start / stop / restart / kill / pause / unpause / rm.
    pub fn docker_container<S: AsRef<str>>(
        &self,
        action: &str,
        ids: &[S],
    ) -> Result<CommandResult> {
        let ids: Vec<&str> = ids.iter().map(AsRef::as_ref).collect();
        let data = self.request(
            &docker_container_mutation(),
            json!({"action": action, "ids": ids}),
        )?;
        Ok(CommandResult::from_graphql(&data["dockerContainer"]))
    }

    /// One container's logs (stdout+stderr, most recent `tail` lines).
    pub fn docker_logs(&self, id: &str, tail: u32) -> Result<String> {
        let data = self.request(DOCKER_LOGS_QUERY, json!({"id": id, "tail": tail}))?;
        Ok(data
            .get("dockerContainerLogs")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    // -- snapshots ---------------------------------------------------------
    //
    // A snapshot is a copy-on-write clone of a machine's disk state: instant
    // to take, free until the two sides diverge. `branch` boots a new machine
    // from one; `restore`/`rollback` put one back.

    /// Snapshots, newest first. `machine` narrows to one machine's.
    pub fn snapshots(&self, machine: Option<&str>) -> Result<Vec<SnapshotInfo>> {
        let data = self.request(&snapshots_query(), json!({ "machine": machine }))?;
        Ok(data
            .get("snapshots")
            .and_then(Value::as_array)
            .map(|rows| rows.iter().map(SnapshotInfo::from_graphql).collect())
            .unwrap_or_default())
    }

    /// Capture a machine's disk state. `name` defaults to `<machine>-<n>`.
    ///
    /// A BSD guest is powered off first — a mounted UFS cannot be cloned
    /// consistently — so the machine is left stopped; [`Client::start`] brings
    /// it back.
    pub fn snapshot(
        &self,
        id: &str,
        name: Option<&str>,
        description: &str,
    ) -> Result<SnapshotInfo> {
        let data = self.request(
            &snapshot_mutation(),
            json!({"id": id, "name": name, "description": description}),
        )?;
        Ok(SnapshotInfo::from_graphql(&data["snapshotMachine"]))
    }

    /// Delete snapshots and their data. Machines branched from them stay.
    pub fn remove_snapshots<S: AsRef<str>>(&self, names: &[S]) -> Result<CommandResult> {
        let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
        let data = self.request(&remove_snapshots_mutation(), json!({ "names": names }))?;
        Ok(CommandResult::from_graphql(&data["removeSnapshots"]))
    }

    /// Put a machine's disk state back to one of its snapshots.
    ///
    /// `force` stops the machine first (it holds the very files being
    /// replaced); `backup` snapshots the state being overwritten, which is a
    /// CoW clone and therefore free. The machine is left stopped.
    pub fn restore(
        &self,
        id: &str,
        snapshot: &str,
        force: bool,
        backup: bool,
    ) -> Result<CommandResult> {
        let data = self.request(
            &restore_mutation(),
            json!({"id": id, "snapshot": snapshot, "force": force, "backup": backup}),
        )?;
        Ok(CommandResult::from_graphql(&data["restoreMachine"]))
    }

    /// Restore a machine to its most recent snapshot.
    pub fn rollback(&self, id: &str, force: bool, backup: bool) -> Result<CommandResult> {
        let data = self.request(
            &rollback_mutation(),
            json!({"id": id, "force": force, "backup": backup}),
        )?;
        Ok(CommandResult::from_graphql(&data["rollbackMachine"]))
    }

    /// Boot a NEW machine from a snapshot — see [`BranchBuilder`].
    ///
    /// The snapshot is cloned, never booted in place, so the machine it came
    /// from is untouched and one snapshot can be branched any number of times.
    pub fn branch(&self, snapshot: impl Into<String>) -> BranchBuilder {
        BranchBuilder {
            client: self.clone(),
            snapshot: snapshot.into(),
            name: None,
            cpus: None,
            mem: None,
            ports: Vec::new(),
            no_ports: false,
        }
    }

    /// One-shot read of a machine's console log (bsdkrun's boot log with
    /// `boot`).
    pub fn logs(&self, id: &str, boot: bool) -> Result<String> {
        let data = self.request(LOGS_QUERY, json!({"id": id, "boot": boot}))?;
        Ok(data
            .get("machineLogs")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// Stream a machine's console log live.
    ///
    /// ```no_run
    /// # let client = bsdkrun_sdk::Client::new("localhost:50052", "tok")?;
    /// let sub = client
    ///     .follow_logs("abc123")
    ///     .on_data(|bytes| print!("{}", String::from_utf8_lossy(&bytes)))
    ///     .start()?;
    /// # Ok::<(), bsdkrun_sdk::Error>(())
    /// ```
    pub fn follow_logs(&self, id: &str) -> FollowLogsBuilder {
        FollowLogsBuilder {
            client: self.clone(),
            id: id.to_string(),
            follow: true,
            boot: false,
            on_data: None,
            on_error: None,
            on_complete: None,
        }
    }

    // -- booting -----------------------------------------------------------

    /// Boot a Linux machine on the daemon — `runLinux`.
    pub fn run_linux(&self) -> RunLinuxBuilder {
        RunLinuxBuilder {
            client: self.clone(),
            image: None,
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            volume: None,
            mounts: Vec::new(),
            attach_disk: Vec::new(),
            env: Vec::new(),
            entrypoint: None,
            initramfs: false,
            kernel: None,
            kernel_version: None,
            console: None,
            repo: None,
            command: Vec::new(),
        }
    }

    /// Boot FreeBSD or NetBSD on the daemon — `runBsd`.
    pub fn run_bsd(&self, os: BsdOs) -> RunBsdBuilder {
        RunBsdBuilder {
            client: self.clone(),
            os,
            version: None,
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            volume: None,
            persist: false,
            force: false,
            firmware: None,
            attach_disk: Vec::new(),
            disk_size: None,
            repo: None,
            command: Vec::new(),
        }
    }

    /// Boot a Nanos unikernel on the daemon — `runNanos`.
    pub fn run_nanos(&self) -> RunNanosBuilder {
        RunNanosBuilder {
            client: self.clone(),
            image: None,
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            kernel: None,
            cmdline: None,
            persist: false,
        }
    }

    /// Boot a Unikraft unikernel on the daemon — `runUnikraft`.
    pub fn run_unikraft(&self) -> RunUnikraftBuilder {
        RunUnikraftBuilder {
            client: self.clone(),
            path: None,
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            cmdline: None,
            initramfs: None,
            mounts: Vec::new(),
        }
    }

    /// Boot a Solo5 (MirageOS) unikernel on the daemon — `runSolo5`. Runs
    /// under the `solo5-hvt` tender rather than libkrun; the unikernel
    /// declares its own devices in its `MFT1` manifest, so only block
    /// backings and its own args are passed.
    pub fn run_solo5(&self) -> RunSolo5Builder {
        RunSolo5Builder {
            client: self.clone(),
            path: None,
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            block: Vec::new(),
            args: Vec::new(),
        }
    }

    /// Boot an OSv unikernel on the daemon — `runOsv`.
    pub fn run_osv(&self) -> RunOsvBuilder {
        RunOsvBuilder {
            client: self.clone(),
            image: None,
            cpus: None,
            mem: None,
            net: NetOpts::default(),
            cmdline: None,
            disk: None,
            no_disk: false,
            attach_disk: Vec::new(),
            gic: None,
            persist: false,
            volume: None,
        }
    }

    /// Boot a named flavor on the daemon — `runFlavor`.
    pub fn run_flavor(&self, name: impl Into<String>) -> RunFlavorBuilder {
        RunFlavorBuilder {
            client: self.clone(),
            name: name.into(),
            cpus: None,
            mem: None,
            ports: Vec::new(),
            volume: None,
            repo: None,
        }
    }

    // -- exec / interactive shell ------------------------------------------

    /// Run a command to completion via the machine's shell agent.
    pub fn exec<I, S>(&self, id: &str, command: I) -> Result<RemoteExecResult>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.exec_with_env(id, command, Vec::<String>::new())
    }

    /// [`Client::exec`] with per-command `"K=V"` environment entries.
    ///
    /// Sequenced exactly as `daemon/README.md` describes: `openShell` (with
    /// `command` set, so the session runs it instead of a login shell), THEN
    /// subscribe to `shellOutput` (output is buffered from the moment the
    /// session opened, so nothing is lost even though the subscribe
    /// necessarily happens after the mutation), collecting bytes until an
    /// event carries a non-null exit code, THEN `closeShell` — called
    /// unconditionally, including on error, since it is idempotent and a
    /// session must never be left dangling.
    pub fn exec_with_env<I, S, E, T>(
        &self,
        id: &str,
        command: I,
        env: E,
    ) -> Result<RemoteExecResult>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
        E: IntoIterator<Item = T>,
        T: Into<String>,
    {
        let transport = self.ws();
        let data = self.request(
            &open_shell_mutation(),
            json!({
                "machineId": id,
                "command": strvec(command),
                "env": strvec(env),
                "rows": 24,
                "cols": 80,
            }),
        )?;
        let session = ShellSessionInfo::from_graphql(&data["openShell"]);

        let chunks = Arc::new(Mutex::new(Vec::<u8>::new()));
        // The reader thread delivers `shellOutput` events via callbacks; this
        // channel is how the calling thread blocks until the one it cares
        // about (an exit code, or a terminal error) arrives, keeping exec() a
        // synchronous call.
        let (done_tx, done_rx) = mpsc::channel::<Result<i32>>();

        let chunk_sink = Arc::clone(&chunks);
        let exit_tx = done_tx.clone();
        let error_tx = done_tx.clone();
        let complete_tx = done_tx;

        let outcome: Result<i32> = (|| {
            let sub_id = transport.subscribe(
                SHELL_OUTPUT_SUBSCRIPTION,
                json!({"sessionId": session.id}),
                Box::new(move |data: Value| {
                    let payload = &data["shellOutput"];
                    if let Some(b64) = payload["dataBase64"].as_str() {
                        if let Ok(bytes) = B64.decode(b64) {
                            chunk_sink.lock().unwrap().extend_from_slice(&bytes);
                        }
                    }
                    if let Some(code) = payload["exitCode"].as_i64() {
                        let _ = exit_tx.send(Ok(code as i32));
                    }
                }),
                Box::new(move |err: Error| {
                    let _ = error_tx.send(Err(err));
                }),
                Box::new(move || {
                    // The subscription ended without ever delivering an exit
                    // code (e.g. the daemon tore the session down) — surface
                    // that instead of blocking forever.
                    let _ = complete_tx.send(Err(Error::GraphQL {
                        message: "shell session ended before an exit code arrived".to_string(),
                        code: None,
                    }));
                }),
            )?;
            let outcome = done_rx.recv().unwrap_or_else(|_| {
                Err(Error::GraphQL {
                    message: "the shell output subscription was dropped".to_string(),
                    code: None,
                })
            });
            transport.unsubscribe(&sub_id);
            outcome
        })();

        // closeShell runs unconditionally — including on error — since it is
        // idempotent and a session must never be left dangling.
        let _ = self.request(CLOSE_MUTATION, json!({"sessionId": session.id}));

        let exit_code = outcome?;
        let output = chunks.lock().unwrap().clone();
        Ok(RemoteExecResult { exit_code, output })
    }

    /// Open a live interactive session — output/exit arrive via callbacks.
    ///
    /// ```no_run
    /// # let client = bsdkrun_sdk::Client::new("localhost:50052", "tok")?;
    /// let session = client.shell("abc123").rows(50).cols(120).open()?;
    /// session.on_output(|bytes| print!("{}", String::from_utf8_lossy(bytes)));
    /// session.on_exit(|code| println!("exited {code}"));
    /// session.write("ls -la\n")?;
    /// # Ok::<(), bsdkrun_sdk::Error>(())
    /// ```
    pub fn shell(&self, id: &str) -> ShellBuilder {
        ShellBuilder {
            client: self.clone(),
            machine_id: id.to_string(),
            command: Vec::new(),
            env: Vec::new(),
            rows: 24,
            cols: 80,
        }
    }
}

/// A raw subscription handle returned by [`Client::subscribe`]. Dropping it
/// does *not* unsubscribe — call [`Subscription::unsubscribe`], matching the
/// Python SDK's explicit unsubscribe function.
pub struct Subscription {
    transport: Arc<WsTransport>,
    id: String,
}

impl Subscription {
    /// The graphql-transport-ws subscription id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// End the subscription.
    pub fn unsubscribe(self) {
        self.transport.unsubscribe(&self.id);
    }
}

// ---------------------------------------------------------------------------
// follow_logs
// ---------------------------------------------------------------------------

type DataFn = Box<dyn FnMut(Vec<u8>) + Send>;
type ErrFn = Box<dyn FnMut(Error) + Send>;
type DoneFn = Box<dyn FnMut() + Send>;

/// A live log stream being assembled — see [`Client::follow_logs`].
pub struct FollowLogsBuilder {
    client: Client,
    id: String,
    follow: bool,
    boot: bool,
    on_data: Option<DataFn>,
    on_error: Option<ErrFn>,
    on_complete: Option<DoneFn>,
}

impl FollowLogsBuilder {
    /// Keep following after the backlog (default true; false replays and ends).
    pub fn follow(mut self, follow: bool) -> Self {
        self.follow = follow;
        self
    }

    /// Stream bsdkrun's boot log instead of the console.
    pub fn boot(mut self, boot: bool) -> Self {
        self.boot = boot;
        self
    }

    /// Receive each chunk of log bytes.
    pub fn on_data(mut self, cb: impl FnMut(Vec<u8>) + Send + 'static) -> Self {
        self.on_data = Some(Box::new(cb));
        self
    }

    /// Receive the terminal error, if the stream fails.
    pub fn on_error(mut self, cb: impl FnMut(Error) + Send + 'static) -> Self {
        self.on_error = Some(Box::new(cb));
        self
    }

    /// Notified when the stream ends cleanly.
    pub fn on_complete(mut self, cb: impl FnMut() + Send + 'static) -> Self {
        self.on_complete = Some(Box::new(cb));
        self
    }

    /// Start streaming. Returns the [`Subscription`] to stop with.
    pub fn start(self) -> Result<Subscription> {
        let mut on_data = self.on_data.unwrap_or_else(|| Box::new(|_| {}));
        let on_error = self.on_error.unwrap_or_else(|| Box::new(|_| {}));
        let on_complete = self.on_complete.unwrap_or_else(|| Box::new(|| {}));
        let transport = self.client.ws();
        let id = transport.subscribe(
            MACHINE_LOGS_SUBSCRIPTION,
            json!({"id": self.id, "follow": self.follow, "boot": self.boot}),
            Box::new(move |data: Value| {
                if let Some(b64) = data
                    .pointer("/machineLogs/dataBase64")
                    .and_then(Value::as_str)
                {
                    if let Ok(bytes) = B64.decode(b64) {
                        on_data(bytes);
                    }
                }
                // exitCode marks the stream's end; graphql-transport-ws
                // follows it with its own "complete" message, which fires
                // on_complete.
            }),
            on_error,
            on_complete,
        )?;
        Ok(Subscription { transport, id })
    }
}

// ---------------------------------------------------------------------------
// run builders
// ---------------------------------------------------------------------------

/// The BSD to boot with [`Client::run_bsd`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsdOs {
    Freebsd,
    Netbsd,
}

impl BsdOs {
    fn graphql(self) -> &'static str {
        match self {
            BsdOs::Freebsd => "FREEBSD",
            BsdOs::Netbsd => "NETBSD",
        }
    }
}

fn net_input(net: &NetOpts) -> Value {
    if !net.touched {
        return Value::Null;
    }
    json!({
        "noNet": net.no_net,
        "ports": net.ports,
        "mac": net.mac,
        "network": net.network,
        "name": net.name,
    })
}

fn launch_mutation(client: &Client, mutation: &str, key: &str, input: Value) -> Result<String> {
    let data = client.request(mutation, json!({"input": input}))?;
    data.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::GraphQL {
            message: format!("the daemon's {key} response carried no machine id"),
            code: None,
        })
}

// The remote builders share the same net/vm option groups as the local create
// builders; the macros keep them from drifting apart between the seven run_*
// mutations, exactly as `NetInput` is one shared input object in the schema.
macro_rules! remote_net_vm_setters {
    () => {
        /// vCPU count.
        pub fn cpus(mut self, cpus: u32) -> Self {
            self.cpus = Some(cpus);
            self
        }

        /// Guest RAM in MiB.
        pub fn mem(mut self, mib: u32) -> Self {
            self.mem = Some(mib);
            self
        }

        /// Add a host->guest TCP port forward, `"HOST:GUEST"`.
        pub fn port(mut self, forward: impl Into<String>) -> Self {
            self.net.touched = true;
            self.net.ports.push(forward.into());
            self
        }

        /// Add a port forward from numbers instead of a string.
        pub fn forward(self, host: u16, guest: u16) -> Self {
            self.port(format!("{host}:{guest}"))
        }

        /// Pin the guest MAC address.
        pub fn mac(mut self, mac: impl Into<String>) -> Self {
            self.net.touched = true;
            self.net.mac = Some(mac.into());
            self
        }

        /// Join a global network.
        pub fn network(mut self, network: impl Into<String>) -> Self {
            self.net.touched = true;
            self.net.network = Some(network.into());
            self
        }

        /// Name the machine (the `NetInput.name` field).
        pub fn name(mut self, name: impl Into<String>) -> Self {
            self.net.touched = true;
            self.net.name = Some(name.into());
            self
        }

        /// Disable guest networking entirely.
        pub fn no_net(mut self) -> Self {
            self.net.touched = true;
            self.net.no_net = true;
            self
        }
    };
}

/// A `dockerStart` mutation being assembled — see [`Client::docker_start`].
pub struct DockerStartBuilder {
    client: Client,
    cpus: Option<u32>,
    mem: Option<u32>,
    mounts: Vec<String>,
    no_home: bool,
    publish_bind: Option<String>,
    disk_size: Option<String>,
}

impl DockerStartBuilder {
    /// vCPUs for the engine VM.
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = Some(cpus);
        self
    }

    /// Guest RAM in MiB.
    pub fn mem(mut self, mib: u32) -> Self {
        self.mem = Some(mib);
        self
    }

    /// Share a host directory into the VM, so `-v` can reach it: `PATH` (same
    /// path in the guest) or `HOST:GUEST`. Repeatable.
    pub fn mount(mut self, spec: impl Into<String>) -> Self {
        self.mounts.push(spec.into());
        self
    }

    /// Do not share `$HOME` (shared by default).
    pub fn no_home(mut self) -> Self {
        self.no_home = true;
        self
    }

    /// Where published container ports bind on the host: `mirror` (default —
    /// what the container asked for) or a fixed address.
    pub fn publish_bind(mut self, bind: impl Into<String>) -> Self {
        self.publish_bind = Some(bind.into());
        self
    }

    /// Give the image store a dedicated disk of this size, e.g. `60G`. Only
    /// applies when the VM is created.
    pub fn disk_size(mut self, size: impl Into<String>) -> Self {
        self.disk_size = Some(size.into());
        self
    }

    /// Start the engine and return its status once dockerd answers.
    pub fn launch(self) -> Result<DockerStatus> {
        let data = self.client.request(
            &docker_start_mutation(),
            json!({"input": {
                "cpus": self.cpus,
                "mem": self.mem,
                "mounts": self.mounts,
                "noHome": self.no_home,
                "publishBind": self.publish_bind,
                "diskSize": self.disk_size,
            }}),
        )?;
        Ok(DockerStatus::from_graphql(&data["dockerStart"]))
    }
}

/// A `branchSnapshot` mutation being assembled — see [`Client::branch`].
///
/// Unlike the `run_*` builders this has no `NetOpts`: a branch inherits the
/// snapshot's own port forwards unless told otherwise, and the guest's network
/// identity comes from the snapshot, not from the caller.
pub struct BranchBuilder {
    client: Client,
    snapshot: String,
    name: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    ports: Vec<String>,
    no_ports: bool,
}

impl BranchBuilder {
    /// Name the new machine. Generated when unset.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// vCPU count. Defaults to what the snapshot recorded.
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = Some(cpus);
        self
    }

    /// Guest RAM in MiB. Defaults to what the snapshot recorded.
    pub fn mem(mut self, mib: u32) -> Self {
        self.mem = Some(mib);
        self
    }

    /// Add a host→guest forward, `"[BIND:]HOST:GUEST"`. Given at least one,
    /// these replace the snapshot's recorded forwards.
    pub fn port(mut self, forward: impl Into<String>) -> Self {
        self.ports.push(forward.into());
        self
    }

    /// Add a port forward from numbers instead of a string.
    pub fn forward(self, host: u16, guest: u16) -> Self {
        self.port(format!("{host}:{guest}"))
    }

    /// Forward nothing, ignoring what the snapshot recorded.
    pub fn no_ports(mut self) -> Self {
        self.no_ports = true;
        self
    }

    /// Boot the branch and return its machine id.
    ///
    /// With no `port` set, the snapshot's own forwards are inherited — with
    /// any host port that is already taken swapped for a free one, since the
    /// machine the snapshot came from is usually still running on it.
    pub fn launch(self) -> Result<String> {
        let input = json!({
            "snapshot": self.snapshot,
            "name": self.name,
            "cpus": self.cpus,
            "mem": self.mem,
            "ports": self.ports,
            "noPorts": self.no_ports,
        });
        launch_mutation(&self.client, BRANCH_MUTATION, "branchSnapshot", input)
    }
}

/// A `runLinux` mutation being assembled — see [`Client::run_linux`].
pub struct RunLinuxBuilder {
    client: Client,
    image: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    net: NetOpts,
    volume: Option<String>,
    mounts: Vec<String>,
    attach_disk: Vec<String>,
    env: Vec<String>,
    entrypoint: Option<String>,
    initramfs: bool,
    kernel: Option<String>,
    kernel_version: Option<String>,
    console: Option<String>,
    repo: Option<String>,
    command: Vec<String>,
}

impl RunLinuxBuilder {
    remote_net_vm_setters!();

    /// The OCI image to boot (required).
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self
    }

    /// Use a persistent CoW volume as the rootfs.
    pub fn volume(mut self, name: impl Into<String>) -> Self {
        self.volume = Some(name.into());
        self
    }

    /// Share a host directory into the guest, `"HOST:GUEST"` (repeatable).
    pub fn mount(mut self, mount: impl Into<String>) -> Self {
        self.mounts.push(mount.into());
        self
    }

    /// Attach a raw disk image as virtio-blk, `"PATH"` or `"PATH:ro"`
    /// (repeatable).
    pub fn attach_disk(mut self, disk: impl Into<String>) -> Self {
        self.attach_disk.push(disk.into());
        self
    }

    /// Set a guest environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push(format!("{}={}", key.into(), value.into()));
        self
    }

    /// Override the image entrypoint.
    pub fn entrypoint(mut self, entrypoint: impl Into<String>) -> Self {
        self.entrypoint = Some(entrypoint.into());
        self
    }

    /// Boot through an initramfs.
    pub fn initramfs(mut self) -> Self {
        self.initramfs = true;
        self
    }

    /// Custom kernel image.
    pub fn kernel(mut self, kernel: impl Into<String>) -> Self {
        self.kernel = Some(kernel.into());
        self
    }

    /// Kernel version to fetch.
    pub fn kernel_version(mut self, version: impl Into<String>) -> Self {
        self.kernel_version = Some(version.into());
        self
    }

    /// Console device.
    pub fn console(mut self, console: impl Into<String>) -> Self {
        self.console = Some(console.into());
        self
    }

    /// Clone a git repo into the guest before running.
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// The command to run in the guest.
    pub fn command<I, S>(mut self, command: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.command = strvec(command);
        self
    }

    /// Boot the machine and return its id.
    pub fn launch(self) -> Result<String> {
        let Some(image) = self.image else {
            return Err(Error::InvalidInput("run_linux requires an image".into()));
        };
        let input = json!({
            "image": image,
            "cpus": self.cpus,
            "mem": self.mem,
            "net": net_input(&self.net),
            "volume": self.volume,
            "mounts": self.mounts,
            "attachDisk": self.attach_disk,
            "env": self.env,
            "entrypoint": self.entrypoint,
            "initramfs": self.initramfs,
            "kernel": self.kernel,
            "kernelVersion": self.kernel_version,
            "console": self.console,
            "repo": self.repo,
            "command": self.command,
        });
        launch_mutation(&self.client, RUN_LINUX_MUTATION, "runLinux", input)
    }
}

/// A `runBsd` mutation being assembled — see [`Client::run_bsd`].
pub struct RunBsdBuilder {
    client: Client,
    os: BsdOs,
    version: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    net: NetOpts,
    volume: Option<String>,
    persist: bool,
    force: bool,
    firmware: Option<String>,
    attach_disk: Vec<String>,
    disk_size: Option<String>,
    repo: Option<String>,
    command: Vec<String>,
}

impl RunBsdBuilder {
    remote_net_vm_setters!();

    /// The release to boot.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Use a persistent CoW volume as the root disk.
    pub fn volume(mut self, name: impl Into<String>) -> Self {
        self.volume = Some(name.into());
        self
    }

    /// Keep the root disk across `rm`.
    pub fn persist(mut self) -> Self {
        self.persist = true;
        self
    }

    /// Re-fetch the image even if cached.
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    /// Custom EFI firmware.
    pub fn firmware(mut self, firmware: impl Into<String>) -> Self {
        self.firmware = Some(firmware.into());
        self
    }

    /// Attach an extra raw disk, `"PATH"` or `"PATH:ro"` (repeatable).
    pub fn attach_disk(mut self, disk: impl Into<String>) -> Self {
        self.attach_disk.push(disk.into());
        self
    }

    /// Root disk size, e.g. `"20G"`.
    pub fn disk_size(mut self, size: impl Into<String>) -> Self {
        self.disk_size = Some(size.into());
        self
    }

    /// Clone a git repo into the guest before running.
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// The command to run in the guest.
    pub fn command<I, S>(mut self, command: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.command = strvec(command);
        self
    }

    /// Boot the machine and return its id.
    pub fn launch(self) -> Result<String> {
        let input = json!({
            "os": self.os.graphql(),
            "version": self.version,
            "cpus": self.cpus,
            "mem": self.mem,
            "net": net_input(&self.net),
            "volume": self.volume,
            "persist": self.persist,
            "force": self.force,
            "firmware": self.firmware,
            "attachDisk": self.attach_disk,
            "diskSize": self.disk_size,
            "repo": self.repo,
            "command": self.command,
        });
        launch_mutation(&self.client, RUN_BSD_MUTATION, "runBsd", input)
    }
}

/// A `runNanos` mutation being assembled — see [`Client::run_nanos`].
pub struct RunNanosBuilder {
    client: Client,
    image: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    net: NetOpts,
    kernel: Option<String>,
    cmdline: Option<String>,
    persist: bool,
}

impl RunNanosBuilder {
    remote_net_vm_setters!();

    /// A path, or a bare name in `~/.ops/images` (required).
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self
    }

    /// Nanos kernel override (Linux hosts).
    pub fn kernel(mut self, kernel: impl Into<String>) -> Self {
        self.kernel = Some(kernel.into());
        self
    }

    /// Kernel command line.
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = Some(cmdline.into());
        self
    }

    /// Keep the root disk across `rm`.
    pub fn persist(mut self) -> Self {
        self.persist = true;
        self
    }

    /// Boot the unikernel and return its machine id.
    pub fn launch(self) -> Result<String> {
        let Some(image) = self.image else {
            return Err(Error::InvalidInput("run_nanos requires an image".into()));
        };
        let input = json!({
            "image": image,
            "cpus": self.cpus,
            "mem": self.mem,
            "net": net_input(&self.net),
            "kernel": self.kernel,
            "cmdline": self.cmdline,
            "persist": self.persist,
        });
        launch_mutation(&self.client, RUN_NANOS_MUTATION, "runNanos", input)
    }
}

/// A `runUnikraft` mutation being assembled — see [`Client::run_unikraft`].
pub struct RunUnikraftBuilder {
    client: Client,
    path: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    net: NetOpts,
    cmdline: Option<String>,
    initramfs: Option<String>,
    mounts: Vec<String>,
}

impl RunUnikraftBuilder {
    remote_net_vm_setters!();

    /// A `kraft` project directory or a built unikernel image (defaults to `.`).
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Kernel command line; Unikraft hands it to the application as argv.
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = Some(cmdline.into());
        self
    }

    /// Initramfs image path.
    pub fn initramfs(mut self, path: impl Into<String>) -> Self {
        self.initramfs = Some(path.into());
        self
    }

    /// A virtio-fs share, `"HOST:GUEST"` with an absolute guest path
    /// (repeatable). Needs a unikernel built for it.
    pub fn mount(mut self, mount: impl Into<String>) -> Self {
        self.mounts.push(mount.into());
        self
    }

    /// Boot the unikernel and return its machine id.
    pub fn launch(self) -> Result<String> {
        let input = json!({
            "path": self.path,
            "cpus": self.cpus,
            "mem": self.mem,
            "net": net_input(&self.net),
            "cmdline": self.cmdline,
            "initramfs": self.initramfs,
            "mounts": self.mounts,
        });
        launch_mutation(&self.client, RUN_UNIKRAFT_MUTATION, "runUnikraft", input)
    }
}

/// A `runSolo5` mutation being assembled — see [`Client::run_solo5`].
pub struct RunSolo5Builder {
    client: Client,
    path: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    net: NetOpts,
    block: Vec<String>,
    args: Vec<String>,
}

impl RunSolo5Builder {
    remote_net_vm_setters!();

    /// A `.hvt` binary, or a project directory whose `dist/` holds one
    /// (defaults to `.`).
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Backing file for a declared block device, `"NAME=FILE"` (repeatable).
    pub fn block(mut self, block: impl Into<String>) -> Self {
        self.block.push(block.into());
        self
    }

    /// Arguments passed to the unikernel itself (e.g. `--ipv4=10.0.0.2/24`).
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = strvec(args);
        self
    }

    /// Boot the unikernel and return its machine id.
    pub fn launch(self) -> Result<String> {
        let input = json!({
            "path": self.path,
            "cpus": self.cpus,
            "mem": self.mem,
            "net": net_input(&self.net),
            "block": self.block,
            "args": self.args,
        });
        launch_mutation(&self.client, RUN_SOLO5_MUTATION, "runSolo5", input)
    }
}

/// A `runOsv` mutation being assembled — see [`Client::run_osv`].
pub struct RunOsvBuilder {
    client: Client,
    image: Option<String>,
    cpus: Option<u32>,
    mem: Option<u32>,
    net: NetOpts,
    cmdline: Option<String>,
    disk: Option<String>,
    no_disk: bool,
    attach_disk: Vec<String>,
    gic: Option<String>,
    persist: bool,
    volume: Option<String>,
}

impl RunOsvBuilder {
    remote_net_vm_setters!();

    /// An aarch64 `loader.img`, or on x86_64 the loader ELF (required).
    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = Some(image.into());
        self
    }

    /// The application to run and its arguments, e.g. `"/hello.so"`.
    pub fn cmdline(mut self, cmdline: impl Into<String>) -> Self {
        self.cmdline = Some(cmdline.into());
        self
    }

    /// Root disk (raw). Required on x86_64.
    pub fn disk(mut self, disk: impl Into<String>) -> Self {
        self.disk = Some(disk.into());
        self
    }

    /// Boot the kernel alone, with no root filesystem to mount.
    pub fn no_disk(mut self) -> Self {
        self.no_disk = true;
        self
    }

    /// Extra disks as virtio-blk, `"PATH"` or `"PATH:ro"` (repeatable).
    pub fn attach_disk(mut self, disk: impl Into<String>) -> Self {
        self.attach_disk.push(disk.into());
        self
    }

    /// `"v2"` (the default) or `"v3"`. aarch64 only.
    pub fn gic(mut self, gic: impl Into<String>) -> Self {
        self.gic = Some(gic.into());
        self
    }

    /// Keep the root disk across `rm`.
    pub fn persist(mut self) -> Self {
        self.persist = true;
        self
    }

    /// Use a persistent CoW volume as the root disk.
    pub fn volume(mut self, name: impl Into<String>) -> Self {
        self.volume = Some(name.into());
        self
    }

    /// Boot the unikernel and return its machine id.
    pub fn launch(self) -> Result<String> {
        let Some(image) = self.image else {
            return Err(Error::InvalidInput("run_osv requires an image".into()));
        };
        let input = json!({
            "image": image,
            "cpus": self.cpus,
            "mem": self.mem,
            "net": net_input(&self.net),
            "cmdline": self.cmdline,
            "disk": self.disk,
            "noDisk": self.no_disk,
            "attachDisk": self.attach_disk,
            "gic": self.gic,
            "persist": self.persist,
            "volume": self.volume,
        });
        launch_mutation(&self.client, RUN_OSV_MUTATION, "runOsv", input)
    }
}

/// A `runFlavor` mutation being assembled — see [`Client::run_flavor`].
pub struct RunFlavorBuilder {
    client: Client,
    name: String,
    cpus: Option<u32>,
    mem: Option<u32>,
    ports: Vec<String>,
    volume: Option<String>,
    repo: Option<String>,
}

impl RunFlavorBuilder {
    /// vCPU count.
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = Some(cpus);
        self
    }

    /// Guest RAM in MiB.
    pub fn mem(mut self, mib: u32) -> Self {
        self.mem = Some(mib);
        self
    }

    /// Add a host->guest TCP port forward, `"HOST:GUEST"`.
    pub fn port(mut self, forward: impl Into<String>) -> Self {
        self.ports.push(forward.into());
        self
    }

    /// Use a persistent CoW volume as the root disk.
    pub fn volume(mut self, name: impl Into<String>) -> Self {
        self.volume = Some(name.into());
        self
    }

    /// Clone a git repo into the guest before running.
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// Boot the flavor and return its machine id.
    pub fn launch(self) -> Result<String> {
        let input = json!({
            "name": self.name,
            "cpus": self.cpus,
            "mem": self.mem,
            "ports": self.ports,
            "volume": self.volume,
            "repo": self.repo,
        });
        launch_mutation(&self.client, RUN_FLAVOR_MUTATION, "runFlavor", input)
    }
}

// ---------------------------------------------------------------------------
// interactive shell sessions
// ---------------------------------------------------------------------------

/// An `openShell` mutation being assembled — see [`Client::shell`].
pub struct ShellBuilder {
    client: Client,
    machine_id: String,
    command: Vec<String>,
    env: Vec<String>,
    rows: u32,
    cols: u32,
}

impl ShellBuilder {
    /// Run this command instead of the machine's login shell.
    pub fn command<I, S>(mut self, command: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.command = strvec(command);
        self
    }

    /// Set a session environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push(format!("{}={}", key.into(), value.into()));
        self
    }

    /// Terminal rows (default 24).
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows;
        self
    }

    /// Terminal columns (default 80).
    pub fn cols(mut self, cols: u32) -> Self {
        self.cols = cols;
        self
    }

    /// Open the session and start streaming its output.
    pub fn open(self) -> Result<ShellSession> {
        let data = self.client.request(
            &open_shell_mutation(),
            json!({
                "machineId": self.machine_id,
                "command": self.command,
                "env": self.env,
                "rows": self.rows,
                "cols": self.cols,
            }),
        )?;
        let info = ShellSessionInfo::from_graphql(&data["openShell"]);
        ShellSession::start(self.client, info.id)
    }
}

type OutputFn = Box<dyn FnMut(&[u8]) + Send>;
type ExitFn = Box<dyn FnMut(i32) + Send>;

struct ShellShared {
    output_cb: Option<OutputFn>,
    exit_cb: Option<ExitFn>,
    /// Anything that arrives *before* a callback is registered — a real
    /// possibility, since the subscription starts inside `open()` and the
    /// daemon can reply before the caller registers anything — is buffered
    /// and flushed the moment a callback is set, so no frame is silently
    /// lost.
    buffered_output: Vec<Vec<u8>>,
    buffered_exit: Option<i32>,
    exit_fired: bool,
}

/// A live interactive session opened by [`Client::shell`].
///
/// Output and exit events arrive on the shared WS transport's reader thread
/// and are handed to whatever callbacks are registered via
/// [`ShellSession::on_output`] / [`ShellSession::on_exit`] at the time they
/// arrive. Callbacks run holding the session's internal lock, so they must
/// not call `on_output`/`on_exit` themselves (writing and resizing is fine).
pub struct ShellSession {
    id: String,
    client: Client,
    transport: Arc<WsTransport>,
    sub_id: String,
    shared: Arc<Mutex<ShellShared>>,
    closed: bool,
}

impl std::fmt::Debug for ShellSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellSession")
            .field("id", &self.id)
            .finish()
    }
}

impl ShellSession {
    fn start(client: Client, session_id: String) -> Result<ShellSession> {
        let shared = Arc::new(Mutex::new(ShellShared {
            output_cb: None,
            exit_cb: None,
            buffered_output: Vec::new(),
            buffered_exit: None,
            exit_fired: false,
        }));
        let transport = client.ws();

        let on_next_shared = Arc::clone(&shared);
        let on_error_shared = Arc::clone(&shared);
        let sub_id = transport.subscribe(
            SHELL_OUTPUT_SUBSCRIPTION,
            json!({"sessionId": session_id}),
            Box::new(move |data: Value| {
                let payload = &data["shellOutput"];
                if let Some(b64) = payload["dataBase64"].as_str() {
                    if let Ok(bytes) = B64.decode(b64) {
                        emit_output(&on_next_shared, bytes);
                    }
                }
                if let Some(code) = payload["exitCode"].as_i64() {
                    emit_exit(&on_next_shared, code as i32);
                }
            }),
            Box::new(move |_err: Error| {
                // A dropped connection ends the session the same way an exit
                // would, so a caller has one place (on_exit) to notice the
                // session is gone. -1 has no exit-code meaning of its own; it
                // just isn't 0.
                emit_exit(&on_error_shared, -1);
            }),
            Box::new(|| {}),
        )?;

        Ok(ShellSession {
            id: session_id,
            client,
            transport,
            sub_id,
            shared,
            closed: false,
        })
    }

    /// The session id, as `openShell` returned it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Register the output callback; anything buffered so far is flushed to
    /// it immediately.
    pub fn on_output(&self, mut cb: impl FnMut(&[u8]) + Send + 'static) {
        let mut shared = self.shared.lock().unwrap();
        for chunk in std::mem::take(&mut shared.buffered_output) {
            cb(&chunk);
        }
        shared.output_cb = Some(Box::new(cb));
    }

    /// Register the exit callback; a buffered exit fires immediately.
    pub fn on_exit(&self, mut cb: impl FnMut(i32) + Send + 'static) {
        let mut shared = self.shared.lock().unwrap();
        if let Some(code) = shared.buffered_exit.take() {
            cb(code);
        }
        shared.exit_cb = Some(Box::new(cb));
    }

    /// Send keystrokes (arbitrary bytes) to the session.
    pub fn write(&self, data: impl AsRef<[u8]>) -> Result<()> {
        self.client.request(
            SEND_INPUT_MUTATION,
            json!({"sessionId": self.id, "dataBase64": B64.encode(data.as_ref())}),
        )?;
        Ok(())
    }

    /// Apply a terminal resize, so full-screen programs in the guest redraw.
    pub fn resize(&self, rows: u32, cols: u32) -> Result<()> {
        self.client.request(
            RESIZE_MUTATION,
            json!({"sessionId": self.id, "rows": rows, "cols": cols}),
        )?;
        Ok(())
    }

    /// Close the session and kill its command. Idempotent.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.transport.unsubscribe(&self.sub_id);
        // closeShell is idempotent; an already-gone session is not a failure.
        let _ = self
            .client
            .request(CLOSE_MUTATION, json!({"sessionId": self.id}));
    }
}

fn emit_output(shared: &Arc<Mutex<ShellShared>>, data: Vec<u8>) {
    let mut guard = shared.lock().unwrap();
    match &mut guard.output_cb {
        Some(cb) => cb(&data),
        None => guard.buffered_output.push(data),
    }
}

fn emit_exit(shared: &Arc<Mutex<ShellShared>>, code: i32) {
    let mut guard = shared.lock().unwrap();
    if guard.exit_fired {
        return;
    }
    guard.exit_fired = true;
    match &mut guard.exit_cb {
        Some(cb) => cb(code),
        None => guard.buffered_exit = Some(code),
    }
}
