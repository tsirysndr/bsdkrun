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

# ---------------------------------------------------------------------------
# 15. mremap() is missing entirely, which breaks musl's stack sizing.
#
# lib/posix-mmap never defines mremap, so the syscall returns ENOSYS. That is
# not merely a missing feature: musl uses mremap to discover how large the
# *initial* thread's stack is. pthread_getattr_np() has no thread descriptor to
# read for it, so it starts above the auxiliary vector and probes downward a
# page at a time, extending its estimate for as long as the probe reports
# ENOMEM. From ld-musl-aarch64.so.1:
#
#     bl   mremap
#     cmn  x0, #0x1          ; returned -1?
#     b.ne exit
#     bl   __errno_location
#     cmp  w0, #0xc          ; 0xc = ENOMEM
#     b.eq loop              ; keep probing ONLY on ENOMEM
#     exit                   ; a->_a_stacksize = l
#
# ENOSYS leaves the loop immediately with the estimate still at one page, so
# musl reports a 4 KiB stack. JavaScriptCore takes its stack bounds from that
# and aborts on its first bounds check, which is why Bun died before running
# any JavaScript. glibc sizes the initial stack differently, which is why node
# and Deno are unaffected.
#
# What this implements, and what it deliberately does not:
#
#   * shrink - unmap the tail, address unchanged.
#   * grow in place - map the remainder at the exact address. Without
#     UK_VMA_MAP_REPLACE that fails with EEXIST if anything is in the way,
#     which is exactly the "can this grow?" question, and ukvmem merges the new
#     VMA into the old one.
#   * moving is NOT implemented. Relocating a mapping without copying needs
#     page-table surgery ukvmem does not expose, and copying would fault in
#     every page of a demand-paged region -- turning a 1 GiB mremap into 1 GiB
#     of committed memory. A grow that cannot happen in place returns ENOMEM,
#     including under MREMAP_MAYMOVE.
#
# ENOMEM is a documented mremap failure and callers already handle it: glibc's
# realloc falls back to malloc+memcpy+free, exactly as it does today against
# ENOSYS. musl's probe never needs a move either -- every call it makes either
# cannot grow in place (ENOMEM) or is below the stack (EFAULT, which ends the
# loop) -- so this is enough to size the stack correctly.
#
# Caveat worth knowing: the estimate musl arrives at is an over-estimate here,
# because ukvmem packs VMAs contiguously and the probe walks straight out of
# the stack into whatever is mapped below it, stopping only at the first hole.
# Linux gets the exact size because the main stack has a guard gap beneath it.
# What actually bounds the application is CONFIG_APPELFLOADER_STACK_NBPAGES.
# ---------------------------------------------------------------------------
MMAPC="$UK/lib/posix-mmap/mmap.c"
if [ -f "$MMAPC" ] && ! grep -q "mremap" "$MMAPC"; then
	echo "patching $MMAPC (implement mremap)"
	python3 - "$MMAPC" <<'PYEOF2'
import sys

p = sys.argv[1]
s = open(p).read()

anchor = """UK_SYSCALL_R_DEFINE(int, msync, void*, addr, size_t, length, int, flags)"""
assert anchor in s, "mmap.c does not contain the expected msync definition"

impl = r"""/* Not provided by nolibc's <sys/mman.h>. */
#ifndef MREMAP_MAYMOVE
#define MREMAP_MAYMOVE		0x01
#endif
#ifndef MREMAP_FIXED
#define MREMAP_FIXED		0x02
#endif
#ifndef MREMAP_DONTUNMAP
#define MREMAP_DONTUNMAP	0x04
#endif

/* Check that [vaddr, vaddr + len) is completely covered by mappings, and
 * return the VMA containing vaddr. Linux requires the source of an mremap to
 * live in a single VMA; we only require it to be contiguously mapped, because
 * ukvmem splits VMAs for reasons of its own that the application cannot see.
 */
static int mremap_src_lookup(struct uk_vas *vas, __vaddr_t vaddr, __sz len,
			     const struct uk_vma **first)
{
	const struct uk_vma *vma;
	__vaddr_t cur = vaddr;

	vma = uk_vma_find(vas, cur);
	if (unlikely(!vma))
		return -EFAULT;

	*first = vma;

	while (cur < vaddr + len) {
		vma = uk_vma_find(vas, cur);
		if (unlikely(!vma))
			return -EFAULT;

		UK_ASSERT(vma->end > cur);
		cur = vma->end;
	}

	return 0;
}

static int do_mremap(void **addr, __sz old_len, __sz new_len, int flags)
{
	struct uk_vas *vas = uk_vas_get_active();
	__vaddr_t old_va = (__vaddr_t)*addr;
	const struct uk_vma *vma;
	unsigned long vma_attr, vma_flags, map_flags;
	const struct uk_vma_ops *vma_ops;
	const char *vma_name;
	__vaddr_t ext_va;
	int vma_page_lvl;
	__sz ext_len;
	int rc;

	if (unlikely(!vas))
		return -EINVAL;

	if (unlikely(!UK_PAGING_PAGE_ALIGNED(old_va)))
		return -EINVAL;

	/* new_len == 0 is invalid; old_len == 0 asks to duplicate a shared
	 * mapping, which we do not support.
	 */
	if (unlikely(new_len == 0 || old_len == 0))
		return -EINVAL;

	if (unlikely(flags & ~(MREMAP_MAYMOVE | MREMAP_FIXED |
			       MREMAP_DONTUNMAP)))
		return -EINVAL;

	if (unlikely((flags & (MREMAP_FIXED | MREMAP_DONTUNMAP)) &&
		     !(flags & MREMAP_MAYMOVE)))
		return -EINVAL;

	if (unlikely(flags & MREMAP_DONTUNMAP))
		return -EINVAL;

	if (unlikely(old_len > __SZ_MAX - UK_PAGING_PAGE_SIZE ||
		     new_len > __SZ_MAX - UK_PAGING_PAGE_SIZE))
		return -ENOMEM;

	old_len = UK_PAGING_PAGE_ALIGN_UP(old_len);
	new_len = UK_PAGING_PAGE_ALIGN_UP(new_len);

	if (unlikely(old_va > __VADDR_MAX - old_len ||
		     old_va > __VADDR_MAX - new_len))
		return -EINVAL;

	rc = mremap_src_lookup(vas, old_va, old_len, &vma);
	if (unlikely(rc))
		return rc;

	/* Any operation on the address space may free the VMA, so take a copy
	 * of everything needed before touching it (see uk_vma_find()).
	 */
	vma_ops      = vma->ops;
	vma_attr     = vma->attr;
	vma_flags    = vma->flags;
	vma_page_lvl = vma->page_lvl;
	vma_name     = vma->name;

	if (new_len == old_len)
		return 0;

	if (new_len < old_len) {
		rc = uk_vma_unmap(vas, old_va + new_len, old_len - new_len, 0);
		if (unlikely(rc && rc != -ENOENT))
			return rc;

		return 0;
	}

	/* Growing. We never relocate, so a request that insists on a new
	 * address cannot be satisfied.
	 */
	if (unlikely(flags & MREMAP_FIXED))
		return -ENOMEM;

	/* Only anonymous memory can be extended: growing a file mapping would
	 * need the file and the offset to continue from, and the public VMA
	 * interface does not expose either.
	 */
	if (vma_ops != &uk_vma_anon_ops)
		return -ENOMEM;

	/* Reproduce the source's mapping flags so that ukvmem merges the
	 * extension into it. Anything that would not merge (a large-page VMA)
	 * is refused rather than left as a silent split.
	 */
	if (unlikely(vma_page_lvl >= 0))
		return -ENOMEM;

	map_flags = vma_flags & (UK_VMA_MAP_EXTF_MASK << UK_VMA_MAP_EXTF_SHIFT);
	if (vma_flags & UK_VMA_FLAG_UNINITIALIZED)
		map_flags |= UK_VMA_MAP_UNINITIALIZED;

	ext_va  = old_va + old_len;
	ext_len = new_len - old_len;

	/* No UK_VMA_MAP_REPLACE: this must fail rather than evict whatever is
	 * above the mapping. EEXIST is how ukvmem says "occupied", which for
	 * mremap is ENOMEM.
	 */
	rc = uk_vma_map(vas, &ext_va, ext_len, vma_attr, map_flags, vma_name,
			&uk_vma_anon_ops, __NULL);
	if (unlikely(rc)) {
		if (rc == -EEXIST)
			return -ENOMEM;

		return rc;
	}

	return 0;
}

/* <sys/mman.h> declares mremap variadic, because the fifth argument only
 * exists with MREMAP_FIXED. A fixed-arity UK_SYSCALL_DEFINE would conflict
 * with that prototype, so define the raw syscall and the libc entry point
 * separately -- the same split lib/ukmmap uses.
 */
UK_LLSYSCALL_R_DEFINE(long, mremap, void *, old_address, size_t, old_size,
		      size_t, new_size, int, flags, void *, new_address)
{
	void *addr = old_address;
	int rc;

	(void)new_address; /* only used by MREMAP_FIXED, which is refused */

	rc = do_mremap(&addr, old_size, new_size, flags);
	if (unlikely(rc))
		return (long)rc; /* negative errno, as Linux returns */

	return (long)(__uptr)addr;
}

#if UK_LIBC_SYSCALLS
void *mremap(void *old_address, size_t old_size, size_t new_size, int flags,
	     ...)
{
	void *addr = old_address;
	int rc;

	rc = do_mremap(&addr, old_size, new_size, flags);
	if (unlikely(rc)) {
		errno = -rc;
		return MAP_FAILED;
	}

	return addr;
}
#endif /* UK_LIBC_SYSCALLS */

"""

