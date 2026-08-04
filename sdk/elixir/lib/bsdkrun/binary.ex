defmodule Bsdkrun.Binary do
  @moduledoc """
  Resolves the `bsdkrun` binary. First match wins; the result is cached in
  `:persistent_term`. Resolution order:

    1. an explicit override set via `set_binary_path/1`,
    2. the `$BSDKRUN_BIN` environment variable,
    3. `bsdkrun` on `$PATH`,
    4. in-repo dev builds relative to this SDK: `<repo>/target/release/bsdkrun`
       then `<repo>/target/debug/bsdkrun`.

  If none exist, `resolve!/0` raises a `Bsdkrun.Error` of kind
  `:binary_not_found` listing everything it looked at.
  """

  alias Bsdkrun.Error

  @override_key {__MODULE__, :override}
  @cache_key {__MODULE__, :resolved}

  @doc """
  Force the SDK to use a specific `bsdkrun` binary, bypassing discovery. Handy
  in tests or when running against a locally built debug binary.
  """
  @spec set_binary_path(String.t()) :: :ok
  def set_binary_path(path) when is_binary(path) do
    :persistent_term.put(@override_key, path)
    :persistent_term.erase(@cache_key)
    :ok
  end

  @doc "Reset cached discovery state and any override (mainly for tests)."
  @spec reset_cache() :: :ok
  def reset_cache do
    :persistent_term.erase(@override_key)
    :persistent_term.erase(@cache_key)
    :ok
  end

  @doc "Resolve (and cache) the path to the `bsdkrun` binary, or raise."
  @spec resolve!() :: String.t()
  def resolve! do
    case :persistent_term.get(@cache_key, nil) do
      nil ->
        searched = candidates()

        case Enum.find_value(searched, &usable/1) do
          nil ->
            raise Error.binary_not_found(searched)

          found ->
            :persistent_term.put(@cache_key, found)
            found
        end

      cached ->
        cached
    end
  end

  # Candidate locations, in priority order.
  defp candidates do
    override = :persistent_term.get(@override_key, nil)
    env = System.get_env("BSDKRUN_BIN")
    on_path = System.find_executable("bsdkrun")

    # This file lives at sdk/elixir/lib/bsdkrun/binary.ex, so the repo root is
    # four levels up. Prefer a release build, fall back to debug.
    repo_root = Path.expand(Path.join(__DIR__, "../../../.."))

    [
      override,
      env,
      on_path,
      Path.join([repo_root, "target", "release", "bsdkrun"]),
      Path.join([repo_root, "target", "debug", "bsdkrun"])
    ]
    |> Enum.reject(&is_nil/1)
  end

  # A path-like candidate must exist on disk; a bare name must be on PATH.
  # Returns the resolved absolute path, or nil if unusable.
  defp usable(candidate) do
    cond do
      String.contains?(candidate, "/") ->
        if File.exists?(candidate), do: candidate, else: nil

      true ->
        System.find_executable(candidate)
    end
  end
end
