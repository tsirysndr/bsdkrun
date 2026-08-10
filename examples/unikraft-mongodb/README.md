# unikraft-mongodb

[MongoDB](https://www.mongodb.com/) 6.0, running as a Unikraft unikernel.
Ported from [`unikraft-cloud/examples`'s
`mongodb`](https://github.com/unikraft-cloud/examples/tree/main/mongodb) to
build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --port 27017:27017 \
  --cmdline "elfloader -- /bin/sh /start.sh"
```

```console
$ mongosh --port 27017 --quiet --eval \
    'db.t.insertOne({greeting: "hello"}); db.t.findOne().greeting'
```

## Status

Untested as of this writing, and the most demanding of the database examples:
mongod is a very large (~150 MiB) heavily threaded C++ server. x86_64 runs in
`.github/workflows/e2e-unikraft-examples.yml` as `strict: false` until it has
its first green run; arm64 is exercised by hand on
macOS/Hypervisor.framework.

Mind the CPU floors inherited from MongoDB itself: 5.0+ requires AVX on
x86_64 and ARMv8.2-A on arm64. GitHub's runners and Apple Silicon both
qualify.

## Differences from upstream

**No `runtime: base-compat:latest`**, and **libraries resolved with `ldd`**
rather than upstream's 36 hardcoded `/lib/x86_64-linux-gnu/...` paths — same
reasons as every other example here: both are x86_64-only.

**The boot command is `sh /start.sh`, not mongod.** libkrun appends its own
words (`earlycon=...`, `tsi_hijack`, a bare `--`) to the end of the kernel
command line, past the `--` stop sequence, so they arrive in the
application's argv — and mongod's option parser rejects positional words it
does not recognise. The start script soaks them up as positional parameters
and `exec`s mongod with a clean argv — one execve(), no fork, enabled by
`CONFIG_APPELFLOADER_MULTIPROCESS` exactly as in `../unikraft-postgres` and
`../unikraft-redis`. It also adds `--wiredTigerCacheSizeGB 0.25`: with no
cgroup to read, WiredTiger would size its cache from the guest's total
memory, most of which the twice-resident rootfs already spent.

**The data directory does not survive a reboot.** `/data/db` ships empty and
WiredTiger initialises it on first boot, in the ramfs the embedded initrd is
unpacked into. Every boot is a first boot.

## Layout

| file         | role                                                             |
|--------------|------------------------------------------------------------------|
| `Dockerfile` | rootfs: `mongod`, its libraries (via `ldd`), busybox, `/data/db` |
| `start.sh`   | argv filter: soaks up libkrun's junk, `exec`s mongod             |
| `Kraftfile`  | the from-source base runtime + preemption/TID + MULTIPROCESS     |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`             |
