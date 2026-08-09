# unikraft-mysql

[MySQL](https://www.mysql.com) 8.0 as a Unikraft unikernel. Ported from
[`unikraft-cloud/examples`' `mysql`](https://github.com/unikraft-cloud/examples/tree/main/mysql)
to build for **arm64** and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --port 3306:3306 \
    --cmdline "elfloader -- /usr/sbin/mysqld --user=root"
```

```console
$ mysql -h 127.0.0.1 --ssl-mode=DISABLED -u root -punikraft -e 'select version()'
```

Credentials are baked into the image: `root` / `unikraft`, reachable from any
host.

## Status

> **This one does not work yet.** It builds and boots, and `mysqld` gets a long
> way into InnoDB startup, but the guest wedges the moment MySQL creates its
> *second* thread. Everything below is accurate about where it stops and what
> has been ruled out.

### What works

The unikernel builds for `fc/arm64`, boots, DHCPs an address, and the binary
runs — `mysqld --version` completes in the guest:

```console
$ bsdkrun unikraft . --mem 2048 --no-net --cmdline "elfloader -- /usr/sbin/mysqld --version"
...
/usr/sbin/mysqld  Ver 8.0.46 for Linux on aarch64 (MySQL Community Server - GPL)
```

So the ELF loader, glibc, `libstdc++`, TLS and the RPATH-resolved private
libraries are all fine. `mysqld` then starts for real, finds and parses
`/etc/my.cnf`, and reaches:

```
[System] [MY-010116] [Server] /usr/sbin/mysqld (mysqld 8.0.46) starting as process 1
[System] [MY-013576] [InnoDB] InnoDB initialization has started.
```

The same root filesystem, run as a plain container (`docker run mysql-rootfs
/usr/sbin/mysqld --user=root`), serves queries — so the image itself, the baked
datadir and the credentials are all good. The problem is in the guest.

### Where it stops

That last log line understates the progress. With syscall tracing on (see the
Kraftfile for how to actually enable it), InnoDB opens and reads its whole
tablespace set first:

```
openat("./ibdata1", O_RDWR)                    = fd
openat("/var/lib/mysql/#innodb_redo", ...)     = fd
openat("./mysql.ibd") ... pread64(..., 16384, 0) = 16384
openat("./undo_001") ... openat("./undo_002")    both read
openat("/tmp/ibw9iB1W", O_RDONLY|O_CREAT|O_EXCL) = fd     <- temp files
openat("/sys/devices/system/cpu/online")       = ENOENT   ┐ glibc get_nprocs()
openat("/proc/stat")                           = ENOENT   ┘
sched_getaffinity(...)                         = 0x1000
mmap(196608, PROT_NONE, MAP_PRIVATE|MAP_ANONYMOUS|MAP_STACK) = va   <- a stack
clone(CLONE_VM|CLONE_FS|CLONE_FILES|...)  ->  1  rt_sigprocmask(...) = 0x0
```

and then **nothing, ever again** — from either thread — while the vCPU sits at
100%. Left alone for nine minutes it produces not one more line, so this is a
hang and not slow demand-paging of the 152 MiB root filesystem.

The last two lines are the whole finding. The child thread is created, gets as
far as the `rt_sigprocmask` that glibc's `start_thread` does before calling the
thread function, and from that point no thread in the guest issues another
system call.

### It is the *second* thread, not threading in general

Threads work. The trace contains exactly two `clone` calls, and the first one
succeeds completely: that thread is the one that prints "InnoDB initialization
has started" and performs every tablespace open above. Only the second `clone`
wedges the guest.

### Ruled out, with evidence

| hypothesis                            | test                                                                       | result                        |
|---------------------------------------|----------------------------------------------------------------------------|-------------------------------|
| One vCPU starves a spinning thread     | `--cpus 4`                                                                  | wedges identically            |
| Slow demand paging, not a hang         | left running 9 minutes                                                      | no further output at all      |
| InnoDB's spin-wait loops               | `--innodb-spin-wait-delay=0 --innodb-sync-spin-loops=0`                     | wedges identically            |
| The dedicated redo-log threads         | `--innodb-log-writer-threads=OFF`                                           | wedges identically            |
| Too many background threads            | `--innodb-page-cleaners=1 --innodb-purge-threads=1`, 2 IO threads           | same trace, same second clone |
| The binary or its libraries            | `mysqld --version` in the guest; whole rootfs under Docker                  | both fine                     |
| The image, datadir or credentials      | `select version()` against the `scratch` image over TCP                     | answers                       |

### Leading explanation

Unikraft's scheduler is cooperative — the built config has
`CONFIG_LIBUKSCHEDCOOP=y`, and `lib/` contains no other scheduler — so a thread
that spins in userspace without entering the kernel can never be preempted. If
the new thread spins on a lock the other thread holds, nothing can ever run
again: 100% CPU, no syscalls, forever. That also explains why `--cpus 4` makes
no difference, since the guest does not use the extra vCPUs.

This is consistent with the evidence but not proven; the alternative is a bug in
`clone`/TLS setup that only shows on a second concurrent thread.

### CI

`.github/workflows/e2e-unikraft-examples.yml` builds and boots this example on
x86_64 alongside the others, with `strict: false` — the payoff check runs and is
reported but does not fail the job. That is how it gets established whether the
second-thread hang is architecture-independent, the same way the bun entry
established that bun's abort is.

Its check is not HTTP. A MySQL server sends its handshake packet unprompted on
connect, so the step reads the first bytes off port 3306 with bash's `/dev/tcp`
and looks for the version string — no client package on the runner.

Everything before that — the build, the boot, the kernel banner, the network
interface, entropy — is asserted for this example exactly as for the working
ones, and all of it passes today.

### Next step

The gdb stub, the way `../unikraft-expressjs/repro/` pinned down the node
failures: build a `qemu/arm64` target, attach `gdb-multiarch` through QEMU's
`-S -gdb tcp:...`, and look at what both threads are doing once the console goes
quiet — which lock, held by whom. Everything cheaper than that has been tried.

## Differences from upstream

Upstream targets Unikraft Cloud, which supplies a prebuilt `base-compat:latest`
runtime, an EROFS root filesystem, and a volume. None of those are available
here, and each one changed something.

### The entrypoint is gone; the database is initialised at build time

Upstream boots into `wrapper.sh`, which execs `bash -x docker-entrypoint.sh
mysqld ...`. That script is a process tree: it runs `mysqld --initialize` once,
starts a temporary server in the background, drives it with `mysql` and
`mysqladmin`, drops privileges with `gosu`, and shells out to `awk`, `sed`,
`find` and `chown` along the way. Hence upstream's Dockerfile copying `/bin/sh`,
`/bin/bash`, `gosu` and a dozen coreutils into the image.

A unikernel runs one program. This example does all of that in the `Dockerfile`,
against the build container's own MySQL, and ships the finished data directory:

* `mysqld --initialize-insecure` creates the system tables,
* a server is started on a unix socket with `--init-file` to give `root` a
  password and a `'root'@'%'` grant, then shut down again,
* the result is copied into the root filesystem as `/var/lib/mysql`.

So the image contains no shell, and the guest's first boot is the same as its
hundredth: `mysqld` opens an already-initialised datadir. That also takes
initialisation out of the measured boot time, which is the interesting number
for a database in a unikernel.

### `mysql:8.0`, not `mysql:8.0-debian`

The Debian-flavoured tag is published for amd64 only. The default Oracle Linux
one has an arm64 manifest, and this example has to build for both.

### The libraries are resolved, not listed

Upstream hardcodes `/lib/x86_64-linux-gnu/...`; on arm64 those paths do not
exist. `ldd` output is copied instead — the same approach as
`../unikraft-expressjs`.

One entry is worth knowing about: `libprotobuf-lite` is found through mysqld's
`RPATH` and `ldd` prints it as `/usr/sbin/../lib64/mysql/private/...` — an
absolute path with a `..` in the middle. Copying it verbatim is exactly right,
because it lands where the same `RPATH` will look in the guest.

### Configuration lives in `/etc/my.cnf`

bsdkrun passes the application's `argv` through `--cmdline`, and upstream's
server options run to eight flags. They are in the image's `my.cnf` instead, so
the command line stays short. Four of them are not tuning but constraints:

| setting                     | why                                                                 |
|-----------------------------|---------------------------------------------------------------------|
| `innodb_use_native_aio = 0` | No `io_setup`/`io_submit` in the guest; InnoDB does not fall back    |
| `skip_name_resolve = ON`    | lwip is built without DNS, so a client lookup would stall on timeout |
| `mysqlx = OFF`              | A second listener on port 33060 that nothing here connects to        |
| `performance_schema = OFF`  | Its instrumentation tables reserve ~100 MiB at startup               |

TLS is off (`tls_version =`, which is how 8.0.26+ spells it) and the generated
`*.pem` are deleted from the datadir. A server generates those on first start;
here that would be build time, so every user of the image would share one
private key. Put TLS in front of the guest instead.

## Memory

**`--mem 2048`.** The root filesystem is 152 MiB — a 53 MiB `mysqld`, 11 MiB of
libraries, and an 87 MiB initialised datadir — and it is resident twice at boot:
embedded in the kernel image *and* unpacked into ramfs. 2048 is what every run
here used; the floor has not been measured, because the hang above made it a
pointless thing to bisect.

If a smaller `--mem` is tried, note that running out during cpio extraction
reports itself as I/O, not memory (see `../unikraft-expressjs/README.md`):

```
ERR: [libukcpio] ...: Failed to load content: Input/output error (5)
```

The datadir is the part worth trimming, and most of it is fixed InnoDB overhead:
29 MiB `mysql.ibd`, 2 × 16 MiB undo tablespaces, 12 MiB `ibdata1`, 8.6 MiB of
doublewrite buffer. `innodb_redo_log_capacity` is already cut from 100 MiB to
8 MiB at initialisation.

## The data does not persist

The root filesystem is a ramfs unpacked from the kernel image, so every write —
every `INSERT` — is gone at shutdown, and each boot starts from the datadir as
it was built. Upstream solves this with a Unikraft Cloud volume mounted at
`/var/lib`.

The equivalent here is `bsdkrun unikraft --mount`, which shares a host directory
over virtio-fs; see `../unikraft-volume`. It needs a kernel built with
`CONFIG_LIBUKFS_VIRTIOFS` and `CONFIG_LIBPOSIX_VFS_FSTAB_USER`, which this
Kraftfile does not enable, and InnoDB on virtio-fs is its own question
(`O_DIRECT`, file locking, `fsync` semantics). Read-only or scratch use is what
this example covers.

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel, so the
program has to be named explicitly. The format is

```
<argv0> -- <application argv>
```

Everything before `--` is parsed as kernel library parameters and **the first
word is skipped** (Unikraft treats it as the program name), so the leading
placeholder is not optional.
