defmodule Bsdkrun do
  @moduledoc """
  Elixir SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
  Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
  Linux, built on [libkrun](https://github.com/containers/libkrun).

  The SDK is a thin, stateless wrapper around the `bsdkrun` binary: it builds
  argv, shells out via `System.cmd/3`, and parses JSON output. It has one runtime
  dependency (`:jason`).

      {:ok, box} = Bsdkrun.create(os: :linux, image: "alpine")
      {:ok, res} = Bsdkrun.exec(box, ["uname", "-a"])
      IO.puts(Bsdkrun.Types.Result.text(res))
      :ok = Bsdkrun.stop(box)

  The binary is resolved (in order) from `Bsdkrun.Binary.set_binary_path/1`, the
  `$BSDKRUN_BIN` env var, `bsdkrun` on `$PATH`, or an in-repo dev build. See
  `Bsdkrun.Binary`.

  ## Modules

    * `Bsdkrun.Sandbox`  — create / inspect / drive microVMs.
    * `Bsdkrun.Images`   — list downloaded images.
    * `Bsdkrun.Volumes`  — list / remove persistent volumes.
    * `Bsdkrun.Networks` — global networks (reach machines by name).
    * `Bsdkrun.System`   — probe, fetch BSD images, list versions, grow disks.
    * `Bsdkrun.Types`    — result structs + `Bsdkrun.Types.Result`.
    * `Bsdkrun.Error`    — the single exception type.
  """

  alias Bsdkrun.Sandbox

  @doc "See `Bsdkrun.Sandbox.create/1`."
  defdelegate create(opts), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.create!/1`."
  defdelegate create!(opts), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.get/1`."
  defdelegate get(id), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.list/1`."
  defdelegate list(opts \\ []), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.exec/3`."
  defdelegate exec(ref, command, opts \\ []), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.logs/2`."
  defdelegate logs(ref, opts \\ []), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.stop/1`."
  defdelegate stop(ref), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.start/1`."
  defdelegate start(ref), to: Sandbox

  @doc "See `Bsdkrun.Sandbox.remove/2`."
  defdelegate remove(ref, opts \\ []), to: Sandbox

  @doc "See `Bsdkrun.Binary.set_binary_path/1`."
  defdelegate set_binary_path(path), to: Bsdkrun.Binary
end
