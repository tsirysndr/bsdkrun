defmodule Bsdkrun.Zfs do
  @moduledoc """
  ZFS volumes: one zpool per volume on a disk image, held by one machine at a
  time (Linux, FreeBSD, NetBSD). Branches are datasets cloned copy-on-write
  inside the pool (`zfs snapshot` + `zfs clone`), so a branch costs nothing
  until it diverges.
  """

  alias Bsdkrun.{Cli, Error, Types}
  alias Bsdkrun.Types.{ZfsBranch, ZfsVolumeInfo}

  @doc "List ZFS volumes, with their branches and holder."
  @spec list() :: {:ok, [ZfsVolumeInfo.t()]} | {:error, Error.t()}
  def list do
    with {:ok, res} <- Cli.checked(["zfs", "ls", "--json"], "bsdkrun zfs ls") do
      {:ok, res.stdout |> rows() |> Enum.map(&Types.zfs_volume_info/1)}
    end
  end

  @doc "The branches of one volume."
  @spec branches(String.t()) :: {:ok, [ZfsBranch.t()]} | {:error, Error.t()}
  def branches(volume) do
    with {:ok, res} <- Cli.checked(["zfs", "ls", volume, "--json"], "bsdkrun zfs ls") do
      {:ok, res.stdout |> rows() |> Enum.map(&Types.zfs_branch/1)}
    end
  end

  @doc """
  Create a volume: a sparse image that becomes a zpool with a `main` branch
  the first time a guest imports it. `size: "10G"` (the default).
  """
  @spec create(String.t(), keyword()) :: {:ok, String.t()} | {:error, Error.t()}
  def create(name, opts \\ []) do
    line(["zfs", "create", name] ++ flag("--size", opts[:size]), "bsdkrun zfs create")
  end

  @doc """
  Fork a branch copy-on-write from `from:` (`main` by default). Runs in the
  machine holding the volume or in a short-lived FreeBSD helper, so it can
  take as long as a BSD boot. Returns the new `VOL@BRANCH`.
  """
  @spec branch(String.t(), String.t(), keyword()) :: {:ok, String.t()} | {:error, Error.t()}
  def branch(volume, name, opts \\ []) do
    line(["zfs", "branch", volume, name] ++ flag("--from", opts[:from]), "bsdkrun zfs branch")
  end

  @doc """
  Attach `VOL` or `VOL@BRANCH` to a machine at `mount:` (`/mnt/VOL`, or
  `/mnt/VOL-BRANCH`, by default). A pool is imported by one machine at a
  time; a stopped machine picks it up on start, a running holder mounts the
  branch live. Returns the CLI's one-line report.
  """
  @spec attach(String.t(), String.t(), keyword()) :: {:ok, String.t()} | {:error, Error.t()}
  def attach(target, machine, opts \\ []) do
    line(["zfs", "attach", target, machine] ++ flag("--mount", opts[:mount]), "bsdkrun zfs attach")
  end

  @doc "Detach one branch (`VOL@BRANCH`) or the whole volume (`VOL`) from a machine."
  @spec detach(String.t(), String.t()) :: {:ok, String.t()} | {:error, Error.t()}
  def detach(target, machine) do
    line(["zfs", "detach", target, machine], "bsdkrun zfs detach")
  end

  @doc """
  Destroy branches (`VOL@BRANCH`) or whole volumes (`VOL`). `force: true`
  destroys a volume a stopped machine still has attached.
  """
  @spec destroy(String.t() | [String.t()], keyword()) :: :ok | {:error, Error.t()}
  def destroy(targets, opts \\ []) do
    base = if Keyword.get(opts, :force, false), do: ["zfs", "destroy", "--force"], else: ["zfs", "destroy"]

    case Cli.checked(base ++ List.wrap(targets), "bsdkrun zfs destroy") do
      {:ok, _} -> :ok
      {:error, _} = err -> err
    end
  end

  defp flag(_name, nil), do: []
  defp flag(name, value), do: [name, value]

  defp line(args, label) do
    with {:ok, res} <- Cli.checked(args, label) do
      {:ok, String.trim(res.stdout)}
    end
  end

  defp rows(stdout) do
    stdout
    |> then(&if(String.trim(&1) == "", do: "[]", else: &1))
    |> Jason.decode!()
  end
end
