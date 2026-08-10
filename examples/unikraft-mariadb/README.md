# unikraft-mariadb

[MariaDB](https://mariadb.org/) 11.4 (LTS), running as a Unikraft unikernel.
Ported from [`unikraft-cloud/examples`'s
`mariadb`](https://github.com/unikraft-cloud/examples/tree/main/mariadb) to
build for **arm64** as well as x86_64 and boot under bsdkrun -- and
restructured the same way `../unikraft-mysql` restructures upstream's mysql
example, because the two ports had the same problem: an entrypoint script that
is a process tree.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --port 3306:3306 \
  --cmdline "elfloader -- /bin/sh /start.sh"
```

```console
$ mysql --protocol=TCP -h 127.0.0.1 -P 3306 -u root -punikraft \
    -e "SELECT VERSION();"
```

The root password is `unikraft`, for `root@localhost` and `root@'%'` both.

## Status

**arm64 works.** The unikernel boots, DHCPs an address, mariadbd reaches
"ready for connections", and a real query round-trips over the forwarded
port:

```console
$ mariadb --skip-ssl -h 127.0.0.1 -P 3306 -u root -punikraft \
    -e "CREATE DATABASE IF NOT EXISTS e2e; USE e2e;
        CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY, name VARCHAR(64));
        REPLACE INTO t VALUES (1,'unikraft-on-bsdkrun');
        SELECT name FROM t WHERE id = 1;"
name
unikraft-on-bsdkrun
```

x86_64 has never been run; `.github/workflows/e2e-unikraft-examples.yml`
runs it as `strict: false` until its first green run.

Four distinct things had to be fixed to get there, each hidden behind the
last: an Aria recovery that wedges the boot, an argv the server refuses, a
transaction-coordinator log it cannot map, and certificates minted in 1970.
All four are described below. The Kraftfile also carries the two fixes
`../unikraft-mysql` needed -- `CONFIG_UKPLAT_PREEMPT` (InnoDB busy-waits for
a thread it has just created; a cooperative scheduler never runs it) and
`CONFIG_LIBPOSIX_PROCESS_MAX_PID=1024` (the default 31 TIDs run out before
the server finishes starting).

## Differences from upstream

**No entrypoint, no shell.** Upstream ships `docker-entrypoint.sh` plus bash,
gosu, awk, sed, find, pwgen and a dozen coreutils to run it at boot. That
script initialises the data directory by starting mariadbd, backgrounding it,
running SQL against it and shutting it down -- a process tree, and a unikernel
runs one program. This port moves all of it to image build time: the datadir
is created there by `mariadb-install-db`, and the network `root` account is
baked by a real server started on a unix socket and shut down cleanly. (Not
`mariadbd --bootstrap`: that mode implies skip-grant-tables, so the account
statements fail, and it leaves Aria's log unclean -- see below.) The guest
boots into `mariadbd` with no entrypoint, no gosu and no coreutils; the only
other program in the image is the busybox that filters argv.

**No `runtime: base-compat:latest`**, and **libraries resolved with `ldd`**
rather than listed -- same reasons as every other example here: the prebuilt
kernel and the hardcoded `/lib/x86_64-linux-gnu/...` paths are both
x86_64-only.

**The boot command is `sh /start.sh`, not mariadbd.** libkrun appends its own
words (`earlycon=...`, `tsi_hijack`, a bare `--`) to the end of the kernel
command line, past the `--` stop sequence, so they arrive in the application's
argv. mysqld ignores them — which is why `../unikraft-mysql` boots its server
directly — but mariadbd refuses to start:

```
mariadbd: Too many arguments (first extra is 'earlycon=pl011,mmio32,0x0a001000').
[ERROR] Aborting
```

The start script soaks them up as positional parameters and `exec`s mariadbd
with a clean argv — one execve(), no fork, enabled by
`CONFIG_APPELFLOADER_MULTIPROCESS`. The shell is the statically linked busybox
(~1 MiB). Same pattern as `../unikraft-redis` and `../unikraft-mongodb`.

**Aria ships with nothing to recover.** After the bootstrap server is shut
down cleanly, the Dockerfile zerofills the Aria system tables and deletes
`aria_log.*` and `aria_log_control`; a fresh server recreates both at startup.
Without this the guest wedges at boot in "Aria engine: starting recovery ...
transactions to roll back: 1" and spins forever in userspace — on a datadir
the same binary opens with no recovery at all on plain Linux. The syscall
trace shows mariadbd reading exactly the bytes the image contains, so the
divergence is behavioural rather than corruption; it is not understood yet,
and this sidesteps it.

**The transaction coordinator is the binary log, not a mapped file.** With
two 2PC-capable engines (InnoDB and Aria) and the binary log off, MariaDB
coordinates commits through `tc.log`, which it maps `MAP_SHARED` -- and
Unikraft has no shared file mappings:

```
mmap(addr=0, len=24576, prot=3, flags=1, fd=21, offset=0) failed: -95
[ERROR] Can't init tc log
[ERROR] Aborting
```

`log_bin` in `my.cnf` selects the binlog-based coordinator instead, which
only writes sequentially.

**TLS is off because the guest has no clock.** MariaDB 11.4 mints a
self-signed certificate at *startup* when none is configured. The guest boots
at the epoch, so that certificate is valid from 1970 to 1971 and every client
refuses it (`ERROR 2026 (HY000): TLS/SSL error: certificate has expired`) --
the server is healthy, but unreachable without `--skip-ssl`. An empty
`tls_version` disables it. Baking a certificate at image build time would be
the wrong fix: everyone pulling the image would share one private key.

**`innodb_use_native_aio = 0`** in `my.cnf`: the guest has neither io_uring
nor `io_submit()`. mariadbd links liburing regardless (so the library is in
the image); it just must never be asked to set a ring up.

**The data directory does not survive a reboot.** It lives in the ramfs the
embedded initrd is unpacked into, exactly as in `../unikraft-mysql`.

## Layout

| file         | role                                                                |
|--------------|---------------------------------------------------------------------|
| `Dockerfile` | bakes the datadir + root account, assembles rootfs via `ldd`        |
| `my.cnf`     | unikernel constraints (no native AIO, no DNS) and memory bounds     |
| `start.sh`   | argv filter: soaks up libkrun's junk, `exec`s mariadbd              |
| `Kraftfile`  | base runtime + preemption/TID fixes + MULTIPROCESS + elfloader      |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`                |
