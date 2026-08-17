# bsdkrun (Ruby SDK)

A Ruby SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

The SDK shells out to the `bsdkrun` binary, so it has **zero runtime
dependencies** — just the Ruby standard library (`open3`, `json`, `pathname`).

```ruby
require "bsdkrun"

sbx = Bsdkrun::Sandbox.create(os: "linux", image: "alpine")

# exec argv directly, with env / stdin / a PTY / a working dir:
puts sbx.exec(["uname", "-a"]).text
sbx.exec(["apk", "add", "curl"], throw_on_error: true)
sbx.run_command("curl", ["-fsSL", "https://example.com"])

sbx.stop
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

### Environment variables

`env:` sets the guest environment for the machine's entrypoint. It is merged
over the image's own config, so a key the image already defines is replaced
rather than duplicated.

```ruby
sbx = Bsdkrun::Sandbox.create(
  os: "linux",
  image: "node:22",
  env: { "NODE_ENV" => "production", "PORT" => "3000" },
  command: ["node", "server.js"]
)
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. For a single command
rather than the whole machine, `exec` takes its own `env:`.

## Running commands

`exec` is the primary programmatic entrypoint. No shell parsing — pass an argv
array (or a program name plus `args:`).

```ruby
sbx.exec(["ls", "-la", "/etc"])

sbx.exec("ruby",
  args: ["-e", "puts ENV['X']"],
  env: { "X" => "hi" },
  cwd: "/app",
  stdin: "data on stdin",
  tty: true,                 # allocate a PTY
  on_stdout: ->(chunk) { $stdout.write(chunk) },
  on_stderr: ->(chunk) { $stderr.write(chunk) },
  throw_on_error: true)      # raise on non-zero exit (default: false)

# Vercel-Sandbox-style alias:
result = sbx.run_command("uname", ["-a"])
result.stdout       # raw stdout
result.text         # stdout, trailing newlines trimmed
result.exit_code
result.ok?          # true on exit 0
result.lines        # non-empty stdout lines
```

`exec` returns a `Bsdkrun::Result`. It only raises `CommandFailed` when you pass
`throw_on_error: true` (or call `result.throw_if_failed!`).

The callbacks run as chunks arrive, and the same bytes remain buffered in the
returned result. They do not require `tty`; a PTY changes command semantics and
may merge stderr into stdout.

## Caching

`sbx.cache` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** —
check `restored` rather than rescuing.

```ruby
key = "deps-#{lock_hash}"
hit = sbx.cache.restore(key: key, restore_keys: ["deps-"])
unless hit.restored
  sbx.exec(["npm", "ci"])
  sbx.cache.save("/app/node_modules", key: key, compression: "zstd")
end

Bsdkrun::Caches.ls          # every stored entry, newest first
Bsdkrun::Caches.rm([key])   # or Bsdkrun::Caches.rm(all: true)
```

`restore_keys` are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `hit.key` says which one was used.
Formats are `gzip` (default), `zstd`, `estargz` and `none`.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

`sbx.fs` reads and writes files in the guest. Parent directories are created
for you, and everything is byte-exact — `read_file` returns a binary string.

```ruby
sbx.fs.write_file("/app/main.py", "print('hi')")
sbx.fs.write_file("/app/logo.png", png_bytes)

text  = sbx.fs.read_text("/app/out.json")
bytes = sbx.fs.read_file("/app/logo.png")

sbx.fs.upload("./src", "/app/src")                       # file or directory
sbx.fs.download("/app/dist", "./dist", recursive: true)
```

`upload` looks at the local path to decide whether to recurse; `download` cannot
(the path is in the guest), so say so for a directory. A directory's *contents*
land in the destination: uploading `./src` to `/app/src` leaves the guest's
`/app/src` holding what `./src` holds.

Failures raise `Bsdkrun::FileTransferFailed`, which carries the offending `path`.

> Transfers ride the same in-guest agent as `exec`, so the sandbox must be
> running. A directory copy also needs `tar` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle & inventory

```ruby
sbx  = Bsdkrun::Sandbox.create(os: "linux", image: "alpine", command: ["sleep", "300"])
same = Bsdkrun::Sandbox.get(sbx.id)          # reconnect (prefix ok)
all  = Bsdkrun::Sandbox.list(all: true)      # Array<SandboxInfo>

sbx.status        # SandboxInfo | nil
sbx.running?      # true / false
sbx.logs          # console log (String)
sbx.shell         # interactive shell (inherits the terminal)
sbx.stop          # BSD guests clean-poweroff; Linux SIGTERM
sbx.start         # restart in place — resumes its own disk/rootfs (data persists)
sbx.update(cpus: 4, mem: 2048)               # applies on next start
sbx.remove(force: true)
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
sbx.ssh_setup                                  # install local ~/.ssh/*.pub keys
sbx.ssh_setup(user: "tsiry", key: "~/.ssh/work.pub")

# put a guest on your tailnet
sbx.tailscale_up(authkey: "tskey-auth-...", hostname: "web")
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

`client.run_linux`/`run_bsd`/`run_nanos`/`run_unikraft`/`run_solo5`/`run_osv`/
`run_flavor` each take the same options as the corresponding GraphQL mutation
(`daemon/src/graphql.rs`) — `run_bsd(os: "freebsd", ...)`, etc. — and return
the new machine's id. `run_solo5` boots a MirageOS unikernel under the
`solo5-hvt` tender rather than libkrun:
`run_solo5(path: "dist/hello.hvt", args: ["--ipv4=10.0.0.2/24"])`.
`stop`/`start`/`remove`/`update`/`commit` return a
`CommandResult` (`exit_code`, `stdout`, `stderr`).

### Snapshots

A snapshot is a **copy-on-write clone of a machine's disk state** — instant to
take, free until the two sides diverge. `branch` boots a new machine from one
(or from a machine, which is snapshotted first); `restore`/`rollback` put one
back, leaving the machine stopped. A BSD guest is powered off to snapshot it:
a mounted UFS cannot be cloned consistently.

```ruby
snap = client.snapshot(machine_id, name: "before-upgrade")
client.snapshots(machine: machine_id)      # newest first
branch_id = client.branch(snap.name, name: "web-test")
client.restore(machine_id, snap.name)      # or client.rollback(machine_id)
client.remove_snapshots(snap.name)
```

### Docker

bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
socket, so the host's own `docker` CLI drives the same engine these calls do.
Starting is idempotent — the VM has a fixed name, so it resumes rather than
creating a second.

```ruby
status = client.docker_start(cpus: 4, mem: 4096)   # or just docker_status
puts status.socket
client.docker_containers.each { |c| puts "#{c.name} #{c.state} #{c.ports}" }
client.docker_container("restart", "web")
puts client.docker_logs("web", tail: 50)
```

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
