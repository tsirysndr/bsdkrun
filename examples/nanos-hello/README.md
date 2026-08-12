# nanos-hello

A [Nanos](https://nanos.org) (NanoVMs) unikernel. Nanos implements the Linux
syscall ABI, so the app is an ordinary **static Linux binary** — no SDK, no
rebuild against a library OS; `ops` wraps it into a bootable image.

```sh
curl https://ops.city/get.sh -sSfL | sh   # install ops
./build.sh                                # builds ~/.ops/images/nanos-hello
bsdkrun nanos nanos-hello --no-net         # boot it
```

`bsdkrun nanos` takes an image path, or a bare name it looks up in
`~/.ops/images/` (what `ops build -i <name>` produces). It picks the boot
method for the host automatically — the direct-kernel and EFI details below
are what it does under the hood, not something you invoke yourself.

Like every unikernel, a Nanos machine has no shell or agent: `exec`, `shell`
and `commit` are rejected; `logs`, `ps`, `stop`, `start` and `rm` work as
usual.

## Status: x86_64 experimental, arm64 boots with patched kernel + libkrun

**x86_64 (Linux/KVM)** boots via direct kernel load — the same path
Firecracker uses, which Nanos supports officially. `bsdkrun nanos` finds the
kernel ops staged (`~/.ops/<version>/kernel.img`) on its own; equivalently:

```sh
bsdkrun nanos nanos-hello --cpus 1 --mem 512 --no-net
```

This is exercised by the `e2e-nanos` workflow on KVM runners; treat it as
experimental until that workflow is green.

**arm64 (macOS) boots to userspace** — with a patched Nanos and a patched
libkrun. Stock 0.1.55 dies before userspace, and the old "Nanos needs
virtio-PCI" diagnosis was only the first layer of five. Every one of these
had to fall, in order (nanos fork branch `fix/aarch64-libkrun-boot`, libkrun
fork branch `feat/pvh-boot`):

1. **No virtio-mmio enumeration** (`platform/virt/service.c`): the virt
   platform never called `virtio_mmio_enum_devs()`, so the DSDT's `LNRO0005`
   devices — the only kind libkrun has — were invisible. Reproducible on
   plain QEMU with a modern virtio-mmio disk
   (`-global virtio-mmio.force-legacy=false`; Nanos requires virtio 1.0 and
   silently skips QEMU's default legacy transport).
2. **DC ZVA before the MMU is on** (`src/runtime/memops.c`): `zero()` used
   the DC ZVA fast path while every access is still Device memory —
   alignment fault on HVF, hidden on QEMU which takes the memset path.
3. **Dirty-cache handover from the EFI loader** (`src/aarch64/uefi.c`): the
   loader hands over with caches off but everything it wrote — kernel image,
   `boot_params`, memory map — still in dirty write-back lines, and stale
   lines masking the kernel's uncached early writes (initial page tables,
   boot stack) once caches come back on. `acpi_rsdp` arrived as 0. QEMU does
   not model caches, so only hardware-backed hypervisors see any of this.
4. **GICv2/v3 selection from `ID_AA64PFR0_EL1.GIC`** (`src/aarch64/gic.c`):
   HVF does not virtualize that field, so it reads "no sysreg interface" and
   Nanos drove a GICv2 GICC at QEMU-virt compile-time addresses that don't
   exist on libkrun. A MADT redistributor entry now outranks PFR0.
5. **virtio-mmio IRQ routing ignored the DSDT** (`src/virtio/virtio_mmio.c`):
   the handler was registered on a vector allocated from a heap that starts
   at QEMU's first mmio slot, not on the SPI the DSDT declared — right on
   QEMU only when the device sits in slot 0, wrong everywhere on libkrun.
   Every block completion was silently lost.

And one bug on the libkrun side: the in-kernel `hv_gic`'s SPIs are real
levels, but nothing ever lowered them — after the first completion the guest
took ~35k empty interrupts per second, forever. The fork now de-asserts the
SPI when the guest's virtio `InterruptACK` leaves no ISR bits set.

To run it, build the patched kernel and stage it over what ops installed
(ops needs no other change; back the originals up first):

```sh
git clone -b fix/aarch64-libkrun-boot https://github.com/tsirysndr/nanos && cd nanos
make kernel        # needs aarch64-elf-gcc (brew install aarch64-elf-gcc)
cp output/platform/virt/bin/kernel.img output/platform/virt/boot/bootaa64.efi \
   ~/.ops/0.1.55-arm/
cd ../nanos-hello && ./build.sh          # rebuild the image with them
bsdkrun nanos nanos-hello --cpus 1 --mem 1024
```

(The arm64 image needs `"Uefi": true` — `config.json` here — or ops emits no
EFI System Partition and the firmware drops to a shell. `bsdkrun nanos` sets
`KRUN_ACPI=1` itself.)

Verified on M-series (2026-08-12): three consecutive `bsdkrun nanos` boots
print `Hello from Nanos on bsdkrun!`, and the same image still boots on QEMU
with both a virtio-PCI disk and a modern virtio-mmio disk.
