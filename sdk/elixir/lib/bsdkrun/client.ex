defmodule Bsdkrun.Client do
  @moduledoc """
  A remote client for a `bsdkrund` daemon's GraphQL API — talks straight to
  `POST <url>` (queries/mutations) and a `graphql-transport-ws` socket
  (subscriptions), instead of shelling out to a local `bsdkrun` binary like
  `Bsdkrun.Sandbox` does. Same contract the web frontend speaks
  (`web/src/lib/graphql.ts`, `web/src/lib/api.ts`) and the daemon documents in
  `daemon/README.md`.

      {:ok, client} = Bsdkrun.Client.from_env()
      {:ok, machines} = Bsdkrun.Client.list(client, true)
      {:ok, %{exit_code: 0, output: out}} = Bsdkrun.Client.exec(client, "abc123", ["uname", "-a"])

  `Bsdkrun.Client.new/1` builds a client lazily — no connection is made until
  the first call. Queries and mutations go straight over HTTP
  (`Bsdkrun.GraphQL`, built on `:httpc`); anything that streams (`exec/4`,
  `shell/3`, `follow_logs/3`, `subscribe/4`) shares one
  `graphql-transport-ws` socket per `{url, token}` pair — a
  `Bsdkrun.GraphQLSocket` GenServer, started lazily under
  `Bsdkrun.Client.SocketSupervisor` on first use and found again via
  `Bsdkrun.Client.Registry` (see `ensure_conn/1`). A `%Client{}` itself stays
  a plain, immutable struct — callers never see the GenServer.

  ## Live output: messages, not callbacks, by default

  `follow_logs/3` and `shell/3` deliver output by sending messages to the
  calling process's mailbox — `{:bsdkrun_logs, subscription_id, event}` and
  `{:bsdkrun_shell, session_id, event}}` respectively, where `event` is
  `{:data, binary}`, `{:exit, exit_code}`, `{:error, %Bsdkrun.Error{}}` or
  `:complete`. That is the idiomatic default for this codebase — every other
  blocking call in this SDK (`Bsdkrun.Sandbox.exec/3` included) already
  returns to an Elixir process, so a mailbox is the natural sink, no
  process-owned closure required. Pass `on_data: fn id, event -> ... end` in
  `opts` to receive a callback instead of messages (and `owner: pid` to
  target a different process's mailbox than the caller's). The escape hatch
  `subscribe/4` follows the same convention with `{:bsdkrun_subscription, id,
  event}`, `event` being the *raw* `{:next, data} | {:error, _} | :complete`.

  `exec/4` is built on the same `shellOutput` subscription internally, but
  blocks the calling process until the command exits (a `receive` loop, not
  a `GenServer.call` — see `await_exec/3`), matching this SDK's otherwise
  synchronous feel (`Bsdkrun.Sandbox.exec/3` blocks too).

  Every fallible function returns `{:ok, value} | {:error, %Bsdkrun.Error{}}`;
  `list/2` and `get/2` (mirroring `Bsdkrun.Sandbox`) and `from_env/0` also
  have bang counterparts that unwrap or raise.
  """

  alias Bsdkrun.{Error, GraphQL, GraphQLSocket, Types}
  alias Bsdkrun.Types.{CommandResult, SandboxInfo}

  @type t :: %__MODULE__{url: String.t(), token: String.t()}
  defstruct [:url, :token]

  @url_env "BSDKRUN_URL"
  @token_env "BSDKRUN_TOKEN"

  defmodule Subscription do
    @moduledoc """
    A handle to a live GraphQL subscription — returned by
    `Bsdkrun.Client.subscribe/4` and `Bsdkrun.Client.follow_logs/3`.
    """

    @type t :: %__MODULE__{ws_pid: pid(), id: String.t()}
    defstruct [:ws_pid, :id]

    @doc "Cancel the subscription. Idempotent."
    @spec cancel(t()) :: :ok
    def cancel(%__MODULE__{ws_pid: ws_pid, id: id}), do: GraphQLSocket.unsubscribe(ws_pid, id)
  end

  defmodule Shell do
    @moduledoc """
    A live interactive shell/exec session, opened by `Bsdkrun.Client.shell/3`.
    Output is delivered exactly as configured on `shell/3` (mailbox messages
    by default, or the `on_data` callback); drive the session with
    `write/2`, `resize/3` and `close/1`.
    """

    alias Bsdkrun.{Error, GraphQLSocket}

    @type t :: %__MODULE__{
            client: Bsdkrun.Client.t(),
            id: String.t(),
            machine_id: String.t(),
            ws_pid: pid(),
            sub_id: String.t()
          }

    defstruct [:client, :id, :machine_id, :ws_pid, :sub_id]

    @doc "Send input bytes (keystrokes) to the session."
    @spec write(t(), binary()) :: :ok | {:error, Error.t()}
    def write(%__MODULE__{client: client, id: id}, data) do
      mutation =
        "mutation($sessionId: String!, $data: String!) { sendShellInput(sessionId: $sessionId, dataBase64: $data) }"

      with {:ok, _data} <- Bsdkrun.Client.request(client, mutation, %{sessionId: id, data: Base.encode64(data)}) do
        :ok
      end
    end

    @doc "Resize the session's pty, so full-screen programs in the guest redraw."
    @spec resize(t(), non_neg_integer(), non_neg_integer()) :: :ok | {:error, Error.t()}
    def resize(%__MODULE__{client: client, id: id}, rows, cols) do
      mutation =
        "mutation($sessionId: String!, $rows: Int!, $cols: Int!) { resizeShell(sessionId: $sessionId, rows: $rows, cols: $cols) }"

      with {:ok, _data} <- Bsdkrun.Client.request(client, mutation, %{sessionId: id, rows: rows, cols: cols}) do
        :ok
      end
    end

    @doc "Stop delivering output and close the session (idempotent)."
    @spec close(t()) :: :ok | {:error, Error.t()}
    def close(%__MODULE__{client: client, id: id, ws_pid: ws_pid, sub_id: sub_id}) do
      GraphQLSocket.unsubscribe(ws_pid, sub_id)
      mutation = "mutation($sessionId: String!) { closeShell(sessionId: $sessionId) }"

      with {:ok, _data} <- Bsdkrun.Client.request(client, mutation, %{sessionId: id}) do
        :ok
      end
    end
  end

  # --- construction / config -----------------------------------------------

  @doc "Build a client. Does not connect — connections are made lazily, on first use."
  @spec new(keyword()) :: t()
  def new(opts) do
    url = Keyword.fetch!(opts, :url)
    token = Keyword.fetch!(opts, :token)
    %__MODULE__{url: normalize_url(url), token: token}
  end

  @doc """
  Build a client from `BSDKRUN_URL` / `BSDKRUN_TOKEN`. `BSDKRUN_URL` unset is
  an error; `BSDKRUN_URL` set without `BSDKRUN_TOKEN` is also an error — this
  never silently proceeds unauthenticated (mirrors `daemon/src/client.rs`'s
  `RemoteConfig::from_env` for the gRPC client; different env vars, same
  philosophy, since a GraphQL endpoint is a different port and URL shape).
  """
  @spec from_env() :: {:ok, t()} | {:error, Error.t()}
  def from_env() do
    case present_env(@url_env) do
      nil ->
        {:error, Error.config_error("#{@url_env} is not set")}

      url ->
        case present_env(@token_env) do
          nil -> {:error, Error.config_error("#{@url_env} is set but #{@token_env} is not")}
          token -> {:ok, new(url: url, token: token)}
        end
    end
  end

  @doc "Like `from_env/0`, but returns the client or raises `Bsdkrun.Error`."
  @spec from_env!() :: t()
  def from_env!(), do: unwrap!(from_env())

  @doc """
  Normalize what a person actually pastes into the daemon's GraphQL endpoint
  URL: trim, default to `http://` when no scheme is given, strip trailing
  slashes, and append `/graphql` unless it is already there. Mirrors
  `web/src/lib/connection.ts`'s `normalizeUrl`.
  """
  @spec normalize_url(String.t()) :: String.t()
  def normalize_url(input) do
    s = String.trim(input)

    if s == "" do
      s
    else
      s = if Regex.match?(~r/^https?:\/\//i, s), do: s, else: "http://" <> s
      s = Regex.replace(~r/\/+$/, s, "")
      if Regex.match?(~r/\/graphql$/i, s), do: s, else: s <> "/graphql"
    end
  end

  # --- escape hatch ----------------------------------------------------------

  @doc "Run a raw query or mutation. Returns `{:ok, data}` (the response's `data` field) or `{:error, %Bsdkrun.Error{}}`."
  @spec request(t(), String.t(), map()) :: {:ok, term()} | {:error, Error.t()}
  def request(client, query, variables \\ %{}), do: gql(client, query, variables)

  @doc """
  Start a raw subscription. `opts[:on_data]` (arity 2, `fn id, event -> end`)
  receives `{:next, data} | {:error, %Bsdkrun.Error{}} | :complete`; with no
  callback, the same arrives as `{:bsdkrun_subscription, id, event}` messages
  to `opts[:owner]` (default: the calling process). Returns a
  `Subscription.t()` — cancel it with `Subscription.cancel/1`.
  """
  @spec subscribe(t(), String.t(), map(), keyword()) :: {:ok, Subscription.t()} | {:error, Error.t()}
  def subscribe(client, query, variables \\ %{}, opts \\ []) do
    owner = Keyword.get(opts, :owner, self())
    on_data = Keyword.get(opts, :on_data)

    handler = fn sub_id, event -> deliver(on_data, owner, :bsdkrun_subscription, sub_id, event) end

    with {:ok, {ws_pid, sub_id}} <- raw_subscribe(client, query, variables, handler) do
      {:ok, %Subscription{ws_pid: ws_pid, id: sub_id}}
    end
  end

  # --- lifecycle / listing ---------------------------------------------------

  @machine_fields "id name image kind command status running exitCode pid detached " <>
                     "cpus mem volume stateDir createdAt finishedAt network netIp ports { bind host guest }"

  @doc "List machines. `all: true` includes exited ones (default: running only)."
  @spec list(t(), boolean()) :: {:ok, [SandboxInfo.t()]} | {:error, Error.t()}
  def list(client, all \\ false) do
    query = "query($all: Boolean!) { machines(all: $all) { #{@machine_fields} } }"

    with {:ok, data} <- gql(client, query, %{all: all}) do
      {:ok, Enum.map(data["machines"], &Types.sandbox_info_from_graphql/1)}
    end
  end

  @doc "Like `list/2`, but returns the list or raises `Bsdkrun.Error`."
  @spec list!(t(), boolean()) :: [SandboxInfo.t()]
  def list!(client, all \\ false), do: unwrap!(list(client, all))

  @doc "Fetch a single machine by id or name (a unique prefix is enough), or `nil` if there is no such machine."
  @spec get(t(), String.t()) :: {:ok, SandboxInfo.t() | nil} | {:error, Error.t()}
  def get(client, id) do
    query = "query($id: String!) { machine(id: $id) { #{@machine_fields} } }"

    with {:ok, data} <- gql(client, query, %{id: id}) do
      case data["machine"] do
        nil -> {:ok, nil}
        row -> {:ok, Types.sandbox_info_from_graphql(row)}
      end
    end
  end

  @doc "Like `get/2`, but returns the machine (or `nil`) or raises `Bsdkrun.Error`."
  @spec get!(t(), String.t()) :: SandboxInfo.t() | nil
  def get!(client, id), do: unwrap!(get(client, id))

  @doc "Stop the machine (BSD guests clean-poweroff; Linux is SIGTERM'd)."
  @spec stop(t(), String.t()) :: {:ok, CommandResult.t()} | {:error, Error.t()}
  def stop(client, id) do
    mutation = "mutation($id: String!) { stopMachine(id: $id) { exitCode stdout stderr } }"
    with {:ok, data} <- gql(client, mutation, %{id: id}), do: {:ok, Types.command_result_from_graphql(data["stopMachine"])}
  end

  @doc "Restart a stopped machine in place — same id, disk/rootfs, resources."
  @spec start(t(), String.t()) :: {:ok, CommandResult.t()} | {:error, Error.t()}
  def start(client, id) do
    mutation = "mutation($id: String!) { startMachine(id: $id) { exitCode stdout stderr } }"

    with {:ok, data} <- gql(client, mutation, %{id: id}) do
      {:ok, Types.command_result_from_graphql(data["startMachine"])}
    end
  end

  @doc "Remove one or more machines and their state. `force: true` stops them first if running."
  @spec remove(t(), String.t() | [String.t()], boolean()) :: {:ok, CommandResult.t()} | {:error, Error.t()}
  def remove(client, ids, force \\ false) do
    mutation =
      "mutation($ids: [String!]!, $force: Boolean!) { removeMachines(ids: $ids, force: $force) { exitCode stdout stderr } }"

    with {:ok, data} <- gql(client, mutation, %{ids: List.wrap(ids), force: force}) do
      {:ok, Types.command_result_from_graphql(data["removeMachines"])}
    end
  end

  @doc "Change a machine's recorded vCPU / RAM (`:cpus`, `:mem`). Applies on next `start/2`."
  @spec update(t(), String.t(), keyword()) :: {:ok, CommandResult.t()} | {:error, Error.t()}
  def update(client, id, opts \\ []) do
    o = to_map(opts)

    mutation =
      "mutation($id: String!, $cpus: Int, $mem: Int) { updateMachine(id: $id, cpus: $cpus, mem: $mem) { exitCode stdout stderr } }"

    variables = %{id: id, cpus: Map.get(o, :cpus), mem: Map.get(o, :mem)}

    with {:ok, data} <- gql(client, mutation, variables) do
      {:ok, Types.command_result_from_graphql(data["updateMachine"])}
    end
  end

  @doc "Snapshot a machine into a named flavor, like `docker commit`."
  @spec commit(t(), String.t(), String.t(), String.t()) :: {:ok, CommandResult.t()} | {:error, Error.t()}
  def commit(client, id, name, description \\ "") do
    mutation =
      "mutation($id: String!, $name: String!, $description: String!) { commitMachine(id: $id, name: $name, description: $description) { exitCode stdout stderr } }"

    with {:ok, data} <- gql(client, mutation, %{id: id, name: name, description: description}) do
      {:ok, Types.command_result_from_graphql(data["commitMachine"])}
    end
  end

  # --- logs --------------------------------------------------------------

  @doc "Read the machine's console log as a single string, one-shot. Pass `boot: true` for bsdkrun's own boot log."
  @spec logs(t(), String.t(), boolean()) :: {:ok, String.t()} | {:error, Error.t()}
  def logs(client, id, boot \\ false) do
    query = "query($id: String!, $boot: Boolean!) { machineLogs(id: $id, boot: $boot) }"
    with {:ok, data} <- gql(client, query, %{id: id, boot: boot}), do: {:ok, data["machineLogs"]}
  end

  @doc """
  Follow the machine's console log live, over the `machineLogs` subscription.
  Opts: `:boot` (bsdkrun's own boot log, default `false`), `:on_data`
  (`fn subscription_id, event -> end`) and `:owner` — see the module doc.
  With no `:on_data`, events arrive as `{:bsdkrun_logs, subscription_id,
  event}` messages, `event` being `{:data, binary} | {:exit, exit_code} |
  {:error, %Bsdkrun.Error{}} | :complete`.
  """
  @spec follow_logs(t(), String.t(), keyword()) :: {:ok, Subscription.t()} | {:error, Error.t()}
  def follow_logs(client, id, opts \\ []) do
    document = """
    subscription($id: String!, $follow: Boolean!, $boot: Boolean!) {
      machineLogs(id: $id, follow: $follow, boot: $boot) { dataBase64 exitCode }
    }
    """

    variables = %{id: id, follow: true, boot: Keyword.get(opts, :boot, false)}
    owner = Keyword.get(opts, :owner, self())
    on_data = Keyword.get(opts, :on_data)

    handler = fn sub_id, event ->
      case event do
        {:next, %{"machineLogs" => %{"dataBase64" => b64}}} when is_binary(b64) ->
          deliver(on_data, owner, :bsdkrun_logs, sub_id, {:data, Base.decode64!(b64)})

        {:next, %{"machineLogs" => %{"exitCode" => code}}} when not is_nil(code) ->
          deliver(on_data, owner, :bsdkrun_logs, sub_id, {:exit, code})

        {:next, _other} ->
          :ok

        {:error, err} ->
          deliver(on_data, owner, :bsdkrun_logs, sub_id, {:error, err})

        :complete ->
          deliver(on_data, owner, :bsdkrun_logs, sub_id, :complete)
      end
    end

    with {:ok, {ws_pid, sub_id}} <- raw_subscribe(client, document, variables, handler) do
      {:ok, %Subscription{ws_pid: ws_pid, id: sub_id}}
    end
  end

  # --- booting ---------------------------------------------------------------
  #
  # Always detached: the daemon outlives any one request, so every one of
  # these returns the new machine id. Field names are transcribed from
  # daemon/src/graphql.rs's Run*Input structs (async-graphql camelCases them
  # on the wire), not guessed.

  @doc "Boot a Linux (OCI) machine. `opts` maps to `RunLinuxInput`; see the module doc for the input shape."
  @spec run_linux(t(), keyword() | map()) :: {:ok, String.t()} | {:error, Error.t()}
  def run_linux(client, opts) do
    o = to_map(opts)

    input = %{
      image: fetch!(o, :image),
      cpus: Map.get(o, :cpus),
      mem: Map.get(o, :mem),
      net: net_input(Map.get(o, :net)),
      volume: Map.get(o, :volume),
      mounts: Map.get(o, :mounts, []),
      env: env_list(Map.get(o, :env, [])),
      entrypoint: Map.get(o, :entrypoint),
      initramfs: Map.get(o, :initramfs, false),
      kernel: Map.get(o, :kernel),
      kernelVersion: Map.get(o, :kernel_version),
      console: Map.get(o, :console),
      repo: Map.get(o, :repo),
      command: command_list(Map.get(o, :command, []))
    }

    mutation = "mutation($input: RunLinuxInput!) { runLinux(input: $input) }"
    with {:ok, data} <- gql(client, mutation, %{input: input}), do: {:ok, data["runLinux"]}
  end

  @doc "Boot a FreeBSD/NetBSD machine. `opts` maps to `RunBsdInput`, `:os` being `:freebsd` or `:netbsd`."
  @spec run_bsd(t(), keyword() | map()) :: {:ok, String.t()} | {:error, Error.t()}
  def run_bsd(client, opts) do
    o = to_map(opts)

    input = %{
      os: bsd_os(fetch!(o, :os)),
      version: Map.get(o, :version),
      cpus: Map.get(o, :cpus),
      mem: Map.get(o, :mem),
      net: net_input(Map.get(o, :net)),
      volume: Map.get(o, :volume),
      persist: Map.get(o, :persist, false),
      force: Map.get(o, :force, false),
      firmware: Map.get(o, :firmware),
      attachDisk: Map.get(o, :attach_disk, []),
      diskSize: Map.get(o, :disk_size),
      repo: Map.get(o, :repo),
      command: command_list(Map.get(o, :command, []))
    }

    mutation = "mutation($input: RunBsdInput!) { runBsd(input: $input) }"
    with {:ok, data} <- gql(client, mutation, %{input: input}), do: {:ok, data["runBsd"]}
  end

  @doc "Boot a Nanos unikernel. `opts` maps to `RunNanosInput`. No agent — no `exec/4`/`commit/4`."
  @spec run_nanos(t(), keyword() | map()) :: {:ok, String.t()} | {:error, Error.t()}
  def run_nanos(client, opts) do
    o = to_map(opts)

    input = %{
      image: fetch!(o, :image),
      cpus: Map.get(o, :cpus),
      mem: Map.get(o, :mem),
      net: net_input(Map.get(o, :net)),
      kernel: Map.get(o, :kernel),
      cmdline: Map.get(o, :cmdline),
      persist: Map.get(o, :persist, false)
    }

    mutation = "mutation($input: RunNanosInput!) { runNanos(input: $input) }"
    with {:ok, data} <- gql(client, mutation, %{input: input}), do: {:ok, data["runNanos"]}
  end

  @doc "Boot a Unikraft unikernel. `opts` maps to `RunUnikraftInput`. No disk, no agent — no `exec/4`/`commit/4`."
  @spec run_unikraft(t(), keyword() | map()) :: {:ok, String.t()} | {:error, Error.t()}
  def run_unikraft(client, opts) do
    o = to_map(opts)

    input = %{
      path: Map.get(o, :path),
      cpus: Map.get(o, :cpus),
      mem: Map.get(o, :mem),
      net: net_input(Map.get(o, :net)),
      cmdline: Map.get(o, :cmdline),
      initramfs: Map.get(o, :initramfs),
      mounts: Map.get(o, :mounts, [])
    }

    mutation = "mutation($input: RunUnikraftInput!) { runUnikraft(input: $input) }"
    with {:ok, data} <- gql(client, mutation, %{input: input}), do: {:ok, data["runUnikraft"]}
  end

  @doc "Boot an OSv unikernel. `opts` maps to `RunOsvInput`. No agent — no `exec/4`/`commit/4`."
  @spec run_osv(t(), keyword() | map()) :: {:ok, String.t()} | {:error, Error.t()}
  def run_osv(client, opts) do
    o = to_map(opts)

    input = %{
      image: fetch!(o, :image),
      cpus: Map.get(o, :cpus),
      mem: Map.get(o, :mem),
      net: net_input(Map.get(o, :net)),
      cmdline: Map.get(o, :cmdline),
      disk: Map.get(o, :disk),
      noDisk: Map.get(o, :no_disk, false),
      attachDisk: Map.get(o, :attach_disk, []),
      gic: Map.get(o, :gic),
      persist: Map.get(o, :persist, false),
      volume: Map.get(o, :volume)
    }

    mutation = "mutation($input: RunOsvInput!) { runOsv(input: $input) }"
    with {:ok, data} <- gql(client, mutation, %{input: input}), do: {:ok, data["runOsv"]}
  end

  @doc "Boot a named flavor. `opts` maps to `RunFlavorInput`."
  @spec run_flavor(t(), keyword() | map()) :: {:ok, String.t()} | {:error, Error.t()}
  def run_flavor(client, opts) do
    o = to_map(opts)

    input = %{
      name: fetch!(o, :name),
      cpus: Map.get(o, :cpus),
      mem: Map.get(o, :mem),
      ports: Enum.map(Map.get(o, :ports, []), &port_str/1),
      volume: Map.get(o, :volume),
      repo: Map.get(o, :repo)
    }

    mutation = "mutation($input: RunFlavorInput!) { runFlavor(input: $input) }"
    with {:ok, data} <- gql(client, mutation, %{input: input}), do: {:ok, data["runFlavor"]}
  end

  # --- exec / interactive shell -----------------------------------------------
  #
  # Both open a shell session (`openShell`) and subscribe to its output
  # (`shellOutput`) — see daemon/README.md's "Interactive shells over
  # GraphQL": a subscription cannot carry input, so a terminal is a mutation
  # plus a subscription plus further mutations. `openShell` MUST happen
  # before the subscription starts, and did here: output is buffered by the
  # daemon from the moment the session opens, so nothing written in between
  # is lost.

  @doc """
  Run a command to completion and collect its output. Blocks the calling
  process until the command exits (`opts[:timeout]`, default 300_000ms).
  `command` is an argv list or a bare program name string. Opts: `:env`
  (a map or `"K=V"` list), `:rows`/`:cols` (pty size, default 24x80),
  `:timeout`.
  """
  @spec exec(t(), String.t(), String.t() | [String.t()], keyword()) ::
          {:ok, %{exit_code: integer() | nil, output: binary()}} | {:error, Error.t()}
  def exec(client, machine_id, command, opts \\ []) do
    timeout = Keyword.get(opts, :timeout, 300_000)

    variables = %{
      machineId: machine_id,
      command: command_list(command),
      env: env_list(Keyword.get(opts, :env, [])),
      rows: Keyword.get(opts, :rows, 24),
      cols: Keyword.get(opts, :cols, 80)
    }

    open_mutation = """
    mutation($machineId: String!, $command: [String!]!, $env: [String!]!, $rows: Int!, $cols: Int!) {
      openShell(machineId: $machineId, command: $command, env: $env, rows: $rows, cols: $cols) { id }
    }
    """

    with {:ok, data} <- gql(client, open_mutation, variables) do
      session_id = data["openShell"]["id"]
      result = await_exec(client, session_id, timeout)

      # Idempotent, and called regardless of how await_exec ended (success,
      # error, or timeout) — a session left open leaks a pty on the daemon.
      _ = gql(client, "mutation($sessionId: String!) { closeShell(sessionId: $sessionId) }", %{sessionId: session_id})

      result
    end
  end

  @doc """
  Open a live interactive session on a machine (or run `opts[:command]` if
  given, instead of a login shell) and return a `Shell.t()` handle. Output is
  delivered as it arrives, exactly as `follow_logs/3` does: `opts[:on_data]`
  (`fn session_id, event -> end`) or, with no callback, `{:bsdkrun_shell,
  session_id, event}` messages to `opts[:owner]` (default: the caller).
  `event` is `{:data, binary} | {:exit, exit_code} | {:error, _} | :complete`.
  """
  @spec shell(t(), String.t(), keyword()) :: {:ok, Shell.t()} | {:error, Error.t()}
  def shell(client, machine_id, opts \\ []) do
    variables = %{
      machineId: machine_id,
      command: command_list(Keyword.get(opts, :command, [])),
      env: env_list(Keyword.get(opts, :env, [])),
      rows: Keyword.get(opts, :rows, 24),
      cols: Keyword.get(opts, :cols, 80)
    }

    open_mutation = """
    mutation($machineId: String!, $command: [String!]!, $env: [String!]!, $rows: Int!, $cols: Int!) {
      openShell(machineId: $machineId, command: $command, env: $env, rows: $rows, cols: $cols) {
        id machineId finished truncated
      }
    }
    """

    with {:ok, data} <- gql(client, open_mutation, variables) do
      session_id = data["openShell"]["id"]
      owner = Keyword.get(opts, :owner, self())
      on_data = Keyword.get(opts, :on_data)

      handler = fn _sub_id, event ->
        case event do
          {:next, %{"shellOutput" => %{"dataBase64" => b64}}} when is_binary(b64) ->
            deliver(on_data, owner, :bsdkrun_shell, session_id, {:data, Base.decode64!(b64)})

          {:next, %{"shellOutput" => %{"exitCode" => code}}} when not is_nil(code) ->
            deliver(on_data, owner, :bsdkrun_shell, session_id, {:exit, code})

          {:next, _other} ->
            :ok

          {:error, err} ->
            deliver(on_data, owner, :bsdkrun_shell, session_id, {:error, err})

          :complete ->
            deliver(on_data, owner, :bsdkrun_shell, session_id, :complete)
        end
      end

      with {:ok, {ws_pid, sub_id}} <- raw_subscribe(client, shell_output_document(), %{sessionId: session_id}, handler) do
        {:ok, %Shell{client: client, id: session_id, machine_id: machine_id, ws_pid: ws_pid, sub_id: sub_id}}
      end
    end
  end

  defp await_exec(client, session_id, timeout) do
    with {:ok, ws_pid} <- ensure_conn(client) do
      me = self()

      handler = fn _sub_id, event ->
        case event do
          {:next, %{"shellOutput" => %{"dataBase64" => b64}}} when is_binary(b64) ->
            send(me, {:bsdkrun_exec, :data, Base.decode64!(b64)})

          {:next, %{"shellOutput" => %{"exitCode" => code}}} when not is_nil(code) ->
            send(me, {:bsdkrun_exec, :exit, code})

          {:next, _other} ->
            :ok

          {:error, err} ->
            send(me, {:bsdkrun_exec, :error, err})

          :complete ->
            send(me, {:bsdkrun_exec, :complete, nil})
        end
      end

      case GraphQLSocket.subscribe(ws_pid, shell_output_document(), %{sessionId: session_id}, handler) do
        {:ok, sub_id} ->
          result = collect_exec(<<>>, timeout)
          GraphQLSocket.unsubscribe(ws_pid, sub_id)
          result

        {:error, _reason} = error ->
          error
      end
    end
  end

  defp collect_exec(acc, timeout) do
    receive do
      {:bsdkrun_exec, :data, bytes} -> collect_exec(acc <> bytes, timeout)
      {:bsdkrun_exec, :exit, code} -> {:ok, %{exit_code: code, output: acc}}
      {:bsdkrun_exec, :error, err} -> {:error, err}
      {:bsdkrun_exec, :complete, _payload} -> {:ok, %{exit_code: nil, output: acc}}
    after
      timeout -> {:error, Error.graphql_error("exec timed out after #{timeout}ms")}
    end
  end

  defp shell_output_document,
    do: "subscription($sessionId: String!) { shellOutput(sessionId: $sessionId) { dataBase64 exitCode } }"

  # --- transport plumbing ----------------------------------------------------

  defp gql(%__MODULE__{url: url, token: token}, query, variables), do: GraphQL.request(url, token, query, variables)

  defp raw_subscribe(client, query, variables, handler) do
    with {:ok, ws_pid} <- ensure_conn(client),
         {:ok, sub_id} <- GraphQLSocket.subscribe(ws_pid, query, variables, handler) do
      {:ok, {ws_pid, sub_id}}
    end
  end

  # Find (or lazily start, under Bsdkrun.Client.SocketSupervisor) the shared
  # GraphQLSocket for this client's {url, token} — see Bsdkrun.Application.
  defp ensure_conn(%__MODULE__{url: url, token: token} = client) do
    key = {url, token}

    case Registry.lookup(Bsdkrun.Client.Registry, key) do
      [{pid, _value}] -> {:ok, pid}
      [] -> start_conn(client, key)
    end
  end

  defp start_conn(client, key) do
    ws = ws_url(client.url)

    # restart: :temporary — a GraphQLSocket is a one-shot, ephemeral
    # connection, not a supervised worker to keep alive: it deliberately
    # `{:stop, :normal, ...}`s itself once its last subscription is gone, and
    # its `init/1` `{:stop, reason}`s on a failed connect. With the
    # `use GenServer` default (:permanent), the DynamicSupervisor would
    # reinterpret both of those as crashes to restart — reconnecting a dead
    # socket with stale args in a loop. ensure_conn/1 is what should decide
    # whether a fresh socket gets started, on the next call that needs one.
    child_spec = %{
      id: {GraphQLSocket, key},
      start: {GraphQLSocket, :start_link, [{ws, client.token, key}]},
      restart: :temporary
    }

    case DynamicSupervisor.start_child(Bsdkrun.Client.SocketSupervisor, child_spec) do
      {:ok, pid} ->
        {:ok, pid}

      {:error, {:already_started, pid}} ->
        {:ok, pid}

      {:error, reason} ->
        {:error, Error.graphql_error("cannot reach the bsdkrun daemon at #{client.url} — #{inspect(reason)}")}
    end
  end

  # http(s):// -> ws(s)://, trailing slashes stripped, "/ws" appended —
  # mirrors web/src/lib/graphql.ts's wsUrl().
  defp ws_url(url) do
    {scheme, rest} =
      cond do
        String.starts_with?(url, "https://") -> {"wss://", String.replace_prefix(url, "https://", "")}
        String.starts_with?(url, "http://") -> {"ws://", String.replace_prefix(url, "http://", "")}
        true -> {"ws://", url}
      end

    scheme <> String.trim_trailing(rest, "/") <> "/ws"
  end

  defp deliver(nil, owner, tag, id, payload), do: send(owner, {tag, id, payload})
  defp deliver(callback, _owner, _tag, id, payload) when is_function(callback, 2), do: callback.(id, payload)

  # --- option shaping ----------------------------------------------------

  defp to_map(opts) when is_map(opts), do: opts

  defp to_map(opts) when is_list(opts) do
    if Keyword.keyword?(opts) do
      Map.new(opts)
    else
      raise ArgumentError, "opts must be a keyword list or map, got: #{inspect(opts)}"
    end
  end

  defp fetch!(o, key) do
    case Map.get(o, key) do
      nil -> raise ArgumentError, "missing required option #{inspect(key)}"
      value -> value
    end
  end

  defp env_list(nil), do: []
  defp env_list(env) when is_map(env), do: Enum.map(env, fn {k, v} -> "#{k}=#{v}" end)

  defp env_list(env) when is_list(env) do
    if Keyword.keyword?(env) do
      Enum.map(env, fn {k, v} -> "#{k}=#{v}" end)
    else
      Enum.map(env, &to_string/1)
    end
  end

  defp command_list(nil), do: []
  defp command_list(cmd) when is_list(cmd), do: cmd
  defp command_list(cmd) when is_binary(cmd), do: [cmd]

  defp net_input(nil), do: nil

  defp net_input(net) do
    n = to_map(net)

    %{
      noNet: Map.get(n, :no_net, false),
      ports: Enum.map(Map.get(n, :ports, []), &port_str/1),
      mac: Map.get(n, :mac),
      network: Map.get(n, :network),
      name: Map.get(n, :name)
    }
  end

  defp port_str(p) when is_binary(p), do: p
  defp port_str({host, guest}), do: "#{host}:#{guest}"

  defp port_str(p) when is_map(p) or is_list(p) do
    m = to_map(p)
    "#{Map.get(m, :host)}:#{Map.get(m, :guest)}"
  end

  defp bsd_os(:freebsd), do: "FREEBSD"
  defp bsd_os(:netbsd), do: "NETBSD"
  defp bsd_os(os) when is_binary(os), do: String.upcase(os)

  defp present_env(name) do
    case System.get_env(name) do
      nil ->
        nil

      value ->
        case String.trim(value) do
          "" -> nil
          trimmed -> trimmed
        end
    end
  end

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, error}), do: raise(error)
end
