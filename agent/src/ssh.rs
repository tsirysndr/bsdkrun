//! SSH management, built into the agent so getting key-based SSH into a guest
//! is one command on every OS:
//!
//!   bsdkrun-agent ssh setup [--user U] [--key "ssh-ed25519 ..."]...
//!       install sshd (Linux only — BSD base has it), generate host keys,
//!       install the public keys, make sure key-based root login is allowed,
//!       enable + start sshd
//!   bsdkrun-agent ssh add-key [--user U] --key K [--key K]...
//!       just install public keys (also creates ~/.ssh as needed)
//!   bsdkrun-agent ssh status
//!       is sshd running + how many keys are installed
//!
//! Public keys come from repeatable `--key` flags and/or the
//! `BSDKRUN_SSH_KEYS` environment variable (newline-separated) — the host's
//! `bsdkrun ssh <id> ...` wrapper fills the latter with the local
//! `~/.ssh/id_*.pub` keys, which is what makes setup a one-liner.
//!
//! Reaching the guest: bsdkrun's gvproxy forwards a host port to guest :22 —
//! `bsdkrun ps`/the boot banner show the `ssh -p <port> user@127.0.0.1` line.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::{find_bin, run_quiet};
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd"))]
use crate::util::run_cmd;

const SSHD_CONFIG: &str = "/etc/ssh/sshd_config";
const SSHD_LOG: &str = "/var/log/sshd.log";

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("setup") => setup(&args[1..]),
        Some("add-key") => add_key(&args[1..]),
        Some("status") => status(),
        _ => {
            eprintln!(
                "usage: bsdkrun-agent ssh <setup|add-key|status>\n\
                 \n\
                 setup   [--user U] [--key K]...   install sshd if needed, host keys,\n\
                 \0                                  authorized_keys, enable + start sshd\n\
                 add-key [--user U] --key K...     just install public keys\n\
                 status                            sshd state + installed key count\n\
                 \n\
                 keys also come from $BSDKRUN_SSH_KEYS (newline-separated)"
            );
            2
        }
    }
}

struct KeyArgs {
    user: String,
    keys: Vec<String>,
}

/// Parse `--user U` + repeatable `--key K`, then merge `$BSDKRUN_SSH_KEYS`.
fn parse_key_args(args: &[String]) -> KeyArgs {
    let mut user = "root".to_string();
    let mut keys: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--user" => {
                if let Some(u) = it.next() {
                    user = u.clone();
                }
            }
            "--key" => {
                if let Some(k) = it.next() {
                    keys.push(k.clone());
                }
            }
            other => eprintln!("ssh: ignoring unknown argument {other:?}"),
        }
    }
    if let Ok(env_keys) = std::env::var("BSDKRUN_SSH_KEYS") {
        keys.extend(env_keys.lines().map(str::to_string));
    }
    // Keep only things that look like public keys (defense against pasting a
    // *private* key or garbage into authorized_keys).
    keys.retain(|k| {
        let k = k.trim();
        !k.is_empty()
            && (k.starts_with("ssh-") || k.starts_with("ecdsa-") || k.starts_with("sk-"))
    });
    KeyArgs { user, keys }
}

// --- setup -----------------------------------------------------------------------

fn setup(args: &[String]) -> i32 {
    let ka = parse_key_args(args);

    // 1. sshd present? (BSD base always has it; Linux OCI guests often don't.)
    if find_bin("sshd").is_none() {
        let code = install_sshd();
        if code != 0 {
            return code;
        }
        if find_bin("sshd").is_none() {
            eprintln!("sshd still not found after install");
            return 1;
        }
    }

    // 2. Host keys (no-op when they already exist).
    if let Some(keygen) = find_bin("ssh-keygen") {
        let _ = run_quiet(Command::new(keygen).arg("-A"));
    }

    // 3. The public keys.
    if !ka.keys.is_empty() {
        let code = install_keys(&ka);
        if code != 0 {
            return code;
        }
    } else {
        eprintln!("note: no public keys given (use --key or $BSDKRUN_SSH_KEYS) — sshd will start, but key logins need a key");
    }

    // 4. Key-based root login must not be blocked. If the config changed while
    //    sshd is already up (e.g. re-running setup), HUP it to reload.
    if ensure_root_login() && sshd_running() {
        if let Some(pkill) = find_bin("pkill") {
            let _ = run_quiet(Command::new(pkill).args(["-HUP", "-x", "sshd"]));
            println!("sshd reloaded (HUP)");
        }
    }

    // 5. Linux OCI guests: the rootfs is served over virtio-fs from an
    //    unprivileged host process, so pre-existing files appear owned by the
    //    HOST user (not root) and chown back is EPERM. sshd then rejects both
    //    /var/empty (privsep dir must be root-owned) and, via StrictModes,
    //    authorized_keys under a non-root-owned $HOME. Fix what can be fixed
    //    (tmpfs over /var/empty — guest-root owned by construction) and relax
    //    StrictModes only when the quirk is actually present.
    #[cfg(target_os = "linux")]
    fixup_virtiofs_root();

    // 6. Enable + start.
    start_sshd()
}

