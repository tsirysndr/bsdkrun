//// Unit tests for `bsdkrun/ws`'s pure protocol logic: RFC 6455 frame
//// encode/decode, the handshake accept-key computation, and the
//// `graphql-transport-ws` JSON envelope. All against `BitArray`/`String`
//// literals — no socket, no daemon, per the feature's own suggested
//// fallback for testing a hand-rolled WebSocket client.

import bsdkrun/ws
import gleam/bit_array
import gleam/string
import gleeunit/should

// --- RFC 6455 §5.7 worked examples ------------------------------------------

/// The spec's own example: a single-frame unmasked text message "Hello",
/// as a server would send it to a client. `0x81 0x05` = FIN+text opcode,
/// 5-byte unmasked payload.
pub fn decode_unmasked_hello_test() {
  let frame = <<0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f>>

  ws.decode_frame(frame)
  |> should.equal(ws.Decoded(ws.TextFrame("Hello"), <<>>))
}

/// The spec's own example: the same message, masked, as a client would send
/// it. `0x81 0x85` = FIN+text opcode, mask bit set, 5-byte payload; the next
/// 4 bytes are the mask key, then the masked payload.
pub fn decode_masked_hello_test() {
  let frame = <<
    0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
  >>

  ws.decode_frame(frame)
  |> should.equal(ws.Decoded(ws.TextFrame("Hello"), <<>>))
}

// --- encode_frame ------------------------------------------------------------

/// `encode_frame` always masks (client->server) — every masked frame's
/// second byte has its top bit set — and round-trips through `decode_frame`
/// (which honours the mask bit, so this is also an end-to-end check).
pub fn encode_frame_is_masked_and_round_trips_test() {
  let bytes = ws.encode_frame(ws.TextFrame("Hello, bsdkrun!"))

  let assert <<_first, second, _rest:bits>> = bytes
  { second |> int_band(0x80) } |> should.equal(0x80)

  ws.decode_frame(bytes)
  |> should.equal(ws.Decoded(ws.TextFrame("Hello, bsdkrun!"), <<>>))
}

/// Two masked encodings of the same text differ (a fresh random mask each
/// time), but both still decode back to the original text.
pub fn encode_frame_masks_differ_test() {
  let a = ws.encode_frame(ws.TextFrame("same text"))
  let b = ws.encode_frame(ws.TextFrame("same text"))

  { a == b } |> should.equal(False)
  ws.decode_frame(a)
  |> should.equal(ws.Decoded(ws.TextFrame("same text"), <<>>))
  ws.decode_frame(b)
  |> should.equal(ws.Decoded(ws.TextFrame("same text"), <<>>))
}

/// A payload at the 126-byte boundary exercises the 16-bit extended-length
/// header path, not just the 7-bit inline length.
pub fn encode_frame_extended_length_test() {
  let text = string_repeat("x", 200)
  let bytes = ws.encode_frame(ws.TextFrame(text))

  ws.decode_frame(bytes)
  |> should.equal(ws.Decoded(ws.TextFrame(text), <<>>))
}

fn string_repeat(s: String, n: Int) -> String {
  case n {
    0 -> ""
    _ -> s <> string_repeat(s, n - 1)
  }
}

fn int_band(a: Int, b: Int) -> Int {
  // gleam/int's bitwise_and, spelled out locally to keep this test file's
  // imports minimal.
  do_band(a, b)
}

@external(erlang, "erlang", "band")
fn do_band(a: Int, b: Int) -> Int

// --- decode_frame edge cases -------------------------------------------------

pub fn decode_frame_incomplete_header_test() {
  ws.decode_frame(<<0x81>>)
  |> should.equal(ws.Incomplete)
}

pub fn decode_frame_incomplete_payload_test() {
  // Header says 5 bytes of payload; only 2 are present.
  ws.decode_frame(<<0x81, 0x05, 0x48, 0x65>>)
  |> should.equal(ws.Incomplete)
}

/// FIN=0 (a continuation/fragmented frame) is a documented shortcut this
/// client does not support.
pub fn decode_frame_fragmented_is_an_error_test() {
  case ws.decode_frame(<<0x01, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f>>) {
    ws.FrameError(_) -> Nil
    other -> panic as string.inspect(other)
  }
}

