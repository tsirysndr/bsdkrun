# bsdkrun (Python SDK)

A Python SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive
microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

It's a thin wrapper that shells out to the `bsdkrun` binary, so it has **zero
runtime dependencies** — stdlib only, Python 3.10+.

```python
from bsdkrun import Sandbox

box = Sandbox.create(os="linux", image="alpine")

# exec argv directly, with env / stdin / a PTY / a working dir:
print(box.exec(["uname", "-a"]).text())
box.exec(["apk", "add", "curl"])
box.run_command("curl", ["-fsSL", "https://example.com"])

box.stop()
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

## Running commands

Pass an argv list (no shell parsing), or a program name plus `args`:

```python
box.exec(["ls", "-la", "/etc"])

box.exec(
    "node",
    args=["-e", "print(1)"],
    env={"X": "hi"},
    cwd="/app",
    stdin="data on stdin",
    tty=True,  # allocate a PTY
    throw_on_error=True,  # raise CommandFailed on a non-zero exit (default: False)
)

# Vercel-Sandbox-style alias:
result = box.run_command("uname", ["-a"])
print(result.stdout, result.exit_code)
```

`exec` returns a `Result` with `.stdout`, `.stderr`, `.exit_code`, `.ok`, and
helpers `.text()`, `.json()`, `.lines()`, `.throw_if_failed()`.

## Lifecycle & inventory

```python
box = Sandbox.create(os="linux", image="alpine", command=["sleep", "300"])
same = Sandbox.get(box.id)  # reconnect (prefix ok)
rows = Sandbox.list(all=True)  # list[SandboxInfo]

box.status()  # SandboxInfo | None
box.is_running()  # bool
box.logs()  # console log (str)
box.shell()  # interactive shell (inherits the terminal)
box.stop()  # BSD guests clean-poweroff; Linux SIGTERM
box.start()  # restart in place — resumes its own disk/rootfs (data persists)
box.update(cpus=4, mem=2048)  # applies on next start
box.remove(force=True)
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
box.ssh_setup()  # install local ~/.ssh/*.pub keys
box.ssh_setup(user="tsiry", key="~/.ssh/work.pub")

# put a guest on your tailnet
box.tailscale_up(authkey="tskey-auth-...", hostname="web")
```

## Errors

All errors extend `BsdkrunError`:

- `BinaryNotFound` — the `bsdkrun` binary wasn't found.
- `CommandFailed` — a command exited non-zero (carries `exit_code`, `stdout`,
  `stderr`). Raised by `exec` when `throw_on_error=True`, by the lifecycle
  methods, and by the agent helpers.
- `SandboxNotFound` — `Sandbox.get` matched no machine.

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
