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
# 4. arm64 syscalls run with IRQs masked, so any blocking one crashes.
#
# arm64 takes exceptions with IRQs masked (plat/kvm/arm/exceptions.S does
# `msr daifset, #2`), and lib/syscall_shim/arch/arm64/syscall_handler.c calls
# ukplat_syscall_handler() without ever re-enabling them. Any syscall that
# blocks therefore reaches the scheduler with interrupts off and takes the guest
# down:
#
#   CRIT: [libukschedcoop] Must not call schedcoop_schedule with IRQs disabled
#
# x86_64 does not have this problem because its prologue is hand-written
# assembly that issues `sti` immediately before dispatching
# (lib/syscall_shim/arch/x86_64/include/arch/syscall_prologue.h). This mirrors
# that: enable IRQs for the dispatch only, leaving the register-state save and
# restore either side of it with IRQs masked as before.
# ---------------------------------------------------------------------------
SC="$UK/lib/syscall_shim/arch/arm64/syscall_handler.c"
if [ -f "$SC" ] && ! grep -q 'mirroring x86_64' "$SC"; then
	echo "patching $SC (dispatch syscalls with IRQs enabled)"
	python3 - "$SC" <<'PYEOF'
import sys
p = sys.argv[1]
s = open(p).read()
old = r"""	ukplat_syscall_handler((struct uk_syscall_ctx *)execenv);
"""
new = r"""	/* Dispatch with IRQs enabled, mirroring x86_64's `sti` before its
	 * call. Exceptions are entered with IRQs masked, and a syscall that
	 * blocks (futex, poll, a socket read) ends up in the scheduler, which
	 * refuses to run with interrupts off. The save/restore either side
	 * stays masked, as it is on x86_64.
	 */
	uk_lcpu_enable_irq();

	ukplat_syscall_handler((struct uk_syscall_ctx *)execenv);

	uk_lcpu_disable_irq();
"""
assert old in s, "syscall_handler.c does not match the expected shape"
s = s.replace(old, new, 1)
# uk_lcpu_enable_irq()/disable_irq() live in <uk/lcpu/except.h>.
if "uk/lcpu/except.h" not in s:
    inc = "#include <uk/lcpu.h>" + chr(10)
    s = s.replace(inc, inc + "#include <uk/lcpu/except.h>" + chr(10), 1)
open(p, "w").write(s)
PYEOF
else
	echo "syscall_handler.c IRQ fix: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 5. struct epoll_event is packed on every architecture; Linux packs only x86_64.
#
# lib/nolibc/include/sys/epoll.h declares
#
#     struct epoll_event { uint32_t events; epoll_data_t data; } __packed;
#
# which is 12 bytes everywhere. That matches the Linux ABI on x86_64, where
# include/uapi/linux/eventpoll.h applies EPOLL_PACKED, but nowhere else: on
# aarch64 the struct is unpacked and 16 bytes (4 for events, 4 of padding, 8 for
# data). The guest kernel therefore fills the caller's array with a 12-byte
# stride while the application's libc reads it with a 16-byte stride, so every
# `data` field is read from the wrong offset.
#
# The symptom is an application that runs happily until the first I/O event and
# then takes a read data abort on a garbage pointer. With actix, whose tokio
# runtime turns each epoll event's token straight into a pointer:
#
#   ESR_EL1 0x96000006 (data abort, translation fault, read)
#   ELR     -> tokio::runtime::io::driver::Driver::turn + 0x140
#
# Match Linux: pack on x86_64 only.
# ---------------------------------------------------------------------------
EP="$UK/lib/nolibc/include/sys/epoll.h"
if [ -f "$EP" ] && ! grep -q 'UK_EPOLL_PACKED' "$EP"; then
	echo "patching $EP (pack epoll_event on x86_64 only, as Linux does)"
	python3 - "$EP" <<'PYEOF'
