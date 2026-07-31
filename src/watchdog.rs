//! Turns a known libkrun HVF panic-hang into a clean exit.
//!
//! On aarch64/macOS this libkrun mishandles the PSCI `CPU_OFF` / `AFFINITY_INFO`
//! calls a guest issues while powering down an **SMP** system: its vCPU threads
//! panic (`src/hvf/src/lib.rs:549: Unexpected val=…`) and die, but the main
//! thread stays blocked forever inside `krun_start_enter`. The guest has fully
//! halted (disks synced + unmounted) — bsdkrun just never returns, so it hangs.
//!
//! Booting with `--cpus 1` sidesteps the bug entirely (no secondary CPU to
//! offline). For the SMP case we can't make `krun_start_enter` return, and the
//! panic happens in libkrun's *own* Rust runtime (a separate cdylib), so a
//! `std::panic` hook of ours can't see it. The one deterministic signal is the
//! panic message libkrun prints to **stderr** — so we tee stderr through a
//! reader thread that watches for that signature and, since the guest is already
//! down, restores the terminal, tears down gvproxy, and exits cleanly.

use std::io::Read;
use std::os::unix::io::{FromRawFd, RawFd};
use std::thread;

/// Redirect fd 2 through a pipe and spawn a reader that tees everything back to
/// the real stderr while scanning for libkrun's HVF-panic signature. Call once,
/// before `krun_start_enter`. On any failure to set up the pipe it leaves stderr
/// untouched — the worst case is the original hang, never lost output.
pub fn install() {
    let real_err: RawFd = unsafe { libc::dup(libc::STDERR_FILENO) };
    if real_err < 0 {
        return;
    }
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        unsafe { libc::close(real_err) };
        return;
    }
    let (rd, wr) = (fds[0], fds[1]);
    // Point fd 2 at the pipe's write end. Everything written to stderr from now
    // on — our `tracing` logs and libkrun's panic alike — flows to the reader.
    if unsafe { libc::dup2(wr, libc::STDERR_FILENO) } < 0 {
        unsafe {
            libc::close(rd);
            libc::close(wr);
            libc::close(real_err);
        }
        return;
    }
    unsafe { libc::close(wr) }; // fd 2 now holds the write end; drop the spare.
    thread::spawn(move || reader(rd, real_err));
}

fn reader(rd: RawFd, real_err: RawFd) {
    let mut pipe = unsafe { std::fs::File::from_raw_fd(rd) };
    let mut buf = [0u8; 4096];
    // A rolling window of recent stderr, so a signature split across reads still
    // matches. One panic line easily fits in far less than this.
    let mut window: Vec<u8> = Vec::with_capacity(8192);
    loop {
        let n = match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break, // EOF: every fd-2 writer is gone (process ending).
            Ok(n) => n,
        };
        // Tee first, so users see identical stderr output regardless.
        write_all(real_err, &buf[..n]);

        window.extend_from_slice(&buf[..n]);
        if window.len() > 8192 {
            let cut = window.len() - 8192;
            window.drain(..cut);
        }
        if is_hvf_panic(&window) {
            let note = b"\n[bsdkrun] guest halted. Working around a known libkrun HVF panic on \
                         SMP shutdown (PSCI CPU_OFF/AFFINITY_INFO); boot with --cpus 1 to avoid \
                         it. Exiting cleanly.\n";
            write_all(real_err, note);
            // The guest is already down; just clean up our side and exit 0.
            crate::tty::restore_stdin_termios();
            crate::net::kill_tracked_gvproxy();
            unsafe { libc::_exit(0) };
        }
    }
}

/// libkrun's vCPU panic prints a line like
/// `thread 'fc_vcpu 0' panicked at src/hvf/src/lib.rs:549:20: Unexpected val=…`.
/// Requiring both markers keeps ordinary output from ever matching.
fn is_hvf_panic(window: &[u8]) -> bool {
    contains(window, b"panicked") && contains(window, b"src/hvf")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Write the whole buffer to a raw fd, retrying short writes. Never closes `fd`.
fn write_all(fd: RawFd, mut buf: &[u8]) {
    while !buf.is_empty() {
        let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        buf = &buf[n as usize..];
    }
}
