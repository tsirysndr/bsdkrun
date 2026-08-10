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
  --cmdline "elfloader -- /usr/sbin/mariadbd --user=root"
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
baked with `mariadbd --bootstrap` (the same no-network stdin mechanism
`mariadb-install-db` itself uses). The guest boots straight into `mariadbd`.

**No `runtime: base-compat:latest`**, and **libraries resolved with `ldd`**
rather than listed -- same reasons as every other example here: the prebuilt
kernel and the hardcoded `/lib/x86_64-linux-gnu/...` paths are both
x86_64-only.

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
| `Kraftfile`  | the from-source base runtime + preemption/TID fixes + elfloader     |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`                |