import sys
p = sys.argv[1]
s = open(p).read()
old = """struct epoll_event {
	uint32_t events;	/* Epoll events */
	epoll_data_t data;	/* User data variable */
} __packed;"""
new = """/* Linux packs this on x86_64 only (include/uapi/linux/eventpoll.h), where it
 * is 12 bytes. Everywhere else it is unpacked - on aarch64, 16 bytes with four
 * bytes of padding after `events`. Packing unconditionally makes the kernel
 * write the caller's array with the wrong stride, and every `data` field comes
 * back from the wrong offset.
 */
#if defined(__X86_64__) || defined(CONFIG_ARCH_X86_64)
#define UK_EPOLL_PACKED __packed
#else
#define UK_EPOLL_PACKED
#endif

struct epoll_event {
	uint32_t events;	/* Epoll events */
	epoll_data_t data;	/* User data variable */
} UK_EPOLL_PACKED;"""
assert old in s, "epoll.h does not match the expected shape"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "epoll.h: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 6. virtio-rng driver (new).
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

# ---------------------------------------------------------------------------
# 7. app-elfloader tests ELF program flags against mmap protection flags.
# ---------------------------------------------------------------------------
EL="$UK/../apps/elfloader/elf_load.c"
if [ -f "$EL" ] && grep -q 'phdr->p_flags & PROT_EXEC' "$EL"; then
	echo "patching $EL (map ELF PF_X segments executable)"
	sed -i.bak 's/phdr->p_flags \& PROT_EXEC/phdr->p_flags \& PF_X/' "$EL"
	rm -f "$EL.bak"
else
	echo "elf_load.c: executable segment mapping already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 8. arm64 must support mprotect() attribute changes for musl GNU_RELRO.
# ---------------------------------------------------------------------------
PT="$UK/plat/native/arch/arm64/pt.c"
if [ -f "$PT" ] && grep -q 'UK_CRASH("%s: Not implemented", __func__)' "$PT"; then
	echo "patching $PT (implement arm64 PTE permission changes)"
	python3 - "$PT" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()
old = """__pte_t uk_plat_native_pte_change_attr(__pte_t pte __unused,
				       unsigned long new_attr __unused,
				       unsigned int level __unused)
{
	UK_CRASH("%s: Not implemented", __func__);

	return 0;
}"""
new = """__pte_t uk_plat_native_pte_change_attr(__pte_t pte,
				       unsigned long new_attr,
				       unsigned int level __unused)
{
	pte &= ~(UK_ARCH_ARM64_PTE_ATTR_AP(UK_ARCH_ARM64_PTE_ATTR_AP_RO) |
		 UK_ARCH_ARM64_PTE_ATTR_XN);

	if (!(new_attr & UK_PLAT_NATIVE_PAGE_ATTR_PROT_WRITE))
		pte |= UK_ARCH_ARM64_PTE_ATTR_AP(UK_ARCH_ARM64_PTE_ATTR_AP_RO);

	if (!(new_attr & UK_PLAT_NATIVE_PAGE_ATTR_PROT_EXEC))
		pte |= UK_ARCH_ARM64_PTE_ATTR_XN;

	return pte;
}"""
assert old in s, "arm64 pt.c does not match the expected PTE attribute stub"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "arm64 PTE permission changes: already patched or absent, skipping"
fi

# Temporary loader diagnosis: report brk syscall values.
BRK="$UK/lib/posix-process/brk.c"
if [ -f "$BRK" ] && ! grep -q 'brk request=' "$BRK"; then
	echo "patching $BRK (report brk requests)"
	python3 - "$BRK" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()
old = """\taddr_arg = (__uptr)addr;

\tif (addr_arg == cur_brk) {
"""
new = """\taddr_arg = (__uptr)addr;
\tuk_pr_err("brk request=%p, base=%p, current=%p\\n", addr,
\t\t  (void *)base, (void *)cur_brk);

\tif (addr_arg == cur_brk) {
"""
assert old in s, "brk.c does not match the expected syscall path"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
fi

