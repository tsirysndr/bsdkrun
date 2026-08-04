defmodule Bsdkrun.Networks do
  @moduledoc """
  Global-network operations: opt machines into a shared subnet so they get
  distinct IPs and reach each other by IP and by name (docker-compose style).

  All functions return `{:ok, value}` or `{:error, %Bsdkrun.Error{}}`.
  """

  alias Bsdkrun.{Cli, Error, Sandbox}
  alias Bsdkrun.Types
  alias Bsdkrun.Types.{NetworkInfo, SandboxInfo}

  @doc "List global networks and their member counts."
  @spec list() :: {:ok, [NetworkInfo.t()]} | {:error, Error.t()}
  def list do
    with {:ok, res} <- Cli.checked(["network", "ls", "--json"], "bsdkrun network ls") do
      rows =
        res.stdout
        |> empty_to_array()
        |> Jason.decode!()
        |> Enum.map(&Types.network_info/1)

      {:ok, rows}
    end
  end

  @doc "Create a global network (starts its shared switch)."
  @spec create(String.t()) :: :ok | {:error, Error.t()}
  def create(name), do: ok(Cli.checked(["network", "create", name], "bsdkrun network create"))

  @doc "Remove one or more networks. `force: true` allows removal with running members."
  @spec remove(String.t() | [String.t()], keyword()) :: :ok | {:error, Error.t()}
  def remove(names, opts \\ []) do
    list = List.wrap(names)
    base = if Keyword.get(opts, :force, false), do: ["network", "rm", "--force"], else: ["network", "rm"]
    ok(Cli.checked(base ++ list, "bsdkrun network rm"))
  end

  @doc "Join or switch a machine (by id or name) to a network. Applies on next start."
  @spec connect(String.t(), String.t()) :: :ok | {:error, Error.t()}
  def connect(machine, network) do
    ok(Cli.checked(["network", "connect", machine, network], "bsdkrun network connect"))
  end

  @doc "Detach a machine from its network. Applies on its next start."
  @spec disconnect(String.t()) :: :ok | {:error, Error.t()}
  def disconnect(machine) do
    ok(Cli.checked(["network", "disconnect", machine], "bsdkrun network disconnect"))
  end

  @doc """
  Refresh members' `/etc/hosts` with the current membership so peers resolve by
  name (notably NetBSD), without restarting members.
  """
  @spec sync(String.t()) :: :ok | {:error, Error.t()}
  def sync(network), do: ok(Cli.checked(["network", "sync", network], "bsdkrun network sync"))

  @doc "The machines currently attached to `network` (running or stopped)."
  @spec members(String.t()) :: {:ok, [SandboxInfo.t()]} | {:error, Error.t()}
  def members(network) do
    with {:ok, all} <- Sandbox.list(all: true) do
      {:ok, Enum.filter(all, &(&1.network == network))}
    end
  end

  defp ok({:ok, _}), do: :ok
  defp ok({:error, _} = err), do: err

  defp empty_to_array(stdout) do
    if String.trim(stdout) == "", do: "[]", else: stdout
  end
end
