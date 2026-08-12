# bsdkrun (Go SDK)

A Go SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive
microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

It's a thin wrapper that shells out to the `bsdkrun` binary, so it has **zero
third-party dependencies** — standard library only, Go 1.25+. The API is
fluent: builders chain and end in a terminal call returning `(T, error)`.

```go
import bsdkrun "github.com/tsirysndr/bsdkrun/sdk/go"

box, err := bsdkrun.Linux("alpine").Create()
if err != nil {
	log.Fatal(err)
}

// exec argv directly, or chain env / stdin / a PTY / a working dir:
res, _ := box.Exec("uname", "-a")
fmt.Println(res.Text())

box.Exec("apk", "add", "curl")
box.Command("curl").Args("-fsSL", "https://example.com").Run()

box.Stop()
```

## Install

```sh
go get github.com/tsirysndr/bsdkrun/sdk/go
```

### The `bsdkrun` binary

You need the `bsdkrun` binary itself. The SDK finds it via, in order:

1. `bsdkrun.SetBinaryPath("/path/to/bsdkrun")`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build

See the [bsdkrun README](../../README.md) for installing the binary (Homebrew on
macOS, or build from source on Linux/KVM). This SDK assumes libkrun is already
provisioned — it does not auto-install it.

## Creating a sandbox

Each guest kind has its own constructor; the chainable options change per kind:

```go
// Linux OCI image (docker run-style)
bsdkrun.Linux("ghcr.io/owner/name:tag").
	Cpus(2).Mem(1024).
	Volume("web").                 // persistent CoW rootfs
	Mount("~/project:/src").
	Mount("~/data:/data:ro").
	Port("8080:80").Port("2222:22").
	Command("node", "server.js").  // args after `--`
	Create()

// FreeBSD (EFI on macOS, PVH on Linux/amd64)
bsdkrun.FreeBSD().Version("14.3").Mem(2048).Create()

// NetBSD (direct-kernel boot everywhere)
bsdkrun.NetBSD().Version("10.1").Volume("db").Create()

// Boot a raw disk through its UEFI loader
bsdkrun.Firmware("KRUN_EFI.fd", "disk.raw").Create()

// Boot a kernel directly, no bootloader
bsdkrun.Kernel("netbsd").Format("elf").Disk("root.raw").Create()

// Unikernels
bsdkrun.Unikraft(".").Cmdline("hello").Create()
bsdkrun.Solo5("dist/hello.hvt").GuestArgs("--ipv4=10.0.0.2/24").Create()
bsdkrun.Nanos("hello").Create()
bsdkrun.OSv("loader.img").Cmdline("/hello.so").Create()
```

Every `Create` runs the machine **detached** and returns a `*Sandbox` handle
(with `ID`, and `SSHPort` when the boot banner reported one).

## Running commands

Pass an argv directly to `Exec`, or chain options on `Command`:

```go
box.Exec("ls", "-la", "/etc")

res, err := box.Command("node").
	Args("-e", "print(1)").
	Env("X", "hi").
	Cwd("/app").
	Stdin("data on stdin").
	TTY().     // allocate a PTY
	Check().   // return *CommandFailedError on a non-zero exit
	Run()

fmt.Println(res.Stdout, res.ExitCode)
```

