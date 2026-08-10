# unikraft-dragonflydb

[DragonflyDB](https://www.dragonflydb.io/) v1.14.1, running as a Unikraft
unikernel. Ported from [`unikraft-cloud/examples`'s
`dragonflydb`](https://github.com/unikraft-cloud/examples/tree/main/dragonflydb)
to build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 1024 --port 6379:6379 \
  --cmdline "elfloader -- /usr/bin/dragonfly --force_epoll --maxmemory=256mb"
```

Dragonfly speaks the Redis protocol:

```console
$ redis-cli -p 6379 SET greeting hello
OK
```

## Status

Untested as of this writing, and the most speculative example in this
directory: Dragonfly's io layer ("helio") is built around io_uring, which
Unikraft does not have. `--force_epoll` selects its epoll backend instead,
but that backend still expects a fairly modern Linux underneath — this
example is how we find out whether Unikraft's syscall surface is enough.
x86_64 runs in `.github/workflows/e2e-unikraft-examples.yml` as
`strict: false`; arm64 is exercised by hand on macOS/Hypervisor.framework.

## Differences from upstream

**No wrapper script, no shell, no cgroups.** Upstream boots
`bash wrapper.sh`, which mounts cgroup2 and then starts the server — Unikraft
has no mount(8) and no cgroups to mount. The two things the wrapper's
environment provided are passed as flags instead:

- `--force_epoll` — there is no io_uring to probe; without this dragonfly
  aborts at startup.
- `--maxmemory=256mb` — with no cgroup limit to read, dragonfly must be told
  its budget rather than sizing itself off the machine.

Unlike redis-server (see `../unikraft-redis`), dragonfly's abseil flag parser
treats unrecognised bare words as positional arguments and ignores them, so
the junk argv libkrun appends should pass through without a trampoline
script. If a future dragonfly starts validating positionals, borrow
`start.sh` from the redis example.

**No `runtime: base-compat:latest`**, and **libraries resolved with `ldd`**
rather than hardcoded x86_64 paths — same reasons as every other example
here. `cp -L` also resolves the `dragonfly` →
`dragonfly-{x86_64,aarch64}` symlink to whichever binary the image
architecture carries.

**Persistence is a non-feature here**: the working directory is a ramfs that
does not survive a reboot. Dragonfly's snapshot save runs in-process (no
fork, unlike redis), so it is not dangerous — just pointless.

## Layout

| file         | role                                                         |
|--------------|--------------------------------------------------------------|
| `Dockerfile` | rootfs: `dragonfly` and its libraries (via `ldd`)            |
| `Kraftfile`  | the from-source base runtime + preemption/TID + elfloader    |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`         |
