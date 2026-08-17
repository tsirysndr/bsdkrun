//// A client for a remote `bsdkrund` daemon's GraphQL API — the network
//// counterpart to `bsdkrun/sandbox`, which shells out to a local `bsdkrun`
//// binary. Point one at a daemon (`client.new`/`client.from_env`) and get
//// the same `SandboxInfo` records back over HTTP + WebSocket instead of a
//// subprocess.
////
//// ```gleam
//// import bsdkrun/client
//// import gleam/option.{None}
////
//// let assert Ok(c) = client.from_env()
//// let assert Ok(machines) = client.list(c, all: True)
//// let assert Ok(res) = client.exec(c, id: "abc123", command: ["uname", "-a"], env: [])
//// ```
////
//// Every function returns `Result(_, GraphqlError)` — an alias for
//// `bsdkrun/error`'s `Error` type (see that module), extended with the two
//// variants this client can produce: `GraphqlError` (a resolver error, a
//// malformed response, or the daemon being unreachable) and `AuthError`
//// (the daemon rejected the bearer token). Pattern-match on
//// `error.GraphqlError`/`error.AuthError`.
////
//// ## Design notes
////
//// - **HTTP transport**: `bsdkrun/graphql_transport`, over `:httpc` — see
////   that module. **WebSocket transport** (subscriptions):
////   `bsdkrun/ws`, hand-rolled RFC 6455 framing over `:gen_tcp`/`:ssl` — see
////   that module's doc for the split between its pure, unit-tested protocol
////   functions and its Erlang-FFI connection process.
//// - **One shared socket per `Client`**: `Client` itself is an immutable
////   url/token pair (constructing one does not connect anything). The
////   socket is opened lazily on first use and cached — see
////   `bsdkrun/ws.ensure`.
//// - **`exec` blocks**; **`shell`/`follow_logs`/`subscribe` do not.** `exec`
////   runs in the calling process and does a bounded `subject.receive` loop
////   itself (see the module doc on `bsdkrun/subject` for what `Subject` is
////   here, given `gleam_erlang` is not a dependency this SDK can use). The
////   other three each spawn one small background process (via Erlang's own
////   `spawn/1`, called directly through `@external`) whose only job is to
////   translate the WebSocket connection's raw events into this module's
////   richer event types and forward them to a `Subject` the caller reads at
////   its own pace — an unbounded wait, since an idle interactive shell or a
////   quiet log stream legitimately has nothing to say for a long time and
////   should not be timed out for it. A dead connection still always
////   produces a terminal event (the WS actor notifies every open
////   subscription when its socket closes), so this does not risk an actual
////   forever-hang.

import bsdkrun/error.{type Error, AuthError, DecodeFailed, GraphqlError}
import bsdkrun/graphql_transport
import bsdkrun/subject.{type Subject}
import bsdkrun/types.{
  type AiAgent, type AiSession, type CommandResult, type DockerContainer,
  type DockerStatus, type ExecResult, type SandboxInfo, type ShellEvent,
  type ShellSessionInfo, type SnapshotInfo, type SubscriptionEvent, ExecResult,
  ShellClosed, ShellData, ShellError, ShellExit, SubComplete, SubError, SubNext,
}
import bsdkrun/ws
import gleam/bit_array
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode
import gleam/json.{type Json}
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result
import gleam/string

/// A `bsdkrun/client` error — every variant `bsdkrun/error.Error` has, but
/// only `GraphqlError` and `AuthError` are ones this module itself produces.
pub type GraphqlError =
  Error

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A handle to a daemon's GraphQL endpoint: its URL and bearer token.
/// Constructing one does not connect to anything — the HTTP transport is a
/// plain request per call, and the WebSocket connection (only needed for
/// `exec`/`shell`/`follow_logs`/`subscribe`) is opened lazily on first use.
pub opaque type Client {
  Client(url: String, token: String)
}

/// Build a client explicitly. `url` is normalized exactly like
/// `bsdkrun/graphql_transport.normalize_url` (and the web UI's connection
/// setup): a scheme is assumed if missing, trailing slashes are stripped,
/// and `/graphql` is appended if the path doesn't already end with it.
pub fn new(url url: String, token token: String) -> Client {
  Client(url: graphql_transport.normalize_url(url), token: string.trim(token))
}

@external(erlang, "bsdkrun_ffi", "get_env")
fn ffi_get_env(name: String) -> Result(String, Nil)

/// Build a client from `BSDKRUN_URL`/`BSDKRUN_TOKEN`. `BSDKRUN_URL` unset is
/// `Error` (nothing to connect to). `BSDKRUN_URL` set without
/// `BSDKRUN_TOKEN` is *also* `Error`, deliberately — not a silent fallback
/// to running unauthenticated — mirroring the same rule
/// `daemon/src/client.rs`'s `RemoteConfig::from_env` applies to the gRPC
/// `BSDKRUN_HOST`/`BSDKRUN_TOKEN` pair (a different pair of variables: this
/// is the GraphQL port, not the gRPC one, so it gets its own).
pub fn from_env() -> Result(Client, String) {
  case ffi_get_env("BSDKRUN_URL") {
    Error(Nil) -> Error("BSDKRUN_URL is not set")
    Ok(url) ->
      case ffi_get_env("BSDKRUN_TOKEN") {
        Error(Nil) -> Error("BSDKRUN_URL is set but BSDKRUN_TOKEN is not")
        Ok(token) -> Ok(new(url: url, token: token))
      }
  }
}

// ---------------------------------------------------------------------------
// internals: query/mutation plumbing shared by every typed call
// ---------------------------------------------------------------------------

fn query(
  client: Client,
  doc: String,
  variables: Json,
) -> Result(Dynamic, Error) {
  graphql_transport.execute(client.url, client.token, doc, variables)
}

/// Pull `name` off `dyn` as a raw `Dynamic`, for a further `types.*`
/// decoder to take from there. Isolated (its own `decode.run`) so a
/// response shaped differently than expected degrades to `Error(Nil)`
/// rather than crashing.
fn field_dynamic(dyn: Dynamic, name: String) -> Result(Dynamic, Nil) {
  case decode.run(dyn, decode.field(name, decode.dynamic, decode.success)) {
    Ok(value) -> Ok(value)
    Error(_) -> Error(Nil)
  }
}

fn field_dynamic_list(dyn: Dynamic, name: String) -> List(Dynamic) {
  case
    decode.run(
      dyn,
      decode.field(name, decode.list(decode.dynamic), decode.success),
    )
  {
    Ok(values) -> values
    Error(_) -> []
  }
}