# Temporary loader diagnosis: report failed POSIX mmap calls.
MMAP="$UK/lib/posix-mmap/mmap.c"
if [ -f "$MMAP" ] && ! grep -q 'mmap(addr=%p, len=%zu' "$MMAP"; then
	echo "patching $MMAP (report failed mmap calls)"
	python3 - "$MMAP" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()
s = s.replace("#include <uk/lcpu.h>\n", "#include <uk/lcpu.h>\n#include <uk/print.h>\n", 1)
old = """\tif (unlikely(rc && !(flags & MAP_FIXED) &&
\t\t     vaddr != UK_PAGING_VADDR_ANY)) {
\t\t/* If addr was meant as a hint and we fail to map, we retry
\t\t * without specifying an address.
\t\t */
\t\tvaddr = UK_PAGING_VADDR_ANY;
\t\trc = uk_vma_map(vas, &vaddr, len, vattr, vflags, NULL, vops,
\t\t\t\tvargs);
\t}
"""
new = old + """
\tif (unlikely(rc))
\t\tuk_pr_err("mmap(addr=%p, len=%zu, prot=%x, flags=%x, fd=%d, offset=%lld) failed: %d\\n",
\t\t\t  *addr, len, prot, flags, fd, (long long)offset, rc);
"""
assert old in s, "posix-mmap source does not match expected mapping path"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
fi

# ---------------------------------------------------------------------------
# 10. arm64 cache maintenance runs in the wrong order to publish new code.
#
# `invalidate_icache_range` invalidates the instruction cache *before* cleaning
# the data cache for the same line. To make freshly written instructions
# visible to fetch the order has to be the other way round -- clean D to the
# point of unification, barrier, then invalidate I -- otherwise the I-cache can
# refill from stale memory in the window between the two. The routine also
# needs a `dsb ish` between the halves and an `isb` at the end, which the
# original only has after the whole loop.
# ---------------------------------------------------------------------------
CACHE="$UK/plat/common/arm/cache64.S"
if [ -f "$CACHE" ] && grep -q 'ic ivau, x0' "$CACHE" &&
	! grep -q 'dc cvau, x0' "$CACHE"; then
	echo "patching $CACHE (order cache maintenance so new code is fetchable)"
	python3 - "$CACHE" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()
old = """	/* Align the start address to line size */
	sub	x4, x3, #1
	and	x2, x0, x4
	add	x1, x1, x2
	bic	x0, x0, x4
1:
	/* Invalidate I cache a clean D cache */
	ic ivau, x0
	dc cvac, x0
	dsb nsh

	/* Move to next line and reduce size */
	add x0, x0, x3
	subs x1, x1, x3

	/* Check if all range has been invalidated */
	b.hi 1b
	isb
	dsb sy
	ret"""
new = """	/* The loops below step by one cache line, so the stride has to be the
	 * *smaller* of the two minimum line sizes -- using IminLine alone
	 * would skip data lines on any part where DminLine is finer.
	 */
	ubfx x5, x4, #UK_ARCH_ARM64_CTR_DMINLINE_SHIFT, #UK_ARCH_ARM64_CTR_DMINLINE_WIDTH
	lsl x5, x2, x5
	cmp x5, x3
	csel x3, x5, x3, lo

	/* Align the start address to the stride */
	sub x4, x3, #1
	and x2, x0, x4
	add x1, x1, x2
	bic x0, x0, x4

	mov x5, x0
	mov x6, x1
1:
	/* Clean the data cache to the point of unification first: the bytes
	 * have to have reached the level instruction fetch looks at before the
	 * instruction cache is told to forget what it has. Doing it the other
	 * way round leaves a window in which the I-cache can refill from stale
	 * memory.
	 */
	dc cvau, x0
	add x0, x0, x3
	subs x1, x1, x3
	b.hi 1b
	dsb ish

	mov x0, x5
	mov x1, x6
2:
	ic ivau, x0
	add x0, x0, x3
	subs x1, x1, x3
	b.hi 2b
	dsb ish
	isb
	ret"""
