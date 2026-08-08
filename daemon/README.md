# bsdkrund — the bsdkrun daemon

A token-authenticated server that drives the `bsdkrun` CLI, so a machine that
can actually run VMs — a Linux/KVM box, bare metal, a VPS — can be controlled
from somewhere else. It speaks two protocols over the same operations:

| API     | Port  | For                                                     |
| ------- | ----- | ------------------------------------------------------- |
| gRPC    | 50051 | The CLI, the desktop app, scripts, anything typed        |
| GraphQL | 50052 | The web frontend — queries, mutations and subscriptions  |

Both are backed by one shared operations layer, so they cannot drift in what
commands they actually run.

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

## The GraphQL API

For the web frontend. Served on `--graphql-bind` (default `127.0.0.1:50052`),
or turned off with `--no-graphql`.

| Endpoint      | What                                                    |
| ------------- | ------------------------------------------------------- |
| `POST /graphql`   | Queries and mutations. `Authorization: Bearer <token>`  |
| `GET /graphql`    | GraphiQL IDE — anonymous, like gRPC reflection          |
| `/graphql/ws`     | Subscriptions over `graphql-transport-ws`               |
| `GET /health`     | Liveness                                                |

```console
$ curl -X POST localhost:50052/graphql \
    -H "authorization: Bearer $BSDKRUN_TOKEN" \
    -H 'content-type: application/json' \
    -d '{"query":"{ machines(all:true){ id name status netIp } }"}'
```

The browser cannot set headers on a WebSocket handshake, so subscriptions carry
the same token in the `connection_init` payload instead:

```js
{ type: 'connection_init', payload: { authorization: `Bearer ${token}` } }
```

CORS is permissive. The API is gated by a bearer token that a browser will not
attach on its own — there is no cookie or session for a hostile page to ride
on — so same-origin policy is not what protects it, and being strict would only
break a dev server on another port.

### Interactive shells over GraphQL

A GraphQL subscription only flows server→client, so there is nowhere to put
keystrokes. A terminal is therefore assembled from three operations over one
socket:

```graphql
mutation { openShell(machineId: "abc", rows: 40, cols: 120) { id } }
subscription { shellOutput(sessionId: "…") { dataBase64 exitCode } }
mutation { sendShellInput(sessionId: "…", dataBase64: "bHMK") }
mutation { resizeShell(sessionId: "…", rows: 50, cols: 100) }
mutation { closeShell(sessionId: "…") }
```

Bytes are base64 because a terminal emits arbitrary binary — escape sequences,
UTF-8 split across chunk boundaries — that a GraphQL `String` would not survive.
Feed `dataBase64` straight into xterm.js and send its `onData` back.

Output is buffered from the moment the session opens. That is a correctness
requirement, not an optimisation: the subscription is necessarily a *separate*
operation from the mutation that opened the shell, so a prompt written in
between would otherwise be lost before anyone was listening.

Two other subscriptions exist: `machineLogs` (a machine's console, `logs -f`)
and `machinesChanged` (a polled machine list, so a dashboard needs no timer of
its own — the CLI has no change feed, so this genuinely polls).

### Machines are always detached

The daemon outlives any single RPC, so a foreground VM would have nowhere to
live. `RunLinux` / `RunBsd` / `RunUnikraft` / `RunNanos` / `RunOsv` / `RunFlavor`
return a machine id;
use `Logs` to watch it boot and `Exec` to get a shell.

`RunUnikraft` boots a [Unikraft](https://unikraft.org) unikernel — the
application linked into the kernel. It takes neither a volume nor a command,
because such a guest has no disk and no in-guest agent; `Exec` and `Commit`
are rejected for the machines it creates, and `Logs` is how you read their
output.

`RunNanos` and `RunOsv` boot the other two unikernels. They share the "no
agent, so no `Exec` and no `Commit`" contract, but both have a root
filesystem, so unlike `RunUnikraft` they do take the disk options — `RunOsv`
in particular takes a `disk` of its own, which is how an x86_64 guest is
given a filesystem (its loader ELF is kernel only).

## How a remote interactive shell works

`Exec` is bidirectional, and the daemon allocates a **real pty on its own host**:

```text
your terminal (raw mode)              daemon host (VPS / bare metal)
  │  keystrokes                         ┌────────────────────────┐
  ├─► ExecStart{id, tty, rows, cols} ──►│ pty master             │
  ├─► stdin bytes ─────────────────────►│   ↕                    │
  ├─► Resize{rows, cols}  (SIGWINCH) ──►│ `bsdkrun shell <id>`   │ ← sees a real tty
  │                                     │   ↕ guest agent        │
  ◄── stdout bytes ─────────────────────┤ guest shell            │
  ◄── exit_code ────────────────────────└────────────────────────┘
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

The suites (`tests/e2e.rs` for gRPC, `tests/graphql.rs` for GraphQL) run against
a **stub** `bsdkrun`. That keeps them hermetic — no hypervisor, no VM boots, no
downloads — while letting each test assert the exact argv produced, which is the
part most likely to break: the whole daemon is a translation layer from wire
messages to command lines.

CI: [`e2e-daemon.yml`](../.github/workflows/e2e-daemon.yml) runs both suites on
macOS and Linux, plus an over-the-wire job — `grpcurl` for gRPC, `curl` for
GraphQL, and a real WebSocket client driving an interactive shell through a
subscription;
[`release-daemon.yml`](../.github/workflows/release-daemon.yml) builds
macOS arm64 and static musl Linux binaries for amd64 and arm64.
