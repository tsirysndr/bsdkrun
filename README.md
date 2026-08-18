<p align="center">
  <img src="https://mirror-fsn.tangled.network/xrpc/sh.tangled.git.temp.getBlob?path=.github%2Fassets%2Fdesktop.png&ref=main&repo=did%3Aplc%3Anbljdkycqux4kcthe34d45vz" alt="bsdkrun Desktop — machines list with a tabbed terminal panel" width="900">
</p>

# bsdkrun

[![nix](https://github.com/tsirysndr/bsdkrun/actions/workflows/nix.yml/badge.svg)](https://github.com/tsirysndr/bsdkrun/actions/workflows/nix.yml)
[![e2e (Linux / KVM)](https://github.com/tsirysndr/bsdkrun/actions/workflows/e2e-linux.yml/badge.svg)](https://github.com/tsirysndr/bsdkrun/actions/workflows/e2e-linux.yml)
[![FlakeHub](https://img.shields.io/endpoint?url=https://flakehub.com/f/tsirysndr/bsdkrun/badge)](https://flakehub.com/flake/tsirysndr/bsdkrun)
[![skills.sh](https://skills.sh/b/tsirysndr/bsdkrun)](https://skills.sh/tsirysndr/bsdkrun/bsdkrun-cli)
![GitHub Downloads (all assets, all releases)](https://img.shields.io/github/downloads/tsirysndr/bsdkrun/total)


A Firecracker-style **microVM launcher for BSD, Linux, and unikernel guests on macOS and Linux**,
built on [libkrun](https://github.com/containers/libkrun) (which drives Apple's
Hypervisor.framework on macOS and KVM on Linux).

`bsdkrun` is a thin, purpose-built CLI: it wraps libkrun's C ABI in a handful of safe Rust
bindings and boots a guest three ways — from a **UEFI firmware** image (the guest's own EFI loader
boots a normal disk), from a **direct kernel + FDT** (no bootloader), or straight from an **OCI
image** (`bsdkrun linux alpine` pulls it from any registry, extracts the rootfs, and boots it like
`docker run`). It also boots **unikernels** — [Unikraft](#unikraft--boot-a-unikraft-unikernel),
[Nanos](#nanos--boot-a-nanos-nanovms-unikernel), OSv, and
[MirageOS/Solo5](#solo5--mirage--run-a-mirageos-solo5-unikernel) — and `bsdkrun pack` turns an
ordinary project into a bootable Unikraft unikernel. It is deliberately small: one FFI module,
one CLI, no daemon required.

> [!IMPORTANT]
> **Platforms:** **macOS on Apple Silicon** (Hypervisor.framework) and **Linux on amd64 or arm64**
> (KVM). A hardware-virtualized guest runs the host's CPU arch, so bsdkrun detects the arch and
> pulls the matching kernel, OCI image, and agent automatically. macOS is arm64-only; Linux works
> on both x86_64 and aarch64. **FreeBSD** boots via EFI on macOS and via **PVH direct kernel** on
> Linux/amd64; **NetBSD** direct-boots its kernel everywhere. The amd64 PVH boots need our
> [PVH-enabled libkrun fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot). _(Linux
> support is new — see the [KVM e2e CI](.github/workflows/e2e-linux.yml).)_

<p align="center">
  <img src="https://mirror.tangled.network/xrpc/sh.tangled.git.temp.getBlob?path=.github%2Fassets%2Fpreview.png&ref=main&repo=did%3Aplc%3Anbljdkycqux4kcthe34d45vz" alt="FreeBSD 15 arm64 booting under bsdkrun on macOS" width="800">
</p>

---

## Contents

- [Features](#features)
- [Why this exists](#why-this-exists)
- [Install](#install)
- [Prerequisites](#prerequisites)
- [Build](#build)
- [Usage](#usage)
  - [`probe`](#probe--sanity-check-the-toolchain)
  - [`freebsd` / `netbsd`](#freebsd--netbsd--one-liner-bsd-microvms)
  - [`firmware`](#firmware--boot-a-disk-through-its-uefi-loader)
  - [`kernel`](#kernel--boot-a-kernel-directly-no-bootloader)
  - [`linux`](#linux--run-an-oci-image-as-a-microvm)
  - [`unikraft`](#unikraft--boot-a-unikraft-unikernel)
  - [`nanos`](#nanos--boot-a-nanos-nanovms-unikernel)
  - [`solo5` / `mirage`](#solo5--mirage--run-a-mirageos-solo5-unikernel)
- [Managing machines](#managing-machines)
- [AI coding agents in sandboxes](#ai-coding-agents-in-sandboxes)
- [Docker — replace Docker Desktop](#docker--replace-docker-desktop)
- [CI — spindle workflows in microVMs](#ci--spindle-workflows-in-microvms)
- [Snapshots, branches & restore](#snapshots-branches--restore)
- [Flavors — preconfigured environments & snapshots](#flavors--preconfigured-environments--snapshots)
- [Networking](#networking)
- [Machine domains — local DNS + HTTPS](#machine-domains--local-dns--https)
- [TUI — the terminal dashboard](#tui--the-terminal-dashboard)
- [Disks](#disks)
- [Console](#console-how-output-reaches-your-terminal)
- [Preparing a guest image](#preparing-a-guest-image)
- [SDKs](#sdks)
- [Project layout](#project-layout)
- [Troubleshooting](#troubleshooting)
- [Status](#status)
- [License](#license)

---

## Features

**Guests & boot paths**

- **One-liner BSD microVMs** — [`bsdkrun freebsd` / `bsdkrun netbsd`](#freebsd--netbsd--one-liner-bsd-microvms)
  fetch a bundled image (guest agent baked in) and boot it: run a command à la `docker run`
  (`bsdkrun freebsd -- uname -a`), or get dropped straight into an interactive shell.
- **Any OCI image as a Linux microVM** — [`bsdkrun linux alpine`](#linux--run-an-oci-image-as-a-microvm)
  pulls from any registry (no Docker daemon), extracts the rootfs, and boots it `docker run`-style;
  [`bsdkrun systemd`](#systemd--turn-an-oci-guest-into-a-full-systemd-system) flips a
  debian/ubuntu/fedora guest into a full systemd system.
- **Unikernels** — [Unikraft](#unikraft--boot-a-unikraft-unikernel),
  [Nanos](#nanos--boot-a-nanos-nanovms-unikernel), OSv, and
  [MirageOS/Solo5](#solo5--mirage--run-a-mirageos-solo5-unikernel), each with its own subcommand;
  `bsdkrun pack` turns an ordinary project into a bootable Unikraft unikernel.
- **Three boot protocols** — [UEFI firmware](#firmware--boot-a-disk-through-its-uefi-loader) (the
  guest's own EFI loader), [direct kernel + FDT](#kernel--boot-a-kernel-directly-no-bootloader) (no
  bootloader), and PVH direct kernel on Linux/amd64 (via the
  [PVH libkrun fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot)).
- **macOS and Linux hosts** — Hypervisor.framework on Apple Silicon, KVM on Linux (amd64/arm64),
  behind one CLI.

**Machine management**

- **Docker-style lifecycle** — short ids, [`ps` / `logs -f` / `exec` / `shell` / `stop` / `start` /
  `update` / `rm`](#managing-machines) across every guest type; state in a small SQLite database,
  **no daemon required**.
- **In-guest exec agent** — [`exec`/`shell`](#the-exec-agent) run real processes in Linux *and* BSD
  guests over a small framed TCP protocol (PTY, env vars, exit codes) — no vsock dependency.
- **Copy-on-write everything** — per-machine APFS/reflink [clones](#managing-machines) boot many
  microVMs from one base image instantly; named persistent volumes (`-v`), host bind mounts
  (`--mount`), [extra disks](#disks) (`--attach-disk`), and [`grow`](#resizing-the-disk) to enlarge
  images.
- **AI coding agents, sandboxed** — [`bsdkrun claude`](#ai-coding-agents-in-sandboxes)
  (or codex / gemini / opencode / crush / copilot / kilo / qwen) runs a coding agent in
  a microVM that sees only the folder you share. Logins persist per agent, skills are
  shared across all of them, and each sandbox ships git, Docker and Nix.
- **Docker Desktop, replaced** — [`bsdkrun docker start`](#docker--replace-docker-desktop)
  runs a Docker engine in a microVM and serves its API on a host socket, so your own
  `docker`, `compose` and `buildx` drive it unchanged: published container ports are
  mirrored onto the host automatically, and `$HOME` is shared at the same path so
  `-v $PWD:/app` resolves.
- **Snapshot, branch, restore** — [`snapshot`](#snapshots-branches--restore) captures a machine's
  disk state as a CoW clone (instant, free until it diverges); `branch` boots a disposable copy of a
  snapshot *or of a running machine*; `restore`/`rollback` put one back — with the replaced state
  saved first, so an undo is always an undo away. Linux, FreeBSD, NetBSD and Unikraft alike.
- **Flavors** — [ready-to-boot environments](#flavors--preconfigured-environments--snapshots)
  (languages, databases, web servers, AI coding agents), your own `flavors.toml` stacks, and
  `commit` to freeze a running machine into a reusable flavor, like `docker commit`.
- **Clone a repo on boot** — `--repo <git-url>` clones into the guest (installing git if the base
  lacks it) and your shell lands in the checkout.

**Networking**

- **Internet by default** — every guest gets a virtio-net NIC behind
  [gvproxy](#networking) (userspace NAT with DHCP and DNS); `--port HOST:GUEST` forwards ports, and
  a unique host port is forwarded to the guest's SSH automatically.
- **Global networks** — [shared subnets with internal DNS](#global-networks--reach-machines-by-name)
  so machines reach each other by IP *and by name*, docker-compose style.
- **Machine domains** — [`bsdkrun domains enable`](#machine-domains--local-dns--https) gives every
  machine a browser-trusted `https://<name>.bsdk` URL (built-in DNS responder + Caddy's local CA).
- **SSH & Tailscale in one command** — [`bsdkrun ssh <id> setup`](#ssh--key-based-access-in-one-command)
  installs your keys and sshd; [`bsdkrun tailscale <id> setup`](#tailscale--put-a-guest-on-your-tailnet)
  puts the guest on your tailnet.

**Interfaces & tooling**

- **TUI dashboard** — [live panels](#tui--the-terminal-dashboard) for machines, images, volumes and
  networks, with fuzzy search, a new-machine wizard, and a follow-mode log viewer.
- **Desktop app** — machines list with a tabbed terminal panel (the screenshot up top).
- **SDKs for nine languages** — [TypeScript, Python, Ruby, Elixir, Gleam, Clojure, Go, Rust, and Scala](#sdks):
  thin, stateless wrappers around the binary.
- **Agent skill** — the [full CLI reference](#agent-skill) packaged for coding agents
  (`npx skills add tsirysndr/bsdkrun`).
- **Install anywhere** — [Homebrew, a curl one-liner, npm, or a Nix flake](#install); the macOS
  binaries ship pre-signed with the hypervisor entitlement.

---

## Why this exists

The usual microVM stacks (Firecracker, Cloud Hypervisor) don't run on macOS, and the usual macOS
VM tooling (QEMU, `vftool`, UTM) isn't microVM-shaped. libkrun gives you a Firecracker-like
"configure a context, then `start_enter`" model on top of Hypervisor.framework and KVM — but its
batteries are aimed at Linux guests. `bsdkrun` leans into that (running any OCI image
`docker run`-style, no Docker daemon) and then points the same machinery at everything libkrun was
never aimed at:

- **BSD guests as first-class microVMs.** FreeBSD and NetBSD boot to multi-user on macOS *and*
  Linux — via EFI firmware, direct kernel, or PVH direct kernel (through our
  [PVH-enabled libkrun fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot)) — and get
  the same `exec`/`shell`, volumes, and networking a Linux guest gets. This started as a research
  question — *can a BSD kernel enumerate the virtio-mmio devices libkrun describes, and find its
  console?* — and the answer turned out to be yes; the pieces that made it true (PVH entry, an
  MPTable, serial-console wiring) now live in the fork and in bsdkrun itself (see
  [Status](#status)).
- **One CLI for every unikernel.** Unikraft, Nanos, OSv, and MirageOS/Solo5 each ship their own
  runner with its own flags and quirks. bsdkrun boots them all with the same Docker-style
  lifecycle (`ps` / `logs` / `stop` / `rm`), and [`bsdkrun pack`](./pack)
  goes further: point it at an ordinary project and it builds a bootable Unikraft unikernel —
  no unikernel expertise required.
- **Docker ergonomics without a daemon.** Short ids, copy-on-write clones, named volumes,
  networks with internal DNS, `https://<name>.bsdk` domains, flavors and `commit` — for every
  guest type above, from one small CLI backed by a SQLite file.

---

## Install

**macOS (Apple Silicon)** — a prebuilt, already-signed binary via Homebrew:

```sh
brew install tsirysndr/tap/bsdkrun
```

This pulls in `tsirysndr/tap/libkrun` — our libkrun fork, which carries the PVH boot support and
the virtio-fs fixes bsdkrun's guests need — plus `gvproxy` from `libkrun/krun`, auto-tapping both.
The binary ships codesigned with the hypervisor entitlement, so there's nothing else to set up —
jump to [Usage](#usage).

If you already have upstream `libkrun/krun/libkrun`, the two conflict (same install paths) and
Homebrew will ask you to remove it first:

```sh
brew uninstall --ignore-dependencies libkrun
```

**curl** — install the prebuilt host binary for your platform (macOS/arm64, Linux/x64, Linux/arm64)
with a one-liner:

```sh
curl -fsSL https://raw.githubusercontent.com/tsirysndr/bsdkrun/main/install.sh | sh
```

The script downloads the matching release archives, verifies their SHA-256s, and unpacks
everything into `~/.bsdkrun/bin` (override with `BSDKRUN_INSTALL`): the `bsdkrun` CLI, the
[`bsdkrund`](daemon/) daemon, and `bsdkrun-supervisor` (which the daemon finds beside itself) —
one directory because the Linux CLI archive bundles libkrun beside the binary. It also fetches
`gvproxy` for guest networking (skip with `BSDKRUN_SKIP_GVPROXY=1`), and on macOS installs our
libkrun fork via Homebrew when `brew` is available. Pin a release with `BSDKRUN_VERSION`:

```sh
curl -fsSL https://raw.githubusercontent.com/tsirysndr/bsdkrun/main/install.sh | BSDKRUN_VERSION=v0.8.1 sh
```

**npm** — install the prebuilt host binary for your platform (macOS/arm64, Linux/x64, Linux/arm64):

```sh
npm install -g @bsdkrun/cli   # or: npx @bsdkrun/cli linux alpine -- echo hi
```

A postinstall step downloads the matching `bsdkrun` from the GitHub release and verifies its
SHA-256. On **Linux** the archive **bundles libkrun** (`libkrun.so`/`libkrunfw.so`, rpath'd to
`$ORIGIN`), so it works with no separate libkrun install — only `gvproxy` is needed for guest
networking. On **macOS** it's just the binary and links Homebrew's libkrun
(`brew install tsirysndr/tap/libkrun`).
Unsupported platforms (Windows, Intel macOS, 32-bit) fail the install with a clear message. See
[`npm/`](npm/) for details.

**Nix flake** — builds bsdkrun with all its dependencies. On **Linux (amd64/arm64)** it links
nixpkgs' libkrun; on **macOS** it links your Homebrew libkrun, so those need `--impure`
(`brew install tsirysndr/tap/libkrun` first) and produce a binary re-signed with the hypervisor
entitlement.

```sh
# Linux — needs /dev/kvm access: sudo usermod -aG kvm $USER && newgrp kvm
nix run           github:tsirysndr/bsdkrun -- linux alpine   # run without installing
nix profile install github:tsirysndr/bsdkrun                 # install into your profile
nix develop       github:tsirysndr/bsdkrun                   # dev shell with the full toolchain

# macOS (Apple Silicon) — impure link against Homebrew's libkrun
brew install tsirysndr/tap/libkrun
nix build  --impure github:tsirysndr/bsdkrun                  # -> ./result/bin/bsdkrun
nix run    --impure github:tsirysndr/bsdkrun -- linux alpine
```

The flake wraps the runtime tools (`curl`, `tar`, `gzip`, `xz`, `cpio`, `gvproxy`, …) onto `PATH`,
and `nix develop` adds the Rust toolchain plus `zig`/`cargo-zigbuild` for cross-building the guest
agents. The Solo5 tender is built from a pinned flake input, so a nix build needs **no**
`?submodules=1` — `nix build .#solo5-hvt` builds just the tender if that is all you want. To hack on bsdkrun without Nix, build from source — see [Prerequisites](#prerequisites) and
[Build](#build).

---

## Prerequisites

You need **libkrun**, a **Rust toolchain** (`rustup default stable`; edition 2021), and access to
the hypervisor. The hypervisor part differs by OS.

Optionally, a **Go toolchain** (`go 1.22+`, matching [`pack/go.mod`](./pack/go.mod)) if you want
`bsdkrun pack` in your build: `core/build.rs` compiles [`pack/`](./pack) and embeds it into the
`bsdkrun` binary. Without Go, the build still succeeds — it just warns and ships without pack
support (see `core/build.rs::ensure_pack_binary`). This is a **source-build-only** requirement: the
released binaries (Homebrew, npm) already have it embedded, so installing bsdkrun that way never
needs Go on your machine.

### macOS (Apple Silicon)

bsdkrun links **our libkrun fork** from the `tsirysndr/tap` tap. `krunkit` comes from the
**`libkrun/krun`** tap (redirected from the old `slp/krun`). Homebrew 6.x requires you to trust a
third-party tap before it will run its install code:

```sh
brew tap tsirysndr/tap
brew tap libkrun/krun
brew trust libkrun/krun     # required on Homebrew 6.x for third-party taps
brew install tsirysndr/tap/libkrun krunkit
```

- **`libkrun`** provides `libkrun.dylib` (the C ABI we link against).
- **`krunkit`** ships the EDK2 UEFI firmware we use for EFI boot
  (`.../share/krunkit/KRUN_EFI.silent.fd`).

**Why the fork and not upstream `libkrun/krun/libkrun`?** It carries two changes bsdkrun depends
on. PVH direct boot, which is how FreeBSD and NetBSD boot on Linux/amd64. And `XATTR_NOFOLLOW` in
the virtio-fs xattr handlers — upstream follows the final symlink, and since a nix profile is
built entirely from symlinks whose absolute targets only exist *inside* the guest, the host
resolves them against its own root and returns `ENOENT`. nix then aborts with `querying extended
attributes of "…": No such file or directory` on a path that `ls` happily shows. Upstream libkrun
still works for most guests; nix guests need the fork.

The two formulae install to the same paths and therefore conflict, so remove upstream's first if
you have it (`brew uninstall --ignore-dependencies libkrun`). They share an opt prefix — it is
keyed on the formula name, not the tap — so an already-linked bsdkrun keeps resolving across the
switch.

**The Hypervisor entitlement (the part that bites everyone).** A binary that calls libkrun must be
codesigned with `com.apple.security.hypervisor` (plus `com.apple.security.cs.disable-library-validation`
so it can load the Homebrew dylibs). Without it, `krun_create_ctx`/`krun_set_vm_config` succeed but
`krun_start_enter` fails at VM creation with `Internal(Vm(VmSetup(VmCreate)))` / **errno 22
(EINVAL)**. Worse, **every `cargo build` strips the codesignature**, so you must re-sign after each
build — the [`Makefile`](./Makefile) does this for you (entitlements in
[`bsdkrun.entitlements`](./bsdkrun.entitlements)).

### Linux (amd64 or arm64, KVM)

libkrun uses **KVM** on Linux — no codesigning, but you need **`/dev/kvm`** access. Add yourself to
the `kvm` group (or run under `sudo`):

```sh
sudo usermod -aG kvm $USER && newgrp kvm
```

Ubuntu has no libkrun package, so build it (and its bundled kernel, libkrunfw) from source — see the
[KVM e2e workflow](.github/workflows/e2e-linux.yml) for the exact steps:

```sh
git clone --depth 1 https://github.com/containers/libkrunfw && make -C libkrunfw && sudo make -C libkrunfw install
git clone --depth 1 https://github.com/containers/libkrun   && make -C libkrun   && sudo make -C libkrun   install
sudo ldconfig
```

`build.rs` finds libkrun via `pkg-config libkrun` (or the standard lib dirs; override with
`LIBKRUN_PREFIX=/path`). There's nothing to sign — `make build` skips the codesign step on Linux.
Some BSD image-prep steps (`losetup`/`mount`) need root, and bsdkrun runs them with `sudo`
automatically when needed.

> On Linux the CI boots the `linux` (OCI), `netbsd`, and `freebsd` (both direct-kernel, PVH) paths
> — on x86_64 only, since GitHub's arm64 runners have no `/dev/kvm`; arm64-on-Linux (which reuses
> the aarch64 kernel + agent) is validated on a KVM-capable host. The BSD-under-KVM amd64 boots go
> via our [PVH libkrun fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot), which the
> CI builds.

---

## Build

```sh
git submodule update --init library/solo5   # the Solo5 tender is built from source
make build      # cargo build (debug)  [+ codesign on macOS]
make release    # cargo build --release [+ codesign on macOS]
```

The submodule is only needed for `bsdkrun solo5`, and only for a `cargo` build — the Nix flake
pins the same commit as a flake input instead. Without it the build still succeeds: it prints a
warning and leaves the tender out, and `bsdkrun solo5` then says so rather than failing
obscurely. Building the tender needs `make` and, on Linux, `libseccomp` headers.

> The two pins must name the same commit. Bump them together with
> `git submodule update --remote library/solo5 && nix flake update solo5`; the `e2e-solo5`
> workflow fails if they drift.

`make run ARGS="..."` builds and runs in one step.

> ⚠️ **macOS: don't run `cargo build` then the binary directly** — it'll be unsigned and fail at
> boot with errno 22. Go through `make`, or re-run `make sign` after a bare `cargo build`. On Linux
> there's nothing to sign, so `cargo build` is fine (the `make` sign steps are no-ops there).

The [`build.rs`](./build.rs) locates libkrun via `brew --prefix libkrun` (macOS) or `pkg-config`
(Linux), override with `LIBKRUN_PREFIX=/path`, and embeds an rpath so the shared library resolves
at runtime.

### Monorepo dev console (`./console`)

Not to be confused with the [guest serial console](#console-how-output-reaches-your-terminal)
below — this is a contributor tool. `./console` (or `cd tools/console && clj -M:rebel`) drops you
into a Clojure REPL that centralizes every build/test/publish command in this monorepo — the
`Makefile` targets above, all six SDKs' test/build/publish steps, and `web`/`desktop`'s dev
servers — as plain functions (`(build/release)`, `(sdk/test :clojure)`, `(sdk/publish :ruby)`,
`(web/dev)`) instead of remembering which tool owns which command. `bb help` from
`tools/console` works the same way without a REPL. See
[`tools/console/README.md`](./tools/console/README.md).

---

## Usage

```
bsdkrun [--log-level N] <command>

Global:
  --log-level N   log verbosity, 0=off .. 5=trace (default 1)
```

`--log-level` drives two things at once: libkrun's internal logging *and* bsdkrun's own logging
(via the [`tracing`](https://docs.rs/tracing) crate). Both go to **stderr**, so they never mix with
the guest console on stdout. `0`→warn, `1..3`→info, `4`→debug, `5`→trace. Set the `RUST_LOG`
environment variable to override bsdkrun's filter with anything
[`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
accepts (e.g. `RUST_LOG=bsdkrun=debug`). For a clean guest console, use `--log-level 0` or `1`.

### `probe` — sanity check the toolchain

Verifies that libkrun links and that a context can be created and configured. Does **not** boot
(so it won't exercise the hypervisor entitlement — see the note above).

```sh
make run ARGS="probe"
```

### `freebsd` / `netbsd` — one-liner BSD microVMs

The quickest way to a BSD guest — fetches the image (and, for NetBSD, the kernel) if needed, then
boots it:

```sh
bsdkrun freebsd                 # bundled FreeBSD image (agent baked in): EFI on macOS, PVH on Linux/amd64
bsdkrun freebsd --version 14.3  # official FreeBSD 14.3 VM image from download.freebsd.org
bsdkrun netbsd  -d              # NetBSD-current in the background; prints its id
bsdkrun netbsd  --version 10.1 -d --port 2222:22
```

Like `bsdkrun linux`, you can pass a **command to run inside the guest** after `--`. The guest
boots, its agent runs the command (streaming stdout/stderr), and — without `-d` — the VM powers
off afterward, with bsdkrun exiting on the command's status (a one-shot, à la `docker run`). With
`-d` the machine is left running once the command returns. Needs networking (the agent), so it's
incompatible with `--no-net`:

```sh
bsdkrun freebsd -- uname -a           # boot, run, print, power off; exit = command's status
bsdkrun netbsd  -- sh -c 'sysctl hw.model'
bsdkrun freebsd -d -- pkg install -y curl   # run a setup step, then leave the VM up
```

With **no command** (and no `-d`), a foreground `bsdkrun freebsd` / `bsdkrun netbsd` drops you
straight into an **interactive shell** over the agent — the bundled images are headless (no console
login), so this is the way in — and powers the VM off when you exit, like foreground `bsdkrun
linux`. A short `⋯ waiting for the guest agent` line shows while the guest boots (~15-20s).

```sh
bsdkrun freebsd        # boot, then an interactive /bin/sh; exit (Ctrl-D) powers it off
bsdkrun netbsd -d      # no shell — just background it and print the id (use exec/ssh/shell)
```

Add **`--verbose`** to stream the guest's **boot console to stdout** while it comes up (instead of
the terse spinner) — handy for watching a boot or asserting on it in CI. The command output / shell
follows once the agent is up:

```sh
bsdkrun freebsd --verbose -- uname -a   # full boot log, then the command output, on stdout
```

Both carry the usual machine options (`-d`, `--persist`, `-v/--volume`, `--version`,
`--attach-disk`, `--port`, `--cpus`/`--mem`), so per-machine [CoW disk clones](#managing-machines)
and `ps`/`logs`/`shell`/`stop` all apply. They differ in how they boot:

- **`freebsd`** boots differently per host OS:
  - On **macOS** it wraps [`fetch`](#the-easy-way--bsdkrun-fetch) + [`firmware`](#firmware--boot-a-disk-through-its-uefi-loader):
    it auto-locates libkrun's `KRUN_EFI` firmware (via `$BSDKRUN_FIRMWARE`, a local
    `images/KRUN_EFI.fd`, or krunkit's Homebrew install; `--firmware` overrides). By default (on
    arm64) it downloads bsdkrun's **bundled image with the guest agent pre-installed**, so
    `exec`/`shell` work out of the box; pass **`--version X`** to boot an official FreeBSD VM image
    from download.freebsd.org instead (no agent — you'd install it manually).
  - On **Linux/amd64** it **PVH-direct-boots** bsdkrun's bundled agent-injected UFS rootfs + the
    FreeBSD **`FIRECRACKER`** kernel (no ACPI, MPTable enumeration, virtio-mmio + serial built in;
    built from source by the
    [image workflow](.github/workflows/release-freebsd-amd64-image.yml)). Needs the
    [PVH libkrun fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot); override the
    kernel command line with `$BSDKRUN_FREEBSD_CMDLINE`.
- **`netbsd`** wraps `fetch` + a **direct kernel** boot — no firmware — so it works on **macOS and
  Linux**.

### `firmware` — boot a disk through its UEFI loader

The explicit form (the `freebsd`/`netbsd` shortcuts wrap it). Point it at libkrun's EDK2 firmware
and a raw disk image that carries an EFI System Partition:

```sh
make release
./target/release/bsdkrun firmware \
  --firmware "$(brew --prefix)/share/krunkit/KRUN_EFI.silent.fd" \
  --disk images/fbsd15.raw \
  --cpus 2 --mem 2048
```

- Use libkrun's **own** `KRUN_EFI.silent.fd` firmware — **not** a generic QEMU `AAVMF`/`edk2`
  build. libkrun uses its own guest memory layout, and only its firmware matches it. The
  `.silent` variant just suppresses EDK2's own console chatter; the guest OS console still comes
  through the serial console (see [Console](#console-how-output-reaches-your-terminal) below).
- The disk is attached read-write as a virtio-blk device (`block_id = "root"`).

This path is confirmed booting **FreeBSD 15 / arm64** all the way to a `login:` prompt. Two
things make it work: bsdkrun wires the guest's serial console to your terminal (see below), and
the FreeBSD image needs a one-line console hint on its ESP (see
[Preparing a FreeBSD arm64 image](#preparing-a-freebsd-arm64-image)).

### `kernel` — boot a kernel directly (no bootloader)

The path for **NetBSD/evbarm** and bare kernel experiments. libkrun generates the FDT and jumps
into the kernel:

```sh
./target/release/bsdkrun kernel \
  --kernel path/to/netbsd \
  --format elf \
  --cmdline "console=..." \
  --disk images/root.raw \
  --cpus 1 --mem 512
```

`--format` is one of `elf` (default) or `raw`. `--initramfs` is optional.

### `linux` — run an OCI image as a microVM

Run any Docker Hub / OCI image as a Linux microVM, `docker run`-style. bsdkrun fetches a prebuilt
aarch64 kernel (cached), pulls the image for `linux/arm64`, extracts its rootfs, and boots it:

```sh
# run alpine's default shell
./target/release/bsdkrun linux alpine

# run a specific command, with more RAM
./target/release/bsdkrun linux alpine --mem 1024 -- /bin/sh -c 'uname -a; cat /etc/os-release'

# any registry / tag
./target/release/bsdkrun linux ghcr.io/owner/name:tag
```

How it works:

- **Kernel** — a prebuilt aarch64 `vmlinux` is downloaded from
  [vmlinux-builder](https://github.com/tsirysndr/vmlinux-builder) and cached. libkrun's aarch64
  loader needs the raw `Image` format, so bsdkrun flattens the ELF to an `Image` (in pure Rust — no
  binutils) and caches that too. Pick a release with `--kernel-version` (default `7.2`), or point
  at your own kernel with `--kernel /path` (ELF or raw `Image`).
- **Rootfs** — the OCI image is pulled straight from the registry (no Docker daemon; just `curl` +
  `tar`) and its layers are extracted, applying whiteouts. The result is **cached, content-addressed
  by image digest**, so a repeat run is instant. By default the rootfs is packed into an
  **initramfs** and booted from RAM, with a generated `/init` that mounts `/proc` + `/sys`,
  configures networking, and runs the image's Entrypoint/Cmd (honoring `Env`/`WorkingDir`).
- **Entrypoint** — Docker semantics: args after `--` replace the image `Cmd`; `--entrypoint`
  replaces the Entrypoint. When the workload exits, the VM powers off cleanly.
- **Networking** — on by default (see [Networking](#networking)); the guest gets internet access
  (ICMP/DNS/TCP) via gvproxy, configured through the kernel command line so the image itself needs
  no `ip`/`dhcp` tools. `--no-net` disables it.

Notes:

- **Rootfs** — by default the rootfs is served from disk over **virtio-fs** (no RAM-size limit, so
  it's fine for large images). bsdkrun clones the cached rootfs per machine with an APFS
  copy-on-write clone (`cp -Rc` — instant, no extra disk until the guest writes), so machines stay
  isolated and the shared image cache stays pristine. It boots our own init from the shared root
  (not libkrun's `init.krun`, which only works with the bundled libkrunfw kernel). Needs a guest
  kernel with `CONFIG_VIRTIO_FS=y` (note: `CONFIG_FUSE_FS` alone is **not** enough — the default
  prebuilt kernel has it).
- **`--initramfs`** boots from an initramfs instead (the whole rootfs is loaded into RAM). Use it
  for a kernel without virtio-fs. Then size `--mem` above the image size — bsdkrun warns if it looks
  too small.
- Either way the image must have a `/bin/sh` (scratch/distroless images won't boot this way), and
  the console defaults to `hvc0` (libkrun's virtio-console; `--console` overrides it).

#### Case-sensitive Linux storage on macOS

Linux filesystems are case-sensitive, but the default macOS APFS boot volume usually is not. OCI
images and Linux workloads can contain paths that differ only by case—for example, the Linux
kernel contains both `ipt_ECN.h` and `ipt_ecn.h`. Serving a rootfs directly from a case-insensitive
directory through virtio-fs would merge those files and silently corrupt the guest filesystem.

On macOS, bsdkrun therefore requires a case-sensitive backing store for OCI image trees, writable
machine roots, and named volumes. The first `bsdkrun linux ...` invocation creates it automatically
as a case-sensitive APFS sparsebundle and mounts it below the bsdkrun cache. A 200 GiB capacity is
the default ceiling, but the sparsebundle initially consumes only the blocks it actually uses.
Linux hosts already use case-sensitive filesystems and do not create this store.

Manage or inspect the store explicitly with:

```sh
bsdkrun store status
bsdkrun store init --size 300g  # optional manual initialization/custom ceiling
bsdkrun store detach           # all machines using it must be stopped
bsdkrun store attach
bsdkrun store rm --force       # destructive: removes cached images and volumes
```

Initialization refuses to migrate storage while a machine is running. Stop active machines and
retry. Existing OCI cache trees are discarded rather than migrated because extraction on a
case-insensitive filesystem may already have corrupted them; images are pulled again into the new
store. Existing Linux machines created before the store should also be removed and recreated so
they cannot resume a damaged rootfs:

```sh
bsdkrun stop <id>
bsdkrun rm <id>
bsdkrun linux alpine
```

Virtio-fs remains the guest transport. The sparsebundle fixes the semantics of its host-side
backing filesystem; it does not replace virtio-fs or load the rootfs into guest memory.

### systemd — turn an OCI guest into a full systemd system

OCI images boot with bsdkrun's tiny generated `/init` as PID 1 — great for `docker run`-style
workloads, but no services, journal, or timers. One command flips a **debian/ubuntu/fedora**
guest to real systemd:

```sh
id=$(bsdkrun linux -d -v dev debian -- sleep infinity)
bsdkrun systemd $id setup      # installs systemd if missing + the agent unit, marks the rootfs
bsdkrun stop $id
id=$(bsdkrun linux -d -v dev debian)   # same volume -> boots systemd as PID 1
bsdkrun systemd $id status     # "PID 1: systemd"
```

`setup` installs systemd where missing (`apt-get`/`dnf` — Alpine has no systemd and fails with a
clear message), writes + enables a `bsdkrun-agent.service` unit (so `exec`/`shell` keep working
under systemd), and drops a marker (`/etc/bsdkrun-systemd`) that makes the generated init **exec
systemd as PID 1** on the next boot. `disable` removes the marker. In systemd mode the image
entrypoint/`--` command is not run — systemd owns userspace; manage workloads as units. Boot on a
**volume** (`-v`) so the installed packages + marker survive across machines.

### `unikraft` — boot a Unikraft unikernel

[Unikraft](https://unikraft.org) builds your application *into* the kernel: one binary, no
userland, no init, no shell. A hello-world image is ~220 KB and boots in milliseconds. bsdkrun
runs those as microVMs the same way it runs everything else.

Build for the **Firecracker platform** (`--plat fc`) — its memory layout and boot protocol are
what libkrun implements — then point `bsdkrun unikraft` at the result.

#### Step by step

**1. Install `kraft`** (Unikraft's build tool):

```sh
brew install unikraft/cli/kraftkit          # macOS
curl -sSfL https://get.kraftkit.sh | sh     # Linux
```

**2. Get an example from the catalog.** [`unikraft/catalog`](https://github.com/unikraft/catalog)
holds ready-made apps — `library/helloworld` is the smallest (one `main.c`):

```sh
git clone --depth 1 https://github.com/unikraft/catalog
cd catalog/library/helloworld
```

**3. On arm64, add the PL011 console driver.** libkrun's aarch64 serial is an ARM PL011;
Firecracker's is an ns16550, so a stock `fc/arm64` build boots *silently*. Both drivers probe the
device tree, so enabling PL011 alongside the default costs nothing and gives you one image that
boots under either VMM. Replace the `Kraftfile` with:

```yaml
spec: v0.6

name: helloworld

unikraft:
  version: stable
  kconfig:
    CONFIG_LIBPL011: 'y'
    CONFIG_LIBPL011_EARLY_CONSOLE: 'y'

targets:
- fc/arm64
```

On **x86_64** no override is needed — the `fc` platform already uses an ns16550 on COM1, which is
the serial port libkrun exposes. Just list `fc/x86_64` as the target.

**4. Build it.** The Unikraft build needs GNU make/sed and a Linux toolchain, so on macOS run it
in a container (`kraft` itself works natively, but the Unikraft tree it drives does not):

```sh
# Linux
kraft build --arch arm64 --plat fc          # or --arch x86_64

# macOS — build inside Linux, the image is written to .unikraft/build/
docker run --rm -v "$PWD":/w -w /w debian:bookworm sh -c '
  apt-get update -qq && apt-get install -y -qq --no-install-recommends \
    build-essential libncurses-dev libyaml-dev flex bison git wget unzip \
    uuid-runtime python3 curl ca-certificates bc >/dev/null
  curl -sSfLo /tmp/k.deb https://github.com/unikraft/kraftkit/releases/download/v0.12.15/kraftkit_0.12.15_linux_arm64.deb
  dpkg -i /tmp/k.deb
  kraft build --arch arm64 --plat fc --log-type basic'
```

**5. Boot it:**

```sh
bsdkrun unikraft .
```

That's it:

```
Powered by
o.   .o       _ _               __ _
Oo   Oo  ___ (_) | __ __  __ _ ' _) :_
oO   oO ' _ `| | |/ /  _)' _` | |_|  _)
oOo oOO| | | | |   (| | | (_) |  _) :_
 OoOoO ._, ._:_:_,\_._,  .__,_:_, \___)
                          Ijiraq 0.21.0
Hello from Unikraft!
```

#### Usage

The argument is a **project directory** (bsdkrun finds the image under `.unikraft/build/`) or the
**image itself**:

```sh
bsdkrun unikraft                                          # the current directory
bsdkrun unikraft ~/catalog/library/helloworld             # a project directory
bsdkrun unikraft .unikraft/build/helloworld_fc-arm64      # the image
bsdkrun unikraft -d .                                     # detached; use logs/stop/rm
bsdkrun unikraft . --cmdline "helloworld --port 8080"     # cmdline arrives as argv
bsdkrun unikraft . --initramfs initrd.cpio                # for a --rootfs build
bsdkrun unikraft . --mount ~/data:/data                   # persistent volume
```

A unikernel has no shell and no agent, so `exec` and `shell` don't apply — `logs`, `ps`, `stop`,
`start`, and `rm` all work as usual. When `main` returns, the guest powers the VM off.

#### Volumes

`--mount HOST:GUEST` (repeatable) shares a host directory into the guest over **virtio-fs**, so
data written there survives the VM. The guest path must be absolute.

```sh
bsdkrun unikraft . --mount ~/pgdata:/var/lib/data
```

virtio-fs is the only option that works: libkrun has no virtio-9p (what `kraft run -v` uses under
QEMU), and while both sides have virtio-blk, Unikraft's core registers only `ramfs` and `virtiofs`
as filesystems — a raw disk gives you blocks, not a mountable tree.

The unikernel has to be built for it (`CONFIG_LIBUKFS_VIRTIOFS`, `CONFIG_LIBPOSIX_VFS_FSTAB_USER`,
`CONFIG_LIBUKFS_RAMFS`), and unikraft 0.21.0 needs two one-line upstream fixes before a virtio-fs
guest works at all. [`examples/unikraft-volume`](examples/unikraft-volume) is a working setup with
both patches and a fetch → patch → build script; its README explains each.

bsdkrun generates the mount table onto the kernel command line — a program name, the `vfs.fstab`
entries, then `--` — and mounts a ramfs at `/` first so the mountpoints have somewhere to exist.
`commit` still doesn't apply: the data lives in a host directory, so there is no disk image to
snapshot.

#### How it boots

The `fc` platform links at `0x8000_0000` (arm64), which is exactly where libkrun's aarch64 loader
places a raw image, and it boots via the Linux protocol with the device tree in `x0` — which is
what libkrun sets up. Two things bsdkrun handles for you:

- **The entry shim (arm64).** Unikraft reserves the first megabyte of RAM for the DTB, so its
  `Image` header asks to be loaded at `0x8000_0000 + 0xf_ffc0`, and the build is not relocatable.
  libkrun ignores `text_offset` and enters at `0x8000_0000` — inside the reserved hole. bsdkrun
  front-pads the image so every byte lands at its link address and writes a single `b` instruction
  at offset 0, so libkrun's fixed entry branches to the real one (a branch preserves `x0`). The
  shimmed image is cached under `$XDG_CACHE_HOME/bsdkrun/unikraft`, keyed by content.
- **The image format.** arm64 takes the raw `Image` (or the `.dbg` ELF beside it — same bytes once
  flattened); x86_64 hands the ELF straight to libkrun's ELF loader, which enters at `e_entry`
  (`_lxboot_entry`) in long mode with the zero page in `RSI` — exactly the 64-bit Linux protocol
  Unikraft's `fc` platform expects, including the `HdrS` magic it checks before it will run.

Booting on **arm64/macOS** is verified end to end; **x86_64** is covered by the
[`e2e-unikraft`](.github/workflows/e2e-unikraft.yml) CI workflow, which builds a catalog unikernel
with `kraft` and boots it under KVM.

> **arm64 needs a patched libkrun.** Unikraft's PL011 driver writes its registers 16 bits at a
> time (`strh`), and libkrun's HVF MMIO path only handled 1/4/8-byte writes — a halfword write
> panicked the vCPU thread with `unsupported mmio len=2`. The read path already handled it; the
> fix is the matching `2 =>` arm in the write path. Upstreaming is in progress; until it lands,
> build libkrun from [`tsirysndr/libkrun`](https://github.com/tsirysndr/libkrun).

### `nanos` — boot a Nanos (NanoVMs) unikernel

[Nanos](https://nanos.org) implements the Linux syscall ABI, so the guest starts as an ordinary
**static Linux binary** and `ops` wraps it into a bootable image. `bsdkrun nanos` boots that image
directly — pass either a path, or a bare name from `~/.ops/images/` (what `ops build -i <name>`
produces):

```sh
curl https://ops.city/get.sh -sSfL | sh   # install ops
./build.sh                                # builds ~/.ops/images/nanos-hello
bsdkrun nanos nanos-hello --no-net
```

Like every unikernel, a Nanos guest has no shell and no agent: `exec`, `shell`, and `commit` do
not apply; `logs`, `ps`, `stop`, `start`, and `rm` work as usual.

#### Status

- **Linux/x86_64** boots via direct kernel load — the same Firecracker-style contract Nanos
  supports officially. `bsdkrun nanos` auto-finds the newest `~/.ops/<version>/kernel.img` that
  `ops build` staged, or you can override it with `--kernel`.
- **macOS/arm64** uses EFI boot with `KRUN_ACPI=1` and needs an image built with `"Uefi": true`.
  It boots to userspace with a **patched Nanos kernel + bootloader** (nanos fork, branch
  `fix/aarch64-libkrun-boot`, staged into `~/.ops/<version>-arm/`) and the libkrun fork's
  level-SPI ack fix; stock 0.1.55 dies before userspace. See
  [`examples/nanos-hello`](examples/nanos-hello/README.md) for the five root causes and the
  staging steps.
- **Linux/arm64** is not bootable today: the Nanos kernel links below libkrun's direct-kernel RAM
  base, and Linux hosts have no EFI path for it.

#### Usage

```sh
bsdkrun nanos nanos-hello                  # name in ~/.ops/images
bsdkrun nanos ~/.ops/images/nanos-hello    # explicit image path
bsdkrun nanos -d nanos-hello --no-net      # detached; use logs/stop/rm
bsdkrun nanos nanos-hello --kernel ~/.ops/0.1.55/kernel.img
```

---

### `solo5` / `mirage` — run a MirageOS (Solo5) unikernel

[MirageOS](https://mirage.io) unikernels run on [Solo5](https://github.com/Solo5/solo5), whose
`hvt` **tender** is itself the hypervisor front end. So this guest is the one exception in
bsdkrun: it does not go through libkrun. bsdkrun builds the tender from the pinned
`library/solo5` submodule at compile time and embeds it, so **running** a unikernel needs no
Solo5 install of your own — only building one does.

```sh
cd examples/mirage-hello && ./build.sh          # needs opam + mirage
bsdkrun solo5 dist/hello.hvt --mem 128 --port 18080:8080
curl http://127.0.0.1:18080/                    # Hello from MirageOS on bsdkrun
```

`bsdkrun mirage` is a visible alias for the same command.

Note what the command line does *not* carry: no device names, no MAC, no IP, no gateway. Every
Solo5 unikernel declares the devices it wants in its own binary (the `MFT1` ELF note), and the
tender refuses to boot unless each one is attached — so bsdkrun reads the manifest and attaches
them itself:

```
INFO running Solo5 unikernel id=4bc3fd4a9851 image=hello.hvt nets=["service"] blocks=[]
```

The network reaches the outside through gvproxy. bsdkrun opens that socket itself and hands the
tender the descriptor (`--net:service=@3`) — on macOS this is the only way to attach a network
at all, there being no TAP device — and pins the guest's MAC so gvproxy's DHCP lease always
lands on the address `--port` forwards to.

A declared block device is backed with `--block`:

```sh
bsdkrun solo5 dist/store.hvt --block storage=disk.img
```

Leave it out and bsdkrun names the missing device before starting anything.

Like every unikernel, a Solo5 guest has no shell and no agent: `exec` and `shell` do not apply;
`ps`, `logs`, `stop` and `rm` work as usual, and `stop` kills the tender rather than just
bsdkrun, so nothing is left holding the ports.

#### Status

- **macOS/arm64 (Hypervisor.framework)** — verified: builds, boots, leases a DHCP address and
  serves over a forwarded port. The HVF backend comes from
  [tsirysndr/solo5](https://github.com/tsirysndr/solo5) `hvf-macos-aarch64`, which upstream
  Solo5 does not have. Two gaps it documents rather than papers over: the tender drops no
  privileges on macOS (no seccomp/pledge/capsicum equivalent), and there is no
  `solo5-hvt-debug` — the gdb and dumpcore backends are unported.
- **Linux (KVM)** — the upstream backend, exercised by the `e2e-solo5` workflow, which asserts
  on the HTTP body the unikernel serves.

#### Usage

```sh
bsdkrun solo5 dist/hello.hvt                  # explicit unikernel
bsdkrun solo5 .                               # a project dir; finds dist/*.hvt
bsdkrun mirage dist/hello.hvt --port 8080:8080
bsdkrun solo5 -d dist/hello.hvt               # detached; use logs/stop/rm
bsdkrun solo5 dist/hello.hvt -- --extra-arg   # arguments for the unikernel itself
```

See [`examples/mirage-hello`](examples/mirage-hello/README.md) for the full walkthrough.

---

## Managing machines

bsdkrun keeps a small SQLite database (`sqlx`) under `$XDG_STATE_HOME/bsdkrun` recording the
machines you run, the images you've pulled, and disks you've attached — each with a **Docker-style
short id**. The same Docker-like commands work for **every guest type** — Linux (`linux`) and BSD
(`firmware` / `kernel`) alike — bsdkrun records each machine's kind and applies the right logic:

```sh
# run any machine in the background; prints its id
id=$(bsdkrun linux -d alpine)
id=$(bsdkrun firmware -d --firmware images/KRUN_EFI.fd --disk images/fbsd15.raw)

bsdkrun ps                 # list running machines (-a for all, incl. exited)
bsdkrun images             # list images: pulled OCI images + fetched BSD images
bsdkrun logs $id           # print the machine's console log
bsdkrun logs -f $id        # follow it live
bsdkrun exec $id uname -a  # run a command inside the guest (-t for a PTY, -e K=V for env)
bsdkrun cp ./app.py $id:/app/app.py   # copy a file in (-r for a directory, - for stdin/stdout)
bsdkrun cp $id:/var/log/app.log ./    # ...and back out
bsdkrun cache save $id:/root/.cargo --key deps-v1     # archive a guest dir under a key
bsdkrun cache restore $id --key deps-v1               # ...and put it back later
bsdkrun shell $id          # open an interactive shell in the guest
bsdkrun doctor             # check this host can run machines, and what to fix if not
bsdkrun stop $id           # stop a running machine (BSD guests clean-poweroff first)
bsdkrun start $id          # re-boot a stopped machine in place — resumes its own disk/rootfs
bsdkrun update $id --cpus 4 --mem 2048   # change recorded vCPU / RAM (applies on next start)
bsdkrun snapshot $id v1    # capture its disk state (CoW — instant); see Snapshots below
bsdkrun branch $id -d      # boot a copy of it, leaving the original untouched
bsdkrun rollback $id -f    # undo back to its most recent snapshot
bsdkrun rm $id             # remove a machine and its state (-f stops it first)
```

Any unique **id prefix** works (`bsdkrun stop 8e1c`). `shell` attaches to the guest console: for a
Linux machine that's an interactive shell (with `exit`/re-attach); for BSD it's the guest's own
console (e.g. the `login:` prompt).

**`stop`/`start` persist your data.** A `start` resumes the machine's **own** disk/rootfs — the
committed snapshot it was booted from *plus* any runtime changes — not a fresh copy of the base
image, so stopping and starting keeps your data (like `docker start`). For BSD guests, `stop`
cleanly powers the guest off (`shutdown -p now`) so its UFS is unmounted and consistent before the
next boot — this takes a few seconds; without it a killed live UFS would be torn and `fsck` would
discard recent writes. (A brand-new `run` without `-v`/`--persist` is still ephemeral — see below.)

**Clone a repo on boot (`--repo`)** — pass a git URL to any run and bsdkrun clones it into the
guest after boot (installing `git` first if the base lacks it — apt/apk/dnf/pacman/pkg/pkgin/…) and
drops you into it when you open a shell:

```sh
bsdkrun linux  -d --repo https://github.com/owner/app node:22   # clone into ~/app, cd on shell
bsdkrun freebsd -d --repo https://github.com/owner/app
```

**Copy-on-write disks** — a BSD machine's root disk is cloned per machine with an APFS `clonefile`
(`cp -c` — instant, and costs no extra disk until the guest writes), so you can boot **many
microVMs from one base image** concurrently without touching it. Pass `--persist` to boot the disk
in place instead (writes persist; one machine at a time). Linux machines get the same isolation via
their per-machine virtio-fs clone.

**Persistent volumes (`-v NAME`)** — by default every boot starts from a fresh clone, so guest
changes are thrown away when the machine exits. To keep them across reboots, name a volume — works
the same for Linux, FreeBSD and NetBSD:

```sh
bsdkrun linux   -d -v web alpine            # persistent Linux rootfs
bsdkrun freebsd -d -v db                    # persistent FreeBSD disk
```

Reuse the same `-v NAME` and the machine comes back up with your changes intact. It's a single
writer at a time (run one machine per volume), and it's mutually exclusive with `--persist` (which
writes to the base image itself). The mechanism is the same idea for every guest — a copy-on-write
clone that persists:

- **Linux** serves the volume as a **writable virtio-fs root**. First use copy-on-write clones the
  OCI image into the volume dir (`clonefile` on APFS / `reflink` on btrfs/xfs — instant, and only
  grows as the guest writes; a plain copy elsewhere); later boots reuse it, so your changes persist.
  Needs the default virtio-fs (not `--initramfs`, a RAM disk with nothing to persist). (Earlier
  builds layered overlayfs over virtio-fs, but the Linux/KVM kernel rejects a virtio-fs overlay
  upperdir, so a plain writable clone is used instead — it works identically on macOS and Linux.)
- **FreeBSD / NetBSD** use an **APFS copy-on-write clone** of the disk image under
  `<state>/volumes/<NAME>` (instant, and only grows as the guest writes).

Volumes are recorded in the state DB and managed Docker-style:

```sh
bsdkrun volume ls              # NAME, GUEST, BASE, SIZE (du, CoW-aware), CREATED
bsdkrun volume rm web          # delete a volume's data (refused if a machine is using it)
bsdkrun volume rm -f web db    # force removal / multiple names
```

**Bind-mount host directories (`--mount`, Linux only)** — share a host directory into the guest at
a path, like `docker run -v`. Repeatable; append `:ro` for read-only:

```sh
bsdkrun linux --mount ~/project:/src --mount ~/data:/data:ro alpine -- ls /src
```

Each `--mount HOST:GUEST[:ro]` becomes a virtio-fs share the generated init mounts at `GUEST`
(the host dir must exist; `GUEST` must be absolute). Reads and writes pass straight through to the
host, and it composes with `-v` (persistent volume) and every Linux root mode.

Linux guests can also attach **raw disk images** as `virtio-blk` devices with
`--attach-disk PATH[:ro]` (repeatable) — see [Disks](#disks).

How it works — no daemon:

- **`-d` detached** — bsdkrun forks; the child `setsid`s, wires the guest console (`hvc0`) to a
  per-machine **PTY**, and a broker thread fans that PTY out to `console.log` and a Unix socket
  (`console.sock`) under `…/machines/<id>/`. The parent records the machine (with the child's pid)
  and prints the id. (libkrun's implicit console only writes to a **tty** — a plain pipe/socket
  would just get *logged* — which is why the console is a PTY.)
- **`logs`** reads `console.log`; **`-f`** then streams `console.sock`.
- **`exec` / `shell`** run a **new** process in the guest through an in-guest agent (see below) —
  `docker exec`, not `docker attach`. `exec` forwards stdin/stdout/stderr and the exit code (`-t`
  allocates a PTY, `-e K=V` sets env); `shell` is `exec -t /bin/sh`. When a machine has no agent,
  `shell` falls back to attaching the guest **console** over `console.sock` (raw-mode proxy, the
  recent console replayed on attach, **Ctrl-]** to detach).
- **Persistence** — a detached machine with **no explicit command** (`bsdkrun linux -d alpine`)
  keeps a console shell alive: typing `exit` (or Ctrl-]) returns you to your **host** prompt and
  leaves the machine **running** — re-attach any time with `shell` for a fresh shell; the machine
  ends only when you `stop` it. Give it a command (`… -d alpine -- myserver`) and it behaves like
  Docker instead — the machine powers off when that command exits.
- **`stop`** sends `SIGTERM` to the machine's process (whose signal handler tears down gvproxy).
- **`ps`** reconciles: a machine still marked *running* whose process is gone is shown as *exited*.

### The exec agent

`exec`/`shell` talk to a tiny in-guest **agent** that listens on **TCP port 1024** and runs one
command per connection over a small framed protocol (stdin/stdout/stderr + exit code, optional
PTY). There's no vsock dependency: bsdkrun forwards a per-machine host port to the guest through
gvproxy, so the same mechanism works for Linux and BSD. The agent needs the guest's network up
(gvproxy leases it `192.168.127.2`), so `exec`/`shell` require networking (not `--no-net`).

- **Linux** — bsdkrun downloads the aarch64 agent from the GitHub release (cached under
  `~/.cache/bsdkrun/agent/`), injects it into the rootfs, and starts it on boot. Nothing to do.
  (Point `BSDKRUN_AGENT_LINUX` at a local binary, or `BSDKRUN_AGENT_VERSION` at a different tag,
  to override.)
- **FreeBSD / NetBSD** — bsdkrun can't write the guest's UFS/FFS from macOS, so you install the
  agent yourself, once, inside the running guest.

**BSD setup** (as root — the default no-password user). Download the matching binary from the
release into the guest, which has internet by default:

```sh
# FreeBSD (fetch is built in):
fetch -o /usr/local/sbin/bsdkrun-agent \
  https://github.com/tsirysndr/bsdkrun/releases/download/v0.1.0/bsdkrun-agent.freebsd-aarch64

# NetBSD (ftp speaks https):
ftp -o /usr/local/sbin/bsdkrun-agent \
  https://github.com/tsirysndr/bsdkrun/releases/download/v0.1.0/bsdkrun-agent.netbsd-aarch64

chmod +x /usr/local/sbin/bsdkrun-agent
/usr/local/sbin/bsdkrun-agent &          # start it now; listens on TCP :1024
```

From the host, `bsdkrun exec <id> uname -a` now works. The quickest way to start it on every boot
is a line in `/etc/rc.local`:

```sh
/usr/local/sbin/bsdkrun-agent &
```

**As a proper service** — drop in an `rc.d` script (both are in [`packaging/`](./packaging)):

<details>
<summary>FreeBSD — <code>/usr/local/etc/rc.d/bsdkrun_agent</code></summary>

```sh
#!/bin/sh
#
# PROVIDE: bsdkrun_agent
# REQUIRE: NETWORKING
# KEYWORD: shutdown

. /etc/rc.subr

name="bsdkrun_agent"
rcvar="bsdkrun_agent_enable"

load_rc_config $name

: ${bsdkrun_agent_enable:="NO"}
: ${bsdkrun_agent_program:="/usr/local/sbin/bsdkrun-agent"}

pidfile="/var/run/${name}.pid"
# daemon(8): -f background, -P track pid, -r restart the agent if it exits.
command="/usr/sbin/daemon"
command_args="-f -P ${pidfile} -r ${bsdkrun_agent_program}"

run_rc_command "$1"
```

```sh
chmod +x /usr/local/etc/rc.d/bsdkrun_agent
sysrc bsdkrun_agent_enable=YES
service bsdkrun_agent start
```
</details>

<details>
<summary>NetBSD — <code>/etc/rc.d/bsdkrun_agent</code> (no <code>daemon(8)</code>, so we track the pid ourselves)</summary>

```sh
#!/bin/sh
#
# PROVIDE: bsdkrun_agent
# REQUIRE: NETWORKING

. /etc/rc.subr

name="bsdkrun_agent"
rcvar=$name
command="/usr/local/sbin/bsdkrun-agent"
pidfile="/var/run/${name}.pid"

start_cmd="agent_start"
stop_cmd="agent_stop"

agent_start()
{
	echo "Starting ${name}."
	${command} &
	echo $! > ${pidfile}
}

agent_stop()
{
	if [ -f ${pidfile} ]; then
		kill "$(cat ${pidfile})" 2>/dev/null && rm -f ${pidfile}
	fi
}

load_rc_config $name
run_rc_command "$1"
```

```sh
chmod +x /etc/rc.d/bsdkrun_agent
echo 'bsdkrun_agent=YES' >> /etc/rc.conf
/etc/rc.d/bsdkrun_agent start
```
</details>

If `exec` times out, check inside the guest that networking is up (`ifconfig` shows
`192.168.127.2`) and the agent is listening (`netstat -an | grep 1024`).

> The FreeBSD binary is dynamically linked for FreeBSD 14+; the NetBSD binary is built natively on
> NetBSD 10.

---

## AI coding agents in sandboxes

`bsdkrun claude` boots a microVM, shares the directory you ran it in, and drops
you into Claude Code. The agent can run anything it likes in there — install
packages, start containers, delete files — and it cannot reach the rest of your
machine.

```sh
cd ~/code/my-app
bsdkrun claude                  # or codex / gemini / opencode / crush / copilot / kilo / qwen

bsdkrun ai agents               # who's available, and whether each is installed
bsdkrun ai ls                   # sandboxes, grouped by project
bsdkrun claude --new --name review   # a second session, same saved login
bsdkrun claude --repo https://github.com/owner/api   # clone instead of sharing
bsdkrun claude --no-workspace   # share nothing at all
bsdkrun ai rm claude            # remove its sandboxes and saved login
```

**Three kinds of state, split on purpose**, because they have different
lifetimes and different blast radii:

| State | Lives in | Why there |
| ----- | -------- | --------- |
| The agent's login | a per-agent volume mounted at `$HOME` | log in once per agent, not once per session — `--new` starts a second sandbox against the same login |
| Skills | one host directory, mounted into **every** sandbox | a skill installed once is visible to every agent, in both directions |
| Your code | nothing, unless you say so | the sandbox is the product; access is a deliberate act |

**Skills are shared.** `~/.agents/skills` — the cross-agent convention — is
mounted into every sandbox, and each agent's own skills path is symlinked at
it. Install a skill on the host and Claude, Codex and Gemini all see it;
install one *from inside* a sandbox and it appears on the host.

**Each sandbox comes with a toolchain**: git, Docker (its own daemon, started
for you) and Determinate Nix. An agent that has to install Docker before it can
run your tests is an agent you wait for.

**Your git identity comes along.** `user.name` and `user.email` are read from
your host config, and `~/.ssh` is mounted **read-only** so `git push` works with
the keys you already use.

> **What that costs.** Read-only stops an agent rewriting your SSH config; it
> does **not** stop it reading a private key. `--no-ssh` opts out, and a sandbox
> without it can still clone over HTTPS. Sharing your whole `$HOME` as a
> workspace is refused outright.

**In the desktop app and the web UI** there's a right-hand panel (⌘J) with the
agent's TUI, an agent dropdown, a folder picker, a git-clone button, and a
searchable session switcher grouped by project. Hiding the panel keeps the
session running.

**Where paths resolve.** `--workspace` names a directory on the machine running
the *engine*. Driving a remote `bsdkrund`, that's the VPS — your laptop's
filesystem isn't reachable from it, so use `--repo` (the clone happens inside
the sandbox) or a path on that host.

> The first launch of an agent installs its toolchain. bsdkrun pulls a prebuilt
> image from `ghcr.io/tsirysndr` when one exists, and otherwise provisions a VM
> once and caches it — the CLI streams the build, the UIs show it in a progress
> log.

---

## Docker — replace Docker Desktop

`bsdkrun docker` runs a Docker engine in a microVM and serves its API on a host
unix socket, so **your own `docker` CLI drives it**. Not a wrapper and not a
different CLI: `docker`, `docker compose` and `docker buildx` work exactly as
they did, pointed at a bsdkrun VM instead of Docker Desktop.

```sh
bsdkrun docker start          # boots the engine, wires the socket + a docker context
docker run -d -p 8080:80 nginx
curl localhost:8080           # the container's port, published on the host
docker compose up -d          # compose and buildx come along for free

bsdkrun docker ps             # containers, without leaving bsdkrun
bsdkrun docker status         # engine version, socket, image store, shares
bsdkrun docker stop           # images and containers stay on its disk
```

There is exactly **one** engine VM, always named `bsdkrun-docker`: `start` on an
existing one resumes it rather than building a second. Its images and containers
live on a persistent volume, so a `stop`/`start` cycle keeps everything.

Three things have to be true for a VM-backed Docker to actually feel like
Docker Desktop, and each is handled rather than documented away:

| What breaks in a naive VM setup | What bsdkrun does |
| ------------------------------- | ----------------- |
| `docker` can't find a daemon    | dockerd's API is served on `<state>/docker/docker.sock`, and a `bsdkrun` **docker context** points at it — no `DOCKER_HOST` needed |
| `-p 8080:80` publishes *inside* the VM | a watcher follows Docker's event stream and mirrors every published port onto the host, withdrawing it when the container stops |
| `-v $PWD:/app` mounts an **empty** dir | `$HOME` is shared into the VM **at the same path** (`--mount` adds more, `--no-home` opts out) |

**Coming from Docker Desktop.** Quit Desktop, run `bsdkrun docker start`, and
carry on — the context switch is the whole migration. If a tool hardcodes
`/var/run/docker.sock` (testcontainers, some CI scripts), add `--system-socket`
to point that path here too; it asks for sudo once.

**The image store.** By default `/var/lib/docker` lives on the VM's host-backed
rootfs, which has no fixed size. `--disk-size` gives it a dedicated sparse ext4
disk instead, and `docker disk` grows it:

```sh
bsdkrun docker start --disk-size 60G   # only applies when the VM is created
bsdkrun docker disk --size 100G        # grow it later
bsdkrun docker disk                    # what it is now
```

A grown disk reaches a *running* guest only after a restart — virtio-blk fixes
a device's size when the VM attaches it — so `disk --size` says so rather than
pretending otherwise.

The engine is a bsdkrun machine like any other: `bsdkrun logs bsdkrun-docker`
follows its console, `bsdkrun docker shell` opens a shell *in the VM* (not in a
container), and it appears in `ps`, in the TUI, and in the desktop and web UIs —
which also grow a **Containers** view with start/stop/restart/remove and logs.
Every SDK exposes the same surface (`docker_status`, `docker_containers`,
`docker_start`, …), as do the daemon's GraphQL and gRPC APIs.

> **Security.** The API port is loopback-only, but it carries no TLS and Docker
> API access is root-equivalent *inside the guest* — any local process can use
> it, exactly as with colima and friends. The unix socket itself is `0600`. The
> blast radius is the VM, not your host, but it is not a multi-user boundary.

---

## CI — spindle workflows in microVMs

`bsdkrun ci` runs [tangled](https://tangled.org) spindle workflows — the
`.tangled/workflows/*.yml` a knot runs on push — **locally, in real microVMs**,
from one command:

```sh
bsdkrun ci run                  # every workflow matching a manual trigger
bsdkrun ci run test             # just this one (naming skips `when:` matching)
bsdkrun ci run --event push     # simulate a push of HEAD to the current branch
bsdkrun ci ls                   # workflows, and whether each would match
bsdkrun ci serve                # a spindle-compatible runner over HTTP
```

Each workflow gets its own VM, built from its `dependencies:` as a
[nixery.dev](https://nixery.dev) image, with the repository's **HEAD commit**
cloned in (never the dirty tree — commit first) and the full spindle
environment (`CI=true`, `TANGLED_*`). The schema and `when:` matching are
tangled's own Go package, imported rather than reimplemented, so a file that
passes here is a file spindle runs the same way. `--keep` leaves a failed
workflow's VM around for `bsdkrun shell`; `--json` emits spindle's LogLine
stream.

`bsdkrun ci serve` accepts `sh.tangled.pipeline` records over plain HTTP —
the runner half of spindle, deployable on a server, with the rest (jetstream,
XRPC, secrets) left to spindle or tack.

Workflows can also be **defined in code** through any of the nine SDKs and run
without a YAML file ever touching the repository:

```go
bsdkrun.Workflow("test").OnPush("main").Deps("go", "gcc").
    Step("test", "go test ./...").Run()
```

See [`ci/README.md`](ci/README.md) for the full account.

---

## Snapshots, branches & restore

A **snapshot** is a named, point-in-time capture of one machine's disk state, and it is a
**copy-on-write clone** (`clonefile` on APFS, `--reflink` on btrfs/XFS): taking one is instant and
costs no disk until the two sides diverge. That is what makes the other two verbs worth having —
**branch** boots a *new* machine from a snapshot, and **restore** puts a machine back to one.

```sh
bsdkrun snapshot web before-upgrade    # capture web's disk state under a name
bsdkrun snapshot web                   # ...or let it name itself (web-1, web-2, …)
bsdkrun snapshots [web]                # list every snapshot, or just this machine's
bsdkrun branch before-upgrade -d       # boot a NEW machine from that state
bsdkrun branch web -d --name web-test  # branch a MACHINE: snapshot it, then boot the copy
bsdkrun restore web before-upgrade -f  # put web back (stops it first with -f)
bsdkrun rollback web -f                # ...or just undo to its most recent snapshot
bsdkrun snapshot rm before-upgrade     # delete a snapshot and its data
```

What is captured depends on the guest, because "disk state" does:

| Guest              | What a snapshot holds                                             |
| ------------------ | ----------------------------------------------------------------- |
| Linux (OCI)        | the writable rootfs tree the guest serves over virtio-fs           |
| FreeBSD / NetBSD   | the raw root disk holding the whole UFS                            |
| Unikraft           | the unikernel image, its cmdline, and any `--mount`ed host dirs    |

Three things are worth knowing before you rely on it:

- **It is a disk snapshot, not a memory one.** libkrun has no save-VM API, so what comes back is
  what the guest *wrote*, not what it was thinking. A restored machine boots; it does not resume.
- **A BSD guest is powered off to snapshot it.** A mounted UFS cannot be cloned consistently — a
  live copy boots into an `fsck` that discards recent writes — so the guest is cleanly shut down
  first and left stopped. Linux guests are only flushed (`sync`), and keep running.
- **`restore` is undoable.** The state being replaced is itself snapshotted first (free, being a
  clone), so a mistyped restore is one `rollback` away from being reversed. `--no-backup` opts out.

`branch` never boots the snapshot itself: every guest family clones the state into the new machine
first, so branching twice gives two independent machines and leaves the snapshot pristine. Port
forwards are inherited from the snapshot, with any host port that is already taken swapped for a
free one — the machine it came from is usually still running on it.

`restore` and `rollback` leave the machine **stopped**; `bsdkrun start <id>` runs the restored
state. All of this is in the desktop app and the web UI too (a **Snapshots** view, plus per-machine
snapshot / branch / restore buttons), and in every SDK.

> **`snapshot` vs `commit`.** `commit` freezes a machine into a reusable *flavor* — a template you
> boot fresh machines from by name, like `docker commit`. A snapshot belongs to the machine it came
> from: it is a point you can go back to, or fork off. Use `commit` to publish an environment, and
> `snapshot` to keep a safety net.

---

## Flavors — preconfigured environments & snapshots

A **flavor** is a named, ready-to-boot environment. Launch one with a single command; the first run
provisions and caches it, and every later launch clones that cache — so it's instant after the first
build, like `docker build` + `docker run`:

```sh
bsdkrun flavors                     # list the catalog + your snapshots + your flavors.toml entries
bsdkrun flavor run -d node          # boot Node.js 22
bsdkrun flavor run -d claude-code   # boot an AI coding agent (Claude Code) — provisioned once
bsdkrun flavor build laravel        # pre-build a flavor's cache (streams the provisioning)
```

Flavors come from three places:

- **Catalog** — curated, built-in environments across several categories:
  - **languages / runtimes:** `node`, `python` (uv), `php` (composer), `laravel`, `symfony`,
    `elixir`, `phoenix`, `gleam` (nix), `clojure`, `nix`, `docker`
  - **AI coding agents:** `claude-code`, `codex`, `opencode`, `crush`, `copilot`
  - **services:** `postgres`, `mysql`, `redis` · **web:** `nginx`, `apache`, `caddy`
  - **operating systems:** `freebsd`, `netbsd`
- **User** — your own stacks, declared in a `flavors.toml` (`$BSDKRUN_FLAVORS_FILE`, else
  `./bsdkrun.flavors.toml`, else `~/.config/bsdkrun/flavors.toml`):

  ```toml
  [[flavor]]
  name = "my-api"
  base = "node:22"          # an OCI ref, or `freebsd` / `netbsd`
  category = "language"
  ports = ["3000:3000"]
  env = ["NODE_ENV=development"]
  provision = ["apt-get update && apt-get install -y git", "npm install -g pnpm"]
  ```

  Manage them from the CLI too: `bsdkrun flavor add my-api --base node:22 --provision "npm i -g pnpm"`
  and `bsdkrun flavor rm my-api`.
- **Snapshots** — freeze a running (or stopped) machine's current state into a reusable flavor, like
  `docker commit`. Boot FreeBSD, install your tools, then capture it:

  ```sh
  id=$(bsdkrun freebsd -d)
  bsdkrun exec $id pkg install -y git tmux vim
  bsdkrun commit $id my-freebsd-dev --description "FreeBSD with my toolchain"
  bsdkrun flavor run -d my-freebsd-dev            # clone that exact state into a fresh machine
  ```

  Snapshots are CoW clones (rootfs for Linux, the raw disk for BSD), so they're cheap to store and
  boot.

**Build methods** are shown per flavor so you know how it's provisioned: **docker** (a plain OCI
image), **nix** (packages via the Determinate Systems installer), or **system** (packages/shell
provisioning in the guest). Provisioning steps compose with `-v` volumes, `--port`, and `--repo`.

---

## Networking

`firmware`, `kernel`, and `linux` all give the guest **internet access by default** — no flags
needed. libkrun's built-in TSI backend only works for Linux guests via an in-guest shim, so we
instead give every guest a real `virtio-net` NIC wired to [**gvproxy**](https://github.com/containers/gvisor-tap-vsock),
a userspace network stack that NATs the guest out to your host's network. The guest DHCPs an
address on `192.168.127.0/24` (gateway `.1`, guest `.2`) with working DNS.

Install gvproxy once:

```sh
brew install gvproxy
```

If gvproxy isn't installed, bsdkrun prints a warning and boots the guest **without** a NIC (unless
you asked for `--port`, which then hard-errors). Disable networking explicitly with `--no-net`.

Each VM gets its **own** gvproxy instance and its own isolated network, so you can run several
guests at once. gvproxy is torn down automatically when the VM exits — including when you interrupt
it (Ctrl-C / `kill`), via a signal handler that also restores your terminal.

### Global networks — reach machines by name

By default each machine is isolated. Opt in to a **shared network** so machines share one subnet
and reach each other **by IP and by name** (docker-compose style), with internal DNS:

```sh
bsdkrun network create devnet                     # one shared gvproxy switch
bsdkrun linux -d --network devnet --name db  postgres
bsdkrun linux -d --network devnet --name api myapi
bsdkrun exec api -- ping db                        # resolves db → 192.168.127.x
```

Members get distinct IPs on `192.168.127.0/24` and a DNS name of their `--name` (defaults to a
generated one). Manage networks and membership:

```sh
bsdkrun network ls                      # networks + running/total members
bsdkrun network connect <machine> devnet   # join/switch an existing machine (applies on next start)
bsdkrun network disconnect <machine>       # back to isolated (applies on next start)
bsdkrun network sync devnet                # refresh members' /etc/hosts (fixes name lookup)
bsdkrun network rm [-f] devnet
```

Membership is editable after creation and re-applied on `start`. Names resolve on Linux and FreeBSD
via the network's DNS; **NetBSD** resolves via a synced `/etc/hosts` block (its resolver rejects the
DNS's AAAA `NXDOMAIN` for A-only names) — joins auto-sync, and `network sync` refreshes an existing
network without restarting members.

### SSH into the guest

gvproxy forwards a **unique** host port to the guest's SSH (`:22`) for each VM. bsdkrun logs it at
boot:

```
INFO networking up — SSH into the guest with: ssh -p 58851 user@127.0.0.1
```

(The guest must be running `sshd` and permit your login, of course.)

### Forwarding extra ports

`--port HOST:GUEST` (repeatable) forwards a host TCP port into the guest:

```sh
./target/release/bsdkrun firmware \
  --firmware "$(brew --prefix)/share/krunkit/KRUN_EFI.silent.fd" \
  --disk images/fbsd15.raw \
  --port 8080:80 --port 2222:22 \
  --cpus 2 --mem 2048
```

`--mac AA:BB:CC:DD:EE:FF` overrides the guest NIC's MAC (default: a fixed locally-administered one).

### SSH — key-based access in one command

The guest agent also manages sshd on **Linux, FreeBSD and NetBSD** guests. The one-liner
installs your local `~/.ssh/id_*.pub` keys, installs sshd where the OS lacks it (Linux OCI
guests — the BSDs ship it in base), generates host keys, and enables + starts the service:

```sh
bsdkrun ssh <id> setup                      # your local public keys, root login
ssh -p <port> root@127.0.0.1                # port from `bsdkrun ps` / the boot banner

bsdkrun ssh <id> setup --key ~/.ssh/work.pub --user tsiry   # explicit key / other user
bsdkrun ssh <id> add-key --key "ssh-ed25519 AAAA..."        # append a key later
bsdkrun ssh <id> status                     # sshd state + installed key count
```

`--key` takes a literal public key or a local `.pub` file path (the wrapper inlines file
contents before sending). Keys are deduplicated by key material, written with the modes sshd
insists on (`700`/`600`, owned by the target user), and an explicit `PermitRootLogin no` is
relaxed to `prohibit-password` — key-only, never passwords.

### Tailscale — put a guest on your tailnet

The guest agent doubles as a tailscale manager on **Linux, FreeBSD and NetBSD** guests — one
command installs tailscale the OS-native way, starts `tailscaled`, and joins the tailnet:

```sh
# one-shot: install + start + `tailscale up` (get an auth key from the admin console)
bsdkrun tailscale <id> setup --authkey tskey-auth-...
# or keep the key out of shell history / ps:
TS_AUTHKEY=tskey-auth-... bsdkrun tailscale <id> setup

bsdkrun tailscale <id> status               # who am I / peers
bsdkrun tailscale <id> install              # just install
bsdkrun tailscale <id> start                # just start tailscaled
```

> **NetBSD images ship tailscale pre-baked.** bsdkrun's bundled NetBSD images (both **amd64** and
> **arm64**) now carry the `tailscale`/`tailscaled` binaries plus an `/etc/rc.d/tailscaled` service
> enabled with `--tun=userspace-networking`, so `tailscaled` is **already running at boot** — no
> `install`/`start` needed. Go straight to `bsdkrun tailscale <id> status` (shows `Logged out.` on a
> fresh guest) or join a tailnet with `bsdkrun tailscale <id> setup --authkey …`. The two static Go
> binaries come from pkgsrc and carry no extra dependencies (they link only base `libc`). Other
> guests (Linux OCI, FreeBSD) still install on demand via the OS-native paths below.

`bsdkrun tailscale` finds the in-guest agent binary itself; the equivalent explicit form is
`bsdkrun exec <id> /usr/local/sbin/bsdkrun-agent tailscale ...` (Linux OCI guests carry the
agent at `/sbin/bsdkrun-agent`).

Install goes through each OS's native channel: `apk` (Alpine) or the official static tarball on
Linux, `pkg install` on FreeBSD, `pkg_add` from the pkgsrc CDN on NetBSD (a no-op on the bundled
NetBSD images, where it's already baked in). `tailscaled` runs with
`--tun=userspace-networking` by default — the microVM kernels bsdkrun boots (Linux microvm,
NetBSD `MICROVM`, FreeBSD `FIRECRACKER`) generally lack tun/tap, and userspace mode still makes
the guest **reachable over the tailnet** (ssh, agent port, anything listening). Pass
`--kernel-tun` to `start`/`setup` to use a real TUN device where the kernel has one
(e.g. full Linux kernels with `/dev/net/tun` — detected automatically there).

---

## Machine domains — local DNS + HTTPS

Give every machine a real, browser-trusted URL on this host:

```sh
bsdkrun linux nginx:alpine --port 18080:80 -d --name web
bsdkrun domains enable          # prompts once to wire the resolver + trust the CA
curl https://web.bsdk           # no -k, no warnings
```

`enable` starts three cooperating pieces and wires them up — all idempotent, so
re-running it repairs whatever is missing:

| Piece         | What it does                                                                                                          |
| ------------- | --------------------------------------------------------------------------------------------------------------------- |
| DNS responder | Built into bsdkrun; answers `*.bsdk` with `127.0.0.1` on a loopback port (5343). No dnsmasq needed.                     |
| Resolver      | `/etc/resolver/bsdk` on macOS, a systemd-resolved drop-in on Linux — only that TLD is routed to bsdkrun.                |
| Caddy         | Found on `PATH` (`brew install caddy` / `apt install caddy`, or `$BSDKRUN_CADDY`); terminates TLS with its local CA and reverse-proxies to each machine's first `--port` forward. |

Guests live behind gvproxy's userspace NAT, so a domain routes to a **forwarded
port** — a machine without `--port` has nothing to route to, and `bsdkrun
domains ls` says so. Machine names become DNS labels (`tidy_turing` →
`tidy-turing.bsdk`); stopped machines keep their domain and serve Caddy's 502,
which beats NXDOMAIN as a diagnostic. New machines join automatically on boot.

`domains status` health-checks every piece (DNS, resolver, Caddy, CA trust);
`domains disable --purge` removes the resolver wiring, un-trusts the CA and
deletes the proxy state. Default TLD is `bsdk`; pick another with `enable
--tld`, and non-443 ports with `--https-port`/`--http-port` (Linux gets a
one-time `net.ipv4.ip_unprivileged_port_start=80` sysctl offer for the low
ports). Which machine maps to which URL and upstream port is `domains ls`.

### Local certificates

Caddy runs its own **local CA** and mints a short-lived leaf per domain; the
root and its key live under `<state>/proxy/data/pki/authorities/local/`
(isolated from any system Caddy, and deleted by `disable --purge`).
`bsdkrun domains ca` prints the root's path, `--pem` the certificate itself.

Trust is installed into the **system trust store** once, on `enable`:

- **macOS** — `sudo security add-trusted-cert -d -r trustRoot -k
  /Library/Keychains/System.keychain <root>`, run through an **interactive
  sudo** prompt in your terminal. It has to be interactive: `add-trusted-cert`
  sets the trust *settings* through an API that a sessionless root (an
  osascript "with administrator privileges" shell, or a GUI-launched process)
  cannot satisfy — it fails with *"authorization was denied since no user
  interaction was possible,"* leaving the cert present but untrusted. So run
  `bsdkrun domains enable` from a real terminal.
- **Linux** — `caddy trust`, which drives both the system store and the NSS
  store browsers use (`libnss3-tools` provides the `certutil` it needs).

`domains status`'s **ca trust** line checks *actual* trust settings
(`security dump-trust-settings -d` / `openssl verify`), not mere presence — a
half-installed cert reads as `NOT OK`, and re-running `enable` repairs it.

**Tools that verify against their own bundle, not the system store.** Browsers
and system `curl` trust the CA after `enable`. Toolchains that ship a bundled
CA list — Python (`requests`/HTTPie, via certifi), Node, Go — never consult the
keychain, so they reject the cert. Point them at the CA:

```sh
http --verify "$(bsdkrun domains ca)" https://web.bsdk       # HTTPie / requests, per call
bsdkrun domains ca --pem >> "$(python3 -m certifi)"          # append to certifi (persists, keeps public roots)
export REQUESTS_CA_BUNDLE="$(bsdkrun domains ca)"            # requests-based tools*
export NODE_EXTRA_CA_CERTS="$(bsdkrun domains ca)"          # Node
```

\*`REQUESTS_CA_BUNDLE`/`SSL_CERT_FILE` *replace* the bundle, dropping the public
roots for that shell — fine for talking only to `.bsdk`, not a login-shell
default. Appending to certifi keeps the public roots. macOS note: `/usr/bin/curl`
(SecureTransport) reads the keychain; a Homebrew `curl` (OpenSSL) does not, so
verify with the system one or point it at `--cacert "$(bsdkrun domains ca)"`.

---

## TUI — the terminal dashboard

```sh
bsdkrun tui
```

Machines, images, volumes and networks as live panels, refreshed every 1.5 s on
a background thread. The persistent bottom status line shows the current
selection, the outcome of the last action (with a spinner while one runs), and
the machine-domains health chip (`https ·bsdk ✓` / `domains off`). Press `?`
for the full keybinding list at any time.

| Key             | Action                                                        |
| --------------- | ------------------------------------------------------------- |
| `Tab` / `S-Tab` | Cycle panels (`j`/`k`/arrows move, `g`/`G` first/last)         |
| `/`             | Fuzzy search across every panel (fzf-style), `Enter` jumps     |
| `n`             | New-machine wizard (image, name, port, cpus, mem)              |
| `s` / `x`       | Start / stop the selected machine                              |
| `e`             | Shell into it (suspends the TUI, resumes on exit)              |
| `l`             | Log viewer — backfills, then follows a running machine live    |
| `i` / `Enter`   | Machine settings (vCPU / memory, applied on next start)        |
| `o`             | Open `https://<name>.bsdk` in the browser (needs domains)      |
| `d`             | Remove, with confirmation                                      |
| `r`             | Refresh now                                                    |
| `?`             | Show every keybinding                                          |
| `q` / `Ctrl-C`  | Quit                                                           |

The `/` search fuzzy-matches across all four panels at once (fzf-quality
ranking, matched characters highlighted); `Enter` jumps focus straight to the
hit. Starting a machine spawns a detached `bsdkrun start` — a boot forks and
becomes the machine, which must never happen inside the TUI's own process —
and stopping runs on a worker thread, so the dashboard stays responsive through
a slow graceful BSD poweroff. The alternate screen is restored on quit, on a
panic, and on `SIGTERM`/`SIGHUP`.

---

## Disks

The root disk is attached read-write as `virtio-blk` (`--disk`). Attach **additional** disks with
`--attach-disk` (repeatable); append `:ro` for a read-only attachment:

```sh
./target/release/bsdkrun firmware \
  --firmware "$(brew --prefix)/share/krunkit/KRUN_EFI.silent.fd" \
  --disk images/fbsd15.raw \
  --attach-disk images/data.raw \
  --attach-disk images/blobs.raw:ro
```

`--attach-disk` works on the BSD guests (`freebsd`/`netbsd`/`firmware`/`kernel`) and on `linux`
guests alike. Extra disks appear in the guest as the next `virtio-blk` devices in the order given
(e.g. FreeBSD `vtbd1`, `vtbd2`…). A `linux` guest's rootfs is virtio-fs — there is no root block
device — so its first attached disk is `/dev/vda`, the next `/dev/vdb`, and so on. Create a blank one with
`truncate -s 8G data.raw` (then partition/newfs it in the guest), or grow an existing image with
[`bsdkrun grow`](#resizing-the-disk).

---

## Console: how output reaches your terminal

This is the single most confusing thing about booting BSD under libkrun, so it gets its own
section.

On aarch64/macOS, libkrun creates an **implicit console** that is *not* the legacy **PL011 UART
(`ttyS0`, MMIO `0xa001000`)** that the EDK2 firmware and BSD EFI loaders actually write to. The
firmware banner, the loader menu, and the early kernel console all go to that PL011 — so with the
default implicit console you see **nothing**, even though the guest is booting fine.

bsdkrun fixes this for you: before boot it calls `krun_disable_implicit_console()` and then
`krun_add_serial_console_default(input, stdout)`, so the explicit serial console lands on `ttyS0`
(the PL011 the firmware drives) and is wired to this process's stdout. See
`Ctx::attach_stdio_serial_console` in `src/krun.rs`.

**The console input fd must be *pollable*.** libkrun registers the console input fd with `kqueue`,
which rejects non-pollable fds — a regular file or `/dev/null`. Handing it `stdin` blindly would
therefore **abort the whole process** (`epoll.rs: assertion left == right failed, left: -1`, seen
as SIGABRT or SIGSEGV) whenever stdin isn't a terminal: output redirected to a file, run
non-interactively, or launched from a shell's `!`/background. To avoid that, bsdkrun uses `stdin`
as the console input **only when it's a TTY**; otherwise it substitutes the read end of an
internal pipe — a pollable fd that just never delivers input. So:

- **Interactive terminal** — `bsdkrun firmware …` drops you straight on the guest console and your
  keystrokes reach it. Just run it; no wrapping needed.
- **Non-interactive / captured** — `bsdkrun firmware … > boot.log 2>&1` works too; you simply
  can't type into the guest (there's no terminal to type from). Use `--log-level 0` to keep
  libkrun's own logs out of the capture (see Troubleshooting).

> If you specifically want to capture the boot *and* keep an interactive stdin available, the old
> pipe trick still works: `tail -f /dev/null | bsdkrun firmware … 2>krun.log | cat > boot.log`.

---

## Preparing a guest image

### The easy way — `bsdkrun fetch`

`fetch` downloads a BSD arm64 VM image, decompresses it, prepares it for a serial console, and
links it into `./images` — everything below, automated. It shells out to tools already on macOS
(`curl`, `xz`/`gzip`, `hdiutil`, `diskutil`).

```sh
# FreeBSD (default OS)
bsdkrun versions                    # list releases (14.3, 14.4, 15.0, 15.1, …)
bsdkrun fetch                       # latest release -> ./images/freebsd-<ver>.raw
bsdkrun fetch --version 15.1        # pin a release
bsdkrun fetch --dir /tmp --force    # custom dir, re-download

# NetBSD
bsdkrun versions --os netbsd        # list builds (current + releases)
bsdkrun fetch --os netbsd           # NetBSD-current -> ./images/netbsd-current.img

# then boot what it printed:
bsdkrun firmware --firmware images/KRUN_EFI.fd --disk images/freebsd-15.1.raw --cpus 2 --mem 2048
```

With no `--version`, FreeBSD resolves the newest release and NetBSD uses **`current`** (see the
NetBSD note below). Downloads are a few hundred MiB and expand to a couple GiB.

Downloaded images are cached under **`~/.cache/bsdkrun/`** (override with `BSDKRUN_CACHE`, or
`XDG_CACHE_HOME`), so fetching a version you already have is instant — it just links the cached
image into `--dir` (a hard link, no second copy). Use `--force` to re-download.

### NetBSD: version handling is arch-specific

`bsdkrun netbsd` **direct-boots** the kernel (no firmware), rooting on the virtio-blk disk (override
the kernel command line with `$BSDKRUN_NETBSD_CMDLINE`). The details differ by host arch:

- **arm64** ✅ downloads bsdkrun's bundled evbarm image (the live `gzimg` with the agent **and
  tailscale** injected) + the evbarm `GENERIC64` kernel from the NetBSD CDN, rooting on the GPT
  wedge `dk1`. libkrun exposes **modern (v2) virtio-mmio**, and NetBSD's driver only gained v2
  support in **-current** (post-10.x): the bundled image is `current`, so `--version` only affects
  the kernel — pinning a **release** (≤ 10.1) kernel prints `virtio: unknown version 0x02; giving
  up`.
- **amd64** ✅ boots via **PVH** — but it needs a **PVH-capable libkrun**:
  [tsirysndr/libkrun `feat/pvh-boot`](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot)
  (stock libkrun only speaks the Linux boot protocol, under which any NetBSD kernel triple-faults
  instantly). bsdkrun downloads its bundled FFS rootfs + the NetBSD `MICROVM` kernel (a PVH ELF),
  sets `KRUN_PVH=1` so libkrun enters via the kernel's `PHYS32_ENTRY` note, and boots with
  `root=ld0a console=com` (`console=com` matters: a PVH boot passes no bootinfo, so without it
  NetBSD's console defaults to nonexistent VGA and all output vanishes). The agent **and tailscale**
  are baked into the image, so `exec`/`shell` (and a boot-time `tailscaled`) work out of the box.
  Against stock libkrun the boot triple-faults
  (`KRUN_PVH` is simply ignored) — build the fork until PVH lands upstream; see the
  [KVM e2e](.github/workflows/e2e-linux.yml), which builds the fork and boots it on every run.

### Resizing the disk

The stock images are small (NetBSD's root is ~1.7 GB) — you'll hit **`no space left on device`**
quickly. `grow` enlarges a raw image; NetBSD then expands its root filesystem to fill the new space
automatically on the next boot:

```sh
bsdkrun grow --disk images/netbsd-current.img --size 8G
bsdkrun firmware --firmware images/KRUN_EFI.fd --disk images/netbsd-current.img --cpus 2 --mem 2048
# NetBSD's resize_root grows the GPT partition + ffs on boot; root becomes ~7.8 GB.
```

`grow` only enlarges (never shrinks). Note it follows hard links, so growing a `fetch`-linked image
also grows the cached copy — that's usually fine (the file is sparse). **FreeBSD** images won't
auto-grow this way (their UFS root is followed by the swap partition, so the trailing space isn't
adjacent to root).

### The manual way

FreeBSD publishes raw arm64 disk images directly:

```sh
V=15.1
base=https://download.freebsd.org/releases/VM-IMAGES/$V-RELEASE/aarch64/Latest
curl -Lo images/fbsd.raw.xz "$base/FreeBSD-$V-RELEASE-arm64-aarch64-ufs.raw.xz"
xz -d images/fbsd.raw.xz          # -> images/fbsd.raw (several GB)
```

A valid image is **GPT** with an **EFI System Partition** (type GUID `C12A7328-…`) plus the
FreeBSD UFS root (`516E7CB5-…`). You can sanity-check the partition table with `xxd`/`gpt`/
`hdiutil` before booting. Then set the console via the ESP as described next (or just run
`bsdkrun fetch` on the same version, which does it for you).

### Point FreeBSD's console at the serial (via the ESP)

FreeBSD's `/boot/loader.conf` lives on the **UFS root**, which macOS can't write. Fortunately the
FreeBSD EFI loader also reads **`/efi/freebsd/loader.env`** from the **ESP** *before* it mounts
UFS — exactly where you can set the console early. macOS *can* mount the FAT ESP, so:

```sh
# attach the raw image and mount only the FAT ESP (disk?s1)
DEV=$(hdiutil attach -imagekey diskimage-class=CRawDiskImage -nomount images/fbsd15.raw \
      | awk '/EFI/{print $1}')
diskutil mount -mountPoint /Volumes/ESP "$DEV"

mkdir -p /Volumes/ESP/EFI/freebsd
cat > /Volumes/ESP/EFI/freebsd/loader.env <<'ENV'
console=efi,eficom
boot_serial=YES
boot_multicons=YES
loader_color=NO      # optional — see note below
ENV

diskutil unmount /Volumes/ESP
hdiutil detach "${DEV%s*}"
```

> **`loader.env` gotchas:** values are **unquoted** (`console=efi`, *not* `console="efi"` — the
> quotes are taken literally and you get `no valid consoles!`). Valid arm64 console names are
> `efi` and `eficom`; the old `comconsole` name is deprecated. Delete the AppleDouble junk macOS
> sprinkles on the FAT volume (`dot_clean /Volumes/ESP`) before unmounting.
>
> **Washed-out / gray console background?** The loader's boot menu paints the screen with ANSI
> black (`ESC[40m`) for its color scheme, which clashes with a terminal whose background isn't
> pure black — it reads as a gray filter over the console. `loader_color=NO` disables the loader's
> colors; the beastie logo and menu still render, just in your terminal's own colors.

---

## SDKs

Drive `bsdkrun` from your own code. Each SDK is a thin, stateless wrapper around the
binary — it builds argv, shells out, and parses the JSON output. There is no daemon and no
long-lived state, so the SDKs are safe to use from short-lived processes and scripts.

| Language       | Package        | Source                             | Notes                                                                                                    |
| -------------- | -------------- | ---------------------------------- | -------------------------------------------------------------------------------------------------------- |
| **TypeScript** | `@bsdkrun/sdk` | [`sdk/typescript`](sdk/typescript) | Node / Deno / Bun. Adds a `sh` template tag, `Terminal`, and direct agent-protocol access.                  |
| **Python**     | `bsdkrun`      | [`sdk/python`](sdk/python)         | No runtime dependencies. Developed with [uv](https://docs.astral.sh/uv/); type-checked under strict mypy.   |
| **Ruby**       | `bsdkrun`      | [`sdk/ruby`](sdk/ruby)             | No runtime dependencies.                                                                                   |
| **Elixir**     | `bsdkrun_ex`   | [`sdk/elixir`](sdk/elixir)         | One dependency (`:jason`). `{:ok, _}` / `{:error, _}` with bang variants. Modules are plain `Bsdkrun.*`.    |
| **Gleam**      | `bsdkrun`      | [`sdk/gleam`](sdk/gleam)           | Erlang target. Fully typed `Result`s; no exceptions.                                                       |
| **Clojure**    | `io.github.tsirysndr/bsdkrun` | [`sdk/clojure`](sdk/clojure) | No runtime dependencies beyond `org.clojure/clojure` + `data.json`. A "sandbox" is a plain map — no object hierarchy. Docs on [cljdoc.org](https://cljdoc.org/d/io.github.tsirysndr/bsdkrun). |
| **Go**         | `github.com/tsirysndr/bsdkrun/sdk/go` | [`sdk/go`](sdk/go) | Zero third-party dependencies — stdlib only, hand-rolled `graphql-transport-ws`. Fluent builders ending in `(T, error)`. |
| **Rust**       | `bsdkrun-sdk`  | [`sdk/rust`](sdk/rust)             | Blocking, no async runtime. Fluent consuming builders; standalone crate outside the workspace.             |
| **Scala**      | `io.github.tsirysndr::bsdkrun` | [`sdk/scala`](sdk/scala) | Scala 3, blocking, `Either[BsdkrunError, A]` throughout. One dependency (upickle) — HTTP and WebSocket come from `java.net.http`. Toolchain pinned with mise. |

Elixir publishes as **`bsdkrun_ex`** because Hex is a single namespace and the Gleam SDK
already takes `bsdkrun` there; its modules are unaffected.

All of them find the binary the same way — an explicit override, then `$BSDKRUN_BIN`, then
`bsdkrun` on `$PATH`, then an in-repo dev build — and expose the same surface: create /
`exec` / logs / lifecycle on a machine, plus the `images`, `volumes`, `networks`, and
`system` namespaces.

```ts
// TypeScript
const box = await Sandbox.create({ os: "linux", image: "alpine" });
await box.exec(["uname", "-a"]);
```
```python
# Python
box = Sandbox.create(os="linux", image="alpine")
box.exec(["uname", "-a"])
```
```ruby
# Ruby
box = Bsdkrun::Sandbox.create(os: "linux", image: "alpine")
box.exec(["uname", "-a"])
```
```elixir
# Elixir
{:ok, box} = Bsdkrun.create(os: :linux, image: "alpine")
{:ok, res} = Bsdkrun.exec(box, ["uname", "-a"])
```
```gleam
// Gleam
let assert Ok(box) = bsdkrun.create(args.linux("alpine"))
let assert Ok(res) = bsdkrun.exec(box, ["uname", "-a"])
```
```clojure
;; Clojure
(require '[bsdkrun.sandbox :as sandbox])
(def box (sandbox/create! {:os "linux" :image "alpine"}))
(sandbox/exec! box ["uname" "-a"])
```
```go
// Go
box, err := bsdkrun.Linux("alpine").Create()
res, err := box.Exec("uname", "-a")
```
```rust
// Rust
let sandbox = Sandbox::linux("alpine").create()?;
let res = sandbox.exec(["uname", "-a"])?;
```

### Try it interactively

The REPL-native SDKs ship a console with the binary resolved and the API already in scope,
so you can poke at real machines without writing a script:

```sh
cd sdk/python  && uv run console.py    # IPython
cd sdk/ruby    && bin/console          # IRB
cd sdk/elixir  && iex -S mix           # IEx, via .iex.exs
cd sdk/clojure && clj -M:rebel         # rebel-readline, via dev/user.clj
```

All four define `ps` (every machine, exited ones included) and accept a
`--bin path/to/bsdkrun` override (`BSDKRUN_BIN=…` for IEx and rebel) to drive a locally built
binary.

See each SDK's README for the full API. The TypeScript SDK is covered by an
[end-to-end CI job](.github/workflows/e2e-sdk.yml) that boots a real microVM under KVM; the
others run [unit + argv tests](.github/workflows/sdk-unit.yml) on every change.

---

## Project layout

| Path                   | What it is |
|------------------------|------------|
| `src/krun.rs`          | Safe Rust FFI bindings to the libkrun C ABI (`krun_create_ctx`, `krun_set_vm_config`, `krun_add_disk`, `krun_set_kernel`, `krun_set_firmware`, `krun_set_root` / `krun_set_exec` for virtio-fs rootfs, `krun_add_net_unixgram` for gvproxy networking, the `krun_disable_implicit_console` / `krun_add_serial_console_default` console wiring, `krun_start_enter`, …). Negative returns are decoded as `-errno`. |
| `src/main.rs`          | `clap` CLI with the `probe` / `kernel` / `firmware` / `linux` boot subcommands and the `ps` / `images` / `stop` / `logs` / `shell` management subcommands. |
| `src/oci.rs`           | Minimal OCI registry client: pulls a `linux/arm64` image (any v2 registry) with `curl`, extracts layers with `tar` (applying whiteouts), and caches the rootfs content-addressed by digest. |
| `src/db.rs`            | State persistence in SQLite (`sqlx` over a small Tokio runtime): machines, images, and disks, each with a short id; plus the state-dir layout. |
| `src/console.rs`       | Detached-machine console broker: wires the guest console to a PTY and fans it out to `console.log` + a `console.sock` that `logs`/`shell` clients attach to. |
| `src/id.rs`            | Docker-style short ids (12 hex chars from `/dev/urandom`). |
| `src/linux.rs`         | The `linux` subcommand: fetches/converts the kernel, resolves the entrypoint, and builds the initramfs (generated `/init`) or wires the virtio-fs root. |
| `src/elf.rs`           | Flattens an aarch64 `vmlinux` ELF into a raw arm64 `Image` (what libkrun's loader wants) — pure Rust, no binutils. |
| `src/net.rs`           | User-mode networking: spawns and drives a per-VM gvproxy (unique host ssh-port, host→guest port forwards over its HTTP control socket), reaped on exit. |
| `src/tty.rs`           | Saves stdin's terminal state before libkrun raws it and restores it on every exit path (clean, error, or signal); the signal handler also tears down gvproxy. |
| `src/watchdog.rs`      | Tees libkrun's stderr to catch its HVF panic-hang on BSD SMP shutdown and exit cleanly. |
| `build.rs`             | Finds libkrun via Homebrew and configures linking + rpath. |
| `Makefile`             | Build **and codesign** (re-signs after every build — mandatory on macOS). |
| `bsdkrun.entitlements` | `com.apple.security.hypervisor` + library-validation opt-out. |
| `images/`              | Guest disk images and a symlink to libkrun's EDK2 firmware (git-ignored blobs). |
| `skills/`              | Agent skills published to [skills.sh](https://skills.sh/tsirysndr/bsdkrun) — `skills/bsdkrun-cli/` documents every subcommand and flag for coding agents. |
| `sdk/`                 | Client [SDKs](#sdks) — TypeScript, Python, Ruby, Elixir, Gleam, Clojure, Go, and Rust. Each builds argv, shells out to the binary, and parses its JSON output. |
| `tools/console/`       | Contributor tooling: a Clojure/Babashka REPL centralizing every build/test/publish command in the monorepo. See [Monorepo dev console](#monorepo-dev-console-console). |
| `console`              | Root shortcut: `./console` == `cd tools/console && clj -M:rebel`. |
| `doc/cljdoc.edn`       | [cljdoc.org](https://cljdoc.org) doc-tree config for the Clojure SDK (`sdk/clojure`), published to Clojars as `io.github.tsirysndr/bsdkrun`. |

---

## Agent skill

The full CLI reference is packaged as an [agent skill](https://skills.sh/tsirysndr/bsdkrun) so
coding agents (Claude Code, Cursor, Codex, …) can drive `bsdkrun` correctly. Install it with:

```sh
npx skills add tsirysndr/bsdkrun
```

It lives in [`skills/bsdkrun-cli/`](skills/bsdkrun-cli/): `SKILL.md` is the command map, and
`references/cli-reference.md` has the exhaustive flag list.

---

## Troubleshooting

**`krun_start_enter failed: Invalid argument (errno 22)`**
The binary isn't signed with the hypervisor entitlement. Run `make sign` (or `make build`) and
try again. Remember a bare `cargo build` re-strips the signature.

**Boot floods the terminal with `rng: … Spurious event received`**
That's libkrun's own WARN-level logging, not the guest. Run with `--log-level 0` to silence
libkrun's internal logs; the guest console still comes through. (At high verbosity this can
generate gigabytes of logs very quickly, so keep it low unless you're debugging libkrun itself.)

**Segfault on boot (`x16 == 0` NULL call in the crash report) — but `probe` works fine**
DYLD is loading an **old libkrun** that predates the console symbols bsdkrun needs
(`krun_add_serial_console_default` / `krun_disable_implicit_console`); the missing symbol becomes a
NULL stub and calling it segfaults. `probe` survives because it never calls them. The usual cause
is a `DYLD_LIBRARY_PATH` in your shell rc pointing at a stale copy — commonly `~/.local/lib`.
Diagnose and fix:

```sh
grep -ri DYLD ~/.zshrc ~/.zprofile ~/.profile          # find the override
nm -gU ~/.local/lib/libkrun.1.dylib | grep serial_console   # empty => too old

# quick check / workaround: run without the override
env -u DYLD_LIBRARY_PATH ./target/release/bsdkrun firmware --firmware images/KRUN_EFI.fd --disk images/fbsd15.raw
```

Permanent fix: update or remove the stale `~/.local/lib/libkrun*` so dyld falls through to
Homebrew's. Current bsdkrun detects this at runtime and prints an actionable error instead of
crashing — if you still get a raw segfault, rebuild (`make build && make release`).

**Firmware boots but there's no guest console output**
The guest is almost certainly booting — its output is just going to the PL011 serial that isn't
wired to your terminal. bsdkrun handles this automatically (see [Console](#console-how-output-reaches-your-terminal)),
but if you're driving libkrun yourself you must disable the implicit console and add an explicit
serial console on fds 0/1. To *see* what the firmware is really doing, run at `--log-level 5` and
decode the writes to `krun_devices::legacy::aarch64::serial … write: offset=0, data=[…]` — those
bytes are the guest console.

**Crash on boot — `epoll.rs: assertion left == right failed, left: -1` (SIGABRT/SIGSEGV)**
libkrun tried to register a non-pollable console **input** fd (a regular file or `/dev/null`) with
`kqueue`. Current bsdkrun avoids this automatically (it only uses stdin when it's a TTY, else an
internal pipe — see [Console](#console-how-output-reaches-your-terminal)), so if you hit this you're
on an **old build**: rebuild with `make build`. If you're calling libkrun yourself, never pass a
file/`/dev/null` as the console input fd.

**`no valid consoles!` from the FreeBSD loader**
Your `loader.env` quoted the value (`console="efi"`). Use unquoted `console=efi,eficom`. See
[Preparing a FreeBSD arm64 image](#point-freebsds-console-at-the-serial-via-the-esp).

**Dynamic linker can't find `libkrun.dylib`**
Make sure `brew --prefix libkrun` resolves, or build with an explicit
`LIBKRUN_PREFIX=/opt/homebrew`.

**Guest hangs on `poweroff`/`reboot` with `panicked at src/hvf/src/lib.rs:549: Unexpected val=…`**
A libkrun bug: on an **SMP** guest, shutting down issues PSCI `CPU_OFF` / `AFFINITY_INFO` calls
libkrun's HVF layer doesn't handle, so its vCPU threads panic and `krun_start_enter` never returns.
The guest has already halted cleanly. bsdkrun **detects this and exits cleanly** for you (a watchdog
tees libkrun's stderr and recognises the panic). To avoid the panic entirely, boot with `--cpus 1`.

**Networking: `gvproxy not found on PATH`**
Install it with `brew install gvproxy` (see [Networking](#networking)). Without it the guest boots
with no NIC; `--no-net` silences the warning.

---

## Status

On **macOS Apple Silicon**, both guests boot to a `login:` prompt via the `firmware` subcommand,
through libkrun's EDK2 firmware and the guest's own EFI loader:

- **FreeBSD 15.1 / arm64** — full multi-user rc sequence to `login:`. `fetch` + `firmware`.
- **NetBSD-current / arm64** (evbarm `GENERIC64`) — efiboot → kernel → root-on-ffs → `login:`.
  `fetch --os netbsd` + `firmware`. (NetBSD *releases* ≤ 10.1 boot but can't mount root — their
  virtio-mmio driver is legacy-only; modern v2 support is only in -current.)

On **Linux/amd64 (KVM)**, both guests boot to multi-user via **PVH direct kernel** on the
[PVH libkrun fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot), validated on every
[KVM e2e](.github/workflows/e2e-linux.yml) run (the in-guest agent answers `exec`):

- **NetBSD 10.1 / amd64** — bundled `MICROVM` kernel + FFS rootfs, `root=ld0a console=com`.
- **FreeBSD 15.1 / amd64** — bundled `FIRECRACKER` kernel (no ACPI; MPTable; virtio-mmio + serial
  built in) + UFS rootfs. The fork supplies what the kernel can't discover on its own: an MPTable,
  the TSC frequency (CPUID leaf `0x40000010`), and FreeBSD's numbered `virtio_mmio.device_N=`
  cmdline keys.

The blocker that made guests look dead — console output going to a serial port libkrun wasn't
forwarding — is fixed by bsdkrun's serial-console wiring (see
[Console](#console-how-output-reaches-your-terminal)). Guest-side virtio-mmio device discovery
under libkrun is confirmed working for FreeBSD and NetBSD on both platforms.

**Recent:** global networks with internal DNS ([reach machines by name](#global-networks--reach-machines-by-name),
editable membership, cross-OS incl. NetBSD via synced `/etc/hosts`); data-preserving
[`stop`/`start`](#managing-machines) (resumes the machine's own disk/rootfs, BSD clean-poweroff);
preconfigured [flavors & snapshots](#flavors--preconfigured-environments--snapshots); and a desktop
app (Machines / Images / Volumes / Flavors / Networks).

Next: upstreaming the PVH work to libkrun.

## License

[MIT](./LICENSE) © Tsiry Sandratraina
