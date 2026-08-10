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

**arm64 works.** The unikernel boots, DHCPs an address, WiredTiger opens and
recovers, mongod listens on 27017, and a real write round-trips over the
forwarded port — verified twice, ninety seconds apart, with the server still
up and no aborts in the console:

```console
$ mongosh --port 27017 --quiet --eval \
    'db.t.replaceOne({_id:1},{_id:1,name:"unikraft-on-bsdkrun"},{upsert:true});
     print(db.t.findOne({_id:1}).name)' e2e
unikraft-on-bsdkrun
```

x86_64 has never been run; `.github/workflows/e2e-unikraft-examples.yml`
runs it as `strict: false` until its first green run. The bug that used to
kill it there is fixed below and is not architecture specific, so it has a
fair chance.

Getting here took five things — three in this directory, two in the kernel:

| | |
|---|---|
| **CPU topology** | mongod counted zero cores and died on `numPartitions > 0`. |
| **1 MiB stacks** | Default pthread stacks are sized from `RLIMIT_STACK`; at 64 KiB a startup thread overflowed its guard page. |
| **No journal pre-allocation** | WiredTiger's log server panicked with `ENOENT` renaming `WiredTigerPreplog` files on ramfs. |
| **Its own `/proc` entries** | `/proc/<pid>/stat` missing throws `Location13538` once a second; a thread that does not catch it calls `terminate()` → `abort()`. |
| **`sigaltstack(SS_DISABLE)`** | Rejected with `ENOMEM`, so every thread teardown aborted the server. Kernel fix. |

The last two were the same symptom — a bare `Got signal: 6 (Aborted)` in a
pool thread, with nothing of mongod's own on the stack — which is why they
took three days and a syscall trace to tell apart. The trace ends:

```
sigaltstack(...) = Cannot allocate memory (-12)
rt_sigprocmask / gettid() / getpid()            <- glibc's raise()
"Writing fatal message" ... "Got signal: 6 (Aborted)."
```

mongod installs an alternate signal stack on each thread and disables it when
the thread goes away, and treats a failing `sigaltstack()` as fatal. Unikraft
validated the stack size before looking at the flags, so a teardown — a
zeroed `stack_t` with `SS_DISABLE`, which describes no stack at all — failed
with `ENOMEM`. Linux checks the size only when the request is not a disable.
Fixed in `../../library/unikraft-base/patches`.

None of that was visible until an earlier fix in the same file: Unikraft kept
one alternate signal stack per *process* where POSIX gives each thread its
own, so the guest crashed *inside* signal delivery
(`Assertion failure: !(altstack->ss_flags & 1)`) and destroyed the diagnostic
the application was printing. Fixing that is what made everything above
legible.

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
