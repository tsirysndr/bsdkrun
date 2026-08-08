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

Six things were needed on top of a stock checkout. All are applied by
`patches/apply.sh`, which is idempotent, except the last which is kconfig:

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

5. **`virtio_mmio_cmdl_probe()` abandons every device after the first failure.**
   It does `return rc` when a device cannot be added. libkrun attaches the memory
   balloon first and Unikraft has no balloon driver, so entry one fails with -14
   and the entropy and network devices behind it are never registered — the guest
   boots with no interface at all. Only x86_64 is affected, because there the
   command line is the sole discovery path; arm64 reads the device tree.
6. **`CONFIG_LIBPOSIX_PROCESS_ARCH_PRCTL`** (kconfig, not a patch) implements
   `arch_prctl(ARCH_SET_FS)`, which is how glibc installs the thread pointer on
   x86_64. Without it no application can set up thread-local storage:
   *"cannot set up thread-local storage: cannot set %fs base address"*. It is
   `depends on ARCH_X86_64`, so it is an x86_64 fix only.

## Status

**x86_64 works end to end** and is covered by
`.github/workflows/e2e-unikraft-examples.yml`: the unikernel builds, boots, gets
a DHCP address, runs the application, and answers HTTP over a forwarded port.

**arm64 runs Rust services.** The actix example builds, boots, and answers HTTP
on macOS/Hypervisor.framework — verified with GET, POST and a second route.

**arm64 does not yet run glibc C programs.** They abort during startup with

    *** stack smashing detected ***: terminated
    ESR_EL1 0xf20003e8 (BRK #1000)

and the faulting address lies inside **libc.so.6**, above both the program and
the interpreter. So the canary check that fails is glibc's own: rebuilding the
application with `-fno-stack-protector` changes nothing (verified — an A/B pair
of the same program fails identically). node fails the same way on Debian; on
Alpine it fails earlier, for a reason of its own.

Two things are ruled out with evidence rather than argument:

  * **musl is fine.** Trivial musl binaries run, both dynamically linked through
    `ld-musl-aarch64.so.1` and as static-PIE. An earlier claim here that musl's
    loader was broken was wrong — it generalised from node alone.
  * **The application's own compiler flags are irrelevant**, since the check
    that fires belongs to libc.

The open question is why actix survives glibc's canary checks when a trivial C
program does not, given both link the same `libc.so.6`. That points at a
specific libc code path rather than a wholesale TLS failure, and is the next
thing to chase — with the GDB stub (`LIBUKDEBUG_GDBSTUB` supports arm64) rather
than more print statements.

**x86_64 is exercised in CI** and needs none of this: the elfloader `base`
runtime, both examples, boot and serve there.

## Notes

`build.sh` builds the root filesystem with `docker buildx --platform` for the
*target* architecture and hands kraft the flattened directory via `--rootfs`.
Letting kraft drive the Dockerfile itself builds it for the *host* architecture,
which silently produces an image whose binaries cannot run in the guest.

Verbosity and other kconfig **`choice`** members cannot be set from a Kraftfile —
kraft's kconfig injection ignores them in both directions, and the choice default
wins. Setting one requires editing the generated `.config` and building with
`make` directly.
