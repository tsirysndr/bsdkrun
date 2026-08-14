defmodule Bsdkrun.Sandbox do
  @moduledoc """
  A handle to a running (or stopped) `bsdkrun` microVM. Create one with
  `create/1`, reconnect with `get/1`, or enumerate with `list/1`.

      {:ok, box} = Bsdkrun.Sandbox.create(os: :linux, image: "alpine")
      {:ok, res} = Bsdkrun.Sandbox.exec(box, ["uname", "-a"])
      :ok = Bsdkrun.Sandbox.stop(box)

  Every fallible function returns `{:ok, value}` or `{:error, %Bsdkrun.Error{}}`,
  with a bang counterpart that unwraps or raises. Functions that act on a
  machine accept either a `%Bsdkrun.Sandbox{}` struct or a bare id string.

  The bang lifecycle functions (`stop!/1`, `start!/1`, `remove!/2`,
  `update!/2`, `connect_network!/2`, `disconnect_network!/1`) return `ref`
  itself — not `:ok` — so they chain with `|>`:

      Bsdkrun.create!(os: :linux, image: "alpine")
      |> Bsdkrun.exec!(["apk", "add", "curl"])
      |> Bsdkrun.stop!()

  `exec!/3`, `logs!/2`, `status!/1`, `ssh_setup!/2` and `tailscale_up!/2`
  return their unwrapped value instead (a `Result`, a string, ...), since
  that value — not the sandbox — is the point of calling them. Reach for
  `tap/2` to run one mid-chain without losing the sandbox:

      Bsdkrun.create!(os: :linux, image: "alpine")
      |> tap(&(Bsdkrun.exec!(&1, ["uname", "-a"]) |> Bsdkrun.Types.Result.text() |> IO.puts()))
      |> Bsdkrun.stop!()
  """

  alias Bsdkrun.{Args, Cli, Error}
  alias Bsdkrun.Types
  alias Bsdkrun.Types.{Result, SandboxInfo}

  @type t :: %__MODULE__{id: String.t(), ssh_port: non_neg_integer() | nil}

  defstruct [:id, :ssh_port]

  @typedoc "A sandbox handle or a bare machine id."
  @type ref :: t() | String.t()

  @id_re ~r/^[0-9a-f]{6,}$/
  @ssh_port_re ~r/ssh -p (\d+)/

  defmodule Builder do
    @moduledoc """
    A pipe-friendly, pure builder for `Bsdkrun.Sandbox.create/1`'s options —
    volumes, mounts, ports and the like are only ever bound at boot (the
    `bsdkrun` CLI has no runtime "attach" for them), so building the spec up
    with `with_*/2` calls before `create/1` is how a volume or network gets
    attached "by pipe":

        Bsdkrun.Sandbox.new(os: :linux, image: "alpine")
        |> Bsdkrun.Sandbox.with_volume("web")
        |> Bsdkrun.Sandbox.with_network("devnet")
        |> Bsdkrun.Sandbox.with_port("8080:80")
        |> Bsdkrun.Sandbox.create!()

    Nothing is sent to `bsdkrun` until `create/1` (or `create!/1`) runs.
    """

    @type t :: %__MODULE__{opts: map()}
    defstruct opts: %{}
  end

  # --- construction / discovery ----------------------------------------------

  @doc """
  Boot a new microVM (detached) and return a handle to it.

  `opts` is a keyword list, a map, or a `Bsdkrun.Sandbox.Builder` (see
  `new/1`), discriminated on `:os` (`:linux`, `:freebsd`, `:netbsd`,
  `:firmware`, `:kernel`), plus the per-kind keys accepted by
  `Bsdkrun.Args`. `:log_level` (default `1`) controls boot diagnostics.
  """
  @spec create(keyword() | map() | Builder.t()) :: {:ok, t()} | {:error, Error.t()}
  def create(%Builder{opts: opts}), do: create(opts)

  def create(opts) do
    log_level = opt(opts, :log_level, 1)
    res = Cli.run(Args.build_create(opts), log_level: log_level)

    cond do
      res.exit_code != 0 ->
        {:error, Error.command_failed(res.exit_code, res.stdout, res.stderr, "bsdkrun create")}

      id = parse_id(res.stdout) ->
        {:ok, %__MODULE__{id: id, ssh_port: parse_ssh_port(res.stderr)}}

      true ->
        {:error,
         Error.command_failed(
           res.exit_code,
           res.stdout,
           res.stderr,
           "bsdkrun create (no machine id in output)"
         )}
    end
  end

  @doc "Like `create/1`, but returns the sandbox or raises `Bsdkrun.Error`."
  @spec create!(keyword() | map() | Builder.t()) :: t()
  def create!(opts), do: unwrap!(create(opts))

  @doc """
  Start building `create/1` options via `|>` (`with_*/2` calls), finished
  with `create/1` or `create!/1`. See `Bsdkrun.Sandbox.Builder`.
  """
  @spec new(keyword() | map()) :: Builder.t()
  def new(opts \\ []), do: %Builder{opts: Args.normalize(opts)}

  @doc "Set the persistent volume to boot from/into (`-v`)."
  @spec with_volume(Builder.t(), String.t()) :: Builder.t()
  def with_volume(%Builder{} = b, name), do: put_opt(b, :volume, name)

  @doc "Add a host<->guest mount (repeatable) — `\"~/project:/src\"` or `\"~/data:/data:ro\"`."
  @spec with_mount(Builder.t(), String.t()) :: Builder.t()
  def with_mount(%Builder{} = b, spec), do: append_opt(b, :mounts, spec)

  @doc "Add several mounts at once — see `with_mount/2`."
  @spec with_mounts(Builder.t(), [String.t()]) :: Builder.t()
  def with_mounts(%Builder{} = b, specs), do: Enum.reduce(specs, b, &with_mount(&2, &1))

  @doc "Join a global network on boot (like `--network`; see `Bsdkrun.Networks`)."
  @spec with_network(Builder.t(), String.t()) :: Builder.t()
  def with_network(%Builder{} = b, network), do: put_net(b, :network, network)

  @doc "Add a host<->guest port forward (repeatable) — `\"8080:80\"`, `{2222, 22}`, or `%{host: 2222, guest: 22}`."
  @spec with_port(Builder.t(), String.t() | {integer(), integer()} | map()) :: Builder.t()
  def with_port(%Builder{} = b, port), do: append_net(b, :ports, port)

  @doc "Add several port forwards at once — see `with_port/2`."
  @spec with_ports(Builder.t(), [String.t() | {integer(), integer()} | map()]) :: Builder.t()
  def with_ports(%Builder{} = b, ports), do: Enum.reduce(ports, b, &with_port(&2, &1))

  @doc "Attach an extra raw disk as virtio-blk (repeatable) — a path, optionally `\"path:ro\"`."
  @spec with_disk(Builder.t(), String.t()) :: Builder.t()
  def with_disk(%Builder{} = b, path), do: append_opt(b, :attach_disk, path)

  @doc "Set the vCPU count."
  @spec with_cpus(Builder.t(), pos_integer()) :: Builder.t()
  def with_cpus(%Builder{} = b, n), do: put_opt(b, :cpus, n)

  @doc "Set the guest RAM, in MiB."
  @spec with_mem(Builder.t(), pos_integer()) :: Builder.t()
  def with_mem(%Builder{} = b, mb), do: put_opt(b, :mem, mb)

  @doc "Set the machine's name."
  @spec with_name(Builder.t(), String.t()) :: Builder.t()
  def with_name(%Builder{} = b, name), do: put_opt(b, :name, name)

  @doc "Set the command run after `--` (Linux / firmware / kernel guests)."
  @spec with_command(Builder.t(), [String.t()]) :: Builder.t()
  def with_command(%Builder{} = b, cmd), do: put_opt(b, :command, cmd)

  @doc "Set an arbitrary `create/1` option — the escape hatch for anything not wrapped above."
  @spec with_opt(Builder.t(), atom(), term()) :: Builder.t()
  def with_opt(%Builder{} = b, key, value) when is_atom(key), do: put_opt(b, key, value)

  @doc "Reconnect to an existing machine by id (a unique prefix is enough)."
  @spec get(String.t()) :: {:ok, t()} | {:error, Error.t()}
  def get(id) when is_binary(id) do
    with {:ok, all} <- list(all: true) do
      case Enum.find(all, &(&1.id == id or String.starts_with?(&1.id, id) or &1.name == id)) do
        nil -> {:error, Error.sandbox_not_found(id)}
        match -> {:ok, %__MODULE__{id: match.id}}
      end
    end
  end

  @doc "Like `get/1`, but returns the sandbox or raises `Bsdkrun.Error`."
  @spec get!(String.t()) :: t()
  def get!(id), do: unwrap!(get(id))

  @doc """
  List machines. `all: true` includes exited ones (default: running only).
  """
  @spec list(keyword()) :: {:ok, [SandboxInfo.t()]} | {:error, Error.t()}
  def list(opts \\ []) do
    args =
      if Keyword.get(opts, :all, false), do: ["ps", "--json", "--all"], else: ["ps", "--json"]

    with {:ok, res} <- Cli.checked(args, "bsdkrun ps") do
      {:ok, decode_rows(res.stdout, &Types.sandbox_info/1)}
    end
  end

  @doc "Like `list/1`, but returns the list or raises `Bsdkrun.Error`."
  @spec list!(keyword()) :: [SandboxInfo.t()]
  def list!(opts \\ []), do: unwrap!(list(opts))

  # --- exec / logs ------------------------------------------------------------

  @doc """
  Run a command in the guest through its exec agent.

  `command` is an argv list, or a bare program name string (with `:args`). Opts:

    * `:args`          — args when `command` is a bare program name.
    * `:env`           — environment variables (`-e K=V`).
    * `:tty`           — allocate a pseudo-TTY (`-t`).
    * `:stdin`         — data piped to the command's stdin.
    * `:cwd`           — working directory (emulated via `sh -c 'cd …'`).
    * `:throw_on_error`— return `{:error, _}` on a non-zero exit (default false).
    * `:log_level`     — per-command bsdkrun log level (default 0).
    * `:on_stdout`     — function called with stdout chunks as they arrive.
    * `:on_stderr`     — function called with stderr chunks as they arrive.

  Returns `{:ok, %Bsdkrun.Types.Result{}}`. With `throw_on_error: true`, a
  non-zero exit yields `{:error, %Bsdkrun.Error{}}` instead.
  """
  @spec exec(ref(), String.t() | [String.t()], keyword()) ::
          {:ok, Result.t()} | {:error, Error.t()}
  def exec(ref, command, opts \\ []) do
    argv =
      case command do
        list when is_list(list) -> list
        str when is_binary(str) -> [str | Keyword.get(opts, :args, [])]
      end

    argv =
      case Keyword.get(opts, :cwd) do
        nil ->
          argv

        cwd ->
          ["/bin/sh", "-c", ~s(cd "$1" && shift && exec "$@"), "sh", cwd] ++ argv
      end

    env = Keyword.get(opts, :env, %{})

    args =
      ["exec"]
      |> then(fn a -> if Keyword.get(opts, :tty, false), do: a ++ ["-t"], else: a end)
      |> then(fn a ->
        a ++ Enum.flat_map(env, fn {k, v} -> ["-e", "#{k}=#{v}"] end)
      end)
      |> Kernel.++([id(ref) | argv])

    res =
      Cli.run(args,
        stdin: Keyword.get(opts, :stdin),
        log_level: Keyword.get(opts, :log_level, 0),
        on_stdout: Keyword.get(opts, :on_stdout),
        on_stderr: Keyword.get(opts, :on_stderr)
      )

    result = %Result{
      stdout: res.stdout,
      stderr: res.stderr,
      exit_code: res.exit_code,
      command: "exec " <> Enum.join(argv, " ")
    }

    if Keyword.get(opts, :throw_on_error, false) and res.exit_code != 0 do
      {:error, Error.command_failed(res.exit_code, res.stdout, res.stderr, result.command)}
    else
      {:ok, result}
    end
  end

  @doc "Like `exec/3`, but returns the `Result` or raises `Bsdkrun.Error`."
  @spec exec!(ref(), String.t() | [String.t()], keyword()) :: Result.t()
  def exec!(ref, command, opts \\ []), do: unwrap!(exec(ref, command, opts))

  @doc "Read the machine's console log. Pass `boot: true` for bsdkrun's own boot log."
  @spec logs(ref(), keyword()) :: {:ok, String.t()} | {:error, Error.t()}
  def logs(ref, opts \\ []) do
    args =
      if Keyword.get(opts, :boot, false), do: ["logs", "--boot", id(ref)], else: ["logs", id(ref)]

    with {:ok, res} <- Cli.checked(args, "bsdkrun logs") do
      {:ok, res.stdout}
    end
  end

  @doc "Like `logs/2`, but returns the log or raises `Bsdkrun.Error`."
  @spec logs!(ref(), keyword()) :: String.t()
  def logs!(ref, opts \\ []), do: unwrap!(logs(ref, opts))

  # --- lifecycle --------------------------------------------------------------

  @doc "Stop the machine (BSD guests clean-poweroff; Linux is SIGTERM'd)."
  @spec stop(ref()) :: :ok | {:error, Error.t()}
  def stop(ref), do: lifecycle(["stop", id(ref)], "bsdkrun stop")

  @doc "Like `stop/1`, but raises on failure and returns `ref` (for chaining)."
  @spec stop!(ref()) :: ref()
  def stop!(ref), do: unwrap_ref!(ref, stop(ref))

  @doc "Restart a stopped machine in place — same id, disk/rootfs, resources."
  @spec start(ref()) :: :ok | {:error, Error.t()}
  def start(ref), do: lifecycle(["start", id(ref)], "bsdkrun start")

  @doc "Like `start/1`, but raises on failure and returns `ref` (for chaining)."
  @spec start!(ref()) :: ref()
  def start!(ref), do: unwrap_ref!(ref, start(ref))

  @doc "Remove the machine and its state. `force: true` stops it first if running."
  @spec remove(ref(), keyword()) :: :ok | {:error, Error.t()}
  def remove(ref, opts \\ []) do
    args =
      if Keyword.get(opts, :force, false), do: ["rm", "--force", id(ref)], else: ["rm", id(ref)]

    lifecycle(args, "bsdkrun rm")
  end

  @doc "Like `remove/2`, but raises on failure and returns `ref` (for chaining)."
  @spec remove!(ref(), keyword()) :: ref()
  def remove!(ref, opts \\ []), do: unwrap_ref!(ref, remove(ref, opts))

  @doc "Change the recorded vCPU / RAM (`:cpus`, `:mem`). Applies on next `start/1`."
  @spec update(ref(), keyword()) :: :ok | {:error, Error.t()}
  def update(ref, opts) do
    args =
      ["update", id(ref)]
      |> then(fn a ->
        if opts[:cpus] != nil, do: a ++ ["--cpus", to_string(opts[:cpus])], else: a
      end)
      |> then(fn a ->
        if opts[:mem] != nil, do: a ++ ["--mem", to_string(opts[:mem])], else: a
      end)

    lifecycle(args, "bsdkrun update")
  end

  @doc "Like `update/2`, but raises on failure and returns `ref` (for chaining)."
  @spec update!(ref(), keyword()) :: ref()
  def update!(ref, opts), do: unwrap_ref!(ref, update(ref, opts))

  @doc "Join or switch this machine to a global network. Applies on next `start/1`."
  @spec connect_network(ref(), String.t()) :: :ok | {:error, Error.t()}
  def connect_network(ref, network) do
    lifecycle(["network", "connect", id(ref), network], "bsdkrun network connect")
  end

  @doc "Like `connect_network/2`, but raises on failure and returns `ref` (for chaining)."
  @spec connect_network!(ref(), String.t()) :: ref()
  def connect_network!(ref, network), do: unwrap_ref!(ref, connect_network(ref, network))

  @doc "Detach this machine from its network. Applies on next `start/1`."
  @spec disconnect_network(ref()) :: :ok | {:error, Error.t()}
  def disconnect_network(ref) do
    lifecycle(["network", "disconnect", id(ref)], "bsdkrun network disconnect")
  end

  @doc "Like `disconnect_network/1`, but raises on failure and returns `ref` (for chaining)."
  @spec disconnect_network!(ref()) :: ref()
  def disconnect_network!(ref), do: unwrap_ref!(ref, disconnect_network(ref))

  # --- status -----------------------------------------------------------------

  @doc "Fetch this machine's current status row, or `nil` if it's gone."
  @spec status(ref()) :: {:ok, SandboxInfo.t() | nil} | {:error, Error.t()}
  def status(ref) do
    machine_id = id(ref)

    with {:ok, all} <- list(all: true) do
      {:ok, Enum.find(all, &(&1.id == machine_id))}
    end
  end

  @doc "Like `status/1`, but returns the row (or `nil`) or raises `Bsdkrun.Error`."
  @spec status!(ref()) :: SandboxInfo.t() | nil
  def status!(ref), do: unwrap!(status(ref))

  @doc "Whether the machine is currently running (false if it can't be found)."
  @spec running?(ref()) :: boolean()
  def running?(ref) do
    case status(ref) do
      {:ok, %SandboxInfo{running: running}} -> running
      _ -> false
    end
  end

  # --- agent conveniences -----------------------------------------------------

  @doc """
  Install SSH keys in the guest via the agent (`ssh setup`). With no `:key`,
  the CLI installs your local `~/.ssh/*.pub`. Opts: `:user`, `:key`
  (a literal key string or `.pub` path, or a list of them).
  """
  @spec ssh_setup(ref(), keyword()) :: {:ok, Result.t()} | {:error, Error.t()}
  def ssh_setup(ref, opts \\ []) do
    args =
      ["setup"]
      |> then(fn a -> if opts[:user], do: a ++ ["--user", opts[:user]], else: a end)
      |> Kernel.++(Enum.flat_map(key_list(opts[:key]), &["--key", &1]))

    agent(ref, "ssh", args)
  end

  @doc "Like `ssh_setup/2`, but returns the `Result` or raises `Bsdkrun.Error`."
  @spec ssh_setup!(ref(), keyword()) :: Result.t()
  def ssh_setup!(ref, opts \\ []), do: unwrap!(ssh_setup(ref, opts))

  @doc """
  Put the guest on your tailnet via the agent (`tailscale setup`). The
  `:authkey` is forwarded as `TS_AUTHKEY` (kept off the arg list). Opts:
  `:authkey`, `:hostname`, `:args` (passthrough).
  """
  @spec tailscale_up(ref(), keyword()) :: {:ok, Result.t()} | {:error, Error.t()}
  def tailscale_up(ref, opts \\ []) do
    args =
      ["setup"]
      |> then(fn a -> if opts[:hostname], do: a ++ ["--hostname", opts[:hostname]], else: a end)
      |> Kernel.++(opts[:args] || [])

    env = if opts[:authkey], do: %{"TS_AUTHKEY" => opts[:authkey]}, else: %{}
    agent(ref, "tailscale", args, env)
  end

  @doc "Like `tailscale_up/2`, but returns the `Result` or raises `Bsdkrun.Error`."
  @spec tailscale_up!(ref(), keyword()) :: Result.t()
  def tailscale_up!(ref, opts \\ []), do: unwrap!(tailscale_up(ref, opts))

  @doc """
  Attach an interactive shell to the machine, inheriting the current terminal.
  Blocks until the shell exits; returns its exit status.
  """
  @spec shell(ref()) :: integer()
  def shell(ref) do
    bin = Bsdkrun.Binary.resolve!()
    args = ["--log-level", "0", "shell", id(ref)]
    {_out, code} = System.cmd(bin, args, into: IO.stream(:stdio, :line))
    code
  end

  # --- internals --------------------------------------------------------------

  @doc "Extract the machine id from a sandbox struct or a bare id string."
  @spec id(ref()) :: String.t()
  def id(%__MODULE__{id: id}), do: id
  def id(id) when is_binary(id), do: id

  defp agent(ref, family, action, env \\ %{}) do
    res = Cli.run([family, id(ref) | action], env: env)

    if res.exit_code == 0 do
      {:ok,
       %Result{
         stdout: res.stdout,
         stderr: res.stderr,
         exit_code: res.exit_code,
         command: "#{family} #{Enum.join(action, " ")}"
       }}
    else
      {:error,
       Error.command_failed(
         res.exit_code,
         res.stdout,
         res.stderr,
         "#{family} #{Enum.join(action, " ")}"
       )}
    end
  end

  defp lifecycle(args, label) do
    case Cli.checked(args, label) do
      {:ok, _} -> :ok
      {:error, _} = err -> err
    end
  end

  defp parse_id(stdout) do
    stdout
    |> String.split("\n")
    |> Enum.map(&String.trim/1)
    |> Enum.filter(&Regex.match?(@id_re, &1))
    |> List.last()
  end

  defp parse_ssh_port(stderr) do
    case Regex.run(@ssh_port_re, stderr) do
      [_, port] -> String.to_integer(port)
      _ -> nil
    end
  end

  defp key_list(nil), do: []
  defp key_list(key) when is_binary(key), do: [key]
  defp key_list(keys) when is_list(keys), do: keys

  defp decode_rows(stdout, mapper) do
    stdout
    |> empty_to_array()
    |> Jason.decode!()
    |> Enum.map(mapper)
  end

  defp empty_to_array(stdout) do
    if String.trim(stdout) == "", do: "[]", else: stdout
  end

  defp opt(opts, key, default) when is_list(opts), do: Keyword.get(opts, key, default)
  defp opt(opts, key, default) when is_map(opts), do: Map.get(opts, key, default)

  # --- Builder internals -------------------------------------------------------

  defp put_opt(%Builder{opts: opts} = b, key, value), do: %{b | opts: Map.put(opts, key, value)}

  defp append_opt(%Builder{opts: opts} = b, key, value) do
    %{b | opts: Map.update(opts, key, [value], &(&1 ++ [value]))}
  end

  defp put_net(%Builder{opts: opts} = b, key, value) do
    net = Args.normalize(opts[:net] || %{})
    %{b | opts: Map.put(opts, :net, Map.put(net, key, value))}
  end

  defp append_net(%Builder{opts: opts} = b, key, value) do
    net = Args.normalize(opts[:net] || %{})
    updated = Map.update(net, key, [value], &(&1 ++ [value]))
    %{b | opts: Map.put(opts, :net, updated)}
  end

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, error}), do: raise(error)

  # Lifecycle ops succeed with a bare `:ok`, not `{:ok, value}` — returning
  # `ref` itself (the only "value" there is) is what makes the bang variants
  # chainable with `|>`.
  defp unwrap_ref!(ref, :ok), do: ref
  defp unwrap_ref!(_ref, {:error, error}), do: raise(error)
end
