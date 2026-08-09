# osv-hello

A plain C program running as an [OSv](https://github.com/cloudius-systems/osv)
unikernel under bsdkrun — on **macOS/Apple Silicon** via Hypervisor.framework,
and on **Linux/x86_64** via KVM.

OSv is not like the other unikernels bsdkrun boots. A Unikraft or Nanos image is
the application *linked into* the kernel; an OSv image is a kernel plus a
filesystem, and the application is an ordinary **Linux binary** that OSv loads
and calls `main()` on. So `hello.c` is just C — nothing in it knows about OSv —
and all the OSv-specific detail lives in how it is linked and packaged.

## Build and run

```sh
./build.sh                 # host arch; or: ./build.sh x86_64
capstan run -e /hello.so osv-hello
```

```
OSv v0.57.0
Booted up in 1.50 ms
Hello from OSv on libkrun!
  running on Linux 3.7.0 (aarch64)
```

`build.sh` compiles inside a Debian container (an OSv app is a Linux binary, and
macOS has no Linux toolchain), then calls `capstan` to compose the OSv loader and
the application into one bootable image.

You need [capstan](https://github.com/cloudius-systems/capstan) with the bsdkrun
backend, which is what turns `capstan run` into a `bsdkrun osv` invocation.
`bsdkrun osv` also works directly on a **raw** image:

```sh
qemu-img convert -O raw ~/.capstan/repository/osv-hello/osv-hello.qemu disk.raw
bsdkrun osv disk.raw --cmdline=/hello.so
```

## The three things that matter

**1. Build position-independent code.** OSv loads the application with its ELF
loader and looks up `main`:

```sh
gcc -O2 -shared -fPIC -o hello.so hello.c    # what this example does
gcc -O2 -fPIE -pie  -o hello.elf hello.c     # also works
```

Either a shared object or a PIE executable is fine — both were verified. What
does *not* work is a position-dependent binary (`-no-pie`): OSv runs the
application in the single address space it shares with the kernel, so a binary
linked at a fixed address has nowhere to go, and the guest hangs with nothing
on the console.

`libc.so.6` in the app's `DT_NEEDED` is fine and expected: OSv resolves the libc
symbols against the kernel's own exports, which is why `printf` and `uname` work
with no libc in the image.

**2. Use `--fs rofs`.** ROFS is built entirely on the host. The `zfs` variant
boots a *builder VM* first, which on Apple Silicon means booting an x86_64
image — the thing that cannot work here.

**3. The image is both kernel and disk.** capstan lays the OSv loader at the
front of the image and the filesystem behind it. `bsdkrun osv` slices the leading
kernel out (libkrun reads the whole kernel file into guest RAM, so it must not be
handed a multi-gigabyte disk) and attaches the image itself as virtio-blk.

## Things you will see, which are fine

**`argv` has extra entries.** libkrun appends its own parameters to the kernel
command line, and OSv hands everything after the application path to the app:

```
  argv[1] = world
  argv[2] = earlycon=pl011,mmio32,0x0a001000
  argv[3] = tsi_hijack
```

**`unhandled InterruptID irq=0x20`.** libkrun assigns its legacy devices the
first SPIs — serial 32, RTC 33, GPIO 34 — before any virtio device (which start
at 35). IRQ 32 is therefore the **PL011 serial**, which OSv writes its console
through but never registers an interrupt handler for. It is one line at boot and
nothing depends on it.

## Building libkrun yourself

Build it with **`make BLK=1 NET=1`**. virtio-blk is behind a feature flag, and a
libkrun built without it has no `krun_add_disk` — which bsdkrun calls through a
now-NULL stub, so the process dies with a bare SIGSEGV the moment a disk is
attached, with nothing on the console to say why. On macOS the build also wants
`LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib` and GNU make (`gmake`), and both the
dylib and any binary relinked against it need re-signing:

```sh
codesign --force -s - target/release/libkrun.*.dylib
codesign --entitlements bsdkrun.entitlements --force -s - target/release/bsdkrun
```

## Why bsdkrun needs a fork of libkrun for this

Two guest-driven reasons, both handled automatically by `bsdkrun osv`:

* **GICv2.** OSv only grew a GICv3 driver *after* its v0.57.0 release, so the
  released aarch64 kernel — the one capstan downloads — aborts with
  `arch-setup: failed to get GICv2 information from dtb` against libkrun's
  GICv3. The fork adds a userspace GICv2 (`KRUN_GIC=v2`).
* **virtio-rng off the bus** (`KRUN_NO_RNG=1`). OSv's rng driver is PCI-only: it
  registers itself but never fills in the MMIO interrupt factory, so probing
  libkrun's virtio-rng throws `std::bad_function_call` and kills the guest
  partway through driver init.

On **Linux/x86_64** none of that applies — there is no GIC — and the loader is
an ELF carrying the Xen `PHYS32_ENTRY` note, so it boots through the same **PVH**
path the fork already uses for NetBSD and FreeBSD (`KRUN_PVH=1`).
