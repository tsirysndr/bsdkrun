/* Loads no extra library, but burns ~400 ms so it outlives the ~15 ms mark
 * where the two-DSO cases die. */
#include <stdio.h>
#include <time.h>
int main(void) {
	volatile unsigned long x = 0;
	struct timespec a, b;
	clock_gettime(CLOCK_MONOTONIC, &a);
	do {
		for (int i = 0; i < 100000; i++) x += i;
		clock_gettime(CLOCK_MONOTONIC, &b);
	} while ((b.tv_sec - a.tv_sec) * 1000000000L + (b.tv_nsec - a.tv_nsec) < 400000000L);
	printf("spin ok %lu\n", x);
	return 0;
}
