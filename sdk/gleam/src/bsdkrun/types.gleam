//// Typed records mirroring `bsdkrun`'s `--json` output, their decoders, and
//// the result of running a command inside a guest.

import bsdkrun/error.{type Error, DecodeFailed}
import gleam/dynamic/decode.{type Decoder}
import gleam/float
import gleam/int
import gleam/json
import gleam/list
import gleam/option.{type Option}
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

/// Decode a `--json` list payload. Blank output — which the CLI emits when
/// there is nothing to list — decodes as the empty list.
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
