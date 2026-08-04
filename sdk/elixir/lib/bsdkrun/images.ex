defmodule Bsdkrun.Images do
  @moduledoc "Image operations: list downloaded OCI + fetched BSD images."

  alias Bsdkrun.{Cli, Error, Types}
  alias Bsdkrun.Types.ImageInfo

  @doc "List downloaded images."
  @spec list() :: {:ok, [ImageInfo.t()]} | {:error, Error.t()}
  def list do
    with {:ok, res} <- Cli.checked(["images", "--json"], "bsdkrun images") do
      rows =
        res.stdout
        |> then(&if(String.trim(&1) == "", do: "[]", else: &1))
        |> Jason.decode!()
        |> Enum.map(&Types.image_info/1)

      {:ok, rows}
    end
  end
end
