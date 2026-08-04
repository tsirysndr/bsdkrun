defmodule Bsdkrun.System do
  @moduledoc "Host-level toolchain + image operations."

  alias Bsdkrun.{Cli, Error}

  @doc """
  Sanity-check the toolchain: verify libkrun links and a context can be
  created/configured. Does not boot. Returns `true` on success.
  """
  @spec probe() :: boolean()
  def probe do
    Cli.run(["probe"]).exit_code == 0
  end

  @doc """
  Download + prepare a BSD image ahead of time. Returns `{:ok, output}`.
  Opts: `:version`, `:dir`, `:force`.
  """
  @spec fetch_image(:freebsd | :netbsd, keyword()) :: {:ok, String.t()} | {:error, Error.t()}
  def fetch_image(os, opts \\ []) do
    args =
      ["fetch", "--os", to_string(os)]
      |> then(fn a -> if opts[:version], do: a ++ ["--version", opts[:version]], else: a end)
      |> then(fn a -> if opts[:dir], do: a ++ ["--dir", opts[:dir]], else: a end)
      |> then(fn a -> if Keyword.get(opts, :force, false), do: a ++ ["--force"], else: a end)

    with {:ok, res} <- Cli.checked(args, "bsdkrun fetch"), do: {:ok, res.stdout}
  end

  @doc """
  List the arm64 builds available to fetch for a BSD. Returns `{:ok, lines}` —
  the raw non-empty output lines.
  """
  @spec versions(:freebsd | :netbsd) :: {:ok, [String.t()]} | {:error, Error.t()}
  def versions(os) do
    with {:ok, res} <- Cli.checked(["versions", "--os", to_string(os)], "bsdkrun versions") do
      lines = res.stdout |> String.split("\n") |> Enum.reject(&(&1 == ""))
      {:ok, lines}
    end
  end

  @doc "Grow a raw disk image (the guest expands its root FS on next boot)."
  @spec grow_disk(String.t(), String.t()) :: :ok | {:error, Error.t()}
  def grow_disk(disk, size) do
    case Cli.checked(["grow", "--disk", disk, "--size", size], "bsdkrun grow") do
      {:ok, _} -> :ok
      {:error, _} = err -> err
    end
  end
end
