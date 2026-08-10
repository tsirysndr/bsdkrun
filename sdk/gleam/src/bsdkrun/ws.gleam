//// A hand-rolled `graphql-transport-ws` client over raw TCP/TLS — no
//// `gleam_erlang`, `gleam_otp`, `mist`, or any other WebSocket/HTTP Hex
//// package, per this SDK's "erlang-only, no new dependencies" constraint.
////
//// This module is split in two, deliberately:
////
//// - **Pure protocol logic** (frame encode/decode, the handshake accept-key
////   computation, and the tiny `graphql-transport-ws` JSON envelope
////   build/parse functions) — ordinary Gleam functions, unit-tested
////   directly against `BitArray`/`String` literals in `test/ws_test.gleam`
////   with no socket involved at all.
//// - **The connection process** (`connect`/`ensure`/`subscribe`/...) — a
////   thin Gleam wrapper around a background process spawned by
////   `bsdkrun_remote_ffi.erl`. That process owns the raw socket and runs a
////   genuine blocking Erlang `receive` loop, dispatching both socket
////   traffic and this SDK's own control messages; Gleam has no `receive`
////   construct of its own; this is the "small amount of Erlang FFI to spawn
////   a receiving process" the feature's design note anticipated. The
////   *stateful* parts (the socket, the ack flag, the pending-subscribe
////   queue, the subscription-id → reply-`Subject` map) live entirely in
////   that Erlang process; it calls back into this module's pure functions
////   (by qualified name, `'bsdkrun@ws':function_name/arity` — Gleam
////   compiles to plain Erlang modules, so this is an ordinary intra-VM call)
////   for every actual protocol decision, so the protocol logic itself still
////   lives, and is tested, in Gleam.
////
//// Known, deliberate shortcut: **fragmented frames (FIN=0 continuations)
//// are not supported** — `decode_frame` reports them as a `FrameError`
//// rather than reassembling them. The daemon's messages here (small JSON
//// control envelopes and shell-output chunks the daemon itself already
//// caps) are not expected to need it in practice; this was judged not
//// worth the extra state machine relative to the value, per the feature's
//// own note that this is an acceptable thing to document and skip.

import bsdkrun/error.{type Error, GraphqlError}
import bsdkrun/subject
import gleam/bit_array
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode.{type Decoder}
import gleam/int
import gleam/json
import gleam/option.{None, Some}
import gleam/result
import gleam/string
import gleam/uri

// ---------------------------------------------------------------------------
// pure: RFC 6455 framing
// ---------------------------------------------------------------------------

/// One decoded (or, for `encode_frame`, to-be-encoded) WebSocket frame.
/// Continuation frames are not represented — see the module doc.
pub type Frame {
  TextFrame(String)
  BinaryFrame(BitArray)
  CloseFrame(code: Int, reason: String)
  PingFrame(BitArray)
  PongFrame(BitArray)
}

/// The result of trying to decode one frame off the front of a buffer.
pub type FrameResult {
  /// A whole frame was decoded; `rest` is whatever was left in the buffer
  /// after it (zero or more further frames, or a partial one).
  Decoded(frame: Frame, rest: BitArray)
  /// Not enough bytes yet for even the frame header, or for its full
  /// payload — wait for more data and try again with the same buffer plus
  /// whatever arrived.
  Incomplete
  /// The buffer starts with something that is not a supported frame at all
  /// (a bad header, or a fragmented/continuation frame).
  FrameError(String)
}

@external(erlang, "bsdkrun_remote_ffi", "sha1")
fn sha1(data: BitArray) -> BitArray

@external(erlang, "bsdkrun_remote_ffi", "strong_rand_bytes")
fn strong_rand_bytes(n: Int) -> BitArray

