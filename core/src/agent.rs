//! Host side of the in-guest exec agent (see `agent/`).
//!
//! The agent binary is injected into the guest rootfs (Linux) or installed by
//! the user (BSD); the guest runs it, listening on TCP port 1024. gvproxy
//! forwards a per-machine host port to that guest port, and `exec`/`shell`
//! connect to `127.0.0.1:<host-port>` and speak the framed protocol below to run
//! a command with full stdin/stdout/stderr + exit-code forwarding (+ a PTY).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::fetch::{cache_dir, run};

/// TCP port the guest agent listens on (matches `agent/src/main.rs`).
pub const GUEST_PORT: u16 = 1024;
/// Where the agent binary is installed inside the guest rootfs.
pub const GUEST_PATH: &str = "sbin/bsdkrun-agent";

/// GitHub release the prebuilt agents are published to (see the `release`
/// workflows). Per-(os, arch) binaries are attached as assets.
const AGENT_RELEASE_BASE: &str = "https://github.com/tsirysndr/bsdkrun/releases/download";
/// GitHub resolves this to whatever the newest published release is, so the
/// fallback needs no API call and no token.
const AGENT_RELEASE_LATEST: &str = "https://github.com/tsirysndr/bsdkrun/releases/latest/download";

// Frame channels (must match the agent).
const CH_STDIN: u8 = 0;
const CH_STDOUT: u8 = 1;
const CH_STDERR: u8 = 2;
const CH_EXIT: u8 = 3;
const CH_WINSZ: u8 = 4;

use crate::host::{Arch, GuestOs};

/// Release asset / cache file name, e.g. `bsdkrun-agent.linux-aarch64`.
fn asset_name(os: GuestOs, arch: Arch) -> String {
    format!("bsdkrun-agent.{}-{}", os.slug(), arch.slug())
}

/// Env var overriding the download with a local prebuilt binary (for dev).
fn env_key(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Linux => "BSDKRUN_AGENT_LINUX",
        GuestOs::Freebsd => "BSDKRUN_AGENT_FREEBSD",
        GuestOs::Netbsd => "BSDKRUN_AGENT_NETBSD",
    }
}

/// Release tag to pull agents from: `$BSDKRUN_AGENT_VERSION`, else this build's
/// version (agents are published alongside each tagged bsdkrun release).
fn agent_version() -> String {
    std::env::var("BSDKRUN_AGENT_VERSION")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")))
}

/// Download (once) and cache the guest agent for `(os, arch)`, returning its
/// path. Cached under `<cache>/agent/<version>/<asset>`; set the matching
/// `BSDKRUN_AGENT_<OS>` env var to use a local prebuilt binary instead.
pub fn ensure_agent(os: GuestOs, arch: Arch) -> Result<PathBuf> {
    if let Ok(p) = std::env::var(env_key(os)) {
        if !p.is_empty() {
            let p = PathBuf::from(p);
            if !p.exists() {
                bail!("{}={} does not exist", env_key(os), p.display());
            }
            return Ok(p);
        }
    }

    let version = agent_version();
    let asset = asset_name(os, arch);
    let dir = cache_dir()?.join("agent").join(&version);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating agent cache dir {}", dir.display()))?;
    let dest = dir.join(&asset);
    if dest.exists() {
        info!(path = %dest.display(), "using cached exec agent");
        return Ok(dest);
    }

    let url = format!("{AGENT_RELEASE_BASE}/{version}/{asset}");
    info!(%url, "downloading exec agent…");
    let tmp = dir.join(format!("{asset}.partial"));
    let _ = std::fs::remove_file(&tmp);
    let fetch = |from: &str| {
        run(
            Command::new("curl")
                .args(["-L", "--fail", "--progress-bar", "-o"])
                .arg(&tmp)
                .arg(from),
            "curl (download agent)",
        )
    };
    if let Err(first) = fetch(&url) {
        // The version bump lands before the release does, so a source build
        // sits at a tag GitHub has never heard of and every VM fails to boot
        // on a 404 for its agent. The newest published agent is the right
        // thing to use there, and saying so beats a dead runner: the protocol
        // is what matters, and it does not change per patch release.
        let _ = std::fs::remove_file(&tmp);
        let latest = format!("{AGENT_RELEASE_LATEST}/{asset}");
        warn!(
            "no agent published for {version} ({first:#}); falling back to the latest release — \
             set BSDKRUN_AGENT_VERSION to pin one"
        );
        info!(url = %latest, "downloading exec agent from the latest release…");
        fetch(&latest).with_context(|| {
            format!(
                "downloading bsdkrun-agent ({asset}) from {version} and from the latest release — \
                 is a bsdkrun release published with that asset? Override the tag with \
                 BSDKRUN_AGENT_VERSION, or point {} at a local binary.",
                env_key(os),
            )
        })?;
    }
    set_executable(&tmp)?;
    std::fs::rename(&tmp, &dest).context("moving agent into cache")?;
    Ok(dest)
}

