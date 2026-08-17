//! Docker Desktop, replaced: a `docker:dind` microVM whose API surfaces on the
//! host as a plain unix socket, so the *host's* `docker` CLI drives it.
//!
//! Three pieces make the illusion complete, and each exists because the
//! alternative breaks a `docker` command someone will type on day one:
//!
//! 1. **The socket.** dind listens on `tcp://0.0.0.0:2375` inside the guest
//!    (which is what an empty `DOCKER_TLS_CERTDIR` configures), gvproxy
//!    forwards a loopback host port to it, and a small proxy here serves a
//!    real unix socket at `<state>/docker/docker.sock`. `docker` speaks HTTP
//!    over that socket and never learns there is a VM. A `docker context`
//!    points at it, so `docker ps` works with no environment variables.
//! 2. **Published ports.** `docker run -p 8080:80` publishes on the *guest's*
//!    interface, which is inside the VM and reachable by nobody. The
//!    [`serve`] loop watches the Docker event stream and mirrors every
//!    published port onto the host through the machine's gvproxy — the same
//!    control socket `--port` uses, driven at runtime instead of at boot.
//! 3. **Bind mounts.** `-v $PWD:/app` resolves inside the guest, so the host
//!    directories a user expects to mount are shared into the VM over
//!    virtio-fs **at the same path**. Docker Desktop shares `/Users`; the
//!    default here is `$HOME`, and `--mount` adds more.
//!
//! What this deliberately is not: a TLS-protected daemon. The forwarded API
//! port is loopback-only, but any local process can reach it, and Docker API
//! access is root-equivalent inside the guest. That is the same trade colima
//! and friends make; a socket relayed through the guest agent would close it,
//! and is the obvious next step if it ever matters.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{db, net};

/// The machine name the Docker VM always has. Fixed, because every command
/// here finds it by name and a user should be able to `bsdkrun logs
/// bsdkrun-docker` without looking anything up.
pub const MACHINE_NAME: &str = "bsdkrun-docker";

/// The named volume holding `/var/lib/docker` (images, containers, volumes).
/// A bsdkrun volume *is* the machine's rootfs, so this is what makes pulled
/// images survive `bsdkrun docker stop`.
pub const VOLUME: &str = "bsdkrun-docker";

/// The OCI image the VM boots.
pub const IMAGE: &str = "docker:dind";

/// dockerd's plaintext API port inside the guest. dind's entrypoint listens
/// here (rather than on TLS 2376) exactly when `DOCKER_TLS_CERTDIR` is empty.
pub const GUEST_API_PORT: u16 = 2375;

/// The `docker context` bsdkrun creates and points at its socket.
pub const CONTEXT: &str = "bsdkrun";

/// The well-known path Docker Desktop owns, claimed only with `--system-socket`.
pub const SYSTEM_SOCKET: &str = "/var/run/docker.sock";

/// Settings keys: the proxy's pid, and what it was started with.
const PID_KEY: &str = "docker.proxy_pid";
const PORT_KEY: &str = "docker.api_port";
const BIND_KEY: &str = "docker.publish_bind";

