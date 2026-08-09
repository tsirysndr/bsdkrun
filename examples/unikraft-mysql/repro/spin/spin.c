/* InnoDB's IB_thread::start() deadlock, in twenty lines.
 *
 * Both halves busy-wait with no yield and no system call, which is what makes
 * this fatal on a cooperative scheduler: whichever thread runs first spins
 * forever on a state word only the other one can change.
 *
 *   parent: publish ALLOWED_TO_START, then spin while the state is still that
 *   child:  spin while the state is still NOT_STARTED, then publish STARTED
 *
 * Without preemption the child is created, queued, and never scheduled: the
 * parent holds the CPU forever. With it, this prints and exits.
 */
#include <pthread.h>
#include <stdio.h>

enum { NOT_STARTED = 1, ALLOWED_TO_START = 2, STARTED = 3 };

static volatile int state = NOT_STARTED;

static void *child(void *arg __attribute__((unused)))
{
	while (state == NOT_STARTED)
		__asm__ __volatile__("" ::: "memory");
	state = STARTED;
	return NULL;
}

int main(void)
{
	pthread_t t;

	printf("spin: creating child\n");
	fflush(stdout);
	if (pthread_create(&t, NULL, child, NULL) != 0) {
		printf("spin: pthread_create failed\n");
		return 1;
	}

	state = ALLOWED_TO_START;
	while (state == ALLOWED_TO_START)
		__asm__ __volatile__("" ::: "memory");

	printf("spin: PREEMPTION WORKS (child reached STARTED)\n");
	fflush(stdout);
	return 0;
}