/// Public download URL of an agent asset (for user-facing hints — BSD guests
/// aren't auto-injected, so the user fetches this themselves).
pub fn asset_url(os: GuestOs, arch: Arch) -> String {
    format!(
        "{AGENT_RELEASE_BASE}/{}/{}",
        agent_version(),
        asset_name(os, arch)
    )
}

fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod +x {}", path.display()))
}

/// Install the agent into a Linux guest rootfs at `GUEST_PATH` (mode 0755),
/// downloading + caching it first if needed.
pub fn inject_linux(rootfs: &Path) -> Result<()> {
    // Guest arch == host arch under KVM/HVF.
    let bin = ensure_agent(GuestOs::Linux, Arch::current()?)?;
    let bytes =
        std::fs::read(&bin).with_context(|| format!("reading agent binary {}", bin.display()))?;
    crate::oci::write_rootfs_file(rootfs, GUEST_PATH, &bytes, 0o755)
}

/// File under a machine's state dir holding the host TCP port gvproxy forwards
/// to the guest agent. Written at boot; read by `exec`/`shell`.
pub fn port_file(machine_dir: &Path) -> std::path::PathBuf {
    machine_dir.join("agent.port")
}

/// Read a machine's forwarded agent port, if it was recorded at boot.
pub fn read_port(machine_dir: &Path) -> Option<u16> {
    std::fs::read_to_string(port_file(machine_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Readiness probe: run a no-op (`/bin/sh -c :`) in the guest and report whether
/// the agent answered with an exit frame. Because gvproxy accepts the host
/// connection before the guest's agent is up, a bare TCP connect isn't enough —
/// this does a full protocol round-trip, so it only returns true once the guest
/// agent is actually serving. Idempotent, so it's safe to poll in a boot loop.
pub fn ping(host_port: u16) -> bool {
    let Ok(stream) = TcpStream::connect(("127.0.0.1", host_port)) else {
        return false;
    };
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let argv = ["/bin/sh".to_string(), "-c".to_string(), ":".to_string()];
    if write_request(&stream, false, &argv, &[]).is_err() {
        return false;
    }
    let mut reader = stream;
    loop {
        match read_frame(&mut reader) {
            Some((CH_EXIT, _)) => return true,
            Some(_) => continue,  // drain any stdout/stderr frames
            None => return false, // closed or timed out before EXIT: not ready
        }
    }
}

/// Run `argv` inside the guest via its agent (a gvproxy-forwarded TCP port on
/// loopback), forwarding stdio and returning the guest process's exit code.
/// `tty` requests a PTY (interactive).
pub fn exec(host_port: u16, argv: &[String], env: &[String], tty: bool) -> Result<i32> {
    let stream = connect(host_port)?;
    stream.set_nodelay(true).ok();

    write_request(&stream, tty, argv, env).context("sending exec request")?;

    let _raw = tty.then(RawGuard::enable);

    // Forward local stdin -> guest, in a background thread.
    let mut stdin_w = stream.try_clone().context("cloning agent stream")?;
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let done_stdin = done.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            if done_stdin.load(Ordering::Relaxed) {
                break;
            }
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                let _ = write_frame(&mut stdin_w, CH_STDIN, &[]); // EOF
                break;
            }
            if write_frame(&mut stdin_w, CH_STDIN, &buf[..n as usize]).is_err() {
                break;
            }
        }
    });

    // In TTY mode, send the initial window size (SIGWINCH updates would be nice
    // but a single size covers the common case).
    if tty {
        if let Some((rows, cols)) = local_winsize() {
            let mut payload = Vec::with_capacity(4);
            payload.extend_from_slice(&rows.to_le_bytes());
            payload.extend_from_slice(&cols.to_le_bytes());
            let mut w = stream.try_clone()?;
            let _ = write_frame(&mut w, CH_WINSZ, &payload);
        }
    }

    // Read guest output frames until EXIT.
    let mut reader = stream;
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut code = 0;
    let mut saw_frame = false;
    let mut saw_exit = false;
    loop {
        match read_frame(&mut reader) {
            Some((CH_STDOUT, data)) => {
                saw_frame = true;
                let mut h = stdout.lock();
                let _ = h.write_all(&data);
                let _ = h.flush();
            }
            Some((CH_STDERR, data)) => {
                saw_frame = true;
                let mut h = stderr.lock();
                let _ = h.write_all(&data);
                let _ = h.flush();
            }
            Some((CH_EXIT, data)) => {
                if data.len() >= 4 {
                    code = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i32;
                }
                saw_exit = true;
                break;
            }
            Some(_) => {}
            None => break, // agent closed without an explicit exit
        }
    }
    done.store(true, Ordering::Relaxed);
    if !saw_exit && !saw_frame {
        anyhow::bail!("the guest agent accepted the connection but sent no output");
    }
    Ok(code)
}

