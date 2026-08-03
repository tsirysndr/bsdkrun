# @bsdkrun/sdk

A TypeScript SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
Firecracker-style microVM launcher for **BSD and Linux** guests on macOS and
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
await box.stop();
```

Host-level namespaces:

```ts
import { images, volumes, probe, fetchImage, versions } from "@bsdkrun/sdk";

await probe();                              // toolchain sanity check
await images.list();                        // ImageInfo[]
await volumes.list();                       // VolumeInfo[]
await volumes.remove("web", { force: true });
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

## Errors

All errors extend `BsdkrunError`:

- `BinaryNotFoundError` — the `bsdkrun` binary wasn't found.
- `CommandFailedError` — a command exited non-zero (carries `exitCode`,
  `stdout`, `stderr`). Thrown by `sh` (unless `.nothrow()`), by `exec` with
  `throwOnError`, and by the agent helpers.
- `SandboxNotFoundError` — `Sandbox.get` matched no machine.

## Examples

See [`examples/`](./examples) for runnable scripts covering every feature above.

## License

MIT
