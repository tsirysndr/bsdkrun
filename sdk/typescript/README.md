# @bsdkrun/sdk

A TypeScript SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS and
Linux, built on [libkrun](https://github.com/containers/libkrun). Boot and drive
microVMs programmatically, inspired by the **Vercel** and **Deno** Sandbox SDKs.

Runs on **Node.js**, **Deno**, and **Bun** — the SDK shells out to the `bsdkrun`
binary, so it has zero npm dependencies.

```ts
import { Sandbox } from "@bsdkrun/sdk";

const box = await Sandbox.create({ os: "linux", image: "alpine" });

// Tagged-template shell — interpolations are shell-quoted for you:
const kernel = await box.sh`uname -a`.text();

// ...or exec argv directly, with env / stdin / a PTY / a working dir:
await box.exec(["apk", "add", "curl"]);
await box.runCommand("curl", ["-fsSL", "https://example.com"]);

await box.stop();
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

## Running commands

Two complementary APIs:

### `sh` — tagged template

Best for quick shell one-liners. Interpolated values are **single-quoted**
(injection-safe); wrap a value in `raw()` to splice it verbatim.

```ts
const dir = "/etc";
await box.sh`ls -la ${dir}`;                 // quoted
await box.sh`grep ${pattern} /var/log/*`;    // quoted

await box.sh`cat /nope`.nothrow();           // don't throw on non-zero exit
await box.sh`echo $X`.env({ X: "1" }).text();

import { raw } from "@bsdkrun/sdk";
await box.sh`ls ${raw("-la /var")}`;         // spliced verbatim (trusted only)
```

An `sh` call is lazy and awaitable; `.text()`, `.json()`, `.lines()` are
convenience accessors. It **throws** `CommandFailedError` on a non-zero exit
unless you call `.nothrow()`.

### `exec` — argv, with options

The primary programmatic entrypoint. No shell parsing; pass an argv array (or a
program name plus `args`). Richer options than `sh`:

```ts
await box.exec(["ls", "-la", "/etc"]);

await box.exec("node", {
  args: ["-e", "console.log(process.env.X)"],
  env: { X: "hi" },
  cwd: "/app",
  stdin: "data on stdin",
  tty: true,               // allocate a PTY
  throwOnError: true,      // throw on non-zero exit (default: false)
});

// Vercel-Sandbox-style alias:
const { stdout, exitCode } = await box.runCommand("uname", ["-a"]);
```

`exec` returns a `CommandResult` with `.stdout`, `.stderr`, `.exitCode`, `.ok`,
and helpers `.text()`, `.json()`, `.lines()`, `.throwIfFailed()`.

> `exec`/`shell` talk to a tiny in-guest agent (needs guest networking).
> Linux guests get it injected automatically; on BSD you install it once — see
> the [bsdkrun README](../../README.md#the-exec-agent).

## Lifecycle & inventory

```ts
const box  = await Sandbox.create({ os: "linux", image: "alpine", command: ["sleep","300"] });
const same = await Sandbox.get(box.id);        // reconnect (prefix ok)
const list = await Sandbox.list({ all: true }); // SandboxInfo[]

await box.status();      // SandboxInfo | null
await box.isRunning();   // boolean
await box.logs();        // console log (string)
box.followLogs();        // live stream (child process)
box.shell();             // interactive shell (inherits the terminal)
await box.stop();        // BSD guests clean-poweroff; Linux SIGTERM
await box.start();       // restart in place — resumes its own disk/rootfs (data persists)
await box.update({ cpus: 4, mem: 2048 }); // applies on next start
await box.remove({ force: true });
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

`box.terminal()` opens a PTY session in the guest, streamed over the agent's TCP
protocol — with **live window-resize**. It's shaped to drop straight into
[xterm.js](https://xtermjs.org): pipe output in, forward keystrokes out, resize
on demand.

```ts
const term = await box.terminal({ command: ["/bin/sh"], cols: 120, rows: 30 });

term.onData((chunk) => xterm.write(chunk));      // guest → xterm
xterm.onData((input) => term.write(input));      // xterm → guest
xterm.onResize(({ cols, rows }) => term.resize(cols, rows));

const code = await term.exited;                  // resolves when the shell exits
```

Server-side, bridge it to a browser over a WebSocket in one call:

```ts
wss.on("connection", async (ws) => {
  const term = await box.terminal();
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
await box.ssh.setup();                              // install local ~/.ssh/*.pub keys
await box.ssh.setup({ user: "tsiry", key: "~/.ssh/work.pub" });
await box.ssh.addKey("ssh-ed25519 AAAA...");
await box.ssh.status();

// put a guest on your tailnet
await box.tailscale.up({ authkey: "tskey-auth-...", hostname: "web" });
await box.tailscale.status();

// turn a Linux guest into a systemd system (debian/ubuntu/fedora only —
// not Alpine, not the BSD guests)
await box.systemd.setup();
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
