# bsdkrun

A Firecracker-style **microVM launcher for BSD and Linux guests on macOS and Linux**, built on
[libkrun](https://github.com/containers/libkrun) (which drives Apple's Hypervisor.framework on
macOS and KVM on Linux).

`bsdkrun` is a thin, purpose-built CLI: it wraps libkrun's C ABI in a handful of safe Rust
bindings and boots a guest three ways — from a **UEFI firmware** image (the guest's own EFI loader
boots a normal disk), from a **direct kernel + FDT** (no bootloader), or straight from an **OCI
image** (`bsdkrun linux alpine` pulls it from any registry, extracts the rootfs, and boots it like
`docker run`). It is deliberately small: one FFI module, one CLI, no daemon.

> **Platforms:** **macOS on Apple Silicon** (Hypervisor.framework) and **Linux on amd64 or arm64**
> (KVM). A hardware-virtualized guest runs the host's CPU arch, so bsdkrun detects the arch and
> pulls the matching kernel, OCI image, and agent automatically. macOS is arm64-only; Linux works
> on both x86_64 and aarch64. **FreeBSD is macOS-only** (its EFI firmware ships only with the
> macOS `libkrun-efi`); **NetBSD** direct-boots its kernel, so it runs on both. _(Linux support is
> new — see the [KVM e2e CI](.github/workflows/e2e-linux.yml).)_

<p align="center">
  <img src=".github/assets/preview.png" alt="FreeBSD 15 arm64 booting under bsdkrun on macOS" width="800">
</p>

---

