defmodule Bsdkrun.GraphQLSocket do
  @moduledoc """
  One shared `graphql-transport-ws` socket per `Bsdkrun.Client` "connection"
  (same url + token), multiplexing every live subscription over it —
  `exec/4`, `shell/3`, `follow_logs/3` and the `subscribe/4` escape hatch all
  share one socket per client. Started lazily on the first subscription,
  under `Bsdkrun.Client.SocketSupervisor` (a `DynamicSupervisor`), and
  registered in `Bsdkrun.Client.Registry` by `{url, token}` so every caller
  using the same `Bsdkrun.Client` finds the same socket. See the private
  `ensure_conn/1` function in `Bsdkrun.Client`.

  Two protocol layers are hand-rolled here, per the daemon's contract
  (`daemon/README.md`, `web/src/lib/graphql.ts`) — no hex dependency for
  either:

    * RFC 6455 WebSocket framing — the HTTP Upgrade handshake and
      frame encode/decode are pure functions in `Bsdkrun.WsFrame`.
    * The `graphql-transport-ws` message protocol on top of it:
      `connection_init` -> wait for `connection_ack` (queuing any
      `subscribe` sent in the meantime, flushed once the ack lands) ->
      `subscribe` / `next` / `error` / `complete`, plus `ping`/`pong`.

  A subscription is registered with a `handler` function, `(id, event) ->
  any`, invoked **from this GenServer's own process** — so a slow handler
  (e.g. a blocking callback) delays this socket's frame processing for every
  other subscription sharing it. `Bsdkrun.Client` keeps handlers cheap
  (message-send or a user callback) precisely to avoid that.
  """

  use GenServer

  alias Bsdkrun.{Error, WsFrame}

  @connect_timeout 10_000

  defstruct [
    :url,
    :token,
    :transport,
    :socket,
    buffer: <<>>,
    acked: false,
    # [{subscribe_id, encoded_subscribe_message}], oldest first once reversed.
    pending: [],
    # subscribe_id => %{handler: (id, event) -> any}
    subs: %{},
    next_id: 1
  ]

  # --- public API --------------------------------------------------------

  @doc false
  @spec start_link({String.t(), String.t(), term()}) :: GenServer.on_start()
  def start_link({ws_url, token, registry_key}) do
    GenServer.start_link(__MODULE__, {ws_url, token},
      name: {:via, Registry, {Bsdkrun.Client.Registry, registry_key}}
    )
  end

  @doc """
  Start a subscription for `query`/`variables`. `handler` is called as
  `handler.(subscription_id, event)` for every event delivered to it:

    * `{:next, data}` — the `payload.data` of a `next` message.
    * `{:error, %Bsdkrun.Error{}}` — a GraphQL `error` message, or the socket
      closing while this subscription was still open.
    * `:complete` — a `complete` message.

  Returns `{:ok, subscription_id}`, immediately — the `subscribe` message may
  be queued internally until `connection_ack` arrives.
  """
  @spec subscribe(pid(), String.t(), map(), (String.t(), term() -> any())) ::
          {:ok, String.t()} | {:error, Error.t()}
  def subscribe(pid, query, variables, handler) do
    GenServer.call(pid, {:subscribe, query, variables, handler})
  catch
    :exit, _reason -> {:error, Error.graphql_error("connection to the daemon was closed")}
  end

  @doc """
  Cancel a subscription. Idempotent: an unknown id, or a socket that already
  died, is not an error. Closes the socket (and this process exits) once no
  subscription is left on it.
  """
  @spec unsubscribe(pid(), String.t()) :: :ok
  def unsubscribe(pid, id) do
    GenServer.call(pid, {:unsubscribe, id})
  catch
    :exit, _reason -> :ok
  end

  # --- GenServer callbacks -------------------------------------------------

  @impl true
  def init({ws_url, token}) do
    with {:ok, transport, socket, leftover} <- connect(ws_url),
         :ok <- send_connection_init(transport, socket, token),
         :ok <- set_active(transport, socket) do
      state = %__MODULE__{
        url: ws_url,
        token: token,
        transport: transport,
        socket: socket,
        buffer: leftover
      }

      {:ok, process_buffer(state)}
    else
      {:error, reason} -> {:stop, reason}
    end
  end

  @impl true
  def handle_call({:subscribe, query, variables, handler}, _from, state) do
    id = to_string(state.next_id)
    msg = Jason.encode!(%{id: id, type: "subscribe", payload: %{query: query, variables: variables}})

    state = %{
      state
      | subs: Map.put(state.subs, id, %{handler: handler}),
        next_id: state.next_id + 1
    }

    state =
      if state.acked do
        send_frame(state, msg)
        state
      else
        %{state | pending: [{id, msg} | state.pending]}
      end

    {:reply, {:ok, id}, state}
  end

  def handle_call({:unsubscribe, id}, _from, state) do
    known? = Map.has_key?(state.subs, id)
    if known?, do: send_frame(state, Jason.encode!(%{id: id, type: "complete"}))

    state = %{
      state
      | subs: Map.delete(state.subs, id),
        pending: Enum.reject(state.pending, fn {sub_id, _msg} -> sub_id == id end)
    }

    if map_size(state.subs) == 0 do
      close_socket(state)
      {:stop, :normal, :ok, state}
    else
      {:reply, :ok, state}
    end
  end

  @impl true
  def handle_info({:tcp, sock, data}, %{socket: sock} = state),
    do: {:noreply, process_buffer(append(state, data))}

  def handle_info({:ssl, sock, data}, %{socket: sock} = state),
    do: {:noreply, process_buffer(append(state, data))}

  def handle_info({:tcp_closed, sock}, %{socket: sock} = state), do: handle_transport_down(state)
  def handle_info({:ssl_closed, sock}, %{socket: sock} = state), do: handle_transport_down(state)

  def handle_info({:tcp_error, sock, _reason}, %{socket: sock} = state),
    do: handle_transport_down(state)

  def handle_info({:ssl_error, sock, _reason}, %{socket: sock} = state),
    do: handle_transport_down(state)

  def handle_info(_other, state), do: {:noreply, state}

  @impl true
  def terminate(_reason, state) do
    close_socket(state)
    :ok
  end

  # --- connect / RFC 6455 handshake ---------------------------------------

  defp connect(ws_url) do
    uri = URI.parse(ws_url)
    secure? = uri.scheme == "wss"
    path = uri.path || "/"
    path = if uri.query, do: path <> "?" <> uri.query, else: path

    with {:ok, transport, socket} <- open_transport(uri.host, uri.port, secure?),
         key = WsFrame.random_key(),
         :ok <- send_handshake(transport, socket, uri.host, uri.port, path, key),
         {:ok, leftover} <- await_handshake(transport, socket, key, <<>>) do
      {:ok, transport, socket, leftover}
    end
  end

  defp open_transport(host, port, false) do
    opts = [:binary, active: false, packet: :raw]

    case :gen_tcp.connect(String.to_charlist(host), port, opts, @connect_timeout) do
      {:ok, sock} -> {:ok, :gen_tcp, sock}
      {:error, reason} -> {:error, reason}
    end
  end

  defp open_transport(host, port, true) do
    _ = Application.ensure_all_started(:ssl)
    # Full system-CA verification by default — see Bsdkrun.GraphQL's
    # http_options/1 for why (same policy on both transports).
    opts = [
      :binary,
      active: false,
      verify: :verify_peer,
      cacerts: :public_key.cacerts_get(),
      server_name_indication: String.to_charlist(host)
    ]

    case :ssl.connect(String.to_charlist(host), port, opts, @connect_timeout) do
      {:ok, sock} -> {:ok, :ssl, sock}
      {:error, reason} -> {:error, reason}
    end
  end

  defp send_handshake(transport, socket, host, port, path, key) do
    request =
      "GET #{path} HTTP/1.1\r\n" <>
        "Host: #{host}:#{port}\r\n" <>
        "Upgrade: websocket\r\n" <>
        "Connection: Upgrade\r\n" <>
        "Sec-WebSocket-Key: #{key}\r\n" <>
        "Sec-WebSocket-Version: 13\r\n" <>
        "Sec-WebSocket-Protocol: graphql-transport-ws\r\n" <>
        "\r\n"

    transport_send(transport, socket, request)
  end

  # Accumulates raw bytes (in passive mode) until the header terminator shows
  # up, then validates the response. Any bytes received past "\r\n\r\n" are
  # already payload — the start of the connection_ack frame, most likely —
  # and are returned as `leftover` so init/1 seeds the frame buffer with them
  # instead of losing them.
  defp await_handshake(transport, socket, key, acc) do
    if String.contains?(acc, "\r\n\r\n") do
      [head, rest] = String.split(acc, "\r\n\r\n", parts: 2)
      validate_handshake(head, key, rest)
    else
      case transport_recv(transport, socket, 0, @connect_timeout) do
        {:ok, data} -> await_handshake(transport, socket, key, acc <> data)
        {:error, reason} -> {:error, reason}
      end
    end
  end

  defp validate_handshake(head, key, rest) do
    [status_line | header_lines] = String.split(head, "\r\n")
    headers = parse_headers(header_lines)

    cond do
      not String.contains?(status_line, " 101 ") ->
        {:error, {:unexpected_handshake_status, status_line}}

      Map.get(headers, "sec-websocket-accept") != WsFrame.accept_key(key) ->
        {:error, :bad_sec_websocket_accept}

      true ->
        {:ok, rest}
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

  defp set_active(:gen_tcp, socket), do: :inet.setopts(socket, active: true)
  defp set_active(:ssl, socket), do: :ssl.setopts(socket, active: true)

  defp send_connection_init(transport, socket, token) do
    msg = Jason.encode!(%{type: "connection_init", payload: %{authorization: "Bearer " <> token}})
    transport_send(transport, socket, WsFrame.encode_text(msg))
  end

  defp transport_send(:gen_tcp, socket, data), do: :gen_tcp.send(socket, data)
  defp transport_send(:ssl, socket, data), do: :ssl.send(socket, data)

  defp transport_recv(:gen_tcp, socket, len, timeout), do: :gen_tcp.recv(socket, len, timeout)
  defp transport_recv(:ssl, socket, len, timeout), do: :ssl.recv(socket, len, timeout)

  # --- frame / message handling --------------------------------------------

  defp append(state, data), do: %{state | buffer: state.buffer <> data}

  defp send_frame(state, text), do: transport_send(state.transport, state.socket, WsFrame.encode_text(text))

  defp process_buffer(state) do
    case WsFrame.decode(state.buffer) do
      {:ok, %{opcode: 0x1, payload: payload}, rest} ->
        state |> handle_text(payload) |> Map.put(:buffer, rest) |> process_buffer()

      {:ok, %{}, rest} ->
        # Binary / ping / pong / close frames at the WS layer, and any
        # continuation frame: not used by this protocol (control messages
        # travel as JSON text frames) — see the module doc's shortcut note.
        process_buffer(%{state | buffer: rest})

      :incomplete ->
        state
    end
  end

  defp handle_text(state, payload) do
    case Jason.decode(payload) do
      {:ok, %{"type" => "connection_ack"}} ->
        flush_pending(%{state | acked: true})

      {:ok, %{"type" => "next", "id" => id, "payload" => %{"data" => data}}} ->
        notify(state, id, {:next, data})

      {:ok, %{"type" => "error", "id" => id, "payload" => errors}} ->
        deliver_error(state, id, errors)

      {:ok, %{"type" => "complete", "id" => id}} ->
        deliver_complete(state, id)

      {:ok, %{"type" => "ping"}} ->
        send_frame(state, Jason.encode!(%{type: "pong"}))
        state

      _other ->
        state
    end
  end

  defp flush_pending(state) do
    state.pending
    |> Enum.reverse()
    |> Enum.each(fn {id, msg} -> if Map.has_key?(state.subs, id), do: send_frame(state, msg) end)

    %{state | pending: []}
  end

  defp notify(state, id, event) do
    case Map.get(state.subs, id) do
      nil -> :ok
      %{handler: handler} -> handler.(id, event)
    end

    state
  end

  defp deliver_error(state, id, errors) do
    message =
      errors
      |> List.wrap()
      |> Enum.map(fn e -> (is_map(e) && e["message"]) || inspect(e) end)
      |> Enum.join("; ")

    state = notify(state, id, {:error, Error.graphql_error(message)})
    %{state | subs: Map.delete(state.subs, id)}
  end

  defp deliver_complete(state, id) do
    state = notify(state, id, :complete)
    %{state | subs: Map.delete(state.subs, id)}
  end

  # "If the socket closes before connection_ack was ever received, treat it
  # as an AuthError for every pending/open subscription. If it closes after
  # ack, a generic 'connection to the daemon was closed' GraphQLError."
  defp handle_transport_down(state) do
    error =
      if state.acked,
        do: Error.graphql_error("the connection to the daemon was closed"),
        else: Error.auth_error()

    Enum.each(state.subs, fn {id, %{handler: handler}} -> handler.(id, {:error, error}) end)
    {:stop, :normal, %{state | subs: %{}}}
  end

  defp close_socket(%{transport: :gen_tcp, socket: sock}), do: :gen_tcp.close(sock)
  defp close_socket(%{transport: :ssl, socket: sock}), do: :ssl.close(sock)
end
