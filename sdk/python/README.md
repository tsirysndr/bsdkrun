# bsdkrun (Python SDK)

A Python SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive
microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

It's a thin wrapper that shells out to the `bsdkrun` binary, so it has **zero
runtime dependencies** — stdlib only, Python 3.10+.

```python
from bsdkrun import Sandbox

sbx = Sandbox.create(os="linux", image="alpine")

# exec argv directly, with env / stdin / a PTY / a working dir:
print(sbx.exec(["uname", "-a"]).text())
sbx.exec(["apk", "add", "curl"])
sbx.run_command("curl", ["-fsSL", "https://example.com"])

sbx.stop()
```

## Install

```sh
uv add bsdkrun     # or: pip install bsdkrun
```

Or from this repo:

```sh
uv add ./sdk/python     # or: pip install sdk/python
```

### The `bsdkrun` binary

You need the `bsdkrun` binary itself. The SDK finds it via, in order:

1. `set_binary_path("/path/to/bsdkrun")`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build

See the [bsdkrun README](../../README.md) for installing the binary (Homebrew on
macOS, or build from source on Linux/KVM). This SDK assumes libkrun is already
provisioned — it does not auto-install it.

## Creating a sandbox

`Sandbox.create` is keyed on `os` — the options change per guest kind:

```python
# Linux OCI image (docker run-style)
Sandbox.create(
    os="linux",
    image="ghcr.io/owner/name:tag",
    cpus=2,
    mem=1024,
    volume="web",  # persistent CoW rootfs
    mounts=["~/project:/src", "~/data:/data:ro"],
    net={"ports": ["8080:80", "2222:22"]},
    command=["node", "server.js"],  # args after `--`
)

# FreeBSD (EFI on macOS, PVH on Linux/amd64)
Sandbox.create(os="freebsd", version="14.3", mem=2048)

# NetBSD (direct-kernel boot everywhere)
Sandbox.create(os="netbsd", version="10.1", volume="db")

# Boot a raw disk through its UEFI loader
Sandbox.create(os="firmware", firmware="KRUN_EFI.fd", disk="disk.raw")

# Boot a kernel directly, no bootloader
Sandbox.create(os="kernel", kernel="netbsd", format="elf", disk="root.raw")
```

Every `create` runs the machine **detached** and returns a `Sandbox` handle.

### Environment variables

``env`` sets the guest environment for the machine's entrypoint. It is merged
over the image's own config, so a key the image already defines is replaced
rather than duplicated.

```python
sbx = Sandbox.create(
    os="linux",
    image="node:22",
    env={"NODE_ENV": "production", "PORT": "3000"},
    command=["node", "server.js"],
)
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from ``exec`` after boot. For a single command
rather than the whole machine, ``exec`` takes its own ``env``.

## Running commands

Pass an argv list (no shell parsing), or a program name plus `args`:

```python
import sys

sbx.exec(["ls", "-la", "/etc"])

sbx.exec(
    "node",
    args=["-e", "print(1)"],
    env={"X": "hi"},
    cwd="/app",
    stdin="data on stdin",
    tty=True,  # allocate a PTY
    throw_on_error=True,  # raise CommandFailed on a non-zero exit (default: False)
    on_stdout=lambda chunk: sys.stdout.buffer.write(chunk),
    on_stderr=lambda chunk: sys.stderr.buffer.write(chunk),
)

# Vercel-Sandbox-style alias:
result = sbx.run_command("uname", ["-a"])
print(result.stdout, result.exit_code)
```

`exec` returns a `Result` with `.stdout`, `.stderr`, `.exit_code`, `.ok`, and
helpers `.text()`, `.json()`, `.lines()`, `.throw_if_failed()`.

The stream callbacks receive `bytes` as they arrive while the complete output
is still captured in the returned `Result`. They are independent of `tty`;
allocating a TTY changes command behavior and may merge stderr into stdout.

## Caching

``sbx.cache`` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** —
check ``restored`` rather than catching.

```python
from bsdkrun import caches

key = f"deps-{lock_hash}"
hit = sbx.cache.restore(key=key, restore_keys=["deps-"])
if not hit.restored:
    sbx.exec(["npm", "ci"])
    sbx.cache.save("/app/node_modules", key=key, compression="zstd")

