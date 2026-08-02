//! Tailscale management, built into the agent so every guest OS gets the same
//! four verbs without shipping scripts into images:
//!
//!   bsdkrun-agent tailscale install            # OS-native install
//!   bsdkrun-agent tailscale start              # start tailscaled (detached)
//!   bsdkrun-agent tailscale status [args..]    # tailscale status passthrough
//!   bsdkrun-agent tailscale setup --authkey K  # install + start + tailscale up
//!
//! Run via the host: `bsdkrun exec <id> /path/to/bsdkrun-agent tailscale ...`.
//!
//! Per-OS install paths (all verified reachable):
//! - **Linux**: `apk add tailscale` when apk exists (Alpine — the common OCI
//!   guest); otherwise the official static tarball
//!   `https://pkgs.tailscale.com/stable/tailscale_latest_<arch>.tgz` via
//!   curl/wget into /usr/local/bin (static Go binaries, run on any distro).
//! - **FreeBSD**: `pkg install -y tailscale` (ASSUME_ALWAYS_YES also
//!   bootstraps pkg itself on first use).
//! - **NetBSD**: `pkg_add tailscale` with PKG_PATH on the pkgsrc CDN. Plain
//!   http, deliberately: base pkg_add's libfetch has no TLS. The CDN redirects
//!   `<release>` to the current quarterly dir and pkg_add resolves the
//!   versioned filename, so nothing is hardcoded.
//!
//! tailscaled runs with `--tun=userspace-networking` unless a kernel TUN is
//! detectable: the microVM kernels here (Linux microvm, NetBSD MICROVM,
//! FreeBSD FIRECRACKER) usually lack tun/tap, and userspace mode still gives
//! the killer feature — reaching the guest (ssh etc.) over the tailnet.
//! Force the kernel path with `--kernel-tun` on `start`/`setup`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const STATE_DIR: &str = "/var/lib/tailscale";
const RUN_DIR: &str = "/var/run/tailscale";
const SOCKET: &str = "/var/run/tailscale/tailscaled.sock";
const LOG_FILE: &str = "/var/log/tailscaled.log";

/// Dirs searched for the binaries beyond $PATH (pkg installs to /usr/local on
/// FreeBSD, /usr/pkg on NetBSD; the static tarball goes to /usr/local/bin).
const EXTRA_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/pkg/bin",
    "/usr/pkg/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/bin",
    "/sbin",
];

/// Entry point: `args` are everything after the `tailscale` word. Returns the
/// process exit code.
pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("install") => install(),
        Some("start") => start(args[1..].contains_flag("--kernel-tun")),
        Some("status") => status(&args[1..]),
        Some("setup") => setup(&args[1..]),
        _ => {
            eprintln!(
                "usage: bsdkrun-agent tailscale <install|start|status|setup>\n\
                 \n\
                 install                          install tailscale (OS-native)\n\
                 start [--kernel-tun]             start tailscaled (userspace networking by default)\n\
                 status [tailscale-status-args]   show tailscale status\n\
                 setup [--authkey K] [--hostname H] [--kernel-tun] [extra `tailscale up` args]\n\
                 \0                                 install + start + tailscale up"
            );
            2
        }
    }
}

trait ContainsFlag {
    fn contains_flag(&self, f: &str) -> bool;
}
impl ContainsFlag for [String] {
    fn contains_flag(&self, f: &str) -> bool {
        self.iter().any(|a| a == f)
    }
}

// --- install -----------------------------------------------------------------

fn install() -> i32 {
    if let Some(p) = find_bin("tailscaled") {
        println!("tailscale already installed: {}", p.display());
        return 0;
    }
    let code = install_os();
    if code != 0 {
        return code;
    }
    match find_bin("tailscaled") {
        Some(p) => {
            println!("tailscale installed: {}", p.display());
            0
        }
        None => {
            eprintln!("install ran but tailscaled not found afterwards");
            1
        }
    }
}

