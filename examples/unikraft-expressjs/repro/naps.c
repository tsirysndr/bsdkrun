/* Loads no extra library, but sleeps, so timer interrupts must be delivered
 * and returned from many times. */
#include <stdio.h>
#include <time.h>
int main(void) {
	for (int i = 0; i < 20; i++) {
		struct timespec t = { 0, 20000000 };
		nanosleep(&t, 0);
		printf("nap %d\n", i);
	}
	puts("naps ok");
	return 0;
}
