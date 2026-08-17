# bsdkrun-sdk (Rust SDK)

A Rust SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on
macOS and Linux, built on [libkrun](https://github.com/containers/libkrun).
Boot and drive microVMs programmatically, inspired by the **Vercel** and
**Deno** Sandbox SDKs.

It's a thin, blocking wrapper that shells out to the `bsdkrun` binary — no
async runtime, no tokio. The API is fluent: consuming builders you chain and
finish with one terminal call.

```rust
use bsdkrun_sdk::Sandbox;

let sandbox = Sandbox::linux("alpine").create()?;

// exec argv directly, or build a command with env / stdin / a PTY / a cwd:
println!("{}", sandbox.exec(["uname", "-a"])?.text());
sandbox.exec(["apk", "add", "curl"])?;
sandbox
    .command("curl")
    .args(["-fsSL", "https://example.com"])
    .run()?;

sandbox.stop()?;
```

## Install

```sh
cargo add bsdkrun-sdk
```

Or from this repo:

```toml
[dependencies]
bsdkrun-sdk = { path = "../bsdkrun/sdk/rust" }
```

### The `bsdkrun` binary

You need the `bsdkrun` binary itself. The SDK finds it via, in order:

1. `bsdkrun_sdk::set_binary_path("/path/to/bsdkrun")`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build

See the [bsdkrun README](../../README.md) for installing the binary (Homebrew
on macOS, or build from source on Linux/KVM). This SDK assumes libkrun is
already provisioned — it does not auto-install it.

## Creating a sandbox

One builder per guest kind — each exposes only the options its kind takes, and
every `create()` runs the machine **detached** and returns a `Sandbox` handle:

```rust
use bsdkrun_sdk::Sandbox;

// Linux OCI image (docker run-style)
Sandbox::linux("ghcr.io/owner/name:tag")
    .cpus(2)
    .mem(1024)
    .volume("web")                  // persistent CoW rootfs
    .mount("~/project:/src")
    .mount("~/data:/data:ro")
    .port("8080:80")
    .forward(2222, 22)              // same thing, from numbers
    .command(["node", "server.js"]) // args after `--`
    .create()?;

// FreeBSD (EFI on macOS, PVH on Linux/amd64)
Sandbox::freebsd().version("14.3").mem(2048).create()?;

// NetBSD (direct-kernel boot everywhere)
Sandbox::netbsd().version("10.1").volume("db").create()?;

// Boot a raw disk through its UEFI loader
Sandbox::firmware("KRUN_EFI.fd", "disk.raw").create()?;

// Boot a kernel directly, no bootloader
Sandbox::kernel("netbsd").format("elf").disk("root.raw").create()?;

// Unikernels: Unikraft, Solo5 (MirageOS), Nanos, OSv
Sandbox::unikraft(".").cmdline("helloworld").create()?;
Sandbox::solo5("dist/hello.hvt").args(["--ipv4=10.0.0.2/24"]).create()?;
Sandbox::nanos("hello").mem(512).create()?;
Sandbox::osv("loader.img").cmdline("/hello.so").create()?;
```

Every builder also has `.to_args()`, returning the exact argv `create()` would
run — handy for debugging and what the unit tests assert on.

### Environment variables

`.env()` sets the guest environment for the machine's entrypoint. It is merged
over the image's own config, so a key the image already defines is replaced
rather than duplicated.

```rust
let sbx = Sandbox::linux("node:22")
    .env("NODE_ENV", "production")
    .envs([("PORT", "3000")])
    .command(["node", "server.js"])
    .create()?;
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. For a single command
rather than the whole machine, the command builder takes its own env.

## Running commands

Pass an argv (no shell parsing), or build a command fluently:

```rust
sandbox.exec(["ls", "-la", "/etc"])?;

let result = sandbox
    .command("node")
    .args(["-e", "console.log(1)"])
    .env("X", "hi")
    .cwd("/app")
    .stdin("data on stdin")
    .stdout(std::io::stdout()) // stream live and keep capturing
    .stderr(std::io::stderr())
    .tty(true) // allocate a PTY
    .run()?;

println!("{} (exit {})", result.stdout, result.exit_code);
```

`run()` (and the `exec` shorthand) return an `ExecResult` with `stdout`,
`stderr`, `exit_code`, and helpers `.ok()`, `.text()`, `.json::<T>()`,
`.lines()`. A non-zero exit is **data, not an error** — chain `.ok_or_err()`
to turn it into `Error::CommandFailed`:

The `stdout` and `stderr` writers receive bytes as they arrive while the full
streams remain in `ExecResult`. They are independent of `tty`; a PTY changes
command semantics and may merge stderr into stdout.

```rust
sandbox.exec(["ping", "-c1", "db"])?.ok_or_err()?;

#[derive(serde::Deserialize)]
struct Info { hostname: String }
let info: Info = sandbox.exec(["cat", "/etc/info.json"])?.json()?;
```

## Caching

`Sandbox::cache()` saves a guest directory under a key and restores it later, so
a rebuild can pick up where the last one left off. **A miss is not an error** —
check `restored` rather than the `Result`.

```rust
use bsdkrun_sdk::cache::{self, Compression};

let key = format!("deps-{lock_hash}");
let hit = sbx.cache().restore(&key, None, &["deps-".to_string()])?;
if !hit.restored {
    sbx.exec(["npm", "ci"])?;
    sbx.cache().save("/app/node_modules", &key, Compression::Zstd, false)?;
}

cache::list()?;                       // every stored entry, newest first
cache::remove(&[key.clone()], false)?; // or (&[], true) for all
```

The restore keys are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `hit.key` says which one was used.
Formats are `Gzip` (default), `Zstd`, `Estargz` and `None`.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

`Sandbox::fs()` reads and writes files in the guest. Parent directories are
created for you, and everything is byte-exact.

```rust
let fs = sandbox.fs();
fs.write_file("/app/main.py", b"print('hi')")?;

let text  = fs.read_to_string("/app/out.json")?;
let bytes = fs.read_file("/app/logo.png")?;

fs.upload("./src", "/app/src")?;             // file or directory
fs.download("/app/dist", "./dist", true)?;   // true = recursive
```

`upload` looks at the local path to decide whether to recurse; `download` cannot
(the path is in the guest), so say so for a directory. A directory's *contents*
land in the destination: uploading `./src` to `/app/src` leaves the guest's
`/app/src` holding what `./src` holds.

Failures are `Error::FileTransfer { path, message }`.

> Transfers ride the same in-guest agent as `exec`, so the sandbox must be
> running. A directory copy also needs `tar` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle & inventory

```rust
let sandbox = Sandbox::linux("alpine").command(["sleep", "300"]).create()?;
let same = Sandbox::get(sandbox.id())?;      // reconnect (prefix ok) — SandboxNotFound otherwise
let rows = Sandbox::list(true)?;             // Vec<SandboxInfo>, exited included

sandbox.status()?;                           // Option<SandboxInfo>
sandbox.is_running()?;                       // bool
sandbox.logs()?;                             // console log (String); boot_logs() for bsdkrun's own
sandbox.shell()?;                            // interactive shell (inherits the terminal)
sandbox.stop()?;                             // BSD guests clean-poweroff; Linux SIGTERM
sandbox.start()?;                            // restart in place — same id, disk/rootfs, network
sandbox.update().cpus(4).mem(2048).apply()?; // applies on next start
sandbox.remove(true)?;                       // force: stop first if running
```

Host-level namespaces:

```rust
use bsdkrun_sdk::{images, networks, system, volumes};

system::probe()?;                                  // toolchain sanity check
images::list()?;                                   // Vec<ImageInfo>
volumes::list()?;                                  // Vec<VolumeInfo>
volumes::remove(&["web"], true)?;
networks::list()?;                                 // Vec<NetworkInfo>
system::fetch_image("freebsd").version("14.3").run()?;
system::versions("netbsd")?;
```

## Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one subnet
and reach each other **by IP and by name** (docker-compose style), with
internal DNS:

```rust
use bsdkrun_sdk::{networks, Sandbox};

networks::create("devnet")?;

let db = Sandbox::linux("postgres").name("db").network("devnet").create()?;
let api = Sandbox::linux("myapi").name("api").network("devnet").create()?;

// `api` resolves `db` to its IP on devnet and pings it by name:
api.exec(["ping", "-c1", "db"])?.ok_or_err()?;

// inspect + manage
networks::list()?;                       // Vec<NetworkInfo>
networks::members("devnet")?;            // Vec<SandboxInfo> on the network
let info = db.status()?.unwrap();        // info.network == Some("devnet"), info.net_ip set

// edit membership (applies on next start — a VM's NIC is fixed at boot)
api.connect_network("devnet")?;          // or networks::connect(api.id(), "devnet")
api.disconnect_network()?;
api.start()?;                            // re-joins with the new membership

networks::sync("devnet")?;               // refresh members' /etc/hosts (fixes NetBSD name lookup)
networks::remove(&["devnet"], true)?;
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD** resolves
via a synced `/etc/hosts` block — joins auto-sync, and `networks::sync`
refreshes an existing network without restarting members.

## SSH & Tailscale

```rust
// agent-managed key-based SSH
sandbox.ssh_setup().run()?; // install local ~/.ssh/*.pub keys
sandbox.ssh_setup().user("tsiry").key("~/.ssh/work.pub").run()?;

// put a guest on your tailnet — the authkey rides in TS_AUTHKEY, never on the argv
sandbox.tailscale_up().authkey("tskey-auth-...").hostname("web").run()?;
```

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `Client` is the network
sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token.

```rust
use bsdkrun_sdk::Client;

let client = Client::new("http://vps.example.com:50052", "9f2c...")?;
// or, from BSDKRUN_URL / BSDKRUN_TOKEN:
let client = Client::from_env()?;

let machines = client.list(true)?; // Vec<SandboxInfo> — same type Sandbox::list returns

let machine_id = client
    .run_linux()
    .image("alpine")
    .cpus(2)
    .mem(1024)
    .command(["sleep", "300"])
    .launch()?;

let result = client.exec(&machine_id, ["uname", "-a"])?;
println!("{} (exit {})", result.text(), result.exit_code);

client.stop(&machine_id)?;
client.remove(&[&machine_id], false)?;
```

`run_linux()` / `run_bsd(BsdOs::Freebsd)` / `run_nanos()` / `run_unikraft()` /
`run_solo5()` / `run_osv()` / `run_flavor("name")` each build the
corresponding GraphQL mutation's input (`daemon/src/graphql.rs`) and
`launch()` returns the new machine's id. `run_solo5` boots a MirageOS
unikernel under the `solo5-hvt` tender rather than libkrun:

```rust
client
    .run_solo5()
    .path("dist/hello.hvt")
    .args(["--ipv4=10.0.0.2/24"])
    .launch()?;
```

`stop`/`start`/`remove`/`update`/`commit` return a
`CommandResult { exit_code, stdout, stderr }` — a non-zero exit there is a
state to report, not an error.

### Snapshots

A snapshot is a **copy-on-write clone of a machine's disk state** — instant to
take, free until the two sides diverge. `branch` boots a new machine from one
(or from a machine, which is snapshotted first); `restore`/`rollback` put one
back, leaving the machine stopped. A BSD guest is powered off to snapshot it:
a mounted UFS cannot be cloned consistently.

```rust
let snap = client.snapshot(&id, Some("before-upgrade"), "")?;
let all = client.snapshots(Some(&id))?; // newest first
let branch = client.branch(&snap.name).name("web-test").launch()?;
client.restore(&id, &snap.name, true, true)?; // or client.rollback(&id, true, true)
client.remove_snapshots(&[snap.name])?;
```

For a live terminal instead of a one-shot `exec`, use `shell()`:

```rust
let mut session = client.shell(&machine_id).rows(50).cols(120).open()?;
session.on_output(|bytes| print!("{}", String::from_utf8_lossy(bytes)));
session.on_exit(|code| println!("\nexited {code}"));
session.write("ls -la\n")?;
session.resize(50, 120)?;
session.close();
```

Output that arrives before `on_output` is registered is buffered and flushed
the moment the callback is set, so no frame is ever lost to the race between
opening the session and wiring it up.

`follow_logs` streams a machine's console live instead of the one-shot
`logs(id, boot)`:

```rust
let sub = client
    .follow_logs(&machine_id)
    .on_data(|bytes| print!("{}", String::from_utf8_lossy(&bytes)))
    .on_complete(|| println!("-- machine stopped --"))
    .start()?;
// ... later:
sub.unsubscribe();
```

Both `exec`/`shell` and `follow_logs` are built on the same
`openShell`/`shellOutput` shell-session protocol the daemon uses for every
interactive terminal — see
[`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed method yet (flavor/network/volume
management, for instance) — `client.request(query, variables)` runs any raw
query or mutation, and `client.subscribe(query, variables, on_next)` runs any
raw subscription, for anything not wrapped above.

The transport is deliberately small and fully synchronous: queries and
mutations are one blocking `ureq` POST each, and subscriptions run over a
single shared `graphql-transport-ws` WebSocket (`tungstenite`) with one
background reader thread — no async runtime anywhere.

`Client::new(url, token)` and `from_env()` both reject a URL configured
without a token rather than silently making an unauthenticated request — set
both `BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

Everything returns `bsdkrun_sdk::Result<T>` with one `Error` enum:

- `BinaryNotFound` — the `bsdkrun` binary wasn't found (carries every
  location searched).
- `CommandFailed` — a command exited non-zero (carries `exit_code`, `stdout`,
  `stderr`). Produced by `ok_or_err()`, the lifecycle methods, and the agent
  helpers.
- `SandboxNotFound` — `Sandbox::get` matched no machine.
- `GraphQL` — a `Client` request failed (carries `code`, the daemon's
  `extensions.code`, when there is one).
- `Auth` — the daemon rejected the bearer token; `err.code()` always answers
  `UNAUTHENTICATED`.
- `InvalidInput`, `Io`, `Json` — a refused option combination, a host-side
  process failure, unparseable JSON output.

## Development

From `sdk/rust`:

```sh
cargo test                     # hermetic: fake daemon in-process, stub CLI script
cargo clippy --all-targets     # warning-free
cargo fmt
BSDKRUN_SDK_E2E=1 cargo test --test e2e   # opt-in: against a real bsdkrun binary
```

The tests never require the real `bsdkrun` binary or a live daemon: the client
suites run against an in-process fake GraphQL server (HTTP + WebSocket,
including the `graphql-transport-ws` `connection_init`/`connection_ack`
handshake and a scripted shell session), and the local `Sandbox` suites run
against a stub shell script that records the exact argv produced.

## License

MIT