#[cfg(target_os = "linux")]
fn install_os() -> i32 {
    // Alpine (the usual OCI guest): community repo has tailscale.
    if find_bin("apk").is_some() {
        return sh("apk add --no-cache tailscale");
    }
    // Anything else: the official static tarball (static Go binaries — no libc
    // or distro dependency). Needs curl or wget in the guest.
    let arch = match uname_m().as_str() {
        "x86_64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => {
            eprintln!("unsupported Linux arch for the static tarball: {other}");
            return 1;
        }
    };
    let url = format!("https://pkgs.tailscale.com/stable/tailscale_latest_{arch}.tgz");
    let dl = if find_bin("curl").is_some() {
        format!("curl -fsSL {url}")
    } else if find_bin("wget").is_some() {
        format!("wget -qO- {url}")
    } else {
        eprintln!("neither apk, curl nor wget available — cannot download tailscale");
        return 1;
    };
    sh(&format!(
        "set -e; mkdir -p /usr/local/bin /tmp/ts.$$; cd /tmp/ts.$$; \
         {dl} | tar -xz; \
         cp tailscale_*/tailscale tailscale_*/tailscaled /usr/local/bin/; \
         cd /; rm -rf /tmp/ts.$$"
    ))
}

#[cfg(target_os = "freebsd")]
fn install_os() -> i32 {
    // ASSUME_ALWAYS_YES both answers the install prompt and lets `pkg`
    // bootstrap itself on a fresh system.
    run_cmd(
        Command::new("pkg")
            .args(["install", "-y", "tailscale"])
            .env("ASSUME_ALWAYS_YES", "YES"),
    )
}

