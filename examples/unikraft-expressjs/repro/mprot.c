/* Permission faults on *present* pages -- what V8 does constantly for W^X,
 * and the one arm64 fault class the ukvmem decoder folds into MISCONFIG.
 *
 * Each step touches a page that is already mapped and present, then changes
 * its protection and touches it again. On Linux every step is silent. */
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

#define SZ 8192

int main(void)
{
	volatile unsigned char *p = mmap(0, SZ, PROT_READ | PROT_WRITE,
					 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (p == MAP_FAILED) { perror("mmap"); return 1; }
	puts("mmap ok");

	p[0] = 1; p[4096] = 2;              /* fault both pages in (present now) */
	puts("write ok");

	if (mprotect((void *)p, SZ, PROT_READ)) { perror("mprotect ro"); return 1; }
	puts("mprotect ro ok");

	/* Read of a present, read-only page: no fault expected. */
	volatile unsigned char r = p[0];
	printf("read-after-ro ok (%u)\n", r);

	/* Back to writable, then write: on arm64 this is a permission fault on
	 * a page that is already present -- FSC 0x0c..0x0f, not a translation
	 * fault. */
	if (mprotect((void *)p, SZ, PROT_READ | PROT_WRITE)) { perror("mprotect rw"); return 1; }
	puts("mprotect rw ok");

	p[0] = 3; p[4096] = 4;
	puts("write-after-rw ok");

	/* And the W^X flip node's JIT does. */
	if (mprotect((void *)p, SZ, PROT_READ | PROT_EXEC)) { perror("mprotect rx"); return 1; }
	r = p[0];
	printf("read-after-rx ok (%u)\n", r);

	puts("mprot ok");
	return 0;
}
