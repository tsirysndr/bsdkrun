# unikraft-bun

[Bun](https://bun.sh) 1.3 — JavaScriptCore — as a Unikraft unikernel.

**Works.**

```console
$ curl http://127.0.0.1:3000/
Hello from Bun on Unikraft!

$ curl http://127.0.0.1:3000/info
{"runtime":"bun","version":"1.3.14","revision":"0d9b296af33f"}
```

```sh
bsdkrun pack .
bsdkrun unikraft . --mem 2048 --port 3000:3000 \
    --cmdline "elfloader -- /usr/bin/bun run /usr/src/server.js"
```

Verified on arm64 (macOS/Hypervisor.framework). x86_64 builds and is exercised
by CI; see below.

Getting here took two missing syscalls, a bigger stack, a procfs stand-in and
one runtime setting. Each is described below, because every one of them is a
thing the next musl-based runtime will hit too.

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

## Worked around: signal-based thread suspension

JavaScriptCore's *concurrent* collector suspends every other thread by
signalling it and waiting for the handler to record its registers. Unikraft
only delivers a signal when the target crosses a syscall boundary, so a thread
running JIT-ed code never takes it. The queue fills, `tkill` starts failing,
and Bun gives up:

```
tkill(..., 0x1e) = Resource temporarily unavailable (-11)
[1] embedder failed to suspend thread 0x106dc69008 for TLC 0x102dbf7000
```

`EAGAIN` comes from `pprocess_signal_enqueue()`'s `queued_count >= queued_max`.
`--cpus 2` and `--cpus 4` change nothing, which rules out simple scheduling
starvation.

The Kraftfile therefore sets `BUN_JSC_useConcurrentGC=0` (as `ENVP4`), which
leaves nothing to suspend. Bun runs, collects garbage on the main thread, and
serves. Remove that line once Unikraft can deliver a signal asynchronously to a
running thread — that is the real fix, and a much larger one than the two
syscalls above.

## Fixed: `epoll_pwait` with a signal mask

`lib/posix-poll` implemented the wait but refused any call carrying a mask:

```c
if (unlikely(sigmask)) {
        uk_pr_warn_once("STUB: epoll_pwait no sigmask support\n");
        return -ENOSYS;
}
```

On arm64 there is no `epoll_wait` syscall at all — musl's `epoll_wait()` *is*
`epoll_pwait()` with a NULL mask — so this only bites callers that pass a real
one. node and Deno do not. Bun does, and its event loop spun on `ENOSYS`
**29,000 times** while a single request sat unanswered: the server listened and
never replied.

Patch 16 applies the mask around the wait and restores it afterwards.

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
reported but does not fail the job.

That flag is now only about x86_64 coverage, not about Bun being broken. None
of the fixes is architecture-specific, so it should pass there too — but this
job is the only x86_64 host available and it has not run since. The step
summary says when to flip it to `strict: true`.

Earlier, that same job is what proved the original abort was
architecture-independent: it was added while the failure was only known on
arm64, and its first run reproduced it on x86_64.

Everything before the HTTP check — build, boot, kernel banner, network
interface, entropy — is asserted for this example exactly as for the working
ones.

## Alpine, unlike Deno

`oven/bun:alpine` is a genuine musl build — unlike `denoland/deno:alpine`,
which is glibc behind a shim — so there is no reason to prefer Debian here.