/// `<state>/docker` — the socket, the proxy's log.
pub fn dir() -> Result<PathBuf> {
    let dir = db::state_dir()?.join("docker");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// The unix socket the host's `docker` CLI talks to.
pub fn socket_path() -> Result<PathBuf> {
    Ok(dir()?.join("docker.sock"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(dir()?.join("proxy.log"))
}

/// The dedicated image store, when one was asked for: `<state>/docker/data.img`.
///
/// Without it the guest's `/var/lib/docker` is the machine's virtio-fs rootfs,
/// which is backed by the host filesystem and therefore has no size of its own
/// — the usual case, and unbounded. `--disk-size` swaps in a real ext4 volume
/// instead: a hard cap, a filesystem Docker's overlay driver is built for, and
/// something `bsdkrun docker disk --size` can grow.
pub fn data_disk() -> Result<PathBuf> {
    Ok(dir()?.join("data.img"))
}

/// The guest path the data disk is mounted at — Docker's whole storage tree.
pub const DATA_MOUNT: &str = "/var/lib/docker";

/// Create the data disk if it is missing, or grow it to `size`. Sparse, so a
/// 100 GiB disk costs nothing until it is filled.
pub fn ensure_data_disk(size: &str) -> Result<PathBuf> {
    let path = data_disk()?;
    if !path.exists() {
        std::fs::File::create(&path)
            .with_context(|| format!("creating the Docker data disk {}", path.display()))?;
    }
    // `grow` refuses to shrink, which is the behaviour we want here too: a
    // smaller number would cut a live filesystem in half.
    match crate::fetch::grow(&path, size) {
        Ok(()) => {}
        Err(e) if path.metadata().map(|m| m.len()).unwrap_or(0) > 0 => {
            // Already at least this big — not an error for `start`.
            tracing::debug!("Docker data disk not grown: {e:#}");
        }
        Err(e) => return Err(e),
    }
    Ok(path)
}

/// Whether the engine is up, and therefore whether a disk that has just been
/// grown still shows its old size to the guest.
///
/// virtio-blk fixes a device's size when the VM attaches it: growing the image
/// file underneath a running guest changes nothing it can see, and `resize2fs`
/// inside it would resize to the *old* device size. The growth lands on the
/// next boot, where the generated init runs `resize2fs` for exactly this
/// reason — so this reports rather than fixes.
pub fn engine_running() -> bool {
    machine()
        .ok()
        .flatten()
        .map(|vm| vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false))
        .unwrap_or(false)
}

/// The data disk's current size in bytes, if there is one.
pub fn data_disk_size() -> Option<u64> {
    data_disk().ok()?.metadata().ok().map(|m| m.len())
}

/// Where a published container port is bound on the host.
///
/// `Mirror` reproduces what the container asked for — `-p 8080:80` binds all
/// interfaces, `-p 127.0.0.1:8080:80` binds loopback — which is what Docker
/// Desktop does and therefore what a `docker run` line copied from a README
/// expects. A fixed address overrides that for anyone who would rather not
/// have containers reach the LAN by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishBind {
    Mirror,
    Fixed(IpAddr),
}

impl PublishBind {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "" | "mirror" => Ok(PublishBind::Mirror),
            other => Ok(PublishBind::Fixed(other.parse().with_context(|| {
                format!("--publish-bind {other:?} is not an IP address (or \"mirror\")")
            })?)),
        }
    }

    fn resolve(self, container_ip: &str) -> IpAddr {
        match self {
            PublishBind::Fixed(ip) => ip,
            // "" and "0.0.0.0" both mean "every interface" in Docker's port
            // list; an IPv6 "::" entry duplicates its IPv4 twin, and gvproxy
            // forwards v4, so it collapses to the same thing.
            PublishBind::Mirror => match container_ip {
                "" | "0.0.0.0" | "::" => Ipv4Addr::UNSPECIFIED.into(),
                ip => ip.parse().unwrap_or(Ipv4Addr::LOCALHOST.into()),
            },
        }
    }
}

impl std::fmt::Display for PublishBind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PublishBind::Mirror => write!(f, "mirror"),
            PublishBind::Fixed(ip) => write!(f, "{ip}"),
        }
    }
}

/// What `bsdkrun docker status` reports, and what the UIs render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    /// The VM is up *and* its API answered a ping.
    pub running: bool,
    pub machine_id: Option<String>,
    /// Whether the machine exists at all (it may be stopped).
    pub machine_running: bool,
    pub socket: String,
    /// The socket exists and the proxy behind it is alive.
    pub socket_ready: bool,
    /// Host port forwarded to the guest's dockerd.
    pub api_port: Option<u16>,
    pub proxy_pid: Option<i64>,
    /// Server version, when the API answered.
    pub version: Option<String>,
    pub containers: Option<i64>,
    pub images: Option<i64>,
    /// Host directories shared into the VM, each `HOST:GUEST`.
    pub mounts: Vec<String>,
    /// `docker context` exists / is the active one.
    pub context: bool,
    pub context_active: bool,
    /// `/var/run/docker.sock` points at our socket.
    pub system_socket: bool,
    /// The dedicated image-store disk, when the VM was started with one.
    pub disk: Option<String>,
    /// Its size in bytes. Sparse: this is the cap, not the usage.
    pub disk_size: Option<u64>,
}

