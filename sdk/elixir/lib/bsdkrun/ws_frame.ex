defmodule Bsdkrun.WsFrame do
  @moduledoc """
  Hand-rolled RFC 6455 WebSocket framing for `Bsdkrun.GraphQLSocket`: the
  `Sec-WebSocket-Accept` handshake computation, encoding a masked client
  frame, and decoding a frame off the front of a byte buffer.

  Every function here is pure (no socket, no process), so the protocol logic
  can be exercised directly in tests via plain binary pattern matching.

  Shortcut taken: fragmented/continuation frames (`fin: 0`, or `opcode: 0x0`)
  are parsed structurally — `decode/1` returns them like any other frame —
  but never reassembled into one logical message. `graphql-transport-ws`
  messages are short JSON control frames and modest binary chunks that fit in
  a single frame in practice, so `Bsdkrun.GraphQLSocket` only acts on
  complete single-frame text messages (`opcode: 0x1`, `fin: 1`) and silently
  drops everything else (binary, ping, pong, close, continuation) at the
  WS-frame layer. The daemon's own connection teardown is still detected —
  that happens at the TCP/TLS level (`:tcp_closed` / `:ssl_closed`), not by
  parsing an RFC 6455 close frame, so this shortcut does not lose that signal.
  """

  import Bitwise

  @rfc6455_guid "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

  @typedoc "A decoded WebSocket frame."
  @type frame :: %{fin: 0 | 1, opcode: non_neg_integer(), payload: binary()}

  @doc """
  The `Sec-WebSocket-Accept` value a compliant server must return for the
  given client `Sec-WebSocket-Key` (RFC 6455 §1.3): SHA-1 of the key
  concatenated with the RFC 6455 magic GUID, base64-encoded.
  """
  @spec accept_key(String.t()) :: String.t()
  def accept_key(client_key) when is_binary(client_key) do
    :crypto.hash(:sha, client_key <> @rfc6455_guid) |> Base.encode64()
  end

  @doc "A fresh random base64 `Sec-WebSocket-Key`, per RFC 6455 §4.1 (16 random bytes)."
  @spec random_key() :: String.t()
  def random_key(), do: Base.encode64(:crypto.strong_rand_bytes(16))

  @doc "Encode a masked client text frame (opcode `0x1`) carrying `text`."
  @spec encode_text(String.t()) :: binary()
  def encode_text(text) when is_binary(text), do: encode(0x1, text)

  @doc """
  Encode a masked client frame (RFC 6455 §5.2) with the given `opcode` and
  binary `payload`. Client -> server frames are always masked, with a fresh
  random 4-byte key per frame.
  """
  @spec encode(non_neg_integer(), binary()) :: binary()
  def encode(opcode, payload) when is_integer(opcode) and is_binary(payload) do
    mask_key = :crypto.strong_rand_bytes(4)
    <<1::1, 0::3, opcode::4>> <> length_field(byte_size(payload)) <> mask_key <> mask(payload, mask_key)
  end

  defp length_field(len) when len < 126, do: <<1::1, len::7>>
  defp length_field(len) when len <= 65_535, do: <<1::1, 126::7, len::16>>
  defp length_field(len), do: <<1::1, 127::7, len::64>>

  @doc """
  XOR `data` with the 4-byte `mask_key`, cycling every 4 bytes (RFC 6455
  §5.3). The same operation masks (client -> server) and unmasks (applying it
  twice is the identity), so `decode/1` reuses this to unmask an incoming
  masked frame.
  """
  @spec mask(binary(), <<_::32>>) :: binary()
  def mask(data, <<_::32>> = mask_key) do
    keys = for(<<b <- mask_key>>, do: b) |> List.to_tuple()
    do_mask(data, keys, 0, [])
  end

  defp do_mask(<<>>, _keys, _i, acc), do: acc |> Enum.reverse() |> IO.iodata_to_binary()

  defp do_mask(<<byte, rest::binary>>, keys, i, acc) do
    do_mask(rest, keys, i + 1, [bxor(byte, elem(keys, rem(i, 4))) | acc])
  end

  @doc """
  Decode one frame off the front of `buffer`.

  Returns `{:ok, frame, rest}` when a complete frame is present at the front
  of `buffer` (`rest` is whatever bytes follow it, possibly empty, possibly
  the start of the next frame), or `:incomplete` when `buffer` does not yet
  hold a full frame and the caller should wait for more bytes.
  """
  @spec decode(binary()) :: {:ok, frame(), binary()} | :incomplete
  def decode(<<fin::1, _rsv::3, opcode::4, masked::1, len7::7, rest::binary>>) do
    case len7 do
      126 -> decode_len16(fin, opcode, masked, rest)
      127 -> decode_len64(fin, opcode, masked, rest)
      len -> decode_payload(fin, opcode, masked, len, rest)
    end
  end

  def decode(_incomplete), do: :incomplete

  defp decode_len16(fin, opcode, masked, <<len::16, rest::binary>>),
    do: decode_payload(fin, opcode, masked, len, rest)

  defp decode_len16(_fin, _opcode, _masked, _rest), do: :incomplete

  defp decode_len64(fin, opcode, masked, <<len::64, rest::binary>>),
    do: decode_payload(fin, opcode, masked, len, rest)

  defp decode_len64(_fin, _opcode, _masked, _rest), do: :incomplete

  # Masked frame (client -> server; only relevant if this module is ever
  # used to decode what this SDK itself sent, e.g. in a test WS server).
  defp decode_payload(fin, opcode, 1, len, rest) do
    case rest do
      <<mask_key::binary-size(4), payload::binary-size(len), remaining::binary>> ->
        {:ok, %{fin: fin, opcode: opcode, payload: mask(payload, mask_key)}, remaining}

      _ ->
        :incomplete
    end
  end

  # Unmasked frame (server -> client — what Bsdkrun.GraphQLSocket receives).
  defp decode_payload(fin, opcode, 0, len, rest) do
    case rest do
      <<payload::binary-size(len), remaining::binary>> ->
        {:ok, %{fin: fin, opcode: opcode, payload: payload}, remaining}

      _ ->
        :incomplete
    end
  end
end