assert old in s, "cache64.S does not match the expected invalidate_icache_range body"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "arm64 cache maintenance order: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 11. Demand-paged executable pages are never synced to the instruction cache.
#
# ukvmem populates a page by copying into a fresh physical frame and installing
# a PTE. On arm64 that is not enough for code: the frame may hold lines the
# instruction cache still remembers from whatever used it before (the CPIO
# extraction of the root filesystem, for a start), so a large binary can end up
# executing stale instructions -- which presents as a crash somewhere
# unrelated, a corrupted stack canary among them.
#
# Linux does this in `set_pte_at()` via `__sync_icache_dcache()`. Unikraft has
# the routine (plat/common/arm/cache64.S) but, before this, no caller outside
# the GDB stub.
#
# The maintenance goes *after* uk_paging_page_mapx() returns, not inside the
# mapx callback: within the callback the PTE has not been installed yet, so the
# virtual address is still untranslatable and `dc cvau` on it takes a level-3
# translation fault.
# ---------------------------------------------------------------------------
VMEM="$UK/lib/ukvmem/vmem.c"
if [ -f "$VMEM" ] && ! grep -q 'vmem_sync_icache' "$VMEM"; then
	echo "patching $VMEM (sync the icache for demand-paged executable pages)"
	python3 - "$VMEM" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

helper = """#if CONFIG_ARCH_ARM_64
/* arm64 instruction and data caches are not coherent with one another. Memory
 * populated through ordinary stores is not guaranteed to be seen by
 * instruction fetch until it has been cleaned to the point of unification and
 * the instruction cache invalidated for those addresses.
 *
 * Call this only once the mapping is live: it touches the virtual address.
 */
void invalidate_icache_range(__sz addr, __sz len);

static inline void vmem_sync_icache(struct uk_vma *vma, __vaddr_t vaddr,
				    __sz len)
{
	if (vma->attr & UK_PAGING_PAGE_ATTR_PROT_EXEC)
		invalidate_icache_range((__sz)vaddr, len);
}
#else /* !CONFIG_ARCH_ARM_64 */
static inline void vmem_sync_icache(struct uk_vma *vma __unused,
				    __vaddr_t vaddr __unused,
				    __sz len __unused)
{
	/* x86_64 keeps instruction fetch coherent with stores. */
}
#endif /* !CONFIG_ARCH_ARM_64 */

static int vmem_mapx_populate("""
assert "static int vmem_mapx_populate(" in s, "vmem.c has no vmem_mapx_populate"
s = s.replace("static int vmem_mapx_populate(", helper, 1)

# MAP_POPULATE path: sync once the whole range is mapped.
old_pop = """			vmem_vma_destroy(vma);

			return rc;
		}
	}

	vmem_vma_insert(vas, vma);"""
new_pop = """			vmem_vma_destroy(vma);

			return rc;
		}

		/* The range is mapped now, so its virtual addresses can be
		 * touched -- which cache maintenance has to do.
		 */
		vmem_sync_icache(vma, vma->start, len);
	}

	vmem_vma_insert(vas, vma);"""
assert old_pop in s, "uk_vma_map populate path does not match"
s = s.replace(old_pop, new_pop, 1)

# Fault path: sync the page that was just paged in.
old_flt = """	return uk_paging_page_mapx(pt, vbase, 0, 1, ctx.vma->attr,
				UK_PAGING_PAGE_FLAG_SIZE(lvl) | flags, &mapx);
}"""
new_flt = """	rc = uk_paging_page_mapx(pt, vbase, 0, 1, ctx.vma->attr,
				 UK_PAGING_PAGE_FLAG_SIZE(lvl) | flags, &mapx);
	if (unlikely(rc))
		return rc;

	vmem_sync_icache(ctx.vma, vbase, UK_PAGING_PAGE_Lx_SIZE(lvl));

	return 0;
}"""
assert old_flt in s, "vmem_pagefault tail does not match"
s = s.replace(old_flt, new_flt, 1)