fn field_string(dyn: Dynamic, name: String) -> Result(String, Nil) {
  case decode.run(dyn, decode.field(name, decode.string, decode.success)) {
    Ok(value) -> Ok(value)
    Error(_) -> Error(Nil)
  }
}

/// Run a mutation whose GraphQL result field is a `CommandResult`
/// (`{ exitCode stdout stderr }`) and decode it into the local
/// `types.CommandResult`, labelling it `label` (see
/// `types.command_result_from_graphql`).
fn command_mutation(
  client: Client,
  doc: String,
  variables: Json,
  field: String,
  label: String,
) -> Result(CommandResult, Error) {
  use data <- result.try(query(client, doc, variables))
  case field_dynamic(data, field) {
    Ok(row) -> types.command_result_from_graphql(row, label)
    Error(Nil) -> Error(DecodeFailed(label, string.inspect(data)))
  }
}

/// Run a mutation whose GraphQL result is a bare `String` (every `run*`
/// mutation: the new machine's id).
fn run_mutation(
  client: Client,
  doc: String,
  variables: Json,
  field: String,
) -> Result(String, Error) {
  use data <- result.try(query(client, doc, variables))
  case field_string(data, field) {
    Ok(id) -> Ok(id)
    Error(Nil) -> Error(DecodeFailed(field, string.inspect(data)))
  }
}

// ---------------------------------------------------------------------------
// lifecycle / listing
// ---------------------------------------------------------------------------

/// The `MACHINE_FIELDS` selection `web/src/lib/api.ts` uses, transcribed
/// verbatim so `sandbox_info_from_graphql` always has what it expects.
const machine_fields = "id name image kind command status running exitCode pid detached cpus mem volume stateDir createdAt finishedAt network netIp origin ports { bind host guest }"

const ai_agent_fields = "id label flavor description installed running"

const ai_session_fields = "id name agent running workspace createdAt"

const docker_status_fields = "running machineId machineRunning socket socketReady apiPort version containers images mounts disk diskSize"

const docker_container_fields = "id name image command state status ports created"

const snapshot_fields = "id name machineId machineName kind image path parent description cpus mem size createdAt ports { bind host guest }"

/// Machines. `all: True` includes stopped ones, like `bsdkrun ps -a`.
pub fn list(client: Client, all all: Bool) -> Result(List(SandboxInfo), Error) {
  let doc =
    "query($all: Boolean!) { machines(all: $all) { " <> machine_fields <> " } }"
  use data <- result.try(query(
    client,
    doc,
    json.object([#("all", json.bool(all))]),
  ))
  field_dynamic_list(data, "machines")
  |> list.try_map(types.sandbox_info_from_graphql)
}

/// A single machine by id, name, or unique id prefix — or `None` if there is
/// no such machine.
pub fn get(
  client: Client,
  id id: String,
) -> Result(Option(SandboxInfo), Error) {
  let doc =
    "query($id: String!) { machine(id: $id) { " <> machine_fields <> " } }"
  use data <- result.try(query(
    client,
    doc,
    json.object([#("id", json.string(id))]),
  ))
  case
    decode.run(
      data,
      decode.field("machine", decode.optional(decode.dynamic), decode.success),
    )
  {
    Ok(Some(row)) -> types.sandbox_info_from_graphql(row) |> result.map(Some)
    Ok(None) -> Ok(None)
    Error(_) -> Error(DecodeFailed("machine", string.inspect(data)))
  }
}

/// Stop a machine.
pub fn stop(client: Client, id id: String) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($id: String!) { stopMachine(id: $id) { exitCode stdout stderr } }",
    json.object([#("id", json.string(id))]),
    "stopMachine",
    "stopMachine",
  )
}

/// Restart a stopped machine in place.
pub fn start(client: Client, id id: String) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($id: String!) { startMachine(id: $id) { exitCode stdout stderr } }",
    json.object([#("id", json.string(id))]),
    "startMachine",
    "startMachine",
  )
}

/// Remove one or more machines. `force` stops any that are still running
/// first.
pub fn remove(
  client: Client,
  ids ids: List(String),
  force force: Bool,
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($ids: [String!]!, $force: Boolean!) { removeMachines(ids: $ids, force: $force) { exitCode stdout stderr } }",
    json.object([
      #("ids", json.array(ids, json.string)),
      #("force", json.bool(force)),
    ]),
    "removeMachines",
    "removeMachines",
  )
}

/// Change a machine's recorded vCPU/RAM. Applies on the next `start`.
pub fn update(
  client: Client,
  id id: String,
  cpus cpus: Option(Int),
  mem mem: Option(Int),
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($id: String!, $cpus: Int, $mem: Int) { updateMachine(id: $id, cpus: $cpus, mem: $mem) { exitCode stdout stderr } }",
    json.object([
      #("id", json.string(id)),
      #("cpus", json.nullable(cpus, json.int)),
      #("mem", json.nullable(mem, json.int)),
    ]),
    "updateMachine",
    "updateMachine",
  )
}

/// Snapshot a machine into a named flavor, like `docker commit`.
pub fn commit(
  client: Client,
  id id: String,
  name name: String,
  description description: String,
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($id: String!, $name: String!, $description: String!) { commitMachine(id: $id, name: $name, description: $description) { exitCode stdout stderr } }",
    json.object([
      #("id", json.string(id)),
      #("name", json.string(name)),
      #("description", json.string(description)),
    ]),
    "commitMachine",
    "commitMachine",
  )
}

// ---------------------------------------------------------------------------
// ai agents
//
// A sandbox is a machine, so its terminal is the ordinary shell with the argv
// `ai_shell_command` returns.
// ---------------------------------------------------------------------------

/// The coding agents, and whether each one's sandbox image is built.
pub fn ai_agents(client: Client) -> Result(List(AiAgent), Error) {
  let doc = "{ aiAgents { " <> ai_agent_fields <> " } }"
  use data <- result.try(query(client, doc, json.object([])))
  field_dynamic_list(data, "aiAgents")
  |> list.try_map(types.ai_agent_from_graphql)
}

/// Agent sandboxes, newest first.
pub fn ai_sessions(client: Client) -> Result(List(AiSession), Error) {
  let doc = "{ aiSessions { " <> ai_session_fields <> " } }"
  use data <- result.try(query(client, doc, json.object([])))
  field_dynamic_list(data, "aiSessions")
  |> list.try_map(types.ai_session_from_graphql)
}

/// Start (or reuse) a sandbox; returns its machine id.
///
/// `workspace` is a path **on the engine's host** — a remote daemon cannot see
/// your own filesystem. `new` boots a second sandbox against the same login.
pub fn ai_start(
  client: Client,
  agent agent: String,
  cpus cpus: Option(Int),
  mem mem: Option(Int),
  workspace workspace: Option(String),
  new new: Bool,
) -> Result(String, Error) {
  let doc = "mutation($input: AiStartInput!) { aiStart(input: $input) }"
  use data <- result.try(query(
    client,
    doc,
    json.object([
      #(
        "input",
        json.object([
          #("agent", json.string(agent)),
          #("cpus", json.nullable(cpus, json.int)),
          #("mem", json.nullable(mem, json.int)),
          #("workspace", json.nullable(workspace, json.string)),
          #("new", json.bool(new)),
        ]),
      ),
    ]),
  ))
  case field_string(data, "aiStart") {
    Ok(id) -> Ok(id)
    Error(Nil) -> Error(DecodeFailed("aiStart", string.inspect(data)))
  }
}