/// A container, as the UIs and SDKs see it — a trimmed `docker ps` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    /// The first name Docker reports, without its leading slash.
    pub name: String,
    pub image: String,
    pub command: String,
    /// "running" | "exited" | "created" | "paused" | …
    pub state: String,
    /// Docker's human status, e.g. "Up 3 minutes".
    pub status: String,
    /// Published forwards, each `HOST:GUEST/proto` — the ones bsdkrun mirrors
    /// onto the host.
    pub ports: Vec<String>,
    /// Unix epoch seconds.
    pub created: i64,
}

// ---------------------------------------------------------------------------
// the machine
// ---------------------------------------------------------------------------

/// The Docker VM's DB row, if it has ever been created.
pub fn machine() -> Result<Option<db::MachineRow>> {
    let db = db::Db::open()?;
    Ok(db.find_machine(MACHINE_NAME).ok())
}

/// The host port forwarded to the guest's dockerd, as recorded on the machine.
pub fn api_port(vm: &db::MachineRow) -> Option<u16> {
    vm.ports
        .as_deref()
        .map(net::parse_ports)
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.guest == GUEST_API_PORT)
        .map(|p| p.host)
}

/// Host directories shared into the VM (`HOST:GUEST` pairs).
///
/// Read from the same record `bsdkrun start` re-applies them from, so `status`
/// can never disagree with what the guest actually has mounted.
pub fn mounts(vm: &db::MachineRow) -> Vec<String> {
    crate::commands::machine_mounts(&PathBuf::from(&vm.state_dir))
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub fn status() -> Result<Status> {
    let db = db::Db::open()?;
    let vm = db.find_machine(MACHINE_NAME).ok();
    let socket = socket_path()?;
    let machine_running = vm
        .as_ref()
        .map(|m| m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false))
        .unwrap_or(false);
    let port = vm.as_ref().and_then(api_port);
    let proxy_pid = db
        .get_setting(PID_KEY)
        .ok()
        .flatten()
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| db::pid_alive(*p));

    // Ask the daemon itself rather than trusting the pid: a VM that is up with
    // a dockerd that died is "not running" for every purpose a caller has.
    let info = port.and_then(|p| api_get(p, "/info").ok());
    let (version, containers, images) = match info.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(v)) => (
            v.get("ServerVersion")
                .and_then(Value::as_str)
                .map(str::to_string),
            v.get("Containers").and_then(Value::as_i64),
            v.get("Images").and_then(Value::as_i64),
        ),
        _ => (None, None, None),
    };

    Ok(Status {
        running: machine_running && version.is_some(),
        machine_id: vm.as_ref().map(|m| m.id.clone()),
        machine_running,
        socket_ready: socket.exists() && proxy_pid.is_some(),
        socket: socket.to_string_lossy().into_owned(),
        api_port: port,
        proxy_pid,
        version,
        containers,
        images,
        mounts: vm.as_ref().map(mounts).unwrap_or_default(),
        context: context_exists(),
        context_active: context_active(),
        system_socket: system_socket_ours(&socket),
        disk: data_disk()
            .ok()
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned()),
        disk_size: data_disk_size(),
    })
}

/// Whether `/var/run/docker.sock` currently points at our socket.
fn system_socket_ours(socket: &Path) -> bool {
    std::fs::read_link(SYSTEM_SOCKET)
        .map(|target| target == socket)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// the Docker API, over the forwarded loopback port
// ---------------------------------------------------------------------------

use serde_json::Value;

fn api_addr(port: u16) -> SocketAddr {
    SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)
}

/// One `GET`, returning the response body.
///
/// `Connection: close` keeps the reply framing trivial: read to EOF, then
/// de-chunk if the daemon chose chunked anyway. Docker's API is HTTP/1.1 on a
/// loopback socket, so a hand-rolled request beats pulling in a client — the
/// same call this codebase already makes for gvproxy's control socket.
fn api_get(port: u16, path: &str) -> Result<String> {
    api_request(port, "GET", path, None)
}

fn api_post(port: u16, path: &str) -> Result<String> {
    api_request(port, "POST", path, None)
}

fn api_delete(port: u16, path: &str) -> Result<String> {
    api_request(port, "DELETE", path, None)
}

