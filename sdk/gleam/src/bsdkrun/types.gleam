//// Typed records mirroring `bsdkrun`'s `--json` output, their decoders, and
//// the result of running a command inside a guest.
////
//// The same records also back `bsdkrun/client` (the remote GraphQL client):
//// `sandbox_info_from_graphql` and `command_result_from_graphql` decode the
//// daemon's camelCase GraphQL responses into these exact same
//// `SandboxInfo` / `CommandResult` types, so code written against the local,
//// CLI-shelling API and code written against a remote daemon see identical
//// shapes.

import bsdkrun/error.{type Error, DecodeFailed}
import gleam/bit_array
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode.{type Decoder}
import gleam/float
import gleam/int
import gleam/json
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result
import gleam/string

/// A host->guest TCP port forward, as reported by `bsdkrun ps --json`.
pub type PortForward {
  PortForward(bind: String, host: Int, guest: Int)
}

/// A machine, as reported by `bsdkrun ps --json`.
pub type SandboxInfo {
  SandboxInfo(
    id: String,
    name: Option(String),
    image: String,
    kind: String,
    command: String,
    running: Bool,
    exit_code: Option(Int),
    pid: Option(Int),
    detached: Bool,
    cpus: Int,
    mem: Int,
    volume: Option(String),
    state_dir: String,
    network: Option(String),
    net_ip: Option(String),
    created_at: Int,
    finished_at: Option(Int),
    ports: List(PortForward),
    /// The snapshot this machine was branched from, if any.
    origin: Option(String),
  )
}

/// A coding agent bsdkrun can sandbox.
///
/// Each runs in a disposable microVM with a persistent login, a shared skills
/// store, and only the folder you choose to share.
pub type AiAgent {
  AiAgent(
    id: String,
    label: String,
    /// The catalog flavor that installs it.
    flavor: String,
    description: String,
    /// Its flavor is provisioned, so a sandbox boots in a second.
    installed: Bool,
    running: Int,
  )
}

/// One agent sandbox. It is a machine, so `logs`/`stop` work on `id`.
pub type AiSession {
  AiSession(
    id: String,
    name: String,
    agent: String,
    running: Bool,
    /// The directory shared into it, on the engine's host.
    workspace: Option(String),
    created_at: Int,
  )
}

/// The Docker engine VM: whether it is up, and how to reach it.
///
/// bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
/// socket, so the host's own `docker` CLI drives the same engine.
pub type DockerStatus {
  DockerStatus(
    running: Bool,
    machine_id: Option(String),
    machine_running: Bool,
    /// The unix socket the `docker` CLI talks to.
    socket: String,
    socket_ready: Bool,
    api_port: Option(Int),
    version: Option(String),
    containers: Option(Int),
    images: Option(Int),
    /// Host directories shared into the VM, each `HOST:GUEST`.
    mounts: List(String),
    /// The dedicated image-store disk, when the VM has one.
    disk: Option(String),
    /// Its size in bytes — sparse, so the cap rather than the usage.
    disk_size: Option(Int),
  )
}

/// A container in the Docker engine VM — a trimmed `docker ps` row.
pub type DockerContainer {
  DockerContainer(
    id: String,
    name: String,
    image: String,
    command: String,
    /// `"running"`, `"exited"`, `"created"`, `"paused"`, …
    state: String,
    /// Docker's human status, e.g. `"Up 3 minutes"`.
    status: String,
    /// Published forwards, each `HOST:GUEST/proto`.
    ports: List(String),
    /// Unix epoch seconds.
    created: Int,
  )
}

/// Whether a container is up.
pub fn container_running(c: DockerContainer) -> Bool {
  c.state == "running"
}

