# bsdkrun_ex (Elixir SDK)

An Elixir SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

The SDK shells out to the `bsdkrun` binary via `System.cmd/3`, so its only
runtime dependency is [`jason`](https://hex.pm/packages/jason) for JSON parsing.

```elixir
{:ok, sbx} = Bsdkrun.create(os: :linux, image: "alpine")

# argv exec — no shell parsing; env / stdin / a PTY / a working dir:
{:ok, res} = Bsdkrun.exec(sbx, ["uname", "-a"])
IO.puts(Bsdkrun.Types.Result.text(res))

{:ok, _} = Bsdkrun.exec(sbx, ["apk", "add", "curl"])
:ok = Bsdkrun.stop(sbx)
```

Or, with the bang variants, as one `|>` chain:

```elixir
Bsdkrun.create!(os: :linux, image: "alpine")
|> Bsdkrun.exec!(["uname", "-a"])
|> Bsdkrun.Types.Result.text()
|> IO.puts()
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

### Environment variables

`:env` sets the guest environment for the machine's entrypoint. It is merged
over the image's own config, so a key the image already defines is replaced
rather than duplicated.

```elixir
{:ok, sbx} =
  Bsdkrun.Sandbox.create(
    os: "linux",
    image: "node:22",
    env: %{"NODE_ENV" => "production", "PORT" => "3000"},
    command: ["node", "server.js"]
  )
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. For a single command
rather than the whole machine, `exec` takes its own `:env`.

## Running commands

`exec/3` is the primary programmatic entrypoint. Pass an argv list (no shell
parsing) or a bare program name with `:args`, plus options:

```elixir
Bsdkrun.exec(sbx, ["ls", "-la", "/etc"])

{:ok, res} =
  Bsdkrun.exec(sbx, "node",
    args: ["-e", "IO.puts System.get_env(\"X\")"],
    env: %{"X" => "hi"},
    cwd: "/app",
    stdin: "data on stdin",
    on_stdout: &IO.binwrite(:stdio, &1),
    on_stderr: &IO.binwrite(:stderr, &1),
    tty: true,               # allocate a PTY
    throw_on_error: true     # return {:error, _} on a non-zero exit
  )

res.stdout
res.exit_code
Bsdkrun.Types.Result.ok?(res)
Bsdkrun.Types.Result.text(res)   # stdout, trailing newlines trimmed
```

The callbacks receive binary chunks in real time while the complete streams
remain buffered in the returned result. They are independent of `:tty`; a PTY
changes command semantics and may merge stderr into stdout.

## Caching

`Bsdkrun.Cache` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** — it
comes back as `{:ok, %{restored: false}}`.

```elixir
alias Bsdkrun.Cache

key = "deps-" <> lock_hash
{:ok, hit} = Cache.restore("web", key: key, restore_keys: ["deps-"])

unless hit.restored do
  Bsdkrun.Sandbox.exec("web", ["npm", "ci"])
  Cache.save("web", "/app/node_modules", key: key, compression: "zstd")
end

Cache.list()          # every stored entry, newest first
Cache.remove([key])   # or Cache.remove([], all: true)
```

`:restore_keys` are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `hit.key` says which one was used.
Formats are `"gzip"` (default), `"zstd"`, `"estargz"` and `"none"`.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

`Bsdkrun.FileSystem` reads and writes files in the guest. Parent directories are
created for you, and everything is byte-exact — `read_file/2` returns a binary.

```elixir
alias Bsdkrun.FileSystem, as: FS

:ok = FS.write_file("web", "/app/main.py", "print('hi')")
{:ok, bytes} = FS.read_file("web", "/app/out.json")

:ok = FS.upload("web", "./src", "/app/src")                       # file or directory
:ok = FS.download("web", "/app/dist", "./dist", recursive: true)
```

`upload` looks at the local path to decide whether to recurse; `download` cannot
(the path is in the guest), so say so for a directory. A directory's *contents*
land in the destination: uploading `./src` to `/app/src` leaves the guest's
`/app/src` holding what `./src` holds.

Failures are `{:error, %Bsdkrun.Error{kind: :file_transfer}}`, whose `:label`
carries the offending path.

> Transfers ride the same in-guest agent as `exec`, so the sandbox must be
> running. A directory copy also needs `tar` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle & inventory

```elixir
{:ok, sbx}  = Bsdkrun.create(os: :linux, image: "alpine", command: ["sleep", "300"])
{:ok, same} = Bsdkrun.get(sbx.id)          # reconnect (prefix ok)
{:ok, list} = Bsdkrun.list(all: true)      # [%Bsdkrun.Types.SandboxInfo{}]

Bsdkrun.Sandbox.status(sbx)      # {:ok, %SandboxInfo{} | nil}
Bsdkrun.Sandbox.running?(sbx)    # boolean
Bsdkrun.logs(sbx)                # {:ok, console_log}
Bsdkrun.stop(sbx)                # BSD guests clean-poweroff; Linux SIGTERM
Bsdkrun.start(sbx)               # restart in place — resumes disk/rootfs
Bsdkrun.Sandbox.update(sbx, cpus: 4, mem: 2048)  # applies on next start
Bsdkrun.remove(sbx, force: true)
```

`SandboxInfo.kind` is an atom — `:linux`, `:freebsd`, `:netbsd`, `:firmware`,
`:kernel`, `:unikraft`, `:solo5`, `:nanos`, or `:osv` — the same vocabulary `create/1`
takes for `:os`, so you can match on it directly:
`case info.kind do :freebsd -> ...; :netbsd -> ...; _ -> ... end`.

## Pipe-friendly / chainable

Every `Bsdkrun.Sandbox` function has a bang (`!`) counterpart that unwraps
`{:ok, value}` or raises `Bsdkrun.Error`. The lifecycle ones — `stop!/1`,
`start!/1`, `remove!/2`, `update!/2`, `connect_network!/2`,
`disconnect_network!/1` — return `ref` itself (not `:ok`), so they chain:

```elixir
Bsdkrun.create!(os: :linux, image: "alpine")
|> Bsdkrun.exec!(["apk", "add", "curl"])
|> Bsdkrun.stop!()
```

An already-created machine can be joined to (or dropped from) a network the
same way — `Sandbox.connect_network!/2` and `Sandbox.disconnect_network!/1`
also return `ref`, so a network hop is one more link in the chain (it takes
effect on the next `start!/1`, same as `connect_network/2`):

```elixir
Bsdkrun.create!(os: :linux, image: "alpine")
|> Bsdkrun.Sandbox.connect_network!("devnet")
|> Bsdkrun.start!()
```

A **volume**, on the other hand, only ever gets attached at *boot* — the
`bsdkrun` CLI has no "attach to a running VM" for it (same for mounts,
ports, and extra disks). `Bsdkrun.Sandbox.new/1` plus `with_*/2` build up
those `create/1` options by pipe instead, so attaching a volume still reads
as one chain — it just ends at `create!/1` rather than starting from it:

```elixir
Bsdkrun.Sandbox.new(os: :linux, image: "alpine")
|> Bsdkrun.Sandbox.with_volume("web")
|> Bsdkrun.Sandbox.with_network("devnet")
|> Bsdkrun.Sandbox.with_port("8080:80")
|> Bsdkrun.Sandbox.create!()
|> Bsdkrun.exec!(["uname", "-a"])
```

Environment variables chain the same way, and **merge** rather than replace —
so a pipeline can build them up in pieces instead of the last call winning:

```elixir
Bsdkrun.Sandbox.new(os: :linux, image: "node:22")
|> Bsdkrun.Sandbox.with_env(%{"NODE_ENV" => "production"})
|> Bsdkrun.Sandbox.with_env("PORT", "3000")
|> Bsdkrun.Sandbox.with_command(["node", "server.js"])
|> Bsdkrun.Sandbox.create!()
```

`with_env/2` takes a map or a list of `{key, value}` pairs; `with_env/3` sets
one. Keys and values are stringified either way.

Nothing is sent to `bsdkrun` until `create/1`/`create!/1` runs. Besides
`with_volume/2`, `with_network/2`, `with_port(s)/2` and `with_env/2,3` above,
there's `with_mount(s)/2`, `with_disk/2`, `with_cpus/2`, `with_mem/2`,
`with_name/2`, `with_command/2`, and `with_opt/3` as an escape hatch for any
other `create/1` option.

`exec!/3`, `logs!/2`, `status!/1`, `Sandbox.ssh_setup!/2` and
`Sandbox.tailscale_up!/2` return their unwrapped value instead (a `Result`,
a log string, ...) since that's the point of calling them — chain into
`Bsdkrun.Types.Result` from there, or use `tap/2` to run one mid-pipeline
without losing the sandbox:

```elixir
Bsdkrun.create!(os: :linux, image: "alpine")
|> tap(&(Bsdkrun.exec!(&1, ["uname", "-a"]) |> Bsdkrun.Types.Result.text() |> IO.puts()))
|> Bsdkrun.stop!()
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

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `Bsdkrun.Client` is the
network sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token.

```elixir
client = Bsdkrun.Client.from_env!()  # BSDKRUN_URL / BSDKRUN_TOKEN
# or: client = Bsdkrun.Client.new(url: "http://vps.example.com:50052", token: "9f2c...")

{:ok, machines} = Bsdkrun.Client.list(client, true)  # same SandboxInfo Bsdkrun.list returns
{:ok, id} = Bsdkrun.Client.run_linux(client, image: "alpine", cpus: 2, mem: 1024, command: ["sleep", "300"])

{:ok, result} = Bsdkrun.Client.exec(client, id, ["uname", "-a"])
IO.puts(result.output)

Bsdkrun.Client.stop(client, id)
Bsdkrun.Client.remove(client, [id])
```

`Client.run_linux`/`run_bsd`/`run_nanos`/`run_unikraft`/`run_solo5`/`run_osv`/`run_flavor`
each take the same options (a keyword list or map) as the corresponding
GraphQL mutation (`daemon/src/graphql.rs`) — `run_bsd(client, os: :freebsd, ...)`,
etc. — and return the new machine's id. `stop`/`start`/`remove`/`update`/
`commit` return a `CommandResult` (`exit_code`, `stdout`, `stderr`).

For a live terminal instead of a one-shot `exec`, use `shell`:

```elixir
{:ok, session} = Bsdkrun.Client.shell(client, id)  # or shell(client, id, command: [...])
Bsdkrun.Client.Shell.write(session, "ls -la\n")
Bsdkrun.Client.Shell.resize(session, 50, 120)
receive do
  {:bsdkrun_shell, _id, {:data, bytes}} -> IO.write(bytes)
  {:bsdkrun_shell, _id, {:exit, code}} -> IO.puts("exited #{code}")
end
Bsdkrun.Client.Shell.close(session)
```

Live output (from `shell`, `follow_logs`, and the raw `subscribe` escape
hatch) delivers either as `{:bsdkrun_shell, id, event}`-style messages to the
calling process, or to an `opts[:on_data]` callback — your choice. Both
`exec`/`shell` and `follow_logs` are built on the same `openShell`/
`shellOutput` shell-session protocol the daemon uses for every interactive
terminal — see [`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed function yet (flavor/network/volume
management, for instance) — `Client.request(client, query, variables)` runs
any raw query or mutation, and `Client.subscribe(client, query, variables)`
runs any raw subscription, for anything not wrapped above.

Like the rest of this SDK, the remote client adds no new dependency —
`jason` (already a dependency for `--json` parsing) is the only one. HTTP
runs over Erlang/OTP's built-in `:httpc`, and subscriptions (used by `exec`/
`shell`/`follow_logs`) run over a hand-rolled `graphql-transport-ws` client
on `:gen_tcp`/`:ssl`, all part of the standard Erlang distribution — plus a
small supervision tree (Bsdkrun.Application, an internal, undocumented app
module) giving each `Client`'s shared
socket somewhere to live.