/// The argv that starts the agent's TUI — pass it to the shell.
pub fn ai_shell_command(
  client: Client,
  agent agent: String,
  machine_id machine_id: String,
) -> Result(List(String), Error) {
  let doc =
    "query($agent: String!, $machineId: String!) { aiShellCommand(agent: $agent, machineId: $machineId) }"
  use data <- result.try(query(
    client,
    doc,
    json.object([
      #("agent", json.string(agent)),
      #("machineId", json.string(machine_id)),
    ]),
  ))
  case
    decode.run(
      data,
      decode.field("aiShellCommand", decode.list(decode.string), decode.success),
    )
  {
    Ok(argv) -> Ok(argv)
    Error(_) -> Error(DecodeFailed("aiShellCommand", string.inspect(data)))
  }
}

/// Stop an agent's sandboxes. Its saved login survives.
pub fn ai_stop(
  client: Client,
  agent agent: String,
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($agent: String!) { aiStop(agent: $agent) { exitCode stdout stderr } }",
    json.object([#("agent", json.string(agent))]),
    "aiStop",
    "aiStop",
  )
}

/// Remove an agent's sandboxes, and unless `keep_home` its saved login too.
pub fn ai_remove(
  client: Client,
  agent agent: String,
  keep_home keep_home: Bool,
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($agent: String!, $keepHome: Boolean!) { aiRemove(agent: $agent, keepHome: $keepHome) { exitCode stdout stderr } }",
    json.object([
      #("agent", json.string(agent)),
      #("keepHome", json.bool(keep_home)),
    ]),
    "aiRemove",
    "aiRemove",
  )
}

// ---------------------------------------------------------------------------
// docker
//
// bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
// socket, so these drive the same engine the host's `docker` CLI does.
// ---------------------------------------------------------------------------

/// Is the Docker engine up, and where is its socket?
pub fn docker_status(client: Client) -> Result(DockerStatus, Error) {
  let doc = "{ dockerStatus { " <> docker_status_fields <> " } }"
  use data <- result.try(query(client, doc, json.object([])))
  case field_dynamic(data, "dockerStatus") {
    Ok(row) -> types.docker_status_from_graphql(row)
    Error(Nil) -> Error(DecodeFailed("dockerStatus", string.inspect(data)))
  }
}

/// Containers in the engine. `all: False` lists only running ones.
pub fn docker_containers(
  client: Client,
  all all: Bool,
) -> Result(List(DockerContainer), Error) {
  let doc =
    "query($all: Boolean!) { dockerContainers(all: $all) { "
    <> docker_container_fields
    <> " } }"
  use data <- result.try(query(
    client,
    doc,
    json.object([#("all", json.bool(all))]),
  ))
  field_dynamic_list(data, "dockerContainers")
  |> list.try_map(types.docker_container_from_graphql)
}

/// Start (or resume) the engine, returning its status once it answers.
///
/// Idempotent: the VM has a fixed name, so this resumes the existing one
/// rather than creating a second.
pub fn docker_start(
  client: Client,
  cpus cpus: Option(Int),
  mem mem: Option(Int),
  mounts mounts: List(String),
  no_home no_home: Bool,
  publish_bind publish_bind: Option(String),
  disk_size disk_size: Option(String),
) -> Result(DockerStatus, Error) {
  let doc =
    "mutation($input: DockerStartInput!) { dockerStart(input: $input) { "
    <> docker_status_fields
    <> " } }"
  use data <- result.try(query(
    client,
    doc,
    json.object([
      #(
        "input",
        json.object([
          #("cpus", json.nullable(cpus, json.int)),
          #("mem", json.nullable(mem, json.int)),
          #("mounts", json.array(mounts, json.string)),
          #("noHome", json.bool(no_home)),
          #("publishBind", json.nullable(publish_bind, json.string)),
          #("diskSize", json.nullable(disk_size, json.string)),
        ]),
      ),
    ]),
  ))
  case field_dynamic(data, "dockerStart") {
    Ok(row) -> types.docker_status_from_graphql(row)
    Error(Nil) -> Error(DecodeFailed("dockerStart", string.inspect(data)))
  }
}

/// Stop the engine. Images and containers stay on its disk.
pub fn docker_stop(client: Client) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation { dockerStop { exitCode stdout stderr } }",
    json.object([]),
    "dockerStop",
    "dockerStop",
  )
}

/// Act on containers: start / stop / restart / kill / pause / unpause / rm.
pub fn docker_container(
  client: Client,
  action action: String,
  ids ids: List(String),
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($action: String!, $ids: [String!]!) { dockerContainer(action: $action, ids: $ids) { exitCode stdout stderr } }",
    json.object([
      #("action", json.string(action)),
      #("ids", json.array(ids, json.string)),
    ]),
    "dockerContainer",
    "dockerContainer",
  )
}

