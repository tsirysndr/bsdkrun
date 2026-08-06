# bsdkrund — the bsdkrun gRPC daemon

A token-authenticated gRPC server that drives the `bsdkrun` CLI, so a machine
that can actually run VMs — a Linux/KVM box, bare metal, a VPS — can be
controlled from somewhere else.

The daemon owns no VM logic. It resolves the `bsdkrun` binary installed beside
it and runs it as a subprocess, so a daemon always exposes exactly the feature
set of that binary and can never drift from it.

**Using a daemon is opt-in.** The CLI and the desktop app keep running `bsdkrun`
directly on the local machine by default, and always will; pointing them at a
daemon is a choice, not a migration.

## Quick start

On the server:

```console
$ bsdkrund --bind 0.0.0.0:50051 --tls-cert cert.pem --tls-key key.pem
bsdkrun daemon listening on https://0.0.0.0:50051

  access token (generated — shown only once):
    9f2c...64 hex chars...

  connect with:
    export BSDKRUN_HOST=https://<this-host>:50051
    export BSDKRUN_TOKEN=9f2c...
```

Set `BSDKRUN_TOKEN` (or `--token`) to pin a token across restarts instead of
generating a fresh one each boot.

## Security

Every RPC requires a bearer token. There is no anonymous or read-only tier —
the daemon can boot a VM, open a root shell in it, and read any file its host
user can, so a single credential guards all of it.

| Concern | Behaviour |
| ------------------ | ----------------------------------------------------------------------------------- |
| Default bind       | `127.0.0.1:50051` — exposing it to a network is a deliberate act                      |
| Token              | `--token` / `BSDKRUN_TOKEN`, else 32 random bytes from `/dev/urandom`, hex-encoded    |
| Header             | `authorization: Bearer <token>`, or `x-bsdkrun-token: <token>`                        |
| Comparison         | Constant-time, so a wrong token leaks nothing through timing                          |
| TLS                | `--tls-cert` / `--tls-key`; a non-loopback bind without TLS logs a warning            |
| Reflection, health | Served **anonymously** — schema and liveness only, never machine state                |

A bearer token on a plaintext socket is readable by anyone on the path. Over the
public internet, use TLS or an encrypted tunnel (SSH, WireGuard, tailscale).

## The API

Two layers, because neither alone is enough:

- **Typed RPCs** for the surface clients program against — machines, images,
  volumes, networks, flavors, lifecycle, exec, logs. These parse the CLI's
  `--json` output into real protobuf messages.
- **A generic `Run` passthrough** that forwards an argv to the CLI and streams
  the result. It covers the long tail (`probe`, `kernel`, `firmware`, `grow`,
  `store`) and any subcommand added to the CLI after the proto was written, so a
  client is never blocked waiting for the daemon to catch up.

Schema: [`proto/bsdkrun.proto`](proto/bsdkrun.proto). Reflection is enabled, so
`grpcurl` needs no local copy of it:

```console
$ grpcurl -plaintext localhost:50051 list
$ grpcurl -plaintext -H "authorization: Bearer $BSDKRUN_TOKEN" \
    -d '{"all":true}' localhost:50051 bsdkrun.v1.Bsdkrun/ListMachines
```

### Machines are always detached

The daemon outlives any single RPC, so a foreground VM would have nowhere to
live. `RunLinux` / `RunBsd` / `RunFlavor` return a machine id; use `Logs` to
watch it boot and `Exec` to get a shell.

## How a remote interactive shell works

`Exec` is bidirectional, and the daemon allocates a **real pty on its own host**:

```text
your terminal (raw mode)              daemon host (VPS / bare metal)
  │  keystrokes                         ┌────────────────────────┐
  ├─► ExecStart{id, tty, rows, cols} ──►│ pty master             │
  ├─► stdin bytes ─────────────────────►│   ↕                    │
  ├─► Resize{rows, cols}  (SIGWINCH) ──►│ `bsdkrun shell <id>`   │ ← sees a real tty
  │                                     │   ↕ guest agent        │
  ◄── stdout bytes ────────────────────┤ guest shell            │
  ◄── exit_code ───────────────────────└────────────────────────┘
        one HTTP/2 bidi stream, TLS + bearer token
```

Plain pipes cannot do this: the CLI and the guest shell both check `isatty`, so
without a terminal there is no prompt, no line editing, no job control and no
window size to honour. Running the CLI *under* a pty makes it see a genuine
terminal; the daemon then only moves bytes between the pty master and the
stream. `TCP_NODELAY` is set on both ends so a keystroke is not delayed by
Nagle's algorithm.

Two rules make the lifetime work, and both were bugs before they were rules:

- The client half-closing its request stream means "no more stdin", **not**
  "cancel". A non-interactive client sends `start` and half-closes immediately;
  treating that as cancellation cut every such call off before its first byte.
- Cleanup is driven by the client dropping the **response** stream. It cannot be
  a background task holding a channel sender, because a live sender keeps the
  channel open and the stream could then never end.

## Clients

`bsdkrun-daemon` is also a library, so the CLI and desktop app can link it and
talk to a daemon. Configuration follows the `DOCKER_HOST` convention:

```bash
export BSDKRUN_HOST=https://vps.example.com:50051
export BSDKRUN_TOKEN=<token>
```

`RemoteConfig::from_env()` returns `Ok(None)` when `BSDKRUN_HOST` is unset — the
signal to run locally. A host set *without* a token is an error rather than a
silent fallback: quietly running a command on the wrong machine is worse than
refusing.

## Development

```console
$ cargo build --release        # needs protoc on PATH
$ cargo test                   # unit + end-to-end
$ cargo clippy --all-targets -- -D warnings
```

The e2e suite (`tests/e2e.rs`) runs a real server over a real socket against a
**stub** `bsdkrun`. That keeps it hermetic — no hypervisor, no VM boots, no
downloads — while letting each test assert the exact argv the service produced,
which is the part most likely to break: the whole daemon is a translation layer
from proto messages to command lines.

CI: [`e2e-daemon.yml`](../.github/workflows/e2e-daemon.yml) runs the suite on
macOS and Linux plus a `grpcurl` pass over the wire;
[`release-daemon.yml`](../.github/workflows/release-daemon.yml) builds
macOS arm64 and static musl Linux binaries for amd64 and arm64.