open(p, "w").write(s.replace(anchor, impl + anchor, 1))
PYEOF2
else
	echo "mremap: already patched or absent, skipping"
fi

# Registered separately from the code above: the two edits are independent, and
# nesting this inside that guard skips it on a tree where only one of them has
# been made. Without the declaration the shim generates no table entry, so a
# binary syscall from the application still lands on the "not implemented"
# stub even though the code is compiled in.
MK="$UK/lib/posix-mmap/Makefile.uk"
if [ -f "$MK" ] && ! grep -q "mremap" "$MK"; then
	echo "patching $MK (declare mremap-5)"
	sed -i.bak 's#^UK_PROVIDED_SYSCALLS-$(CONFIG_LIBPOSIX_MMAP) += munmap-2$#&\
UK_PROVIDED_SYSCALLS-$(CONFIG_LIBPOSIX_MMAP) += mremap-5#' "$MK"
	rm -f "$MK.bak"
	grep -q 'mremap-5' "$MK" || { echo "failed to register mremap" >&2; exit 1; }
else
	echo "mremap syscall registration: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 16. CONFIG_LIBPOSIX_PROCESS_SIGNALFD cannot be built on arm64.
#
# lib/posix-process/signal/signal_file.c defines both signalfd4() and the
# three-argument legacy signalfd(), but lib/syscall_shim's legacy list does not
# mention the latter. arm64 is one of the architectures that never had a
# __NR_signalfd -- glibc and musl both reach signalfd() through signalfd4 --
# so the generated table has nowhere to put it and the build stops with
#
#   .../uk/bits/syscall_provided.h:898:2: error: #error Failed to map system
#   call 'signalfd': No system call number available
#
# in every translation unit that includes it. Turning the option on is
# therefore impossible on arm64, which matters because PostgreSQL 13 and later
# read signals through a file descriptor and abort at startup with
# "FATAL: signalfd() failed" without it (see ../../../examples/unikraft-postgres).
#
# The list exists for exactly this case -- eventfd, epoll_create, poll, dup2
# and a dozen others are already on it -- so the fix is the missing line. A
# legacy entry only suppresses the "no number on this architecture" error; the
# x86_64 build still gets its __NR_signalfd table entry as before.
# ---------------------------------------------------------------------------
LEG="$UK/lib/syscall_shim/include/uk/legacy_syscall.h"
if [ -f "$LEG" ] && ! grep -q 'LEGACY_SYS_signalfd' "$LEG"; then
	echo "patching $LEG (mark signalfd legacy; arm64 has only signalfd4)"
	sed -i.bak 's|^#define LEGACY_SYS_eventfd /\* modern: eventfd2 \*/$|&\
#define LEGACY_SYS_signalfd /* modern: signalfd4 */|' "$LEG"
	rm -f "$LEG.bak"
	grep -q 'LEGACY_SYS_signalfd' "$LEG" || {
		echo "failed to mark signalfd legacy" >&2
		exit 1
	}
else
	echo "legacy_syscall.h: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 17. A leading zero-length iovec entry fails the whole writev() with EIO.
#
# POSIX says a zero-length iovec entry contributes nothing; Linux skips them.
# lwip does not. lwip_sendmsg() hands the vectors to
# netconn_write_vectors_partly() unfiltered, lwip_netconn_do_writemore() walks
# them one by one, and tcp_write() rejects a NULL data pointer
#
#   LWIP_ERROR("tcp_write: arg == NULL (programmer violates API)",
#              arg != NULL, return ERR_ARG;);
#
# *before* tcp_write_checks() gets to its `len == 0 -> ERR_OK` shortcut. The
# ERR_ARG comes back to the application as EIO -- err_to_errno() maps ERR_ARG
# to EIO -- so a single empty entry fails a writev() that would have succeeded
# on Linux, and no byte of the real payload is sent.
#
# Erlang's inet driver produces exactly that shape. erts/emulator/drivers/
# common/inet_drv.c reserves ev->iov[0] for the packet-length header and fills
# it in only when there is one:
#
#     if (h_len > 0) { ev->iov[0].iov_base = buf; ... }
#     ...
#     sock_sendv(desc->inet.s, ev->iov, vsize, &n, 0)   /* = writev() */
#
# An HTTP server's socket is `{packet, raw}`, so h_len is 0, iov[0] stays
# {NULL, 0}, and every response write returns EIO. Cowboy closes the
# connection without sending anything and curl reports an empty reply -- see
# ../../../examples/unikraft-elixir.
#
# The fix is in posix-socket rather than in lib-lwip: the normalisation is
# POSIX behaviour that every socket driver is entitled to assume, and this is
# the one place all of them funnel through. Only leading entries are skipped,
# which is the structural case above; ERTS builds the rest of the vector from
# io_list_to_vec(), which never emits an empty one.
# ---------------------------------------------------------------------------
SOCK="$UK/lib/posix-socket/socket.c"
if [ -f "$SOCK" ] && ! grep -q 'while (iovcnt && !iov\[0\].iov_len)' "$SOCK"; then
	echo "patching $SOCK (skip leading zero-length iovec entries on write)"
	sed -i.bak 's|^\tif (d->ops->write) {$|\t/* A zero-length entry is a no-op per POSIX, but lwip'"'"'s tcp_write()\n\t * rejects its NULL pointer with ERR_ARG and fails the whole call\n\t * with EIO. Erlang'"'"'s inet driver always leads with one.\n\t */\n\twhile (iovcnt \&\& !iov[0].iov_len) {\n\t\tiov++;\n\t\tiovcnt--;\n\t}\n\tif (unlikely(!iovcnt)) {\n\t\tuk_file_runlock(sock);\n\t\treturn 0;\n\t}\n&|' "$SOCK"
	rm -f "$SOCK.bak"
	grep -q 'while (iovcnt && !iov\[0\].iov_len)' "$SOCK" || {
		echo "failed to patch socket_write" >&2
		exit 1
	}
else
	echo "posix-socket/socket.c: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 16. epoll_pwait() refuses any request that carries a signal mask.
#
# lib/posix-poll implements the wait but gives up as soon as a mask is present:
#
#     if (unlikely(sigmask)) {
#             uk_pr_warn_once("STUB: epoll_pwait no sigmask support\n");
#             return -ENOSYS;
#     }
#
# On arm64 there is no epoll_wait syscall at all -- musl's epoll_wait() is
# epoll_pwait() with a NULL mask -- so this only bites callers that actually
# pass one. node and Deno do not; Bun does, and its event loop spins on ENOSYS
# forever (29,000 calls while a single request sat unanswered), so the server
# listens but never replies.
#
# The mask is applied around the wait rather than inside it: uk_sys_epoll_pwait
# returns from several places in its polling loop, and wrapping at the syscall
# entry point keeps the save/restore on one path. That is also where Linux's
# semantics are easiest to read -- set the mask, wait, put the old one back.
#
# The swap is not atomic with the start of the wait, unlike Linux. Closing that
# window needs the wait itself to take the mask, which means touching the loop.
# It matters only if a signal arrives between the two, and it is strictly
# better than refusing the call.
#
# With CONFIG_LIBPOSIX_PROCESS_SIGNAL off there is no signal delivery for the
# mask to affect, so the request is simply honoured with the mask ignored --
# still better than ENOSYS.
# ---------------------------------------------------------------------------
EPOLLC="$UK/lib/posix-poll/epoll.c"
if [ -f "$EPOLLC" ] && ! grep -q "epoll_pwait_sigmask_enter" "$EPOLLC"; then
	echo "patching $EPOLLC (honour the epoll_pwait signal mask)"
	python3 - "$EPOLLC" <<'PYEOF3'
import sys

p = sys.argv[1]
s = open(p).read()

# 1. Drop the ENOSYS bail-out; the mask is handled by the callers below.
stub = """	if (unlikely(sigmask)) {
		uk_pr_warn_once("STUB: epoll_pwait no sigmask support\\n");
		return -ENOSYS;
	}

"""
assert stub in s, "epoll.c does not contain the expected sigmask stub"
s = s.replace(stub, "", 1)

# 2. Helpers that swap the thread's signal mask around the wait.
# Insert ahead of epoll_pwait2, which is the first of the two users.
anchor = """UK_SYSCALL_R_DEFINE(int, epoll_pwait2, int, epfd, struct epoll_event *, events,"""
assert anchor in s, "epoll.c does not contain the expected epoll_pwait2 definition"

# SIG_SETMASK comes from <signal.h>, which epoll.c does not include.
inc_old = """#include <errno.h>
"""
inc_new = """#include <errno.h>
#include <signal.h>
"""
assert inc_old in s, "epoll.c include block does not match"
s = s.replace(inc_old, inc_new, 1)