/// A machine snapshot: one machine's disk state, captured under a name.
///
/// A copy-on-write clone rather than a memory image — the files the guest
/// wrote, not what it was executing. `client.branch` boots a new machine from
/// one; `client.restore` puts one back over the machine it came from.
pub type SnapshotInfo {
  SnapshotInfo(
    id: String,
    name: String,
    machine_id: String,
    /// The machine's name when it was taken; empty if it had none.
    machine_name: String,
    /// `"linux"`, `"freebsd"`, `"netbsd"` or `"unikraft"`.
    kind: String,
    image: String,
    path: String,
    /// The snapshot the source machine was itself branched from, if any.
    parent: Option(String),
    description: String,
    cpus: Int,
    mem: Int,
    ports: List(PortForward),
    /// Human-readable, when measured — a CoW clone costs nothing to take.
    size: Option(String),
    created_at: Int,
  )
}

/// `"running"` or `"exited"` — the status column `bsdkrun ps` prints.
pub fn status(info: SandboxInfo) -> String {
  case info.running {
    True -> "running"
    False -> "exited"
  }
}

/// An image, as reported by `bsdkrun images --json`.
pub type ImageInfo {
  ImageInfo(
    id: String,
    reference: String,
    digest: String,
    size: Int,
    rootfs: String,
    created_at: Int,
  )
}

/// A persistent volume, as reported by `bsdkrun volume ls --json`.
pub type VolumeInfo {
  VolumeInfo(
    name: String,
    guest: Option(String),
    base: Option(String),
    path: String,
    size: String,
    created_at: Option(Int),
    tracked: Bool,
  )
}

/// A stored cache entry, as reported by `bsdkrun cache ls --json`.
pub type CacheEntry {
  CacheEntry(
    key: String,
    path: String,
    compression: String,
    size: Int,
    created: Int,
    digest: String,
  )
}

/// What a `bsdkrun cache restore --json` did. A miss is not an error — check
/// `restored`.
pub type RestoreResult {
  RestoreResult(
    restored: Bool,
    requested_key: String,
    key: Option(String),
    path: Option(String),
    size: Option(Int),
    compression: Option(String),
    created: Option(Int),
  )
}

/// A global network, as reported by `bsdkrun network ls --json`.
pub type NetworkInfo {
  NetworkInfo(
    name: String,
    subnet: String,
    gateway: String,
    members: Int,
    running: Int,
    up: Bool,
    created_at: Option(Int),
  )
}

/// The captured result of running a command in a guest.
pub type CommandResult {
  CommandResult(stdout: String, stderr: String, exit_code: Int, command: String)
}

/// Whether the command succeeded (exit 0).
pub fn is_ok(res: CommandResult) -> Bool {
  res.exit_code == 0
}

/// stdout with trailing newlines trimmed — the common case.
pub fn text(res: CommandResult) -> String {
  string.trim_end(res.stdout)
}

/// Non-empty stdout lines.
pub fn lines(res: CommandResult) -> List(String) {
  res.stdout
  |> string.split("\n")
  |> list.filter(fn(line) { line != "" })
}

// --- remote-client-only types -----------------------------------------------
//
// These back `bsdkrun/client`, the GraphQL client for a remote `bsdkrund`.
// They have no local-CLI equivalent to reuse (unlike `SandboxInfo` and
// `CommandResult` above), because they describe daemon-only concepts: a
// base64-framed exec result, and a shell session's identity as the daemon
// reports it.

/// The captured result of `client.exec` — a one-shot command run through
/// `openShell` + `shellOutput` + `closeShell` (see `bsdkrun/client`).
///
/// Unlike the local `CommandResult`, output is a single interleaved
/// `BitArray` rather than separate stdout/stderr: the daemon's shell
/// protocol is a pty, which does not keep the streams apart.
pub type ExecResult {
  ExecResult(exit_code: Int, output: BitArray)
}

/// A shell session, as reported by the daemon's `openShell` mutation /
/// `shellSessions` query.
pub type ShellSessionInfo {
  ShellSessionInfo(
    id: String,
    machine_id: String,
    finished: Bool,
    truncated: Bool,
  )
}

