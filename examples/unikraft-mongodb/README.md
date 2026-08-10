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

**Not working yet, on either architecture — but for different reasons, and
both are now visible rather than hidden.** This is the most demanding example
here: mongod is a very large (~150 MiB) heavily threaded C++ server. It runs
in `.github/workflows/e2e-unikraft-examples.yml` as `strict: false`, so the
check reports without failing the job.

How far it gets:

| | |
|---|---|
| **arm64** | Starts, opens WiredTiger, completes recovery, creates collections, listens on 27017 — then WiredTiger's log server panics a few seconds in. `log=(prealloc=false)` in `start.sh` addresses that path; not yet re-verified. |
| **x86_64** | Same startup, then a thread in `WaitForMajorityServiceThreadPool` takes `SIGABRT` about half a second in and the process quits. It prints a backtrace with nothing but its own signal handler in it, so the abort is asynchronous — nothing in mongod's own code is on the stack. |

The x86_64 abort is the open question. It does **not** happen on arm64, where
the same code path starts fine, which points at something architecture
specific in the guest rather than at mongod. A newly created thread aborting
with no diagnostic of its own is the shape of a stack-protector failure —
glibc calls `abort()` from `__stack_chk_fail` — which would make thread TLS
setup the place to look; see the vfork TLS fix in
`../../library/unikraft-base/patches` for a related x86_64 TLS bug.

Until recently this was invisible: the guest itself crashed *inside* signal
delivery (`Assertion failure: !(altstack->ss_flags & 1)`), destroying the
diagnostic mongod was printing. That was a genuine Unikraft bug — the
alternate signal stack is per process where POSIX makes it per thread — and
is fixed in `../../library/unikraft-base/patches`.

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

**A one-CPU topology is baked into the image.** mongod sizes its plan-cache
partitions from the CPU count (`std::thread::hardware_concurrency` →
glibc's `__get_nprocs`, which reads `/sys/devices/system/cpu/online` and falls
back to `/proc/stat`). Unikraft has neither procfs nor sysfs, so it counted
zero cores and died on its own invariant — `numPartitions > 0`. Nothing
shadows those paths either, so the ramfs can simply serve them: the Dockerfile
ships `online`, `/proc/stat` and `/proc/cpuinfo` describing the single vcpu
libkrun gives the guest. Same trick as `../unikraft-dragonflydb` uses for
`/proc/self/cgroup`.

**WiredTiger does not pre-allocate journal files.** Its log server otherwise
keeps a pool of `WiredTigerPreplog.NNN` files that it renames into place, and
that cycle fails on ramfs with `ENOENT` — `__log_server:924` — panicking the
library and taking mongod with it.

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
