defmodule Bsdkrun.Cache do
  @moduledoc """
  Cached guest directories.

  Entries are keyed, so a rebuild can pick up where the last one left off:

      case Bsdkrun.Cache.restore("web", key: key, restore_keys: ["deps-"]) do
        {:ok, %{restored: false}} ->
          Bsdkrun.Sandbox.exec("web", ["npm", "ci"])
          Bsdkrun.Cache.save("web", "/app/node_modules", key: key)

        {:ok, _hit} ->
          :ok
      end

  Where entries live — host disk or S3 — is host configuration, not an SDK
  concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or write
  `~/.config/bsdkrun/cache.toml`.
  """

  alias Bsdkrun.{Cli, Error}

  @typedoc "A stored cache entry, as `cache ls` reports it."
  @type entry :: %{
          key: String.t(),
          path: String.t(),
          compression: String.t(),
          size: non_neg_integer(),
          created: non_neg_integer(),
          digest: String.t()
        }

  @typedoc "What a restore did. A miss is not an error — check `:restored`."
  @type result :: %{
          restored: boolean(),
          requested_key: String.t(),
          key: String.t() | nil,
          path: String.t() | nil,
          size: non_neg_integer() | nil,
          compression: String.t() | nil,
          created: non_neg_integer() | nil
        }

  @doc """
  Archive the guest directory at `path` under `:key`.

  Options: `:key` (required), `:compression` (`"gzip"` by default, or `"zstd"`,
  `"estargz"`, `"none"`), `:force` to replace an existing entry.
  """
  @spec save(String.t(), String.t(), keyword()) :: {:ok, entry()} | {:error, Error.t()}
  def save(id, path, opts) do
    key = Keyword.fetch!(opts, :key)
    compression = Keyword.get(opts, :compression, "gzip")

    args = ["cache", "save", "#{id}:#{path}", "--key", key, "--json"]
    args = if compression == "gzip", do: args, else: args ++ ["--compression", compression]
    args = if Keyword.get(opts, :force, false), do: args ++ ["--force"], else: args

    with {:ok, map} <- json(args, "bsdkrun cache save") do
      {:ok, to_entry(map)}
    end
  end

  @doc """
  Restore a stored tree.

  Options: `:key` (required), `:path` (defaults to where it was saved from),
  `:restore_keys` — prefixes tried in order when the key misses.
  """
  @spec restore(String.t(), keyword()) :: {:ok, result()} | {:error, Error.t()}
  def restore(id, opts) do
    key = Keyword.fetch!(opts, :key)
    target = if path = Keyword.get(opts, :path), do: "#{id}:#{path}", else: id
    restore_keys = Keyword.get(opts, :restore_keys, [])

    args = ["cache", "restore", target, "--key", key, "--json"]
    args = if restore_keys == [], do: args, else: args ++ ["--restore-keys" | restore_keys]

    with {:ok, map} <- json(args, "bsdkrun cache restore") do
      {:ok,
       %{
         restored: Map.get(map, "restored", false),
         requested_key: Map.get(map, "requested_key", key),
         key: Map.get(map, "key"),
         path: Map.get(map, "path"),
         size: Map.get(map, "size"),
         compression: Map.get(map, "compression"),
         created: Map.get(map, "created")
       }}
    end
  end

  @doc "Every stored cache entry, newest first."
  @spec list() :: {:ok, [entry()]} | {:error, Error.t()}
  def list do
    res = Cli.run(["cache", "ls", "--json"])

    if res.exit_code == 0 do
      {:ok, res.stdout |> decode("[]") |> Enum.map(&to_entry/1)}
    else
      {:error, Error.command_failed(res.exit_code, res.stdout, res.stderr, "bsdkrun cache ls")}
    end
  end

  @doc "Remove entries by key, or every one of them with `all: true`."
  @spec remove([String.t()], keyword()) :: :ok | {:error, Error.t()}
  def remove(keys \\ [], opts \\ []) do
    args = ["cache", "rm"]
    args = if Keyword.get(opts, :all, false), do: args ++ ["--all"], else: args ++ keys

    case Cli.checked(args, "bsdkrun cache rm") do
      {:ok, _} -> :ok
      error -> error
    end
  end

  defp json(args, label) do
    res = Cli.run(args)

    if res.exit_code == 0 do
      {:ok, decode(res.stdout, "{}")}
    else
      {:error, Error.command_failed(res.exit_code, res.stdout, res.stderr, label)}
    end
  end

  defp decode(text, empty) do
    case String.trim(text) do
      "" -> Jason.decode!(empty)
      body -> Jason.decode!(body)
    end
  end

  defp to_entry(map) do
    %{
      key: Map.get(map, "key", ""),
      path: Map.get(map, "path", ""),
      compression: Map.get(map, "compression", ""),
      size: Map.get(map, "size", 0),
      created: Map.get(map, "created", 0),
      digest: Map.get(map, "digest", "")
    }
  end
end
