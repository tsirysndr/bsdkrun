//! Terminal state save/restore plus the unified signal cleanup.
//!
//! While the guest's serial console runs, libkrun puts the controlling TTY into
//! raw mode (no echo, byte-at-a-time input) and never restores it — so after the
//! guest exits, or the VM is interrupted, the user's shell is left broken until
//! they blindly type `reset`. We snapshot stdin's termios before booting and
//! restore it on *every* exit path: clean shutdown, an error, or a signal.
//!
//! The signal handler additionally reaps gvproxy (see [`crate::net`]), because
//! `krun_start_enter` blocks the main thread and a Ctrl-C / SIGTERM would
//! otherwise skip every destructor.

use std::mem::MaybeUninit;
use std::ptr::{addr_of, addr_of_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// Whether [`SAVED_TERMIOS`] holds a valid snapshot to restore.
static TERMIOS_SAVED: AtomicBool = AtomicBool::new(false);

/// stdin's terminal settings, captured before libkrun switched it to raw mode.
/// Written once (single-threaded) in [`save_stdin_termios`] before the guest
/// runs, then only ever read — including from the async-signal-safe handler.
static mut SAVED_TERMIOS: MaybeUninit<libc::termios> = MaybeUninit::uninit();

/// Snapshot stdin's terminal settings (if it is a TTY) and install signal
/// handlers that restore them — and tear down gvproxy — on interrupt. Call once,
/// before booting, on any path that hands the console to the guest.
pub fn install() {
    save_stdin_termios();
    install_signal_handlers();
}

fn save_stdin_termios() {
    // STDIN_FILENO == 0. Only meaningful when stdin is a terminal; a redirected
    // or piped stdin was never raw, so there is nothing to restore.
    if unsafe { libc::isatty(0) } != 1 {
        return;
    }
    let mut t = MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(0, t.as_mut_ptr()) } == 0 {
        unsafe { addr_of_mut!(SAVED_TERMIOS).write(t) };
        TERMIOS_SAVED.store(true, Ordering::SeqCst);
    }
}

/// Restore the saved terminal settings. Idempotent, and async-signal-safe
/// (`tcsetattr` is on the POSIX signal-safe list), so it is fine to call from a
/// signal handler, on a normal exit, and on an error path alike.
pub fn restore_stdin_termios() {
    if TERMIOS_SAVED.load(Ordering::SeqCst) {
        // `addr_of!` avoids forming a reference to the `static mut`.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, addr_of!(SAVED_TERMIOS).cast()) };
    }
}

/// Handler for interrupting signals: put the terminal back, kill gvproxy, exit.
/// Only async-signal-safe operations are used.
extern "C" fn handle_signal(sig: i32) {
    restore_stdin_termios();
    kill_tracked_child();
    crate::net::kill_tracked_gvproxy();
    // Conventional 128+signum status. `_exit` skips (non-signal-safe) atexit
    // hooks; the OS reclaims fds and the socket dir is swept on the next launch.
    unsafe { libc::_exit(128 + sig) };
}

/// PID of a child process that *is* the running guest, or `-1` for none.
///
/// Every libkrun guest runs inside this process, so killing bsdkrun kills the
/// VM. The Solo5 tender does not: it is a separate process bsdkrun spawns and
/// waits on ([`crate::commands::solo5`]). Without this, a `stop` or a Ctrl-C
/// would reap bsdkrun and leave the tender running the guest, holding its
/// ports, with nothing left that knows its id.
static CHILD_PID: AtomicI32 = AtomicI32::new(-1);

/// Track `pid` as the guest process to kill on an interrupting signal.
pub fn track_child(pid: i32) {
    CHILD_PID.store(pid, Ordering::SeqCst);
}

/// Stop tracking a child that has already exited, so its (possibly reused) pid
/// is never signalled.
pub fn untrack_child() {
    CHILD_PID.store(-1, Ordering::SeqCst);
}

/// Kill the tracked guest process, if any. Async-signal-safe.
fn kill_tracked_child() {
    let pid = CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

fn install_signal_handlers() {
    let handler = handle_signal as extern "C" fn(i32) as libc::sighandler_t;
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(sig, handler) };
    }
}
