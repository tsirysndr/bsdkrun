defmodule Bsdkrun.Args do
  @moduledoc """
  Builds the full `bsdkrun` argv (minus the binary and global flags) for a
  detached `create`. Ported verbatim from the TypeScript SDK's `args.ts`; every
  path ends with `-d` so `create` yields a handle.

  Options are given as a keyword list or map, discriminated on `:os`
  (`:linux`, `:freebsd`, `:netbsd`, `:firmware`, `:kernel`). See
  `Bsdkrun.Sandbox.create/1` for the full set of per-kind keys.
  """

  @doc "Build the create argv (a list of strings) for the given options."
  @spec build_create([keyword() | map()]) :: [String.t()]
  def build_create(opts) do
    opts = normalize(opts)

    args =
      case os(opts) do
        :linux -> linux(opts)
        :freebsd -> freebsd(opts)
        :netbsd -> netbsd(opts)
        :firmware -> firmware(opts)
        :kernel -> kernel(opts)
        other -> raise ArgumentError, "unknown os #{inspect(other)}"
      end

    Enum.map(args, &to_string/1)
  end

  # --- per-guest-kind builders ------------------------------------------------

  defp linux(o) do
    ["linux", fetch!(o, :image), "-d"]
    |> opt(o, :kernel, "--kernel")
    |> opt(o, :kernel_version, "--kernel-version")
    |> flag(o, :initramfs, "--initramfs")
    |> opt(o, :volume, "-v")
    |> multi(o, :mounts, "--mount")
    |> opt(o, :entrypoint, "--entrypoint")
    |> opt(o, :console, "--console")
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
    |> command_args(o)
  end

  defp freebsd(o) do
    ["freebsd", "-d"]
    |> opt(o, :version, "--version")
    |> opt(o, :firmware, "--firmware")
    |> flag(o, :force, "--force")
    |> concat(disk_args(o))
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
  end

  defp netbsd(o) do
    ["netbsd", "-d"]
    |> opt(o, :version, "--version")
    |> flag(o, :force, "--force")
    |> concat(disk_args(o))
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
  end

  defp firmware(o) do
    ["firmware", "--firmware", fetch!(o, :firmware), "--disk", fetch!(o, :disk), "-d"]
    |> concat(disk_args(o))
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
  end

  defp kernel(o) do
    ["kernel", "--kernel", fetch!(o, :kernel), "-d"]
    |> opt(o, :format, "--format")
    |> opt(o, :initramfs, "--initramfs")
    |> opt(o, :cmdline, "--cmdline")
    |> opt(o, :disk, "--disk")
    |> concat(disk_args(o))
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
  end

  # --- shared fragment helpers ------------------------------------------------

  defp net_args(nil), do: []

  defp net_args(net) do
    net = normalize(net)

    disabled = if net[:disabled], do: ["--no-net"], else: []
    ports = Enum.flat_map(net[:ports] || [], fn p -> ["--port", port_str(p)] end)
    mac = if net[:mac], do: ["--mac", net[:mac]], else: []
    network = if net[:network], do: ["--network", net[:network]], else: []

    disabled ++ ports ++ mac ++ network
  end

  defp name_args(o), do: if(o[:name], do: ["--name", o[:name]], else: [])

  defp vm_args(o) do
    cpus = if o[:cpus] != nil, do: ["--cpus", to_string(o[:cpus])], else: []
    mem = if o[:mem] != nil, do: ["--mem", to_string(o[:mem])], else: []
    cpus ++ mem
  end

  defp disk_args(o) do
    persist = if o[:persist], do: ["--persist"], else: []
    volume = if o[:volume], do: ["-v", o[:volume]], else: []
    attach = Enum.flat_map(o[:attach_disk] || [], fn d -> ["--attach-disk", d] end)
    persist ++ volume ++ attach
  end

  defp command_args(acc, o) do
    case o[:command] do
      cmd when is_list(cmd) and cmd != [] -> acc ++ ["--" | cmd]
      _ -> acc
    end
  end

  # A port is a `"HOST:GUEST"` string, a `%{host:, guest:}` map, or a
  # `{host, guest}` tuple.
  defp port_str(p) when is_binary(p), do: p
  defp port_str({host, guest}), do: "#{host}:#{guest}"

  defp port_str(p) when is_map(p) or is_list(p) do
    p = normalize(p)
    "#{p[:host]}:#{p[:guest]}"
  end

  # --- pipe-friendly building blocks ------------------------------------------

  defp opt(acc, o, key, flag) do
    case o[key] do
      nil -> acc
      value -> acc ++ [flag, to_string(value)]
    end
  end

  defp flag(acc, o, key, flag) do
    if o[key], do: acc ++ [flag], else: acc
  end

  defp multi(acc, o, key, flag) do
    acc ++ Enum.flat_map(o[key] || [], fn v -> [flag, to_string(v)] end)
  end

  defp concat(acc, list), do: acc ++ list

  # --- option normalization ---------------------------------------------------

  defp os(o) do
    case o[:os] do
      value when is_atom(value) and not is_nil(value) -> value
      value when is_binary(value) -> String.to_existing_atom(value)
      other -> other
    end
  end

  defp fetch!(o, key) do
    case o[key] do
      nil -> raise ArgumentError, "missing required option #{inspect(key)}"
      value -> value
    end
  end

  # Accept keyword lists or maps (with atom or string keys); return an
  # atom-keyed map for uniform access.
  defp normalize(opts) when is_list(opts) do
    if Keyword.keyword?(opts), do: Map.new(opts), else: opts
  end

  defp normalize(opts) when is_map(opts) do
    Map.new(opts, fn
      {k, v} when is_binary(k) -> {String.to_atom(k), v}
      {k, v} -> {k, v}
    end)
  end
end
