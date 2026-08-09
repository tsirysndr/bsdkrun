//! Console broker + client for detached microVMs.
//!
//! A detached VM has no terminal, so its guest console (hvc0) is wired to one
//! end of a pty; a broker process alongside the VM pumps the other
//! end to two places: an append-only `console.log`, and a Unix socket
//! (`console.sock`) that `logs`/`shell` clients connect to. `logs` reads the
//! file (optionally following the live socket); `shell` connects to the socket
//! and proxies the local terminal in raw mode — like `docker attach`, pointed at
//! the guest console.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use anyhow::{Context, Result};

/// Set up a detached console under `dir`. Returns the fd to give libkrun for the
/// guest console (the caller `dup2`s it onto fd 0 and 1); a broker process is
/// forked off that fans guest output to `console.log` and to `console.sock`
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
            // A raw pointer, not `&mut`: apple types this parameter `*mut
            // winsize` and linux types it `*const`, and only a pointer suits
            // both — `&mut` trips clippy on linux, `&` fails to compile on mac.
            &mut winsize as *mut libc::winsize,
        )
    } != 0
    {
        anyhow::bail!("openpty: {}", std::io::Error::last_os_error());
    }
    // Leave the slave at the default (cooked) tty settings: the guest console
    // (hvc0) is a normal terminal, so its `\n`->`\r\n` output translation must
    // stay on — making it raw produces staircased output on the attaching
    // terminal. Interactive programs (the shell's readline) set their own modes.

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

    // The broker runs in its own process, not a thread of the VM's.
    //
    // libkrun does not return from `krun_start_enter`: when the guest powers
    // off, its `Vmm::stop` ends the process with a bare `_exit`, which kills
    // every other thread wherever it happens to stand. A broker *thread* would
    // therefore lose whatever the guest wrote in its last moments — and a
    // unikernel's whole life is its last moments: it prints and powers off in
    // tens of milliseconds, so `logs` showed an empty console for a machine
    // that had run perfectly. No amount of flushing on our side helps, because
    // no code of ours runs after that `_exit`. A separate process outlives it
    // and reads the pty dry afterwards.
    //
    // `live` is how it knows when that happened: the VM process holds the write
    // end and never touches it, so the broker sees EOF exactly when the process
    // dies, however it dies. CLOEXEC keeps the write end out of gvproxy and
    // friends, which would otherwise hold the pipe open past the VM's exit.
    let mut live = [-1 as RawFd; 2];
    if unsafe { libc::pipe(live.as_mut_ptr()) } != 0 {
        anyhow::bail!("pipe: {}", std::io::Error::last_os_error());
    }
    for fd in live {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }

    match unsafe { libc::fork() } {
        -1 => anyhow::bail!("fork (console broker): {}", std::io::Error::last_os_error()),
        0 => {
            // Fork once more so the broker is reparented to init: a machine
            // that outlives its broker must not collect a zombie child.
            if unsafe { libc::fork() } == 0 {
                // Its own session, so the hangup the kernel sends to the pty's
                // foreground group when the VM process (the session leader)
                // dies cannot kill the broker before it has drained. The pty
                // slave stays open here on purpose — it keeps the terminal
                // alive so that last read returns the guest's output rather
                // than EOF.
                unsafe {
                    libc::setsid();
                    libc::close(live[1]);
                    detach_stdio();
                }
                broker_loop(master, live[0], listener, logfile, log_path);
            }
            unsafe { libc::_exit(0) };
        }
        pid => {
            // VM process: reap the intermediate fork and keep only what the
            // guest needs — the pty slave, and the liveness pipe whose close is
            // the broker's cue.
            unsafe {
                libc::waitpid(pid, std::ptr::null_mut(), 0);
                libc::close(master);
                libc::close(live[0]);
            }
            drop(listener);
            drop(logfile);
        }
    }
    Ok(slave)
}