/// One event from a live `shellOutput` or `machineLogs` subscription, as
/// delivered to a `bsdkrun/subject.Subject` by `bsdkrun/client`.
pub type ShellEvent {
  /// A chunk of output, already base64-decoded.
  ShellData(BitArray)
  /// The session's command exited. Terminal — no further events follow.
  ShellExit(Int)
  /// The subscription itself failed (a GraphQL `error` message, or the
  /// socket closing). Terminal.
  ShellError(String)
  /// The subscription ended with no more data (a GraphQL `complete`, or the
  /// caller unsubscribed). Terminal.
  ShellClosed
}

/// One event from `client.subscribe`, the generic subscription escape hatch.
pub type SubscriptionEvent {
  /// One `next` payload's `data`, exactly as the operation's document shapes
  /// it — decode it the same way you would decode `client.request`'s result.
  SubNext(Dynamic)
  /// A GraphQL `error` message (or the socket closing). Terminal.
  SubError(String)
  /// A GraphQL `complete`. Terminal.
  SubComplete
}

// --- decoders ---------------------------------------------------------------

/// `bsdkrun` writes numeric columns as JSON numbers, but a few (`created_at`
/// timestamps in particular) can come back as strings. Accept either.
fn lenient_int() -> Decoder(Int) {
  decode.one_of(decode.int, [
    decode.string |> decode.map(fn(s) { int.parse(s) |> result.unwrap(0) }),
    decode.float |> decode.map(float.truncate),
  ])
}

/// A field that may be absent or `null`.
fn optional_field(
  name: String,
  inner: Decoder(a),
  next: fn(Option(a)) -> Decoder(b),
) -> Decoder(b) {
  decode.optional_field(name, option.None, decode.optional(inner), next)
}

/// A field that may be absent, falling back to `default`.
fn field_or(
  name: String,
  default: a,
  inner: Decoder(a),
  next: fn(a) -> Decoder(b),
) -> Decoder(b) {
  decode.optional_field(name, default, inner, next)
}

/// Decoder for one `ports` entry of a `ps --json` row.
pub fn port_forward_decoder() -> Decoder(PortForward) {
  use bind <- field_or("bind", "", decode.string)
  use host <- field_or("host", 0, lenient_int())
  use guest <- field_or("guest", 0, lenient_int())

  decode.success(PortForward(bind:, host:, guest:))
}

/// Decoder for one `cache ls --json` row, and for `cache save --json`.
pub fn cache_entry_decoder() -> Decoder(CacheEntry) {
  use key <- field_or("key", "", decode.string)
  use path <- field_or("path", "", decode.string)
  use compression <- field_or("compression", "", decode.string)
  use size <- field_or("size", 0, lenient_int())
  use created <- field_or("created", 0, lenient_int())
  use digest <- field_or("digest", "", decode.string)

  decode.success(CacheEntry(key:, path:, compression:, size:, created:, digest:))
}

/// Decoder for `cache restore --json`.
pub fn restore_result_decoder() -> Decoder(RestoreResult) {
  use restored <- field_or("restored", False, decode.bool)
  use requested_key <- field_or("requested_key", "", decode.string)
  use key <- optional_field("key", decode.string)
  use path <- optional_field("path", decode.string)
  use size <- optional_field("size", lenient_int())
  use compression <- optional_field("compression", decode.string)
  use created <- optional_field("created", lenient_int())

  decode.success(RestoreResult(
    restored:,
    requested_key:,
    key:,
    path:,
    size:,
    compression:,
    created:,
  ))
}

