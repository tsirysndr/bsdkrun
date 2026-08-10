# unikraft-dragonflydb

[DragonflyDB](https://www.dragonflydb.io/) v1.14.1, running as a Unikraft
unikernel. Ported from [`unikraft-cloud/examples`'s
`dragonflydb`](https://github.com/unikraft-cloud/examples/tree/main/dragonflydb)
to build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 1024 --port 6379:6379 \
  --cmdline "elfloader -- /usr/bin/dragonfly --force_epoll --maxmemory=256mb --alsologtostderr"
```

Dragonfly speaks the Redis protocol:

```console
$ redis-cli -p 6379 SET greeting hello
OK
```

## Status

**arm64 works.** The unikernel boots, DHCPs an address, dragonfly's epoll
proactor comes up ("Host OS: Unikraft 5.15.148-Ijiraq arm64 with 1 threads"),
it listens on 6379 and answers SET/GET over the forwarded port — despite its
io layer ("helio") being built around io_uring, which Unikraft does not have.
Getting there took the two adaptations below (`--force_epoll`, and a fake
cgroup2 file surface). x86_64 has never been run;
`.github/workflows/e2e-unikraft-examples.yml` runs it as `strict: false`
until its first green run.

## Differences from upstream

**No wrapper script, no shell — a fake cgroup2 surface instead.** Upstream
boots `bash wrapper.sh`, which mounts cgroup2 and then starts the server.
Unikraft has no mount(8) and no cgroups — and without them dragonfly aborts
at its first startup check:

```
F19700101 00:00:01.025056  2 dfly_main.cc:361] Check failed: cg.has_value()
Failed to read /proc/self/cgroup
```

But no procfs also means nothing *shadows* `/proc`: the ramfs the rootfs is
unpacked into serves `/proc/self/cgroup` like any other path. So the image
bakes the minimal cgroup2 surface dragonfly reads — `0::/` there, and
unlimited `memory.max`/`cpu.max` under `/sys/fs/cgroup` — and the check
passes without a wrapper, a shell, or a mount. The flags the wrapper's
environment used to provide are passed directly:

- `--force_epoll` — there is no io_uring to probe; without this dragonfly
  aborts at startup.
- `--maxmemory=256mb` — with no real cgroup limit to read, dragonfly must be
  told its budget rather than sizing itself off the machine.
- `--alsologtostderr` — glog writes to files under `/tmp` by default, where
  nobody will ever read them; this mirrors the server's log to the guest
  console.

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
