# unikraft-redis

[Redis](https://redis.io/) 7.2, running as a Unikraft unikernel. Ported from
[`unikraft-cloud/examples`'s
`redis7.2`](https://github.com/unikraft-cloud/examples/tree/main/redis7.2) to
build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 512 --port 6379:6379 \
  --cmdline "elfloader -- /bin/sh /start.sh"
```

```console
$ redis-cli -p 6379 SET greeting hello
OK
$ redis-cli -p 6379 GET greeting
"hello"
```

## Status

**arm64 works.** The unikernel boots, DHCPs an address, the start script
execs redis-server, and SET/GET round-trip over the forwarded port. x86_64
has never been run; `.github/workflows/e2e-unikraft-examples.yml` runs it as
`strict: false` (the check runs and reports, but does not fail the job) until
its first green run.

Of the servers in this examples directory, redis asks the least of the kernel:
one process, a handful of threads, no fork at runtime (see below), no
filesystem beyond a scratch directory. Two things still bit on the way to a
first PONG — the argv trampoline and the ARM64-COW-BUG probe, both below.

## Differences from upstream

**No `runtime: base-compat:latest`.** Upstream pulls a prebuilt Unikraft Cloud
kernel, which is published for x86_64 only. The Kraftfile here builds the
equivalent runtime (`library/base` from `unikraft/catalog`) from source, plus
the arm64 fixes in `../../library/unikraft-base`.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
hardcodes `/lib/x86_64-linux-gnu/...` and `/lib64/ld-linux-x86-64.so.2`; on
arm64 those paths do not exist. `ldd` keeps the list correct on both
architectures.

**`protected-mode no` in redis.conf.** Upstream relies on Unikraft Cloud's
network for reachability. Under bsdkrun every client comes through a forwarded
port, and those connections arrive from the gvproxy gateway address rather
than loopback -- with no password set, protected mode would answer each one
with `-DENIED`. The server is still only reachable through ports the host
explicitly forwards.

**The boot command is `sh /start.sh`, not redis-server.** libkrun appends its
own words (`earlycon=...`, `tsi_hijack`, a bare `--`) to the end of the kernel
command line, past the `--` stop sequence, so they arrive in the application's
argv. redis-server parses everything after its config-file argument as
configuration directives and dies on the first one:

```
*** FATAL CONFIG FILE ERROR (Redis 7.2.15) ***
Reading the configuration file, at line 24
>>> '"earlycon=pl011,mmio32,0x0a001000"'
Bad directive or wrong number of arguments
```

mysqld shrugs these words off, which is why `../unikraft-mysql` boots its
server directly; redis cannot. The start script soaks them up as positional
parameters and `exec`s redis-server with a clean argv — one execve(), no fork,
enabled by `CONFIG_APPELFLOADER_MULTIPROCESS` exactly as in
`../unikraft-postgres`. The shell is the statically linked busybox (~1 MiB).

**Persistence stays off, and here it is load-bearing.** Upstream's config
already sets `appendonly no` / `save ""`; this port keeps it because the
alternative does not exist: BGSAVE and AOF rewrite both `fork()`, which
Unikraft does not have. The default save schedule would eventually kill the
server. The data directory is a ramfs anyway -- nothing written there survives
a reboot.

**`ignore-warnings ARM64-COW-BUG`.** On arm64, redis tests for the Linux
ARM64 copy-on-write bug by actually fork()ing — and treats the probe itself
failing as if the bug were present:

```
[libposix_process] CLONE_VM not set: Multiple address spaces are not supported
# Failed to test the kernel for a bug that could lead to data corruption
  during background save. ... Redis will now exit
```

The bug it guards against only matters during a background save, which the
persistence settings above make impossible, so the config tells redis to skip
the verdict. Ignored on x86_64.

## Layout

| file         | role                                                              |
|--------------|-------------------------------------------------------------------|
| `Dockerfile` | rootfs: `redis-server`, its libraries (via `ldd`), busybox, conf  |
| `redis.conf` | bind/protected-mode/persistence settings, commented               |
| `start.sh`   | argv filter: soaks up libkrun's junk, `exec`s the server          |
| `Kraftfile`  | the from-source base runtime + elfloader + MULTIPROCESS for exec  |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`              |