/// Point the broker's stdin/stdout/stderr at `/dev/null`.
///
/// It forks before the guest console is wired up, so it starts life holding
/// whatever stdio bsdkrun was launched with — and it outlives the machine by
/// design. A caller that runs `bsdkrun ... -d` on a pipe and waits for that
/// pipe to close (as the SDK's `runCli` does, awaiting Node's `close` event)
/// would wait for the whole machine instead of for the id it just printed.
///
/// # Safety
/// Async-signal-safe calls only; must be called in the freshly forked child.
unsafe fn detach_stdio() {
    let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
    if devnull < 0 {
        return;
    }
    libc::dup2(devnull, libc::STDIN_FILENO);
    libc::dup2(devnull, libc::STDOUT_FILENO);
    libc::dup2(devnull, libc::STDERR_FILENO);
    if devnull > libc::STDERR_FILENO {
        libc::close(devnull);
    }
}

/// Window (from the end of the log) searched for the current line to replay to
/// a newly-attached client. Only that line — the live prompt — is sent, so the
/// attach shows the prompt immediately without dumping old scrollback.
const REPLAY_WINDOW: u64 = 4096;

/// The broker: `poll` over the PTY master, the listener, the VM process's
/// liveness pipe, and connected clients.
fn broker_loop(
    master_fd: RawFd,
    live_fd: RawFd,
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
    let mut paint = FirmwarePaintFilter::new();

    loop {
        let mut pfds: Vec<libc::pollfd> = Vec::with_capacity(3 + clients.len());
        pfds.push(pollin(guest.as_raw_fd()));
        pfds.push(pollin(listener.as_raw_fd()));
        pfds.push(pollin(live_fd));
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
                    let out = paint.feed(&buf[..k]);
                    let _ = logfile.write_all(&out);
                    let _ = logfile.flush();
                    // Mirror to clients best-effort: keep a client on a transient
                    // WouldBlock (a slow reader just misses those bytes); only
                    // drop it on a real disconnect.
                    clients.retain_mut(|c| match c.write_all(&out) {
                        Ok(()) => true,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                        Err(_) => false,
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        }

        // The VM process is gone. Its output can still be sitting in the pty,
        // so read the terminal dry before going away — this is the whole reason
        // the broker is a process and not a thread.
        if pfds[2].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut buf = [0u8; 4096];
            while let Ok(k) = guest.read(&mut buf) {
                if k == 0 {
                    break;
                }
                let out = paint.feed(&buf[..k]);
                let _ = logfile.write_all(&out);
                for c in clients.iter_mut() {
                    let _ = c.write_all(&out);
                }
            }
            // The guest is gone for good, so a half-parsed escape will never be
            // completed: emit it rather than swallowing the last bytes.
            let rest = paint.flush();
            let _ = logfile.write_all(&rest);
            for c in clients.iter_mut() {
                let _ = c.write_all(&rest);
            }
            let _ = logfile.flush();
            return;
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
        let polled_clients = pfds.len() - 3;
        let mut drop_idx = Vec::new();
        for i in 0..polled_clients {
            if pfds[3 + i].revents & (libc::POLLIN | libc::POLLHUP) == 0 {
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

    // Left the loop by a `break` (guest console closed / poll error): same as the
    // drain path, don't let a held-back partial escape take bytes with it.
    let rest = paint.flush();
    if !rest.is_empty() {
        let _ = logfile.write_all(&rest);
        for c in clients.iter_mut() {
            let _ = c.write_all(&rest);
        }
    }
    let _ = logfile.flush();
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

    // Print the banner BEFORE entering raw mode, so its trailing newline still
    // gets a carriage return (in raw mode `\n` is a bare line-feed, which would
    // leave the cursor mid-line and render the guest prompt off to the right).
    eprintln!("[bsdkrun] attached — press Ctrl-] to detach (or `exit` the shell)");
    // Raw mode so keystrokes (incl. Ctrl-C) reach the guest; restored on drop.
    let raw = RawGuard::enable();

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
    // Restore the terminal (cooked) before the banner, so its newline is normal.
    drop(raw);
    eprintln!("\n[bsdkrun] detached");
    Ok(())
}

/// EDK2 paints over the console before handing off to the guest: it emits
/// `ESC[2J` (erase display) + `ESC[01;01H` (cursor home) four times, then its
/// two `BdsDxe:` lines — so whatever was on screen is erased and the handoff
/// lines land on row 1, on top of anything written there. For a unikernel that
/// prints and exits in milliseconds, that can bury the guest's entire output.
///
/// Stripping those sequences makes the console append-only, like a log. But the
/// broker is shared by *every* machine kind, and a guest running `clear`, `vi`
/// or `top` needs its cursor control intact — so the filter is pinned to the
/// firmware and nothing else. It starts *disarmed* and only switches on when
/// EDK2 introduces itself with [`FW_BANNER`], then retires for good at the
/// [`HANDOFF_ANCHOR`]. A guest that direct-boots (Linux, NetBSD) never prints
/// that banner, so the filter stays off for its whole life.
///
/// A budget backstops both ends: if the banner has not appeared within
/// [`FW_PAINT_BUDGET`] bytes it never will, and a firmware that somehow never
/// reaches its handoff line still cannot hold the filter open forever.
///
/// Bounding this by byte count *alone* is not enough, and the mistake is easy
/// to make: a Debian guest reaches its init in ~3 KiB of kernel log, so a
/// budget-only filter was still armed and ate the `clear` its entrypoint ran.
const FW_PAINT_BUDGET: usize = 8192;

/// EDK2's opening line — the filter arms only after seeing this, so a
/// direct-booted guest is never touched.
const FW_BANNER: &[u8] = b"UEFI firmware (version";

/// Firmware handoff line: nothing EDK2 prints after this, so the filter can
/// retire the moment it appears.
const HANDOFF_ANCHOR: &[u8] = b"BdsDxe: starting";

/// CSI final bytes worth stripping inside the preamble: erase-display (`J`),
/// cursor positioning (`H`, `f`) and mode set/reset (`h`, `l`, as in EDK2's
/// `ESC[=3h`). Notably *not* `m` — colour should survive.
const FW_PAINT_FINALS: &[u8] = b"JHfhl";

/// Where the filter is in the firmware's short life.
#[derive(PartialEq)]
enum Paint {
    /// Watching for [`FW_BANNER`]. Bytes pass through untouched.
    Disarmed,
    /// Firmware is talking: strip its screen-painting until the handoff.
    Armed,
    /// Retired for the rest of the machine's life.
    Off,
}

/// Strips EDK2's screen-painting escapes from the firmware preamble. See
/// [`FW_PAINT_BUDGET`] for why this is deliberately short-lived.
struct FirmwarePaintFilter {
    state: Paint,
    /// A CSI sequence split across two reads waits here for its tail.
    carry: Vec<u8>,
    /// Trailing emitted bytes, so a marker is still found when a read splits it.
    probe: Vec<u8>,
    seen: usize,
}

impl FirmwarePaintFilter {
    fn new() -> Self {
        FirmwarePaintFilter {
            state: Paint::Disarmed,
            carry: Vec::new(),
            probe: Vec::new(),
            seen: 0,
        }
    }

    /// Offset in `chunk` just past `marker`, tolerating a read that splits it.
    /// `None` if the marker has not appeared yet.
    fn saw(&mut self, chunk: &[u8], marker: &[u8]) -> Option<usize> {
        let carried = self.probe.len();
        self.probe.extend_from_slice(chunk);
        // Only a window the size of the marker (plus this chunk) can ever match,
        // so trim the front and keep the probe bounded.
        let cut = self
            .probe
            .len()
            .saturating_sub(chunk.len() + marker.len())
            .min(carried);
        self.probe.drain(..cut);
        let at = self.probe.windows(marker.len()).position(|w| w == marker)? + marker.len();
        // Back out the carried-over prefix to land in `chunk`'s coordinates. The
        // marker can end inside that prefix only if we already reported it.
        Some(at.saturating_sub(carried - cut))
    }

    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        self.seen += data.len();

        if self.state == Paint::Off {
            return data.to_vec();
        }

        // Not the firmware's console yet: pass bytes through, watching for EDK2
        // to announce itself. It may do so part-way into this very chunk, so arm
        // at that point and filter the remainder rather than the whole read.
        let mut head: &[u8] = &[];
        let mut body = data;
        if self.state == Paint::Disarmed {
            match self.saw(data, FW_BANNER) {
                Some(at) => {
                    self.state = Paint::Armed;
                    self.probe = Vec::new();
                    (head, body) = data.split_at(at.min(data.len()));
                }
                None => {
                    if self.seen >= FW_PAINT_BUDGET {
                        self.state = Paint::Off;
                        self.probe = Vec::new();
                    }
                    return data.to_vec();
                }
            }
        }

        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(body);

        let mut out = Vec::with_capacity(head.len() + buf.len());
        out.extend_from_slice(head);
        let mut i = 0;
        while i < buf.len() {
            if buf[i] != 0x1b {
                out.push(buf[i]);
                i += 1;
                continue;
            }
            match parse_csi(&buf[i..]) {
                // Complete sequence: drop it if it paints, else pass it through.
                CsiScan::Complete { len, final_byte } => {
                    if !FW_PAINT_FINALS.contains(&final_byte) {
                        out.extend_from_slice(&buf[i..i + len]);
                    }
                    i += len;
                }
                // Truncated by the read boundary — hold it for the next one.
                CsiScan::Partial => {
                    self.carry = buf[i..].to_vec();
                    break;
                }
                // A lone ESC or some non-CSI escape: not ours, pass it on.
                CsiScan::NotCsi => {
                    out.push(buf[i]);
                    i += 1;
                }
            }
        }

        // Retire at the handoff, or if this firmware never gets there — and let
        // any half-parsed sequence through on the way out, so nothing is eaten.
        if self.saw(&out, HANDOFF_ANCHOR).is_some() || self.seen >= FW_PAINT_BUDGET {
            self.state = Paint::Off;
            out.extend_from_slice(&std::mem::take(&mut self.carry));
            self.probe = Vec::new();
        }
        out
    }

    /// Release anything still held back (the VM is going away).
    fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

/// How a byte slice starting at ESC parses as a CSI sequence:
/// `ESC [` params (0x30-0x3F) intermediates (0x20-0x2F) final (0x40-0x7E).
enum CsiScan {
    Complete { len: usize, final_byte: u8 },
    Partial,
    NotCsi,
}

fn parse_csi(s: &[u8]) -> CsiScan {
    debug_assert_eq!(s.first(), Some(&0x1b));
    match s.get(1) {
        None => CsiScan::Partial,
        Some(&b'[') => {
            let mut i = 2;
            while i < s.len() && (0x30..=0x3f).contains(&s[i]) {
                i += 1;
            }
            while i < s.len() && (0x20..=0x2f).contains(&s[i]) {
                i += 1;
            }
            match s.get(i) {
                None => CsiScan::Partial,
                Some(&f) if (0x40..=0x7e).contains(&f) => CsiScan::Complete {
                    len: i + 1,
                    final_byte: f,
                },
                // Out-of-range byte: not a well-formed CSI, don't touch it.
                Some(_) => CsiScan::NotCsi,
            }
        }
        Some(_) => CsiScan::NotCsi,
    }
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

/// How long [`connect_when_ready`] waits for a just-booted machine's broker.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const CONNECT_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// Connect to a machine's `console.sock`, waiting for it to appear.
///
/// `run_detached` prints the machine id in the parent right after `fork()`, but
/// the child only binds the socket once it reaches [`setup_detached`]. A client
/// that acts on that id immediately (the daemon hands it straight to the
/// desktop, which opens the console view) would otherwise lose the race and see
/// a bare ENOENT. Retry the two errors that mean "not yet": no socket file, and
/// a socket nobody is listening on.
fn connect_when_ready(sock: &Path) -> Result<UnixStream> {
    let deadline = std::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        match UnixStream::connect(sock) {
            Ok(s) => return Ok(s),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(CONNECT_POLL);
            }
            Err(e) => return Err(e).with_context(|| format!("connecting to {}", sock.display())),
        }
    }
}

/// Stream a detached VM's console socket to stdout (for `logs -f`), read-only,
/// until the guest closes it or the user interrupts.
pub fn follow(dir: &Path) -> Result<()> {
    let sock = dir.join("console.sock");
    let mut stream = connect_when_ready(&sock)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The real preamble libkrun's EDK2 emits, byte for byte.
    const PREAMBLE: &[u8] = b"UEFI firmware (version edk2-13e8adac8a)\r\n\
        \x1b[2J\x1b[01;01H\x1b[=3h\x1b[2J\x1b[01;01H\x1b[2J\x1b[01;01H\x1b[=3h\x1b[2J\x1b[01;01H\
        BdsDxe: loading Boot0001\r\nBdsDxe: starting Boot0001\r\n";

    #[test]
    fn strips_the_firmware_paint_but_keeps_the_text() {
        let mut f = FirmwarePaintFilter::new();
        let out = f.feed(PREAMBLE);
        assert!(!out.contains(&0x1b), "escapes survived: {:?}", out);
        assert!(out.starts_with(b"UEFI firmware"));
        assert!(out.ends_with(b"BdsDxe: starting Boot0001\r\n"));
    }

    /// A CSI split across reads must not leak its tail as visible text.
    #[test]
    fn handles_a_sequence_split_across_reads() {
        for split in 1..PREAMBLE.len() {
            let mut f = FirmwarePaintFilter::new();
            let mut out = f.feed(&PREAMBLE[..split]);
            out.extend_from_slice(&f.feed(&PREAMBLE[split..]));
            out.extend_from_slice(&f.flush());
            assert!(!out.contains(&0x1b), "split at {split} leaked an escape");
            assert!(
                out.ends_with(b"BdsDxe: starting Boot0001\r\n"),
                "split at {split} mangled the tail: {:?}",
                String::from_utf8_lossy(&out)
            );
        }
    }

    /// The whole point of the bound: once the guest is up, its cursor control is
    /// none of our business — `clear`, `vi` and `top` must come through intact.
    #[test]
    fn retires_at_the_handoff_so_guest_escapes_survive() {
        let mut f = FirmwarePaintFilter::new();
        f.feed(PREAMBLE);
        assert!(f.state == Paint::Off, "should retire at the BdsDxe handoff");
        let guest = b"\x1b[2J\x1b[H$ vi\r\n";
        assert_eq!(f.feed(guest), guest, "guest escapes must pass through");
    }

    /// Regression: a direct-booted guest prints no EDK2 banner, so the filter
    /// must never arm — a Debian guest reaches init in ~3 KiB, well inside the
    /// budget, and a budget-only filter ate the `clear` its entrypoint ran.
    #[test]
    fn never_arms_without_the_firmware_banner() {
        let mut f = FirmwarePaintFilter::new();
        f.feed(b"[    0.076848] Run /.bsdkrun-init as init process\r\n");
        let guest = b"\x1b[2J\x1b[Hcleared\r\n";
        assert_eq!(f.feed(guest), guest, "direct-boot guest must be untouched");
        assert!(f.state != Paint::Armed);
    }

    /// A firmware that never reaches its handoff still cannot hold the filter
    /// open for the life of the machine.
    #[test]
    fn retires_on_budget_when_no_handoff_appears() {
        let mut f = FirmwarePaintFilter::new();
        f.feed(FW_BANNER);
        assert!(f.state == Paint::Armed);
        f.feed(&vec![b'x'; FW_PAINT_BUDGET]);
        assert!(f.state == Paint::Off, "budget should retire the filter");
        let guest = b"\x1b[2Jclear";
        assert_eq!(f.feed(guest), guest);
    }

    /// Colour is not screen-painting — SGR must survive even while armed.
    #[test]
    fn keeps_colour_while_armed() {
        let mut f = FirmwarePaintFilter::new();
        f.feed(FW_BANNER);
        assert_eq!(f.feed(b"\x1b[31mred\x1b[0m"), b"\x1b[31mred\x1b[0m");
    }

    #[test]
    fn passes_through_a_lone_escape() {
        let mut f = FirmwarePaintFilter::new();
        f.feed(FW_BANNER);
        let mut out = f.feed(b"a\x1bZb");
        out.extend_from_slice(&f.flush());
        assert_eq!(out, b"a\x1bZb");
    }
}