/// Encode `frame` as a **masked** client→server frame — RFC 6455 requires
/// every frame a client sends to be masked with a fresh random 4-byte key.
/// Server→client frames (what `decode_frame` reads) are never masked by a
/// spec-compliant server, but `decode_frame` honours the mask bit if a
/// server sets it anyway, since unmasking is symmetric with masking.
pub fn encode_frame(frame: Frame) -> BitArray {
  let #(opcode, payload) = case frame {
    TextFrame(text) -> #(1, bit_array.from_string(text))
    BinaryFrame(bytes) -> #(2, bytes)
    CloseFrame(code, reason) -> #(
      8,
      bit_array.concat([<<code:16>>, bit_array.from_string(reason)]),
    )
    PingFrame(bytes) -> #(9, bytes)
    PongFrame(bytes) -> #(10, bytes)
  }
  let mask = strong_rand_bytes(4)
  let masked = xor_with_mask(payload, mask)
  let header = frame_header(opcode, bit_array.byte_size(payload))
  bit_array.concat([header, mask, masked])
}

fn frame_header(opcode: Int, len: Int) -> BitArray {
  let byte1 = <<1:1, 0:1, 0:1, 0:1, opcode:4>>
  let len_part = case len {
    n if n < 126 -> <<1:1, n:7>>
    n if n <= 65_535 -> <<1:1, 126:7, n:16>>
    n -> <<1:1, 127:7, n:64>>
  }
  bit_array.concat([byte1, len_part])
}

/// XOR `data` against `mask`, cycled every 4 bytes — RFC 6455 §5.3. The same
/// operation masks (encoding) and unmasks (decoding), since XOR is its own
/// inverse.
fn xor_with_mask(data: BitArray, mask: BitArray) -> BitArray {
  case mask {
    <<m0, m1, m2, m3>> -> {
      let keys = #(m0, m1, m2, m3)
      mask_bytes(data, keys, 0)
      |> bit_array.concat
    }
    _ -> data
  }
}

fn mask_bytes(
  data: BitArray,
  keys: #(Int, Int, Int, Int),
  index: Int,
) -> List(BitArray) {
  case data {
    <<byte, rest:bytes>> -> {
      let key = case index % 4 {
        0 -> keys.0
        1 -> keys.1
        2 -> keys.2
        _ -> keys.3
      }
      [
        <<int.bitwise_exclusive_or(byte, key)>>,
        ..mask_bytes(rest, keys, index + 1)
      ]
    }
    _ -> []
  }
}

/// Decode one frame off the front of `buffer`, per RFC 6455 §5.2. Every
/// branch that does not have enough bytes yet falls through to `Incomplete`
/// rather than crashing, so a caller can feed this the same (growing) buffer
/// across repeated TCP reads.
pub fn decode_frame(buffer: BitArray) -> FrameResult {
  case buffer {
    <<fin:1, _rsv:3, opcode:4, mask_bit:1, len7:7, rest:bits>> ->
      case fin {
        0 -> FrameError("fragmented (continuation) frames are not supported")
        _ -> read_length(opcode, mask_bit, len7, rest)
      }
    _ -> Incomplete
  }
}

fn read_length(
  opcode: Int,
  mask_bit: Int,
  len7: Int,
  rest: BitArray,
) -> FrameResult {
  case len7 {
    126 ->
      case rest {
        <<len:16, rest2:bits>> -> read_mask(opcode, mask_bit, len, rest2)
        _ -> Incomplete
      }
    127 ->
      case rest {
        <<len:64, rest2:bits>> -> read_mask(opcode, mask_bit, len, rest2)
        _ -> Incomplete
      }
    len -> read_mask(opcode, mask_bit, len, rest)
  }
}

fn read_mask(
  opcode: Int,
  mask_bit: Int,
  len: Int,
  rest: BitArray,
) -> FrameResult {
  case mask_bit {
    1 ->
      case rest {
        <<mask:bytes-size(4), rest2:bits>> ->
          read_payload(opcode, Some(mask), len, rest2)
        _ -> Incomplete
      }
    _ -> read_payload(opcode, None, len, rest)
  }
}

fn read_payload(
  opcode: Int,
  mask: option.Option(BitArray),
  len: Int,
  rest: BitArray,
) -> FrameResult {
  case rest {
    <<payload:bytes-size(len), rest2:bits>> -> {
      let data = case mask {
        Some(m) -> xor_with_mask(payload, m)
        None -> payload
      }
      build_frame(opcode, data, rest2)
    }
    _ -> Incomplete
  }
}