caches.ls()  # every stored entry, newest first
caches.rm([key])  # or caches.rm(all=True)
```

``restore_keys`` are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and ``hit.key`` says which one was used.
Formats are ``gzip`` (default), ``zstd``, ``estargz`` and ``none``.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

``sbx.fs`` reads and writes files in the guest. Parent directories are created
for you, and everything is byte-exact — ``read_file`` returns ``bytes``, so a
PNG survives the round trip.

```python
sbx.fs.write_file("/app/main.py", "print('hi')")
sbx.fs.write_file("/app/logo.png", png_bytes)

text = sbx.fs.read_text("/app/out.json")
data = sbx.fs.read_file("/app/logo.png")

sbx.fs.upload("./src", "/app/src")  # file or directory
sbx.fs.download("/app/dist", "./dist", recursive=True)
```

``upload`` looks at the local path to decide whether to recurse; ``download``
cannot (the path is in the guest), so pass ``recursive=True`` for a directory. A
directory's *contents* land in the destination: ``upload("./src", "/app/src")``
leaves the guest's ``/app/src`` holding what ``./src`` holds.

Failures raise ``FileTransferError``, which carries the offending ``path``.

> Transfers ride the same in-guest agent as ``exec``, so the sandbox must be
> running. A directory copy also needs ``tar`` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle & inventory

```python
sbx = Sandbox.create(os="linux", image="alpine", command=["sleep", "300"])
same = Sandbox.get(sbx.id)  # reconnect (prefix ok)
rows = Sandbox.list(all=True)  # list[SandboxInfo]

sbx.status()  # SandboxInfo | None
sbx.is_running()  # bool
sbx.logs()  # console log (str)
sbx.shell()  # interactive shell (inherits the terminal)
sbx.stop()  # BSD guests clean-poweroff; Linux SIGTERM
sbx.start()  # restart in place — resumes its own disk/rootfs (data persists)
sbx.update(cpus=4, mem=2048)  # applies on next start
sbx.remove(force=True)
```

Host-level namespaces:

```python
from bsdkrun import images, volumes, networks, system

system.probe()  # toolchain sanity check
images.list()  # list[ImageInfo]
volumes.list()  # list[VolumeInfo]
volumes.remove("web", force=True)
networks.list()  # list[NetworkInfo]
system.fetch_image("freebsd", version="14.3")
system.versions("netbsd")
```

## Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one subnet
and reach each other **by IP and by name** (docker-compose style), with internal
DNS:

```python
from bsdkrun import Sandbox, networks

networks.create("devnet")

db = Sandbox.create(os="linux", image="postgres", name="db", net={"network": "devnet"})
api = Sandbox.create(os="linux", image="myapi", name="api", net={"network": "devnet"})

# `api` resolves `db` to its IP on devnet and pings it by name:
api.exec(["ping", "-c1", "db"], throw_on_error=True)

# inspect + manage
networks.list()  # list[NetworkInfo]
networks.members("devnet")  # list[SandboxInfo] on the network
info = db.status()  # info.network == "devnet", info.net_ip set

# edit membership (applies on next start — a VM's NIC is fixed at boot)
api.connect_network("devnet")  # or networks.connect(api.id, "devnet")
api.disconnect_network()
api.start()  # re-joins with the new membership

networks.sync("devnet")  # refresh members' /etc/hosts (fixes NetBSD name lookup)
networks.remove("devnet", force=True)
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD** resolves
via a synced `/etc/hosts` block — joins auto-sync, and `networks.sync` refreshes
an existing network without restarting members.

## SSH & Tailscale

```python
# agent-managed key-based SSH
sbx.ssh_setup()  # install local ~/.ssh/*.pub keys
sbx.ssh_setup(user="tsiry", key="~/.ssh/work.pub")

# put a guest on your tailnet
sbx.tailscale_up(authkey="tskey-auth-...", hostname="web")
```

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `Client` is the network
sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token.

```python
from bsdkrun import Client

client = Client(url="http://vps.example.com:50052", token="9f2c...")
# or, from BSDKRUN_URL / BSDKRUN_TOKEN:
client = Client.from_env()

machines = client.list(all=True)  # list[SandboxInfo] — same type Sandbox.list() returns
machine_id = client.run_linux(image="alpine", cpus=2, mem=1024, command=["sleep", "300"])

result = client.exec(machine_id, ["uname", "-a"])
print(result.output.decode(), result.exit_code)

client.stop(machine_id)
client.remove([machine_id])
```