fn api_request(port: u16, method: &str, path: &str, body: Option<&str>) -> Result<String> {
    let mut stream = TcpStream::connect_timeout(&api_addr(port), Duration::from_secs(5))
        .with_context(|| format!("connecting to the Docker API on 127.0.0.1:{port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: docker\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("the Docker API sent a truncated response"))?;
    let status = head.lines().next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or("");
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_string()
    };
    if !code.starts_with('2') {
        anyhow::bail!("the Docker API answered {status}: {}", body.trim());
    }
    Ok(body)
}

/// Strip HTTP chunked framing: alternating `<hex length>\r\n<data>\r\n`.
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size_line, tail)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        if size == 0 || tail.len() < size {
            break;
        }
        out.push_str(&tail[..size]);
        rest = tail[size..].strip_prefix("\r\n").unwrap_or("");
    }
    out
}

/// Wait until dockerd answers `/_ping`. dind untars its storage and starts the
/// daemon after the VM is up, so this is seconds, not milliseconds.
pub fn wait_for_api(port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut last: Option<String> = None;
    while Instant::now() < deadline {
        match api_get(port, "/_ping") {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(format!("{e:#}")),
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    anyhow::bail!(
        "dockerd did not answer on 127.0.0.1:{port} within {timeout:?}{}",
        last.map(|e| format!(" (last error: {e})"))
            .unwrap_or_default()
    )
}

// ---------------------------------------------------------------------------
// containers
// ---------------------------------------------------------------------------

/// `docker ps` as data. `all` includes stopped containers.
pub fn containers(all: bool) -> Result<Vec<Container>> {
    let port = require_port()?;
    let path = if all {
        "/containers/json?all=1"
    } else {
        "/containers/json"
    };
    let rows: Vec<Value> =
        serde_json::from_str(&api_get(port, path)?).context("parsing the Docker container list")?;
    Ok(rows.iter().map(container_from_json).collect())
}

fn container_from_json(c: &Value) -> Container {
    let str_of = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let name = c
        .get("Names")
        .and_then(Value::as_array)
        .and_then(|n| n.first())
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string();
    Container {
        id: str_of("Id").chars().take(12).collect(),
        name,
        image: str_of("Image"),
        command: str_of("Command"),
        state: str_of("State"),
        status: str_of("Status"),
        // Deduplicated: Docker lists a published port once per address family,
        // and "18081:80/tcp, 18081:80/tcp" is noise, not information.
        ports: published(c)
            .into_iter()
            .map(|(_, host, guest, proto)| format!("{host}:{guest}/{proto}"))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        created: c.get("Created").and_then(Value::as_i64).unwrap_or(0),
    }
}

/// Every published port of one container row: `(bind ip, host, guest, proto)`.
///
/// Unpublished ports (no `PublicPort`) are the container's own business, and
/// gvproxy forwards TCP only — a UDP entry would be silently unforwarded, so
/// it is skipped rather than half-handled.
fn published(c: &Value) -> Vec<(String, u16, u16, String)> {
    let Some(ports) = c.get("Ports").and_then(Value::as_array) else {
        return vec![];
    };
    ports
        .iter()
        .filter_map(|p| {
            let proto = p.get("Type").and_then(Value::as_str).unwrap_or("tcp");
            if proto != "tcp" {
                return None;
            }
            let host = p.get("PublicPort").and_then(Value::as_u64)? as u16;
            let guest = p.get("PrivatePort").and_then(Value::as_u64).unwrap_or(0) as u16;
            let ip = p
                .get("IP")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some((ip, host, guest, proto.to_string()))
        })
        .collect()
}

/// Start / stop / restart / kill / remove one container, by id or name.
pub fn container_action(id: &str, action: &str) -> Result<String> {
    let port = require_port()?;
    let id = urlencode(id);
    match action {
        "start" | "stop" | "restart" | "kill" | "pause" | "unpause" => {
            api_post(port, &format!("/containers/{id}/{action}"))?;
        }
        "rm" | "remove" => {
            api_delete(port, &format!("/containers/{id}?force=1&v=1"))?;
        }
        other => anyhow::bail!(
            "unknown container action {other:?} \
             (start|stop|restart|kill|pause|unpause|rm)"
        ),
    }
    Ok(id)
}

/// A container's logs, as one string (stdout+stderr, most recent `tail` lines).
pub fn container_logs(id: &str, tail: u32) -> Result<String> {
    let port = require_port()?;
    let raw = api_get(
        port,
        &format!(
            "/containers/{}/logs?stdout=1&stderr=1&tail={tail}",
            urlencode(id)
        ),
    )?;
    Ok(demux_logs(&raw))
}

/// Docker frames non-TTY logs as 8-byte headers + payload. A TTY container's
/// logs are raw, and the header bytes are not valid UTF-8 text, so this strips
/// frames when it sees them and passes anything else through untouched.
fn demux_logs(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i + 8 <= bytes.len() {
        let stream = bytes[i];
        // A frame header starts with the stream number (0/1/2) and three zero
        // bytes; anything else means this is not framed output.
        if stream > 2 || bytes[i + 1] != 0 || bytes[i + 2] != 0 || bytes[i + 3] != 0 {
            return raw.to_string();
        }
        let len =
            u32::from_be_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        let start = i + 8;
        let end = (start + len).min(bytes.len());
        out.push_str(&String::from_utf8_lossy(&bytes[start..end]));
        i = end;
    }
    if out.is_empty() {
        raw.to_string()
    } else {
        out
    }
}

/// Percent-encode the few characters a container name or id can carry that
/// would otherwise break the request line.
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

fn require_port() -> Result<u16> {
    let vm = machine()?.ok_or_else(|| {
        anyhow::anyhow!("no Docker VM yet — start one with `bsdkrun docker start`")
    })?;
    api_port(&vm).ok_or_else(|| {
        anyhow::anyhow!("the Docker VM has no API port forwarded — `bsdkrun docker start` again")
    })
}

// ---------------------------------------------------------------------------
// the host-side proxy: unix socket -> forwarded API port, and port publishing
// ---------------------------------------------------------------------------

/// Spawn [`serve`] as a detached daemon, recording its pid.
///
/// Same shape as the domains DNS responder: `setsid`, stderr to a log, pid in
/// the settings table so the next invocation can see whether it is still alive.
pub fn spawn_proxy(port: u16, machine_id: &str, bind: PublishBind) -> Result<u32> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("resolving the bsdkrun binary path")?;
    let log_path = log_path()?;
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("creating {}", log_path.display()))?;
    let mut cmd = Command::new(exe);
    cmd.arg("docker")
        .arg("__serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--machine")
        .arg(machine_id)
        .arg("--publish-bind")
        .arg(bind.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("spawning the Docker socket proxy")?;
    let pid = child.id();
    std::mem::forget(child); // a daemon tracked by pid — don't reap on drop

    let socket = socket_path()?;
    for _ in 0..50 {
        if socket.exists() {
            let db = db::Db::open()?;
            db.set_setting(PID_KEY, &pid.to_string())?;
            db.set_setting(PORT_KEY, &port.to_string())?;
            db.set_setting(BIND_KEY, &bind.to_string())?;
            return Ok(pid);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!(
        "the Docker socket proxy did not create {} (see {})",
        socket.display(),
        log_path.display()
    )
}

/// Stop the proxy and remove its socket. Safe to call when nothing is running.
pub fn stop_proxy() -> Result<()> {
    let db = db::Db::open()?;
    if let Some(pid) = db
        .get_setting(PID_KEY)?
        .and_then(|p| p.parse::<i32>().ok())
        .filter(|p| db::pid_alive(*p as i64))
    {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    db.remove_setting(PID_KEY).ok();
    let _ = std::fs::remove_file(socket_path()?);
    Ok(())
}

/// Respawn the proxy if its pid is gone but the VM is up — lazy supervision,
/// mirroring `domains::dns::ensure_running`.
pub fn ensure_proxy() -> Result<()> {
    let db = db::Db::open()?;
    let alive = db
        .get_setting(PID_KEY)?
        .and_then(|p| p.parse::<i64>().ok())
        .map(db::pid_alive)
        .unwrap_or(false);
    if alive && socket_path()?.exists() {
        return Ok(());
    }
    let Some(vm) = machine()? else { return Ok(()) };
    if vm.status != "running" || !vm.pid.map(db::pid_alive).unwrap_or(false) {
        return Ok(());
    }
    let Some(port) = api_port(&vm) else {
        return Ok(());
    };
    let bind = db
        .get_setting(BIND_KEY)?
        .and_then(|b| PublishBind::parse(&b).ok())
        .unwrap_or(PublishBind::Mirror);
    spawn_proxy(port, &vm.id, bind)?;
    warn!("restarted the Docker socket proxy");
    Ok(())
}

/// The detached process: serve the unix socket, and keep published container
/// ports mirrored onto the host. Never returns except on a bind error.
pub fn serve(port: u16, machine_id: &str, bind: PublishBind) -> Result<()> {
    let socket = socket_path()?;
    // A socket left behind by a killed proxy would make bind fail with
    // EADDRINUSE even though nothing is listening.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("binding the Docker socket {}", socket.display()))?;
    // Docker API access is root-equivalent; keep the socket to this user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
    }
    info!(socket = %socket.display(), api_port = port, "serving the Docker API");

    {
        let machine_id = machine_id.to_string();
        std::thread::spawn(move || publisher(port, &machine_id, bind));
    }

    for stream in listener.incoming() {
        match stream {
            Ok(client) => {
                std::thread::spawn(move || {
                    if let Err(e) = pipe(client, port) {
                        warn!("Docker API connection ended: {e:#}");
                    }
                });
            }
            Err(e) => warn!("accept on the Docker socket failed: {e}"),
        }
    }
    Ok(())
}

/// Splice one client connection to the guest's dockerd, both directions.
///
/// A byte pipe, not an HTTP proxy, deliberately: `docker exec -it` and
/// `docker attach` hijack the connection and speak a raw stream afterwards,
/// which anything parsing requests would have to hand back anyway.
fn pipe(client: UnixStream, port: u16) -> Result<()> {
    let upstream = TcpStream::connect_timeout(&api_addr(port), Duration::from_secs(10))
        .with_context(|| format!("connecting to dockerd on 127.0.0.1:{port}"))?;
    let (mut cr, mut cw) = (client.try_clone()?, client);
    let (mut ur, mut uw) = (upstream.try_clone()?, upstream);
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut cr, &mut uw);
        let _ = uw.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut ur, &mut cw);
    let _ = cw.shutdown(std::net::Shutdown::Write);
    let _ = up.join();
    Ok(())
}

/// Keep the host's forwards equal to the set of ports the guest's containers
/// publish: react to the Docker event stream, and re-check on a timer in case
/// the stream drops.
///
/// The stream lives in its own thread and only ever *pokes* this one. Reading
/// events and reconciling in one loop loses every event that lands while the
/// reconcile is in flight — which is exactly when they land, since a `docker
/// run -p` emits `create` and `start` microseconds apart — and the port then
/// waited for the next timeout to appear. A channel holds that poke instead.
fn publisher(port: u16, machine_id: &str, bind: PublishBind) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || event_pump(port, tx));

    let mut exposed: BTreeSet<(IpAddr, u16)> = BTreeSet::new();
    loop {
        if let Err(e) = reconcile(port, machine_id, bind, &mut exposed) {
            warn!("could not sync published ports: {e:#}");
        }
        // Wake on the next event, or re-check on a timer regardless: a
        // reconcile is one local HTTP call, and a periodic one is what makes a
        // dropped event stream a delay rather than a silently missing port.
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Collapse a burst (create + start + …) into one reconcile.
                while rx.try_recv().is_ok() {}
            }
            // The pump thread is gone; without it this loop would spin.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                warn!("the Docker event watcher stopped; falling back to polling");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