fn build_frame(opcode: Int, data: BitArray, rest: BitArray) -> FrameResult {
  case opcode {
    1 ->
      case bit_array.to_string(data) {
        Ok(text) -> Decoded(TextFrame(text), rest)
        Error(Nil) -> FrameError("text frame was not valid utf-8")
      }
    2 -> Decoded(BinaryFrame(data), rest)
    8 -> {
      let #(code, reason) = decode_close_payload(data)
      Decoded(CloseFrame(code, reason), rest)
    }
    9 -> Decoded(PingFrame(data), rest)
    10 -> Decoded(PongFrame(data), rest)
    other -> FrameError("unsupported opcode " <> int.to_string(other))
  }
}

fn decode_close_payload(data: BitArray) -> #(Int, String) {
  case data {
    <<code:16, reason:bytes>> -> #(
      code,
      result.unwrap(bit_array.to_string(reason), ""),
    )
    _ -> #(1005, "")
  }
}

// ---------------------------------------------------------------------------
// pure: the RFC 6455 handshake accept key
// ---------------------------------------------------------------------------

/// The magic GUID RFC 6455 §1.3 defines for computing `Sec-WebSocket-Accept`.
const handshake_guid = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

/// `base64(sha1(client_key <> magic_guid))` — what a compliant server must
/// echo back as `Sec-WebSocket-Accept`, and what a client must therefore
/// verify the response against.
pub fn compute_accept_key(client_key: String) -> String {
  sha1(bit_array.from_string(client_key <> handshake_guid))
  |> bit_array.base64_encode(True)
}

/// A fresh `Sec-WebSocket-Key`: 16 random bytes, base64-encoded.
pub fn generate_client_key() -> String {
  strong_rand_bytes(16)
  |> bit_array.base64_encode(True)
}

// ---------------------------------------------------------------------------
// pure: the graphql-transport-ws JSON envelope
// ---------------------------------------------------------------------------

/// One parsed `graphql-transport-ws` protocol message.
pub type Incoming {
  Ack
  Next(id: String, data: Dynamic)
  ErrorMsg(id: String, message: String)
  Complete(id: String)
  Ping
}

pub fn parse_incoming(text: String) -> Result(Incoming, Nil) {
  case json.parse(text, incoming_decoder()) {
    Ok(msg) -> Ok(msg)
    Error(_) -> Error(Nil)
  }
}

fn incoming_decoder() -> Decoder(Incoming) {
  use kind <- decode.field("type", decode.string)
  case kind {
    "connection_ack" -> decode.success(Ack)
    "next" -> {
      use id <- decode.field("id", decode.string)
      decode.at(["payload", "data"], decode.dynamic)
      |> decode.then(fn(data) { decode.success(Next(id, data)) })
    }
    "error" -> {
      use id <- decode.field("id", decode.string)
      use messages <- decode.field(
        "payload",
        decode.list(error_message_decoder()),
      )
      decode.success(ErrorMsg(id, string.join(messages, "; ")))
    }
    "complete" -> {
      use id <- decode.field("id", decode.string)
      decode.success(Complete(id))
    }
    "ping" -> decode.success(Ping)
    other -> decode.failure(Ping, "unknown message type " <> other)
  }
}

fn error_message_decoder() -> Decoder(String) {
  use message <- decode.optional_field(
    "message",
    "unknown error",
    decode.string,
  )
  decode.success(message)
}

