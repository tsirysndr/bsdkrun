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

/// Window (from the end of the log) searched for the current line to replay to
/// a newly-attached client. Only that line — the live prompt — is sent, so the
/// attach shows the prompt immediately without dumping old scrollback.
const REPLAY_WINDOW: u64 = 4096;

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

/// Replay just the current console line (the live prompt) to a freshly-connected
/// client, so the attach shows the prompt immediately without old scrollback.
fn replay_tail(log_path: &Path, stream: &mut UnixStream) {
    use std::io::{Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log_path) else {
        return;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return;
    }
    let window = REPLAY_WINDOW.min(len);
    if f.seek(SeekFrom::End(-(window as i64))).is_err() {
        return;
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return;
    }
    // Send only the bytes after the last newline — the current (prompt) line.
    let start = buf
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let _ = stream.write_all(&buf[start..]);
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

/// Invisible (OSC) marker the guest init emits when its console shell exits, so
/// an attached `shell` detaches back to the host. Kept in sync with the string
/// printed by `crate::linux`'s generated `/init`.
const EXIT_MARKER: &[u8] = b"\x1b]6666;bsdkrun-exit\x07";

/// Connect to a detached machine's console socket and proxy the local terminal
/// in raw mode (for `shell`). Returns when: the user presses Ctrl-], the guest
/// shell exits (via the exit marker), or the machine goes away.
pub fn attach_interactive(dir: &Path) -> Result<()> {
    use std::os::fd::AsRawFd;

    let sock = dir.join("console.sock");
    let stream = UnixStream::connect(&sock).with_context(|| {
        format!(
            "connecting to {} — is the machine running (and detached)?",
            sock.display()
        )
    })?;
    let sock_fd = stream.as_raw_fd();

    // Raw mode so keystrokes (incl. Ctrl-C) reach the guest; restored on drop.
    let _raw = RawGuard::enable();
    eprintln!("[bsdkrun] attached — press Ctrl-] to detach (or `exit` the shell)");

    let stdout = std::io::stdout();
    let mut filter = MarkerFilter::new();
    // The replayed console (sent on connect) can contain exit markers from
    // *previous* sessions — ignore those. Only markers seen after the user has
    // interacted mean the shell they're in has exited.
    let mut user_typed = false;
    let mut input = [0u8; 4096];
    let mut sockbuf = [0u8; 4096];

    loop {
        let mut pfds = [pollin(0), pollin(sock_fd)];
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }

        // Guest console -> stdout (filtering out the exit marker).
        if pfds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            match (&stream).read(&mut sockbuf) {
                Ok(0) => break, // machine gone
                Ok(k) => {
                    let (out, markers) = filter.feed(&sockbuf[..k]);
                    {
                        let mut h = stdout.lock();
                        let _ = h.write_all(&out);
                        let _ = h.flush();
                    }
                    if markers > 0 && user_typed {
                        break; // the shell we were in exited: return to host
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }

        // Local stdin -> guest console (watching for the detach key).
        if pfds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let k = unsafe { libc::read(0, input.as_mut_ptr().cast(), input.len()) };
            if k <= 0 {
                break;
            }
            user_typed = true;
            let k = k as usize;
            if let Some(pos) = input[..k].iter().position(|&b| b == DETACH_KEY) {
                let _ = (&stream).write_all(&input[..pos]);
                break;
            }
            if (&stream).write_all(&input[..k]).is_err() {
                break;
            }
        }
    }

    // Flush any bytes the filter was holding back (a possible partial marker).
    let rest = filter.flush();
    {
        let mut h = stdout.lock();
        let _ = h.write_all(&rest);
        let _ = h.flush();
    }
    eprintln!("\n[bsdkrun] detached");
    Ok(())
}

/// Streams bytes through, stripping any [`EXIT_MARKER`] occurrences (and holding
/// back a trailing partial marker across reads), reporting how many it stripped.
struct MarkerFilter {
    carry: Vec<u8>,
}

impl MarkerFilter {
    fn new() -> Self {
        MarkerFilter { carry: Vec::new() }
    }

    /// Returns (bytes to emit, number of markers stripped).
    fn feed(&mut self, data: &[u8]) -> (Vec<u8>, usize) {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(data);
        // Hold back a suffix that could be the start of a marker split across reads.
        let hold = partial_marker_suffix(&buf);
        let split = buf.len() - hold;
        self.carry = buf[split..].to_vec();
        let process = &buf[..split];

        let mut out = Vec::with_capacity(process.len());
        let mut markers = 0;
        let mut i = 0;
        while i < process.len() {
            if process[i..].starts_with(EXIT_MARKER) {
                markers += 1;
                i += EXIT_MARKER.len();
            } else {
                out.push(process[i]);
                i += 1;
            }
        }
        (out, markers)
    }

    fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

/// Length of the longest suffix of `buf` that is a proper prefix of the marker
/// (so it might complete on the next read).
fn partial_marker_suffix(buf: &[u8]) -> usize {
    let max = (EXIT_MARKER.len() - 1).min(buf.len());
    for h in (1..=max).rev() {
        if buf[buf.len() - h..] == EXIT_MARKER[..h] {
            return h;
        }
    }
    0
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
