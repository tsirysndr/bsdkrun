//! User-mode networking for guests, backed by gvproxy (gvisor-tap-vsock).
//!
//! libkrun's built-in TSI backend only works for Linux guests (it needs an
//! in-kernel shim), so BSD guests need a real virtio-net device wired to a
//! userspace network stack. gvproxy provides exactly that: a NAT'd network on
//! 192.168.127.0/24 (gateway .1, guest .2) with DHCP + DNS, reachable over a
//! unixgram "vfkit" socket that libkrun connects to (see
//! [`crate::krun::Ctx::add_net_gvproxy`]).
//!
//! We shell out to the `gvproxy` binary (Homebrew: `brew install gvproxy`),
//! matching the rest of bsdkrun's "drive tools already on the host" approach.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tracing::{debug, info, warn};

/// The address gvproxy leases to the (first) guest that connects. Port forwards
/// target this IP inside gvproxy's virtual network.
pub const GUEST_IP: &str = "192.168.127.2";

/// PID of the live gvproxy child, published for the signal handler to reap.
/// `-1` means "none running". A single VM per process means one entry is enough.
static GVPROXY_PID: AtomicI32 = AtomicI32::new(-1);

/// Kill the tracked gvproxy child, if any. Called from the interrupt handler in
/// [`crate::tty`] (so an interrupted VM doesn't orphan gvproxy and leak its host
/// ssh-port, blocking the next VM). Uses only async-signal-safe operations.
pub fn kill_tracked_gvproxy() {
    let pid = GVPROXY_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// Remove `bsdkrun-net-<pid>` socket dirs whose owning bsdkrun is no longer
/// running. Called before each launch so dirs orphaned by a signal-/SIGKILL-ed
/// VM don't pile up in `$TMPDIR`. A live pid's dir is always left untouched.
fn sweep_stale_dirs() {
    let tmp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&tmp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("bsdkrun-net-"))
            .and_then(|p| p.parse::<i32>().ok())
        else {
            continue;
        };
        // kill(pid, 0) probes existence: 0 => alive; EPERM => alive (not ours).
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Ask the OS for an unused loopback TCP port (bind :0, read it back, release).
/// Used to give each gvproxy a unique host ssh-port so multiple guests can run
/// at once — gvproxy always binds one and refuses port 0.
fn free_local_port() -> Result<u16> {
    let listener =
        TcpListener::bind("127.0.0.1:0").context("reserving a free host port for gvproxy")?;
    let port = listener.local_addr()?.port();
    // Dropped here; the tiny window before gvproxy rebinds it is acceptable.
    Ok(port)
}

/// A host→guest TCP port forward.
#[derive(Clone, Copy)]
pub struct PortForward {
    pub host: u16,
    pub guest: u16,
}

/// A running gvproxy process serving one microVM's network. Killed and its
/// sockets removed on drop.
pub struct Gvproxy {
    child: Child,
    dir: PathBuf,
    /// The `-listen-vfkit` unixgram socket libkrun connects to.
    pub vfkit_socket: PathBuf,
    /// The `-listen` HTTP control socket (used to configure port forwards).
    control_socket: PathBuf,
    /// Host port gvproxy forwards to the guest's SSH (`:22`). Unique per VM.
    ssh_port: u16,
}

impl Gvproxy {
    /// Spawn gvproxy, wait for its vfkit socket to come up, and configure the
    /// requested port forwards. The returned handle must be kept alive for the
    /// lifetime of the VM — dropping it tears the network down.
    pub fn spawn(ports: &[PortForward]) -> Result<Self> {
        // Reap socket dirs left by VMs that were signal-killed (our handler uses
        // `_exit`, which can't run `Drop`; SIGKILL can't run anything at all).
        sweep_stale_dirs();

        // Per-VM scratch dir for the two sockets. `std::env::temp_dir()` honours
        // $TMPDIR; the pid keeps concurrent VMs from colliding.
        let dir = std::env::temp_dir().join(format!("bsdkrun-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating gvproxy socket dir {}", dir.display()))?;
        let vfkit_socket = dir.join("vfkit.sock");
        let control_socket = dir.join("control.sock");

        // Stale sockets from a crashed prior run would make gvproxy's bind fail.
        let _ = std::fs::remove_file(&vfkit_socket);
        let _ = std::fs::remove_file(&control_socket);

        // gvproxy always binds one host TCP port for its built-in SSH forward
        // (guest :22) and rejects port 0, so give each VM a *unique* one —
        // otherwise a second guest dies with "address already in use".
        let ssh_port = free_local_port()?;

        let bin = locate()?;
        info!(gvproxy = %bin.display(), "starting user-mode networking");
        // gvproxy's own logs go to a file in its dir (kept off the guest console
        // on stdout); we surface the tail if it dies unexpectedly.
        let log_path = dir.join("gvproxy.log");
        let log = std::fs::File::create(&log_path)
            .with_context(|| format!("creating gvproxy log {}", log_path.display()))?;
        let child = Command::new(&bin)
            .arg("-ssh-port")
            .arg(ssh_port.to_string())
            .arg("-listen")
            .arg(format!("unix://{}", control_socket.display()))
            .arg("-listen-vfkit")
            .arg(format!("unixgram://{}", vfkit_socket.display()))
            .stdout(std::process::Stdio::null())
            .stderr(log)
            .spawn()
            .with_context(|| format!("spawning gvproxy ({})", bin.display()))?;

        // Publish the pid before we might block, so an interrupt during startup
        // still reaps this child. The handlers themselves are installed once, up
        // front, by `crate::tty::install` (they also restore the terminal).
        GVPROXY_PID.store(child.id() as i32, Ordering::SeqCst);

        let mut gv = Gvproxy {
            child,
            dir,
            vfkit_socket,
            control_socket,
            ssh_port,
        };

        gv.wait_for_socket(&gv.vfkit_socket.clone(), Duration::from_secs(5))?;

        for pf in ports {
            gv.expose_port(*pf).with_context(|| {
                format!(
                    "forwarding host port {} to guest port {}",
                    pf.host, pf.guest
                )
            })?;
            info!(host = pf.host, guest = pf.guest, "forwarding TCP port");
        }

        info!(
            ssh_port = gv.ssh_port,
            "networking up — SSH into the guest with: ssh -p {} user@127.0.0.1", gv.ssh_port
        );

        Ok(gv)
    }

    /// Poll until `sock` exists (gvproxy creates it a beat after launch) or the
    /// deadline passes, checking the child didn't die in the meantime.
    fn wait_for_socket(&mut self, sock: &Path, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if sock.exists() {
                debug!(socket = %sock.display(), "gvproxy socket is up");
                return Ok(());
            }
            if let Some(status) = self.child.try_wait().context("polling gvproxy")? {
                let log = std::fs::read_to_string(self.dir.join("gvproxy.log")).unwrap_or_default();
                bail!(
                    "gvproxy exited before its socket appeared (status: {status})\n{}",
                    log.trim()
                );
            }
            if Instant::now() >= deadline {
                bail!(
                    "gvproxy did not create {} within {:?}",
                    sock.display(),
                    timeout
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Ask gvproxy (over its HTTP control socket) to forward a host TCP port to
    /// the guest. gvproxy holds the forward and connects once the guest is up,
    /// so this is safe to call before the VM boots.
    fn expose_port(&self, pf: PortForward) -> Result<()> {
        let body = format!(
            r#"{{"local":"127.0.0.1:{}","remote":"{}:{}","protocol":"tcp"}}"#,
            pf.host, GUEST_IP, pf.guest
        );
        let resp = self.control_post("/services/forwarder/expose", &body)?;
        // gvproxy answers 200 with an empty body on success.
        if !resp.starts_with("HTTP/1.1 200") && !resp.starts_with("HTTP/1.0 200") {
            let status = resp.lines().next().unwrap_or("<no status line>");
            bail!("gvproxy rejected the port forward: {status}");
        }
        Ok(())
    }

    /// Minimal HTTP/1.1 POST over the unix control socket. gvproxy's API is tiny
    /// and local, so a hand-rolled request avoids pulling in an HTTP crate.
    fn control_post(&self, path: &str, body: &str) -> Result<String> {
        let mut stream = UnixStream::connect(&self.control_socket).with_context(|| {
            format!(
                "connecting to gvproxy control socket {}",
                self.control_socket.display()
            )
        })?;
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: gvproxy\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(req.as_bytes())
            .context("writing gvproxy control request")?;
        stream
            .shutdown(Shutdown::Write)
            .context("half-closing gvproxy control request")?;
        let mut resp = String::new();
        stream
            .read_to_string(&mut resp)
            .context("reading gvproxy control response")?;
        Ok(resp)
    }
}

impl Drop for Gvproxy {
    fn drop(&mut self) {
        // Clear the published pid first so the signal handler won't also try to
        // kill it (we're already tearing down cleanly here).
        GVPROXY_PID.store(-1, Ordering::SeqCst);
        // Kill gvproxy so it doesn't linger after the VM is gone. `start_enter`
        // ends with `std::process::exit`, which skips destructors — callers must
        // drop this explicitly (or let it drop before exiting) for cleanup. The
        // signal handler covers the interrupted-VM case.
        if let Err(e) = self.child.kill() {
            warn!(error = %e, "failed to kill gvproxy");
        }
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Find the `gvproxy` binary on `PATH`, with an actionable error if missing.
/// Also used as an availability probe before deciding to bring networking up.
pub fn locate() -> Result<PathBuf> {
    // `BSDKRUN_GVPROXY` lets users point at a non-PATH install.
    if let Some(p) = std::env::var_os("BSDKRUN_GVPROXY") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        bail!("BSDKRUN_GVPROXY={} does not exist", p.display());
    }

    let out = Command::new("/usr/bin/which")
        .arg("gvproxy")
        .output()
        .context("running `which gvproxy`")?;
    if out.status.success() {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    bail!(
        "gvproxy not found on PATH. It provides user-mode networking for the guest.\n\
         Install it with `brew install gvproxy`, or set BSDKRUN_GVPROXY to its path."
    )
}

/// A locally-administered, unicast default MAC for the guest NIC (the `0x02`
/// low bits of the first octet mark it locally-administered + unicast).
pub const DEFAULT_MAC: [u8; 6] = [0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee];

/// Parse a `AA:BB:CC:DD:EE:FF` MAC string into six bytes.
pub fn parse_mac(s: &str) -> Result<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        bail!("MAC address must have 6 colon-separated octets, got {s:?}");
    }
    let mut mac = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(p, 16)
            .with_context(|| format!("invalid MAC octet {p:?} in {s:?}"))?;
    }
    Ok(mac)
}
