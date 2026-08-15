# Loaded automatically by `iex -S mix`, so the SDK is ready at the prompt.
#
#   cd sdk/elixir && iex -S mix
#
# To drive a locally built binary for the session:
#
#   BSDKRUN_BIN=../../target/release/bsdkrun iex -S mix

alias Bsdkrun.{Images, Networks, Sandbox, System, Types, Volumes}

defmodule IExHelpers do
  @moduledoc "Shorthands available at the console."

  @doc "Every machine, exited ones included."
  def ps(all \\ true), do: Sandbox.list!(all: all)

  @doc "The machine created most recently."
  def last, do: ps() |> Enum.max_by(& &1.created_at, fn -> nil end)
end

import IExHelpers

binary =
  try do
    Bsdkrun.Binary.resolve!()
  rescue
    e in Bsdkrun.Error -> "NOT FOUND (#{Exception.message(e)})"
  end

IO.puts("""
bsdkrun #{Application.spec(:bsdkrun_ex, :vsn)} — interactive console
binary: #{binary}

  Sandbox            create / get / list machines
  Images, Volumes    host-level image and volume operations
  Networks, System   global networks; probe, fetch, versions, grow_disk
  ps/0, last/0       every machine / the newest one

  {:ok, sbx} = Sandbox.create(os: :linux, image: "alpine")
  {:ok, res} = Sandbox.exec(sbx, ["uname", "-a"])
  Types.Result.text(res)
""")
