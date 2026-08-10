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

Untested as of this writing. x86_64 runs in
`.github/workflows/e2e-unikraft-examples.yml` as `strict: false` until it has
its first green run; arm64 is exercised by hand on macOS/Hypervisor.framework.

The odds are decent: mariadbd ships the same InnoDB as the mysqld that already
works in `../unikraft-mysql`, and the Kraftfile carries the same two fixes
that example needed -- `CONFIG_UKPLAT_PREEMPT` (InnoDB busy-waits for a thread
it has just created; a cooperative scheduler never runs it) and
`CONFIG_LIBPOSIX_PROCESS_MAX_PID=1024` (the default 31 TIDs run out before the
server finishes starting).

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
