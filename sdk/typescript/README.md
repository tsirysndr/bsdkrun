# @bsdkrun/sdk

A TypeScript SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive
microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

Runs on **Node.js**, **Deno**, and **Bun** — the SDK shells out to the `bsdkrun`
binary, so it has zero npm dependencies.

```ts
import { Sandbox } from "@bsdkrun/sdk";

const sbx = await Sandbox.create({ os: "linux", image: "alpine" });

// Tagged-template shell — interpolations are shell-quoted for you:
const kernel = await sbx.sh`uname -a`.text();

// ...or exec argv directly, with env / stdin / a PTY / a working dir:
await sbx.exec(["apk", "add", "curl"]);
await sbx.runCommand("curl", ["-fsSL", "https://example.com"]);

await sbx.stop();
```

## Install

```sh
bun  add @bsdkrun/sdk
npm  install @bsdkrun/sdk
deno add npm:@bsdkrun/sdk
```

### libkrun is provisioned on install

bsdkrun links **libkrun** (Apple's Hypervisor.framework on macOS, KVM on Linux).
A `postinstall` step provisions it for you when you install this package:

- **macOS** — `brew install libkrun` (if not already installed).
- **Linux** — downloads our [PVH-enabled libkrun fork](https://github.com/tsirysndr/libkrun/releases)
  (needed for BSD PVH boots; not packaged elsewhere) into the bsdkrun cache and,
  at runtime, points the loader at it via `LD_LIBRARY_PATH`. You still need
  `libkrunfw` (the bundled-kernel lib it links) from your distro / nixpkgs, and
  `/dev/kvm` access.

If `postinstall` was skipped (`--ignore-scripts`, or Deno, which doesn't run
install hooks), the SDK provisions libkrun the same way on its first call.

Provisioning knobs:

| Env var                      | Effect                                             |
| ---------------------------- | -------------------------------------------------- |
| `BSDKRUN_NO_AUTO_INSTALL=1`  | Skip all provisioning — you manage libkrun.        |
| `BSDKRUN_LIBKRUN_DIR=/path`  | Use an already-extracted libkrun lib dir (Linux).  |
| `BSDKRUN_LIBKRUN_VERSION=vX` | Pin a specific PVH-fork release tag.               |

### The `bsdkrun` binary

You also need the `bsdkrun` binary itself. The SDK finds it via, in order:

1. `setBinaryPath("/path/to/bsdkrun")`
2. the `BSDKRUN_BIN` environment variable
3. `bsdkrun` on your `PATH`
4. an in-repo `target/release/bsdkrun` or `target/debug/bsdkrun` build

See the [bsdkrun README](../../README.md) for installing the binary (Homebrew on
macOS, or build from source on Linux/KVM).

## Creating a sandbox

`Sandbox.create` is a discriminated union on `os` — the options change per
guest kind:

```ts
// Linux OCI image (docker run-style)
await Sandbox.create({
  os: "linux",
  image: "ghcr.io/owner/name:tag",
  cpus: 2,
  mem: 1024,
  volume: "web",                     // persistent CoW rootfs
  mounts: ["~/project:/src", "~/data:/data:ro"],
  net: { ports: ["8080:80", "2222:22"] },
  command: ["node", "server.js"],    // args after `--`
});

// FreeBSD (EFI on macOS, PVH on Linux/amd64)
await Sandbox.create({ os: "freebsd", version: "14.3", mem: 2048 });

// NetBSD (direct-kernel boot everywhere)
await Sandbox.create({ os: "netbsd", version: "10.1", volume: "db" });

// Boot a raw disk through its UEFI loader
await Sandbox.create({ os: "firmware", firmware: "KRUN_EFI.fd", disk: "disk.raw" });

// Boot a kernel directly, no bootloader
await Sandbox.create({ os: "kernel", kernel: "netbsd", format: "elf", disk: "root.raw" });
```

Every `create` runs the machine **detached** and returns a `Sandbox` handle.

### Environment variables

`env` sets the guest environment for the machine's entrypoint. It is merged over
the image's own config, so a key the image already defines is replaced rather
than duplicated.

```ts
const sbx = await Sandbox.create({
  os: "linux",
  image: "node:22",
  env: { NODE_ENV: "production", PORT: "3000" },
  command: ["node", "server.js"],
});
```

Linux guests only — BSD guests boot their own init, so there is no generated
init to export into; set those from `exec` after boot. For a single command
rather than the whole machine, `exec` takes its own `env`.

## Running commands

Two complementary APIs:

### `sh` — tagged template

Best for quick shell one-liners. Interpolated values are **single-quoted**
(injection-safe); wrap a value in `raw()` to splice it verbatim.

```ts
const dir = "/etc";
await sbx.sh`ls -la ${dir}`;                 // quoted
await sbx.sh`grep ${pattern} /var/log/*`;    // quoted

await sbx.sh`cat /nope`.nothrow();           // don't throw on non-zero exit
await sbx.sh`echo $X`.env({ X: "1" }).text();

import { raw } from "@bsdkrun/sdk";
await sbx.sh`ls ${raw("-la /var")}`;         // spliced verbatim (trusted only)
```

An `sh` call is lazy and awaitable; `.text()`, `.json()`, `.lines()` are
convenience accessors. It **throws** `CommandFailedError` on a non-zero exit
unless you call `.nothrow()`.

### `exec` — argv, with options

The primary programmatic entrypoint. No shell parsing; pass an argv array (or a
program name plus `args`). Richer options than `sh`:

```ts
await sbx.exec(["ls", "-la", "/etc"]);

await sbx.exec("node", {
  args: ["-e", "console.log(process.env.X)"],
  env: { X: "hi" },
  cwd: "/app",
  stdin: "data on stdin",
  tty: true,               // allocate a PTY
  throwOnError: true,      // throw on non-zero exit (default: false)
  onStdout: chunk => process.stdout.write(chunk),
  onStderr: chunk => process.stderr.write(chunk),
});

// Vercel-Sandbox-style alias:
const { stdout, exitCode } = await sbx.runCommand("uname", ["-a"]);
```

`exec` returns a `CommandResult` with `.stdout`, `.stderr`, `.exitCode`, `.ok`,
and helpers `.text()`, `.json()`, `.lines()`, `.throwIfFailed()`.

`onStdout` and `onStderr` are optional real-time byte-stream callbacks. Output
is still accumulated in the returned `CommandResult`, so existing buffered
usage is unchanged. Streaming does not require `tty`; a TTY changes process
semantics and commonly merges stderr into stdout.

> `exec`/`shell` talk to a tiny in-guest agent (needs guest networking).
> Linux guests get it injected automatically; on BSD you install it once — see
> the [bsdkrun README](../../README.md#the-exec-agent).

## Caching

`sandbox.cache` saves a guest directory under a key and restores it later, so a
rebuild can pick up where the last one left off. **A miss is not an error** —
check `restored` rather than catching.

```ts
import { caches } from "@bsdkrun/sdk";

const key = `deps-${lockHash}`;
const hit = await sbx.cache.restore({ key, restoreKeys: ["deps-"] });
if (!hit.restored) {
  await sbx.exec(["npm", "ci"]);
  await sbx.cache.save("/app/node_modules", { key, compression: "zstd" });
}

await caches.list();          // every stored entry, newest first
await caches.remove([key]);   // or removeCache([], { all: true })
```

`restoreKeys` are prefixes tried in order when the exact key misses; within a
prefix the newest matching entry wins, and `hit.key` says which one was used.
Formats are `gzip` (default), `zstd`, `estargz` and `none`.

Where entries live is host configuration, not an SDK concern: the default is
this host's disk, and `BSDKRUN_CACHE_BACKEND=s3` + `BSDKRUN_CACHE_S3_*` (or
`~/.config/bsdkrun/cache.toml`) points them at a bucket instead.

## Files

`sandbox.fs` reads and writes files in the guest. Parent directories are created
for you, and everything is byte-exact — `readFile` hands back a `Buffer`, so a
PNG survives the round trip.

```ts
await sbx.fs.writeFile("/app/main.py", "print('hi')");
await sbx.fs.writeFile("/app/logo.png", pngBytes);

const text  = await sbx.fs.readTextFile("/app/out.json");
const bytes = await sbx.fs.readFile("/app/logo.png");

await sbx.fs.upload("./src", "/app/src");                       // file or directory
await sbx.fs.download("/app/dist", "./dist", { recursive: true });
```

`upload` looks at the local path to decide whether to recurse; `download` cannot
(the path is in the guest), so pass `recursive` for a directory. A directory's
*contents* land in the destination: `upload("./src", "/app/src")` leaves the
guest's `/app/src` holding what `./src` holds.

Failures throw `FileTransferError`, which carries the offending `path`.

> Transfers ride the same in-guest agent as `exec`, so the sandbox must be
> running. A directory copy also needs `tar` in the guest; single files need
> only the shell every bootable image already has.

## Lifecycle & inventory

```ts
const sbx  = await Sandbox.create({ os: "linux", image: "alpine", command: ["sleep","300"] });
const same = await Sandbox.get(sbx.id);        // reconnect (prefix ok)
const list = await Sandbox.list({ all: true }); // SandboxInfo[]

await sbx.status();      // SandboxInfo | null
await sbx.isRunning();   // boolean
await sbx.logs();        // console log (string)
sbx.followLogs();        // live stream (child process)
sbx.shell();             // interactive shell (inherits the terminal)
await sbx.stop();        // BSD guests clean-poweroff; Linux SIGTERM
await sbx.start();       // restart in place — resumes its own disk/rootfs (data persists)
await sbx.update({ cpus: 4, mem: 2048 }); // applies on next start
await sbx.remove({ force: true });
```

`stop`/`start` **persist your data**: `start` resumes the machine's own
disk/rootfs (any committed snapshot + runtime changes), like `docker start`.

Host-level namespaces:

```ts
import { images, volumes, networks, probe, fetchImage, versions } from "@bsdkrun/sdk";

await probe();                              // toolchain sanity check
await images.list();                        // ImageInfo[]
await volumes.list();                       // VolumeInfo[]
await volumes.remove("web", { force: true });
await networks.list();                      // NetworkInfo[]
await fetchImage("freebsd", { version: "14.3" });
await versions("netbsd");
```

## Interactive terminal (xterm.js in the browser)

`sbx.terminal()` opens a PTY session in the guest, streamed over the agent's TCP
protocol — with **live window-resize**. It's shaped to drop straight into
[xterm.js](https://xtermjs.org): pipe output in, forward keystrokes out, resize
on demand.

```ts
const term = await sbx.terminal({ command: ["/bin/sh"], cols: 120, rows: 30 });

term.onData((chunk) => xterm.write(chunk));      // guest → xterm
xterm.onData((input) => term.write(input));      // xterm → guest
xterm.onResize(({ cols, rows }) => term.resize(cols, rows));

const code = await term.exited;                  // resolves when the shell exits
```

Server-side, bridge it to a browser over a WebSocket in one call:

```ts
wss.on("connection", async (ws) => {
  const term = await sbx.terminal();
  term.bindWebSocket(ws);   // wires output, input, and {"resize":[c,r]} frames
});
```

See [`examples/08-browser-terminal`](./examples/08-browser-terminal) for a
complete Bun server + xterm.js page.

## Networking, SSH & Tailscale

```ts
// forward ports at create time
await Sandbox.create({ os: "linux", image: "alpine", net: { ports: ["2222:22"] } });

// agent-managed key-based SSH (typed helpers)
await sbx.ssh.setup();                              // install local ~/.ssh/*.pub keys
await sbx.ssh.setup({ user: "tsiry", key: "~/.ssh/work.pub" });
await sbx.ssh.addKey("ssh-ed25519 AAAA...");
await sbx.ssh.status();

// put a guest on your tailnet
await sbx.tailscale.up({ authkey: "tskey-auth-...", hostname: "web" });
await sbx.tailscale.status();

// turn a Linux guest into a systemd system (debian/ubuntu/fedora only —
// not Alpine, not the BSD guests)
await sbx.systemd.setup();
```

### Global networks — reach machines by name

Opt machines into a **shared network** so they get distinct IPs on one subnet and
reach each other **by IP and by name** (docker-compose style), with internal DNS:

```ts
import { Sandbox, networks } from "@bsdkrun/sdk";

await networks.create("devnet");

const db  = await Sandbox.create({ os: "linux", image: "postgres", name: "db",  net: { network: "devnet" } });
const api = await Sandbox.create({ os: "linux", image: "myapi",    name: "api", net: { network: "devnet" } });

await api.sh`ping -c1 db`;              // resolves db → its IP on devnet

// inspect + manage
await networks.list();                 // NetworkInfo[]
await networks.members("devnet");      // SandboxInfo[] on the network
const info = await db.status();        // info.network === "devnet", info.netIp set

// edit membership (applies on next start — a VM's NIC is fixed at boot)
await api.connectNetwork("devnet");    // or networks.connect(api.id, "devnet")
await api.disconnectNetwork();
await api.start();                     // re-joins with the new membership

await networks.sync("devnet");         // refresh members' /etc/hosts (fixes NetBSD name lookup)
await networks.remove("devnet", { force: true });
```

Names resolve on Linux and FreeBSD via the network's DNS; **NetBSD** resolves via
a synced `/etc/hosts` block (its resolver rejects the DNS's AAAA `NXDOMAIN`) —
joins auto-sync, and `networks.sync` refreshes an existing network without
restarting members.

## Connecting to a remote daemon

Everything above talks to a local `bsdkrun` binary. `Client` is the network
sibling: it drives the same operations against a remote
[`bsdkrund`](../../daemon/README.md) over its GraphQL API — no local binary
needed, just a URL and a bearer token.

```ts
import { Client } from "@bsdkrun/sdk";

const client = new Client({ url: "http://vps.example.com:50052", token: "9f2c..." });
// or, from BSDKRUN_URL / BSDKRUN_TOKEN:
const client = Client.fromEnv();

const machines = await client.list(true);   // SandboxInfo[] — same type Sandbox.list() returns
const id = await client.runLinux({ image: "alpine", cpus: 2, mem: 1024, command: ["sleep", "300"] });

const { exitCode, output } = await client.exec(id, ["uname", "-a"]);
console.log(new TextDecoder().decode(output), exitCode);

await client.stop(id);
await client.remove([id]);
```

`client.runLinux`/`runBsd`/`runNanos`/`runUnikraft`/`runSolo5`/`runOsv`/
`runFlavor` each take the same fields as the corresponding GraphQL mutation
(`daemon/src/graphql.rs`) — `runBsd({ os: "FREEBSD", ... })`, etc. — and
return the new machine's id. `runSolo5` boots a MirageOS unikernel under the
`solo5-hvt` tender rather than libkrun:
`runSolo5({ path: "dist/hello.hvt", args: ["--ipv4=10.0.0.2/24"] })`.
`stop`/`start`/`remove`/`update`/`commit` return
a `RemoteCommandResult` (`{exitCode, stdout, stderr}`).

### Snapshots

A snapshot is a **copy-on-write clone of a machine's disk state** — instant to
take, free until the two sides diverge. `branch` boots a new machine from one
(or from a machine, which is snapshotted first); `restore`/`rollback` put one
back, leaving the machine stopped. A BSD guest is powered off to snapshot it:
a mounted UFS cannot be cloned consistently.

```ts
const snap = await client.snapshot(machineId, "before-upgrade");
await client.snapshots(machineId); // newest first
const branchId = await client.branch(snap.name, { name: "web-test" });
await client.restore(machineId, snap.name); // or client.rollback(machineId)
await client.removeSnapshots([snap.name]);
```

### Docker

bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
socket, so the host's own `docker` CLI drives the same engine these calls do.
Starting is idempotent — the VM has a fixed name, so it resumes rather than
creating a second.

```ts
const status = await client.dockerStart({ cpus: 4, mem: 4096 });
console.log(status.socket);
for (const c of await client.dockerContainers()) console.log(c.name, c.state, c.ports);
await client.dockerContainer("restart", "web");
console.log(await client.dockerLogs("web", 50));
```

For a live terminal instead of a one-shot `exec`, use `shell()`:

```ts
const session = await client.shell(id);   // or shell(id, { command: [...] }) for a non-login command
session.onOutput((chunk) => process.stdout.write(chunk));
session.onExit((code) => console.log(`exited ${code}`));
session.write("ls -la\n");
session.resize(50, 120);
session.close();
```

`followLogs(id, {}, { onData, onError, onComplete })` streams a machine's
console live instead of the one-shot `logs(id)`. Both `exec`/`shell` and
`followLogs` are built on the same `openShell`/`shellOutput` shell-session
protocol the daemon uses for every interactive terminal — see
[`daemon/README.md`](../../daemon/README.md#interactive-shells-over-graphql)
for the wire-level story.

Not every GraphQL operation has a typed method yet (flavor/network/volume
management, for instance) — `client.request(query, variables)` runs any raw
query or mutation, and `client.subscribe(query, variables, { onNext, ... })`
runs any raw subscription, for anything not wrapped above.

Like the rest of this SDK, `Client` has **zero npm dependencies** — the HTTP
transport is the platform's global `fetch`, and subscriptions (used by
`exec`/`shell`/`followLogs`) run over the platform's global `WebSocket`
speaking `graphql-transport-ws` directly (no `graphql-ws`/Apollo/urql).

`new Client({...})` and `Client.fromEnv()` both reject a URL configured
without a token rather than silently making an unauthenticated request — set
both `BSDKRUN_URL` and `BSDKRUN_TOKEN`, or pass both explicitly.

## Errors

All errors extend `BsdkrunError`:

- `BinaryNotFoundError` — the `bsdkrun` binary wasn't found.
- `CommandFailedError` — a command exited non-zero (carries `exitCode`,
  `stdout`, `stderr`). Thrown by `sh` (unless `.nothrow()`), by `exec` with
  `throwOnError`, and by the agent helpers.
- `SandboxNotFoundError` — `Sandbox.get` matched no machine.
- `GraphQLError` — a `Client` request failed (carries `code`, the daemon's
  `extensions.code`, when there is one).
- `AuthError` (a `GraphQLError`) — the daemon rejected the bearer token.

## Examples

See [`examples/`](./examples) for runnable scripts covering every feature above.

## License

MIT