`Run` returns a `*Result` with `Stdout`, `Stderr`, `ExitCode`, and helpers
`Ok()`, `Text()`, `JSON(&v)`, `Lines()`, `Err()` — `Err()` returns a
`*CommandFailedError` when the exit was non-zero, nil otherwise (the Go
rendering of Python's `throw_if_failed`).

## Lifecycle & inventory

```go
box, _ := bsdkrun.Linux("alpine").Command("sleep", "300").Create()
same, _ := bsdkrun.GetSandbox(box.ID)   // reconnect (prefix ok)
rows, _ := bsdkrun.ListSandboxes(true)  // []SandboxInfo, incl. exited

box.Status()     // *SandboxInfo (nil if gone)
box.IsRunning()  // bool
box.Logs()       // console log; box.BootLogs() for the boot log
box.Shell()      // interactive shell (inherits the terminal)
box.Stop()       // BSD guests clean-poweroff; Linux SIGTERM
box.Start()      // restart in place — resumes its own disk/rootfs (data persists)
box.Update().Cpus(4).Mem(2048).Apply()  // applies on next start
box.Remove(true) // force: stop first if running
```

Host-level namespaces:

```go
bsdkrun.System.Probe()   // toolchain sanity check
bsdkrun.Images.List()    // []ImageInfo
bsdkrun.Volumes.List()   // []VolumeInfo
bsdkrun.Volumes.ForceRemove("web")
bsdkrun.Networks.List()  // []NetworkInfo
bsdkrun.System.FetchImage("freebsd").Version("14.3").Run()
bsdkrun.System.Versions("netbsd")
```

## Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one subnet
and reach each other **by IP and by name** (docker-compose style), with internal
DNS:

```go
bsdkrun.Networks.Create("devnet")

db, _ := bsdkrun.Linux("postgres").Name("db").Network("devnet").Create()
api, _ := bsdkrun.Linux("myapi").Name("api").Network("devnet").Create()

// `api` resolves `db` to its IP on devnet and pings it by name:
res, _ := api.Command("ping").Args("-c1", "db").Check().Run()

// inspect + manage
bsdkrun.Networks.List()            // []NetworkInfo
bsdkrun.Networks.Members("devnet") // []SandboxInfo on the network
info, _ := db.Status()             // info.Network == "devnet", info.NetIP set

// edit membership (applies on next start — a VM's NIC is fixed at boot)
api.ConnectNetwork("devnet") // or bsdkrun.Networks.Connect(api.ID, "devnet")
api.DisconnectNetwork()
api.Start() // re-joins with the new membership

bsdkrun.Networks.Sync("devnet") // refresh members' /etc/hosts (fixes NetBSD name lookup)
bsdkrun.Networks.ForceRemove("devnet")
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD** resolves
via a synced `/etc/hosts` block — joins auto-sync, and `Networks.Sync`
refreshes an existing network without restarting members.

## SSH & Tailscale

```go
// agent-managed key-based SSH
box.SSHSetup().Run() // install local ~/.ssh/*.pub keys
box.SSHSetup().User("tsiry").Key("~/.ssh/work.pub").Run()

// put a guest on your tailnet
box.TailscaleUp().AuthKey("tskey-auth-...").Hostname("web").Run()
```

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `Client` is the network
sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token.

```go
client, err := bsdkrun.NewClient("http://vps.example.com:50052", "9f2c...")
// or, from BSDKRUN_URL / BSDKRUN_TOKEN:
client, err = bsdkrun.ClientFromEnv()

machines, _ := client.List(true) // []SandboxInfo — same type ListSandboxes returns

id, _ := client.RunLinux().
	Image("alpine").
	Cpus(2).Mem(1024).
	Command("sleep", "300").
	Launch()

result, _ := client.Exec(id, []string{"uname", "-a"})
fmt.Println(result.Text(), result.ExitCode)

client.Stop(id)
client.Remove([]string{id}, false)
```

`RunLinux`/`RunBSD`/`RunNanos`/`RunUnikraft`/`RunSolo5`/`RunOSv`/`RunFlavor`
each build the matching GraphQL mutation's input (`daemon/src/graphql.rs`) —
`client.RunBSD().OS("freebsd")...`, etc. — and `Launch` returns the new
machine's id. `RunSolo5` boots a MirageOS unikernel under the `solo5-hvt`
tender rather than libkrun:
`client.RunSolo5().Path("dist/hello.hvt").Args("--ipv4=10.0.0.2/24").Launch()`.
`Stop`/`Start`/`Remove`/`Update`/`Commit` return a
`*CommandResult{ExitCode, Stdout, Stderr}`.

For a live terminal instead of a one-shot `Exec`, use `Shell`:

```go
session, _ := client.Shell(id, nil) // or &bsdkrun.ShellOpts{Command: [...]} for a non-login command
session.OnOutput(func(data []byte) { os.Stdout.Write(data) })
session.OnExit(func(code int) { fmt.Printf("\nexited %d\n", code) })
session.WriteString("ls -la\n")
session.Resize(50, 120)
session.Close()
```

`FollowLogs(id, onData, opts)` streams a machine's console live instead of the
one-shot `Logs(id, boot)`. Both `Exec`/`Shell` and `FollowLogs` are built on
the same `openShell`/`shellOutput` shell-session protocol the daemon uses for
every interactive terminal — see [`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed method yet (flavor/network/volume
management, for instance) — `client.Request(query, variables)` runs any raw
query or mutation, and `client.Subscribe(query, variables, handlers)` runs any
raw subscription, for anything not wrapped above.

Like the local SDK, `Client` has **zero third-party dependencies** — the HTTP
transport is `net/http`, and subscriptions (used by `Exec`/`Shell`/
`FollowLogs`) run over a hand-rolled `graphql-transport-ws` WebSocket client
on top of `net`/`crypto/tls`, since Go's standard library has no WebSocket
client of its own.

`NewClient(url, token)` and `ClientFromEnv()` both reject a URL configured
without a token rather than silently making an unauthenticated request — set
both `BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

Every failure mode has a typed error, matched with `errors.As`:

- `*BinaryNotFoundError` — the `bsdkrun` binary wasn't found (carries the
  searched paths).
- `*CommandFailedError` — a command exited non-zero (carries `ExitCode`,
  `Stdout`, `Stderr`). Returned by `Result.Err()`, by `Check()`-ed execs, by
  the lifecycle methods, and by the agent helpers.
- `*SandboxNotFoundError` — `GetSandbox` matched no machine.
- `*GraphQLError` — a `Client` request failed (carries `Code`, the daemon's
  `extensions.code`, when there is one).
- `*AuthError` — the daemon rejected the bearer token. It unwraps to a
  `*GraphQLError` with code `UNAUTHENTICATED`, so a single
  `errors.As(err, &gqlErr)` handles both.

## Development

From `sdk/go`:

```sh
go test ./...   # unit tests against in-process fake servers — no binary, no VM
go vet ./...
gofmt -l .
```

The end-to-end test (boots a real VM) is gated behind `BSDKRUN_SDK_E2E=1`.

## License

MIT