/// `{"type":"connection_init","payload":{"authorization":"Bearer <token>"}}`
pub fn build_connection_init(token: String) -> String {
  json.object([
    #("type", json.string("connection_init")),
    #(
      "payload",
      json.object([#("authorization", json.string("Bearer " <> token))]),
    ),
  ])
  |> json.to_string
}

/// `{"id":..,"type":"subscribe","payload":{"query":..,"variables":..}}`.
/// `variables_json` is spliced in verbatim — it must already be a valid,
/// serialized JSON value (an object, or `"{}"`), which is what every caller
/// in `bsdkrun/client` produces via `gleam_json`'s builders before handing
/// it here. This (string-based, rather than composed from `json.Json`
/// values) shape is what lets the Erlang connection process build the same
/// frame text the pure Gleam functions here would, without needing to link
/// `gleam_json`'s `Json` builder type across the Gleam/Erlang FFI boundary.
pub fn build_subscribe(
  id: String,
  query: String,
  variables_json: String,
) -> String {
  "{\"id\":"
  <> json.to_string(json.string(id))
  <> ",\"type\":\"subscribe\",\"payload\":{\"query\":"
  <> json.to_string(json.string(query))
  <> ",\"variables\":"
  <> variables_json
  <> "}}"
}

/// `{"id":..,"type":"complete"}` — how a client unsubscribes.
pub fn build_complete(id: String) -> String {
  json.object([#("id", json.string(id)), #("type", json.string("complete"))])
  |> json.to_string
}

/// `{"type":"pong"}` — the reply to a server `{"type":"ping"}` keepalive.
pub fn build_pong() -> String {
  json.object([#("type", json.string("pong"))])
  |> json.to_string
}

// ---------------------------------------------------------------------------
// the connection process
// ---------------------------------------------------------------------------

/// An event delivered to a subscriber's `Subject`, as the Erlang connection
/// process observes them off the socket. `bsdkrun/client` translates these
/// into the richer `ShellEvent` / `SubscriptionEvent` types callers see.
pub type RawEvent {
  RawNext(Dynamic)
  /// A GraphQL `error` message for this subscription, or the socket closing
  /// *after* `connection_ack` — a `GraphqlError`, not an auth failure.
  RawError(String)
  /// The socket closed *before* `connection_ack` ever arrived — the
  /// contract's signal to treat this as `AuthError`, since the daemon closes
  /// unacknowledged sockets exactly when it rejects the token.
  RawAuthError(String)
  RawComplete
}

/// An opaque handle to the background process that owns one WebSocket to one
/// daemon. Get one with `ensure` (shared, cached by url+token) or `connect`
/// (always fresh).
pub opaque type Connection {
  Connection(pid: subject.Pid)
}

@external(erlang, "bsdkrun_remote_ffi", "ws_start")
fn ffi_ws_start(
  host: String,
  port: Int,
  path: String,
  tls: Bool,
  token: String,
  client_key: String,
  reply_owner: subject.Pid,
  reply_tag: String,
) -> Nil

@external(erlang, "bsdkrun_remote_ffi", "ws_subscribe")
fn ffi_ws_subscribe(
  pid: subject.Pid,
  id: String,
  query: String,
  variables_json: String,
  reply_owner: subject.Pid,
  reply_tag: String,
) -> Nil

@external(erlang, "bsdkrun_remote_ffi", "ws_unsubscribe")
fn ffi_ws_unsubscribe(pid: subject.Pid, id: String) -> Nil

@external(erlang, "bsdkrun_remote_ffi", "ws_close")
fn ffi_ws_close(pid: subject.Pid) -> Nil

@external(erlang, "bsdkrun_remote_ffi", "ws_alive")
fn ffi_ws_alive(pid: subject.Pid) -> Bool

@external(erlang, "bsdkrun_remote_ffi", "ws_cache_get")
fn ffi_ws_cache_get(key: String) -> Result(subject.Pid, Nil)

@external(erlang, "bsdkrun_remote_ffi", "ws_cache_put")
fn ffi_ws_cache_put(key: String, pid: subject.Pid) -> Nil

type ConnectOutcome {
  Connected(pid: subject.Pid)
  ConnectFailed(reason: String)
}

/// Where a `ws://`/`wss://` URL points, in the shape the raw-socket FFI
/// needs: host, port (with the scheme's default filled in), path, and
/// whether to speak TLS.
pub type Target {
  Target(host: String, port: Int, path: String, tls: Bool)
}

fn parse_target(url: String) -> Result(Target, Error) {
  case uri.parse(url) {
    Ok(u) -> {
      let tls = u.scheme == Some("wss") || u.scheme == Some("https")
      case u.host {
        Some(host) if host != "" -> {
          let port =
            option.unwrap(u.port, case tls {
              True -> 443
              False -> 80
            })
          let path = case u.path {
            "" -> "/"
            p -> p
          }
          Ok(Target(host:, port:, path:, tls:))
        }
        _ -> Error(GraphqlError("invalid websocket url: " <> url, None))
      }
    }
    Error(_) -> Error(GraphqlError("invalid websocket url: " <> url, None))
  }
}

/// Derive the subscriptions URL from the GraphQL HTTP endpoint URL:
/// `http://` → `ws://`, `https://` → `wss://`, trailing slashes on the path
/// stripped, `/ws` appended. Mirrors `web/src/lib/graphql.ts`'s `wsUrl`.
pub fn derive_url(http_url: String) -> String {
  let with_ws_scheme = case string.starts_with(http_url, "https://") {
    True -> "wss://" <> string.drop_start(http_url, 8)
    False ->
      case string.starts_with(http_url, "http://") {
        True -> "ws://" <> string.drop_start(http_url, 7)
        False -> http_url
      }
  }
  drop_trailing_slashes(with_ws_scheme) <> "/ws"
}

fn drop_trailing_slashes(s: String) -> String {
  case string.ends_with(s, "/") {
    True -> drop_trailing_slashes(string.drop_end(s, 1))
    False -> s
  }
}

/// Open a fresh WebSocket to `url` (a `ws://`/`wss://` URL — see
/// `derive_url`), complete the RFC 6455 handshake, and send
/// `connection_init`. Blocks until the handshake either succeeds or fails;
/// does **not** wait for `connection_ack` — later `subscribe` calls are
/// queued internally until the ack arrives (see the module doc).
pub fn connect(url: String, token: String) -> Result(Connection, Error) {
  use target <- result.try(parse_target(url))
  let client_key = generate_client_key()
  let reply = subject.new()
  ffi_ws_start(
    target.host,
    target.port,
    target.path,
    target.tls,
    token,
    client_key,
    subject.owner(reply),
    subject.tag(reply),
  )
  case subject.receive(reply, 15_000) {
    Ok(Connected(pid)) -> Ok(Connection(pid))
    Ok(ConnectFailed(reason)) ->
      Error(GraphqlError(
        "cannot reach the bsdkrun daemon at " <> url <> " — " <> reason,
        None,
      ))
    Error(Nil) -> Error(GraphqlError("timed out connecting to " <> url, None))
  }
}

/// Like `connect`, but reuses an existing live connection for the same
/// `url`/`token` pair rather than opening a new socket — "one shared socket
/// per client connection" (the design note's phrase), implemented as a
/// small `persistent_term`-backed cache in `bsdkrun_remote_ffi.erl` keyed by
/// `url <> token`. `bsdkrun/client`'s `Client` is a plain, immutable
/// url/token pair with nowhere of its own to stash a live pid — this cache
/// is what gives repeated calls against the same `Client` a shared socket
/// without making `Client` itself mutable.
pub fn ensure(url: String, token: String) -> Result(Connection, Error) {
  let key = url <> "\u{0}" <> token
  case ffi_ws_cache_get(key) {
    Ok(pid) -> Ok(Connection(pid))
    Error(Nil) -> {
      use conn <- result.try(connect(url, token))
      let Connection(pid) = conn
      ffi_ws_cache_put(key, pid)
      Ok(conn)
    }
  }
}

/// Start a subscription. Events (`RawNext`/`RawError`/`RawAuthError`/
/// `RawComplete`) are delivered to `reply` as they arrive — including
/// whatever was already buffered before this call, per the daemon's
/// "output is buffered from the moment the session opens" guarantee.
pub fn subscribe(
  conn: Connection,
  id: String,
  query: String,
  variables_json: String,
  reply: subject.Subject(RawEvent),
) -> Nil {
  let Connection(pid) = conn
  ffi_ws_subscribe(
    pid,
    id,
    query,
    variables_json,
    subject.owner(reply),
    subject.tag(reply),
  )
}

/// Send `{"id":..,"type":"complete"}` to end a subscription. Closes the
/// socket once it was the last one open, per the design note.
pub fn unsubscribe(conn: Connection, id: String) -> Nil {
  let Connection(pid) = conn
  ffi_ws_unsubscribe(pid, id)
}

/// Close the connection outright, regardless of open subscriptions.
pub fn close(conn: Connection) -> Nil {
  let Connection(pid) = conn
  ffi_ws_close(pid)
}

/// Whether the connection process is still alive. `False` after the socket
/// closed for any reason.
pub fn alive(conn: Connection) -> Bool {
  let Connection(pid) = conn
  ffi_ws_alive(pid)
}
