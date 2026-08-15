# bsdkrun — Gleam SDK

[![Package Version](https://img.shields.io/hexpm/v/bsdkrun)](https://hex.pm/packages/bsdkrun)
[![Hex Docs](https://img.shields.io/badge/hex-docs-ffaff3)](https://hexdocs.pm/bsdkrun/)

A Gleam SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun).

The SDK is a thin, stateless wrapper around the `bsdkrun` binary: it builds
argv, shells out through an Erlang port, and decodes the JSON output. There is
no daemon and no long-lived state — every call is one CLI invocation.

> **Erlang target only.** The SDK spawns a subprocess, so it does not run on the
> JavaScript target.

## Install

```sh
gleam add bsdkrun
```

You also need the `bsdkrun` binary itself — see
[the install instructions](https://github.com/tsirysndr/bsdkrun#install):

```sh
brew install tsirysndr/tap/bsdkrun
```

### Finding the binary

Resolution order, first match wins:

| # | Source                                                             |
| - | ------------------------------------------------------------------ |
| 1 | `binary.set_binary_path("…")`                                      |
| 2 | the `$BSDKRUN_BIN` environment variable                            |
| 3 | `bsdkrun` on `$PATH`                                               |
| 4 | an in-repo dev build — `target/release/bsdkrun`, then `debug`      |

If nothing matches you get `error.BinaryNotFound`, listing every path tried.

## Quick start

```gleam
import bsdkrun
import bsdkrun/args
import bsdkrun/types

pub fn main() {
  let assert Ok(sbx) = bsdkrun.create(args.linux("alpine"))
  let assert Ok(res) = bsdkrun.exec(sbx, ["uname", "-a"])
  echo types.text(res)
  let assert Ok(sbx) = bsdkrun.stop(sbx)
}
```

Every fallible call returns `Result(a, bsdkrun/error.Error)`; nothing in the
SDK panics on its own. Render an error with `error.to_string`.

## Creating machines

`bsdkrun/args` builds the create options. Start from a per-guest constructor and
refine with the `with_*` helpers:

```gleam
import bsdkrun/args

// an OCI image, as a microVM
args.linux("alpine")
|> args.with_name("web")
|> args.with_cpus(2)
|> args.with_mem(2048)
|> args.with_ports([args.Port(8080, 80)])
|> args.with_mounts(["/host/src:/src"])
|> args.with_command(["sh", "-c", "httpd -f"])

// FreeBSD, on a persistent disk
args.freebsd()
|> args.with_version("15.0")
|> args.with_persist(True)

// NetBSD
args.netbsd()

// an arbitrary disk, booted through UEFI firmware
args.firmware("/path/edk2.fd", "/path/disk.raw")

// an arbitrary kernel, no bootloader
args.kernel("/path/vmlinuz")
```

Setters that do not apply to the chosen guest kind are ignored rather than
rejected — `with_command` on a NetBSD guest is a no-op, since only Linux guests
take a trailing command.

A volume, a mount, or a port forward has no runtime "attach" in `bsdkrun` —
they're only ever chosen at boot — so `with_volume`/`with_network`/`with_port`
before `create` *is* how one gets attached "by pipe":

```gleam
args.linux("alpine")
|> args.with_volume("web")
|> args.with_network("devnet")
|> args.with_ports([args.Port(8080, 80)])
|> sandbox.create
```

### Environment variables

`with_env` sets the guest environment for the machine's entrypoint. It is merged
over the image's own config, so a key the image already defines is replaced
rather than duplicated.

```gleam
let assert Ok(sbx) =
  args.linux("node:22")
  |> args.with_env([#("NODE_ENV", "production"), #("PORT", "3000")])
  |> args.with_command(["node", "server.js"])
  |> sandbox.create
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. Pairs are emitted sorted
by key, so the argv does not depend on the order you added them.

## Running commands

`bsdkrun.exec` covers the common case. For env vars, a TTY, stdin, or a working
directory, use `sandbox.exec` with `sandbox.exec_options()`:

```gleam
import bsdkrun/sandbox
import bsdkrun/types

let assert Ok(res) =
  sandbox.exec(
    sbx,
    ["sh", "-c", "cat > out.txt && wc -l < out.txt"],
    sandbox.exec_options()
      |> sandbox.with_env([#("RUST_LOG", "debug")])
      |> sandbox.with_stdin("one\ntwo\n")
      |> sandbox.with_stdout(fn(chunk) { io.print(chunk) })
      |> sandbox.with_stderr(fn(chunk) { io.print_error(chunk) })
      |> sandbox.with_cwd("/tmp"),
  )

types.text(res)      // stdout, trailing newlines trimmed
types.lines(res)     // non-empty stdout lines
types.is_ok(res)     // exit_code == 0
res.exit_code
res.stderr
```

A non-zero exit is **not** an error by default — it comes back in the
`CommandResult`. Pass `sandbox.with_fail_on_error(True)` to turn it into
`error.CommandFailed` instead.

The callbacks receive chunks as they arrive, while the completed
`CommandResult` still contains all stdout and stderr. Streaming is independent
of `with_tty`; a PTY changes command semantics and may merge stderr into
stdout.

## Caching

`bsdkrun/cache` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** — it
comes back as `Ok` with `restored: False`.

```gleam
import bsdkrun/cache
import gleam/option.{None, Some}

let assert Ok(hit) = cache.restore("web", key, None, ["deps-"])
case hit.restored {
  False -> cache.save("web", "/app/node_modules", key, cache.Zstd, False)
  True -> Ok(cache.CacheEntry("", "", "", 0, 0, ""))
}

cache.list()               // every stored entry, newest first
cache.remove([key], False) // or ([], True) for all
```

The restore keys are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `hit.key` says which one was used.
Formats are `Gzip` (default), `Zstd`, `Estargz` and `Uncompressed`.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

`bsdkrun/filesystem` reads and writes files in the guest. Parent directories are
created for you, and everything is byte-exact — `read_file` returns a
`BitArray`, because a Gleam `String` must be valid UTF-8 and a PNG is not.

```gleam
import bsdkrun/filesystem

let assert Ok(Nil) = filesystem.write_text("web", "/app/main.py", "print(1)")
let assert Ok(bytes) = filesystem.read_file("web", "/app/logo.png")
let assert Ok(text) = filesystem.read_text("web", "/app/out.json")

let assert Ok(Nil) = filesystem.upload("web", "./src", "/app/src", False)
let assert Ok(Nil) = filesystem.download("web", "/app/dist", "./dist", True)
```

Both `upload` and `download` take an explicit `recursive` flag — Gleam has no
stat in its standard library, so it cannot be inferred. A directory's *contents*
land in the destination: uploading `./src` to `/app/src` leaves the guest's
`/app/src` holding what `./src` holds.

Failures are `error.FileTransferFailed(path, message)`.

> Transfers ride the same in-guest agent as `exec`, so the sandbox must be
> running. A directory copy also needs `tar` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle

```gleam
bsdkrun.stop(sbx)              // Ok(sbx) back — not Ok(Nil)
bsdkrun.start(sbx)             // restart in place: same id, disk, resources
bsdkrun.remove(sbx, True)      // force: stop first if running
bsdkrun.status(sbx)            // Ok(Some(SandboxInfo)) or Ok(None) if gone
bsdkrun.is_running(sbx)
bsdkrun.logs(sbx)              // console log
sandbox.boot_logs(sbx)         // bsdkrun's own boot log
sandbox.update(sbx, Some(4), Some(4096))  // cpus, mem — applies on next start
sandbox.connect_network(sbx, "devnet")    // join/switch — applies on next start
sandbox.disconnect_network(sbx)
sandbox.shell(sbx)             // interactive shell, inherits stdio
```

`stop`, `start`, `remove`, `update`, `connect_network` and `disconnect_network`
all return `Result(Sandbox, Error)` — the same `sbx`, not `Nil` — so a
sequence of them chains with `|>` through `gleam/result.try` instead of
re-threading `sbx` by hand:

```gleam
import gleam/result

bsdkrun.create(args.linux("alpine"))
|> result.try(sandbox.connect_network(_, "devnet"))
|> result.try(sandbox.start)
|> result.try(bsdkrun.exec(_, ["uname", "-a"]))
```

Reconnect to a machine you already booted with `bsdkrun.get(id)` — a unique id
prefix is enough — or enumerate with `bsdkrun.list()` / `bsdkrun.list_all(True)`.

## Host operations

```gleam
import bsdkrun/images
import bsdkrun/networks
import bsdkrun/system
import bsdkrun/volumes

images.list()

volumes.list()
volumes.remove(["scratch"], False)

networks.list()
networks.create("lab")
networks.connect("web", "lab")
networks.members("lab")
networks.sync("lab")           // refresh name resolution without restarting
networks.disconnect("web")
networks.remove(["lab"], False)

system.probe()                 // does the toolchain work? does not boot
system.versions(system.Freebsd)
system.fetch_image(system.Netbsd, Some("10.1"), None, False)
system.grow_disk("/path/disk.raw", "20G")
```

Machines on a global network reach each other by name: Linux and FreeBSD
resolve via the network's DNS, NetBSD via a synced `/etc/hosts` block.

## SSH & Tailscale

```gleam
// install key-based SSH via the guest agent
sandbox.ssh_setup(sbx, None, [])                       // your local ~/.ssh/*.pub
sandbox.ssh_setup(sbx, Some("tsiry"), ["~/.ssh/work.pub"])

// put the guest on your tailnet
sandbox.tailscale_up(sbx, Some("tskey-auth-…"), Some("web"), [])
```

The Tailscale auth key travels in the environment as `TS_AUTHKEY`, so it never
appears in an argument list.

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `bsdkrun/client` is the
network sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token. Erlang target only.

```gleam
import bsdkrun/client

let assert Ok(c) = client.from_env()  // BSDKRUN_URL / BSDKRUN_TOKEN
// or: let c = client.new(url: "http://vps.example.com:50052", token: "9f2c...")

let assert Ok(machines) = client.list(c, all: True)  // List(SandboxInfo) — same type sandbox.list returns

let opts =
  client.RunLinuxOptions(
    ..client.run_linux_options("alpine"),
    cpus: Some(2),
    mem: Some(1024),
    command: ["sleep", "300"],
  )
let assert Ok(id) = client.run_linux(c, opts: opts)

let assert Ok(result) = client.exec(c, id: id, command: ["uname", "-a"], env: [])
io.println(bit_array.to_string(result.output) |> result.unwrap(""))

let assert Ok(_) = client.stop(c, id: id)
let assert Ok(_) = client.remove(c, ids: [id], force: False)
```

`client.run_linux`/`run_bsd`/`run_nanos`/`run_unikraft`/`run_solo5`/`run_osv`/`run_flavor`
each take an `Options` record built from a `*_options(...)` default
constructor (`run_bsd_options`, `run_nanos_options`, ...) and Gleam's record
update syntax, matching the corresponding GraphQL mutation's fields
(`daemon/src/graphql.rs`). `stop`/`start`/`remove`/`update`/`commit` return a
`CommandResult` (`exit_code`, `stdout`, `stderr`).

For a live terminal instead of a one-shot `exec`, use `shell`:

```gleam
let assert Ok(session) = client.shell(c, id: id, command: None, env: [], rows: 24, cols: 80)
process.spawn(fn() {
  let assert Ok(event) = subject.receive(client.shell_output(session), -1)
  // ShellData(bytes) | ShellExit(code)
})
let assert Ok(_) = client.shell_send(session, <<"ls -la\n":utf8>>)
let assert Ok(_) = client.shell_resize(session, rows: 50, cols: 120)
client.shell_close(session)
```

`follow_logs` streams a machine's console live instead of the one-shot
`logs`. Both `exec`/`shell` and `follow_logs` are built on the same
`openShell`/`shellOutput` shell-session protocol the daemon uses for every
interactive terminal — see [`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed function yet (flavor/network/volume
management, for instance) — `client.request(c, query, variables)` runs any
raw query or mutation, for anything not wrapped above.

Like the rest of this package, the remote client adds **no new Hex
dependency** — HTTP is Erlang/OTP's built-in `:httpc`, and subscriptions
(used by `exec`/`shell`/`follow_logs`) run over a hand-rolled
`graphql-transport-ws` client on `:gen_tcp`/`:ssl`/`:crypto`, all part of the
standard Erlang distribution.

`client.new`/`from_env` both reject a URL configured without a token rather
than silently making an unauthenticated request — set both `BSDKRUN_URL` and
`BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

`bsdkrun/error.Error` is a single type with one variant per failure mode:

| Variant           | Meaning                                                     |
| ----------------- | ----------------------------------------------------------- |
| `BinaryNotFound`  | the `bsdkrun` binary wasn't found; carries the paths tried   |
| `CommandFailed`   | a command exited non-zero; carries code, stdout, stderr      |
| `SandboxNotFound` | no machine matched the given id or prefix                    |
| `DecodeFailed`    | `--json` output could not be decoded; carries the raw text   |
| `InvalidOptions`  | the create options were inconsistent, e.g. an empty image    |
| `GraphqlError`    | a `bsdkrun/client` request failed; carries the daemon's `extensions.code` when there is one |
| `AuthError`       | the daemon rejected the bearer token                        |

## Development

```sh
gleam test    # unit tests, incl. real subprocess round-trips through the FFI
gleam format  # format
gleam check   # type-check
gleam docs build
```

The subprocess tests use `test/support/fake-bsdkrun`, a small shell script that
absorbs the SDK's global `--log-level` prefix and then behaves like `sh`, so
stdout, stderr, stdin and exit codes can all be driven from a shell snippet.

## License

MIT