open(p, "w").write(s)
PYEOF
else
	echo "ukvmem icache sync: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 12. `struct stat` is the x86_64 layout on every architecture.
#
# lib/nolibc/include/sys/stat.h says so itself:
#
#     Imported from Musl (arch/x86_64/bits/stat.h)
#     FIXME: This structure is defined for x86_64. On Musl, the ARM layout
#     is different.
#
# It is 144 bytes with st_nlink before st_mode. Linux and musl on arm64 use
# 128 bytes, with 32-bit st_mode/st_nlink before st_uid and a 32-bit
# st_blksize. Two consequences, and the second is the serious one:
#
#   * st_mode, st_nlink, st_uid, st_gid and st_rdev all come back wrong, while
#     st_size, st_blocks and the timestamps happen to line up -- which is why
#     this stayed hidden for so long.
#
#   * vfscore's vn_stat() begins with memset(st, 0, sizeof(struct stat)) on the
#     *application's* buffer, so every stat()/fstat() from an arm64 guest
#     writes 16 zero bytes past the end of it. In musl's load_library() the
#     `struct stat` sits directly above the saved frame pointer and return
#     address, so those get zeroed and the function returns to address 0. That
#     is the "any binary that loads a second shared library dies on arm64"
#     failure: node, and equally a two-line C program linked against one
#     trivial .so.
# ---------------------------------------------------------------------------
STATH="$UK/lib/nolibc/include/sys/stat.h"
if [ -f "$STATH" ] && ! grep -q "__ARM_64__" "$STATH"; then
	echo "patching $STATH (arm64 struct stat layout)"
	python3 - "$STATH" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old_comment = """/*
 * Imported from Musl (arch/x86_64/bits/stat.h)
 *
 * Copied from kernel definition, but with padding replaced
 * by the corresponding correctly-sized userspace types.
 *
 * FIXME: This structure is defined for x86_64. On Musl, the ARM layout
 * is different.
 */

struct stat {"""

old_struct = """struct stat {
	dev_t st_dev;
	ino_t st_ino;
	nlink_t st_nlink;

	mode_t st_mode;
	uid_t st_uid;
	gid_t st_gid;
	unsigned int    __pad0;
	dev_t st_rdev;
	off_t st_size;
	blksize_t st_blksize;
	blkcnt_t st_blocks;

	struct timespec st_atim;
	struct timespec st_mtim;
	struct timespec st_ctim;
	long unused[3];
};"""

assert old_comment in s, "stat.h header comment does not match"
assert old_struct in s, "stat.h struct definition does not match"

new = """/*
 * Imported from Musl, per architecture.
 *
 * Copied from the kernel definition, but with padding replaced by the
 * corresponding correctly-sized userspace types.
 *
 * These layouts are ABI: the pointer an application passes to stat() is
 * written through directly by vfscore's vn_stat(), which starts with
 * memset(st, 0, sizeof(struct stat)). A definition that is larger than the
 * caller's therefore corrupts the caller's memory, so the sizes below must
 * match the target's Linux ABI exactly (144 bytes on x86_64, 128 on arm64).
 */

#if defined(__ARM_64__) || defined(__aarch64__)
/* Musl arch/aarch64/bits/stat.h: 128 bytes. Note that st_mode and st_nlink
 * are 32-bit and precede st_uid, and st_blksize is 32-bit -- none of which is
 * true of the x86_64 layout below.
 */
struct stat {
	dev_t st_dev;
	ino_t st_ino;
	mode_t st_mode;
	unsigned int st_nlink;

	uid_t st_uid;
	gid_t st_gid;
	dev_t st_rdev;
	unsigned long long __pad1;
	off_t st_size;
	int st_blksize;
	int __pad2;
	blkcnt_t st_blocks;

	struct timespec st_atim;
	struct timespec st_mtim;
	struct timespec st_ctim;
	unsigned int __unused4;
	unsigned int __unused5;
};
#else
/* Musl arch/x86_64/bits/stat.h: 144 bytes. */
struct stat {
	dev_t st_dev;
	ino_t st_ino;
	nlink_t st_nlink;

	mode_t st_mode;
	uid_t st_uid;
	gid_t st_gid;
	unsigned int    __pad0;
	dev_t st_rdev;
	off_t st_size;
	blksize_t st_blksize;
	blkcnt_t st_blocks;

	struct timespec st_atim;
	struct timespec st_mtim;
	struct timespec st_ctim;
	long unused[3];
};
#endif"""