/// Read the Docker event stream forever, poking `tx` on every container event.
///
/// Only the *fact* of an event matters — [`reconcile`] then reads the
/// authoritative container list — so this deliberately does not parse the
/// stream beyond spotting an event's `"status":` key.
fn event_pump(port: u16, tx: std::sync::mpsc::Sender<()>) {
    loop {
        match stream_events(port, &tx) {
            Ok(()) => {} // the daemon closed the stream; reconnect
            Err(e) => warn!("Docker event stream ended: {e:#}"),
        }
        // dockerd restarts (a `docker system prune`, a daemon reload) show up
        // here as a closed stream; back off rather than hammering it.
        std::thread::sleep(Duration::from_secs(2));
        if tx.send(()).is_err() {
            return; // the publisher is gone
        }
    }
}

fn stream_events(port: u16, tx: &std::sync::mpsc::Sender<()>) -> Result<()> {
    let mut stream = TcpStream::connect_timeout(&api_addr(port), Duration::from_secs(5))
        .with_context(|| format!("connecting to the Docker API on 127.0.0.1:{port}"))?;
    let filters = r#"{"type":["container"]}"#;
    let req = format!(
        "GET /events?filters={} HTTP/1.1\r\nHost: docker\r\nAccept: application/json\r\n\r\n",
        urlencode(filters)
    );
    stream.write_all(req.as_bytes())?;
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(()); // EOF: the daemon closed the stream
        }
        if String::from_utf8_lossy(&buf[..n]).contains("\"status\":") && tx.send(()).is_err() {
            return Ok(()); // nobody left to tell
        }
    }
}