`Client.run_linux`/`run_bsd`/`run_nanos`/`run_unikraft`/`run_solo5`/`run_osv`/
`run_flavor` each take the same keyword options as the corresponding GraphQL
mutation (`daemon/src/graphql.rs`) — `run_bsd(os="freebsd", ...)`, etc. — and
return the new machine's id. `run_solo5` boots a MirageOS unikernel under the
`solo5-hvt` tender rather than libkrun:
`run_solo5(path="dist/hello.hvt", args=["--ipv4=10.0.0.2/24"])`.
`stop`/`start`/`remove`/`update`/`commit` return a
`CommandResult(exit_code, stdout, stderr)`.

### Snapshots

A snapshot is a **copy-on-write clone of a machine's disk state** — instant to
take, free until the two sides diverge. `branch` boots a new machine from one
(or from a machine, which is snapshotted first); `restore`/`rollback` put one
back, leaving the machine stopped. A BSD guest is powered off to snapshot it:
a mounted UFS cannot be cloned consistently.

```python
snap = client.snapshot(machine_id, "before-upgrade")
client.snapshots(machine_id)  # newest first
branch_id = client.branch(snap.name, name="web-test")
client.restore(machine_id, snap.name)  # or client.rollback(machine_id)
client.remove_snapshots([snap.name])
```

### Docker

bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
socket, so the host's own `docker` CLI drives the same engine these calls do.
Starting is idempotent — the VM has a fixed name, so it resumes rather than
creating a second.

```python
status = client.docker_start(cpus=4, mem=4096)  # or just docker_status()
print(status.socket)  # export DOCKER_HOST=unix://...
for c in client.docker_containers():
    print(c.name, c.state, c.ports)
client.docker_container("restart", "web")
print(client.docker_logs("web", tail=50))
```

For a live terminal instead of a one-shot `exec`, use `shell()`:

```python
session = client.shell(machine_id)  # or shell(machine_id, command=[...]) for a non-login command
session.on_output(lambda data: print(data.decode(), end=""))
session.on_exit(lambda code: print(f"\nexited {code}"))
session.write(b"ls -la\n")
session.resize(rows=50, cols=120)
session.close()
```

`follow_logs(id, on_data=...)` streams a machine's console live instead of
the one-shot `logs(id)`. Both `exec`/`shell` and `follow_logs` are built on
the same `openShell`/`shellOutput` shell-session protocol the daemon uses for
every interactive terminal — see [`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed method yet (flavor/network/volume
management, for instance) — `client.request(query, variables)` runs any raw
query or mutation, and `client.subscribe(query, variables, on_next=...)` runs
any raw subscription, for anything not wrapped above.

Like the local SDK, `Client` has **zero runtime dependencies** — the HTTP
transport is stdlib `urllib`, and subscriptions (used by `exec`/`shell`/
`follow_logs`) run over a hand-rolled `graphql-transport-ws` WebSocket client
on top of stdlib `socket`/`ssl`, since Python's standard library has no
WebSocket client of its own.

`Client(url=..., token=...)` and `from_env()` both reject a URL configured
without a token rather than silently making an unauthenticated request — set
both `BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

All errors extend `BsdkrunError`:

- `BinaryNotFound` — the `bsdkrun` binary wasn't found.
- `CommandFailed` — a command exited non-zero (carries `exit_code`, `stdout`,
  `stderr`). Raised by `exec` when `throw_on_error=True`, by the lifecycle
  methods, and by the agent helpers.
- `SandboxNotFound` — `Sandbox.get` matched no machine.
- `GraphQLError` — a `Client` request failed (carries `code`, the daemon's
  `extensions.code`, when there is one).
- `AuthError` (a `GraphQLError`) — the daemon rejected the bearer token.

## Try it interactively

```sh
uv run console.py
```

Starts IPython with the SDK preloaded — `Sandbox`, the `images` / `volumes` /
`networks` / `system` namespaces, and a `ps()` shorthand for
`Sandbox.list(all=True)`. Pass `--bin ../../target/release/bsdkrun` to drive a
locally built binary for the session. Falls back to the stdlib REPL if IPython
isn't installed.

## Development

The SDK is developed with [uv](https://docs.astral.sh/uv/). From `sdk/python`:

```sh
uv sync            # create .venv and install the dev group
uv run pytest      # tests
uv run ruff check  # lint
uv run ruff format # format
uv run mypy        # type-check (strict)
```

The package itself has **no runtime dependencies** — `pytest`, `ruff`, and
`mypy` live in the `dev` dependency group and are never installed for consumers.

## License

MIT
