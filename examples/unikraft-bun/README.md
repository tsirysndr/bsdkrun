# unikraft-bun

[Bun](https://bun.sh) 1.3 — JavaScriptCore — as a Unikraft unikernel.

> **Closer, but not there yet.** Bun starts, initialises JavaScriptCore, and
> binds its HTTP listener — it prints `Bun listening on port 3000`. It then
> fails during garbage collection, because JSC suspends threads with signals
> and Unikraft cannot deliver a signal to a thread that is not sitting at a
> syscall boundary. `../unikraft-deno` is the working JS-runtime example.

```sh
./build.sh
bsdkrun unikraft . --mem 2048 --port 3000:3000 \
    --cmdline "elfloader -- /usr/bin/bun run /usr/src/server.js"
```

## Fixed on the way here: `mremap`

Unikraft had no `mremap` at all, and musl needs it to size the *initial*
thread's stack. `pthread_getattr_np()` has no thread descriptor to read for
that thread, so it probes downward from the auxiliary vector, extending its
estimate for as long as the probe returns `ENOMEM`. The disassembly of
`ld-musl-aarch64.so.1` is explicit:

```
bl   mremap
cmn  x0, #0x1          ; returned -1?
b.ne exit
bl   __errno_location
cmp  w0, #0xc          ; 0xc = ENOMEM
b.eq loop              ; keep probing ONLY on ENOMEM
exit                   ; a->_a_stacksize = l
```

`ENOSYS` left that loop on its first iteration, so musl reported a **4096-byte**
stack. JavaScriptCore took its bounds from that and aborted on the first bounds
check, before running any JavaScript — with no message, because it is a bare
`abort()` on a bounds check rather than an assertion.

Patch 15 of `../../library/unikraft-base/patches/apply.sh` implements the
syscall; `../../library/unikraft-base/tests/mremaptest.c` checks it (14
assertions, and the reported stack goes from 4096 bytes to 3.4 MB).

Two things then had to change here too:

* **`CONFIG_APPELFLOADER_STACK_NBPAGES: 2048`** — 8 MiB, matching Linux's
  default. With the 512 KiB the other examples use, JSC sizes its recursion
  limits from the real stack and runs into the guard page:
  `Guard page 0x100030f000 of stack VMA 0x100030f000 - 0x1000394000 hit!`
* the `/proc/self/exe` symlink, below.

## The current blocker: signal-based thread suspension

JavaScriptCore's garbage collector suspends every other thread by signalling it
and waiting for the handler to record its registers. The signal is never
delivered, the queue fills, and `tkill` starts failing:

```
tkill(..., 0x1e) = Resource temporarily unavailable (-11)
[1] embedder failed to suspend thread 0x106dc69008 for TLC 0x102dbf7000
```

`EAGAIN` comes from `pprocess_signal_enqueue()`'s `queued_count >= queued_max`:
nothing drains the queue, because a thread only looks at pending signals when
it crosses a syscall boundary, and a thread spinning in JIT-ed code never does.
`--cpus 2` and `--cpus 4` change nothing, which rules out simple scheduling
starvation.

Delivering a signal asynchronously to a *running* thread is a much larger piece
of work than the missing syscall above, so it is left as the next step.

Until then Bun listens but never answers — the request arrives while the
runtime is stuck in that handshake, and `curl` times out.

## Also fixed: `/proc/self/exe`

Before any of the above, Bun panicked with its own message:

```
panic(main thread): unexpected error from createFakeTemporaryNodeExecutable: error.FileNotFound
```

Bun locates its own binary through `/proc/self/exe` in order to symlink itself
as `node` for child processes. Unikraft has no procfs, so the read fails.

The Dockerfile ships a static `/proc/self/exe -> /usr/bin/bun` symlink. That is
less a hack than the correct answer: a unikernel runs exactly one program, so
the target is known when the image is built. It would be wrong on a
multi-process system and is exactly right here. Unikraft's cpio extractor
creates symlinks and ramfs stores them, so it survives into the guest.

## What was ruled out along the way

Recorded because each cost a build, and because the wrong guesses are the
useful part. Before `mremap` was found, the abort was blamed on JSC's memory
setup; none of these was the cause:

| hypothesis | test | result |
|---|---|---|
| Not enough guest RAM | `--mem 4096`, `--mem 6144` | aborts identically |
| JSC heap sized off a bad `sysinfo` | `BUN_JSC_forceRAMSize=134217728` | accepted, still aborts |
| Bun's own memory profile | `bun --smol` | still aborts |
| Gigacage cannot be placed | `GIGACAGE_ENABLED=0`, `=no` | still aborts |
| JIT cage | `BUN_JSC_useJITCage=0` | accepted, still aborts |
| Addresses too high (Unikraft's mmap base is 64 GiB, and JSC makes assumptions about pointer layout) | lowered `LIBUKVMEM_DEFAULT_BASE` to 4 GiB *in `lib/ukvmem/Config.uk`*, since kraft silently ignores that injection from a Kraftfile — mmap moved to `0x12d42c000` | aborts at the same point |

`BUN_JSC_useGigacage=0` *appears* to fix it and does not: Bun rejects the name
("invalid JSC environment variable") and exits cleanly, so the abort merely
stops happening. Only the syscall trace caught that.

## CI

`.github/workflows/e2e-unikraft-examples.yml` builds and boots this example on
x86_64 alongside the others, with `strict: false` so the HTTP check runs and is
reported but does not fail the job. That job is what established the failure is
architecture-independent: it was added while the abort was only known on arm64,
and the first run reproduced it on x86_64. Clear the flag once Bun answers.

Everything before the HTTP check — build, boot, kernel banner, network
interface, entropy — is asserted for this example exactly as for the working
ones.

## Alpine, unlike Deno

`oven/bun:alpine` is a genuine musl build — unlike `denoland/deno:alpine`,
which is glibc behind a shim — so there is no reason to prefer Debian here.