/// Diff what containers publish against what gvproxy is forwarding, and fix
/// the difference. Doing it as a full reconcile rather than per event means a
/// missed or duplicated event cannot leave a stale forward behind.
fn reconcile(
    port: u16,
    machine_id: &str,
    bind: PublishBind,
    exposed: &mut BTreeSet<(IpAddr, u16)>,
) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine_id)?;
    let vdir = PathBuf::from(&vm.state_dir);
    let Some(control) = net::machine_control_socket(&vdir) else {
        anyhow::bail!("the Docker VM's gvproxy control socket is gone — is it still running?");
    };
    let guest_ip = vm
        .net_ip
        .clone()
        .filter(|ip| !ip.is_empty())
        .unwrap_or_else(|| net::GUEST_IP.to_string());

    let rows: Vec<Value> = serde_json::from_str(&api_get(port, "/containers/json")?)
        .context("parsing the Docker container list")?;
    let mut want: BTreeSet<(IpAddr, u16)> = BTreeSet::new();
    let mut guest_of = std::collections::BTreeMap::new();
    for c in &rows {
        for (ip, host, guest, _) in published(c) {
            want.insert((bind.resolve(&ip), host));
            guest_of.insert(host, guest);
        }
    }

    for (ip, host) in want.difference(exposed) {
        // The container publishes on `host` *inside* the guest, so the host
        // side and the guest side of the forward are the same number — what
        // changes is which machine you can reach it from.
        match net::expose_on_control(&control, *ip, *host, &guest_ip, *host) {
            Ok(()) => info!(
                host = host,
                container = guest_of.get(host).copied().unwrap_or(0),
                "published a container port on the host"
            ),
            Err(e) => warn!(host = host, "could not publish a container port: {e:#}"),
        }
    }
    for (ip, host) in exposed.difference(&want) {
        // A forward for a container that is gone; gvproxy answers 500 if it
        // was never there, which is not worth reporting.
        let _ = net::unexpose_on_control(&control, *ip, *host);
        info!(host = host, "withdrew a published container port");
    }
    *exposed = want;
    Ok(())
}

