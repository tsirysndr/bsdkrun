defmodule Bsdkrun.Sandbox do
  @moduledoc """
  A handle to a running (or stopped) `bsdkrun` microVM. Create one with
  `create/1`, reconnect with `get/1`, or enumerate with `list/1`.

      {:ok, box} = Bsdkrun.Sandbox.create(os: :linux, image: "alpine")
      {:ok, res} = Bsdkrun.Sandbox.exec(box, ["uname", "-a"])
      :ok = Bsdkrun.Sandbox.stop(box)

  Every fallible function returns `{:ok, value}` or `{:error, %Bsdkrun.Error{}}`,
  with a bang counterpart (`create!/1`, `get!/1`, `list!/1`) that unwraps or
  raises. Functions that act on a machine accept either a `%Bsdkrun.Sandbox{}`
  struct or a bare id string.
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

  # --- construction / discovery ----------------------------------------------

  @doc """
  Boot a new microVM (detached) and return a handle to it.

  `opts` is a keyword list or map discriminated on `:os`
  (`:linux`, `:freebsd`, `:netbsd`, `:firmware`, `:kernel`), plus the per-kind
  keys accepted by `Bsdkrun.Args`. `:log_level` (default `1`) controls boot
  diagnostics.
  """
  @spec create(keyword() | map()) :: {:ok, t()} | {:error, Error.t()}
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
  @spec create!(keyword() | map()) :: t()
  def create!(opts), do: unwrap!(create(opts))

  @doc "Reconnect to an existing machine by id (a unique prefix is enough)."
  @spec get(String.t()) :: {:ok, t()} | {:error, Error.t()}
  def get(id) when is_binary(id) do
    with {:ok, all} <- list(all: true) do
      case Enum.find(all, &(&1.id == id or String.starts_with?(&1.id, id))) do
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
    args = if Keyword.get(opts, :all, false), do: ["ps", "--json", "--all"], else: ["ps", "--json"]

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
        log_level: Keyword.get(opts, :log_level, 0)
      )

    result = %Result{
      stdout: res.stdout,
      stderr: res.stderr,
      exit_code: res.exit_code,
      command: "exec " <> Enum.join(argv, " ")
    }

    if Keyword.get(opts, :throw_on_error, false) and res.exit_code != 0 do
      {:error,
       Error.command_failed(res.exit_code, res.stdout, res.stderr, result.command)}
    else
      {:ok, result}
    end
  end

  @doc "Read the machine's console log. Pass `boot: true` for bsdkrun's own boot log."
  @spec logs(ref(), keyword()) :: {:ok, String.t()} | {:error, Error.t()}
  def logs(ref, opts \\ []) do
    args = if Keyword.get(opts, :boot, false), do: ["logs", "--boot", id(ref)], else: ["logs", id(ref)]

    with {:ok, res} <- Cli.checked(args, "bsdkrun logs") do
      {:ok, res.stdout}
    end
  end

  # --- lifecycle --------------------------------------------------------------

  @doc "Stop the machine (BSD guests clean-poweroff; Linux is SIGTERM'd)."
  @spec stop(ref()) :: :ok | {:error, Error.t()}
  def stop(ref), do: lifecycle(["stop", id(ref)], "bsdkrun stop")

  @doc "Restart a stopped machine in place — same id, disk/rootfs, resources."
  @spec start(ref()) :: :ok | {:error, Error.t()}
  def start(ref), do: lifecycle(["start", id(ref)], "bsdkrun start")

  @doc "Remove the machine and its state. `force: true` stops it first if running."
  @spec remove(ref(), keyword()) :: :ok | {:error, Error.t()}
  def remove(ref, opts \\ []) do
    args = if Keyword.get(opts, :force, false), do: ["rm", "--force", id(ref)], else: ["rm", id(ref)]
    lifecycle(args, "bsdkrun rm")
  end

  @doc "Change the recorded vCPU / RAM (`:cpus`, `:mem`). Applies on next `start/1`."
  @spec update(ref(), keyword()) :: :ok | {:error, Error.t()}
  def update(ref, opts) do
    args =
      ["update", id(ref)]
      |> then(fn a -> if opts[:cpus] != nil, do: a ++ ["--cpus", to_string(opts[:cpus])], else: a end)
      |> then(fn a -> if opts[:mem] != nil, do: a ++ ["--mem", to_string(opts[:mem])], else: a end)

    lifecycle(args, "bsdkrun update")
  end

  @doc "Join or switch this machine to a global network. Applies on next `start/1`."
  @spec connect_network(ref(), String.t()) :: :ok | {:error, Error.t()}
  def connect_network(ref, network) do
    lifecycle(["network", "connect", id(ref), network], "bsdkrun network connect")
  end

  @doc "Detach this machine from its network. Applies on next `start/1`."
  @spec disconnect_network(ref()) :: :ok | {:error, Error.t()}
  def disconnect_network(ref) do
    lifecycle(["network", "disconnect", id(ref)], "bsdkrun network disconnect")
  end

  # --- status -----------------------------------------------------------------

  @doc "Fetch this machine's current status row, or `nil` if it's gone."
  @spec status(ref()) :: {:ok, SandboxInfo.t() | nil} | {:error, Error.t()}
  def status(ref) do
    machine_id = id(ref)

    with {:ok, all} <- list(all: true) do
      {:ok, Enum.find(all, &(&1.id == machine_id))}
    end
  end

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

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, error}), do: raise(error)
end
