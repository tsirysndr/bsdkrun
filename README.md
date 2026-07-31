# bsdkrun

A Firecracker-style **microVM launcher for BSD guests on macOS Apple Silicon**, built on
[libkrun](https://github.com/containers/libkrun) (which drives Apple's Hypervisor.framework).

`bsdkrun` is a thin, purpose-built CLI: it wraps libkrun's C ABI in a handful of safe Rust
bindings and boots a BSD guest two ways — from a **UEFI firmware** image (the guest's own EFI
loader boots a normal disk) or from a **direct kernel + FDT** (no bootloader). It is deliberately
small: one FFI module, one CLI, no daemon.

> **Platform:** macOS on Apple Silicon (arm64) only. libkrun's macOS backend is
> Hypervisor.framework, and the guests we target are arm64 BSD images.

<p align="center">
  <img src=".github/assets/preview.png" alt="FreeBSD 15 arm64 booting under bsdkrun on macOS" width="800">
</p>

---

## Why this exists

The usual microVM stacks (Firecracker, Cloud Hypervisor) don't run on macOS, and the usual macOS
VM tooling (QEMU, `vftool`, UTM) isn't microVM-shaped. libkrun gives you a Firecracker-like
"configure a context, then `start_enter`" model on top of Hypervisor.framework — but its batteries
are aimed at Linux guests. `bsdkrun` is an experiment in pointing that same machinery at
**FreeBSD / NetBSD / OpenBSD** guests.

- **FreeBSD / OpenBSD (arm64):** boot via **firmware/EFI** — these systems expect to come up
  through a UEFI loader, so we hand libkrun its bundled EDK2 firmware and let the guest's
  `loader.efi` take over from the EFI System Partition on the disk.
- **NetBSD (evbarm):** boot via **direct kernel** — libkrun generates an FDT and jumps straight
  into the kernel, no bootloader.

> DragonFly BSD is out of scope: there's no arm64 port.

The open research question is guest-side **virtio-mmio device discovery**. libkrun exposes its
virtio devices over MMIO (there is no PCI bus), and describes them via the ACPI/FDT it hands the
guest. Whether a given BSD kernel enumerates those virtio-mmio devices — and routes its console to
libkrun's virtio-console — is exactly what this tool is for probing.

---

## Prerequisites

### 1. Install libkrun (Homebrew)

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

### 2. A Rust toolchain

```sh
rustup default stable   # or any recent stable; project is edition 2021
```

### 3. The Hypervisor entitlement (this is the part that bites everyone)

**A binary that calls libkrun must be codesigned with the `com.apple.security.hypervisor`
entitlement** (plus `com.apple.security.cs.disable-library-validation` so it can load the
Homebrew dylibs). Without it:

- `krun_create_ctx` and `krun_set_vm_config` **succeed** (they never touch the hypervisor), so
  everything looks fine…
- …until `krun_start_enter`, which fails at VM creation with
  `Internal(Vm(VmSetup(VmCreate)))` / **errno 22 (EINVAL)**.

Worse: **every `cargo build` strips the codesignature**, so you must re-sign after each build.
The [`Makefile`](./Makefile) handles this for you (see below). The entitlements live in
[`bsdkrun.entitlements`](./bsdkrun.entitlements).

---

## Build

```sh
make build      # cargo build (debug) + codesign with the hypervisor entitlement
make release    # cargo build --release + codesign
```

`make run ARGS="..."` builds, signs, and runs in one step.

> ⚠️ **Don't run `cargo build` and then the binary directly** — the binary will be unsigned and
> will fail at boot with errno 22. Always go through `make`, or re-run `make sign` yourself after
> a bare `cargo build`.

The [`build.rs`](./build.rs) locates libkrun via `brew --prefix libkrun` (override with
`LIBKRUN_PREFIX=/path`) and embeds an rpath so the versioned dylib resolves at runtime.

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

### `firmware` — boot a disk through its UEFI loader

The compatible path for **FreeBSD / OpenBSD**. Point it at libkrun's EDK2 firmware and a raw disk
image that carries an EFI System Partition:

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

# OpenBSD (installer image — see note below)
bsdkrun fetch --os openbsd          # OpenBSD installer -> ./images/openbsd-<ver>.img

# then boot what it printed:
bsdkrun firmware --firmware images/KRUN_EFI.fd --disk images/freebsd-15.1.raw --cpus 2 --mem 2048
```

With no `--version`, FreeBSD resolves the newest release, NetBSD uses **`current`**, and OpenBSD
uses the newest release (see the notes below). Downloads are a few hundred MiB (OpenBSD's are
uncompressed; the others expand to a couple GiB).

Downloaded images are cached under **`~/.cache/bsdkrun/`** (override with `BSDKRUN_CACHE`, or
`XDG_CACHE_HOME`), so fetching a version you already have is instant — it just links the cached
image into `--dir` (a hard link, no second copy). Use `--force` to re-download.

### NetBSD: use `current`, not a release

NetBSD boots through the **same `firmware` path** as FreeBSD (its `efiboot` + `GENERIC64` kernel
run under libkrun, and the kernel auto-selects libkrun's PL011 UART as console — no `loader.env`
needed). But there's a catch: libkrun exposes **modern (v2) virtio-mmio**, and NetBSD's
virtio-mmio driver only gained v2 support in **-current** (post-10.x). So:

- **`bsdkrun fetch --os netbsd`** (→ `current`) boots all the way to `login:` with a working root
  disk. ✅
- Any NetBSD **release** (≤ 10.1) boots (kernel + console) but prints
  `virtio: unknown version 0x02; giving up` and can't mount its root disk. `fetch` warns you if you
  pin one.

### OpenBSD: installer image only (work in progress)

OpenBSD ships an **installer** (RAMDISK) image for arm64, not a preinstalled disk. `fetch --os
openbsd` downloads it and `firmware` boots it. The good news: OpenBSD's EFI bootloader, the RAMDISK
kernel, the **virtio-mmio disk** (`sd0`), and the PL011 console all come up under libkrun. The
current limitation: the interactive installer stalls just after `softraid0` and doesn't reach its
`(I)nstall/(S)hell` prompt yet — under investigation. A fully installed, persistent OpenBSD also
needs a second target disk (multi-disk support, not yet wired up).

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
| `src/krun.rs`          | Safe Rust FFI bindings to the libkrun C ABI (`krun_create_ctx`, `krun_set_vm_config`, `krun_add_disk`, `krun_set_kernel`, `krun_set_firmware`, the `krun_disable_implicit_console` / `krun_add_serial_console_default` console wiring, `krun_start_enter`, …). Negative returns are decoded as `-errno`. |
| `src/main.rs`          | `clap` CLI with the `probe` / `kernel` / `firmware` subcommands. |
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

---

## Status

Both guests boot to a `login:` prompt via the `firmware` subcommand on macOS Apple Silicon, through
libkrun's EDK2 firmware and the guest's own EFI loader:

- **FreeBSD 15.1 / arm64** — full multi-user rc sequence to `login:`. `fetch` + `firmware`.
- **NetBSD-current / arm64** (evbarm `GENERIC64`) — efiboot → kernel → root-on-ffs → `login:`.
  `fetch --os netbsd` + `firmware`. (NetBSD *releases* ≤ 10.1 boot but can't mount root — their
  virtio-mmio driver is legacy-only; modern v2 support is only in -current.)
- **OpenBSD / arm64** — installer RAMDISK boots (EFI bootloader → kernel → virtio-mmio `sd0` +
  console). Reaching the installer prompt / installing to disk is still WIP (see the OpenBSD note
  above). `fetch --os openbsd`.

The blocker that made guests look dead — console output going to a PL011 serial libkrun wasn't
forwarding — is fixed by bsdkrun's serial-console wiring (see
[Console](#console-how-output-reaches-your-terminal)). Guest-side virtio-mmio device discovery
under libkrun is confirmed working for FreeBSD and NetBSD-current.

Next: interactive login + networking shakedown, `kernel`-mode direct boot experiments, and trying
**OpenBSD/arm64** via the same firmware path.

## License

[MIT](./LICENSE) © Tsiry Sandratraina
