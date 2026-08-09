# Tests for the Unikraft patches

Guest programs that check a patch in `../patches/apply.sh` does what it claims.
They are not wired into a build: drop one into an example's rootfs, point
`--cmdline` at it, and read the output.

```sh
# from an example directory, with a Dockerfile that ships the binary:
bsdkrun unikraft . --mem 1024 --no-net --cmdline "elfloader -- /usr/bin/mremaptest"
```

## `mremaptest.c` — patch 15 (`mremap`)

Checks shrink, grow-in-place, the refusal to grow into an occupied range, the
argument validation, and the documented limitation that the implementation
never relocates a mapping. The last case is the one the patch exists for:
whether musl can size the initial thread's stack.

Expected on a patched arm64 build:

```
mmap 4 pages                                   ok
shrink 4->2 pages keeps the address            ok
shrink preserves the surviving contents        ok
grow 2->4 pages in place keeps the address     ok
grown pages are writable                       ok
grow preserves the original contents           ok
same-size mremap returns the same address      ok
set up a region with a neighbour above it      ok
blocked grow without MAYMOVE gives ENOMEM      ok
blocked grow with MAYMOVE gives ENOMEM (no move) ok
unmapped source gives EFAULT                   ok
new_size of 0 gives EINVAL                     ok
misaligned source gives EINVAL                 ok
main thread stack size                         3497984 bytes
musl sizes the main stack plausibly (>64K)     ok

mremaptest ok (0 failures)
```

Without the patch every `mremap` case fails and the stack reports **4096
bytes** — one page, because musl's probe loop exits on the first `ENOSYS`
instead of walking down the stack.

Two expectations here are deliberately *not* Linux's behaviour, and would fail
if you built this for the host:

* `blocked grow with MAYMOVE gives ENOMEM` — Linux relocates. This
  implementation never moves a mapping; see the patch comment for why.
* the stack size is an over-estimate (3.4 MB against a 512 KiB stack), because
  ukvmem packs VMAs contiguously and musl's probe walks out of the stack into
  whatever is mapped below it, stopping at the first hole. Linux reports the
  exact size because the main stack has a guard gap beneath it. What actually
  bounds the application is `CONFIG_APPELFLOADER_STACK_NBPAGES`, so a runtime
  that recurses deeply still needs that raised — `unikraft-bun` sets it to
  2048 (8 MiB) for exactly this reason.