s = s.replace(old_comment + s[s.index(old_comment) + len(old_comment):s.index(old_struct) + len(old_struct)][len(old_comment) - len(old_comment):], new, 1) \
    if False else s.replace(old_comment, "@@MARK@@", 1)
s = s.replace("@@MARK@@" + old_struct[len("struct stat {"):], new, 1)
assert "@@MARK@@" not in s, "failed to splice the new definition"
open(p, "w").write(s)
PYEOF
else
	echo "struct stat arm64 layout: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 13. The arm64 signal trampoline does not assemble.
#
# lib/posix-process/arch/arm64/signal.S uses SP as an operand of AND, which
# AArch64 does not encode (only ADD/SUB/MOV take SP):
#
#   signal.S:37: Error: operand 2 must be an integer register -- `and sp,sp,#~0xf'
#   signal.S:55: Error: operand 2 must be an integer register -- `and x11,sp,#~(16-1)'
#
# So CONFIG_LIBPOSIX_PROCESS_SIGNAL has never been buildable on arm64, and
# without it no CPU fault reaches the application as a signal. node needs it:
# OpenSSL probes for the SM3 extension by *executing* an SM3 instruction under
# sigsetjmp and catching the resulting SIGILL, which is how it discovers that
# Apple silicon does not implement SM3. Without signal delivery that probe is
# a fatal trap.
#
# The fix is the usual idiom -- materialise SP into a scratch register first.
# x9 is free at both sites (x19/x20 hold the arguments, x10/x11 the auxspcb
# fields, and x21..x24 are saved further down).
# ---------------------------------------------------------------------------
SIGS="$UK/lib/posix-process/arch/arm64/signal.S"
if [ -f "$SIGS" ] && grep -q "and sp, sp," "$SIGS"; then
	echo "patching $SIGS (SP is not a valid AND operand on AArch64)"
	python3 - "$SIGS" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old1 = """	sub sp, sp, #UK_LCPU_SYSCTX_SIZE  /* reserve space for ukarch sysctx */
	and sp, sp, #~0xf                         /* mare sure we're aligned */
	mov x0, sp                                /* prepare args */"""
new1 = """	sub x9, sp, #UK_LCPU_SYSCTX_SIZE  /* reserve space for ukarch sysctx */
	and x9, x9, #~0xf                         /* make sure we're aligned  */
	mov sp, x9                                /* SP is not an AND operand */
	mov x0, sp                                /* prepare args */"""

old2 = """	/* Align auxsp */
	and	x11, sp, #~(UKARCH_AUXSP_ALIGN - 1)
	str	x11, [x10]"""
new2 = """	/* Align auxsp. As above, SP cannot be an operand of AND. */
	mov	x9, sp
	and	x11, x9, #~(UKARCH_AUXSP_ALIGN - 1)
	str	x11, [x10]"""

assert old1 in s, "signal.S sysctx alignment does not match"
assert old2 in s, "signal.S auxsp alignment does not match"
open(p, "w").write(s.replace(old1, new1, 1).replace(old2, new2, 1))
PYEOF
else
	echo "arm64 signal trampoline: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 14. arm64 W^X is enforced with SCTLR_EL1.WXN, which no JIT can survive.