/// See setup() step 5. No-ops on roots with sane ownership (initramfs boots,
/// real disks).
#[cfg(target_os = "linux")]
fn fixup_virtiofs_root() {
    use std::os::unix::fs::MetadataExt;

    let uid_of = |p: &str| std::fs::metadata(p).map(|m| m.uid()).ok();

    if uid_of("/var/empty") != Some(0) {
        let _ = std::fs::create_dir_all("/var/empty");
        let _ = crate::util::sh("mount -t tmpfs -o mode=0755 tmpfs /var/empty");
        println!("virtio-fs root: tmpfs over /var/empty (sshd privsep dir)");
    }
    // $HOME (e.g. /root) owned by the host user => StrictModes would refuse
    // the authorized_keys we just installed. Relax it only in that case.
    if uid_of("/root") != Some(0) {
        let cfg = std::fs::read_to_string(SSHD_CONFIG).unwrap_or_default();
        let already = cfg
            .lines()
            .any(|l| l.trim().to_lowercase().starts_with("strictmodes"));
        if !already {
            let mut out = cfg;
            out.push_str(
                "\n# bsdkrun: rootfs is virtio-fs (host-owned files); ownership checks\n\
                 # can't pass and chown is EPERM, so disable them. Key-only logins are\n\
                 # still enforced by PermitRootLogin prohibit-password.\n\
                 StrictModes no\n",
            );
            let _ = std::fs::write(SSHD_CONFIG, out);
            println!("virtio-fs root: StrictModes no (host-owned $HOME)");
        }
    }
}

#[cfg(target_os = "linux")]
fn install_sshd() -> i32 {
    // Alpine first (the usual OCI guest), then the other common managers.
    if find_bin("apk").is_some() {
        return run_cmd(Command::new("apk").args(["add", "--no-cache", "openssh-server"]));
    }
    if find_bin("apt-get").is_some() {
        let code = run_cmd(Command::new("apt-get").args(["update", "-qq"]));
        if code != 0 {
            return code;
        }
        return run_cmd(
            Command::new("apt-get")
                .args(["install", "-y", "-qq", "openssh-server"])
                .env("DEBIAN_FRONTEND", "noninteractive"),
        );
    }
    if find_bin("dnf").is_some() {
        return run_cmd(Command::new("dnf").args(["install", "-y", "openssh-server"]));
    }
    eprintln!("no known package manager (apk/apt-get/dnf) to install openssh-server");
    1
}

#[cfg(not(target_os = "linux"))]
fn install_sshd() -> i32 {
    // FreeBSD and NetBSD ship sshd in base — nothing to install.
    eprintln!("sshd not found in base system?!");
    1
}

/// Append missing keys to ~user/.ssh/authorized_keys with the right modes and
/// ownership (sshd rejects keys in group/world-writable paths).
fn install_keys(ka: &KeyArgs) -> i32 {
    let (home, uid, gid) = match passwd_entry(&ka.user) {
        Some(t) => t,
        None => {
            eprintln!("user {:?} not found in /etc/passwd", ka.user);
            return 1;
        }
    };
    let ssh_dir = Path::new(&home).join(".ssh");
    let auth = ssh_dir.join("authorized_keys");

    if std::fs::create_dir_all(&ssh_dir).is_err() {
        eprintln!("cannot create {}", ssh_dir.display());
        return 1;
    }
    let existing = std::fs::read_to_string(&auth).unwrap_or_default();
    let mut added = 0;
    let mut out = existing.clone();
    for key in &ka.keys {
        let key = key.trim();
        // Dedupe on the key material (type + base64), ignoring the comment.
        let material = |s: &str| {
            s.split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        };
        if existing.lines().any(|l| material(l) == material(key)) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(key);
        out.push('\n');
        added += 1;
    }
    if added > 0 {
        let write = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&auth)
            .and_then(|mut f| f.write_all(out.as_bytes()));
        if write.is_err() {
            eprintln!("cannot write {}", auth.display());
            return 1;
        }
    }

    // 700 / 600, owned by the target user — sshd insists.
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&ssh_dir, std::fs::Permissions::from_mode(0o700));
    let _ = std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600));
    for p in [&ssh_dir, &auth] {
        if let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) {
            unsafe { libc::chown(c.as_ptr(), uid, gid) };
        }
    }

    println!(
        "{added} key(s) added for {} ({} total)",
        ka.user,
        out.lines().filter(|l| !l.trim().is_empty()).count()
    );
    0
}

/// `home`, `uid`, `gid` for a user, straight from /etc/passwd (no getent —
/// portable across all three guests and static-musl safe).
fn passwd_entry(user: &str) -> Option<(String, libc::uid_t, libc::gid_t)> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 6 && f[0] == user {
            return Some((f[5].to_string(), f[2].parse().ok()?, f[3].parse().ok()?));
        }
    }
    None
}

