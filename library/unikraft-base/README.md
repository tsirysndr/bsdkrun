# unikraft-base

Unikraft's `base` runtime — the [app-elfloader](https://github.com/unikraft/app-elfloader)
stack that runs an unmodified Linux binary as a unikernel — built from source so
it can target **arm64**.

```sh
./build.sh all          # or: ./build.sh arm64 | x86_64
./push.sh all           # needs GH_TOKEN with write:packages
```

## Why build it at all

Upstream publishes `unikraft.org/base:latest` for x86_64 only:

```console
$ kraft pkg pull --plat fc --arch arm64 unikraft.org/base:latest
could not find unikraft.org/base:latest
```

libkrun on Apple Silicon runs arm64 guests only, so the prebuilt runtime cannot
be used under bsdkrun at all. app-elfloader itself is architecture-clean —
nothing in its `Config.uk` gates on architecture and it ships a `qemu-aarch64`
defconfig — so the gap is packaging, not portability.

Four things were needed on top of a stock checkout. All are applied by
`patches/apply.sh`, which is idempotent:

1. **`lib/syscall_shim/arch/arm64/syscall_handler.c` is missing an include.**
   It uses `struct ukarch_execenv` but only includes `<uk/arch/types.h>`, so any
   arm64 build fails with *"invalid use of undefined type"*. x86_64 has a
   separate handler and never hits it.
2. **`CONFIG_FPSIMD`** is `default n` and `depends on ARCH_ARM_64`. Without it
   nothing clears `CPACR_EL1.FPEN`, and any binary using FP/SIMD traps on its
   first NEON instruction (`ESR_EL1` EC=0x07). x86_64 never needs it, which is
   why upstream's config omits it.
3. **No entropy source.** `LIBUKRANDOM_LCPU` on arm64 needs FEAT_RNG (the
   armv8.5 `RNDR` instruction), which Hypervisor.framework guests do not get, so
   lwip aborts the boot with *"Could not obtain randomness (-19)"*. Fixed by
   `patches/virtio-rng`, a driver for the virtio-rng device libkrun already
   exposes (Unikraft has none in-tree). The alternative — `random.seed=` on the
   kernel command line — puts the CSPRNG key somewhere neither secret nor
   unpredictable.
4. **`lib/ukboot/early_init.c` mishandles an empty parameter section.** It scans
   for the `--` stop sequence and rewrites `argv` to drop kernel parameters, but
   the guard is `rc > 0` where `rc` is the *index* of `--`. A command line with
   no kernel parameters puts `--` at index 0, the rewrite is skipped, and the
   application receives `"--"` as its `argv[0]`. `rc >= 0` is the correct bound.
   This one is not arm64-specific.

## Known issues

**The default `/fallback` program crashes on arm64.** Run with no rootfs, the
image gets as far as executing `/fallback` and then dies with
`*** stack smashing detected ***` (`ESR_EL1` EC=0x3c, `BRK #1000`). A
dynamically-linked musl binary fails differently — it branches to a null GOT
entry (`br x17` with `x17 == 0`), i.e. an unresolved PLT relocation. Both faults
are in libc startup, about a second into the boot.

Ruled out so far, with evidence: `AT_RANDOM` (fed correctly by the virtio-rng
driver); FP/SIMD context sizing (`UK_PLAT_NATIVE_ECTX_SIZE` is 520, exactly the
saved FP state); elfloader's entry selection (it does jump to `interp->entry`
for dynamic binaries); and the ELF machine-type / `AT_PLATFORM` handling.

The kernel side is sound — it boots, seeds entropy, brings up networking, parses
`argv`, and loads ELF images. What is unresolved is application startup. The
next step is Unikraft's GDB stub (`LIBUKDEBUG_GDBSTUB`, which supports arm64)
against the `.dbg` image, to single-step the interpreter's self-relocation.

**x86_64 is built but never booted.** libkrun on macOS/arm64 cannot run x86_64
guests, so that target has only ever been compiled here.

## Notes

`build.sh` builds the root filesystem with `docker buildx --platform` for the
*target* architecture and hands kraft the flattened directory via `--rootfs`.
Letting kraft drive the Dockerfile itself builds it for the *host* architecture,
which silently produces an image whose binaries cannot run in the guest.

Verbosity and other kconfig **`choice`** members cannot be set from a Kraftfile —
kraft's kconfig injection ignores them in both directions, and the choice default
wins. Setting one requires editing the generated `.config` and building with
`make` directly.