helpers = """/* Apply the caller's signal mask for the duration of the wait, returning the
 * previous one so it can be restored. rt_sigprocmask is reached through the
 * syscall shim because the mask lives in posix-process's private per-thread
 * state, and posix-poll does not depend on that library.
 */
static int epoll_pwait_sigmask_enter(const sigset_t *sigmask, size_t sigsetsize,
				     sigset_t *oldmask)
{
#if CONFIG_LIBPOSIX_PROCESS_SIGNAL
	if (!sigmask)
		return 0;

	return uk_syscall_r_rt_sigprocmask(SIG_SETMASK, (long)sigmask,
					   (long)oldmask, (long)sigsetsize);
#else /* !CONFIG_LIBPOSIX_PROCESS_SIGNAL */
	/* Nothing delivers signals in this build, so the mask cannot change
	 * what the wait observes. Honour the call and ignore it.
	 */
	(void)sigmask;
	(void)sigsetsize;
	(void)oldmask;
	return 0;
#endif /* !CONFIG_LIBPOSIX_PROCESS_SIGNAL */
}

static void epoll_pwait_sigmask_leave(const sigset_t *sigmask,
				      size_t sigsetsize, sigset_t *oldmask)
{
#if CONFIG_LIBPOSIX_PROCESS_SIGNAL
	if (!sigmask)
		return;

	uk_syscall_r_rt_sigprocmask(SIG_SETMASK, (long)oldmask, 0,
				    (long)sigsetsize);
#else /* !CONFIG_LIBPOSIX_PROCESS_SIGNAL */
	(void)sigmask;
	(void)sigsetsize;
	(void)oldmask;
#endif /* !CONFIG_LIBPOSIX_PROCESS_SIGNAL */
}

"""
s = s.replace(anchor, helpers + anchor, 1)

# 3. Wrap both entry points.
old_pwait = """	r = uk_sys_epoll_pwait(of->file, events, maxevents,
			       timeout, sigmask, sigsetsize);
	uk_ofile_release(of);
	return r;"""
new_pwait = """	if (unlikely(epoll_pwait_sigmask_enter(sigmask, sigsetsize, &oldmask))) {
		uk_ofile_release(of);
		return -EINVAL;
	}
	r = uk_sys_epoll_pwait(of->file, events, maxevents,
			       timeout, __NULL, sigsetsize);
	epoll_pwait_sigmask_leave(sigmask, sigsetsize, &oldmask);
	uk_ofile_release(of);
	return r;"""
assert old_pwait in s, "epoll_pwait body does not match"
s = s.replace(old_pwait, new_pwait, 1)

old_pwait2 = """	r = uk_sys_epoll_pwait2(of->file, events, maxevents,
				timeout, sigmask, sigsetsize);
	uk_ofile_release(of);
	return r;"""
new_pwait2 = """	if (unlikely(epoll_pwait_sigmask_enter(sigmask, sigsetsize, &oldmask))) {
		uk_ofile_release(of);
		return -EINVAL;
	}
	r = uk_sys_epoll_pwait2(of->file, events, maxevents,
				timeout, __NULL, sigsetsize);
	epoll_pwait_sigmask_leave(sigmask, sigsetsize, &oldmask);
	uk_ofile_release(of);
	return r;"""
assert old_pwait2 in s, "epoll_pwait2 body does not match"
s = s.replace(old_pwait2, new_pwait2, 1)

# 4. Both wrappers need the oldmask local.
for sig in ("""		      int, maxevents, int, timeout,
		      const sigset_t *, sigmask, size_t, sigsetsize)
{
	int r;""",
            """		    int, maxevents, struct timespec *, timeout,
		    const sigset_t *, sigmask, size_t, sigsetsize)
{
	int r;"""):
    assert sig in s, "wrapper prologue does not match"
    s = s.replace(sig, sig + "\n\tsigset_t oldmask;", 1)

open(p, "w").write(s)
PYEOF3
else
	echo "epoll_pwait sigmask: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 18. execve() never fills in argc/envc, so the loader reads garbage counts.
#
# lib/posix-process/execve.c declares `struct uk_binfmt_loader_args loader_args`
# as a plain stack local and then sets every field of it -- pathname, progname,
# argv, envp, alloc, ctx, stack_size, loader, user -- except the two counts:
#
#     loader_args.argv = (const char **)argv;
#     loader_args.envp = (const char **)envp;
#
# `argc` and `envc` keep whatever was on the stack. A loader that trusts them
# walks off the end of the vectors and dereferences whatever it finds:
#
#   CRIT: Unikraft Crash - Ijiraq (0.21.0)
#   ESR_EL1: 0x0000000096000006     (data abort, translation fault, EL1)
#   ELR_EL1: 0x000000008015e6b8     -> elfloader_rs::sys::cstr
#   LR:      0x000000008015ea34     -> elfloader_rs::binfmt::vec_from_c
#
# Nothing in-tree caught this because nothing in-tree reads the counts:
# lib/ukbinfmt never mentions argc, and the C app-elfloader walks argv to its
# NULL terminator instead. The struct offers the counts, though, and a loader is
# entitled to believe them -- app-elfloader-rs does.
#
# The fix is to count the vectors, which the caller has already NULL-terminated
# (Linux's execve(2) contract). Both may be NULL: Linux treats a NULL argv or
# envp as an empty list, which the loop below produces as a count of zero.
# ---------------------------------------------------------------------------
EXECVE="$UK/lib/posix-process/execve.c"
if [ -f "$EXECVE" ] && ! grep -q 'loader_args.argc' "$EXECVE"; then
	echo "patching $EXECVE (count argv/envp for the binfmt loader)"
	python3 - "$EXECVE" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old = """	loader_args.argv = (const char **)argv;
	loader_args.envp = (const char **)envp;
"""
new = """	loader_args.argv = (const char **)argv;
	loader_args.envp = (const char **)envp;

	/* Count both vectors. They are NULL-terminated per execve(2), and a
	 * NULL vector is an empty list. Without this, argc/envc keep whatever
	 * was on the stack and a loader that trusts them reads past the end.
	 */
	loader_args.argc = 0;
	if (argv)
		while (argv[loader_args.argc])
			loader_args.argc++;

	loader_args.envc = 0;
	if (envp)
		while (envp[loader_args.envc])
			loader_args.envc++;
"""
assert old in s, "execve.c does not match the expected shape"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "execve.c argc/envc: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 20. ppoll() refuses a signal mask of its own accord, before epoll ever sees it.
#
# The section above teaches uk_sys_epoll_pwait2() to honour a mask, and
# uk_sys_ppoll() already forwards one to it -- but not before failing on its
# own copy of the same stub:
#
#	if (unlikely(sigmask)) {
#		uk_pr_warn_once("STUB: ppoll no sigmask support\n");
#		return -ENOSYS;
#	}
#
# so ppoll() with a mask still returns ENOSYS. Dropping the block is the whole
# fix; the mask then reaches the machinery that now handles it.
#
# This is not a corner case on arm64. aarch64 has no poll() system call at all,
# so glibc implements poll() as ppoll(), and *every* glibc program that polls a
# file descriptor arrives here. MySQL is one: its connection layer does a
# non-blocking read, gets EAGAIN, and waits for the socket to become readable.
# When that wait fails it declares the connection broken --
#
#	[Note] Got an error reading communication packets
#
# -- and hangs up on the client mid-handshake, having already sent its greeting.
# Programs that use epoll directly (node, Deno, Actix) never notice.
# ---------------------------------------------------------------------------
POLLC="$UK/lib/posix-poll/poll.c"
if [ -f "$POLLC" ] && grep -q "STUB: ppoll no sigmask support" "$POLLC"; then
	echo "patching $POLLC (let ppoll pass its signal mask through)"
	python3 - "$POLLC" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

stub = """	if (unlikely(sigmask)) {
		uk_pr_warn_once("STUB: ppoll no sigmask support\\n");
		return -ENOSYS;
	}
"""
assert stub in s, "poll.c does not contain the expected sigmask stub"
open(p, "w").write(s.replace(stub, "", 1))
PYEOF
else
	echo "poll.c ppoll sigmask: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 21. Preempt application code on a timer tick (preempt.patch).
#
# The scheduler is cooperative -- ukschedcoop is the only one in the tree, and
# it says so itself -- so a thread that never enters the kernel is never
# descheduled. That is fine for programs that block in syscalls and fatal for
# ones that busy-wait on another thread. InnoDB does exactly that:
#
#   void IB_thread::start() {
#     m_state->store(State::ALLOWED_TO_START);
#     wait(State::STARTED);        // spins; no yield, no syscall in the loop
#   }
#
# The thread it waits for cannot run until the waiter yields, and the waiter
# never does. mysqld hangs forever at "InnoDB initialization has started", on
# both architectures. See examples/unikraft-mysql/repro/.
#
# The patch adds CONFIG_UKPLAT_PREEMPT (default off) and, when set:
#
#   * arm64 gets a periodic timer tick. It had none -- the generic timer is
#     armed only by time_block_until(), i.e. when a thread blocks, so a guest
#     whose threads all spin takes no interrupts at all. x86_64 already has
#     one: the PIT is left in rate-generator mode at CONFIG_HZ.
#
#   * The interrupt return path reschedules, when the interrupted PC lies
#     outside [__TEXT, __ETEXT). Kernel code may hold a lock or be running on
#     the auxiliary syscall stack, and does not spin on other threads;
#     application code is what needs preempting.
#
# The switch cannot happen in the interrupt handler: both architectures take
# interrupts on a per-LCPU stack (arm64 via except_stack_base, x86_64 via
# IST 1) that the next interrupt reuses, so a thread suspended there would
# lose its frame. Instead the frame is copied onto the interrupted thread's
# own stack and the return is redirected to a trampoline that runs there,
# yields like any other thread, and restores the frame afterwards -- so the
# interrupt "returns" arbitrarily later, from a different call stack.
#
# The trampoline saves the extended context by hand. uk_sched_thread_switch()
# deliberately does not, because a cooperative switch only ever happens at a
# call boundary; a preempted thread is suspended mid-instruction-stream with
# live vector registers. Interrupt handlers may not touch that state (x86_64
# asserts it under CONFIG_LIBUKPLAT_NATIVE_ECTX_ISR_ASSERTIONS, and arm64
# builds -mgeneral-regs-only), so it is still intact when the trampoline runs.
# ---------------------------------------------------------------------------
if [ -f "$UK/plat/Config.uk" ] && ! grep -q "UKPLAT_PREEMPT" "$UK/plat/Config.uk"; then
	echo "applying preempt.patch (timer-tick preemption of application code)"
	patch -p1 -d "$UK" -i "$HERE/preempt.patch" >/dev/null