/// Decoder for one `ps --json` row.
pub fn sandbox_info_decoder() -> Decoder(SandboxInfo) {
  use id <- field_or("id", "", decode.string)
  use name <- optional_field("name", decode.string)
  use image <- field_or("image", "", decode.string)
  use kind <- field_or("kind", "", decode.string)
  use command <- field_or("command", "", decode.string)
  use running <- field_or("running", False, decode.bool)
  use exit_code <- optional_field("exit_code", lenient_int())
  use pid <- optional_field("pid", lenient_int())
  use detached <- field_or("detached", False, decode.bool)
  use cpus <- field_or("cpus", 0, lenient_int())
  use mem <- field_or("mem", 0, lenient_int())
  use volume <- optional_field("volume", decode.string)
  use state_dir <- field_or("state_dir", "", decode.string)
  use network <- optional_field("network", decode.string)
  use net_ip <- optional_field("net_ip", decode.string)
  use created_at <- field_or("created_at", 0, lenient_int())
  use finished_at <- optional_field("finished_at", lenient_int())
  use origin <- optional_field("origin", decode.string)
  use ports <- field_or("ports", [], decode.list(port_forward_decoder()))

  decode.success(SandboxInfo(
    id:,
    name:,
    image:,
    kind:,
    command:,
    running:,
    exit_code:,
    pid:,
    detached:,
    cpus:,
    mem:,
    volume:,
    state_dir:,
    network:,
    net_ip:,
    created_at:,
    finished_at:,
    ports:,
    origin:,
  ))
}

/// Decoder for one `images --json` row.
pub fn image_info_decoder() -> Decoder(ImageInfo) {
  use id <- field_or("id", "", decode.string)
  use reference <- field_or("reference", "", decode.string)
  use digest <- field_or("digest", "", decode.string)
  use size <- field_or("size", 0, lenient_int())
  use rootfs <- field_or("rootfs", "", decode.string)
  use created_at <- field_or("created_at", 0, lenient_int())

  decode.success(ImageInfo(
    id:,
    reference:,
    digest:,
    size:,
    rootfs:,
    created_at:,
  ))
}

/// Decoder for one `volume ls --json` row.
pub fn volume_info_decoder() -> Decoder(VolumeInfo) {
  use name <- field_or("name", "", decode.string)
  use guest <- optional_field("guest", decode.string)
  use base <- optional_field("base", decode.string)
  use path <- field_or("path", "", decode.string)
  use size <- field_or("size", "", decode.string)
  use created_at <- optional_field("created_at", lenient_int())
  use tracked <- field_or("tracked", False, decode.bool)

  decode.success(VolumeInfo(
    name:,
    guest:,
    base:,
    path:,
    size:,
    created_at:,
    tracked:,
  ))
}

/// Decoder for one `network ls --json` row.
pub fn network_info_decoder() -> Decoder(NetworkInfo) {
  use name <- field_or("name", "", decode.string)
  use subnet <- field_or("subnet", "", decode.string)
  use gateway <- field_or("gateway", "", decode.string)
  use members <- field_or("members", 0, lenient_int())
  use running <- field_or("running", 0, lenient_int())
  use up <- field_or("up", False, decode.bool)
  use created_at <- optional_field("created_at", lenient_int())

  decode.success(NetworkInfo(
    name:,
    subnet:,
    gateway:,
    members:,
    running:,
    up:,
    created_at:,
  ))
}

// --- GraphQL decoders (bsdkrun/client) ---------------------------------------
//
// The daemon's GraphQL schema is camelCase, and several fields that are
// unconditionally present (with a CLI default) in `--json` output are
// instead `Option`s of their GraphQL type — the `Machine` object can and does
// send `"cpus": null` for a field the local decoders above never see absent
// *or* null. `field_or` (unlike `optional_field`) runs its inner decoder
// straight over a present-but-null value and fails, so every field below
// that GraphQL types as nullable goes through `optional_field` and is
// unwrapped afterwards, even where `SandboxInfo` itself wants a bare value.

/// A field that decodes through `decode.run` on its own, for use outside a
/// `use`-chain decoder — needed once for `payload.data` (kept in
/// `bsdkrun/ws`), and useful here for one-off top-level decodes.
fn decode_or(
  dyn: Dynamic,
  decoder: Decoder(a),
  label: String,
) -> Result(a, Error) {
  case decode.run(dyn, decoder) {
    Ok(value) -> Ok(value)
    Error(_) -> Error(DecodeFailed(label, string.inspect(dyn)))
  }
}

