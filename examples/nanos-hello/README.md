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

## Status: x86_64 experimental, arm64 blocked on virtio-mmio support

**x86_64 (Linux/KVM)** boots via direct kernel load — the same path
Firecracker uses, which Nanos supports officially. `bsdkrun nanos` finds the
kernel ops staged (`~/.ops/<version>/kernel.img`) on its own; equivalently:

```sh
bsdkrun nanos nanos-hello --cpus 1 --mem 512 --no-net
```

This is exercised by the `e2e-nanos` workflow on KVM runners; treat it as
experimental until that workflow is green.

**arm64 (macOS) does not boot yet: Nanos needs virtio-PCI, and libkrun has
only virtio-mmio.**

The whole failure reproduces with no libkrun involved. Same image, same QEMU,
same stock edk2 — only the disk transport differs:

```sh
# boots, prints "Hello from Nanos on bsdkrun!"
qemu-system-aarch64 -machine virt -cpu cortex-a72 -m 512 -nographic \
  -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
  -drive file=$HOME/.ops/images/nanos-hello,format=raw,if=virtio -net none

# hangs at `BdsDxe: starting ... VenHw(...)` — exactly what bsdkrun shows
qemu-system-aarch64 -machine virt -cpu cortex-a72 -m 512 -nographic \
  -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
  -drive file=$HOME/.ops/images/nanos-hello,format=raw,if=none,id=d0 \
  -device virtio-blk-device,drive=d0 -net none
```

The Nanos kernel's own strings say why: it has **no devicetree support at all**
on arm64 (`virtio,mmio`, `arm,gic`, `arm,pl011` are all absent) and discovers
devices purely through ACPI (`SPCR`, `APIC`, `PNP0`) — but it knows nothing
about `LNRO0005`, the ACPI HID that describes a virtio-mmio device. The mmio
driver itself is there (`src/virtio/virtio_mmio.c`, `vtmmio:` messages); the
devices are simply never enumerated, so it finds no storage. At the hang the
vCPU sits at ~100% CPU with zero VM exits — a guest-internal spin, not a halt.

Things measured and ruled out, so nobody re-digs them:

- **ACPI is not the problem.** With `KRUN_ACPI=1` the libkrun fork (branch
  `feat/pvh-boot`) serves QEMU-shaped tables over fw_cfg and the firmware
  installs them — an EFI-shell `memmap` probe reports 16 pages of ACPI reclaim
  memory (zero before), and a level-5 trace shows ~570 fw_cfg accesses.
- **The PSCI conduit is consistent.** The FADT declares SMC and libkrun's HVF
  layer services `EC_AA64_SMC` through `handle_psci_request()`.
- **Nothing is wrong with the console path.** The guest emits no bytes at all.

Unblocking it means upstream Nanos enumerating `LNRO0005` virtio-mmio devices
from the DSDT; the two commands above are a self-contained bug report. The
alternative — a virtio-PCI transport in libkrun — is a much larger job.

The bsdkrun side is otherwise ready: `bsdkrun nanos` sets `KRUN_ACPI=1` and
boots the image via EFI on macOS:

```sh
bsdkrun nanos nanos-hello --cpus 1 --mem 1024
```

(The arm64 image needs `"Uefi": true` — `config.json` here — or ops emits no
EFI System Partition and the firmware drops to a shell.)
