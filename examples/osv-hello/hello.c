/*
 * A plain C program, built as a shared object for OSv.
 *
 * There is nothing OSv-specific in here, which is the point: OSv runs an
 * ordinary Linux shared library and calls its main(). The only thing that
 * matters is how it is *linked* — see the Makefile.
 */

#include <stdio.h>
#include <sys/utsname.h>

int main(int argc, char **argv)
{
    struct utsname u;

    printf("Hello from OSv on libkrun!\n");

    if (uname(&u) == 0) {
        /* Proves we really are inside the unikernel and not on the host. */
        printf("  running on %s %s (%s)\n", u.sysname, u.release, u.machine);
    }

    for (int i = 1; i < argc; i++) {
        printf("  argv[%d] = %s\n", i, argv[i]);
    }

    return 0;
}