else
	echo "preemption: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 19. arm64 execve() enters the new program at the wrong register: it has never
#     worked.
#
# lib/posix-process/arch/arm64/execve.c builds the execution environment the
# new program is resumed with, and sets the entry point like this:
#
#     uk_lcpu_regs_set(execenv_new->regs, LR, ip);
#     uk_lcpu_regs_set(execenv_new->regs, SP, sp);
#     /* Leave gpregs and ectx uninitialized for the new
#      * execution context.
#      */
#
# But nothing returns to LR. arch/arm/arm64/execenv.S restores ELR_EL1 from the
# PC slot and leaves through `eret`:
#
#     /* Restore LR and exception PC */
#     ldp     x30, x21, [sp, #16 * 15]
#     msr     elr_el1, x21
#     ...
#     eret
#
# So the entry point is written to a register the CPU never jumps to, and the
# one it does jump to is left "uninitialized" -- which in practice is whatever
# is in the freshly allocated stack the execenv was carved out of. That is
# usually zero, so the new program starts executing at address 0:
#
#   CRIT: [libposix_process] Cannot deliver SIGSEGV for pf at 0x0
#   (with the fault taken at pc=0x0, sp=<the new stack>)
#
# The x86_64 version of the same function sets RIP, which is why this was never
# noticed: upstream's base image is x86_64-only. On arm64 it means execve() has
# never worked at all -- and with it, every multiprocess application, since
# Unikraft creates processes with vfork() + execve().
#
# LR is set to 0 rather than to `ip`: the AArch64 process-entry ABI leaves it
# undefined, and 0 turns a stray `ret` in _start into an immediate, obvious
# fault instead of a jump back into the loader.
# ---------------------------------------------------------------------------
AEXECVE="$UK/lib/posix-process/arch/arm64/execve.c"
if [ -f "$AEXECVE" ] && ! grep -q 'regs, PC, ip' "$AEXECVE"; then
	echo "patching $AEXECVE (enter the new program at PC, not LR)"
	python3 - "$AEXECVE" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old = """	uk_lcpu_regs_set(execenv_new->regs, LR, ip);
	uk_lcpu_regs_set(execenv_new->regs, SP, sp);
"""
new = """	/* PC, not LR: ukarch_execenv_load() restores ELR_EL1 from the PC slot
	 * and returns to the application with `eret`. Nothing ever branches to
	 * LR, so setting it here left the new program's entry point in a
	 * register the CPU does not use, and ELR_EL1 holding whatever was in
	 * the freshly allocated stack -- normally zero.
	 */
	uk_lcpu_regs_set(execenv_new->regs, PC, ip);
	uk_lcpu_regs_set(execenv_new->regs, LR, 0x0);
	uk_lcpu_regs_set(execenv_new->regs, SP, sp);
"""
assert old in s, "arm64 execve.c does not match the expected shape"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "arm64 execve.c: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 20. A signal blocked by the current thread is run in it anyway.
#
# lib/posix-process/signal/deliver.c delivers process-directed signals like
# this: for each pending signal, walk the process's threads, find one that does
# not block it, and deliver.
#
#     uk_pprocess_foreach_pthread(proc, thread, threadn) {
#             if (thread->tid == this_thread->tid)
#                     continue;
#             if (IS_MASKED(thread, signum))
#                     continue;
#             while ((sig = pprocess_signal_dequeue(proc, __NULL, signum))) {
#                     do_deliver(thread, sig, execenv);
#
# But `do_deliver()` cannot deliver to another thread. It calls `handle_self()`,
# which -- as the name says -- builds the signal frame on the *current* context:
# the execenv passed down is this thread's, and the handler runs on this
# thread's stack. The chosen `thread` is used only to look the handler up.
#
# The result is the exact inverse of what the check intends: a signal is run in
# the one thread that asked not to receive it, because some *other* thread did
# not block it.
#
# PostgreSQL trips over this immediately. Its postmaster blocks every signal
# while it installs handlers, and only afterwards initialises the latch those
# handlers use:
#
#     pqinitmask();
#     PG_SETMASK(&BlockSig);
#     pqsignal(SIGCHLD, handle_pm_child_exit_signal);   /* ... */
#     InitializeLatchSupport();
#     MyLatch = &MyLatchData;                           /* NULL until here */
#
# A child exits during that window (the postmaster popen()s `postgres -V` to
# version-check itself), SIGCHLD is delivered despite the mask, and the handler
# runs with MyLatch still NULL:
#
#   handle_pm_child_exit_signal -> SetLatch(NULL)
#   CRIT: [libposix_process] Cannot deliver SIGSEGV for pf at 0x0
#
# The fix is to leave the signal queued when the current thread blocks it.
# Delivery is not lost: it stays pending on the process queue and the thread
# that has it unblocked picks it up at its own next syscall exit, which is where
# it can actually be run. That is also what Linux does -- the handler runs in
# the thread that accepts the signal, not in one that blocked it.
# ---------------------------------------------------------------------------
DELIVER="$UK/lib/posix-process/signal/deliver.c"
if [ -f "$DELIVER" ] && ! grep -q 'leave it queued' "$DELIVER"; then
	echo "patching $DELIVER (do not run a signal the current thread blocks)"
	python3 - "$DELIVER" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old = """		/* POSIX specifies that if a signal targets the
		 * current process / thread, then at least one
		 * signal for this process /thread must be
		 * delivered before the syscall returns, as long as:
		 *
		 * 1. No other thread has that signal unblocked
		 * 2. No other thread is in sigwait() for that signal (TODO)
		 */
		uk_pprocess_foreach_pthread(proc, thread, threadn) {
			if (thread->tid == this_thread->tid)
				continue;

			if (IS_MASKED(thread, signum))
				continue;

			while ((sig = pprocess_signal_dequeue(proc, __NULL,
							      signum))) {
				do_deliver(thread, sig, execenv);
				uk_signal_free(proc->_a, sig);
				handled = true;
				handled_cnt++;
			}
			break;
		}

		/* Try to deliver to this thread */
		if (!handled) {
			if (IS_MASKED(this_thread, signum))
				continue;
"""
new = """		/* Deliver on this thread, or not at all.
		 *
		 * There used to be a loop here that looked for any thread of
		 * the process that did not block this signal and delivered on
		 * its behalf. That cannot work: do_deliver() -> handle_self()
		 * builds the signal frame on the *current* context, so the
		 * handler ran in this thread even when this thread was the one
		 * that had blocked the signal.
		 *
		 * If this thread blocks it, leave it queued. The thread that
		 * has it unblocked will take it at its own next syscall exit,
		 * which is the only place it can actually be run.
		 */
		if (!handled) {
			if (IS_MASKED(this_thread, signum))
				continue;
"""
assert old in s, "deliver_pending_proc does not match the expected shape"
s = s.replace(old, new, 1)

# `thread` / `threadn` were only used by the loop just removed.
old_decl = """	struct posix_thread *thread, *threadn;
	struct posix_thread *this_thread;
	struct uk_signal *sig;
	int handled_cnt = 0;
	bool handled;
	int signum;
"""
new_decl = """	struct posix_thread *this_thread;
	struct uk_signal *sig;
	int handled_cnt = 0;
	bool handled;
	int signum;
"""
assert old_decl in s, "deliver_pending_proc declarations do not match"
s = s.replace(old_decl, new_decl, 1)
open(p, "w").write(s)
PYEOF
else
	echo "deliver.c cross-thread delivery: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 21. setsid() always fails, so no daemon can start a child process.
