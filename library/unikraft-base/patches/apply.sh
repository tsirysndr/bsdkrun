#!/bin/sh
# Apply the out-of-tree fixes the arm64 base image needs, to a fetched
# Unikraft source tree.
#
# Usage: ./patches/apply.sh <path-to-.unikraft/unikraft>
#
# Idempotent: re-running on an already-patched tree is a no-op, so rebuilds
# work without a clean fetch.
set -eu

UK="${1:?usage: apply.sh <path-to-.unikraft/unikraft>}"
HERE=$(cd "$(dirname "$0")" && pwd)

[ -f "$UK/Makefile.uk" ] || { echo "not a unikraft tree: $UK" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. arm64 syscall_shim: missing include.
#
# lib/syscall_shim/arch/arm64/syscall_handler.c dereferences
# `struct ukarch_execenv` but only includes <uk/arch/types.h>, so an arm64
# build of anything using syscall_shim (i.e. every elfloader build) dies with
# "invalid use of undefined type 'struct ukarch_execenv'". x86_64 has a
# separate handler and never hits it — which is why upstream's x86_64-only
# base image never needed this.
# ---------------------------------------------------------------------------
SH="$UK/lib/syscall_shim/arch/arm64/syscall_handler.c"
if [ -f "$SH" ] && ! grep -q 'uk/arch/ctx.h' "$SH"; then
	echo "patching $SH (add <uk/arch/ctx.h>)"
	sed -i.bak 's|#include <uk/arch/types.h>|#include <uk/arch/ctx.h>\n#include <uk/arch/types.h>|' "$SH"
	rm -f "$SH.bak"
else
	echo "syscall_handler.c: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 2. ukboot: a command line whose parameter half is empty loses its argv.
#
# lib/ukboot/early_init.c scans for the `--` stop sequence and then rewrites
# argv to drop the kernel parameters. uk_libparam_parse() returns the *index*
# of the stop sequence, so `--` as the very first token — i.e. no kernel
# parameters at all — returns 0, and the guard `rc > 0` skips the rewrite
# entirely. The application then receives "--" as its argv[0]:
#
#   ERR: [appelfloader] --: Failed to find executable in environment ($PATH)
#
# `rc >= 0` is the correct bound. The no-stop-sequence case returns argc and
# is still excluded by the existing `rc < (boot_argc - 1)` condition, so this
# only adds back the empty-parameter case. With rc == 0 the arithmetic below
# it works out: rc becomes 1, argv[1] (the "--") is overwritten with argv[0],
# and argv is advanced past it.
# ---------------------------------------------------------------------------
EI="$UK/lib/ukboot/early_init.c"
if [ -f "$EI" ] && grep -q 'if (rc > 0 && rc < (boot_argc - 1))' "$EI"; then
	echo "patching $EI (allow an empty uklibparam section)"
	sed -i.bak 's|if (rc > 0 \&\& rc < (boot_argc - 1))|if (rc >= 0 \&\& rc < (boot_argc - 1))|' "$EI"
	rm -f "$EI.bak"
else
	echo "early_init.c: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 3. virtio-mmio: one unsupported device must not hide the rest.
#
# drivers/virtio/mmio/virtio_mmio_cmdl.c walks the `virtio_mmio.device=`
# parameters and `return rc` on the first device it cannot add — abandoning
# every device after it. libkrun attaches the memory balloon first, and Unikraft
# has no balloon driver, so the very first entry fails with -14 and the entropy
# and network devices behind it are never registered:
#
#   ERR: [libvirtio_bus]  Failed to find the driver for the virtio device (id:5)
#   ERR: [libvirtio_mmio] Could not add device (-14)
#
# The guest then boots with no NIC at all, so a forwarded port has nothing to
# reach, and lwip never starts (which also makes any "no entropy" check vacuous).
# A device with no driver is a normal condition, not a reason to stop probing —
# Linux binds what it can and moves on. This only bites on x86_64, where the
# command line is the sole discovery path; arm64 finds its devices in the device
# tree and is unaffected.
# ---------------------------------------------------------------------------
CMDL="$UK/drivers/virtio/mmio/virtio_mmio_cmdl.c"
if [ -f "$CMDL" ] && ! grep -q 'keep probing the remaining devices' "$CMDL"; then
	echo "patching $CMDL (an unaddable device no longer aborts discovery)"
	python3 - "$CMDL" <<'PYEOF'
import sys
p = sys.argv[1]
s = open(p).read()
old = r"""		rc = virtio_mmio_add_dev(dev);
		if (unlikely(rc)) {
			uk_pr_err("Could not add device (%d)\n", rc);
			free(dev);
			return rc;
		}"""
new = r"""		rc = virtio_mmio_add_dev(dev);
		if (unlikely(rc)) {
			/* Not fatal: a device we have no driver for is normal
			 * (libkrun always attaches a memory balloon). Report it
			 * and keep probing the remaining devices, otherwise
			 * everything behind it - entropy, network - is lost.
			 */
			uk_pr_err("Could not add device (%d)\n", rc);
			free(dev);
			continue;
		}"""
assert old in s, "virtio_mmio_cmdl.c does not match the expected shape"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "virtio_mmio_cmdl.c: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 4. virtio-rng driver (new).
#
# Unikraft has no virtio-rng driver at all. On arm64 that leaves no usable
# entropy source under libkrun — LIBUKRANDOM_LCPU needs FEAT_RNG, which
# Hypervisor.framework guests do not get — so lwip aborts the boot with
# "Could not obtain randomness (-19)". See virtio-rng/virtio_rng.c.
# ---------------------------------------------------------------------------
RNG="$UK/drivers/virtio/rng"
if [ ! -d "$RNG" ]; then
	echo "installing virtio-rng driver into $RNG"
	mkdir -p "$RNG"
	cp "$HERE/virtio-rng/virtio_rng.c" "$HERE/virtio-rng/Config.uk" \
	   "$HERE/virtio-rng/Makefile.uk" "$RNG/"
else
	echo "virtio-rng: already installed, skipping"
fi

# Register the driver's sources. Makefile.uk lists each virtio subdirectory
# explicitly, so a new driver is not compiled until it is added here.
# (Config.uk needs no equivalent: drivers/Config.uk pulls in the whole virtio
# directory through support/build/config-submenu.sh, which discovers
# rng/Config.uk on its own.)
VMK="$UK/drivers/virtio/Makefile.uk"
if ! grep -q 'LIBVIRTIO_BASE)/rng' "$VMK"; then
	echo "registering virtio-rng in $VMK"
	printf '\n$(eval $(call import_lib,$(UK_DRIV_LIBVIRTIO_BASE)/rng))\n' >>"$VMK"
else
	echo "virtio-rng: already in Makefile.uk, skipping"
fi

# LIBVIRTIO_BUS `depends on VIRTIO_DEVICE`, and VIRTIO_DEVICE only defaults to
# y for the four drivers that existed when it was written. Without adding the
# new driver to that list, `select LIBVIRTIO_BUS` from rng/Config.uk silently
# fails to take effect in a build that enables no other virtio device.
VCFG="$UK/drivers/virtio/Config.uk"
if ! grep -q 'LIBVIRTIO_RNG' "$VCFG"; then
	echo "adding LIBVIRTIO_RNG to VIRTIO_DEVICE in $VCFG"
	sed -i.bak 's|default y if (LIBVIRTIO_9P |default y if (LIBVIRTIO_RNG \|\| LIBVIRTIO_9P |' "$VCFG"
	rm -f "$VCFG.bak"
	grep -q 'LIBVIRTIO_RNG' "$VCFG" || {
		echo "failed to patch VIRTIO_DEVICE default in $VCFG" >&2
		exit 1
	}
else
	echo "VIRTIO_DEVICE: already lists LIBVIRTIO_RNG, skipping"
fi

echo "patches applied."
