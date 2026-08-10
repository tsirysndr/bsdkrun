# bsdkrun (Ruby SDK)

A Ruby SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

The SDK shells out to the `bsdkrun` binary, so it has **zero runtime
dependencies** — just the Ruby standard library (`open3`, `json`, `pathname`).

```ruby
require "bsdkrun"

box = Bsdkrun::Sandbox.create(os: "linux", image: "alpine")

# exec argv directly, with env / stdin / a PTY / a working dir:
puts box.exec(["uname", "-a"]).text
box.exec(["apk", "add", "curl"], throw_on_error: true)
box.run_command("curl", ["-fsSL", "https://example.com"])

box.stop
```

## Install

```sh
gem install bsdkrun
```

Or in a `Gemfile`:

```ruby
gem "bsdkrun"
```

### The `bsdkrun` binary

You need the `bsdkrun` binary itself. The SDK finds it via, in order:

1. `Bsdkrun.binary_path = "/path/to/bsdkrun"`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build

See the [bsdkrun README](../../README.md) for installing the binary (Homebrew on
macOS, or build from source on Linux/KVM). This SDK assumes libkrun is already
linked — it does not auto-provision it.

## Creating a sandbox

`Sandbox.create` is discriminated on `os:` — the options change per guest kind.
Pass keyword arguments or a Hash.

```ruby
# Linux OCI image (docker run-style)
Bsdkrun::Sandbox.create(
  os: "linux",
  image: "ghcr.io/owner/name:tag",
  cpus: 2,
  mem: 1024,
  volume: "web",                       # persistent CoW rootfs
  mounts: ["~/project:/src", "~/data:/data:ro"],
  net: { ports: ["8080:80", "2222:22"] },
  command: ["node", "server.js"]       # args after `--`
)

# FreeBSD (EFI on macOS, PVH on Linux/amd64)
Bsdkrun::Sandbox.create(os: "freebsd", version: "14.3", mem: 2048)

# NetBSD (direct-kernel boot everywhere)
Bsdkrun::Sandbox.create(os: "netbsd", version: "10.1", volume: "db")

# Boot a raw disk through its UEFI loader
Bsdkrun::Sandbox.create(os: "firmware", firmware: "KRUN_EFI.fd", disk: "disk.raw")

# Boot a kernel directly, no bootloader
Bsdkrun::Sandbox.create(os: "kernel", kernel: "netbsd", format: "elf", disk: "root.raw")
```

Every `create` runs the machine **detached** and returns a `Sandbox` handle.

## Running commands

`exec` is the primary programmatic entrypoint. No shell parsing — pass an argv
array (or a program name plus `args:`).

```ruby
box.exec(["ls", "-la", "/etc"])

box.exec("ruby",
  args: ["-e", "puts ENV['X']"],
  env: { "X" => "hi" },
  cwd: "/app",
  stdin: "data on stdin",
  tty: true,                 # allocate a PTY
  throw_on_error: true)      # raise on non-zero exit (default: false)

# Vercel-Sandbox-style alias:
result = box.run_command("uname", ["-a"])
result.stdout       # raw stdout
result.text         # stdout, trailing newlines trimmed
result.exit_code
result.ok?          # true on exit 0
result.lines        # non-empty stdout lines
```

`exec` returns a `Bsdkrun::Result`. It only raises `CommandFailed` when you pass
`throw_on_error: true` (or call `result.throw_if_failed!`).

## Lifecycle & inventory

```ruby
box  = Bsdkrun::Sandbox.create(os: "linux", image: "alpine", command: ["sleep", "300"])
same = Bsdkrun::Sandbox.get(box.id)          # reconnect (prefix ok)
all  = Bsdkrun::Sandbox.list(all: true)      # Array<SandboxInfo>

box.status        # SandboxInfo | nil
box.running?      # true / false
box.logs          # console log (String)
box.shell         # interactive shell (inherits the terminal)
box.stop          # BSD guests clean-poweroff; Linux SIGTERM
box.start         # restart in place — resumes its own disk/rootfs (data persists)
box.update(cpus: 4, mem: 2048)               # applies on next start
box.remove(force: true)
```

Host-level namespaces:

```ruby
Bsdkrun::System.probe                          # toolchain sanity check -> Boolean
Bsdkrun::Images.list                           # Array<ImageInfo>
Bsdkrun::Volumes.list                          # Array<VolumeInfo>
Bsdkrun::Volumes.remove("web", force: true)
Bsdkrun::Networks.list                         # Array<NetworkInfo>
Bsdkrun::System.fetch_image("freebsd", version: "14.3")
Bsdkrun::System.versions("netbsd")             # Array<String>
```

## Networking, SSH & Tailscale

```ruby
# forward ports at create time
Bsdkrun::Sandbox.create(os: "linux", image: "alpine", net: { ports: ["2222:22"] })

# agent-managed key-based SSH
box.ssh_setup                                  # install local ~/.ssh/*.pub keys
box.ssh_setup(user: "tsiry", key: "~/.ssh/work.pub")

# put a guest on your tailnet
box.tailscale_up(authkey: "tskey-auth-...", hostname: "web")
```

### Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one subnet
and reach each other **by IP and by name** (docker-compose style), with internal
DNS:

```ruby
require "bsdkrun"

Bsdkrun::Networks.create("devnet")

db = Bsdkrun::Sandbox.create(
  os: "linux", image: "alpine", name: "db",
  net: { network: "devnet" }, command: ["sleep", "3600"]
)
api = Bsdkrun::Sandbox.create(
  os: "linux", image: "alpine", name: "api",
  net: { network: "devnet" }, command: ["sleep", "3600"]
)

# api reaches db by name over the shared subnet
api.exec(["ping", "-c1", "db"], throw_on_error: true)

# inspect + manage
Bsdkrun::Networks.list                # Array<NetworkInfo>
Bsdkrun::Networks.members("devnet")   # Array<SandboxInfo> on the network
info = db.status                      # info.network == "devnet", info.net_ip set

# edit membership (applies on next start — a VM's NIC is fixed at boot)
api.connect_network("devnet")         # or Bsdkrun::Networks.connect(api.id, "devnet")
api.disconnect_network
api.start                             # re-joins with the new membership

Bsdkrun::Networks.sync("devnet")      # refresh members' /etc/hosts (fixes NetBSD name lookup)
Bsdkrun::Networks.remove("devnet", force: true)
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD** resolves
via a synced `/etc/hosts` block — joins auto-sync, and `Networks.sync` refreshes
an existing network without restarting members.

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `Bsdkrun::Client` is the
network sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token.

```ruby
require "bsdkrun"

client = Bsdkrun::Client.new(url: "http://vps.example.com:50052", token: "9f2c...")
# or, from BSDKRUN_URL / BSDKRUN_TOKEN:
client = Bsdkrun::Client.from_env

machines = client.list(all: true)  # Array<SandboxInfo> — same type Sandbox.list returns
id = client.run_linux(image: "alpine", cpus: 2, mem: 1024, command: ["sleep", "300"])

result = client.exec(id, ["uname", "-a"])
puts result.output, result.exit_code

client.stop(id)
client.remove([id])
```

`client.run_linux`/`run_bsd`/`run_nanos`/`run_unikraft`/`run_osv`/`run_flavor`
each take the same options as the corresponding GraphQL mutation
(`daemon/src/graphql.rs`) — `run_bsd(os: "freebsd", ...)`, etc. — and return
the new machine's id. `stop`/`start`/`remove`/`update`/`commit` return a
`CommandResult` (`exit_code`, `stdout`, `stderr`).

For a live terminal instead of a one-shot `exec`, use `shell`:

```ruby
session = client.shell(id)  # or shell(id, command: [...]) for a non-login command
session.on_output { |bytes| $stdout.write(bytes) }
session.on_exit { |code| puts "\nexited #{code}" }
session.write("ls -la\n")
session.resize(50, 120)
session.close
```

`follow_logs(id) { |bytes| ... }` streams a machine's console live instead of
the one-shot `logs(id)`. Both `exec`/`shell` and `follow_logs` are built on
the same `openShell`/`shellOutput` shell-session protocol the daemon uses for
every interactive terminal — see [`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed method yet (flavor/network/volume
management, for instance) — `client.request(query, variables)` runs any raw
query or mutation, and `client.subscribe(query, variables, on_next: ...)` runs
any raw subscription, for anything not wrapped above.

Like the rest of this gem, `Client` uses **only the Ruby standard library** —
the HTTP transport is `Net::HTTP`, and subscriptions (used by `exec`/`shell`/
`follow_logs`) run over a hand-rolled `graphql-transport-ws` client on top of
`Socket`/`OpenSSL`, since Ruby's standard library has no WebSocket client of
its own.

`Client.new(url:, token:)` and `.from_env` both reject a URL configured
without a token rather than silently making an unauthenticated request — set
both `BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

All errors extend `Bsdkrun::Error`:

- `Bsdkrun::BinaryNotFound` — the `bsdkrun` binary wasn't found.
- `Bsdkrun::CommandFailed` — a command exited non-zero (carries `exit_code`,
  `stdout`, `stderr`, `command`). Raised by `exec` with `throw_on_error: true`,
  by the lifecycle/namespace helpers, and by the agent helpers.
- `Bsdkrun::SandboxNotFound` — `Sandbox.get` matched no machine.
- `Bsdkrun::GraphQLError` — a `Client` request failed (carries `code`, the
  daemon's `extensions.code`, when there is one).
- `Bsdkrun::AuthError` (a `GraphQLError`) — the daemon rejected the bearer token.

## Try it interactively

```sh
bin/console
```

Starts IRB with the SDK preloaded — `Bsdkrun::Sandbox`, the `Bsdkrun.images` /
`.volumes` / `.networks` / `.system` namespaces, plus `ps` (every machine,
exited ones included) and `last` (the newest one). Pass
`--bin ../../target/release/bsdkrun` to drive a locally built binary for the
session.

## License

MIT