#
# lib/posix-process/deprecated.c:
#
#     UK_SYSCALL_R_DEFINE(pid_t, setsid)
#     {
#             /* We have a single "session" with a single "process" */
#             return (pid_t) -EPERM;
#     }
#
# That comment describes a Unikraft without multiprocess support. With
# CONFIG_LIBPOSIX_PROCESS_MULTIPROCESS there are several processes, and putting
# a freshly spawned one "in its own session" is the first thing a well-behaved
# daemon child does. PostgreSQL does it in every child it spawns:
#
#   FATAL:  setsid() failed: Operation not permitted
#
# and the child exits before doing any work.
#
# Refusing is also inconsistent with the neighbouring getsid(), which reports
# UNIKRAFT_SID for whoever asks. There is exactly one session, every process is
# already in it, and the caller's request is therefore already satisfied --
# which is a success, not a permission error. Report the session it is in, as
# setsid(2) does. (Linux returns EPERM only when the caller is already a
# process group leader, i.e. when the new session would collide with an
# existing one; there are no such collisions here.)
# ---------------------------------------------------------------------------
DEPR="$UK/lib/posix-process/deprecated.c"
if [ -f "$DEPR" ] && ! grep -q 'single session and every process' "$DEPR"; then
	echo "patching $DEPR (setsid() reports the one session instead of EPERM)"
	python3 - "$DEPR" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old = """UK_SYSCALL_R_DEFINE(pid_t, setsid)
{
	/* We have a single "session" with a single "process" */
	return (pid_t) -EPERM;
}"""
new = """UK_SYSCALL_R_DEFINE(pid_t, setsid)
{
	/* There is a single session and every process is already in it, so the
	 * caller's request is satisfied by construction. Report that session,
	 * as setsid(2) does, rather than failing: a daemon that spawns children
	 * calls this in each one, and EPERM stops it dead. Consistent with
	 * getsid() below, which answers UNIKRAFT_SID for any process.
	 */
	return (pid_t) UNIKRAFT_SID;
}"""
assert old in s, "setsid() does not match the expected shape"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "setsid(): already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 22. A vfork child starts with the kernel's TLS as its userland TLS.
#
# lib/posix-process/clone.c, in the CLONE_VFORK branch:
#
#     /* We will be blocking the parent and pass control to the child
#      * via the scheduler. Therefore we need to set the child's TLS
#      * pointer the Unikraft TLS.
#      */
#     child->tlsp = child->uktlsp;
#
# tlsp is what the child wakes up with in its TLS register (the arch code
# copies it into FSBASE / TPIDR_EL0), so this starts the child in *userland*
# with the kernel's TLS block. vfork semantics are the opposite: the child is
# the parent, briefly -- on Linux it inherits the parent's registers including
# the thread pointer -- and everything musl's posix_spawn child() touches
# before execve() is TLS-relative: the stack-protector canary (%fs:0x28),
# errno, the pthread self pointer.
#
# The damage is architecture-lopsided. On arm64 the thread pointer names the
# START of the TLS block, so those accesses land inside the (wrong, kernel)
# block and are absorbed; that is the only reason PostgreSQL's spawns worked
# there. On x86_64 the thread pointer names the END (TLS variant 2), so the
# same accesses reach past the allocation: the child faults on its very first
# instructions, before its first syscall, and dies silently. Its death wakes
# the vfork parent, the status pipe reads EOF -- which posix_spawn interprets
# as success -- and the postmaster continues around a corpse. That shows up
# later, as a crash in pthread_exit's thread-list unlink over a kernel-heap
# "self" with garbage links, which is how it was found:
#
#   rip: 0x1001acb190 -> [ld-musl] pthread_exit+0x17f  (the unlink store)
#   rbx (self): 0x400713060                            (kernel heap)
#
# The fix is to give the child the parent's userland TLS, exactly as Linux
# does. The kernel-side Unikraft TLS is untouched: the child's uktlsp keeps
# serving syscall entry, and the scheduler's switch-in loads tlsp like it
# does for every SETTLS pthread -- which already works on both architectures.
# The uktlsp fallback covers a parent with no userland TLS at all.
# ---------------------------------------------------------------------------
CLONEC="$UK/lib/posix-process/clone.c"
if [ -f "$CLONEC" ] && ! grep -q 'child is the parent for a moment' "$CLONEC"; then
	echo "patching $CLONEC (vfork child inherits the parent's userland TLS)"
	python3 - "$CLONEC" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

old = """		/* We will be blocking the parent and pass control to the child
		 * via the scheduler. Therefore we need to set the child's TLS
		 * pointer the Unikraft TLS.
		 */
		child->tlsp = child->uktlsp;
"""
new = """		/* The child must wake up with the *parent's* userland TLS:
		 * under vfork the child is the parent for a moment -- Linux
		 * hands it the parent's registers, thread pointer included --
		 * and everything the libc touches before execve() is
		 * TLS-relative (stack canary, errno, the pthread self
		 * pointer). Starting it on the Unikraft TLS instead faults on
		 * x86_64 before the first syscall: the thread pointer names
		 * the END of the block there (TLS variant 2), so the canary
		 * read at %fs+0x28 lands beyond the allocation.
		 *
		 * The kernel-side TLS is unaffected: uktlsp keeps serving
		 * syscall entry, same as for a SETTLS pthread.
		 */
		child->tlsp = uk_lcpu_sysctx_get(execenv->sysctx, TLSP);
		if (!child->tlsp)
			child->tlsp = child->uktlsp;
"""
assert old in s, "clone.c vfork TLS does not match the expected shape"
open(p, "w").write(s.replace(old, new, 1))
PYEOF
else
	echo "clone.c vfork TLS: already patched or absent, skipping"
fi


# --- Patch 24: signal delivery must not assert on a busy alternate stack ---
#
# sigaltstack(2) is per *thread* in POSIX and in Linux. Unikraft keeps a single
# stack_t in the process signal descriptor (struct uk_signal_pdesc), shared by
# every thread -- so in a threaded program that installs an alternate stack and
# marks any handler SA_ONSTACK, the second thread to take a signal walks into
# one of two assertions and brings the guest down:
#
#   CRIT: [libposix_process] <deliver.c @ 124> Assertion failure: altstack->ss_sp
#   CRIT: [libposix_process] <deliver.c @ 125> Assertion failure:
#                                              !(altstack->ss_flags & 1)
#
# (1 is SS_ONSTACK: "somebody is already running on it".) mongod hits this on
# every abort() -- see examples/unikraft-mongodb -- and because the guest dies
# *inside* signal delivery, the diagnostic the application was in the middle of
# printing is lost, which is the worse half of the bug.
#
# Linux does not treat either case as fatal. If the alternate stack is unusable
# -- not installed, or already in use because the thread is nested inside a
# handler -- it simply runs the handler on the stack the thread is already on
# (kernel/signal.c, sigsp()/on_sig_stack()). Do the same, and only clear
# SS_ONSTACK on the way out if this delivery is what set it.
#
# This is the conservative half of the fix: it makes delivery match Linux for
# the cases that currently abort. The complete fix is to move `altstack` from
# uk_signal_pdesc to uk_signal_tdesc so each thread gets its own, which is a
# larger change to sigaltstack(2), clone() inheritance and thread init.
f=$1/lib/posix-process/signal/deliver.c
if [ -f "$f" ] && grep -q "UK_ASSERT(!(altstack->ss_flags & SS_ONSTACK));" "$f"; then
	python3 - "$f" <<'PYEOF'
import sys
p = sys.argv[1]
s = open(p).read()

old_decl = """	struct posix_process *this_process;
	stack_t *altstack;
	__uptr ulsp;
"""
new_decl = """	struct posix_process *this_process;
	stack_t *altstack;
	int on_altstack = 0;
	__uptr ulsp;
"""

old_head = """	if ((ks->ks_flags & SA_ONSTACK) && !(altstack->ss_flags & SS_DISABLE)) {
		UK_ASSERT(altstack->ss_sp);
		UK_ASSERT(!(altstack->ss_flags & SS_ONSTACK));

		altstack->ss_flags |= SS_ONSTACK;
"""
new_head = """	/* Use the alternate stack only if it is actually usable: installed,
	 * not disabled, and not already occupied. Unikraft shares one
	 * alternate stack across the whole process where POSIX gives each
	 * thread its own, so "already occupied" is reachable in any threaded
	 * program -- and a handler running on a stack another thread is
	 * using would corrupt it. Linux falls back to the interrupted
	 * thread's own stack in exactly these cases (sigsp(), on_sig_stack())
	 * rather than treating them as fatal.
	 */
	if ((ks->ks_flags & SA_ONSTACK) && !(altstack->ss_flags & SS_DISABLE) &&
	    altstack->ss_sp && !(altstack->ss_flags & SS_ONSTACK)) {
		altstack->ss_flags |= SS_ONSTACK;
		on_altstack = 1;
"""

old_tail = """	if (ks->ks_flags & SA_ONSTACK) {
		UK_ASSERT(altstack->ss_flags & SS_ONSTACK);
		UK_ASSERT(!(altstack->ss_flags & SS_DISABLE));
		altstack->ss_flags &= ~SS_ONSTACK;
	}
"""
new_tail = """	/* Release the alternate stack only if this delivery claimed it. */
	if (on_altstack) {
		UK_ASSERT(altstack->ss_flags & SS_ONSTACK);
		altstack->ss_flags &= ~SS_ONSTACK;
	}
"""

for old, new, what in ((old_decl, new_decl, "declaration"),
                       (old_head, new_head, "altstack selection"),
                       (old_tail, new_tail, "altstack release")):
    assert old in s, "deliver.c %s does not match the expected shape" % what
    s = s.replace(old, new, 1)

open(p, "w").write(s)
PYEOF
else
	echo "deliver.c altstack: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 23. brk() logs every successful call at error level.
