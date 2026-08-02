//! Small helpers shared by the agent's CLI modules (tailscale, ssh).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Dirs searched for binaries beyond $PATH (pkg installs to /usr/local on
/// FreeBSD, /usr/pkg on NetBSD; static installs go to /usr/local/bin).
pub const EXTRA_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/pkg/bin",
    "/usr/pkg/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/bin",
    "/sbin",
];

/// Look for `name` in $PATH plus the usual package prefixes.
pub fn find_bin(name: &str) -> Option<PathBuf> {
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

pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Run a command inheriting stdio; map the exit status to a code.
pub fn run_cmd(cmd: &mut Command) -> i32 {
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("failed to run {:?}: {e}", cmd.get_program());
            1
        }
    }
}

/// Run a command silently; true on exit 0.
pub fn run_quiet(cmd: &mut Command) -> bool {
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn sh(script: &str) -> i32 {
    run_cmd(Command::new("/bin/sh").arg("-c").arg(script))
}

#[cfg(any(target_os = "linux", target_os = "netbsd"))]
pub fn uname(flag: &str) -> String {
    Command::new("uname")
        .arg(flag)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}
