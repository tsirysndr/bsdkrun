//! Console broker + client for detached microVMs.
//!
//! A detached VM has no terminal, so its guest console (hvc0) is wired to one
//! end of a socketpair; a broker thread inside the VM process pumps the other
//! end to two places: an append-only `console.log`, and a Unix socket
//! (`console.sock`) that `logs`/`shell` clients connect to. `logs` reads the
//! file (optionally following the live socket); `shell` connects to the socket
//! and proxies the local terminal in raw mode — like `docker attach`, pointed at
//! the guest console.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;

use anyhow::{Context, Result};

/// Set up a detached console under `dir`. Returns the fd to give libkrun for the
/// guest console (the caller `dup2`s it onto fd 0 and 1); a broker thread is
/// spawned that fans guest output to `console.log` and to `console.sock`
/// clients, and forwards client input back to the guest.
///
/// The guest console must be a **PTY slave**: libkrun's implicit virtio-console
/// only writes guest output to fd 1 when it is a tty (otherwise it merely *logs*
/// the output). The broker holds the PTY master.
pub fn setup_detached(dir: &Path) -> Result<RawFd> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // Give the PTY a sane default window size so guest programs don't see 0x0.
    let mut winsize = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
        )
    } != 0
    {
        anyhow::bail!("openpty: {}", std::io::Error::last_os_error());
    }
    // Raw mode on the slave so console bytes pass through untouched (no CR/LF
    // translation or echo between libkrun and the broker).
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave, &mut t) == 0 {
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(slave, libc::TCSANOW, &t);
        }
    }

    let log_path = dir.join("console.log");
    let sock_path = dir.join("console.sock");
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path)
        .with_context(|| format!("binding {}", sock_path.display()))?;
    listener.set_nonblocking(true).ok();
    let logfile = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;

    thread::spawn(move || broker_loop(master, listener, logfile, log_path));
    Ok(slave)
}

/// How much recent console output to replay to a newly-attached client, so it
/// immediately sees the current prompt instead of a blank screen.
const REPLAY_BYTES: u64 = 4096;