/// Make sure key-based root login is allowed; returns true when the config
/// changed. Two cases:
/// - an explicit `PermitRootLogin no` line → rewrite to prohibit-password;
/// - NO active PermitRootLogin directive at all → append one. This is the
///   FreeBSD/NetBSD case: both patch OpenSSH's COMPILED default to "no" (the
///   shipped config only has a `#PermitRootLogin no` comment documenting it),
///   so root pubkey is refused and the client falls through to
///   keyboard-interactive → "PAM: Authentication error for root" spam.
///   On Linux appending prohibit-password just restates the upstream default.
fn ensure_root_login() -> bool {
    let Ok(cfg) = std::fs::read_to_string(SSHD_CONFIG) else {
        return false;
    };
    let mut rewrote = false;
    let mut has_active = false;
    let new: String = cfg
        .lines()
        .map(|l| {
            let t = l.trim();
            if !t.starts_with('#') && t.to_lowercase().starts_with("permitrootlogin") {
                has_active = true;
                if t.to_lowercase().ends_with(" no") {
                    rewrote = true;
                    return "PermitRootLogin prohibit-password".to_string();
                }
            }
            l.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    if rewrote {
        let _ = std::fs::write(SSHD_CONFIG, new + "\n");
        println!("sshd_config: PermitRootLogin no -> prohibit-password (key logins)");
        return true;
    }
    if !has_active {
        let mut out = cfg;
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(
            "# bsdkrun: FreeBSD/NetBSD compile PermitRootLogin's default to \"no\";\n\
             # allow key-only root logins (never passwords).\n\
             PermitRootLogin prohibit-password\n",
        );
        let _ = std::fs::write(SSHD_CONFIG, out);
        println!("sshd_config: PermitRootLogin prohibit-password added (key logins)");
        return true;
    }
    false
}

/// "Running" = something answers on 127.0.0.1:22 — the property ssh actually
/// needs, and far more reliable than pgrep across busybox/BSD ps variants.
fn sshd_running() -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 22));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
        return true;
    }
    find_bin("pgrep")
        .map(|p| run_quiet(Command::new(p).args(["-x", "sshd"])))
        .unwrap_or(false)
}

/// Enable at boot (where the OS has rc) and start now.
fn start_sshd() -> i32 {
    if sshd_running() {
        println!("sshd already running");
        return 0;
    }
    let code = start_sshd_os();
    if code != 0 {
        return code;
    }
    // sshd double-forks; give it a moment (10s) before declaring it dead.
    for _ in 0..100 {
        if sshd_running() {
            println!("sshd running");
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("sshd started but nothing listens on :22 — its log:");
    if let Ok(log) = std::fs::read_to_string(SSHD_LOG) {
        for l in log.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev() {
            eprintln!("  {l}");
        }
    } else {
        eprintln!("  ({SSHD_LOG} not written)");
    }
    1
}

#[cfg(target_os = "freebsd")]
fn start_sshd_os() -> i32 {
    // Persist across reboots, then start (the rc script generates any missing
    // host keys itself).
    let _ = run_quiet(Command::new("sysrc").arg("sshd_enable=YES"));
    run_cmd(Command::new("service").args(["sshd", "start"]))
}

#[cfg(target_os = "netbsd")]
fn start_sshd_os() -> i32 {
    // rc.conf gate + rc.d start (onestart works even without the gate, but
    // the gate persists the service across reboots).
    let rc = std::fs::read_to_string("/etc/rc.conf").unwrap_or_default();
    if !rc.lines().any(|l| l.trim_start().starts_with("sshd=")) {
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open("/etc/rc.conf") {
            let _ = writeln!(f, "sshd=YES");
        }
    }
    run_cmd(Command::new("/etc/rc.d/sshd").arg("start"))
}

#[cfg(target_os = "linux")]
fn start_sshd_os() -> i32 {
    // OCI microVM guests have no service manager — run sshd directly (it
    // daemonizes itself). Needs the privilege-separation dir on some distros.
    // -E: keep the daemon's complaints somewhere we can show on failure.
    let _ = std::fs::create_dir_all("/var/empty");
    let _ = std::fs::create_dir_all("/run/sshd");
    let sshd = find_bin("sshd").expect("checked in setup()");
    run_cmd(Command::new(sshd).args(["-E", SSHD_LOG]))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd")))]
fn start_sshd_os() -> i32 {
    eprintln!("ssh setup: unsupported guest OS");
    1
}

// --- add-key / status ---------------------------------------------------------------

fn add_key(args: &[String]) -> i32 {
    let ka = parse_key_args(args);
    if ka.keys.is_empty() {
        eprintln!("no public keys given (use --key or $BSDKRUN_SSH_KEYS)");
        return 1;
    }
    install_keys(&ka)
}

fn status() -> i32 {
    let running = sshd_running();
    println!("sshd: {}", if running { "running" } else { "not running" });
    for user in ["root"] {
        if let Some((home, _, _)) = passwd_entry(user) {
            let auth = PathBuf::from(home).join(".ssh/authorized_keys");
            let n = std::fs::read_to_string(&auth)
                .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
                .unwrap_or(0);
            println!("{user}: {n} authorized key(s)");
        }
    }
    if running {
        0
    } else {
        1
    }
}
