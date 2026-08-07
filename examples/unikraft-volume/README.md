# unikraft-volume

A [Unikraft](https://unikraft.org) unikernel with a **persistent volume**: a
host directory shared in over virtio-fs. The app reads a counter from
`/data/counter`, increments it, and writes it back — so the count can only go
up if the write really reached the host and outlived the VM.

```sh
./build.sh                                  # host arch; or: ./build.sh x86_64
mkdir -p /tmp/ukvol
bsdkrun unikraft . --mount /tmp/ukvol:/data
bsdkrun unikraft . --mount /tmp/ukvol:/data
bsdkrun unikraft . --mount /tmp/ukvol:/data
```

```
volume test: reading /data/counter
  (no counter yet — first boot)
VOLUME OK: boot count is now 1
...
  read counter = 2
VOLUME OK: boot count is now 3

$ cat /tmp/ukvol/counter
3
```

`--mount HOST:GUEST` is repeatable; the guest path must be absolute.

## Why virtio-fs

It is the only option that works. libkrun has **no virtio-9p** device, which is
what `kraft run -v` uses under QEMU. It does have virtio-blk, and Unikraft has
a driver for it, but Unikraft's core registers only two filesystems — `ramfs`
and `virtiofs` — so a raw disk gives you blocks, not something mountable.

virtio-fs lines up exactly on both sides: `krun_add_virtiofs(tag, dir)` writes a
36-byte tag into the device's config space, and Unikraft's
`uk_virtiofs_dev_lookup()` matches devices on that same field.

## What bsdkrun generates

`--mount` turns into a virtio-fs share plus a mount table on the kernel command
line. For `--mount /tmp/ukvol:/data`:

```
volume_fc-arm64 vfs.fstab=[ "ramfs:/:ramfs" "vol0:/data:virtiofs:::mkmp" ] --
```

Three details in there each cost a debugging session, and getting any of them
wrong mounts nothing while the guest still boots perfectly:

- **A program name comes first.** `lib/ukboot/early_init.c` hands the parameter
  parser `&argv[1]`, treating the first word as the program name. A command
  line that *starts* with `vfs.fstab=` has the mount table silently eaten.
- **`--` separates the halves.** Kernel parameters before it, the application's
  `argv` after. Without it nothing is parsed as a parameter at all.
- **A root filesystem, and `mkmp`.** A unikernel boots with no filesystem
  whatsoever, so `/data` does not exist to mount onto — the mount fails with
  `ENOENT` before virtio-fs is ever consulted. The generated table mounts a
  ramfs at `/` first, and `mkmp` (make mount point) creates the directory.

## The patches

`build.sh` is a three-step build — **fetch, patch, build** — because two bugs in
unikraft 0.21.0 stop a virtio-fs guest working at all. Both are one-liners, in
`patches/`:

1. `lib/ukpod/anon.c` calls `UK_ASSERT` without including `<uk/assert.h>`, so
   the link fails with `undefined reference to 'UK_ASSERT'`. Unavoidable here:
   virtio-fs needs the posix-vfs stack, which pulls in ramfs, which selects
   `LIBUKPOD_ANON`.
2. `lib/ukfs-virtiofs/virtiofs.c` asserts that the **parent directory** is a
   regular file when creating a file, instead of the file it just created. A
   directory never is, so creating *any* file on a virtio-fs mount crashes the
   guest — on any VMM, with assertions at their default `y`.

Both should go upstream. Until they do, this example carries them; `patch -N`
makes the step idempotent, so rebuilds work without a clean fetch. Note
`build.sh` deliberately does **not** pass `--no-cache`, which would re-fetch the
sources and discard the patches.

## Kraftfile notes

`CONFIG_LIBUKFS_RAMFS=y` is required, not optional — the generated table mounts
a ramfs root, and without the driver the boot fails with `-ENODEV`.

The virtiofs driver lives on the newer ukfs/posix-vfs stack, which is mutually
exclusive with vfscore (`lib/posix-vfs/Config.uk`: `depends on !LIBVFSCORE`).
Nothing here uses vfscore, so this simply selects the new stack — but an
existing app built on vfscore cannot gain volumes by flipping a kconfig.

See [`../unikraft-helloworld`](../unikraft-helloworld) for the basics (why the
target must be `fc`, and the PL011 console).
