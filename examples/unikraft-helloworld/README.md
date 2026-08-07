# unikraft-helloworld

A [Unikraft](https://unikraft.org) unikernel that boots under bsdkrun. The
application is linked *into* the kernel: one ~220 KB binary, no userland, no
init, no shell — it boots in milliseconds, prints, and powers the VM off.

Taken from [`unikraft/catalog`](https://github.com/unikraft/catalog)
(`library/helloworld`), with one addition to the `Kraftfile` — see below.

## Build and run

```sh
./build.sh          # host arch; or: ./build.sh x86_64
bsdkrun unikraft .
```

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

`build.sh` runs `kraft` inside a Debian container. That is not incidental on
macOS: the Unikraft build tree needs GNU make/sed and a Linux toolchain, and a
host build fails with `gsed: command not found`, then `Target architecture ()
is currently not supported`. On Linux you can skip the script:

```sh
kraft build --plat fc --arch arm64      # or --arch x86_64
```

## Why `fc`, and why the PL011 line in the Kraftfile

Build for the **Firecracker** platform (`--plat fc`), not `qemu`: only `fc`
links arm64 at `0x8000_0000`, which is where libkrun's aarch64 loader places a
raw image. A `qemu` build links at `0x4000_0000` and will never match.

The `Kraftfile` here adds `CONFIG_LIBPL011` on top of the catalog's version.
libkrun's aarch64 console is an ARM PL011 while Firecracker's is an ns16550, so
a stock `fc/arm64` build boots **silently** — it looks like a hang, but the
guest is fine and you simply never see its output. Both drivers probe the
device tree, so enabling PL011 alongside the default costs nothing and produces
one image that boots under either VMM. On x86_64 the default ns16550 on COM1
already matches what libkrun exposes, so no override is needed.

See the [`unikraft` section of the README](../../README.md#unikraft--boot-a-unikraft-unikernel)
for the rest — the `text_offset` entry shim, and the libkrun fix arm64 needs.

## What does not apply

A unikernel has no disk and no agent, so `exec`, `shell`, and `commit`
(snapshot) are rejected for these machines. `logs`, `ps`, `stop`, `start`, and
`rm` work as usual.
