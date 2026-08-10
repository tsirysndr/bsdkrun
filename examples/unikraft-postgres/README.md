# unikraft-postgres

**PostgreSQL 16.4 as a Unikraft unikernel**, ported from
[`unikraft-cloud/examples`'s `postgres`](https://github.com/unikraft-cloud/examples/tree/main/postgres)
to build for **arm64** and boot under bsdkrun — with a **real postmaster**
serving SQL over TCP, not single-user mode.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --port 5432:5432 \
  --cmdline "elfloader -- /usr/local/bin/postgres -D /var/lib/postgresql/data"
```

```console
$ psql -h 127.0.0.1 -p 5432 -U postgres postgres
postgres=# CREATE TABLE t (id INT PRIMARY KEY, name TEXT);
postgres=# INSERT INTO t VALUES (1, 'unikraft-on-bsdkrun');
postgres=# SELECT name FROM t;
        name
---------------------
 unikraft-on-bsdkrun
(1 row)
```

## Status

| what                                              | state    |
|---------------------------------------------------|----------|
| Builds and boots (arm64)                          | works    |
| Postmaster starts and listens on 5432             | works    |
| Spawns child processes (`posix_spawn` + `execve`) | works    |
| WAL recovery, checkpointer, background writer     | works    |
| Clients over the forwarded port, one backend each | works — 10/10 sequential connections served |
| `postgres --single` (SQL on the console)          | works    |

arm64 on macOS/Hypervisor.framework is what was tested; x86_64 goes through
`.github/workflows/e2e-unikraft-examples.yml` (non-strict until it passes, and
the workflow symbolicates any guest crash against the built image's `.dbg`).

## How a forking server runs on a kernel with no fork

Upstream's Kraftfile names `runtime: base-compat:latest`, a Unikraft Cloud
runtime. This example runs on stock Unikraft, where there is no `fork()` at all
— `lib/posix-process/clone.c` rejects any clone that does not share the address
space, because there is only ever one address space:

```
ERR: [libposix_process] CLONE_VM not set: Multiple address spaces are not supported
```

What Unikraft *does* have is `vfork()` + `execve()`, and its own documentation
points at the way through
([`lib/posix-process/README.md`](https://github.com/unikraft/unikraft/blob/staging/lib/posix-process/README.md)):

> Fortunately it is common that applications spawn new processes by calling
> `fork()` immediately followed by an `execve()` […] Applications can use the
> `posix_spawn()` libc function that spawns a process using `vfork()` in a safe
> way.

PostgreSQL already has exactly that mode. **`EXEC_BACKEND`** — what it uses on
Windows, and supports on Unix for testing — makes the postmaster fork and
immediately `exec` `postgres --forkchild` for *every* child, handing the child
its state through a file instead of through inherited memory. Every
`fork_process()` call site in the tree is inside `#else /* !EXEC_BACKEND */`, so
building `-DEXEC_BACKEND` leaves none behind, and
`exec-backend-posix-spawn.patch` turns the one remaining fork+exec into the
`posix_spawn()` that Unikraft supports.

So the shape is: `CONFIG_APPELFLOADER_MULTIPROCESS` on the kernel side (which
pulls in multiprocess, signals and an init process), `-DEXEC_BACKEND` plus four
small patches on the PostgreSQL side. The cluster is still initialised at image
build time, because `initdb` runs `postgres --boot` and `postgres --single` as
children and there is no reason to do at boot what can be done once.

## What it took: five Unikraft fixes

All are in `../../library/unikraft-base/patches/apply.sh`, and none of them is
about PostgreSQL being unusual — each is a place where the guest is not Linux.
Two of them mean **`execve()` had never worked on arm64 at all**.

| #  | what was wrong |
|----|----------------------------------------------------------------------|
| 16 | `signalfd` is defined but not on the shim's legacy list, and arm64 has no `__NR_signalfd` — so `CONFIG_LIBPOSIX_PROCESS_SIGNALFD` could not be built. PostgreSQL 13+ reads signals through a file descriptor and exits with `FATAL: signalfd() failed` without it. |
| 18 | `execve()` sets every field of the binfmt loader's argument struct **except `argc`/`envc`**, which it leaves as stack garbage. A loader that believes them walks off the end of `argv` and dereferences whatever it finds. Nothing in-tree reads those counts, so upstream never noticed. |
| 19 | **arm64 `execve()` enters the new program at the wrong register.** It writes the entry point to `LR`, but `ukarch_execenv_load()` restores `ELR_EL1` from the `PC` slot and leaves through `eret` — so the new program started at whatever was in freshly allocated stack memory, i.e. `0`. x86_64 sets `RIP` correctly, which is why an x86_64-only upstream never saw it. |
| 20 | **A signal the current thread has blocked is run in it anyway.** Process-directed delivery looks for *any* thread that does not block the signal, then calls `do_deliver()` — which builds the frame on the *current* context. PostgreSQL blocks every signal while it installs handlers and only then initialises the latch they use, so a `SIGCHLD` arriving in that window ran `handle_pm_child_exit_signal` → `SetLatch(NULL)`. |
| 21 | `setsid()` was a flat `return -EPERM` ("we have a single session with a single process" — written before multiprocess existed). Every PostgreSQL child calls it and dies with `FATAL: setsid() failed`. The neighbouring `getsid()` already reports `UNIKRAFT_SID` to anyone who asks, so refusing was inconsistent as well as fatal. |

Findings 19 and 20 are worth restating: any multiprocess application on arm64
hits 19 on its first `execve()`, and any daemon that blocks signals around its
own startup hits 20.

## What it took: four PostgreSQL patches

| file | why |
|-------------------------------|--------------------------------------------|
| `allow-root.patch`            | Upstream's, trimmed to the two programs this image ships. A unikernel has one user, uid 0, and nothing to drop privileges to. |
| `no-sysv-ipc.patch`           | There is no System V IPC, so `shmget()` answers `ENOSYS`. Anonymous memory is not a downgrade here: one page table means an anonymous mapping is shared between the postmaster and its children by construction, at the same address — which is exactly what the segment was for. The second hunk is the child side, where re-attaching is a no-op. |
| `dsm-single-address-space.patch` | Dynamic shared memory cannot be turned off (PostgreSQL 12 removed `none`) and maps a *file* `MAP_SHARED`, which Unikraft supports only for anonymous mappings. The rewrite demotes the file from storage to a name: it records the address of an anonymous mapping, and attaching becomes a lookup. |
| `exec-backend-posix-spawn.patch` | The fork+exec in `internal_forkexec()` becomes one `posix_spawn()`. Hand-writing `vfork()` here would be undefined behaviour — `fork_process()` assigns to globals that would land in the *parent's* memory — and musl's `posix_spawn()` is precisely this sequence done safely. |
| `exec-backend-self-path.patch`   | The postmaster execs *itself*, so skip `find_other_exec()` — which locates a `postgres` beside argv[0] and version-checks it by running `"<path>" -V` through `popen()`: a `/bin/sh` plus a second postgres before the server has done anything. In a unikernel there is exactly one postgres and it is the one running. This is also what unblocked startup: with those two extra process lifetimes gone, the startup child no longer hits a heap-consistency abort during WAL recovery (see below). |

Three more things are configuration rather than patches, all in the
`Dockerfile`:

- **No Unix-domain socket.** Binding one ends in `chmod()`, which the guest does
  not implement (`could not create any Unix-domain sockets`). Clients arrive
  over the forwarded TCP port anyway.
- **`max_stack_depth = 1MB`**, against the 2 MiB stacks the Kraftfile sets — see
  below.
- **Background workers off.** Each one is another process to spawn; the aux
  processes the postmaster always starts are not covered by those settings and
  do run.

## Other things worth knowing

**The root filesystem may not contain hard links.** `ramfs_link` is literally
`vfscore_vop_eperm`, and PostgreSQL's bundled timezone database is built almost
entirely out of them, so the cpio extractor stops the boot dead. `build.sh`
replaces each with a private copy — and has to do it **after** `docker export`,
because BuildKit deduplicates identical files into hard links as it writes each
`COPY`: a tree that leaves the build stage with none arrives in the image with
245.

**`RLIMIT_STACK` describes the wrong stack.** `getrlimit()` answers
`__STACK_SIZE`, the *thread* stack size, while the application runs on the stack
app-elfloader allocated for it. PostgreSQL subtracts a fixed 512 KiB from the
reported value and refuses to start if what is left is under `max_stack_depth`,
whose minimum is 100 KiB — so with the default 64 KiB no value is accepted:

```
DETAIL:  "max_stack_depth" must not exceed -448kB.
```

The Kraftfile raises both stacks to 2 MiB: the reported one *and* the real one,
since raising only the report hands PostgreSQL a limit it could overrun.

> `kraft` will not change a symbol in an already-generated `.config`. After
> editing kconfig in the Kraftfile, `rm .config.postgres_fc-*` or the build
> silently keeps the old values.

**There is still a shell in the image** even though nothing invokes it at boot
any more (`exec-backend-self-path.patch` removed the `popen` that needed it) —
it stays because archive recovery and `COPY FROM PROGRAM` also go through
`/bin/sh`, and at 800 KiB busybox is cheap. It links against this image's musl
and — the part that matters — is a PIE, which `execve()` requires on a
single-address-space kernel: the loader has to place it somewhere other than
where the caller is running.

**libkrun appends words to the application's argv** (`earlycon=`, `tsi_hijack`,
a bare `--`), because they land after the `--` stop sequence. It does not bite
here: they arrive after `-D <dir>`, and PostgreSQL's `getopt` consumes the `--`.
It does bite `postgres --single`, which takes a database name as a non-option
argument — omit the name there and the stray word is absorbed instead.

