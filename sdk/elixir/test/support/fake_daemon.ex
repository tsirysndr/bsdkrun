defmodule Bsdkrun.Test.FakeDaemon do
  @moduledoc """
  A minimal stand-in for `bsdkrund`'s GraphQL endpoint, for exercising
  `Bsdkrun.GraphQL` and `Bsdkrun.GraphQLSocket` against a real socket with no
  hex dependency (no Plug/Cowboy/etc): a plain `:gen_tcp` listener that
  routes each connection to an `:http` handler (a raw HTTP/1.1 request in,
  `{status, body}` out) or, on a WebSocket Upgrade request, performs the RFC
  6455 handshake itself and hands the live socket to a `:ws` handler to
  drive directly (send/recv frames).

  One listener serves both, on one port — exactly like the real daemon,
  which lets a `Bsdkrun.Client` pointed at this fake do everything it would
  against `bsdkrund` (HTTP mutations *and* the `/ws` subscription) without
  knowing the difference.
  """

  alias Bsdkrun.WsFrame

  @doc """
  Start listening on a random local port. `opts`:

    * `:http` — `(request map -> {status, response body})`, required.
    * `:ws`   — `(socket -> any)`, called after this module completes the WS
      handshake on the client's behalf. Defaults to a no-op (close).

  Returns the base URL, e.g. `"http://127.0.0.1:54321"`.
  """
  @spec start(keyword()) :: String.t()
  def start(opts) do
    http_handler = Keyword.fetch!(opts, :http)
    ws_handler = Keyword.get(opts, :ws, fn _sock -> :ok end)

    {:ok, listen} = :gen_tcp.listen(0, [:binary, packet: :raw, active: false, reuseaddr: true])
    {:ok, port} = :inet.port(listen)

    spawn_link(fn -> accept_loop(listen, http_handler, ws_handler) end)

    "http://127.0.0.1:#{port}"
  end

  defp accept_loop(listen, http_handler, ws_handler) do
    case :gen_tcp.accept(listen, 3_000) do
      {:ok, sock} ->
        spawn(fn -> handle_conn(sock, http_handler, ws_handler) end)
        accept_loop(listen, http_handler, ws_handler)

      {:error, _timeout_or_closed} ->
        :gen_tcp.close(listen)
    end
  end

  defp handle_conn(sock, http_handler, ws_handler) do
    raw = recv_until(sock, "\r\n\r\n", <<>>)
    [head, rest] = String.split(raw, "\r\n\r\n", parts: 2)
    [request_line | header_lines] = String.split(head, "\r\n")
    headers = parse_headers(header_lines)

    if String.downcase(Map.get(headers, "upgrade", "")) == "websocket" do
      handshake!(sock, headers)
      ws_handler.(sock)
    else
      body = read_body(sock, headers, rest)
      {status, resp_body} = http_handler.(%{request_line: request_line, headers: headers, body: body})
      :gen_tcp.send(sock, http_response(status, resp_body))
    end

    :gen_tcp.close(sock)
  catch
    _kind, _reason -> :gen_tcp.close(sock)
  end

  defp handshake!(sock, headers) do
    key = Map.fetch!(headers, "sec-websocket-key")
    accept = WsFrame.accept_key(key)

    response =
      "HTTP/1.1 101 Switching Protocols\r\n" <>
        "Upgrade: websocket\r\n" <>
        "Connection: Upgrade\r\n" <>
        "Sec-WebSocket-Accept: #{accept}\r\n" <>
        "Sec-WebSocket-Protocol: graphql-transport-ws\r\n" <>
        "\r\n"

    :gen_tcp.send(sock, response)
  end

  defp read_body(sock, headers, already_read) do
    content_length = headers |> Map.get("content-length", "0") |> String.to_integer()
    missing = content_length - byte_size(already_read)

    if missing > 0 do
      {:ok, more} = :gen_tcp.recv(sock, missing, 5_000)
      already_read <> more
    else
      already_read
    end
  end

  defp recv_until(sock, marker, acc) do
    if String.contains?(acc, marker) do
      acc
    else
      {:ok, data} = :gen_tcp.recv(sock, 0, 5_000)
      recv_until(sock, marker, acc <> data)
    end
  end

  defp parse_headers(lines) do
    Enum.reduce(lines, %{}, fn line, acc ->
      case String.split(line, ":", parts: 2) do
        [k, v] -> Map.put(acc, k |> String.trim() |> String.downcase(), String.trim(v))
        _other -> acc
      end
    end)
  end

  defp http_response(status, body) do
    "HTTP/1.1 #{status} #{reason_phrase(status)}\r\n" <>
      "content-type: application/json\r\n" <>
      "content-length: #{byte_size(body)}\r\n" <>
      "connection: close\r\n" <>
      "\r\n" <>
      body
  end

  defp reason_phrase(200), do: "OK"
  defp reason_phrase(401), do: "Unauthorized"
  defp reason_phrase(_other), do: "Status"

  # --- helpers for a :ws handler to drive its socket ------------------------

  @doc "Read and decode the next client -> server frame off `sock` (blocking, buffering as needed)."
  @spec recv_frame(port(), binary()) :: {WsFrame.frame(), binary()}
  def recv_frame(sock, buffer \\ <<>>) do
    case WsFrame.decode(buffer) do
      {:ok, frame, rest} ->
        {frame, rest}

      :incomplete ->
        {:ok, data} = :gen_tcp.recv(sock, 0, 5_000)
        recv_frame(sock, buffer <> data)
    end
  end

  @doc "Send an unmasked server -> client text frame (RFC 6455 §5.1: the server never masks)."
  @spec send_text(port(), String.t()) :: :ok
  def send_text(sock, text) do
    :gen_tcp.send(sock, unmasked_frame(0x1, text))
    :ok
  end

  defp unmasked_frame(opcode, payload) do
    <<1::1, 0::3, opcode::4>> <> unmasked_length_field(byte_size(payload)) <> payload
  end

  defp unmasked_length_field(len) when len < 126, do: <<0::1, len::7>>
  defp unmasked_length_field(len) when len <= 65_535, do: <<0::1, 126::7, len::16>>
  defp unmasked_length_field(len), do: <<0::1, 127::7, len::64>>
end