// ---------------------------------------------------------------------------
// docker context
// ---------------------------------------------------------------------------

/// Whether a `docker` CLI is on the host at all. Without one, bsdkrun still
/// serves the socket — the user may be driving it from an SDK.
pub fn docker_cli() -> Option<PathBuf> {
    which_in_path("docker")
}

fn which_in_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

fn docker_cmd(args: &[&str]) -> Option<std::process::Output> {
    let bin = docker_cli()?;
    Command::new(bin).args(args).output().ok()
}

fn context_exists() -> bool {
    docker_cmd(&["context", "inspect", CONTEXT])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn context_active() -> bool {
    docker_cmd(&["context", "show"])
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == CONTEXT)
        .unwrap_or(false)
}

/// Create (or re-point) the `bsdkrun` docker context, and optionally select
/// it. Returns what happened, for the caller to print.
pub fn setup_context(socket: &Path, activate: bool) -> Result<Option<String>> {
    if docker_cli().is_none() {
        return Ok(None);
    }
    let host = format!("host=unix://{}", socket.display());
    let existing = context_exists();
    let out = if existing {
        docker_cmd(&["context", "update", CONTEXT, "--docker", &host])
    } else {
        docker_cmd(&[
            "context",
            "create",
            CONTEXT,
            "--docker",
            &host,
            "--description",
            "bsdkrun microVM Docker engine",
        ])
    };
    if let Some(o) = out {
        if !o.status.success() {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            warn!("could not configure the docker context: {err}");
            return Ok(None);
        }
    }
    if activate {
        docker_cmd(&["context", "use", CONTEXT]);
    }
    Ok(Some(CONTEXT.to_string()))
}

