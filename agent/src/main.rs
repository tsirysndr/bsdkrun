//! bsdkrun in-guest exec agent (async / tokio).
//!
//! Listens on TCP (port 1024) and, for each connection the host opens, runs one
//! command and proxies its stdin/stdout/stderr + exit code back over a tiny
//! framed protocol. This is how `bsdkrun exec` spawns a *new* process inside the
//! guest (the console can't — it's a single shared shell). The host reaches this
//! port through gvproxy's host->guest TCP forward, so one static binary works on
//! Linux, FreeBSD and NetBSD guests without any vsock/virtio dependency.
//!
//! Protocol (all integers little-endian):
//!   request  (host -> agent, once):  u8 tty; u32 argc, argc×(u32 len, bytes);
//!                                     u32 envc, envc×(u32 len, bytes "K=V")
//!   frame    (both directions):      u8 channel; u32 len; len bytes
//!     channels: 0 stdin, 1 stdout, 2 stderr, 3 exit (payload = u32 code),
//!               4 winsize (payload = u16 rows, u16 cols)
//!
//! The same binary doubles as an in-guest CLI, run through `bsdkrun exec`:
//!   `bsdkrun-agent tailscale <install|start|status|setup>`  (tailnet access)
//!   `bsdkrun-agent ssh <setup|add-key|status>`              (key-based sshd)
//!   `bsdkrun-agent systemd <setup|status|disable>`          (Linux: systemd as PID 1)

mod ssh;
#[cfg(target_os = "linux")]
mod systemd;
mod tailscale;
mod util;

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// TCP port the agent listens on (gvproxy forwards a host port to it).
const PORT: u16 = 1024;

const CH_STDIN: u8 = 0;
const CH_STDOUT: u8 = 1;
const CH_STDERR: u8 = 2;
const CH_EXIT: u8 = 3;
const CH_WINSZ: u8 = 4;

/// Shared, lock-guarded writer half of a connection (stdout/stderr/exit all
/// frame onto the same socket).
type Writer = Arc<Mutex<OwnedWriteHalf>>;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // CLI mode: `bsdkrun-agent tailscale <install|start|status|setup> ...` —
    // typically invoked *through* the daemon via `bsdkrun exec`. Everything
    // else (no args) is the exec daemon below.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("tailscale") => std::process::exit(tailscale::run(&args[1..])),
        Some("ssh") => std::process::exit(ssh::run(&args[1..])),
        #[cfg(target_os = "linux")]
        Some("systemd") => std::process::exit(systemd::run(&args[1..])),
        #[cfg(not(target_os = "linux"))]
        Some("systemd") => {
            eprintln!("bsdkrun-agent: systemd is a Linux-guest feature (BSDs use rc.d)");
            std::process::exit(1);
        }
        Some(other) => {
            eprintln!("bsdkrun-agent: unknown command {other:?} (try: tailscale, ssh, systemd)");
            std::process::exit(2);
        }
        None => {} // no args: run the exec daemon below
    }

    let listener = match TcpListener::bind(("0.0.0.0", PORT)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bsdkrun-agent: TCP listen on :{PORT} failed: {e}");
            std::process::exit(1);
        }
    };
    loop {
        match listener.accept().await {
            // One task per connection so a crash never takes the agent down and
            // concurrent execs work.
            Ok((sock, _)) => {
                let _ = sock.set_nodelay(true);
                tokio::spawn(handle(sock));
            }
            Err(_) => continue,
        }
    }
}

async fn handle(sock: tokio::net::TcpStream) {
    let (mut rd, wr) = sock.into_split();
    let wr: Writer = Arc::new(Mutex::new(wr));

    let req = match read_request(&mut rd).await {
        Some(r) => r,
        None => return,
    };
    if req.argv.is_empty() {
        let _ = send_frame(&wr, CH_EXIT, &127u32.to_le_bytes()).await;
        return;
    }
    if req.tty {
        run_tty(req, rd, wr).await;
    } else {
        run_piped(req, rd, wr).await;
    }
}

struct Req {
    tty: bool,
    argv: Vec<Vec<u8>>,
    env: Vec<Vec<u8>>,
}