#
# plat/common/w_xor_x.c protects kernel sections by walking the memory regions
# and setting page protections -- except on arm64, where it *skips every
# writable region* and instead sets SCTLR_EL1.WXN:
#
#     /* Skip RW regions. These will be protected by WXN */
#     ...
#     /* Enable WXN to protect RW regions.
#      * This saves us from manually updating the PTEs.
#      */
#
# WXN is not a per-page attribute. It is a control for the entire EL1&0
# translation regime: while it is set, *any* writable page is execute-never,
# whatever its PTE says. Unikraft runs the application at EL1, so this applies
# to application memory too, and no page can ever be both writable and
# executable.
#
# That is fatal to anything that generates code at run time. node/V8 keeps its
# code space RWX; the first jump into JIT-ed code takes an instruction abort
# (ESR EC=0x21, FSC=0x0f: permission fault on a *present* page) even though the
# VMA is RWX and ukvmem agrees the access is allowed.
#
# x86_64 has no equivalent global control, so there the same Kconfig option
# only adjusts kernel section PTEs and applications are unaffected -- which is
# why node works there and not here.
#
# The fix is to do on arm64 what x86_64 already does: stop skipping writable
# regions, let the existing loop set them R+W (which makes them XN through the
# normal PTE path), and drop the global WXN. Kernel regions end up with exactly
# the same protections; only the blanket restriction on application memory
# goes away.
# ---------------------------------------------------------------------------
WXORX="$UK/plat/common/w_xor_x.c"
if [ -f "$WXORX" ] && grep -q "enable_wxn();" "$WXORX"; then
	echo "patching $WXORX (per-region XN instead of a global SCTLR_EL1.WXN)"
	python3 - "$WXORX" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old_skip = """#ifdef CONFIG_ARCH_ARM_64
		/* Skip RW regions. These will be protected by WXN */
		if (d->flags & UKPLAT_MEMRF_WRITE)
			continue;
#endif /* CONFIG_ARCH_ARM64 */

"""
new_skip = """		/* Writable regions are *not* skipped on arm64. They used to
		 * be, with SCTLR_EL1.WXN left to cover them, but that bit
		 * applies to the whole EL1&0 regime -- including the
		 * application -- and so forbids every RWX page a JIT needs.
		 * Setting R+W here marks them XN through the normal PTE path
		 * instead, which is what x86_64 has always done.
		 */

"""
assert old_skip in s, "w_xor_x.c RW skip does not match"
s = s.replace(old_skip, new_skip, 1)

old_en = """#ifdef CONFIG_ARCH_ARM_64
	/* Enable WXN to protect RW regions.
	 * This saves us from manually updating the PTEs.
	 */
	uk_pr_debug("Enabling WXN\\n");
	enable_wxn();
	uk_paging_tlb_flush();
#endif /* CONFIG_ARCH_ARM64 */
"""
new_en = """#ifdef CONFIG_ARCH_ARM_64
	/* Note: SCTLR_EL1.WXN is deliberately NOT set. It would make every
	 * writable page in the EL1&0 regime execute-never regardless of its
	 * PTE, which no run-time code generator can work with. The loop above
	 * has already given each region its own protections.
	 */
	uk_paging_tlb_flush();
#endif /* CONFIG_ARCH_ARM64 */
"""
assert old_en in s, "w_xor_x.c WXN enable does not match"
s = s.replace(old_en, new_en, 1)

# enable_wxn() is now unused and would be a -Werror build failure.
old_macro = """#ifdef CONFIG_ARCH_ARM_64
#define enable_wxn() ({					\\
	__u64 reg;					\\
	reg = UK_ARCH_ARM64_SYSREG_READ64(SCTLR_EL1);	\\
	reg |= UK_ARCH_ARM64_SCTLR_EL1_WXN_BIT;		\\
	UK_ARCH_ARM64_SYSREG_WRITE64(SCTLR_EL1, reg);	\\
	uk_arch_arm64_isb();				\\
})
#endif /* CONFIG_ARCH_ARM_64 */
"""
assert old_macro in s, "w_xor_x.c enable_wxn macro does not match"
s = s.replace(old_macro, "", 1)

open(p, "w").write(s)
PYEOF
else
	echo "arm64 W^X via WXN: already patched or absent, skipping"
fi

echo "patches applied."