#
# lib/posix-process/brk.c prints, unconditionally, on the way into every brk():
#
#     uk_pr_err("brk request=%p, base=%p, current=%p\n", ...);
#
# Nothing has gone wrong at that point -- it is the entry trace of a syscall
# musl makes at least twice per process start. At KLVL_ERR (which is where the
# level stays, since the kernel log level is a Kconfig `choice` a Kraftfile
# cannot set) that is two error lines per process on the console, and a
# multiprocess application makes it many more: PostgreSQL's postmaster plus its
# aux processes and a backend per connection.
#
# Demote to debug, where the rest of this file's tracing already lives. Real
# failures in brk() have their own messages and keep them.
# ---------------------------------------------------------------------------
BRK="$UK/lib/posix-process/brk.c"
if [ -f "$BRK" ] && grep -q 'uk_pr_err("brk request=' "$BRK"; then
	echo "patching $BRK (brk() entry trace is debug, not error)"
	sed -i.bak 's|uk_pr_err("brk request=|uk_pr_debug("brk request=|' "$BRK"
	rm -f "$BRK.bak"
	grep -q 'uk_pr_debug("brk request=' "$BRK" || {
		echo "failed to demote the brk trace" >&2
		exit 1
	}
else
	echo "brk() entry trace: already patched or absent, skipping"
fi


# ---------------------------------------------------------------------------
# 25. sigaltstack(SS_DISABLE) is rejected with ENOMEM.
#
# Tearing an alternate signal stack down is normally spelled
#
#     stack_t ss = {};
#     ss.ss_flags = SS_DISABLE;
#     sigaltstack(&ss, nullptr);
#
# -- ss_sp and ss_size left zero, because a teardown does not describe a
# stack. Unikraft validates the size before it looks at the flags:
#
#     if (unlikely(ss->ss_size < MINSIGSTKSZ))
#             return -ENOMEM;
#
# so that call fails with ENOMEM (0 < 6144 on arm64, < 2048 on x86_64).
# Linux checks the size only when the request is *not* a disable
# (kernel/signal.c, do_sigaltstack(): the check sits in the `else` of
# `if (ss_flags & SS_DISABLE)`, which zeroes ss_sp/ss_size instead).
#
# mongod installs an alternate stack on each of its threads and disables it
# again when the thread goes away, and it treats a failing sigaltstack() as
# fatal -- it calls abort() with no diagnostic of its own. The result was a
# server that started, listened, served queries, and then died the moment a
# thread pool recycled a thread:
#
#     sigaltstack(...) = Cannot allocate memory (-12)
#     ... "ctx":"WaitForMajorityServiceThreadPool-0",
#         "msg":"Writing fatal message","attr":{"message":"Got signal: 6"}
#
# Skip the size check for SS_DISABLE, and clear ss_sp/ss_size on the way out
# as Linux does rather than leaving the old ones behind a disabled flag.
# ---------------------------------------------------------------------------
SAS="$UK/lib/posix-process/signal/sigaltstack.c"
if [ -f "$SAS" ] && grep -q 'if (unlikely(ss->ss_size < MINSIGSTKSZ))' "$SAS"; then
	echo "patching $SAS (sigaltstack(SS_DISABLE) must not be size-checked)"
	python3 - "$SAS" <<'PYEOF'
import sys
p = sys.argv[1]
s = open(p).read()

old_size = """		if (unlikely(ss->ss_size < MINSIGSTKSZ))
			return -ENOMEM;
"""
new_size = """		/* A disable request carries no stack, so there is no size to
		 * validate -- callers pass a zeroed stack_t. Linux checks the
		 * size only in the `else` of its SS_DISABLE branch; checking
		 * it first turns every teardown into ENOMEM.
		 */
		if (unlikely((unsigned int)ss->ss_flags != SS_DISABLE &&
			     ss->ss_size < MINSIGSTKSZ))
			return -ENOMEM;
"""

old_dis = """		if ((unsigned int)ss->ss_flags == SS_DISABLE) {
			proc->signal->altstack.ss_flags |= SS_DISABLE;
			return 0;
		}
"""
new_dis = """		if ((unsigned int)ss->ss_flags == SS_DISABLE) {
			/* Linux zeroes the stack description as it disables,
			 * so a later sigaltstack(NULL, &old) reports an empty
			 * disabled stack rather than a stale pointer.
			 */
			proc->signal->altstack.ss_sp = __NULL;
			proc->signal->altstack.ss_size = 0;
			proc->signal->altstack.ss_flags = SS_DISABLE;
			return 0;
		}
"""

for old, new, what in ((old_size, new_size, "size check"),
                       (old_dis, new_dis, "disable branch")):
    assert old in s, "sigaltstack.c %s does not match the expected shape" % what
    s = s.replace(old, new, 1)

open(p, "w").write(s)
PYEOF
else
	echo "sigaltstack.c SS_DISABLE: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# 26. fchmodat() is not implemented, so chmod() always fails on arm64.
#
# arm64's Linux syscall ABI dropped every "legacy" path syscall x86_64 still
# carries -- chmod(2) is one of them (there is no SYS_chmod on arm64 at all).
# glibc and musl paper over that the same way they do for open -> openat,
# mkdir -> mkdirat, unlink -> unlinkat: chmod(path, mode) is implemented as
# fchmodat(AT_FDCWD, path, mode, 0). vfscore already has all three of those
# *at replacements, and its `chmod` UK_SYSCALL_R_DEFINE (lib/vfscore/main.c)
# is itself a fully working implementation -- sys_chmod() -> vn_setmode() ->
# ramfs_setattr() sets the mode bits and returns success -- but on arm64
# nothing ever reaches it, because that entry point is registered under a
# syscall number (SYS_chmod) that does not exist on this architecture. Only
# fchmodat was ever missing; `fchmodat-4` was never declared as a provided
# syscall and no UK_SYSCALL_R_DEFINE for it exists, so every call -- from
# any program, not a specific one -- fell straight through to "no such
# syscall" (ENOSYS).
#
# This was found chasing ../../examples/unikraft-apache: apache2 calls
# chmod() unconditionally on every startup, while writing its pid file
# (ap_log_pid() in server/log.c), and APR turns the ENOSYS into a fatal
# "Failed creating pid file" -- see that example's README for the trace. But
# the gap is generic: any arm64 program calling chmod() hits it identically,
# the same way ../../examples/unikraft-bun hit a missing mremap.
#
# What this does not handle: AT_SYMLINK_NOFOLLOW. Linux's own fchmodat
# rejects that flag too (ENOTSUP; changing a symlink's own mode is not
# supported on Linux at all), so refusing anything but flags == 0 here is not
# a narrower contract than upstream, just an unimplemented corner nothing in
# this repo's examples exercises.
# ---------------------------------------------------------------------------
MAINC="$UK/lib/vfscore/main.c"
if [ -f "$MAINC" ] && ! grep -q ', fchmodat,' "$MAINC"; then
	echo "patching $MAINC (implement fchmodat)"
	python3 - "$MAINC" <<'PYEOF'
import sys

p = sys.argv[1]
s = open(p).read()

anchor = """UK_TRACEPOINT(trace_vfs_chmod, "\\"%s\\" 0%0o", const char*, mode_t);
UK_TRACEPOINT(trace_vfs_chmod_ret, "");
UK_TRACEPOINT(trace_vfs_chmod_err, "%d", int);

UK_SYSCALL_R_DEFINE(int, chmod, const char*, pathname, mode_t, mode)"""
assert anchor in s, "main.c does not contain the expected chmod definition"

impl = """UK_TRACEPOINT(trace_vfs_fchmodat, "%d \\"%s\\" 0%0o %d", int, const char*, mode_t, int);
UK_TRACEPOINT(trace_vfs_fchmodat_ret, "");
UK_TRACEPOINT(trace_vfs_fchmodat_err, "%d", int);

UK_SYSCALL_R_DEFINE(int, fchmodat, int, dirfd, const char*, pathname,
		    mode_t, mode, int, flags)
{
	struct task *t = main_task;
	char path[PATH_MAX];
	int error;

	trace_vfs_fchmodat(dirfd, pathname, mode, flags);

	if (unlikely(flags != 0)) {
		error = -EINVAL;
		goto out_error;
	}

	if ((error = taskat_conv(t, dirfd, pathname, path)) != 0)
		goto out_error;

	error = sys_chmod(path, mode & UK_ALLPERMS);
	if (error)
		goto out_error;

	trace_vfs_fchmodat_ret();
	return 0;

out_error:
	trace_vfs_fchmodat_err(error);
	return error < 0 ? error : -error;
}

"""

open(p, "w").write(s.replace(anchor, impl + anchor, 1))
PYEOF
else
	echo "fchmodat: already patched or absent, skipping"
fi

# Registered separately, same reasoning as mremap above: without the
# declaration the shim generates no table entry regardless of whether the
# handler function is compiled in.
VFSMK="$UK/lib/vfscore/Makefile.uk"
if [ -f "$VFSMK" ] && ! grep -q 'fchmodat-4' "$VFSMK"; then
	echo "patching $VFSMK (declare fchmodat-4)"
	sed -i.bak 's#^UK_PROVIDED_SYSCALLS-$(CONFIG_LIBVFSCORE) += chmod-2$#&\
UK_PROVIDED_SYSCALLS-$(CONFIG_LIBVFSCORE) += fchmodat-4#' "$VFSMK"
	rm -f "$VFSMK.bak"
	grep -q 'fchmodat-4' "$VFSMK" || { echo "failed to register fchmodat" >&2; exit 1; }
