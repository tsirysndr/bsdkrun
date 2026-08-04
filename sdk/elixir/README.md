# bsdkrun_ex (Elixir SDK)

An Elixir SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

The SDK shells out to the `bsdkrun` binary via `System.cmd/3`, so its only
runtime dependency is [`jason`](https://hex.pm/packages/jason) for JSON parsing.

```elixir
{:ok, box} = Bsdkrun.create(os: :linux, image: "alpine")

# argv exec — no shell parsing; env / stdin / a PTY / a working dir:
{:ok, res} = Bsdkrun.exec(box, ["uname", "-a"])
IO.puts(Bsdkrun.Types.Result.text(res))

{:ok, _} = Bsdkrun.exec(box, ["apk", "add", "curl"])
:ok = Bsdkrun.stop(box)
```

## Install

Add `:bsdkrun_ex` to your `mix.exs` deps:

```elixir
def deps do
  [
    {:bsdkrun_ex, "~> 0.1.0"}
  ]
end
```

Then `mix deps.get`.

> The Hex package is **`bsdkrun_ex`** — the Gleam SDK already publishes as
> `bsdkrun`, and Hex is one namespace shared by both. The modules are plain
> `Bsdkrun.*`, so nothing in the code below carries the suffix.

### The `bsdkrun` binary

You also need the `bsdkrun` binary itself (and a linked libkrun — see the
[bsdkrun README](../../README.md)). The SDK finds the binary via, in order:

1. `Bsdkrun.Binary.set_binary_path("/path/to/bsdkrun")`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build

## Creating a sandbox

`Bsdkrun.Sandbox.create/1` is discriminated on `:os` — the options change per
guest kind. Every `create` runs the machine **detached** and returns a
`%Bsdkrun.Sandbox{}` handle.

```elixir
# Linux OCI image (docker run-style)
Bsdkrun.create(
  os: :linux,
  image: "ghcr.io/owner/name:tag",
  cpus: 2,
  mem: 1024,
  volume: "web",                       # persistent CoW rootfs
  mounts: ["~/project:/src", "~/data:/data:ro"],
  net: %{ports: ["8080:80", "2222:22"]},
  command: ["node", "server.js"]       # args after `--`
)

# FreeBSD (EFI on macOS, PVH on Linux/amd64)
Bsdkrun.create(os: :freebsd, version: "14.3", mem: 2048)

# NetBSD (direct-kernel boot everywhere)
Bsdkrun.create(os: :netbsd, version: "10.1", volume: "db")

# Boot a raw disk through its UEFI loader
Bsdkrun.create(os: :firmware, firmware: "KRUN_EFI.fd", disk: "disk.raw")

# Boot a kernel directly, no bootloader
Bsdkrun.create(os: :kernel, kernel: "netbsd", format: "elf", disk: "root.raw")
```

## Running commands

`exec/3` is the primary programmatic entrypoint. Pass an argv list (no shell
parsing) or a bare program name with `:args`, plus options:

```elixir
Bsdkrun.exec(box, ["ls", "-la", "/etc"])

{:ok, res} =
  Bsdkrun.exec(box, "node",
    args: ["-e", "IO.puts System.get_env(\"X\")"],
    env: %{"X" => "hi"},
    cwd: "/app",
    stdin: "data on stdin",
    tty: true,               # allocate a PTY
    throw_on_error: true     # return {:error, _} on a non-zero exit
  )

res.stdout
res.exit_code
Bsdkrun.Types.Result.ok?(res)
Bsdkrun.Types.Result.text(res)   # stdout, trailing newlines trimmed
```

## Lifecycle & inventory

```elixir
{:ok, box}  = Bsdkrun.create(os: :linux, image: "alpine", command: ["sleep", "300"])
{:ok, same} = Bsdkrun.get(box.id)          # reconnect (prefix ok)
{:ok, list} = Bsdkrun.list(all: true)      # [%Bsdkrun.Types.SandboxInfo{}]

Bsdkrun.Sandbox.status(box)      # {:ok, %SandboxInfo{} | nil}
Bsdkrun.Sandbox.running?(box)    # boolean
Bsdkrun.logs(box)                # {:ok, console_log}
Bsdkrun.stop(box)                # BSD guests clean-poweroff; Linux SIGTERM
Bsdkrun.start(box)               # restart in place — resumes disk/rootfs
Bsdkrun.Sandbox.update(box, cpus: 4, mem: 2048)  # applies on next start
Bsdkrun.remove(box, force: true)
```

Host-level modules:

```elixir
Bsdkrun.System.probe()                         # toolchain sanity check -> boolean
Bsdkrun.Images.list()                          # {:ok, [ImageInfo]}
Bsdkrun.Volumes.list()                         # {:ok, [VolumeInfo]}
Bsdkrun.Volumes.remove("web", force: true)
Bsdkrun.Networks.list()                        # {:ok, [NetworkInfo]}
Bsdkrun.System.fetch_image(:freebsd, version: "14.3")
Bsdkrun.System.versions(:netbsd)
```

## Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one subnet
and reach each other **by IP and by name** (docker-compose style), with internal
DNS:

```elixir
alias Bsdkrun.{Sandbox, Networks}

:ok = Networks.create("devnet")

{:ok, db} =
  Sandbox.create(os: :linux, image: "postgres", name: "db", net: %{network: "devnet"})

{:ok, api} =
  Sandbox.create(os: :linux, image: "myapi", name: "api", net: %{network: "devnet"})

# resolves db -> its IP on devnet
Sandbox.exec(api, ["ping", "-c1", "db"])

# inspect + manage
{:ok, _networks} = Networks.list()             # [%NetworkInfo{}]
{:ok, _members}  = Networks.members("devnet")  # [%SandboxInfo{}] on the network
{:ok, info}      = Sandbox.status(db)          # info.network == "devnet", info.net_ip set

# edit membership (applies on next start — a VM's NIC is fixed at boot)
:ok = Sandbox.connect_network(api, "devnet")   # or Networks.connect(api.id, "devnet")
:ok = Sandbox.disconnect_network(api)
:ok = Sandbox.start(api)                        # re-joins with the new membership

:ok = Networks.sync("devnet")                   # refresh members' /etc/hosts (fixes NetBSD)
:ok = Networks.remove("devnet", force: true)
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD** resolves via
a synced `/etc/hosts` block — joins auto-sync, and `Networks.sync/1` refreshes an
existing network without restarting members.

## Errors

Every fallible function returns `{:ok, value}` or `{:error, %Bsdkrun.Error{}}`.
The `%Bsdkrun.Error{}` exception has a `:kind`:

- `:binary_not_found`  — the `bsdkrun` binary wasn't found.
- `:command_failed`    — a command exited non-zero (carries `:exit_code`,
  `:stdout`, `:stderr`, `:label`).
- `:sandbox_not_found` — `Bsdkrun.get/1` matched no machine.

The bang variants (`Bsdkrun.create!/1`, `Bsdkrun.Sandbox.get!/1`,
`Bsdkrun.Sandbox.list!/1`) unwrap the value or `raise` the error.

## Try it interactively

```sh
iex -S mix
```

A `.iex.exs` in this directory aliases the SDK's modules and defines `ps/0`
(every machine, exited ones included) and `last/0` (the newest one), so the API
is in scope at the prompt. To drive a locally built binary for the session:

```sh
BSDKRUN_BIN=../../target/release/bsdkrun iex -S mix
```

## License

MIT
