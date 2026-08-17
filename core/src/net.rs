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
use std::os::unix::net::{UnixDatagram, UnixStream};
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
/// at once — gvproxy always binds one and refuses port 0. Also used for the
/// per-machine forwarded agent port.
pub fn free_local_port() -> Result<u16> {
    let listener =
        TcpListener::bind("127.0.0.1:0").context("reserving a free host port for gvproxy")?;
    let port = listener.local_addr()?.port();
    // Dropped here; the tiny window before gvproxy rebinds it is acceptable.
    Ok(port)
}

/// `preferred` if nothing else holds it, else a free port the kernel picks.
///
/// Used when a machine inherits another's port forwards (`bsdkrun branch`):
/// the machine it was branched from is usually still running and still bound,
/// and gvproxy simply fails to bind rather than telling anyone why.
pub fn free_host_port(bind: std::net::IpAddr, preferred: u16) -> Option<u16> {
    if TcpListener::bind((bind, preferred)).is_ok() {
        return Some(preferred);
    }
    TcpListener::bind((bind, 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// A host→guest TCP port forward.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PortForward {
    /// Host interface to bind, e.g. `127.0.0.1` (default) or `0.0.0.0` (LAN-reachable).
    #[serde(default = "PortForward::default_bind")]
    pub bind: std::net::IpAddr,
    pub host: u16,
    pub guest: u16,
}

impl PortForward {
    fn default_bind() -> std::net::IpAddr {
        std::net::Ipv4Addr::LOCALHOST.into()
    }

    /// A forward on the default (loopback-only) bind address. Used for
    /// bsdkrun's own internal forwards (the exec/shell agent port) — those
    /// are never meant to be LAN-reachable, regardless of what the user's
    /// `--port` flags asked for.
    pub fn loopback(host: u16, guest: u16) -> Self {
        PortForward {
            bind: Self::default_bind(),
            host,
            guest,
        }
    }
}

impl std::fmt::Display for PortForward {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.bind == Self::default_bind() {
            write!(f, "{}:{}", self.host, self.guest)
        } else {
            write!(f, "{}:{}:{}", self.bind, self.host, self.guest)
        }
    }
}

/// Serialize a machine's port forwards for the `machines.ports` DB column:
/// comma-joined `HOST:GUEST` pairs, or `None` if there are none.
pub fn format_ports(ports: &[PortForward]) -> Option<String> {
    if ports.is_empty() {
        return None;
    }
    Some(
        ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Parse the `machines.ports` DB column back into port forwards, e.g. to
/// re-apply them on `start`. Malformed entries (there shouldn't be any — this
/// only ever round-trips what `format_ports` wrote) are skipped.
pub fn parse_ports(s: &str) -> Vec<PortForward> {
    s.split(',')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// A running gvproxy process serving one microVM's network. Killed and its
/// sockets removed on drop.
pub struct Gvproxy {
    child: Child,
    dir: PathBuf,
    /// The `-listen-vfkit` unixgram socket libkrun connects to.
    pub vfkit_socket: PathBuf,
    /// The `-listen` HTTP control socket (used to configure port forwards).
    pub control_socket: PathBuf,
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
            r#"{{"local":"{}:{}","remote":"{}:{}","protocol":"tcp"}}"#,
            pf.bind, pf.host, GUEST_IP, pf.guest
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
        control_post_to(&self.control_socket, path, body)
    }
}

/// Ask a gvproxy at `control_socket` to forward host `bind:host` to
/// `guest_ip:guest`. Used to add a forward to a **shared network** gvproxy that
/// this process doesn't own (each member targets its own IP).
pub fn expose_on_control(
    control_socket: &Path,
    bind: std::net::IpAddr,
    host: u16,
    guest_ip: &str,
    guest: u16,
) -> Result<()> {
    let body =
        format!(r#"{{"local":"{bind}:{host}","remote":"{guest_ip}:{guest}","protocol":"tcp"}}"#);
    let resp = control_post_to(control_socket, "/services/forwarder/expose", &body)?;
    if !resp.starts_with("HTTP/1.1 200") && !resp.starts_with("HTTP/1.0 200") {
        let status = resp.lines().next().unwrap_or("<no status line>");
        bail!("gvproxy rejected the port forward: {status}");
    }
    Ok(())
}

/// Drop a forward previously added by [`expose_on_control`].
///
/// gvproxy keys a forward by its *local* address alone, which is also why two
/// forwards cannot share a host port. Unexposing something that was never
/// exposed answers 500; that is not worth failing a reconcile over, so a
/// caller syncing a set of forwards can ignore the error.
pub fn unexpose_on_control(control_socket: &Path, bind: std::net::IpAddr, host: u16) -> Result<()> {
    let body = format!(r#"{{"local":"{bind}:{host}","protocol":"tcp"}}"#);
    let resp = control_post_to(control_socket, "/services/forwarder/unexpose", &body)?;
    if !resp.starts_with("HTTP/1.1 200") && !resp.starts_with("HTTP/1.0 200") {
        let status = resp.lines().next().unwrap_or("<no status line>");
        bail!("gvproxy rejected the unexpose: {status}");
    }
    Ok(())
}

/// The file a machine's state dir carries naming its gvproxy control socket.
///
/// The socket itself lives in a temp dir keyed by the VM process's pid, which
/// is knowable but not *discoverable* — so the boot path writes the path down
/// and anything that wants to add a forward to a **running** machine (the
/// Docker port publisher, say) reads it back instead of reconstructing it.
pub const CONTROL_SOCKET_FILE: &str = "gvproxy-control";

/// Record where a machine's gvproxy control socket is, in its state dir.
pub fn record_control_socket(vdir: &Path, control_socket: &Path) {
    let f = vdir.join(CONTROL_SOCKET_FILE);
    if let Err(e) = std::fs::write(&f, control_socket.to_string_lossy().as_bytes()) {
        warn!(file = %f.display(), "could not record the gvproxy control socket: {e}");
    }
}

/// Read back [`record_control_socket`]. `None` when the machine predates it,
/// was booted with `--no-net`, or the socket has since gone away with its VM.
pub fn machine_control_socket(vdir: &Path) -> Option<PathBuf> {
    let path = std::fs::read_to_string(vdir.join(CONTROL_SOCKET_FILE)).ok()?;
    let path = PathBuf::from(path.trim());
    path.exists().then_some(path)
}

/// gvproxy's multi-tenant switch endpoint. A `POST /connect` on the control
/// socket is hijacked into a raw ethernet tap on the shared network switch, so
/// multiple VMs join the same L2 network. Frames use the "hyperkit" framing: a
/// **2-byte little-endian length** prefix, then the frame (no handshake).
const CONNECT_PATH: &str = "/connect";

/// Bridge a libkrun **vfkit** unixgram socket into a **shared** gvproxy's
/// `/connect` switch, so many machines share one L2 subnet (a global network).
///
/// libkrun speaks the vfkit protocol — one raw ethernet frame per datagram — and
/// replies go to the datagram's source address (mirroring gvproxy's own vfkit
/// handler). gvproxy's `/connect` tap speaks length-prefixed frames. This binds
/// the vfkit socket libkrun connects to, opens a `/connect` tap, and pumps frames
/// between them (translating framing) for the VM's lifetime.
///
/// Binds synchronously (so the socket exists before libkrun connects at boot);
/// the blocking pump runs on background threads that die with the process.
pub fn start_network_bridge(vfkit_path: &Path, control_socket: &Path) -> Result<()> {
    let _ = std::fs::remove_file(vfkit_path);
    let dgram = UnixDatagram::bind(vfkit_path)
        .with_context(|| format!("binding vfkit bridge socket {}", vfkit_path.display()))?;
    let control_socket = control_socket.to_path_buf();
    let vfkit_path = vfkit_path.to_path_buf();

    std::thread::spawn(move || {
        if let Err(e) = run_bridge(dgram, &control_socket) {
            warn!(socket = %vfkit_path.display(), "network bridge ended: {e:#}");
        }
    });
    Ok(())
}

fn run_bridge(dgram: UnixDatagram, control_socket: &Path) -> Result<()> {
    use std::os::unix::io::AsRawFd;

    // Open the /connect tap on the shared gvproxy (it hijacks the HTTP conn and
    // starts reading frames — no response is sent).
    let mut up = UnixStream::connect(control_socket)
        .with_context(|| format!("connecting to shared network {}", control_socket.display()))?;
    up.write_all(
        format!("POST {CONNECT_PATH} HTTP/1.1\r\nHost: gvproxy\r\nContent-Length: 0\r\n\r\n")
            .as_bytes(),
    )
    .context("opening /connect tap")?;
    up.flush().ok();

    let fd = dgram.as_raw_fd();

    // Learn libkrun's datagram source address from its first packet using a raw
    // recvfrom — libkrun binds an autobind/abstract name that has no filesystem
    // path, so we must reply with the exact sockaddr the kernel gives us (this is
    // what gvproxy's own vfkit handler does), not a std path-based send.
    let mut buf = vec![0u8; 65536];
    let mut peer: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let mut peer_len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    let n = unsafe {
        libc::recvfrom(
            fd,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            0,
            &mut peer as *mut _ as *mut libc::sockaddr,
            &mut peer_len,
        )
    };
    if n < 0 {
        return Err(std::io::Error::last_os_error()).context("first frame from libkrun");
    }
    let n = n as usize;
    // Diagnostic: what address did libkrun bind (so we can reply to it)?
    //
    // `sun_path` is `[c_char; _]`, and `c_char` is *signed* on x86_64 and
    // aarch64-darwin but *unsigned* on aarch64-linux — so this cast is doing
    // real work on most targets and is a no-op on exactly one. Clippy only ever
    // sees the target it is running on, and calls it redundant there.
    #[allow(clippy::unnecessary_cast)]
    let path_bytes: Vec<u8> = peer
        .sun_path
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    debug!(
        peer_len,
        family = peer.sun_family as i32,
        path = %String::from_utf8_lossy(&path_bytes),
        "libkrun vfkit peer address"
    );
    let first: Vec<u8> = buf[..n].to_vec();

    // gvproxy → libkrun: read [u16 LE len][frame], deliver as one datagram to the
    // learned peer sockaddr via raw sendto.
    let mut down = up.try_clone().context("cloning /connect stream")?;
    let tx_fd = fd;
    std::thread::spawn(move || {
        let mut hdr = [0u8; 2];
        let mut frame = vec![0u8; 65536];
        loop {
            if down.read_exact(&mut hdr).is_err() {
                break;
            }
            let len = u16::from_le_bytes(hdr) as usize;
            if len == 0 || len > frame.len() || down.read_exact(&mut frame[..len]).is_err() {
                break;
            }
            let sent = unsafe {
                libc::sendto(
                    tx_fd,
                    frame.as_ptr() as *const libc::c_void,
                    len,
                    0,
                    &peer as *const _ as *const libc::sockaddr,
                    peer_len,
                )
            };
            if sent < 0 {
                break;
            }
        }
    });

    // libkrun → gvproxy: each datagram is one frame; prefix with [u16 LE len].
    // A legacy "VFKT" handshake magic (if libkrun sends it first) isn't a frame.
    if !(first.len() == 4 && first == b"VFKT") {
        write_hyperkit_frame(&mut up, &first)?;
    }
    let mut b = vec![0u8; 65536];
    loop {
        let n = dgram.recv(&mut b).context("reading frame from libkrun")?;
        if n == 0 {
            break;
        }
        write_hyperkit_frame(&mut up, &b[..n])?;
    }
    Ok(())
}

/// Write one ethernet frame to a gvproxy `/connect` (hyperkit-framed) stream.
fn write_hyperkit_frame(w: &mut UnixStream, frame: &[u8]) -> Result<()> {
    let len = (frame.len() as u16).to_le_bytes();
    w.write_all(&len).context("writing frame length")?;
    w.write_all(frame).context("writing frame")?;
    Ok(())
}

/// Register a name → IP record in a network gvproxy's built-in DNS (guests use
/// the gateway `.1` as their resolver, so this needs no guest changes). Records
/// live under a zone named for the network; with a `search <network>` domain in
/// the guest, bare names (`db`) resolve to peers.
pub fn dns_add(control_socket: &Path, zone: &str, name: &str, ip: &str) -> Result<()> {
    // gvproxy matches a FQDN query (`db.devnet.`) against `.<zone>`, so the zone
    // must be a FQDN with a trailing dot (like the built-in `docker.internal.`).
    let zone = if zone.ends_with('.') {
        zone.to_string()
    } else {
        format!("{zone}.")
    };
    let body = format!(r#"{{"Name":"{zone}","Records":[{{"Name":"{name}","IP":"{ip}"}}]}}"#);
    let resp = control_post_to(control_socket, "/services/dns/add", &body)?;
    if !resp.starts_with("HTTP/1.1 200") && !resp.starts_with("HTTP/1.0 200") {
        bail!(
            "gvproxy DNS add failed: {}",
            resp.lines().next().unwrap_or("<no status>")
        );
    }
    Ok(())
}

/// Look up the DHCP-leased IP for a MAC on a network gvproxy (BSD guests DHCP
/// rather than take a static kernel IP, so we discover their IP after boot). The
/// `/leases` endpoint returns a JSON `{ "<ip>": "<mac>" }` map.
pub fn lease_ip_for_mac(control_socket: &Path, mac: &str) -> Result<Option<String>> {
    let resp = control_get(control_socket, "/leases")?;
    // Body is after the blank line; find the JSON object.
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    let map: std::collections::HashMap<String, String> =
        serde_json::from_str(body.trim()).unwrap_or_default();
    let want = mac.to_ascii_lowercase();
    Ok(map
        .into_iter()
        .find(|(_, m)| m.to_ascii_lowercase() == want)
        .map(|(ip, _)| ip))
}

/// Minimal HTTP/1.1 GET over a gvproxy unix control socket.
fn control_get(control_socket: &Path, path: &str) -> Result<String> {
    let mut stream = UnixStream::connect(control_socket).with_context(|| {
        format!(
            "connecting to gvproxy control socket {}",
            control_socket.display()
        )
    })?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: gvproxy\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .context("writing gvproxy GET")?;
    stream.shutdown(Shutdown::Write).ok();
    let mut resp = String::new();
    stream
        .read_to_string(&mut resp)
        .context("reading gvproxy response")?;
    Ok(resp)
}

/// Minimal HTTP/1.1 POST over a gvproxy unix control socket.
fn control_post_to(control_socket: &Path, path: &str, body: &str) -> Result<String> {
    {
        let mut stream = UnixStream::connect(control_socket).with_context(|| {
            format!(
                "connecting to gvproxy control socket {}",
                control_socket.display()
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

/// The four bytes gvproxy's vfkit listener expects before any frame, as its
/// "this end is a VM" handshake.
const VFKIT_MAGIC: &[u8; 4] = b"VFKT";

/// Connect a datagram socket to gvproxy's vfkit listener and hand back the
/// socket, ready to be passed to a guest process as a file descriptor.
///
/// libkrun does this internally for every other guest here (see
/// `krun::Ctx::add_net_gvproxy`). The Solo5 tender is a separate process, so
/// bsdkrun has to be the one holding the socket: the tender takes a network
/// as `--net:NAME=@<fd>` and reads and writes **raw Ethernet frames**, one per
/// `read`/`write` — which is exactly the framing gvproxy's vfkit mode uses.
/// The virtio-net header libkrun deals with is added on its side of the wire,
/// not on the socket, so nothing has to be reframed here.
///
/// `local` is where this end binds. A unix datagram socket has no reply
/// address unless it is bound, so without this gvproxy would receive the
/// guest's frames and have nowhere to send answers — the guest's DHCP request
/// goes out and nothing ever comes back.
pub fn vfkit_connect(vfkit_socket: &Path, local: &Path) -> Result<UnixDatagram> {
    // A leftover from a VM that was SIGKILLed would fail the bind.
    let _ = std::fs::remove_file(local);
    let sock = UnixDatagram::bind(local).with_context(|| {
        format!(
            "binding the guest's end of the network at {}",
            local.display()
        )
    })?;
    sock.connect(vfkit_socket).with_context(|| {
        format!(
            "connecting to gvproxy's vfkit socket {}",
            vfkit_socket.display()
        )
    })?;

    // Sized so a full-MTU frame always fits: on macOS the send buffer of a
    // unix datagram socket caps the datagram size outright rather than just
    // queueing less, so a small one silently truncates frames instead of
    // slowing them down. Best-effort — a kernel that refuses the size still
    // carries standard 1500-byte MTU traffic on its default.
    set_socket_buf(&sock, libc::SO_SNDBUF, 1 << 16);
    set_socket_buf(&sock, libc::SO_RCVBUF, 1 << 20);

    sock.send(VFKIT_MAGIC)
        .context("sending the vfkit handshake to gvproxy")?;
    Ok(sock)
}

/// Best-effort `setsockopt` for a buffer size, warning rather than failing.
fn set_socket_buf(sock: &UnixDatagram, opt: libc::c_int, size: libc::c_int) {
    use std::os::fd::AsRawFd;
    let rc = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            opt,
            &size as *const _ as *const libc::c_void,
            std::mem::size_of_val(&size) as libc::socklen_t,
        )
    };
    if rc != 0 {
        debug!(
            option = opt,
            size,
            error = %std::io::Error::last_os_error(),
            "could not resize the network socket buffer"
        );
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