`Client.new/1`/`from_env/0` both reject a URL configured without a token
rather than silently making an unauthenticated request — set both
`BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

Every fallible function returns `{:ok, value}` or `{:error, %Bsdkrun.Error{}}`.
The `%Bsdkrun.Error{}` exception has a `:kind`:

- `:binary_not_found`  — the `bsdkrun` binary wasn't found.
- `:command_failed`    — a command exited non-zero (carries `:exit_code`,
  `:stdout`, `:stderr`, `:label`).
- `:sandbox_not_found` — `Bsdkrun.get/1` matched no machine.
- `:auth_error`        — a `Bsdkrun.Client` request's bearer token was rejected.
- `:graphql_error`     — any other `Bsdkrun.Client` request failure (carries
  `:code`, the daemon's `extensions.code`, when there is one).
- `:config_error`      — invalid or missing `Bsdkrun.Client.from_env/0` configuration.

The bang variants (`Bsdkrun.create!/1`, `Bsdkrun.Sandbox.get!/1`,
`Bsdkrun.Sandbox.list!/1`, `Bsdkrun.exec!/3`, `Bsdkrun.stop!/1`, ...) unwrap
the value or `raise` the error — see
[Pipe-friendly / chainable](#pipe-friendly--chainable) above for how the
lifecycle ones return `ref` for chaining.

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
