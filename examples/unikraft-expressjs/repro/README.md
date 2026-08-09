# arm64 `ld-musl` reproducer

A minimal stand-in for "boot node and wait two minutes". Every program here
builds into a rootfs of a few hundred KiB, so the unikernel builds in about a
minute and boots in about a second — against ~10 minutes and 133 MiB for the
real ExpressJS image.

To use it, point the example's `Dockerfile` at these sources instead of node
(see the git history of `../Dockerfile` for the shape), build, and pick a
program on the command line:

```sh
SKIP_FETCH=1 ./build.sh arm64
bsdkrun unikraft . --mem 512 --no-net --cmdline "elfloader -- /usr/bin/t64k"
```

Add `CONFIG_LIBSYSCALL_SHIM_STRACE: 'y'` to the Kraftfile's `unikraft:` kconfig
block for a syscall trace. It prints through `uk_printk(UK_PRINT_RAW)`, so
unlike `uk_pr_info` it is *not* swallowed by the `KLVL_ERR` default.

## The programs, and what each one ruled out

| program | what it does | arm64 result |
|-----------|--------------------------------------------------|-----------|
| `spin` | no extra library; burns ~400 ms of CPU | **runs** |
| `naps` | no extra library; 20 × `nanosleep`, so timer interrupts are delivered and returned from repeatedly | **runs** |
| `mmapsta` | *statically linked*; performs musl `map_library()`'s exact mmap sequence by hand — map the whole span `PROT_READ\|PROT_EXEC` from the file, then `MAP_FIXED` a read/write mapping at a different file offset over the tail, then read the RX page and read/write the RW page | **runs** |
| `mmapdyn` | the same, dynamically linked | **runs** |
| `t64k` | trivial program linked against one trivial `libfoo.so` | **faults** |
| `t4k` | same, library linked `-z max-page-size=4096` | **faults** |
| `tpad` | same, library zero-padded so nothing is mapped beyond EOF | **faults** |
| `cxx` | C++ `iostream`, i.e. `libstdc++.so.6` + `libgcc_s` | **faults** |

Between them these eliminate:

* **the ELF loader** — the C `app-elfloader` fails identically (see `../README.md`);
* **libstdc++, C++ and node** — one trivial `libfoo.so` is enough;
* **uptime and interrupt delivery** — `spin` and `naps` outlive the failure
  point by more than an order of magnitude;
* **the 64 KiB segment alignment gap**, and mapping beyond end-of-file — `t4k`
  and `tpad` remove each in turn and change nothing;
* **Unikraft's `mmap` / VMA split / page-attribute path on arm64** — `mmapsta`
  performs exactly those operations, including the `MAP_FIXED` split of a
  file-backed mapping that only the second-DSO case otherwise reaches, and
  reads and writes both sides afterwards.

What is left is the guest-side control flow immediately after
`map_library()` returns. The syscall trace ends like this, with no further
syscalls before the fault:

```
openat(…, "/usr/local/lib/libfoo.so", …) = No such file or directory (-2)
openat(…, "/usr/lib/libfoo.so", …)       = fd:3
fstat(…, {st_size=70712, st_mode=0100755}) = OK
read(…, "\x7FELF\x02\x01\x01…", 960)     = 960
mmap(…, 135168, PROT_EXEC|PROT_READ,  MAP_PRIVATE,           fd:3, 0)     = 0x100045c000
mmap(…,   8192, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_FIXED, fd:3, 61440) = 0x100047b000
close(…) = OK
<crash>
```

The library is found, mapped and closed successfully. The fault is an
**instruction abort at address 0** (`ESR_EL1=0x86000006`, `ELR_EL1=0`) with
`LR=0` and `x29=0`, while the stack frame is intact and its saved `x30` points
back into `__dls3`. A `blr` would have set `LR`, so this is a `br` or a `ret`
to a null target — a tail call or return through a pointer that is zero.

## Root cause (found)

`vn_stat()` in vfscore begins with `memset(st, 0, sizeof(struct stat))` on the
**application's** buffer. Unikraft's `struct stat` is the x86_64 Linux layout
on every architecture — the header says so itself:

```
 * Imported from Musl (arch/x86_64/bits/stat.h)
 * FIXME: This structure is defined for x86_64. On Musl, the ARM layout
 * is different.
```

It is 144 bytes. Linux and musl on arm64 use 128. So every `stat()`/`fstat()`
from an arm64 guest writes **16 zero bytes past the end of the caller's
buffer**. In musl's `load_library()` the `struct stat` sits just above the
saved frame pointer and return address, so those two words are zeroed and the
function's epilogue

```
6b790:  ldp  x29, x30, [sp]
6b7a8:  add  sp, sp, #0x460
6b7ac:  ret                  <- to 0
```

