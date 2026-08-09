# unikraft-bun

[Bun](https://bun.sh) 1.3 — JavaScriptCore — as a Unikraft unikernel.

> **This one does not work yet.** Bun builds, boots and starts, but aborts
> during JavaScriptCore's heap setup before it runs any JavaScript. Everything
> below is accurate about where it stops and what has been ruled out. The Deno
> example in `../unikraft-deno` is the working JS-runtime example.

```sh
./build.sh
bsdkrun unikraft . --mem 2048 --port 3000:3000 \
    --cmdline "elfloader -- /usr/bin/bun run /usr/src/server.js"
```

## What works

`bun --version` runs to completion in the guest:

```console
$ bsdkrun unikraft . --mem 2048 --no-net --cmdline "elfloader -- /usr/bin/bun --version"
...
1.3.14
```

So the ELF loader, musl, `libstdc++`, TLS, and Bun's own start-up path are all
fine. Bun is a ~87 MiB Zig binary and it gets through its whole initialisation.

## Where it stops

Anything that starts JavaScriptCore aborts. With
`CONFIG_LIBSYSCALL_SHIM_STRACE: 'y'` the last syscalls are:

```
openat(dirfd:-2, "/proc/self/cgroup", ...) = No such file or directory (-2)
sysinfo(...) = 0x0
mmap(va:0x102d42c000, 1073741824, PROT_READ|PROT_WRITE,
     MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE, fd:-1, 0) = va:0x102d42c000
prctl(...) = 0x0
rt_sigprocmask(...) = 0x0
tkill(0x0, 0x6, ...)          <- SIGABRT
```

A 1 GiB reservation **succeeds**, and then Bun calls `abort()` with no
diagnostic of its own — so this is a raw `CRASH()`/`RELEASE_ASSERT` inside
bmalloc or JSC, not Bun's panic handler (which does print, see below).

## Ruled out, with evidence

| hypothesis | test | result |
|---|---|---|
| Not enough guest RAM | `--mem 4096`, `--mem 6144` | aborts identically |
| JSC heap sized off bad `sysinfo` | `BUN_JSC_forceRAMSize=134217728` | accepted, still aborts |
| Bun's own memory profile | `bun --smol` | still aborts |
| Gigacage cannot be placed | `GIGACAGE_ENABLED=0`, `=no` | still aborts |
| JIT cage | `BUN_JSC_useJITCage=0` | accepted, still aborts |
| Addresses too high — Unikraft's mmap base is 64 GiB, and JSC makes assumptions about pointer layout | lowered `LIBUKVMEM_DEFAULT_BASE` to 4 GiB *in `lib/ukvmem/Config.uk`*, since kraft silently ignores that injection from a Kraftfile — mmap moved to `0x12d42c000` | aborts at exactly the same point |

`BUN_JSC_useGigacage=0` *appears* to fix it and does not: Bun rejects the name
("invalid JSC environment variable") and exits cleanly, so the abort merely
stops happening. The trace is what caught that.

## One thing that was fixed

Before the above, Bun panicked earlier with its own message:

```
panic(main thread): unexpected error from createFakeTemporaryNodeExecutable: error.FileNotFound
```

Bun locates its own binary through `/proc/self/exe` in order to symlink itself
as `node` for child processes. Unikraft has no procfs, so the read fails.

The Dockerfile ships a static `/proc/self/exe -> /usr/bin/bun` symlink. That is
not really a hack: a unikernel runs exactly one program, so the target is known
when the image is built. It would be wrong on a multi-process system and is
exactly right here. Unikraft's cpio extractor creates symlinks and ramfs stores
them, so it survives into the guest.

## Next step

The gdb stub, the same way `../unikraft-expressjs/repro/` pinned down the node
failures: build a `qemu/arm64` target, attach `gdb-multiarch` through QEMU's
`-S -gdb tcp:...`, break on `abort`/`tkill`, and walk back up the stack to
whichever `RELEASE_ASSERT` is firing. Everything cheaper than that has now been
tried.

## Alpine, unlike Deno

`oven/bun:alpine` is a genuine musl build — unlike `denoland/deno:alpine`,
which is glibc behind a shim — so there is no reason to prefer Debian here.