/// Point the CLI back at whatever it used before, and drop our context.
pub fn remove_context() {
    if context_active() {
        docker_cmd(&["context", "use", "default"]);
    }
    docker_cmd(&["context", "rm", "-f", CONTEXT]);
}

/// Symlink `/var/run/docker.sock` at our socket, asking for sudo once.
///
/// Opt-in, because it hijacks a path another runtime may own — and because a
/// tool that reads `DOCKER_HOST` or the active context (nearly all of them)
/// does not need it. Testcontainers and friends do.
pub fn claim_system_socket(socket: &Path) -> Result<()> {
    if system_socket_ours(socket) {
        return Ok(());
    }
    let script = format!(
        "rm -f {sock} && ln -s {ours} {sock}",
        sock = SYSTEM_SOCKET,
        ours = socket.display()
    );
    let status = Command::new("sudo")
        .args(["sh", "-c", &script])
        .status()
        .context("running sudo to claim /var/run/docker.sock")?;
    if !status.success() {
        anyhow::bail!("could not link {SYSTEM_SOCKET} (sudo failed)");
    }
    Ok(())
}

/// Drop the `/var/run/docker.sock` symlink, if it is ours.
pub fn release_system_socket(socket: &Path) {
    if !system_socket_ours(socket) {
        return;
    }
    let _ = Command::new("sudo")
        .args(["rm", "-f", SYSTEM_SOCKET])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_bind_mirrors_the_container_and_honours_an_override() {
        let mirror = PublishBind::Mirror;
        assert_eq!(
            mirror.resolve("0.0.0.0"),
            IpAddr::from(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(mirror.resolve(""), IpAddr::from(Ipv4Addr::UNSPECIFIED));
        assert_eq!(mirror.resolve("::"), IpAddr::from(Ipv4Addr::UNSPECIFIED));
        assert_eq!(
            mirror.resolve("127.0.0.1"),
            IpAddr::from(Ipv4Addr::LOCALHOST)
        );

        let fixed = PublishBind::parse("127.0.0.1").unwrap();
        assert_eq!(fixed.resolve("0.0.0.0"), IpAddr::from(Ipv4Addr::LOCALHOST));
        assert!(PublishBind::parse("not-an-ip").is_err());
    }

    #[test]
    fn published_skips_udp_and_unpublished_ports() {
        let c: Value = serde_json::from_str(
            r#"{"Ports":[
                 {"IP":"0.0.0.0","PrivatePort":80,"PublicPort":8080,"Type":"tcp"},
                 {"PrivatePort":9000,"Type":"tcp"},
                 {"IP":"0.0.0.0","PrivatePort":53,"PublicPort":5353,"Type":"udp"}]}"#,
        )
        .unwrap();
        let ports = published(&c);
        assert_eq!(ports.len(), 1);
        assert_eq!(
            ports[0],
            ("0.0.0.0".to_string(), 8080, 80, "tcp".to_string())
        );
    }

    #[test]
    fn dechunk_reassembles_a_chunked_body() {
        assert_eq!(dechunk("4\r\n[{}]\r\n0\r\n\r\n"), "[{}]");
        assert_eq!(dechunk("2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n"), "abcd");
    }

    #[test]
    fn demux_strips_docker_log_frames_but_leaves_tty_output() {
        let mut framed = vec![1u8, 0, 0, 0, 0, 0, 0, 5];
        framed.extend_from_slice(b"hello");
        let framed = String::from_utf8_lossy(&framed).into_owned();
        assert_eq!(demux_logs(&framed), "hello");
        assert_eq!(demux_logs("plain tty output\n"), "plain tty output\n");
    }
}