#[cfg(target_os = "netbsd")]
fn install_os() -> i32 {
    // MACHINE_ARCH (x86_64/aarch64) + major.minor release make up the CDN path;
    // -current (x.99.z) has no packages of its own, so use the latest release
    // branch — its packages run fine there. The CDN redirects the release dir
    // to the current quarterly build, and pkg_add resolves the versioned
    // package filename, so nothing here goes stale.
    let arch = uname("-p");
    let rel = {
        let r = uname("-r");
        let mut it = r.split('.');
        let major = it.next().unwrap_or("10");
        let minor = it.next().unwrap_or("1");
        if minor == "99" {
            format!("{major}.1")
        } else {
            format!("{major}.{minor}")
        }
    };
    // Plain http on purpose: base pkg_add's libfetch has no TLS.
    let pkg_path = format!("http://cdn.netbsd.org/pub/pkgsrc/packages/NetBSD/{arch}/{rel}/All");
    run_cmd(
        Command::new("pkg_add")
            .arg("tailscale")
            .env("PKG_PATH", &pkg_path),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd")))]
fn install_os() -> i32 {
    eprintln!("tailscale install: unsupported guest OS");
    1
}

// --- start ---------------------------------------------------------------------

fn start(kernel_tun: bool) -> i32 {
    let tailscaled = match find_bin("tailscaled") {
        Some(p) => p,
        None => {
            eprintln!("tailscaled not found — run `bsdkrun-agent tailscale install` first");
            return 1;
        }
    };
    if daemon_running() {
        println!("tailscaled already running");
        return 0;
    }

    let _ = std::fs::create_dir_all(STATE_DIR);
    let _ = std::fs::create_dir_all(RUN_DIR);

    let mut cmd = Command::new(&tailscaled);
    cmd.arg(format!("--state={STATE_DIR}/tailscaled.state"))
        .arg(format!("--socket={SOCKET}"));
    // Kernel TUN only when asked for or clearly present (Linux /dev/net/tun);
    // the microVM kernels bsdkrun boots usually don't have tun/tap compiled in,
    // and userspace networking still makes the guest reachable over the tailnet.
    let use_kernel_tun = kernel_tun || (cfg!(target_os = "linux") && Path::new("/dev/net/tun").exists());
    if !use_kernel_tun {
        cmd.arg("--tun=userspace-networking");
    }

    // Detach: own session, stdio to a log file, and never waited on — it must
    // outlive this (exec'd, short-lived) CLI process.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_FILE);
    match log {
        Ok(f) => {
            let ferr = f.try_clone().ok();
            cmd.stdin(Stdio::null()).stdout(f);
            if let Some(fe) = ferr {
                cmd.stderr(fe);
            }
        }
        Err(_) => {
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(child) => std::mem::forget(child), // orphan on purpose; init adopts it
        Err(e) => {
            eprintln!("failed to start tailscaled: {e}");
            return 1;
        }
    }

    // Wait for the control socket (up to ~10s).
    for _ in 0..100 {
        if daemon_running() {
            println!("tailscaled started (log: {LOG_FILE})");
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("tailscaled did not come up within 10s — check {LOG_FILE}");
    1
}

/// The daemon is up when its control socket answers `tailscale version`.
fn daemon_running() -> bool {
    if !Path::new(SOCKET).exists() {
        return false;
    }
    match find_bin("tailscale") {
        Some(ts) => Command::new(ts)
            .args([&format!("--socket={SOCKET}"), "version", "--daemon"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        None => true, // socket exists, client missing — assume up
    }
}

// --- status --------------------------------------------------------------------

fn status(args: &[String]) -> i32 {
    let ts = match find_bin("tailscale") {
        Some(p) => p,
        None => {
            eprintln!("tailscale not found — run `bsdkrun-agent tailscale install` first");
            return 1;
        }
    };
    run_cmd(
        Command::new(ts)
            .arg(format!("--socket={SOCKET}"))
            .arg("status")
            .args(args),
    )
}

// --- setup ---------------------------------------------------------------------

/// install (if needed) + start (if needed) + `tailscale up`.
/// Recognized: --authkey/--auth-key K, --hostname H, --kernel-tun; anything
/// else is passed through to `tailscale up` verbatim.
fn setup(args: &[String]) -> i32 {
    let mut authkey: Option<String> = None;
    let mut hostname: Option<String> = None;
    let mut kernel_tun = false;
    let mut up_extra: Vec<String> = Vec::new();

    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--authkey" | "--auth-key" => authkey = it.next().cloned(),
            "--hostname" => hostname = it.next().cloned(),
            "--kernel-tun" => kernel_tun = true,
            other => up_extra.push(other.to_string()),
        }
    }
    // TS_AUTHKEY from the exec environment works too (bsdkrun exec forwards env).
    if authkey.is_none() {
        authkey = std::env::var("TS_AUTHKEY").ok().filter(|s| !s.is_empty());
    }

    if find_bin("tailscaled").is_none() {
        let code = install();
        if code != 0 {
            return code;
        }
    }
    if !daemon_running() {
        let code = start(kernel_tun);
        if code != 0 {
            return code;
        }
    }

    let ts = match find_bin("tailscale") {
        Some(p) => p,
        None => {
            eprintln!("tailscale client not found after install");
            return 1;
        }
    };
    let mut cmd = Command::new(ts);
    cmd.arg(format!("--socket={SOCKET}")).arg("up");
    if let Some(k) = authkey {
        cmd.arg(format!("--auth-key={k}"));
    }
    if let Some(h) = hostname {
        cmd.arg(format!("--hostname={h}"));
    }
    cmd.args(&up_extra);
    let code = run_cmd(&mut cmd);
    if code == 0 {
        // Show the result (IPs + peers) so `setup` ends with something useful.
        let _ = run_cmd(
            Command::new(find_bin("tailscale").unwrap())
                .arg(format!("--socket={SOCKET}"))
                .arg("status"),
        );
    }
    code
}

// --- helpers ---------------------------------------------------------------------

/// Look for `name` in $PATH plus the usual package prefixes.
fn find_bin(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':').chain(EXTRA_DIRS.iter().copied()) {
        if dir.is_empty() {
            continue;
        }
        let p = Path::new(dir).join(name);
        if is_executable(&p) {
            return Some(p);
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Run a command inheriting stdio; map the exit status to a code.
fn run_cmd(cmd: &mut Command) -> i32 {
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("failed to run {:?}: {e}", cmd.get_program());
            1
        }
    }
}

#[cfg(target_os = "linux")]
fn sh(script: &str) -> i32 {
    run_cmd(Command::new("/bin/sh").arg("-c").arg(script))
}

#[cfg(target_os = "linux")]
fn uname_m() -> String {
    uname("-m")
}

#[cfg(any(target_os = "linux", target_os = "netbsd"))]
fn uname(flag: &str) -> String {
    Command::new("uname")
        .arg(flag)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}