## What is left

**One unexplained kernel behaviour, worked around rather than root-caused.**
With the `popen("postgres -V")` version check still in place, a *later* exec'd
child (the startup process) died in musl's allocator — `mov x0,#0; strb wzr,[x0];
brk #0x3e8`, mallocng's deliberate abort on a failed heap-consistency check — a
few syscalls into WAL recovery. Two extra process lifetimes before it were
enough to corrupt state somewhere; ten backend lifetimes after it are not. The
difference between those two shapes (a pipe and a `pclose`/`wait`, versus plain
spawn-and-exit) is where the remaining bug lives, and it is in the kernel, not
in PostgreSQL. `exec-backend-self-path.patch` removes the trigger and is the
right change on its own merits; the underlying suspect is tracked separately.

**x86_64 crashes earlier**, at a kernel-mode fault inside a `read()` — quite
possibly the same popen-shaped trigger, since the postmaster reads the version
string back over a pipe. The e2e job now symbolicates guest crashes, so the next
CI run answers this.

`postgres --single` — the whole server in one process, SQL on stdin, no
postmaster and no children — also works on the same image, from a terminal
(piped stdin does not reach the guest; the console reads EOF immediately).

## Differences from upstream

**No `runtime: base-compat:latest`** — that runtime is Unikraft Cloud's and is
not published for arm64, so this Kraftfile builds `library/base` from source
like the other examples here, plus multiprocess, signalfd and larger stacks.

**No `wrapper.sh`** — upstream boots into a bash port of docker-library's
entrypoint, which runs `initdb`, starts a temporary server through `pg_ctl` and
creates the database with `psql`. All of that happens in the Dockerfile here, so
the guest starts where upstream's wrapper finishes.

**No `pg_ukc_scaletozero`** — a Unikraft Cloud scale-to-zero integration, with
nothing to do with booting.

**A much smaller build** — ICU, readline, zlib, lz4, libxml and libxslt are
configured out, about 40 MiB, doubled: a Unikraft root filesystem is resident
twice (embedded in the kernel image and unpacked into ramfs).

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel. The
format is `<argv0> -- <application argv>`; everything before `--` is parsed as
kernel library parameters and the first word is skipped, so the leading
placeholder is not optional.