/// Decoder for one GraphQL `Machine` object (the `MACHINE_FIELDS` selection:
/// `id name image kind command status running exitCode pid detached cpus mem
/// volume stateDir createdAt finishedAt network netIp ports{bind host
/// guest}`) into the same `SandboxInfo` the local `ps --json` decoder
/// produces. `status`/`stateDir` GraphQL fields not covered above: `status`
/// is redundant with `running` (this SDK derives it via `types.status`) and
/// is not decoded here.
fn sandbox_info_from_graphql_decoder() -> Decoder(SandboxInfo) {
  use id <- field_or("id", "", decode.string)
  use name <- optional_field("name", decode.string)
  use image <- field_or("image", "", decode.string)
  use kind <- field_or("kind", "", decode.string)
  use command <- field_or("command", "", decode.string)
  use running <- field_or("running", False, decode.bool)
  use exit_code <- optional_field("exitCode", lenient_int())
  use pid <- optional_field("pid", lenient_int())
  use detached <- field_or("detached", False, decode.bool)
  use cpus <- optional_field("cpus", lenient_int())
  use mem <- optional_field("mem", lenient_int())
  use volume <- optional_field("volume", decode.string)
  use state_dir <- optional_field("stateDir", decode.string)
  use network <- optional_field("network", decode.string)
  use net_ip <- optional_field("netIp", decode.string)
  use created_at <- optional_field("createdAt", lenient_int())
  use finished_at <- optional_field("finishedAt", lenient_int())
  use origin <- optional_field("origin", decode.string)
  use ports <- field_or("ports", [], decode.list(port_forward_decoder()))

  decode.success(SandboxInfo(
    id:,
    name:,
    image:,
    kind:,
    command:,
    running:,
    exit_code:,
    pid:,
    detached:,
    cpus: option.unwrap(cpus, 0),
    mem: option.unwrap(mem, 0),
    volume:,
    state_dir: option.unwrap(state_dir, ""),
    network:,
    net_ip:,
    created_at: option.unwrap(created_at, 0),
    finished_at:,
    ports:,
    origin:,
  ))
}

fn ai_agent_decoder() -> Decoder(AiAgent) {
  use id <- field_or("id", "", decode.string)
  use label <- field_or("label", "", decode.string)
  use flavor <- field_or("flavor", "", decode.string)
  use description <- field_or("description", "", decode.string)
  use installed <- field_or("installed", False, decode.bool)
  use running <- optional_field("running", lenient_int())

  decode.success(AiAgent(
    id:,
    label:,
    flavor:,
    description:,
    installed:,
    running: option.unwrap(running, 0),
  ))
}

/// Decode a GraphQL `AiAgent` object.
pub fn ai_agent_from_graphql(dyn: Dynamic) -> Result(AiAgent, Error) {
  decode_or(dyn, ai_agent_decoder(), "aiAgent")
}

fn ai_session_decoder() -> Decoder(AiSession) {
  use id <- field_or("id", "", decode.string)
  use name <- field_or("name", "", decode.string)
  use agent <- field_or("agent", "", decode.string)
  use running <- field_or("running", False, decode.bool)
  use workspace <- optional_field("workspace", decode.string)
  use created_at <- optional_field("createdAt", lenient_int())

  decode.success(AiSession(
    id:,
    name:,
    agent:,
    running:,
    workspace:,
    created_at: option.unwrap(created_at, 0),
  ))
}

/// Decode a GraphQL `AiSession` object.
pub fn ai_session_from_graphql(dyn: Dynamic) -> Result(AiSession, Error) {
  decode_or(dyn, ai_session_decoder(), "aiSession")
}