else
	echo "fchmodat syscall registration: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# CLOCK_PROCESS_CPUTIME_ID
#
# posix-time knows CLOCK_THREAD_CPUTIME_ID but not the process one, so
# clock_gettime(CLOCK_PROCESS_CPUTIME_ID) returns EINVAL. GHC's runtime asks
# for it during startup and dies with "clock_gettime: Invalid argument"
# before main() runs, which is what a packed Haskell binary hits.
#
# A unikernel is one process, so process CPU time is the guest's CPU time.
# The thread's exec_time is the same approximation the thread clock already
# makes, and the callers of this (runtime statistics, mostly) want a
# monotonic CPU-ish counter rather than an exact one.
# ---------------------------------------------------------------------------
TIME="$UK/lib/posix-time/time.c"
if [ -f "$TIME" ] && ! grep -q 'CLOCK_PROCESS_CPUTIME_ID' "$TIME"; then
	echo "patching $TIME (support CLOCK_PROCESS_CPUTIME_ID)"
	sed -i.bak 's|^\tcase CLOCK_THREAD_CPUTIME_ID:$|\tcase CLOCK_PROCESS_CPUTIME_ID:\n&|' "$TIME"
	rm -f "$TIME.bak"
	grep -q 'CLOCK_PROCESS_CPUTIME_ID' "$TIME" || { echo "failed to add CLOCK_PROCESS_CPUTIME_ID" >&2; exit 1; }
else
	echo "CLOCK_PROCESS_CPUTIME_ID: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# POSIX timers
#
# lib/posix-time/timer.c stubs timer_create and friends with -ENOTSUP. A
# program that creates a timer during startup and treats failure as fatal
# therefore cannot run at all: PHP built with --enable-zend-max-execution-timers
# dies with "Could not create timer: Not supported (95)" before serving,
# which is what a FrankenPHP guest hits.
#
# The replacement is a table of timers scanned by a single polling thread.
# See the file's own comment for why that shape, and for what it
# approximates (CPU-time clocks are measured against the monotonic one).
# ---------------------------------------------------------------------------
PTIMER="$UK/lib/posix-time/timer.c"
if [ -f "$PTIMER" ] && ! grep -q 'bsdkrun: POSIX timers' "$PTIMER"; then
	echo "patching $PTIMER (implement POSIX timers)"
	cat > "$PTIMER" <<'BSDKRUN_TIMER_EOF'
/* SPDX-License-Identifier: BSD-3-Clause */
/* bsdkrun: POSIX timers (timer_create and friends).
 *
 * Upstream stubs all five of these with -ENOTSUP, which is fine until
 * something creates a timer during startup and treats failure as fatal.
 * PHP does exactly that: built with --enable-zend-max-execution-timers (as
 * static-php-cli configures it for ZTS, with no way to turn it off from the
 * outside) it arms a per-process timer to enforce max_execution_time, and
 * dies with "Could not create timer: Not supported (95)" before serving a
 * request. That is what a FrankenPHP guest hits.
 *
 * The implementation is a table of timers scanned by one polling thread,
 * rather than a per-timer platform timer:
 *
 *   - It is one thread for all timers, not one per timer. A server creates
 *     a timer per worker, and a thread apiece would cost more than the
 *     timers do.
 *   - The resolution is the scan interval, which is coarse. That suits what
 *     these are actually used for here — execution timeouts measured in
 *     seconds — and no caller in this position needs millisecond accuracy.
 *   - The thread sleeps longer when nothing is armed, so an idle guest is
 *     not woken ten times a second for timers nobody set.
 *
 * CLOCK_THREAD_CPUTIME_ID and CLOCK_PROCESS_CPUTIME_ID timers are measured
 * against the monotonic clock. For a thread that is busy — which is when an
 * execution timeout matters — the two run together; for one that blocks,
 * this expires earlier than a true CPU-time timer would.
 */

#include <errno.h>
#include <signal.h>
#include <time.h>

#include <uk/arch/time.h>
#include <uk/arch/types.h>
#include <uk/errptr.h>
#include <uk/essentials.h>
#include <uk/plat/time.h>
#include <uk/print.h>
#include <uk/sched.h>
#include <uk/syscall.h>
#include <uk/thread.h>

#define UK_PTIMER_MAX		16
/* Scan interval while something is armed, and while nothing is. */
#define UK_PTIMER_TICK_NSEC	(10ULL * 1000000ULL)
#define UK_PTIMER_IDLE_NSEC	(200ULL * 1000000ULL)

struct uk_ptimer {
	/* Written by the caller's thread, read by the timer thread. */
	volatile int used;
	volatile int armed;
	volatile int overrun;
	clockid_t clockid;
	int signo;
	__nsec next;
	__nsec interval;
};

static struct uk_ptimer uk_ptimers[UK_PTIMER_MAX];
static struct uk_thread *uk_ptimer_thread;

static inline __nsec uk_ptimer_ts2nsec(const struct timespec *ts)
{
	return ukarch_time_sec_to_nsec((__nsec)ts->tv_sec) + (__nsec)ts->tv_nsec;
}

static inline void uk_ptimer_nsec2ts(__nsec n, struct timespec *ts)
{
	ts->tv_sec = ukarch_time_nsec_to_sec(n);
	ts->tv_nsec = ukarch_time_subsec(n);
}

/* Timer ids are the table index plus one, so that a valid id is never the
 * NULL that timer_t (a pointer type) would otherwise collide with.
 */
static struct uk_ptimer *uk_ptimer_get(timer_t timerid)
{
	__uptr idx = (__uptr)timerid;

	if (unlikely(!idx || idx > UK_PTIMER_MAX))
		return __NULL;
	if (unlikely(!uk_ptimers[idx - 1].used))
		return __NULL;

	return &uk_ptimers[idx - 1];
}

static void uk_ptimer_expire(struct uk_ptimer *t, __nsec now)
{
	if (t->signo)
#if CONFIG_LIBPOSIX_PROCESS_SIGNAL
		uk_syscall_r_kill(uk_syscall_r_getpid(), t->signo);
#else
		uk_pr_warn_once("timer expired but signals are not configured in\n");
#endif

	if (!t->interval) {
		t->armed = 0;
		return;
	}

	/* A timer whose interval elapsed more than once while we were not
	 * looking has overrun; POSIX wants that counted, not replayed.
	 */
	do {
		t->next += t->interval;
		if (now >= t->next)
			t->overrun++;
	} while (now >= t->next);
}

static void uk_ptimer_monitor(void *arg __unused)
{
	for (;;) {
		__nsec now;
		int i, armed = 0;

		now = ukplat_monotonic_clock();

		for (i = 0; i < UK_PTIMER_MAX; i++) {
			struct uk_ptimer *t = &uk_ptimers[i];

			if (!t->used || !t->armed)
				continue;

			armed = 1;
			if (now >= t->next)
				uk_ptimer_expire(t, now);
		}

		uk_sched_thread_sleep(armed ? UK_PTIMER_TICK_NSEC
					    : UK_PTIMER_IDLE_NSEC);
	}
}

UK_SYSCALL_R_DEFINE(int, timer_create, clockid_t, clockid,
		    struct sigevent *__restrict, sevp,
		    timer_t *__restrict, timerid)
{
	struct uk_ptimer *t = __NULL;
	int i, idx = -1, signo = SIGALRM;

	if (unlikely(!timerid))
		return -EFAULT;

	switch (clockid) {
	case CLOCK_REALTIME:
	case CLOCK_REALTIME_COARSE:
	case CLOCK_MONOTONIC:
	case CLOCK_MONOTONIC_RAW:
	case CLOCK_MONOTONIC_COARSE:
	case CLOCK_BOOTTIME:
	case CLOCK_PROCESS_CPUTIME_ID:
	case CLOCK_THREAD_CPUTIME_ID:
		break;
	default:
		return -EINVAL;
	}

	if (sevp) {
		switch (sevp->sigev_notify) {
		case SIGEV_NONE:
			signo = 0;
			break;
		case SIGEV_SIGNAL:
		case SIGEV_THREAD_ID:
			signo = sevp->sigev_signo;
			break;
		default:
			/* SIGEV_THREAD delivers by starting a userspace
			 * thread per expiry, which is the C library's job
			 * and not something this can do from here.
			 */
			uk_pr_warn("timer_create: SIGEV_THREAD is not supported\n");
			return -ENOTSUP;
		}
	}

	for (i = 0; i < UK_PTIMER_MAX; i++) {
		if (!uk_ptimers[i].used) {
			idx = i;
			t = &uk_ptimers[i];
			break;
		}
	}
	if (unlikely(!t))
		return -EAGAIN;

	t->armed = 0;
	t->overrun = 0;
	t->clockid = clockid;
	t->signo = signo;
	t->next = 0;
	t->interval = 0;
	t->used = 1;

	if (!uk_ptimer_thread) {
		uk_ptimer_thread = uk_sched_thread_create(uk_sched_current(),
							  uk_ptimer_monitor,
							  __NULL, "posix-timer");
		if (unlikely(PTRISERR(uk_ptimer_thread))) {
			uk_pr_err("timer_create: could not start the timer thread\n");
			uk_ptimer_thread = __NULL;
			t->used = 0;
			return -EAGAIN;
		}
	}

	*timerid = (timer_t)(__uptr)(idx + 1);
	return 0;
}

