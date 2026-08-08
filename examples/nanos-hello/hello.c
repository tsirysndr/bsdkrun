#include <stdio.h>
#include <unistd.h>

int main() {
    printf("Hello from Nanos on bsdkrun!\n");
    fflush(stdout);

    /* Park here instead of returning. A unikernel that leaves main powers the
     * VM off within milliseconds, which makes the message very easy to miss:
     * the machine is gone before you can look at it, and on a fresh boot the
     * firmware's screen repaint lands on top of it. Staying alive keeps the
     * console attachable — `bsdkrun logs <id>` / `bsdkrun shell <id>` — so the
     * output can be read at leisure. Stop it with `bsdkrun stop <id>`. */
    for (;;)
        sleep(3600);
}
