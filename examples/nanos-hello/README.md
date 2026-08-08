# nanos-hello

A [Nanos](https://nanos.org) (NanoVMs) unikernel. Nanos implements the Linux
syscall ABI, so the app is an ordinary **static Linux binary** — no SDK, no
rebuild against a library OS; `ops` wraps it into a bootable image.

```sh
curl https://ops.city/get.sh -sSfL | sh   # install ops
./build.sh                                # builds ~/.ops/images/nanos-hello
```

## Status: x86_64 experimental, arm64 blocked upstream

**x86_64 (Linux/KVM)** boots via direct kernel load — the same path
Firecracker uses, which Nanos supports officially:

```sh
bsdkrun kernel \
  --kernel ~/.ops/0.1.55/kernel.img --format elf \
  --disk ~/.ops/images/nanos-hello \
  --cpus 1 --mem 512 --no-net
```

This is exercised by the `e2e-nanos` workflow on KVM runners; treat it as
experimental until that workflow is green.

**arm64 (macOS) does not boot yet, and the blocker is upstream.** What was
established, so nobody re-digs it:

- Nanos/arm64 is ACPI-only: GIC via MADT, timer via GTDT, console via SPCR,
  virtio-mmio via DSDT `LNRO0005` nodes. Its device tree code reads memory
  ranges only — there are no DT interrupt-controller bindings at all.
- libkrun's firmware published no ACPI. That half is **fixed**: with
  `KRUN_ACPI=1`, the libkrun fork (branch `feat/pvh-boot`) serves
  QEMU-shaped ACPI tables over an fw_cfg device, and the firmware installs
  them (verified via an EFI-shell `memmap` probe: 16 pages of ACPI reclaim
  memory, versus zero before).
- Nanos still hangs — and it hangs **identically under QEMU + stock edk2**,
  silently, right after BdsDxe starts its `bootaa64.efi`. `ops run` ignores
  `"Uefi": true` for local runs (it boots the BIOS-style image instead), so
  the arm64 UEFI path appears untested upstream at nanos 0.1.55 / ops 0.1.46.

When upstream's arm64 UEFI loader works, the libkrun side is already in
place:

```sh
KRUN_ACPI=1 bsdkrun firmware \
  --firmware ~/.cache/bsdkrun/KRUN_EFI.fd \
  --disk ~/.ops/images/nanos-hello --cpus 1 --mem 1024
```

(The arm64 image needs `"Uefi": true` — `config.json` here — or ops emits no
EFI System Partition and the firmware drops to a shell.)
