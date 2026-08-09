/* Exercise mremap() directly.
 *
 * "Bun starts now" is evidence that the stack probe terminates, not that the
 * syscall is right. These cases check the behaviour Unikraft's implementation
 * actually claims: shrink, grow in place, refuse to grow into an occupied
 * range, and reject a source that is not mapped -- plus the documented
 * limitation that it never relocates.
 *
 * Every expectation here matches Linux except MREMAP_MAYMOVE-when-blocked,
 * which is called out below. Build it for the host and run it on Linux to
 * confirm that.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define PAGE 4096

static int fails;

static void ok(int cond, const char *what)
{
	printf("%-46s %s\n", what, cond ? "ok" : "FAIL");
	if (!cond)
		fails++;
}

/* A region with something mapped immediately above it, so it cannot grow. */
static char *blocked_region(size_t len)
{
	char *p = mmap(NULL, len + PAGE, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (p == MAP_FAILED)
		return NULL;
	/* Split it: [p, p+len) stays, [p+len, p+len+PAGE) is a separate
	 * mapping with different protection so the two cannot merge.
	 */
	if (mprotect(p + len, PAGE, PROT_READ) != 0)
		return NULL;
	return p;
}

int main(void)
{
	char *p, *q;

	/* --- shrink ------------------------------------------------------ */
	p = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE,
		 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	ok(p != MAP_FAILED, "mmap 4 pages");
	memset(p, 0xA5, 4 * PAGE);

	q = mremap(p, 4 * PAGE, 2 * PAGE, 0);
	ok(q == p, "shrink 4->2 pages keeps the address");
	ok(q != MAP_FAILED && q[0] == (char)0xA5 && q[2 * PAGE - 1] == (char)0xA5,
	   "shrink preserves the surviving contents");

	/* --- grow in place ----------------------------------------------- */
	q = mremap(p, 2 * PAGE, 4 * PAGE, 0);
	ok(q == p, "grow 2->4 pages in place keeps the address");
	if (q != MAP_FAILED) {
		q[3 * PAGE] = 0x5A;  /* the newly added page must be usable */
		ok(q[3 * PAGE] == 0x5A, "grown pages are writable");
		ok(q[0] == (char)0xA5, "grow preserves the original contents");
	}
	munmap(p, 4 * PAGE);

	/* --- same size is a no-op ---------------------------------------- */
	p = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE,
		 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	q = mremap(p, 2 * PAGE, 2 * PAGE, 0);
	ok(q == p, "same-size mremap returns the same address");
	munmap(p, 2 * PAGE);

	/* --- blocked growth ----------------------------------------------- */
	p = blocked_region(2 * PAGE);
	ok(p != NULL, "set up a region with a neighbour above it");
	if (p) {
		errno = 0;
		q = mremap(p, 2 * PAGE, 4 * PAGE, 0);
		ok(q == MAP_FAILED && errno == ENOMEM,
		   "blocked grow without MAYMOVE gives ENOMEM");

		/* Linux would relocate here. Unikraft's implementation never
		 * moves a mapping and reports ENOMEM instead, which callers
		 * must already handle. This assertion therefore encodes the
		 * documented limitation, not Linux's behaviour.
		 */
		errno = 0;
		q = mremap(p, 2 * PAGE, 4 * PAGE, MREMAP_MAYMOVE);
		ok(q == MAP_FAILED && errno == ENOMEM,
		   "blocked grow with MAYMOVE gives ENOMEM (no move)");

		munmap(p, 3 * PAGE);
	}

	/* --- bad arguments ------------------------------------------------ */
	errno = 0;
	q = mremap((void *)(uintptr_t)(64UL << 40), PAGE, 2 * PAGE, 0);
	ok(q == MAP_FAILED && errno == EFAULT, "unmapped source gives EFAULT");

	p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE,
		 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	errno = 0;
	q = mremap(p, PAGE, 0, 0);
	ok(q == MAP_FAILED && errno == EINVAL, "new_size of 0 gives EINVAL");

	errno = 0;
	q = mremap((char *)p + 1, PAGE, 2 * PAGE, 0);
	ok(q == MAP_FAILED && errno == EINVAL, "misaligned source gives EINVAL");

	errno = 0;
	q = mremap(p, PAGE, 2 * PAGE, MREMAP_FIXED);
	ok(q == MAP_FAILED && errno == EINVAL, "MREMAP_FIXED without MAYMOVE is EINVAL");
	munmap(p, PAGE);

	/* --- the case this was all for ------------------------------------ */
	{
		pthread_attr_t a;
		void *stackaddr;
		size_t stacksize = 0;

		if (pthread_getattr_np(pthread_self(), &a) == 0 &&
		    pthread_attr_getstack(&a, &stackaddr, &stacksize) == 0) {
			printf("%-46s %zu bytes\n", "main thread stack size", stacksize);
			ok(stacksize > 64 * 1024,
			   "musl sizes the main stack plausibly (>64K)");
		} else {
			ok(0, "pthread_getattr_np works");
		}
	}

	printf("\n%s (%d failure%s)\n", fails ? "FAILED" : "mremaptest ok",
	       fails, fails == 1 ? "" : "s");
	return fails != 0;
}
