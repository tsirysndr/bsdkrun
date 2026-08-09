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

> Needs a libkrun new enough to honour `KRUN_NO_EARLYCON`. Without it libkrun
> appends its `earlycon=` hint *after* the `--` stop sequence, so it lands in
> mysqld's argv and mysqld — unlike node or bun — refuses to start on an
> argument nobody wrote: `Too many arguments (first extra is 'earlycon=...')`.

## Status

**It works.** The unikernel boots, `mysqld` starts, and it serves SQL over a
forwarded port:

```console
$ mysql -h 127.0.0.1 -P 3306 --protocol=TCP --ssl-mode=DISABLED -u root -punikraft \
    -e "create database demo; use demo;
        create table t (id int primary key, name varchar(32));
        insert into t values (1,'unikraft'),(2,'bsdkrun');
        select * from t;"
id      name
1       unikraft
2       bsdkrun
```

Verified on arm64 under macOS/Hypervisor.framework. x86_64 is covered by
`.github/workflows/e2e-unikraft-examples.yml`, which runs the same round trip.

Getting there needed three fixes in the guest kernel, none of them in this
example. Each was hidden behind the one before it.

### 1. InnoDB deadlocks a cooperative scheduler

`IB_thread::start()` busy-waits for the thread it has just created, in a
six-instruction loop with no yield and no syscall in it. Unikraft's scheduler is
cooperative, so the thread it waits for can never run: the boot stops dead at
`InnoDB initialization has started`, at 100% CPU, forever, on both
architectures.

Fixed by `preempt.patch` in `../../library/unikraft-base/patches`, which
preempts application code on a timer tick. `repro/` has the gdb session that
found it and the reasoning behind the patch. With it, InnoDB initialisation
takes **0.3 s**.

### 2. Thirty-one threads

`CONFIG_LIBPOSIX_PROCESS_MAX_PID` defaults to 31, and `find_and_reserve_tid()`
returns `EAGAIN` past that. InnoDB spends a dozen threads on page cleaners,
purge, IO and monitors before a client ever connects, so mysqld died with

```
[ERROR] [MY-010106] [Server] Can't create interrupt-thread (error 11, errno: 11)
```

The Kraftfile raises it to 1024. Every connection is another thread.

### 3. `ppoll()` refused a signal mask

`uk_sys_ppoll()` returned `ENOSYS` for any non-NULL `sigmask`. MySQL's
connection layer does a non-blocking read, gets the expected `EAGAIN`, and waits
for the socket to become readable — so the wait failed, and mysqld hung up on
the client mid-handshake having already sent its greeting:

```
sendto(... "8.0.46" ...)  = OK            <- greeting
recvfrom(..., MSG_DONTWAIT) = EAGAIN      <- expected
ppoll(...)                 = ENOSYS       <- the bug
[Note] Got an error reading communication packets
```

This one is not MySQL-specific: **aarch64 has no `poll` syscall**, so glibc's
`poll()` is always `ppoll()`. Every glibc program that polls a file descriptor
hits it; node, Deno and Actix escape only by using `epoll` directly.

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
| `event_scheduler = OFF`     | One thread fewer; its creation failure is fatal rather than degrading |
| `auto_generate_certs = OFF` | Regenerating the deleted `*.pem` needs `chmod`, which ramfs lacks     |

TLS is off (`tls_version =`, which is how 8.0.26+ spells it) and the generated
`*.pem` are deleted from the datadir. A server generates those on first start;
here that would be build time, so every user of the image would share one
private key. Put TLS in front of the guest instead.

## Memory

**`--mem 2048`.** The root filesystem is 152 MiB — a 53 MiB `mysqld`, 11 MiB of
libraries, and an 87 MiB initialised datadir — and it is resident twice at boot:
embedded in the kernel image *and* unpacked into ramfs. 2048 is what every run
here used; the floor has not been measured.

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