/// The broker: `poll` over the PTY master, the listener, and connected clients.
fn broker_loop(
    master_fd: RawFd,
    listener: UnixListener,
    mut logfile: std::fs::File,
    log_path: std::path::PathBuf,
) {
    // Non-blocking master, owned as a File for convenient I/O.
    unsafe {
        let flags = libc::fcntl(master_fd, libc::F_GETFL);
        libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let mut guest = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let mut clients: Vec<UnixStream> = Vec::new();

    loop {
        let mut pfds: Vec<libc::pollfd> = Vec::with_capacity(2 + clients.len());
        pfds.push(pollin(guest.as_raw_fd()));
        pfds.push(pollin(listener.as_raw_fd()));
        for c in &clients {
            pfds.push(pollin(c.as_raw_fd()));
        }
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }

        // Guest console output -> log + all clients.
        if pfds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let mut buf = [0u8; 4096];
            match guest.read(&mut buf) {
                Ok(0) => break, // guest console closed: VM is going away
                Ok(k) => {
                    let _ = logfile.write_all(&buf[..k]);
                    let _ = logfile.flush();
                    // Mirror to clients best-effort: keep a client on a transient
                    // WouldBlock (a slow reader just misses those bytes); only
                    // drop it on a real disconnect.
                    clients.retain_mut(|c| match c.write_all(&buf[..k]) {
                        Ok(()) => true,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                        Err(_) => false,
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }

        // New client connections.
        if pfds[1].revents & libc::POLLIN != 0 {
            while let Ok((mut stream, _)) = listener.accept() {
                // Replay recent console output (while the socket is still
                // blocking) so an attaching `shell`/`logs -f` sees the current
                // prompt immediately rather than a blank screen.
                replay_tail(&log_path, &mut stream);
                stream.set_nonblocking(true).ok();
                clients.push(stream);
            }
        }

        // Client input (from `shell`) -> guest console. Only iterate the clients
        // that were in *this* poll set: `accept` above may have appended new ones
        // (they have no pollfd yet and are handled next iteration).
        let polled_clients = pfds.len() - 2;
        let mut drop_idx = Vec::new();
        for i in 0..polled_clients {
            if pfds[2 + i].revents & (libc::POLLIN | libc::POLLHUP) == 0 {
                continue;
            }
            let mut buf = [0u8; 4096];
            match clients[i].read(&mut buf) {
                Ok(0) => drop_idx.push(i),
                Ok(k) => {
                    // Forward to the guest best-effort. A transient WouldBlock on
                    // the (nonblocking) PTY master must NOT kill the broker — just
                    // drop those input bytes; only a hard error means the PTY /
                    // VM is gone.
                    match guest.write_all(&buf[..k]) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => return, // PTY master closed: VM is going away
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => drop_idx.push(i),
            }
        }
        for i in drop_idx.into_iter().rev() {
            clients.remove(i);
        }
    }
}

/// Send the last `REPLAY_BYTES` of the console log to a freshly-connected client.
fn replay_tail(log_path: &Path, stream: &mut UnixStream) {
    use std::io::{Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log_path) else {
        return;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let tail = REPLAY_BYTES.min(len);
    if f.seek(SeekFrom::End(-(tail as i64))).is_err() {
        return;
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_ok() {
        let _ = stream.write_all(&buf);
    }
}

fn pollin(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}

/// Byte that detaches an interactive `shell` session: Ctrl-] (like telnet).
const DETACH_KEY: u8 = 0x1d;

/// Connect to a detached VM's console socket and proxy the local terminal in
/// raw mode (for `shell`). Returns when the guest closes the console or the user
/// presses Ctrl-] to detach.
pub fn attach_interactive(dir: &Path) -> Result<()> {
    let sock = dir.join("console.sock");
    let stream = UnixStream::connect(&sock).with_context(|| {
        format!(
            "connecting to {} — is the microVM running (and detached)?",
            sock.display()
        )
    })?;

    // Put stdin into raw mode so keystrokes (incl. Ctrl-C) reach the guest; the
    // guard restores it on exit.
    let _raw = RawGuard::enable();
    eprintln!("[bsdkrun] attached — press Ctrl-] to detach");

    // Reader thread: guest console -> local stdout.
    let mut reader = stream.try_clone().context("cloning console stream")?;
    let reader_thread = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let stdout = std::io::stdout();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    let mut h = stdout.lock();
                    if h.write_all(&buf[..k]).is_err() || h.flush().is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Main: local stdin -> guest console, watching for the detach key.
    let mut writer = stream;
    let mut input = [0u8; 4096];
    loop {
        let k = unsafe { libc::read(0, input.as_mut_ptr().cast(), input.len()) };
        if k <= 0 {
            break;
        }
        let k = k as usize;
        if let Some(pos) = input[..k].iter().position(|&b| b == DETACH_KEY) {
            // Forward anything before the detach key, then stop.
            let _ = writer.write_all(&input[..pos]);
            break;
        }
        if writer.write_all(&input[..k]).is_err() {
            break;
        }
    }
    // Shut the socket down both ways so the reader thread's blocking read
    // returns (dropping alone wouldn't — the cloned fd keeps it half-open).
    let _ = writer.shutdown(std::net::Shutdown::Both);
    let _ = reader_thread.join();
    eprintln!("\n[bsdkrun] detached");
    Ok(())
}

/// Stream a detached VM's console socket to stdout (for `logs -f`), read-only,
/// until the guest closes it or the user interrupts.
pub fn follow(dir: &Path) -> Result<()> {
    let sock = dir.join("console.sock");
    let mut stream =
        UnixStream::connect(&sock).with_context(|| format!("connecting to {}", sock.display()))?;
    let mut buf = [0u8; 4096];
    let stdout = std::io::stdout();
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                let mut h = stdout.lock();
                if h.write_all(&buf[..k]).is_err() || h.flush().is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// RAII guard that puts stdin into raw mode and restores it on drop.
struct RawGuard {
    saved: Option<libc::termios>,
}

impl RawGuard {
    fn enable() -> Self {
        if unsafe { libc::isatty(0) } != 1 {
            return RawGuard { saved: None };
        }
        let mut term = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(0, &mut term) } != 0 {
            return RawGuard { saved: None };
        }
        let saved = term;
        unsafe { libc::cfmakeraw(&mut term) };
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &term) };
        RawGuard { saved: Some(saved) }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        if let Some(term) = self.saved {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &term) };
        }
    }
}
