defmodule Bsdkrun.Volumes do
  @moduledoc "Persistent volume operations."

  alias Bsdkrun.{Cli, Error, Types}
  alias Bsdkrun.Types.VolumeInfo

  @doc "List persistent volumes."
  @spec list() :: {:ok, [VolumeInfo.t()]} | {:error, Error.t()}
  def list do
    with {:ok, res} <- Cli.checked(["volume", "ls", "--json"], "bsdkrun volume ls") do
      rows =
        res.stdout
        |> then(&if(String.trim(&1) == "", do: "[]", else: &1))
        |> Jason.decode!()
        |> Enum.map(&Types.volume_info/1)

      {:ok, rows}
    end
  end

  @doc "Remove one or more volumes (and their data)."
  @spec remove(String.t() | [String.t()], keyword()) :: :ok | {:error, Error.t()}
  def remove(names, opts \\ []) do
    list = List.wrap(names)
    base = if Keyword.get(opts, :force, false), do: ["volume", "rm", "--force"], else: ["volume", "rm"]

    case Cli.checked(base ++ list, "bsdkrun volume rm") do
      {:ok, _} -> :ok
      {:error, _} = err -> err
    end
  end
end
