# bsdkrun — Gleam SDK

[![Package Version](https://img.shields.io/hexpm/v/bsdkrun)](https://hex.pm/packages/bsdkrun)
[![Hex Docs](https://img.shields.io/badge/hex-docs-ffaff3)](https://hexdocs.pm/bsdkrun/)

A Gleam SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
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
  let assert Ok(box) = bsdkrun.create(args.linux("alpine"))
  let assert Ok(res) = bsdkrun.exec(box, ["uname", "-a"])
  echo types.text(res)
  let assert Ok(Nil) = bsdkrun.stop(box)
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

## Running commands

`bsdkrun.exec` covers the common case. For env vars, a TTY, stdin, or a working
directory, use `sandbox.exec` with `sandbox.exec_options()`:

```gleam
import bsdkrun/sandbox
import bsdkrun/types

let assert Ok(res) =
  sandbox.exec(
    box,
    ["sh", "-c", "cat > out.txt && wc -l < out.txt"],
    sandbox.exec_options()
      |> sandbox.with_env([#("RUST_LOG", "debug")])
      |> sandbox.with_stdin("one\ntwo\n")
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

## Lifecycle

```gleam
bsdkrun.stop(box)
bsdkrun.start(box)             // restart in place: same id, disk, resources
bsdkrun.remove(box, True)      // force: stop first if running
bsdkrun.status(box)            // Ok(Some(SandboxInfo)) or Ok(None) if gone
bsdkrun.is_running(box)
bsdkrun.logs(box)              // console log
sandbox.boot_logs(box)         // bsdkrun's own boot log
sandbox.update(box, Some(4), Some(4096))  // cpus, mem — applies on next start
sandbox.shell(box)             // interactive shell, inherits stdio
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
sandbox.ssh_setup(box, None, [])                       // your local ~/.ssh/*.pub
sandbox.ssh_setup(box, Some("tsiry"), ["~/.ssh/work.pub"])

// put the guest on your tailnet
sandbox.tailscale_up(box, Some("tskey-auth-…"), Some("web"), [])
```

The Tailscale auth key travels in the environment as `TS_AUTHKEY`, so it never
appears in an argument list.

## Errors

`bsdkrun/error.Error` is a single type with one variant per failure mode:

| Variant           | Meaning                                                     |
| ----------------- | ----------------------------------------------------------- |
| `BinaryNotFound`  | the `bsdkrun` binary wasn't found; carries the paths tried   |
| `CommandFailed`   | a command exited non-zero; carries code, stdout, stderr      |
| `SandboxNotFound` | no machine matched the given id or prefix                    |
| `DecodeFailed`    | `--json` output could not be decoded; carries the raw text   |
| `InvalidOptions`  | the create options were inconsistent, e.g. an empty image    |

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