/// Build a `std::process::Command` from the request's argv + env (env is applied
/// on top of the agent's own environment).
fn build_command(req: &Req) -> std::process::Command {
    let mut cmd = std::process::Command::new(os_str(&req.argv[0]));
    for a in &req.argv[1..] {
        cmd.arg(os_str(a));
    }
    for kv in &req.env {
        if let Some(pos) = kv.iter().position(|&b| b == b'=') {
            cmd.env(os_str(&kv[..pos]), os_str(&kv[pos + 1..]));
        }
    }
    cmd
}

/// Non-TTY: separate stdout/stderr pipes + a stdin pipe.
async fn run_piped(req: Req, mut rd: OwnedReadHalf, wr: Writer) {
    let mut cmd = tokio::process::Command::from(build_command(&req));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            let _ = send_frame(&wr, CH_EXIT, &127u32.to_le_bytes()).await;
            return;
        }
    };

    let mut cin = child.stdin.take().unwrap();
    let cout = child.stdout.take().unwrap();
    let cerr = child.stderr.take().unwrap();

    let t_out = tokio::spawn(pipe_to_channel(cout, wr.clone(), CH_STDOUT));
    let t_err = tokio::spawn(pipe_to_channel(cerr, wr.clone(), CH_STDERR));

    // host frames -> child stdin (until stdin EOF or the connection closes).
    let t_in = tokio::spawn(async move {
        while let Some((chan, data)) = read_frame(&mut rd).await {
            if chan == CH_STDIN {
                if data.is_empty() || cin.write_all(&data).await.is_err() {
                    break;
                }
            }
        }
        drop(cin); // close the child's stdin
    });

    let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0);
    let _ = t_out.await;
    let _ = t_err.await;
    t_in.abort();
    let _ = send_frame(&wr, CH_EXIT, &(code as u32).to_le_bytes()).await;
}

/// TTY: the child runs on a PTY (stdout+stderr merged, controlling terminal set).
async fn run_tty(req: Req, mut rd: OwnedReadHalf, wr: Writer) {
    let (master, slave) = match open_pty() {
        Ok(p) => p,
        Err(_) => {
            let _ = send_frame(&wr, CH_EXIT, &127u32.to_le_bytes()).await;
            return;
        }
    };

    let mut cmd = build_command(&req);
    let slave_dup = || -> std::io::Result<Stdio> {
        let fd = unsafe { libc::dup(slave) };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { Stdio::from_raw_fd(fd) })
        }
    };
    match (slave_dup(), slave_dup(), slave_dup()) {
        (Ok(i), Ok(o), Ok(e)) => {
            cmd.stdin(i).stdout(o).stderr(e);
        }
        _ => {
            unsafe { libc::close(master) };
            unsafe { libc::close(slave) };
            let _ = send_frame(&wr, CH_EXIT, &127u32.to_le_bytes()).await;
            return;
        }
    }
    unsafe {
        cmd.pre_exec(|| {
            // New session with the PTY as controlling terminal (fd 0 is the slave
            // by now — std sets up stdio before pre_exec).
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let mut child = match tokio::process::Command::from(cmd).spawn() {
        Ok(c) => c,
        Err(_) => {
            unsafe { libc::close(master) };
            unsafe { libc::close(slave) };
            let _ = send_frame(&wr, CH_EXIT, &127u32.to_le_bytes()).await;
            return;
        }
    };
    unsafe { libc::close(slave) };

    // Non-blocking master, shared between the read and write tasks via an
    // AsyncFd (epoll/kqueue readiness — this is what makes us notice the slave
    // closing when the child exits, which a blocking read on a PTY master won't).
    set_nonblocking(master);
    let afd = match AsyncFd::new(unsafe { OwnedFd::from_raw_fd(master) }) {
        Ok(a) => Arc::new(a),
        Err(_) => {
            let _ = send_frame(&wr, CH_EXIT, &127u32.to_le_bytes()).await;
            return;
        }
    };

    // master -> host (channel stdout).
    let afd_r = afd.clone();
    let wr_r = wr.clone();
    let t_out = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            let mut guard = match afd_r.readable().await {
                Ok(g) => g,
                Err(_) => break,
            };
            match guard.try_io(|inner| {
                let fd = inner.get_ref().as_raw_fd();
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(Ok(0)) => break,       // EOF
                Ok(Err(_)) => break,      // slave closed (EIO)
                Ok(Ok(n)) => {
                    if send_frame(&wr_r, CH_STDOUT, &buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_would_block) => continue,
            }
        }
    });

    // host frames -> master (stdin) / winsize.
    let afd_w = afd.clone();
    let t_in = tokio::spawn(async move {
        while let Some((chan, data)) = read_frame(&mut rd).await {
            match chan {
                CH_STDIN => {
                    if data.is_empty() || write_all_fd(&afd_w, &data).await.is_err() {
                        break;
                    }
                }
                CH_WINSZ if data.len() >= 4 => {
                    let ws = libc::winsize {
                        ws_row: u16::from_le_bytes([data[0], data[1]]),
                        ws_col: u16::from_le_bytes([data[2], data[3]]),
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    };
                    let fd = afd_w.get_ref().as_raw_fd();
                    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) };
                }
                _ => {}
            }
        }
    });

    let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(0);
    // The child is gone; its slave closes, so t_out drains the last output then
    // ends. Bound the wait in case a leftover fd keeps the slave open.
    let _ = tokio::time::timeout(Duration::from_millis(500), t_out).await;
    t_in.abort();
    let _ = send_frame(&wr, CH_EXIT, &(code as u32).to_le_bytes()).await;
}

