/* Reproduce musl's map_library() mmap sequence with no dynamic linker in the
 * picture: map the whole span of a shared object read+execute from the file,
 * then replace the tail with a MAP_FIXED read/write mapping at a different
 * file offset -- which forces Unikraft to split the file VMA and change page
 * attributes, the arm64 path that only exists in patched form. */
#include <stdio.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>

int main(void)
{
	int fd = open("/usr/lib/lib64k.so", O_RDONLY);
	if (fd < 0) { perror("open"); return 1; }

	size_t span = 0x21000;
	unsigned char *m = mmap(0, span, PROT_READ | PROT_EXEC, MAP_PRIVATE, fd, 0);
	if (m == MAP_FAILED) { perror("mmap span"); return 1; }
	printf("span  at %p\n", (void *)m);

	unsigned char *d = mmap(m + 0x1f000, 0x2000, PROT_READ | PROT_WRITE,
				MAP_PRIVATE | MAP_FIXED, fd, 0xf000);
	if (d == MAP_FAILED) { perror("mmap fixed"); return 1; }
	printf("fixed at %p\n", (void *)d);

	volatile unsigned a = *(volatile unsigned *)m;         /* RX page  */
	volatile unsigned b = *(volatile unsigned *)d;         /* RW page  */
	*(volatile unsigned *)d = 0xdeadbeef;                  /* write it */
	volatile unsigned c = *(volatile unsigned *)(m + 0x1000);
	printf("read %08x %08x %08x\n", a, b, c);

	close(fd);
	puts("mmaptest ok");
	return 0;
}
