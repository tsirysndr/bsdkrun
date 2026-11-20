# bsdkrun — PureScript SDK

A Node.js SDK with immutable option builders, `Aff` operations, and functions
that take their configuration or sandbox last for chaining with Prelude's `#`.
It uses [`@bsdkrun/sdk`](../typescript) as its JavaScript runtime, sharing binary
resolution, subprocess handling, file transfers, and the daemon protocol.

## Install

This new PureScript package is available from the repository; it has not been
published to the PureScript registry. Add a checkout to your application's
`spago.yaml`:

```yaml
package:
  name: my-app
  dependencies:
    - bsdkrun
    - prelude
    - effect
    - aff
    - console
workspace:
  packageSet:
    registry: 77.0.0
  extraPackages:
    bsdkrun:
      path: ../bsdkrun/sdk/purescript
```

Install the JavaScript runtime in **your application** as well:

```sh
npm install @bsdkrun/sdk@^0.8.0
spago build
```

Use Node.js 22 or newer (including its built-in WebSocket for remote exec).
Local operations require the `bsdkrun` CLI and its native dependencies; see
[installation](../../README.md#install). The runtime's automatic libkrun setup
can be disabled with `BSDKRUN_NO_AUTO_INSTALL=1` when you manage it yourself.

## Quick start

```purescript
module Main where

import Prelude
import Bsdkrun as B
import Bsdkrun.Exec as Exec
import Bsdkrun.Options as Options
import Effect (Effect)
import Effect.Aff (bracket, launchAff_)
import Effect.Class (liftEffect)
import Effect.Console (log)

main :: Effect Unit
main = launchAff_ do
  let options = Options.linux "alpine"
        # Options.withName "web"
        # Options.withCpus 2
        # Options.withMem 1024
        # Options.withPorts ["8080:80"]
        # Options.withCommand ["sleep", "infinity"]
  bracket (options # B.create) (void <<< B.remove true) \box -> do
    result <- box # B.exec ["uname", "-a"]
    liftEffect $ log $ Exec.text result
```

`#` applies the next function to the preceding value. To chain **asynchronous**
operations, use `B.andThen`, a flipped `bind`:

```purescript
Options.linux "alpine"
  # Options.withVolume "web"
  # B.create
  # B.andThen B.stop
  # B.andThen (B.connectNetwork "devnet")
  # B.andThen B.start
  # B.andThen (B.exec ["uname", "-a"])
```

Create `devnet` first with `Bsdkrun.Host.createNetwork`. Network and resource
changes apply on the next start. Lifecycle methods return the same `Sandbox`,
including `remove`, so they can participate in these chains. Use `bracket` for
cleanup when an intervening action may fail.

## Guest options

`Bsdkrun.Options` provides `linux image`, `freebsd`, `netbsd`,
`firmware firmwarePath diskPath`, `kernel kernelPath`, `nanos image`,
`unikraft path`, `solo5 path`, and `osv image`.

Builders are immutable: a reused prefix is safe. Options irrelevant to a guest
are ignored by the shared argument builder. `buildCreateArgs` lets you inspect
the exact argv without starting a VM.

```purescript
Options.freebsd
  # Options.withVersion "15.0"
  # Options.withPersist true
  # Options.withVolume "bsddata"

Options.linux "node:22"
  # Options.withMounts ["/host/src:/src:ro"]
  # Options.withEnv (Object.singleton "NODE_ENV" "production")
  # Options.withEntrypoint "node"
  # Options.withCommand ["server.js"]
```

Import `Foreign.Object as Object` for environment maps. Other setters cover
network/MAC/ports, extra disks, kernel/version/format/initramfs, console,
command line, Solo5 blocks/arguments, OSv GIC, and log level. Linux's boolean
`--initramfs` uses `withLinuxInitramfs`; other guests' initramfs paths use
`withInitramfs`.

## Exec and streaming

```purescript
let options = Exec.options
      # Exec.withEnv (Object.singleton "RUST_LOG" "debug")
      # Exec.withCwd "/tmp"
      # Exec.withStdin "one\ntwo\n"
      # Exec.withStdout (log <<< Exec.decodeUtf8)

result <- box # B.execWith ["cat"] options
```

Callbacks take `Bytes -> Effect Unit`, so an equivalent stdout callback is
`\chunk -> log (Exec.decodeUtf8 chunk)`. Use `withStderr` independently.
Output is always buffered too: the completed result contains `stdout`,
`stderr`, and `exitCode`, even when callbacks are enabled. Both pipes are
drained concurrently. Streaming does not enable a TTY; `withTty true` is an
explicit choice that changes process semantics and may merge stderr.

Nonzero guest exits are returned as data. `Exec.isOk` checks the exit code;
`Exec.withThrowOnError true` opts into an `Aff` failure with the captured
result available through `Bsdkrun.Error.commandResult`.

Commands, environment values, and paths remain structured arguments, including
spaces and shell metacharacters. A shell is invoked only when you explicitly
choose one, such as `["sh", "-c", "echo hello"]`.

`Bytes` represents a `Uint8Array`/Node Buffer. `Exec.fromBytes`/`toBytes`
convert arrays of byte values, and `encodeUtf8`/`decodeUtf8` explicitly convert
text. UTF-8 characters may span streaming chunks; use a stateful decoder when
rendering arbitrary chunked text. Cancelling an exec or file-transfer `Aff`
aborts its subprocess. Other operations suppress late callbacks on cancellation
but may continue in the runtime.

## Lifecycle, files, caches, and host operations

| Module | Operations |
| --- | --- |
| `Bsdkrun` | `create`, `get`, `list all`, `sandboxId`, `status`, `isRunning`, `logs boot`, `stop`, `start`, `remove force`, `update cpus mem`, `connectNetwork`, `disconnectNetwork` |
| `Bsdkrun.Sandbox` | Also `sshSetup`, `sshAddKey`, `sshStatus`, `tailscaleUp`, `tailscaleStatus` |
| `Bsdkrun.FileSystem` | `writeText path text`, `writeFile path bytes`, `readText path`, `readFile path`, `upload local remote`, `download remote local recursive` (sandbox last) |
| `Bsdkrun.Cache` | `save path options box`, `restore options box`, `list`, `remove keys all` |
| `Bsdkrun.Host` | `images`, `volumes`, `removeVolumes`, `networks`, `createNetwork`, `removeNetworks`, `syncNetwork`, `networkMembers`, `probe`, `versions`, `fetchImage`, `growDisk` |

`B.status` returns `Maybe SandboxInfo`. Nullable fields in info records use
`Data.Nullable`; convert them with `toMaybe`. `B.list true` includes stopped
machines. `B.setBinaryPath` is an `Effect` override; otherwise the runtime tries
`BSDKRUN_BIN`, PATH, then a development build.

```purescript
box # FS.writeText "/app/main.py" "print('hello')"
bytes <- box # FS.readFile "/app/logo.png"
box # FS.upload "./src" "/app/src"
box # FS.download "/app/dist" "./dist" true

hit <- box # Cache.restore
  { key: "deps-v1", path: null, restoreKeys: ["deps-"] }
unless hit.restored do
  _ <- box # Cache.save "/app/node_modules"
    { key: "deps-v1", compression: "zstd", force: false }
  pure unit
```

Import `Bsdkrun.FileSystem as FS`, `Bsdkrun.Cache as Cache`, and
`Data.Nullable (null)` for this example. A cache miss succeeds with
`restored: false`; absent restore metadata is normalized to `Nullable`.
Compression values are `gzip`, `zstd`, `estargz`, and `none`. Files are byte-exact;
upload detects local directories, while download takes an explicit recursive flag.

## Remote daemon

```purescript
import Bsdkrun.Client as Client

client <- Client.fromEnv -- BSDKRUN_URL and BSDKRUN_TOKEN; both required
machines <- client # Client.list true
id <- client # Client.run
  (Options.linux "alpine" # Options.withName "remote")
result <- client # Client.exec id ["uname", "-a"] []
_ <- client # Client.stop id
_ <- client # Client.remove [id] false
```

`Client.connect url token` constructs a client explicitly. `run` accepts the
same immutable options as local creation for Linux, BSD, and unikernel guests;
firmware/kernel guests are unavailable through the daemon. `runLinux` and
`runBsd` also check the guest kind. Remote exec uses the daemon's shell session
protocol and returns `{ exitCode, output :: Bytes }` with combined terminal
output. It does not expose separate stderr or local streaming callbacks.

The typed remote surface includes `get`, `list`, `run`, `exec`, `stop`, `start`,
`remove`, `update`, `commit`, and `logs`. For additional daemon operations,
`Client.request query variables client` accepts an `Object Json` and returns
`Aff Json`. HTTP, authentication, and GraphQL errors remain `Aff` failures.

## CI workflows

```purescript
import Bsdkrun.CI as CI

let plan = CI.workflow "test"
      # CI.onPush ["main"]
      # CI.deps ["nodejs", "pnpm"]
      # CI.env "CI" "true"
      # CI.step "install" "pnpm install --frozen-lockfile"
      # CI.step "test" "pnpm test"

liftEffect $ log $ CI.yaml plan
-- Write only when desired:
-- path <- liftEffect $ plan # CI.save "."
-- plan # CI.run "."
```

The workflow is immutable; rendering and execution reuse the runtime's YAML
format. `save` writes `.tangled/workflows/<name>.yml` and `run` executes the
workflow with streamed inherited stdio.

## Errors

Use `Effect.Aff.attempt` or `catchError`. `Bsdkrun.Error.kind` distinguishes
binary lookup, command, sandbox lookup, file transfer, GraphQL, authentication,
and other errors. `commandResult` recovers captured output from command
failures; `graphqlCode` returns a daemon error code when present. Error messages
remain accessible through `Effect.Exception.message`.

## Development

```sh
cd sdk/purescript
BSDKRUN_NO_AUTO_INSTALL=1 npm ci
npm run build
npm test
purs-tidy check 'src/**/*.purs' 'test/**/*.purs'
```

Tests compile the actual PureScript API, run a fake CLI as a real subprocess,
check concurrent live/buffered output and stdin, preserve literal argv and
binary files, exercise cache misses, and intercept HTTP to verify remote
GraphQL encoding. They need no VM, libkrun installation, or listening socket.

When changing the shared runtime in this checkout, build `sdk/typescript` and
use `npm install --no-save --package-lock=false ../typescript` here to test
against that local version.

MIT license, as in the parent repository.