/// Two frames back to back decode one at a time, leaving the second as
/// `rest` — how the connection process drains a buffer that may contain
/// more than one frame per TCP read.
pub fn decode_frame_leaves_remaining_bytes_as_rest_test() {
  let one = <<0x81, 0x02, 0x68, 0x69>>
  // "hi"
  let two = <<0x81, 0x02, 0x6f, 0x6b>>
  // "ok"
  let buffer = bit_array.concat([one, two])

  let assert ws.Decoded(ws.TextFrame("hi"), rest) = ws.decode_frame(buffer)
  ws.decode_frame(rest) |> should.equal(ws.Decoded(ws.TextFrame("ok"), <<>>))
}

/// A ping frame (opcode 0x9) decodes with its payload intact — the
/// connection process replies with a pong, but decoding it is this
/// module's job.
pub fn decode_ping_frame_test() {
  ws.decode_frame(<<0x89, 0x00>>)
  |> should.equal(ws.Decoded(ws.PingFrame(<<>>), <<>>))
}

// --- the RFC 6455 handshake --------------------------------------------------

/// RFC 6455 §1.3's own worked example.
pub fn compute_accept_key_rfc_example_test() {
  ws.compute_accept_key("dGhlIHNhbXBsZSBub25jZQ==")
  |> should.equal("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
}

pub fn generate_client_key_is_fresh_each_time_test() {
  { ws.generate_client_key() == ws.generate_client_key() }
  |> should.equal(False)
}

// --- the graphql-transport-ws JSON envelope ----------------------------------

pub fn parse_ack_test() {
  ws.parse_incoming("{\"type\":\"connection_ack\"}")
  |> should.equal(Ok(ws.Ack))
}

pub fn parse_ping_test() {
  ws.parse_incoming("{\"type\":\"ping\"}")
  |> should.equal(Ok(ws.Ping))
}

pub fn parse_complete_test() {
  ws.parse_incoming("{\"type\":\"complete\",\"id\":\"1\"}")
  |> should.equal(Ok(ws.Complete("1")))
}

pub fn parse_error_joins_messages_test() {
  let raw =
    "{\"type\":\"error\",\"id\":\"1\",\"payload\":["
    <> "{\"message\":\"boom\"},{\"message\":\"again\"}]}"

  ws.parse_incoming(raw)
  |> should.equal(Ok(ws.ErrorMsg("1", "boom; again")))
}

pub fn parse_next_carries_data_test() {
  let raw =
    "{\"type\":\"next\",\"id\":\"1\",\"payload\":{\"data\":{\"machines\":[]}}}"

  let assert Ok(ws.Next("1", _data)) = ws.parse_incoming(raw)
}

pub fn parse_garbage_is_an_error_test() {
  ws.parse_incoming("not json")
  |> should.equal(Error(Nil))
}

pub fn build_connection_init_test() {
  ws.build_connection_init("secret-token")
  |> should.equal(
    "{\"type\":\"connection_init\",\"payload\":{\"authorization\":\"Bearer secret-token\"}}",
  )
}

pub fn build_subscribe_test() {
  ws.build_subscribe("1", "{ machines { id } }", "{\"all\":true}")
  |> should.equal(
    "{\"id\":\"1\",\"type\":\"subscribe\",\"payload\":{\"query\":\"{ machines { id } }\",\"variables\":{\"all\":true}}}",
  )
}

pub fn build_complete_test() {
  ws.build_complete("1")
  |> should.equal("{\"id\":\"1\",\"type\":\"complete\"}")
}

pub fn build_pong_test() {
  ws.build_pong() |> should.equal("{\"type\":\"pong\"}")
}

// --- URL derivation -----------------------------------------------------------

pub fn derive_url_http_test() {
  ws.derive_url("http://host:50052/graphql")
  |> should.equal("ws://host:50052/graphql/ws")
}

pub fn derive_url_https_test() {
  ws.derive_url("https://host:50052/graphql")
  |> should.equal("wss://host:50052/graphql/ws")
}

pub fn derive_url_strips_trailing_slashes_test() {
  ws.derive_url("http://host:50052/graphql///")
  |> should.equal("ws://host:50052/graphql/ws")
}