/// Async `write_all` onto a raw fd wrapped in an AsyncFd (the PTY master).
async fn write_all_fd(afd: &AsyncFd<OwnedFd>, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let mut guard = afd.writable().await?;
        match guard.try_io(|inner| {
            let fd = inner.get_ref().as_raw_fd();
            let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }) {
            Ok(Ok(n)) => data = &data[n..],
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => continue,
        }
    }
    Ok(())
}

/// Copy an async reader (child stdout/stderr) into framed writes on `chan`.
async fn pipe_to_channel<R: AsyncRead + Unpin>(mut src: R, wr: Writer, chan: u8) {
    let mut buf = [0u8; 8192];
    loop {
        match src.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if send_frame(&wr, chan, &buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Open a PTY master/slave pair without libutil (posix_openpt + grantpt +
/// unlockpt + ptsname), so it links the same on Linux, FreeBSD and NetBSD.
fn open_pty() -> std::io::Result<(RawFd, RawFd)> {
    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
    if master < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::grantpt(master) } != 0 || unsafe { libc::unlockpt(master) } != 0 {
        unsafe { libc::close(master) };
        return Err(std::io::Error::last_os_error());
    }
    let name = unsafe { libc::ptsname(master) };
    if name.is_null() {
        unsafe { libc::close(master) };
        return Err(std::io::Error::last_os_error());
    }
    let slave = unsafe { libc::open(name, libc::O_RDWR | libc::O_NOCTTY) };
    if slave < 0 {
        unsafe { libc::close(master) };
        return Err(std::io::Error::last_os_error());
    }
    Ok((master, slave))
}

fn set_nonblocking(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

// --- framing helpers ---------------------------------------------------------

async fn read_request(r: &mut OwnedReadHalf) -> Option<Req> {
    let tty = read_u8(r).await? != 0;
    let argc = read_u32(r).await?;
    let mut argv = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        argv.push(read_bytes(r).await?);
    }
    let envc = read_u32(r).await?;
    let mut env = Vec::with_capacity(envc as usize);
    for _ in 0..envc {
        env.push(read_bytes(r).await?);
    }
    Some(Req { tty, argv, env })
}

async fn send_frame(w: &Writer, chan: u8, payload: &[u8]) -> std::io::Result<()> {
    let mut g = w.lock().await;
    g.write_all(&[chan]).await?;
    g.write_all(&(payload.len() as u32).to_le_bytes()).await?;
    g.write_all(payload).await?;
    g.flush().await
}

async fn read_frame(r: &mut OwnedReadHalf) -> Option<(u8, Vec<u8>)> {
    let chan = read_u8(r).await?;
    let data = read_bytes(r).await?;
    Some((chan, data))
}

async fn read_bytes(r: &mut OwnedReadHalf) -> Option<Vec<u8>> {
    let len = read_u32(r).await? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.ok()?;
    Some(buf)
}

async fn read_u8(r: &mut OwnedReadHalf) -> Option<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).await.ok()?;
    Some(b[0])
}

async fn read_u32(r: &mut OwnedReadHalf) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).await.ok()?;
    Some(u32::from_le_bytes(b))
}

fn os_str(b: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(b).to_owned()
}