returns to address 0. Fixed by entry 12 of
`../../../library/unikraft-base/patches/apply.sh`, which gives arm64 its own
128-byte layout. `st_size`, `st_blocks` and the timestamps happen to land at
the same offsets in both layouts, which is why so much appeared to work;
`st_mode`, `st_nlink`, `st_uid`, `st_gid` and `st_rdev` were all wrong too.

With that fix, `t64k` and `cxx` run.

## How it was found

Cheap methods first, each ruling something out (table above). Then:

1. **QEMU reproduced it**, both under HVF and TCG (`-cpu cortex-a710`; the
   Firecracker-platform image needs a separate `qemu/arm64` target, and
   `cortex-a57` traps on an instruction the build uses). Identical `ESR`,
   `ELR_EL1 = 0`, `LR = 0` — which is what exonerated libkrun.
2. **QEMU's gdb stub** (`-S -gdb tcp:0.0.0.0:1234`), driven by `gdb-multiarch`
   in a container against `host.docker.internal`. Scripts in `gdb/`.
3. `monitor log exec,nochain` switched on at a breakpoint gave a 27,477-block
   execution trace with exactly **one** user→kernel transition — the fault, no
   page faults — pinning the faulting block to `ld+0x6b788`, an epilogue.
4. Breakpoints on `open`/`fstat`/`read`/`mmap` inside `load_library` bisected
   the corruption to the `fstat` call, and single-stepping from there landed
   on `str q0, [x19, #128]` — the tail of the inlined 144-byte memset — which
   symbolised to `vn_stat+0x44`.

Conditional and hardware watchpoints did not survive the remote link; the
breakpoint bisection did.

## Two more arm64 bugs behind it

With the `struct stat` fix, node got much further and stopped in OpenSSL's CPU
probe:

```
_armv8_sm3_probe:  sm3partw1 v4.4s, v0.4s, v3.4s ; ret
arm_probe_for:     bl sigsetjmp ; cbnz w0, <recover> ; blr x0
```

OpenSSL *deliberately* executes an SM3 instruction and expects `SIGILL` when
the CPU does not implement it (Apple silicon does not). That needs
`CONFIG_LIBPOSIX_PROCESS_SIGNAL` — and enabling it showed that Unikraft's arm64
signal trampoline had never been compiled:

```
lib/posix-process/arch/arm64/signal.S:37: Error: operand 2 must be an integer
    register -- `and sp,sp,#~0xf'
```

SP is not a valid operand of `AND` on AArch64 (entry 13 of `apply.sh`).

That got as far as a kernel panic inside `sys_error_handler`, which turned out
to be a `UK_BUG()` reached after a diagnostic — the real message was:

```
[libukvmem]        <vma_anon.c @ 39>  Assertion failure: fault->type & 0x04
[libposix_process] <deliver.c @ 428>  Cannot deliver SIGSEGV for pf at 0x101f2c0040
```

A temporary probe in the arm64 fault decoder, and then in `vmem_pagefault`
where the VMA is in scope, gave the whole story in two lines:

```
PFPROBE  vaddr=0x101f2c0040 esr=0x8600000f ec=0x21 dfsc=0xf wnr=0 isv=0 faulttype=0x8
VMAPROBE vaddr=0x101f2c0040 type=0x8 vma=[0x101f2c0000-0x102f280000] attr=0x7 allowed=1
```

`ec=0x21` is an instruction abort at EL1 and `dfsc=0xf` a permission fault at
level 3 — an instruction fetch refused on a page that is **present**, inside a
250 MB VMA whose attributes are `0x7`, i.e. RWX, which ukvmem agrees permits
the access. Hardware was refusing something software had allowed, which points
at a control outside the page tables: `SCTLR_EL1.WXN`. `plat/common/w_xor_x.c`
sets it, with the comment "This saves us from manually updating the PTEs" —
but WXN governs the whole EL1&0 translation regime, so it silently forbids
every RWX page a JIT needs. Entry 14 of `apply.sh` replaces it with per-region
PTE protections, which is what x86_64 already did.

Note that the decoder also mis-reported this fault as `READ | MISCONFIG`:
arm64 detects instruction fetches from the ISS `ISV` bit, which actually means
"instruction syndrome valid" and has nothing to do with instruction fetch (the
exception class does), and it folds permission faults into `MISCONFIG` where
x86_64 sets no flag at all. That mis-classification is why a fault that should
have produced a clean `SIGSEGV` instead walked into an anon-VMA fault handler
that asserts the page is absent. Worth fixing, but with entry 14 in place node
no longer generates the fault, so it is left as a known sharp edge rather than
a speculative patch.

## Result

node/ExpressJS serves HTTP on arm64:

```console
$ curl http://127.0.0.1:3000/
Bye, World!
```