fn docker_status_decoder() -> Decoder(DockerStatus) {
  use running <- field_or("running", False, decode.bool)
  use machine_id <- optional_field("machineId", decode.string)
  use machine_running <- field_or("machineRunning", False, decode.bool)
  use socket <- field_or("socket", "", decode.string)
  use socket_ready <- field_or("socketReady", False, decode.bool)
  use api_port <- optional_field("apiPort", lenient_int())
  use version <- optional_field("version", decode.string)
  use containers <- optional_field("containers", lenient_int())
  use images <- optional_field("images", lenient_int())
  use mounts <- field_or("mounts", [], decode.list(decode.string))
  use disk <- optional_field("disk", decode.string)
  use disk_size <- optional_field("diskSize", lenient_int())

  decode.success(DockerStatus(
    running:,
    machine_id:,
    machine_running:,
    socket:,
    socket_ready:,
    api_port:,
    version:,
    containers:,
    images:,
    mounts:,
    disk:,
    disk_size:,
  ))
}

/// Decode a GraphQL `DockerStatus` object.
pub fn docker_status_from_graphql(dyn: Dynamic) -> Result(DockerStatus, Error) {
  decode_or(dyn, docker_status_decoder(), "dockerStatus")
}

fn docker_container_decoder() -> Decoder(DockerContainer) {
  use id <- field_or("id", "", decode.string)
  use name <- field_or("name", "", decode.string)
  use image <- field_or("image", "", decode.string)
  use command <- field_or("command", "", decode.string)
  use state <- field_or("state", "", decode.string)
  use status <- field_or("status", "", decode.string)
  use ports <- field_or("ports", [], decode.list(decode.string))
  use created <- optional_field("created", lenient_int())

  decode.success(DockerContainer(
    id:,
    name:,
    image:,
    command:,
    state:,
    status:,
    ports:,
    created: option.unwrap(created, 0),
  ))
}

/// Decode a GraphQL `DockerContainer` object.
pub fn docker_container_from_graphql(
  dyn: Dynamic,
) -> Result(DockerContainer, Error) {
  decode_or(dyn, docker_container_decoder(), "dockerContainer")
}

/// Decoder for one GraphQL `Snapshot` object (the `SNAPSHOT_FIELDS`
/// selection) into a `SnapshotInfo`.
fn snapshot_info_from_graphql_decoder() -> Decoder(SnapshotInfo) {
  use id <- field_or("id", "", decode.string)
  use name <- field_or("name", "", decode.string)
  use machine_id <- field_or("machineId", "", decode.string)
  use machine_name <- field_or("machineName", "", decode.string)
  use kind <- field_or("kind", "", decode.string)
  use image <- field_or("image", "", decode.string)
  use path <- field_or("path", "", decode.string)
  use parent <- optional_field("parent", decode.string)
  use description <- field_or("description", "", decode.string)
  use cpus <- optional_field("cpus", lenient_int())
  use mem <- optional_field("mem", lenient_int())
  use ports <- field_or("ports", [], decode.list(port_forward_decoder()))
  use size <- optional_field("size", decode.string)
  use created_at <- optional_field("createdAt", lenient_int())

  decode.success(SnapshotInfo(
    id:,
    name:,
    machine_id:,
    machine_name:,
    kind:,
    image:,
    path:,
    parent:,
    description:,
    cpus: option.unwrap(cpus, 0),
    mem: option.unwrap(mem, 0),
    ports:,
    size:,
    created_at: option.unwrap(created_at, 0),
  ))
}

/// Decode a GraphQL `Snapshot` object into a `SnapshotInfo`.
pub fn snapshot_info_from_graphql(dyn: Dynamic) -> Result(SnapshotInfo, Error) {
  decode_or(dyn, snapshot_info_from_graphql_decoder(), "snapshot")
}

/// Decode a GraphQL `Machine` object (the `machine`/`machines` query result,
/// or `data.machine` from a raw `client.request` call) into a `SandboxInfo`.
pub fn sandbox_info_from_graphql(dyn: Dynamic) -> Result(SandboxInfo, Error) {
  decode_or(dyn, sandbox_info_from_graphql_decoder(), "machine")
}