/// Run `argv` inside the guest via its agent without touching local stdio —
/// stdout/stderr are drained and discarded. For fire-and-forget setup commands
/// (e.g. tweaking `/etc/resolv.conf` post-boot). Returns the guest exit code.
pub fn exec_quiet(host_port: u16, argv: &[String]) -> Result<i32> {
    let mut stream = connect(host_port)?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    write_request(&stream, false, argv, &[]).context("sending exec request")?;
    // Signal stdin EOF so a command reading stdin doesn't hang.
    let _ = write_frame(&mut stream, CH_STDIN, &[]);
    let mut code = 0;
    let mut saw_exit = false;
    loop {
        match read_frame(&mut stream) {
            Some((CH_EXIT, data)) => {
                if data.len() >= 4 {
                    code = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i32;
                }
                saw_exit = true;
                break;
            }
            Some(_) => {} // drain stdout/stderr
            None => break,
        }
    }
    // gvproxy accepts the forwarded port before the guest agent is serving, so a
    // connect can succeed yet yield no exit frame — treat that as a failure
    // rather than a silent success.
    if !saw_exit {
        bail!("guest agent accepted the connection but sent no exit status");
    }
    Ok(code)
}

/// Run `argv` in the guest with programmatic stdio: `input` is streamed to the
/// guest's stdin, the guest's stdout is written to `out`, and stderr is captured
/// and returned rather than printed. Returns `(exit code, stderr)`.
///
/// This is the primitive `bsdkrun cp` is built on. It differs from [`exec`] in
/// the two ways a file transfer needs: the bytes come from somewhere other than
/// the host's fd 0/1 (a file, or an in-memory tar), and the guest's stderr has
/// to survive as a *string* so a failure can say "no such file or directory"
/// instead of leaking a diagnostic into the copied data.
pub fn exec_stream(
    host_port: u16,
    argv: &[String],
    input: Option<Box<dyn Read + Send>>,
    out: &mut dyn Write,
) -> Result<(i32, String)> {
    let stream = connect(host_port)?;
    stream.set_nodelay(true).ok();
    write_request(&stream, false, argv, &[]).context("sending exec request")?;

    // Pump the input in a thread: the guest can block on a full socket buffer
    // while its stdout backs up behind frames we haven't read yet, so writing
    // and reading have to make progress independently or a large file deadlocks.
    let mut w = stream.try_clone().context("cloning agent stream")?;
    let pump = thread::spawn(move || -> std::io::Result<()> {
        if let Some(mut src) = input {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => write_frame(&mut w, CH_STDIN, &buf[..n])?,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        // Always close stdin, including when there was no input at all: a guest
        // reading stdin (`cat > file`) waits for EOF and would otherwise hang.
        write_frame(&mut w, CH_STDIN, &[])
    });

    let mut reader = stream;
    let mut stderr = Vec::new();
    let mut code = 0;
    let mut saw_exit = false;
    loop {
        match read_frame(&mut reader) {
            Some((CH_STDOUT, data)) => out.write_all(&data).context("writing copied data")?,
            Some((CH_STDERR, data)) => stderr.extend_from_slice(&data),
            Some((CH_EXIT, data)) => {
                if data.len() >= 4 {
                    code = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as i32;
                }
                saw_exit = true;
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    out.flush().context("flushing copied data")?;

    // A broken pipe here is the guest exiting early (a failed `cat >`), which
    // the exit code already describes far better than "write failed" does.
    if let Ok(Err(e)) = pump.join() {
        if e.kind() != std::io::ErrorKind::BrokenPipe && !saw_exit {
            return Err(e).context("streaming data to the guest");
        }
    }
    if !saw_exit {
        bail!("guest agent closed the connection without an exit status");
    }
    Ok((code, String::from_utf8_lossy(&stderr).trim().to_string()))
}

/// Connect to the forwarded agent port, retrying briefly: gvproxy holds the
/// forward but the guest agent may still be starting right after boot.
fn connect(host_port: u16) -> Result<TcpStream> {
    let addr = ("127.0.0.1", host_port);
    let mut last = None;
    for _ in 0..40 {
        match TcpStream::connect(addr) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = Some(e);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(anyhow::anyhow!(
        "could not reach the guest agent on 127.0.0.1:{host_port}: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn write_request(mut stream: &TcpStream, tty: bool, argv: &[String], env: &[String]) -> Result<()> {
    let mut buf = Vec::new();
    buf.push(tty as u8);
    buf.extend_from_slice(&(argv.len() as u32).to_le_bytes());
    for a in argv {
        buf.extend_from_slice(&(a.len() as u32).to_le_bytes());
        buf.extend_from_slice(a.as_bytes());
    }
    buf.extend_from_slice(&(env.len() as u32).to_le_bytes());
    for e in env {
        buf.extend_from_slice(&(e.len() as u32).to_le_bytes());
        buf.extend_from_slice(e.as_bytes());
    }
    stream.write_all(&buf)?;
    stream.flush()?;
    Ok(())
}

fn write_frame(w: &mut impl Write, chan: u8, payload: &[u8]) -> std::io::Result<()> {
    w.write_all(&[chan])?;
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut hdr = [0u8; 5];
    r.read_exact(&mut hdr).ok()?;
    let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
    let mut data = vec![0u8; len];
    r.read_exact(&mut data).ok()?;
    Some((hdr[0], data))
}

fn local_winsize() -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 {
        Some((ws.ws_row, ws.ws_col))
    } else {
        None
    }
}

/// RAII raw-mode guard for the local terminal (TTY exec).
struct RawGuard {
    saved: Option<libc::termios>,
}

impl RawGuard {
    fn enable() -> Self {
        if unsafe { libc::isatty(0) } != 1 {
            return RawGuard { saved: None };
        }
        let mut term: libc::termios = unsafe { std::mem::zeroed() };
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
