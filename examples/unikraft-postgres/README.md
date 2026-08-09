# unikraft-postgres

**PostgreSQL 16.4 as a Unikraft unikernel**, ported from
[`unikraft-cloud/examples`'s `postgres`](https://github.com/unikraft-cloud/examples/tree/main/postgres)
to build for **arm64** and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 \
  --cmdline "elfloader -- /usr/local/bin/postgres --single -D /var/lib/postgresql/data"
```

```console
PostgreSQL stand-alone backend 16.4
backend> LOG:  checkpoint starting: shutdown immediate
LOG:  checkpoint complete: wrote 3 buffers (0.1%); ... lsn=0/147F5B8
```

## Status

**The server starts, opens the cluster and answers as a stand-alone backend.**
Getting there took five fixes, each described below; two problems remain, both
outside PostgreSQL.

| what                                     | state                                       |
|------------------------------------------|---------------------------------------------|
| PostgreSQL 16.4 builds and boots (arm64) | works                                       |
| Cluster mounts, recovers, checkpoints    | works                                       |
| SQL typed at the console                 | **untested** — no TTY in this session       |
| SQL piped in (`< demo.sql`)              | **does not work** — the guest reads EOF     |
| A postmaster listening on 5432           | **impossible** — no `fork()`                |

arm64 on macOS/Hypervisor.framework is what was tested. x86_64 is not built
here; it goes through the e2e workflow.

## No fork, so no postmaster

Upstream's Kraftfile names `runtime: base-compat:latest`, a Unikraft Cloud
runtime with real processes. On stock Unikraft there is no `fork()` at all —
`lib/posix-process/clone.c` rejects any clone that does not share the address
space:

```c
if (unlikely(!(flags & CLONE_VM))) {
        uk_pr_err("CLONE_VM not set: Multiple address spaces are not supported\n");
        return -ENOTSUP;
}
```

`vfork()` exists (`CLONE_VM | CLONE_VFORK`, for `exec`), and `CONFIG_LIBPOSIX_-
PROCESS_MULTIPROCESS` adds processes that share the one address space — neither
is `fork()`, and PostgreSQL needs the real thing. The postmaster forks the
checkpointer, the background writer, the WAL writer and the autovacuum launcher
*before* it accepts a connection, then a backend per client. None of that is
optional and none of it can be configured away.

What is left is **single-user mode**: `postgres --single` is the whole server in
one process, reading SQL from stdin, with no postmaster, no listener and no
forks. That is the shape this example takes, and it is why the cluster is
initialised at image build time — `initdb` is itself multi-process (it runs
`postgres --boot` and `postgres --single` as children), so it could not run in
the guest even if there were a shell to start it from.

## What it took to get there

Five things, in the order they were hit. None of them is about PostgreSQL being
unusual; each is a place where the guest is not Linux.

### 1. The root filesystem may not contain hard links

`ramfs_link` in Unikraft is literally `vfscore_vop_eperm` — the filesystem the
image is unpacked into has no `link()`. PostgreSQL's bundled timezone database
is built almost entirely out of hard links, so the boot dies in the extractor:

```
ERR:  [libukcpio] Failed to create new hard link
      /./usr/local/share/postgresql/timezone/Africa/Accra
      (from /./usr/local/share/postgresql/timezone/Africa/Abidjan).
CRIT: [libvfscore] Failed to extract cpio archive to /: -11
```

`build.sh` replaces each one with a private copy (about 500 KiB in total). It
has to do that **after** `docker export`, not in the Dockerfile: BuildKit
deduplicates identical files into hard links as it writes each `COPY`, so a tree
that leaves the build stage with none arrives in the image with 245.

### 2. `signalfd` cannot be built on arm64 (upstream fix)

PostgreSQL 13 and later read signals through a file descriptor
(`WAIT_USE_SIGNALFD` in `src/backend/storage/ipc/latch.c`), and it is not a
fallback — without it the backend prints `FATAL: signalfd() failed` and exits.

Turning `CONFIG_LIBPOSIX_PROCESS_SIGNALFD` on does not build:

```
uk/bits/syscall_provided.h:898:2: error: #error Failed to map system call
'signalfd': No system call number available
```

`lib/posix-process` defines both `signalfd4()` and the three-argument legacy
`signalfd()`, but the shim's legacy list never mentions the latter, and arm64 is
one of the architectures that has no `__NR_signalfd`. The list exists for
exactly this case — `eventfd`, `epoll_create`, `poll` and a dozen others are
already on it — so the fix is the missing line, added as **patch 16** in
`../../library/unikraft-base/patches/apply.sh`.

### 3. `RLIMIT_STACK` describes the wrong stack

`getrlimit(RLIMIT_STACK)` answers `__STACK_SIZE`, the *thread* stack size, while
the application runs on the stack app-elfloader allocated for it. They are
unrelated numbers and only one is reported. PostgreSQL subtracts a fixed 512 KiB
of slop from it, and refuses to start if what is left is under `max_stack_depth`
— whose minimum is 100 KiB:

```
LOG:  invalid value for parameter "max_stack_depth": 100
DETAIL:  "max_stack_depth" must not exceed -448kB.
FATAL:  failed to initialize max_stack_depth to 100
```

-448 KiB is 64 KiB (`CONFIG_STACK_SIZE_PAGE_ORDER: 4`) minus the slop, so no
value of `max_stack_depth` is accepted. The Kraftfile raises both stacks to
2 MiB — the reported one *and* the real one, since raising only the report hands
PostgreSQL a limit it could overrun.

> `kraft` will not change a symbol in an already-generated `.config`. After
> editing kconfig in the Kraftfile, `rm .config.postgres_fc-*` or the build
> silently keeps the old values.

### 4. No System V IPC (`no-sysv-ipc.patch`)

```
FATAL:  could not create shared memory segment: Function not implemented
DETAIL:  Failed system call was shmget(key=749, size=56, 03600).
```

`shared_memory_type = mmap` is not enough: that moves the *real* shared memory
to an anonymous mapping but PostgreSQL still creates a 56-byte SysV segment,
whose only job is to stop a second postmaster from attaching to the same data
directory. A unikernel cannot have a second postmaster, so the patch hands back
anonymous memory when `shmget()` answers `ENOSYS` and the interlock becomes
vacuous rather than broken.

### 5. No file-backed `MAP_SHARED` (`dsm-private-mmap.patch`)

```
ERR: [libposix_mmap] mmap(addr=0, len=8192, prot=3, flags=1, fd=5, offset=0) failed: -95
FATAL:  could not map shared memory segment "pg_dynshmem/mmap.3858750128": Not supported
```

Unikraft supports `MAP_SHARED` for anonymous mappings only, and dynamic shared
memory maps a file. It cannot be switched off either — PostgreSQL 12 removed
`dynamic_shared_memory_type = none`, and the control segment is created during
startup whether or not anything uses it. The patch maps those segments
`MAP_PRIVATE`: the only thing `MAP_SHARED` buys is coherence with other
processes mapping the same segment, and there are none.

## Two things that still do not work

**The VMM appends words to the application's argv.** libkrun adds its own tokens
to the end of the kernel command line, which is *after* the `--` stop sequence —
so they are not kernel parameters, they are the last words of `argv`. Three
sources, all in libkrun: the `earlycon=` console hint, `tsi_hijack` from the
default TSI vsock, and a bare `--` from
`epilog: Some(format!(" -- {}", ctx_cfg.get_args()))`. node and bun carry the
extra word without noticing; PostgreSQL validates its own argv:

```
FATAL:  postgres: invalid command-line argument: earlycon=pl011,mmio32,0x0a001000
FATAL:  postgres: invalid command-line argument: --
```

`bsdkrun` now sets `KRUN_NO_EARLYCON=1` for unikraft guests (it already did for
OSv), which removes the first — but that gate only exists in a libkrun newer
than the installed dylib, and the other two have no gate at all. The workaround
in the command above is to **omit the database name**, so the stray word is
absorbed: `getopt` swallows a bare `--`, and the database then defaults to the
user's name. It is a coincidence, not a design; the real fix is for libkrun to
leave the command line alone when the payload is an explicitly-set kernel.

**Console input never reaches the guest.** Single-user mode reads SQL from
stdin, and a piped stdin does not arrive — the backend reads EOF immediately,
prints one `backend>` prompt and shuts the cluster down cleanly:

```console
PostgreSQL stand-alone backend 16.4
backend> LOG:  checkpoint starting: shutdown immediate
```

That is with data already waiting on the pipe and the pipe held open, so it is
not a race. Typing at a real terminal was not tested here (this session has no
TTY) and may well work — libkrun puts the controlling TTY into raw mode, which
is why `src/tty.rs` exists. `demo.sql` is there for when it does.

## Differences from upstream

**No `runtime: base-compat:latest`.** That runtime is Unikraft Cloud's and is
not published for arm64, so this Kraftfile builds `library/base` from source
like the other examples here. The only kconfig this example adds to that base is
`CONFIG_LIBPOSIX_PROCESS_SIGNALFD` and the two stack sizes.

**No `wrapper.sh`.** Upstream boots into a bash port of docker-library's
entrypoint, which runs `initdb`, starts a temporary server through `pg_ctl`,
creates the database with `psql`, then `exec`s postgres. Every step is a
separate process. Here `initdb` runs in the Dockerfile and the finished data
directory is baked into the image, so the guest starts where upstream's wrapper
finishes.

**No `pg_ukc_scaletozero`.** It is a Unikraft Cloud scale-to-zero integration
and has nothing to do with booting.

**A much smaller build.** ICU, readline, zlib, lz4, libxml and libxslt are all
configured out — about 40 MiB, doubled, because a Unikraft root filesystem is
resident twice (embedded in the kernel image and unpacked into ramfs). The image
is still 55 MiB, 37 MiB of which is the cluster.

**`allow-root.patch` is upstream's**, trimmed to the two programs this image
ships. A unikernel has one user, uid 0, and nothing to drop privileges to.

## Memory

`--mem 2048`. The rootfs is resident twice before `shared_buffers` is allocated;
the defaults in `postgresql.conf` are trimmed to match (32 MiB buffers, 10
connections, all worker processes off — those are forks).

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel. The
format is `<argv0> -- <application argv>`; everything before `--` is parsed as
kernel library parameters and the first word is skipped, so the leading
placeholder is not optional.
