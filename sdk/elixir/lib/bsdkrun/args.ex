defmodule Bsdkrun.Args do
  @moduledoc """
  Builds the full `bsdkrun` argv (minus the binary and global flags) for a
  detached `create`. Ported verbatim from the TypeScript SDK's `args.ts`; every
  path ends with `-d` so `create` yields a handle.

  Options are given as a keyword list or map, discriminated on `:os`
  (`:linux`, `:freebsd`, `:netbsd`, `:firmware`, `:kernel`, `:unikraft`,
  `:solo5`, `:nanos`, `:osv`). See
  `Bsdkrun.Sandbox.create/1` for the full set of per-kind keys.
  """

  @typedoc "The guest kind, discriminating `create/1` and mirrored by `SandboxInfo.kind`."
  @type os ::
          :linux
          | :freebsd
          | :netbsd
          | :firmware
          | :kernel
          | :unikraft
          | :solo5
          | :nanos
          | :osv

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
        :unikraft -> unikraft(opts)
        :solo5 -> solo5(opts)
        :nanos -> nanos(opts)
        :osv -> osv(opts)
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

  # Nanos: no agent (like unikraft), but it has a root disk, so :persist is
  # honored. :image is a path or a ~/.ops/images name.
  defp nanos(o) do
    ["nanos", "-d"]
    |> opt(o, :kernel, "--kernel")
    |> opt(o, :cmdline, "--cmdline")
    |> concat(if o[:persist], do: ["--persist"], else: [])
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
    |> concat([fetch!(o, :image)])
  end

  # OSv: like nanos, no agent (no exec/shell/snapshot), but it does have a root
  # filesystem, so :persist is honored. :image is a loader.img, or on x86_64 the
  # loader ELF plus a :disk.
  defp osv(o) do
    ["osv", "-d"]
    |> opt(o, :cmdline, "--cmdline")
    |> opt(o, :disk, "--disk")
    |> opt(o, :gic, "--gic")
    |> concat(if o[:persist], do: ["--persist"], else: [])
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
    |> concat([fetch!(o, :image)])
  end

  # No `disk_args`: a unikernel has no disk, so there is nothing to persist,
  # attach or clone. `:path` is a kraft project dir or an image; default ".".
  defp unikraft(o) do
    ["unikraft", "-d"]
    |> opt(o, :cmdline, "--cmdline")
    |> opt(o, :initramfs, "--initramfs")
    # Volumes are the exception to "no disk options": virtio-fs shares, which
    # need neither a disk nor an agent.
    |> multi(o, :mounts, "--mount")
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
    |> concat([o[:path] || "."])
  end

  # Solo5 (MirageOS): runs under the `solo5-hvt` tender rather than libkrun.
  # The unikernel declares its devices in its own MFT1 manifest, so only the
  # `:block` backing files ("NAME=FILE") are passed. `:path` is a `.hvt`
  # binary or a project dir whose `dist/` holds one; default ".". Guest
  # `:args` go last, after a literal "--" — MirageOS options look like
  # bsdkrun's own (e.g. --ipv4=...), so the CLI takes them as trailing args.
  defp solo5(o) do
    ["solo5", "-d"]
    |> multi(o, :block, "--block")
    |> concat(net_args(o[:net]))
    |> concat(name_args(o))
    |> concat(vm_args(o))
    |> concat([o[:path] || "."])
    |> concat(
      case o[:args] do
        nil -> []
        [] -> []
        args -> ["--" | args]
      end
    )
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

  @doc """
  Accept a keyword list or a map (atom or string keys) and return an
  atom-keyed map for uniform access. Exposed for `Bsdkrun.Sandbox.Builder`,
  which normalizes `:net` the same way `create/1`'s options are normalized
  here.
  """
  @spec normalize(keyword() | map()) :: map()
  def normalize(opts) when is_list(opts) do
    if Keyword.keyword?(opts), do: Map.new(opts), else: opts
  end

  def normalize(opts) when is_map(opts) do
    Map.new(opts, fn
      {k, v} when is_binary(k) -> {String.to_atom(k), v}
      {k, v} -> {k, v}
    end)
  end
end
