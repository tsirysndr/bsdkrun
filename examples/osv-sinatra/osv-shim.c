/*
 * Two symbols OSv's libc does not provide, which Ruby's dependencies import.
 *
 * This is shipped as a small shared library and added to the importers'
 * DT_NEEDED with patchelf, so OSv's loader resolves them here. Both are
 * genuinely unused on this path — see the comments — so stubbing them changes
 * no behaviour that Ruby relies on.
 */
#include <errno.h>
#include <stdarg.h>

/*
 * libgmp imports this for its obstack printf helpers (gmp_obstack_printf and
 * friends). Ruby uses libgmp only for Bignum arithmetic and never calls them.
 */
struct obstack;
int obstack_vprintf(struct obstack *obstack, const char *format, va_list args)
{
    (void)obstack; (void)format; (void)args;
    errno = ENOTSUP;
    return -1;
}

/*
 * POSIX per-process timers, which OSv does not implement. Ruby uses one to
 * drive its UBF ("unblocking function") timer and falls back to a timer thread
 * when creation fails, printing "timer_create failed: Not supported, signals
 * racy". Reporting failure is therefore the honest answer, and the fallback is
 * the path Ruby takes on any platform lacking them.
 */
int timer_create(int clockid, void *sevp, void *timerid)
{
    (void)clockid; (void)sevp; (void)timerid;
    errno = ENOTSUP;
    return -1;
}

int timer_settime(void *timerid, int flags, const void *new_value, void *old_value)
{
    (void)timerid; (void)flags; (void)new_value; (void)old_value;
    errno = ENOTSUP;
    return -1;
}

int timer_delete(void *timerid)
{
    (void)timerid;
    errno = ENOTSUP;
    return -1;
}