UK_SYSCALL_R_DEFINE(int, timer_delete,
		    timer_t, timerid)
{
	struct uk_ptimer *t = uk_ptimer_get(timerid);

	if (unlikely(!t))
		return -EINVAL;

	t->armed = 0;
	t->used = 0;
	return 0;
}

UK_SYSCALL_R_DEFINE(int, timer_settime,
		    timer_t, timerid,
		    int, flags,
		    const struct itimerspec *__restrict, new_value,
		    struct itimerspec *__restrict, old_value)
{
	struct uk_ptimer *t = uk_ptimer_get(timerid);
	__nsec now, value;

	if (unlikely(!t))
		return -EINVAL;
	if (unlikely(!new_value))
		return -EFAULT;

	now = ukplat_monotonic_clock();

	if (old_value) {
		uk_ptimer_nsec2ts((t->armed && t->next > now) ? t->next - now : 0,
				  &old_value->it_value);
		uk_ptimer_nsec2ts(t->interval, &old_value->it_interval);
	}

	value = uk_ptimer_ts2nsec(&new_value->it_value);
	t->interval = uk_ptimer_ts2nsec(&new_value->it_interval);

	/* A zero it_value disarms, which is how a caller turns a timeout
	 * off — PHP does this whenever max_execution_time is 0.
	 */
	if (!value) {
		t->armed = 0;
		return 0;
	}

	t->next = (flags & TIMER_ABSTIME) ? value : now + value;
	t->overrun = 0;
	t->armed = 1;
	return 0;
}

UK_SYSCALL_R_DEFINE(int, timer_gettime,
		    timer_t, timerid,
		    struct itimerspec *, curr_value)
{
	struct uk_ptimer *t = uk_ptimer_get(timerid);
	__nsec now;

	if (unlikely(!t))
		return -EINVAL;
	if (unlikely(!curr_value))
		return -EFAULT;

	now = ukplat_monotonic_clock();

	/* A disarmed timer reports zero, which is how a caller tells the
	 * difference between "not set" and "about to expire".
	 */
	uk_ptimer_nsec2ts((t->armed && t->next > now) ? t->next - now : 0,
			  &curr_value->it_value);
	uk_ptimer_nsec2ts(t->interval, &curr_value->it_interval);
	return 0;
}

UK_SYSCALL_R_DEFINE(int, timer_getoverrun,
		    timer_t, timerid)
{
	struct uk_ptimer *t = uk_ptimer_get(timerid);

	if (unlikely(!t))
		return -EINVAL;

	return t->overrun;
}
BSDKRUN_TIMER_EOF
	grep -q 'bsdkrun: POSIX timers' "$PTIMER" || { echo "failed to implement POSIX timers" >&2; exit 1; }
else
	echo "POSIX timers: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# getsid/getpgid/setpgid for the caller's own pid
#
# These answer -ESRCH for any pid that is not 0, including the caller's own
# — so getsid(getpid()) fails even though that process is the only one there
# is, and is the caller. Nothing about "no such process" is true there.
#
# .NET's PAL does exactly that pair during startup (getpid, then getsid on
# the result) and faults on the unexpected failure; the guest dies with a
# data abort right after the failing call.
#
# 0 already means "me". This adds the caller's real pid as a second spelling
# of the same thing, which is what Linux does.
# ---------------------------------------------------------------------------
DEPR="$UK/lib/posix-process/deprecated.c"
if [ -f "$DEPR" ] && ! grep -q 'uk_sys_getpid()' "$DEPR"; then
	echo "patching $DEPR (getsid/getpgid/setpgid accept the caller's own pid)"
	sed -i.bak 's|^	if (pid != 0) {$|	if (pid != 0 \&\& pid != uk_sys_getpid()) {|' "$DEPR"
	rm -f "$DEPR.bak"
	grep -q 'uk_sys_getpid()' "$DEPR" || { echo "failed to relax the pid checks" >&2; exit 1; }
else
	echo "getsid/getpgid/setpgid pid checks: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# Bound the tracer's %s
#
# uk_prsyscall.c prints a char* syscall argument with %s, which walks guest
# memory until it finds a NUL. The argument is whatever the application
# passed, so that walk can run off the end of what is mapped and fault --
# inside the tracer, while formatting the line meant to explain the
# application. The guest dies with a data abort in vsnprintf/memchr that
# reads as the application crashing at a fixed point.
#
# .NET traces ended this way every time, at the same line, which is what
# made it look like an application failure rather than a tracer one.
#
# Only built into --strace images, so this cannot affect an ordinary build.
# ---------------------------------------------------------------------------
PRSC="$UK/lib/syscall_shim/uk_prsyscall.c"
if [ -f "$PRSC" ] && ! grep -q 'prsyscall_charp' "$PRSC"; then
	echo "patching $PRSC (bound the tracer's string printing)"
	python3 - "$PRSC" <<'BSDKRUN_PRSC_EOF'
import sys

path = sys.argv[1]
src = open(path).read()

helper = r"""#include <uk/streambuf.h>

#if CONFIG_LIBUKVMEM
#include <uk/vmem.h>
#endif /* CONFIG_LIBUKVMEM */

/* bsdkrun: how much of a string argument to print.
 *
 * The %s below walks guest memory until it finds a NUL. A syscall argument
 * is whatever the application passed, so that walk can run off the end of
 * what is mapped and fault -- inside the tracer, while formatting the line
 * that was meant to explain the application. The guest then dies with a
 * data abort in vsnprintf/memchr that looks like the application crashing
 * at a fixed point, which is a memorably unhelpful way to be misled.
 */
#define PRSYSCALL_STRMAX 128

static void prsyscall_charp(struct uk_streambuf *sb, const char *s)
{
	__sz max = PRSYSCALL_STRMAX;
#if CONFIG_LIBUKVMEM
	const struct uk_vma *vma;

	/* An address in no VMA cannot be read at all: print the pointer,
	 * which is more than a crash would have told anyone.
	 */
	vma = uk_vma_find(uk_vas_get_active(), (__vaddr_t) s);
	if (unlikely(!vma)) {
		uk_streambuf_printf(sb, "0x%lx", (unsigned long) s);
		return;
	}

	/* Stop at the end of the mapping the string starts in, so an
	 * unterminated string cannot walk into the next one.
	 */
	if ((__sz)(vma->end - (__vaddr_t) s) < max)
		max = (__sz)(vma->end - (__vaddr_t) s);
#endif /* CONFIG_LIBUKVMEM */

	/* %.*s still stops at a NUL; the precision only bounds how far it
	 * will look for one.
	 */
	uk_streambuf_printf(sb, "\"%.*s\"", (int) max, s);
}"""

anchor = "#include <uk/streambuf.h>"
assert src.count(anchor) == 1, "include anchor moved"
src = src.replace(anchor, helper, 1)

call = '			uk_streambuf_printf(sb, "\\"%s\\"", (const char *) param);'
assert src.count(call) == 1, "PT_CHARP print site moved"
src = src.replace(call, '			prsyscall_charp(sb, (const char *) param);', 1)

open(path, "w").write(src)
BSDKRUN_PRSC_EOF
	grep -q 'prsyscall_charp' "$PRSC" || { echo "failed to bound the tracer's %s" >&2; exit 1; }
else
	echo "tracer string bounding: already patched or absent, skipping"
fi

# ---------------------------------------------------------------------------
# RLIMIT_STACK reports the kernel thread's stack, not the application's
#
# getrlimit(RLIMIT_STACK) answers __STACK_SIZE — the size of a Unikraft
# *kernel* thread stack (STACK_SIZE_PAGE_ORDER=4: 64 KiB) — while the
# application actually runs on the elfloader-provided stack, which is
# CONFIG_APPELFLOADER_STACK_NBPAGES (512 KiB by default) and can be far
# deeper than 64 KiB at any given moment.
#
# glibc's pthread_getattr_np() clamps the main thread's computed stack size
# to this rlimit, so any runtime that asks glibc for its own stack bounds
# gets an answer that excludes the stack pointer it is currently running
# on. CoreCLR does exactly that during PAL startup and fails — reported,
# unhelpfully, as E_OUTOFMEMORY.
#
# Report the Linux default (8 MiB) instead. An rlimit is a limit, not a
# measurement; claiming 8 MiB over a smaller mapping is what Linux itself
# does, and glibc bounds the final answer by the mapping either way.
# ---------------------------------------------------------------------------
DEPR="$UK/lib/posix-process/deprecated.c"
if [ -f "$DEPR" ] && ! grep -q 'bsdkrun app stack rlimit' "$DEPR"; then
	echo "patching $DEPR (RLIMIT_STACK reports an application-sized limit)"
	sed -i.bak 's|old_limit->rlim_cur = __STACK_SIZE;|/* bsdkrun app stack rlimit: see patches/apply.sh */\n		old_limit->rlim_cur = 0x800000;|; s|old_limit->rlim_max = __STACK_SIZE;|old_limit->rlim_max = 0x800000;|' "$DEPR"
	rm -f "$DEPR.bak"
	grep -q 'bsdkrun app stack rlimit' "$DEPR" || { echo "failed to patch RLIMIT_STACK" >&2; exit 1; }
else
	echo "RLIMIT_STACK: already patched or absent, skipping"
fi

echo "patches applied."