/// One container's logs (stdout+stderr, most recent `tail` lines).
pub fn docker_logs(
  client: Client,
  id id: String,
  tail tail: Int,
) -> Result(String, Error) {
  let doc =
    "query($id: String!, $tail: Int!) { dockerContainerLogs(id: $id, tail: $tail) }"
  use data <- result.try(query(
    client,
    doc,
    json.object([#("id", json.string(id)), #("tail", json.int(tail))]),
  ))
  case field_string(data, "dockerContainerLogs") {
    Ok(text) -> Ok(text)
    Error(Nil) ->
      Error(DecodeFailed("dockerContainerLogs", string.inspect(data)))
  }
}

// ---------------------------------------------------------------------------
// snapshots
//
// A snapshot is a copy-on-write clone of a machine's disk state: instant to
// take, free until the two sides diverge. `branch` boots a new machine from
// one; `restore`/`rollback` put one back.
// ---------------------------------------------------------------------------

/// Snapshots, newest first. `machine` narrows the list to one machine's.
pub fn snapshots(
  client: Client,
  machine machine: Option(String),
) -> Result(List(SnapshotInfo), Error) {
  let doc =
    "query($machine: String) { snapshots(machine: $machine) { "
    <> snapshot_fields
    <> " } }"
  use data <- result.try(query(
    client,
    doc,
    json.object([#("machine", json.nullable(machine, json.string))]),
  ))
  field_dynamic_list(data, "snapshots")
  |> list.try_map(types.snapshot_info_from_graphql)
}

/// Capture a machine's disk state. `name` of `None` yields `<machine>-<n>`.
///
/// A BSD guest is powered off first — a mounted UFS cannot be cloned
/// consistently — so the machine is left stopped; `start` brings it back.
pub fn snapshot(
  client: Client,
  id id: String,
  name name: Option(String),
  description description: String,
) -> Result(SnapshotInfo, Error) {
  let doc =
    "mutation($id: String!, $name: String, $description: String!) { snapshotMachine(id: $id, name: $name, description: $description) { "
    <> snapshot_fields
    <> " } }"
  use data <- result.try(query(
    client,
    doc,
    json.object([
      #("id", json.string(id)),
      #("name", json.nullable(name, json.string)),
      #("description", json.string(description)),
    ]),
  ))
  case field_dynamic(data, "snapshotMachine") {
    Ok(row) -> types.snapshot_info_from_graphql(row)
    Error(Nil) -> Error(DecodeFailed("snapshotMachine", string.inspect(data)))
  }
}

/// Delete snapshots and their data. Machines already branched from them are
/// unaffected.
pub fn remove_snapshots(
  client: Client,
  names names: List(String),
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($names: [String!]!) { removeSnapshots(names: $names) { exitCode stdout stderr } }",
    json.object([#("names", json.array(names, json.string))]),
    "removeSnapshots",
    "removeSnapshots",
  )
}

/// Put a machine's disk state back to one of its snapshots.
///
/// `force` stops the machine first — it holds the very files being replaced.
/// `backup` snapshots the state being overwritten, which is a CoW clone and
/// therefore free. The machine is left stopped.
pub fn restore(
  client: Client,
  id id: String,
  snapshot snapshot: String,
  force force: Bool,
  backup backup: Bool,
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($id: String!, $snapshot: String!, $force: Boolean!, $backup: Boolean!) { restoreMachine(id: $id, snapshot: $snapshot, force: $force, backup: $backup) { exitCode stdout stderr } }",
    json.object([
      #("id", json.string(id)),
      #("snapshot", json.string(snapshot)),
      #("force", json.bool(force)),
      #("backup", json.bool(backup)),
    ]),
    "restoreMachine",
    "restoreMachine",
  )
}

/// Restore a machine to its most recent snapshot.
pub fn rollback(
  client: Client,
  id id: String,
  force force: Bool,
  backup backup: Bool,
) -> Result(CommandResult, Error) {
  command_mutation(
    client,
    "mutation($id: String!, $force: Boolean!, $backup: Boolean!) { rollbackMachine(id: $id, force: $force, backup: $backup) { exitCode stdout stderr } }",
    json.object([
      #("id", json.string(id)),
      #("force", json.bool(force)),
      #("backup", json.bool(backup)),
    ]),
    "rollbackMachine",
    "rollbackMachine",
  )
}

/// Boot a NEW machine from a snapshot — or from a machine, which is
/// snapshotted first — and return the new machine's id.
///
/// The state is cloned, never booted in place, so the source is untouched and
/// one snapshot can be branched any number of times. An empty `ports`
/// inherits the snapshot's own forwards, with any host port that is already
/// taken swapped for a free one; `no_ports` drops them instead.
pub fn branch(
  client: Client,
  snapshot snapshot: String,
  name name: Option(String),
  cpus cpus: Option(Int),
  mem mem: Option(Int),
  ports ports: List(String),
  no_ports no_ports: Bool,
) -> Result(String, Error) {
  let doc = "mutation($input: BranchInput!) { branchSnapshot(input: $input) }"
  use data <- result.try(query(
    client,
    doc,
    json.object([
      #(
        "input",
        json.object([
          #("snapshot", json.string(snapshot)),
          #("name", json.nullable(name, json.string)),
          #("cpus", json.nullable(cpus, json.int)),
          #("mem", json.nullable(mem, json.int)),
          #("ports", json.array(ports, json.string)),
          #("noPorts", json.bool(no_ports)),
        ]),
      ),
    ]),
  ))
  case field_string(data, "branchSnapshot") {
    Ok(id) -> Ok(id)
    Error(Nil) -> Error(DecodeFailed("branchSnapshot", string.inspect(data)))
  }
}

/// A machine's console log as a single string, as it stands right now. Use
/// `follow_logs` to watch it live.
pub fn logs(
  client: Client,
  id id: String,
  boot boot: Bool,
) -> Result(String, Error) {
  let doc =
    "query($id: String!, $boot: Boolean!) { machineLogs(id: $id, boot: $boot) }"
  use data <- result.try(query(
    client,
    doc,
    json.object([#("id", json.string(id)), #("boot", json.bool(boot))]),
  ))
  case field_string(data, "machineLogs") {
    Ok(text) -> Ok(text)
    Error(Nil) -> Error(DecodeFailed("machineLogs", string.inspect(data)))
  }
}

/// Follow a machine's console log live — everything buffered since the
/// subscription started, then new lines as they're written. Ends
/// (`ShellClosed`) when the underlying `bsdkrun logs -f` exits, which with
/// `follow: True` is when the machine stops.
pub fn follow_logs(
  client: Client,
  id id: String,
  follow follow: Bool,
  boot boot: Bool,
) -> Result(Subject(ShellEvent), Error) {
  let doc =
    "subscription($id: String!, $follow: Boolean!, $boot: Boolean!) { machineLogs(id: $id, follow: $follow, boot: $boot) { dataBase64 exitCode } }"
  let vars =
    json.object([
      #("id", json.string(id)),
      #("follow", json.bool(follow)),
      #("boot", json.bool(boot)),
    ])
  start_shell_events(client, doc, vars)
}

// ---------------------------------------------------------------------------
// booting
// ---------------------------------------------------------------------------

/// The `net` field shared by every `run*` mutation except `runFlavor`
/// (`daemon/src/graphql.rs`'s `NetInput`, ~line 360).
pub type NetOptions {
  NetOptions(
    no_net: Bool,
    ports: List(String),
    mac: Option(String),
    network: Option(String),
    name: Option(String),
  )
}

/// Default networking: attached, no forwards, no mac/network/name override.
pub fn net_options() -> NetOptions {
  NetOptions(no_net: False, ports: [], mac: None, network: None, name: None)
}

fn net_json(net: Option(NetOptions)) -> Json {
  case net {
    None -> json.null()
    Some(n) ->
      json.object([
        #("noNet", json.bool(n.no_net)),
        #("ports", json.array(n.ports, json.string)),
        #("mac", json.nullable(n.mac, json.string)),
        #("network", json.nullable(n.network, json.string)),
        #("name", json.nullable(n.name, json.string)),
      ])
  }
}

/// `RunLinuxInput` (`daemon/src/graphql.rs` ~line 384).
pub type RunLinuxOptions {
  RunLinuxOptions(
    image: String,
    cpus: Option(Int),
    mem: Option(Int),
    net: Option(NetOptions),
    volume: Option(String),
    mounts: List(String),
    attach_disk: List(String),
    env: List(String),
    entrypoint: Option(String),
    initramfs: Bool,
    kernel: Option(String),
    kernel_version: Option(String),
    console: Option(String),
    repo: Option(String),
    command: List(String),
  )
}

/// Defaults for `RunLinuxOptions`: everything unset/empty except `image`.
pub fn run_linux_options(image: String) -> RunLinuxOptions {
  RunLinuxOptions(
    image: image,
    cpus: None,
    mem: None,
    net: None,
    volume: None,
    mounts: [],
    attach_disk: [],
    env: [],
    entrypoint: None,
    initramfs: False,
    kernel: None,
    kernel_version: None,
    console: None,
    repo: None,
    command: [],
  )
}

/// Boot a Linux (OCI) machine, detached. Returns the new machine's id.
pub fn run_linux(
  client: Client,
  opts opts: RunLinuxOptions,
) -> Result(String, Error) {
  let input =
    json.object([
      #("image", json.string(opts.image)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("net", net_json(opts.net)),
      #("volume", json.nullable(opts.volume, json.string)),
      #("mounts", json.array(opts.mounts, json.string)),
      #("attachDisk", json.array(opts.attach_disk, json.string)),
      #("env", json.array(opts.env, json.string)),
      #("entrypoint", json.nullable(opts.entrypoint, json.string)),
      #("initramfs", json.bool(opts.initramfs)),
      #("kernel", json.nullable(opts.kernel, json.string)),
      #("kernelVersion", json.nullable(opts.kernel_version, json.string)),
      #("console", json.nullable(opts.console, json.string)),
      #("repo", json.nullable(opts.repo, json.string)),
      #("command", json.array(opts.command, json.string)),
    ])
  run_mutation(
    client,
    "mutation($input: RunLinuxInput!) { runLinux(input: $input) }",
    json.object([#("input", input)]),
    "runLinux",
  )
}

/// `BsdOs` (`daemon/src/graphql.rs` ~line 324): which BSD to boot.
pub type BsdOs {
  Freebsd
  Netbsd
}

fn bsd_os_json(os: BsdOs) -> Json {
  case os {
    Freebsd -> json.string("FREEBSD")
    Netbsd -> json.string("NETBSD")
  }
}

/// `RunBsdInput` (`daemon/src/graphql.rs` ~line 406).
pub type RunBsdOptions {
  RunBsdOptions(
    os: BsdOs,
    version: Option(String),
    cpus: Option(Int),
    mem: Option(Int),
    net: Option(NetOptions),
    volume: Option(String),
    persist: Bool,
    force: Bool,
    firmware: Option(String),
    attach_disk: List(String),
    disk_size: Option(String),
    repo: Option(String),
    command: List(String),
  )
}

/// Defaults for `RunBsdOptions`: everything unset/empty/false except `os`.
pub fn run_bsd_options(os: BsdOs) -> RunBsdOptions {
  RunBsdOptions(
    os: os,
    version: None,
    cpus: None,
    mem: None,
    net: None,
    volume: None,
    persist: False,
    force: False,
    firmware: None,
    attach_disk: [],
    disk_size: None,
    repo: None,
    command: [],
  )
}

/// Boot a FreeBSD/NetBSD machine, detached. Returns the new machine's id.
pub fn run_bsd(
  client: Client,
  opts opts: RunBsdOptions,
) -> Result(String, Error) {
  let input =
    json.object([
      #("os", bsd_os_json(opts.os)),
      #("version", json.nullable(opts.version, json.string)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("net", net_json(opts.net)),
      #("volume", json.nullable(opts.volume, json.string)),
      #("persist", json.bool(opts.persist)),
      #("force", json.bool(opts.force)),
      #("firmware", json.nullable(opts.firmware, json.string)),
      #("attachDisk", json.array(opts.attach_disk, json.string)),
      #("diskSize", json.nullable(opts.disk_size, json.string)),
      #("repo", json.nullable(opts.repo, json.string)),
      #("command", json.array(opts.command, json.string)),
    ])
  run_mutation(
    client,
    "mutation($input: RunBsdInput!) { runBsd(input: $input) }",
    json.object([#("input", input)]),
    "runBsd",
  )
}

/// `RunNanosInput` (`daemon/src/graphql.rs` ~line 428). Nanos has no agent
/// (no `exec`/`shell`/`commit`), but does have a root disk, so `persist` is
/// the one disk option it takes.
pub type RunNanosOptions {
  RunNanosOptions(
    image: String,
    cpus: Option(Int),
    mem: Option(Int),
    net: Option(NetOptions),
    kernel: Option(String),
    cmdline: Option(String),
    persist: Bool,
  )
}

/// Defaults for `RunNanosOptions`: everything unset/false except `image`.
pub fn run_nanos_options(image: String) -> RunNanosOptions {
  RunNanosOptions(
    image: image,
    cpus: None,
    mem: None,
    net: None,
    kernel: None,
    cmdline: None,
    persist: False,
  )
}

/// Boot a Nanos unikernel, detached. Returns the new machine's id.
pub fn run_nanos(
  client: Client,
  opts opts: RunNanosOptions,
) -> Result(String, Error) {
  let input =
    json.object([
      #("image", json.string(opts.image)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("net", net_json(opts.net)),
      #("kernel", json.nullable(opts.kernel, json.string)),
      #("cmdline", json.nullable(opts.cmdline, json.string)),
      #("persist", json.bool(opts.persist)),
    ])
  run_mutation(
    client,
    "mutation($input: RunNanosInput!) { runNanos(input: $input) }",
    json.object([#("input", input)]),
    "runNanos",
  )
}

/// `RunUnikraftInput` (`daemon/src/graphql.rs` ~line 449). A unikernel has
/// no disk and no agent, so this carries none of the volume/persist/
/// repo/command fields the other guests take.
pub type RunUnikraftOptions {
  RunUnikraftOptions(
    path: Option(String),
    cpus: Option(Int),
    mem: Option(Int),
    net: Option(NetOptions),
    cmdline: Option(String),
    initramfs: Option(String),
    mounts: List(String),
  )
}

/// Defaults for `RunUnikraftOptions`: everything unset/empty (`path`
/// defaults to `"."` daemon-side when left `None`).
pub fn run_unikraft_options() -> RunUnikraftOptions {
  RunUnikraftOptions(
    path: None,
    cpus: None,
    mem: None,
    net: None,
    cmdline: None,
    initramfs: None,
    mounts: [],
  )
}

/// Boot a Unikraft unikernel, detached. Returns the new machine's id.
pub fn run_unikraft(
  client: Client,
  opts opts: RunUnikraftOptions,
) -> Result(String, Error) {
  let input =
    json.object([
      #("path", json.nullable(opts.path, json.string)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("net", net_json(opts.net)),
      #("cmdline", json.nullable(opts.cmdline, json.string)),
      #("initramfs", json.nullable(opts.initramfs, json.string)),
      #("mounts", json.array(opts.mounts, json.string)),
    ])
  run_mutation(
    client,
    "mutation($input: RunUnikraftInput!) { runUnikraft(input: $input) }",
    json.object([#("input", input)]),
    "runUnikraft",
  )
}

/// `RunSolo5Input` (`daemon/src/graphql.rs`). Solo5 (MirageOS) runs under
/// the `solo5-hvt` tender rather than libkrun; the unikernel declares its
/// own network and block devices in its `MFT1` manifest note, so only what
/// the host alone can know is carried: `block` backing files (`"NAME=FILE"`)
/// and the `args` handed to the unikernel itself. Always a single vCPU —
/// `cpus` above 1 is warned about and ignored. No disk, no agent.
pub type RunSolo5Options {
  RunSolo5Options(
    path: Option(String),
    cpus: Option(Int),
    mem: Option(Int),
    net: Option(NetOptions),
    block: List(String),
    args: List(String),
  )
}

/// Defaults for `RunSolo5Options`: everything unset/empty (`path` defaults
/// to `"."` daemon-side when left `None`).
pub fn run_solo5_options() -> RunSolo5Options {
  RunSolo5Options(
    path: None,
    cpus: None,
    mem: None,
    net: None,
    block: [],
    args: [],
  )
}

/// Boot a Solo5 (MirageOS) unikernel, detached. Returns the new machine's id.
pub fn run_solo5(
  client: Client,
  opts opts: RunSolo5Options,
) -> Result(String, Error) {
  let input =
    json.object([
      #("path", json.nullable(opts.path, json.string)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("net", net_json(opts.net)),
      #("block", json.array(opts.block, json.string)),
      #("args", json.array(opts.args, json.string)),
    ])
  run_mutation(
    client,
    "mutation($input: RunSolo5Input!) { runSolo5(input: $input) }",
    json.object([#("input", input)]),
    "runSolo5",
  )
}

/// `RunOsvInput` (`daemon/src/graphql.rs` ~line 467). Like Nanos, no agent —
/// but it does have a root filesystem, so unlike Unikraft it takes the disk
/// options.
pub type RunOsvOptions {
  RunOsvOptions(
    image: String,
    cpus: Option(Int),
    mem: Option(Int),
    net: Option(NetOptions),
    cmdline: Option(String),
    disk: Option(String),
    no_disk: Bool,
    attach_disk: List(String),
    gic: Option(String),
    persist: Bool,
    volume: Option(String),
  )
}

/// Defaults for `RunOsvOptions`: everything unset/empty/false except
/// `image`.
pub fn run_osv_options(image: String) -> RunOsvOptions {
  RunOsvOptions(
    image: image,
    cpus: None,
    mem: None,
    net: None,
    cmdline: None,
    disk: None,
    no_disk: False,
    attach_disk: [],
    gic: None,
    persist: False,
    volume: None,
  )
}

/// Boot an OSv unikernel, detached. Returns the new machine's id.
pub fn run_osv(
  client: Client,
  opts opts: RunOsvOptions,
) -> Result(String, Error) {
  let input =
    json.object([
      #("image", json.string(opts.image)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("net", net_json(opts.net)),
      #("cmdline", json.nullable(opts.cmdline, json.string)),
      #("disk", json.nullable(opts.disk, json.string)),
      #("noDisk", json.bool(opts.no_disk)),
      #("attachDisk", json.array(opts.attach_disk, json.string)),
      #("gic", json.nullable(opts.gic, json.string)),
      #("persist", json.bool(opts.persist)),
      #("volume", json.nullable(opts.volume, json.string)),
    ])
  run_mutation(
    client,
    "mutation($input: RunOsvInput!) { runOsv(input: $input) }",
    json.object([#("input", input)]),
    "runOsv",
  )
}

/// `RunFlavorInput` (`daemon/src/graphql.rs` ~line 506).
pub type RunFlavorOptions {
  RunFlavorOptions(
    name: String,
    cpus: Option(Int),
    mem: Option(Int),
    ports: List(String),
    volume: Option(String),
    repo: Option(String),
  )
}

/// Defaults for `RunFlavorOptions`: everything unset/empty except `name`.
pub fn run_flavor_options(name: String) -> RunFlavorOptions {
  RunFlavorOptions(
    name: name,
    cpus: None,
    mem: None,
    ports: [],
    volume: None,
    repo: None,
  )
}

/// Boot a saved flavor, detached. Returns the new machine's id.
pub fn run_flavor(
  client: Client,
  opts opts: RunFlavorOptions,
) -> Result(String, Error) {
  let input =
    json.object([
      #("name", json.string(opts.name)),
      #("cpus", json.nullable(opts.cpus, json.int)),
      #("mem", json.nullable(opts.mem, json.int)),
      #("ports", json.array(opts.ports, json.string)),
      #("volume", json.nullable(opts.volume, json.string)),
      #("repo", json.nullable(opts.repo, json.string)),
    ])
  run_mutation(
    client,
    "mutation($input: RunFlavorInput!) { runFlavor(input: $input) }",
    json.object([#("input", input)]),
    "runFlavor",
  )
}

// ---------------------------------------------------------------------------
// exec / shell — openShell + shellOutput + closeShell, per daemon/README.md
// ---------------------------------------------------------------------------

const shell_output_doc = "subscription($sessionId: String!) { shellOutput(sessionId: $sessionId) { dataBase64 exitCode } }"

fn open_shell(
  client: Client,
  id: String,
  command: Option(List(String)),
  env: List(String),
  rows: Int,
  cols: Int,
) -> Result(ShellSessionInfo, Error) {
  let doc =
    "mutation($machineId: String!, $command: [String!]!, $env: [String!]!, $rows: Int!, $cols: Int!) { openShell(machineId: $machineId, command: $command, env: $env, rows: $rows, cols: $cols) { id machineId finished truncated } }"
  let vars =
    json.object([
      #("machineId", json.string(id)),
      #("command", json.array(option.unwrap(command, []), json.string)),
      #("env", json.array(env, json.string)),
      #("rows", json.int(rows)),
      #("cols", json.int(cols)),
    ])
  use data <- result.try(query(client, doc, vars))
  case field_dynamic(data, "openShell") {
    Ok(row) -> types.shell_session_info_from_graphql(row)
    Error(Nil) -> Error(DecodeFailed("openShell", string.inspect(data)))
  }
}

fn close_shell(client: Client, session_id: String) -> Nil {
  let doc =
    "mutation($sessionId: String!) { closeShell(sessionId: $sessionId) }"
  let vars = json.object([#("sessionId", json.string(session_id))])
  // Idempotent and best-effort per daemon/README.md: called regardless of
  // how the caller's wait ended, so its own failure is not surfaced.
  let _ = query(client, doc, vars)
  Nil
}

/// Decode one `shellOutput`/`machineLogs` event object (`{ dataBase64
/// exitCode }`) into a `ShellEvent`. Exactly one of the two GraphQL fields
/// is set per the daemon's own contract.
fn shell_event_from_data(data: Dynamic) -> ShellEvent {
  case
    decode.run(
      data,
      decode.field("exitCode", decode.optional(decode.int), decode.success),
    )
  {
    Ok(Some(code)) -> ShellExit(code)
    _ ->
      case field_string(data, "dataBase64") {
        Ok(b64) -> ShellData(types.decode_base64_chunk(b64))
        Error(Nil) -> ShellData(<<>>)
      }
  }
}

/// Run `erl_spawn`: `erlang:spawn/1`, called directly — Gleam functions
/// compile to Erlang funs, so a zero-argument Gleam closure is already
/// exactly what `spawn/1` wants. No custom Erlang glue needed for this one.
@external(erlang, "erlang", "spawn")
fn erl_spawn(run: fn() -> Nil) -> subject.Pid

/// Subscribe to `doc`/`variables` (a `shellOutput`/`machineLogs`-shaped
/// subscription) and translate its raw WS events into `ShellEvent`s
/// delivered to a freshly returned `Subject`, via one small forwarder
/// process — see the module doc's design note on why `shell`/`follow_logs`
/// are non-blocking while `exec` is not.
fn start_shell_events(
  client: Client,
  doc: String,
  variables: Json,
) -> Result(Subject(ShellEvent), Error) {
  use conn <- result.try(ws.ensure(ws.derive_url(client.url), client.token))
  let sub_id = fresh_id()
  let variables_json = json.to_string(variables)
  let out = subject.new()
  erl_spawn(fn() {
    // `raw` must be created *inside* the spawned process: a `Subject`
    // addresses whichever process called `subject.new()`, and this is the
    // process that must receive the WS actor's events, not the caller of
    // `start_shell_events`.
    let raw = subject.new()
    ws.subscribe(conn, sub_id, doc, variables_json, raw)
    forward_shell_events(raw, out)
  })
  Ok(out)
}

fn forward_shell_events(
  raw: Subject(ws.RawEvent),
  out: Subject(ShellEvent),
) -> Nil {
  case subject.receive(raw, -1) {
    Ok(ws.RawNext(data)) -> {
      let event = shell_event_from_data(data)
      subject.send(out, event)
      case event {
        ShellExit(_) -> Nil
        _ -> forward_shell_events(raw, out)
      }
    }
    Ok(ws.RawError(message)) -> subject.send(out, ShellError(message))
    Ok(ws.RawAuthError(message)) -> subject.send(out, ShellError(message))
    Ok(ws.RawComplete) -> subject.send(out, ShellClosed)
    Error(Nil) ->
      subject.send(out, ShellError("the shell subscription ended unexpectedly"))
  }
}

/// One-shot command execution: `openShell` (with `command`, so it runs that
/// instead of a login shell) + `shellOutput` + `closeShell`, exactly the
/// sequence `daemon/README.md`'s "Interactive shells over GraphQL" section
/// describes (open, *then* subscribe, *then* wait for exit — no input to
/// send). Blocks the calling process until the command exits.
pub fn exec(
  client: Client,
  id id: String,
  command command: List(String),
  env env: List(String),
) -> Result(ExecResult, Error) {
  use session <- result.try(open_shell(client, id, Some(command), env, 24, 80))
  use conn <- result.try(ws.ensure(ws.derive_url(client.url), client.token))

  let sub_id = fresh_id()
  let raw = subject.new()
  let vars = json.object([#("sessionId", json.string(session.id))])
  ws.subscribe(conn, sub_id, shell_output_doc, json.to_string(vars), raw)

  let result = collect_exec_output(raw, <<>>)
  close_shell(client, session.id)
  result
}

/// A generous but bounded wait per message: `exec` blocks the caller, so
/// unlike the background forwarders it should eventually give control back
/// even if the daemon stops responding entirely.
const exec_receive_timeout_ms = 600_000

fn collect_exec_output(
  raw: Subject(ws.RawEvent),
  acc: BitArray,
) -> Result(ExecResult, Error) {
  case subject.receive(raw, exec_receive_timeout_ms) {
    Ok(ws.RawNext(data)) ->
      case shell_event_from_data(data) {
        ShellExit(code) -> Ok(ExecResult(exit_code: code, output: acc))
        ShellData(bytes) ->
          collect_exec_output(raw, bit_array.append(acc, bytes))
        _ -> collect_exec_output(raw, acc)
      }
    Ok(ws.RawError(message)) -> Error(GraphqlError(message, None))
    Ok(ws.RawAuthError(message)) -> Error(AuthError(message))
    Ok(ws.RawComplete) -> Ok(ExecResult(exit_code: 0, output: acc))
    Error(Nil) ->
      Error(GraphqlError("timed out waiting for the command to finish", None))
  }
}

/// A live, interactive shell session opened with `shell`. Output streams to
/// `shell_output`'s `Subject`; `shell_send`/`shell_resize`/`shell_close`
/// drive it — these four function names (there is no single "shell handle"
/// API in the daemon's schema to mirror 1:1) are this SDK's own choice of
/// shape for the "session ID + a way to write/resize/close it" the design
/// note asked for.
pub opaque type ShellSession {
  ShellSession(client: Client, id: String, output: Subject(ShellEvent))
}

/// Open an interactive shell (or, with `command`, run that command with a
/// live, writable session rather than blocking — the non-blocking sibling
/// of `exec`). Output arrives on `shell_output(session)` as it's produced;
/// this function itself returns as soon as the session and its subscription
/// are set up.
pub fn shell(
  client: Client,
  id id: String,
  command command: Option(List(String)),
  env env: List(String),
  rows rows: Int,
  cols cols: Int,
) -> Result(ShellSession, Error) {
  use session <- result.try(open_shell(client, id, command, env, rows, cols))
  use conn <- result.try(ws.ensure(ws.derive_url(client.url), client.token))

  let sub_id = fresh_id()
  let vars = json.object([#("sessionId", json.string(session.id))])
  let variables_json = json.to_string(vars)
  let out = subject.new()
  erl_spawn(fn() {
    let raw = subject.new()
    ws.subscribe(conn, sub_id, shell_output_doc, variables_json, raw)
    forward_shell_events(raw, out)
  })

  Ok(ShellSession(client: client, id: session.id, output: out))
}

/// This session's live output `Subject` — `subject.receive` it in a loop.
pub fn shell_output(session: ShellSession) -> Subject(ShellEvent) {
  session.output
}

/// The daemon-side session id, if you need it for `client.request`.
pub fn shell_id(session: ShellSession) -> String {
  session.id
}

/// Send keystrokes/input to the session.
pub fn shell_send(session: ShellSession, data: BitArray) -> Result(Nil, Error) {
  let doc =
    "mutation($sessionId: String!, $dataBase64: String!) { sendShellInput(sessionId: $sessionId, dataBase64: $dataBase64) }"
  let vars =
    json.object([
      #("sessionId", json.string(session.id)),
      #("dataBase64", json.string(bit_array.base64_encode(data, True))),
    ])
  use _ <- result.try(query(session.client, doc, vars))
  Ok(Nil)
}

/// Apply a terminal resize, so full-screen programs in the guest redraw.
pub fn shell_resize(
  session: ShellSession,
  rows rows: Int,
  cols cols: Int,
) -> Result(Nil, Error) {
  let doc =
    "mutation($sessionId: String!, $rows: Int!, $cols: Int!) { resizeShell(sessionId: $sessionId, rows: $rows, cols: $cols) }"
  let vars =
    json.object([
      #("sessionId", json.string(session.id)),
      #("rows", json.int(rows)),
      #("cols", json.int(cols)),
    ])
  use _ <- result.try(query(session.client, doc, vars))
  Ok(Nil)
}

/// Close the session and kill its command. Idempotent.
pub fn shell_close(session: ShellSession) -> Nil {
  close_shell(session.client, session.id)
}

// ---------------------------------------------------------------------------
// escape hatch
// ---------------------------------------------------------------------------

@external(erlang, "bsdkrun_remote_ffi", "dynamic_to_json")
fn ffi_dynamic_to_json(variables: Dynamic) -> String

/// Run any query or mutation `bsdkrun/client`'s typed API does not cover.
/// `variables` is a `Dynamic` — build one from a `Dict`/`List`/literal via
/// `gleam/dynamic.from`; see `bsdkrun_remote_ffi.erl`'s `dynamic_to_json/1`
/// doc comment for exactly which shapes it understands. Returns the `data`
/// field, for the caller to decode the same way `bsdkrun/types`'s decoders
/// do (`gleam/dynamic/decode`).
pub fn request(
  client: Client,
  query query: String,
  variables variables: Dynamic,
) -> Result(Dynamic, Error) {
  graphql_transport.execute_raw(
    client.url,
    client.token,
    query,
    ffi_dynamic_to_json(variables),
  )
}

/// Subscribe to any subscription `bsdkrun/client`'s typed API does not
/// cover. Each `next` payload's `data` arrives as `SubNext(Dynamic)` on the
/// returned `Subject`, terminated by `SubError`/`SubComplete` — decode it
/// the same way `request`'s result.
pub fn subscribe(
  client: Client,
  query query: String,
  variables variables: Dynamic,
) -> Result(Subject(SubscriptionEvent), Error) {
  use conn <- result.try(ws.ensure(ws.derive_url(client.url), client.token))
  let sub_id = fresh_id()
  let variables_json = ffi_dynamic_to_json(variables)
  let out = subject.new()
  erl_spawn(fn() {
    let raw = subject.new()
    ws.subscribe(conn, sub_id, query, variables_json, raw)
    forward_subscription_events(raw, out)
  })
  Ok(out)
}

fn forward_subscription_events(
  raw: Subject(ws.RawEvent),
  out: Subject(SubscriptionEvent),
) -> Nil {
  case subject.receive(raw, -1) {
    Ok(ws.RawNext(data)) -> {
      subject.send(out, SubNext(data))
      forward_subscription_events(raw, out)
    }
    Ok(ws.RawError(message)) -> subject.send(out, SubError(message))
    Ok(ws.RawAuthError(message)) -> subject.send(out, SubError(message))
    Ok(ws.RawComplete) -> subject.send(out, SubComplete)
    Error(Nil) ->
      subject.send(out, SubError("the subscription ended unexpectedly"))
  }
}

@external(erlang, "bsdkrun_remote_ffi", "new_tag")
fn fresh_id() -> String