/// Decode a GraphQL `CommandResult` object (`{ exitCode stdout stderr }`,
/// what every lifecycle mutation returns) into the local `CommandResult`
/// type. GraphQL's `CommandResult` has no `command` field — the mutation
/// name is supplied by the caller (`bsdkrun/client`) so error messages still
/// name the operation that failed, exactly as the local CLI path does.
pub fn command_result_from_graphql(
  dyn: Dynamic,
  label: String,
) -> Result(CommandResult, Error) {
  let decoder = {
    use exit_code <- field_or("exitCode", 0, lenient_int())
    use stdout <- field_or("stdout", "", decode.string)
    use stderr <- field_or("stderr", "", decode.string)
    decode.success(CommandResult(stdout:, stderr:, exit_code:, command: label))
  }
  decode_or(dyn, decoder, label)
}

/// Decode a GraphQL `ShellSessionInfo` object (`openShell`'s result, or a row
/// of `shellSessions`).
pub fn shell_session_info_from_graphql(
  dyn: Dynamic,
) -> Result(ShellSessionInfo, Error) {
  let decoder = {
    use id <- field_or("id", "", decode.string)
    use machine_id <- field_or("machineId", "", decode.string)
    use finished <- field_or("finished", False, decode.bool)
    use truncated <- field_or("truncated", False, decode.bool)
    decode.success(ShellSessionInfo(id:, machine_id:, finished:, truncated:))
  }
  decode_or(dyn, decoder, "openShell")
}

/// Base64-decode one `shellOutput`/`machineLogs` chunk's `dataBase64` field.
/// Invalid base64 (should not happen — the daemon only ever sends what it
/// itself encoded) decodes as empty, so a display glitch never becomes a
/// crash.
pub fn decode_base64_chunk(data_base64: String) -> BitArray {
  case bit_array.base64_decode(data_base64) {
    Ok(bits) -> bits
    Error(Nil) -> <<>>
  }
}

/// Decode a `decode.optional_field(name, option.None, decode.optional(inner),
/// next)` shaped field returning a `String`. A convenience for the few call
/// sites outside this module (`bsdkrun/client`) that need one field decoded
/// out of a `Dynamic` without building a full record decoder.
pub fn optional_string_field(dyn: Dynamic, name: String) -> Option(String) {
  let decoder = optional_field(name, decode.string, decode.success)
  case decode.run(dyn, decoder) {
    Ok(value) -> value
    Error(_) -> None
  }
}

/// Decode a `Dynamic`'s `Int` field by name, defaulting to `default` when the
/// field is absent, null, or the wrong shape.
pub fn int_field(dyn: Dynamic, name: String, default: Int) -> Int {
  let decoder = optional_field(name, lenient_int(), decode.success)
  case decode.run(dyn, decoder) {
    Ok(Some(value)) -> value
    _ -> default
  }
}

/// Decode a `Dynamic`'s `String` field by name, defaulting to `default` when
/// the field is absent, null, or the wrong shape.
pub fn string_field(dyn: Dynamic, name: String, default: String) -> String {
  let decoder = optional_field(name, decode.string, decode.success)
  case decode.run(dyn, decoder) {
    Ok(Some(value)) -> value
    _ -> default
  }
}

/// Decode a `--json` list payload. Blank output — which the CLI emits when
/// there is nothing to list — decodes as the empty list.
/// Decode a single JSON object, as `decode_rows` does for a list.
pub fn decode_one(
  raw: String,
  label: String,
  row: Decoder(a),
) -> Result(a, Error) {
  let payload = case string.trim(raw) {
    "" -> "{}"
    trimmed -> trimmed
  }

  case json.parse(payload, row) {
    Ok(value) -> Ok(value)
    Error(_) -> Error(DecodeFailed(label, raw))
  }
}

pub fn decode_rows(
  raw: String,
  label: String,
  row: Decoder(a),
) -> Result(List(a), Error) {
  let payload = case string.trim(raw) {
    "" -> "[]"
    trimmed -> trimmed
  }

  case json.parse(payload, decode.list(row)) {
    Ok(rows) -> Ok(rows)
    Error(_) -> Error(DecodeFailed(label, raw))
  }
}