## Contents

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
- [Managing machines](#managing-machines)
- [Networking](#networking)
- [Disks](#disks)
- [Console](#console-how-output-reaches-your-terminal)
- [Preparing a guest image](#preparing-a-guest-image)
- [Project layout](#project-layout)
- [Troubleshooting](#troubleshooting)
- [Status](#status)
- [License](#license)

---

## Why this exists

The usual microVM stacks (Firecracker, Cloud Hypervisor) don't run on macOS, and the usual macOS
VM tooling (QEMU, `vftool`, UTM) isn't microVM-shaped. libkrun gives you a Firecracker-like
"configure a context, then `start_enter`" model on top of Hypervisor.framework — its batteries are
aimed at Linux guests, and `bsdkrun` both leans into that (running OCI images directly) and points
the same machinery at **FreeBSD / NetBSD** guests.

- **FreeBSD:** boot via **firmware/EFI** — these systems expect to come up through a UEFI loader,
  so we hand libkrun its bundled EDK2 firmware and let the guest's `loader.efi` take over from the
  EFI System Partition on the disk. That firmware ships only with **`libkrun-efi`, which is
  macOS-only**, so **`bsdkrun freebsd` is a macOS-only command** (compiled out on Linux).
- **NetBSD:** boot via **direct kernel** — **no bootloader or firmware**, so `bsdkrun netbsd` works
  on **both macOS and Linux**. On **arm64** it downloads NetBSD's evbarm `GENERIC64` kernel + live
  `gzimg`. NetBSD ships no amd64 disk image, so on **amd64** bsdkrun uses its own bundled FFS rootfs
  plus the **`MICROVM`** kernel (a PVH ELF libkrun boots exactly like the Linux vmlinux); both are
  hosted as release assets (built by the `release-netbsd-amd64-image` workflow).
- **Linux:** run any **OCI image** as a microVM — bsdkrun fetches a prebuilt kernel, pulls the
  image from any registry, extracts its rootfs, and boots it `docker run`-style, with internet
  access out of the box. See [`linux`](#linux--run-an-oci-image-as-a-microvm).

> DragonFly BSD is out of scope: there's no arm64 port.

The open research question is guest-side **virtio-mmio device discovery**. libkrun exposes its
virtio devices over MMIO (there is no PCI bus), and describes them via the ACPI/FDT it hands the
guest. Whether a given BSD kernel enumerates those virtio-mmio devices — and routes its console to
libkrun's virtio-console — is exactly what this tool is for probing.

---

## Install

**macOS (Apple Silicon)** — a prebuilt, already-signed binary via Homebrew:

```sh
brew install tsirysndr/tap/bsdkrun
```

This auto-taps `libkrun/krun` and pulls in its dependencies (`libkrun`, `gvproxy`). The binary
ships codesigned with the hypervisor entitlement, so there's nothing else to set up — jump to
[Usage](#usage).

**Nix flake** — builds bsdkrun with all its dependencies. On **Linux (amd64/arm64)** it links
nixpkgs' libkrun; on **macOS** it links your Homebrew libkrun, so those need `--impure`
(`brew install libkrun/krun/libkrun` first) and produce a binary re-signed with the hypervisor
entitlement.

```sh
# Linux (needs /dev/kvm access)
nix run           github:tsirysndr/bsdkrun -- linux alpine   # run without installing
nix profile install github:tsirysndr/bsdkrun                 # install into your profile
nix develop       github:tsirysndr/bsdkrun                   # dev shell with the full toolchain

# macOS (Apple Silicon) — impure link against Homebrew's libkrun
brew install libkrun/krun/libkrun
nix build  --impure github:tsirysndr/bsdkrun                  # -> ./result/bin/bsdkrun
nix run    --impure github:tsirysndr/bsdkrun -- linux alpine
```

The flake wraps the runtime tools (`curl`, `tar`, `gzip`, `xz`, `cpio`, `gvproxy`, …) onto `PATH`,
and `nix develop` adds the Rust toolchain plus `zig`/`cargo-zigbuild` for cross-building the guest
agents. To hack on bsdkrun without Nix, build from source — see [Prerequisites](#prerequisites) and
[Build](#build).

---

## Prerequisites

You need **libkrun**, a **Rust toolchain** (`rustup default stable`; edition 2021), and access to
the hypervisor. The hypervisor part differs by OS.

### macOS (Apple Silicon)

libkrun, `krunvm`, and `krunkit` live in the **`libkrun/krun`** tap (redirected from the old
`slp/krun`). Homebrew 6.x requires you to trust a third-party tap before it will run its install
code:

```sh
brew tap libkrun/krun
brew trust libkrun/krun     # required on Homebrew 6.x for third-party taps
brew install libkrun krunkit
```

- **`libkrun`** provides `libkrun.dylib` (the C ABI we link against).
- **`krunkit`** ships the EDK2 UEFI firmware we use for EFI boot
  (`.../share/krunkit/KRUN_EFI.silent.fd`).

**The Hypervisor entitlement (the part that bites everyone).** A binary that calls libkrun must be
codesigned with `com.apple.security.hypervisor` (plus `com.apple.security.cs.disable-library-validation`
so it can load the Homebrew dylibs). Without it, `krun_create_ctx`/`krun_set_vm_config` succeed but
`krun_start_enter` fails at VM creation with `Internal(Vm(VmSetup(VmCreate)))` / **errno 22
(EINVAL)**. Worse, **every `cargo build` strips the codesignature**, so you must re-sign after each
build — the [`Makefile`](./Makefile) does this for you (entitlements in
[`bsdkrun.entitlements`](./bsdkrun.entitlements)).

### Linux (amd64 or arm64, KVM)

libkrun uses **KVM** on Linux — no codesigning, but you need **`/dev/kvm`** access (be in the `kvm`
group, or run under `sudo`). Ubuntu has no libkrun package, so build it (and its bundled kernel,
libkrunfw) from source — see the [KVM e2e workflow](.github/workflows/e2e-linux.yml) for the exact
steps:

```sh
git clone --depth 1 https://github.com/containers/libkrunfw && make -C libkrunfw && sudo make -C libkrunfw install
git clone --depth 1 https://github.com/containers/libkrun   && make -C libkrun   && sudo make -C libkrun   install
sudo ldconfig
```

`build.rs` finds libkrun via `pkg-config libkrun` (or the standard lib dirs; override with
`LIBKRUN_PREFIX=/path`). There's nothing to sign — `make build` skips the codesign step on Linux.
Some BSD image-prep steps (`losetup`/`mount`) need root, and bsdkrun runs them with `sudo`
automatically when needed.

> On Linux the CI boots the `linux` (OCI) path and the `netbsd` (direct-kernel) path — on x86_64
> only, since GitHub's arm64 runners have no `/dev/kvm`; arm64-on-Linux (which reuses the aarch64
> kernel + agent) is validated on a KVM-capable host. `freebsd` needs the macOS-only EFI firmware,
> so it's not offered on Linux; NetBSD-under-KVM on amd64 is still experimental.

---

## Build

```sh
make build      # cargo build (debug)  [+ codesign on macOS]
make release    # cargo build --release [+ codesign on macOS]
```

`make run ARGS="..."` builds and runs in one step.

> ⚠️ **macOS: don't run `cargo build` then the binary directly** — it'll be unsigned and fail at
> boot with errno 22. Go through `make`, or re-run `make sign` after a bare `cargo build`. On Linux
> there's nothing to sign, so `cargo build` is fine (the `make` sign steps are no-ops there).

The [`build.rs`](./build.rs) locates libkrun via `brew --prefix libkrun` (macOS) or `pkg-config`
(Linux), override with `LIBKRUN_PREFIX=/path`, and embeds an rpath so the shared library resolves
at runtime.

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
bsdkrun freebsd                 # latest FreeBSD, foreground   (macOS only — needs KRUN_EFI)
bsdkrun netbsd  -d              # NetBSD-current in the background; prints its id
bsdkrun netbsd  --version 10.1 -d --port 2222:22
```

Both carry the usual machine options (`-d`, `--persist`, `-v/--volume`, `--version`,
`--attach-disk`, `--port`, `--cpus`/`--mem`), so per-machine [CoW disk clones](#managing-machines)
and `ps`/`logs`/`shell`/`stop` all apply. They differ in how they boot:

- **`freebsd`** wraps [`fetch`](#the-easy-way--bsdkrun-fetch) + [`firmware`](#firmware--boot-a-disk-through-its-uefi-loader):
  it auto-locates libkrun's `KRUN_EFI` firmware (via `$BSDKRUN_FIRMWARE`, a local
  `images/KRUN_EFI.fd`, or krunkit's Homebrew install; `--firmware` overrides). That firmware is
  **macOS-only**, so **`freebsd` is a macOS-only command**.
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
  binutils) and caches that too. Pick a release with `--kernel-version` (default `7.1.5`), or point
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
bsdkrun shell $id          # open an interactive shell in the guest
bsdkrun stop $id           # stop a running machine
```

Any unique **id prefix** works (`bsdkrun stop 8e1c`). `shell` attaches to the guest console: for a
Linux machine that's an interactive shell (with `exit`/re-attach); for BSD it's the guest's own
console (e.g. the `login:` prompt).

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

Extra disks appear in the guest as the next `virtio-blk` devices (e.g. FreeBSD `vtbd1`, `vtbd2`…),
in the order given. Create a blank one with `truncate -s 8G data.raw` (then partition/newfs it in
the guest), or grow an existing image with [`bsdkrun grow`](#resizing-the-disk).

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

- **arm64** ✅ downloads bsdkrun's bundled evbarm image (the live `gzimg` with the agent injected)
  + the evbarm `GENERIC64` kernel from the NetBSD CDN, rooting on the GPT wedge `dk1`. libkrun
  exposes **modern (v2) virtio-mmio**, and NetBSD's driver only gained v2 support in **-current**
  (post-10.x): the bundled image is `current`, so `--version` only affects the kernel — pinning a
  **release** (≤ 10.1) kernel prints `virtio: unknown version 0x02; giving up`.
- **amd64** ❌ is **not supported under libkrun** and is gated off with a clear error. libkrun boots
  x86_64 kernels with the **Linux boot protocol** (a `boot_params` zero page), not **PVH**, so the
  NetBSD kernel triple-faults instantly (`KVM_EXIT_SHUTDOWN`, no console) — this is a libkrun
  limitation, not a NetBSD one (arm64 works because libkrun uses the OS-agnostic Image+FDT there).
  The bundled amd64 FFS rootfs + `MICROVM` kernel are built and ready for a PVH-capable libkrun; set
  `BSDKRUN_NETBSD_AMD64=1` to attempt the boot against one.

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

Both guests boot to a `login:` prompt via the `firmware` subcommand on macOS Apple Silicon, through
libkrun's EDK2 firmware and the guest's own EFI loader:

- **FreeBSD 15.1 / arm64** — full multi-user rc sequence to `login:`. `fetch` + `firmware`.
- **NetBSD-current / arm64** (evbarm `GENERIC64`) — efiboot → kernel → root-on-ffs → `login:`.
  `fetch --os netbsd` + `firmware`. (NetBSD *releases* ≤ 10.1 boot but can't mount root — their
  virtio-mmio driver is legacy-only; modern v2 support is only in -current.)

The blocker that made guests look dead — console output going to a PL011 serial libkrun wasn't
forwarding — is fixed by bsdkrun's serial-console wiring (see
[Console](#console-how-output-reaches-your-terminal)). Guest-side virtio-mmio device discovery
under libkrun is confirmed working for FreeBSD and NetBSD-current.

Next: interactive login + networking shakedown, and `kernel`-mode direct boot experiments.

## License

[MIT](./LICENSE) © Tsiry Sandratraina
