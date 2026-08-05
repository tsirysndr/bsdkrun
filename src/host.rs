//! Host platform detection and OS/arch-specific helpers.
//!
//! bsdkrun runs on macOS (Hypervisor.framework) and Linux (KVM). A KVM/HVF
//! microVM's guest runs the **same** CPU architecture as the host, so the host
//! arch also selects the guest kernel, OCI image platform, and agent binary.
//!
//! Platform-specific behavior is gated with `#[cfg(target_os = ...)]` /
//! `target_arch`, so the right code is compiled in per build target — the
//! arch/OS is detected at compile time, no runtime feature flag needed.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};

/// Remove a directory tree even when it contains read-only entries. A nix-based
/// rootfs holds a `/nix/store` whose directories are mode `0555`, so you can't
/// unlink their contents without write permission on the dir — plain
/// `remove_dir_all` fails partway and leaves the tree behind (which then makes a
/// re-clone nest as `rootfs/rootfs`). chmod the tree writable first, then remove.
pub fn force_remove_dir_all(path: &Path) {
    if path.symlink_metadata().is_err() {
        return;
    }
    // chmod the tree writable so `rm` can unlink entries inside 0555 nix dirs,
    // then shell out to `rm -rf` (Rust's remove_dir_all fails partway on those).
    let _ = Command::new("chmod").args(["-R", "u+w"]).arg(path).status();
    let _ = Command::new("rm").args(["-rf"]).arg(path).status();
}

/// Like [`force_remove_dir_all`] but fire-and-forget: spawns the chmod+rm in the
/// background and returns immediately. `rm -rf` of a big read-only nix store is
/// slow; a restart shouldn't block on GC'ing the old clone it already renamed
/// aside. Detached (setsid) so it survives this process exiting.
pub fn force_remove_dir_all_async(path: &Path) {
    if path.symlink_metadata().is_err() {
        return;
    }
    let p = path.to_string_lossy().replace('\'', r"'\''");
    // `nohup … &` (portable — macOS has no `setsid` command) so the cleanup
    // outlives this short-lived process. Detached stdio so it can't hold pipes.
    let _ = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "nohup sh -c 'chmod -R u+w '\\''{p}'\\'' 2>/dev/null; rm -rf '\\''{p}'\\''' </dev/null >/dev/null 2>&1 &"
        ))
        .spawn();
}

/// Delete a directory tree *instantly from its real path*, GC'ing it in the
/// background. Renaming the tree aside is atomic and O(1) — it needs write on the
/// parent only, not on the read-only `0555` nix dirs inside — so the path vanishes
/// immediately (e.g. a machine's state dir disappears the moment it's removed),
/// while the slow `chmod -R`/`rm -rf` of a huge `/nix/store` runs detached. This
/// is what keeps `bsdkrun rm` from blocking (and the desktop's delete from
/// spinning) on a nix machine. Falls back to an in-place async remove if the
/// rename fails.
pub fn remove_dir_all_detached(path: &Path) {
    if path.symlink_metadata().is_err() {
        return;
    }
    // Rename to a hidden sibling in the same parent (same filesystem → atomic).
    let trash = path.file_name().map(|name| {
        path.with_file_name(format!(
            ".trash-{}-{}",
            name.to_string_lossy(),
            std::process::id()
        ))
    });
    let target = match trash {
        Some(t) if t != path && std::fs::rename(path, &t).is_ok() => t,
        _ => path.to_path_buf(),
    };
    force_remove_dir_all_async(&target);
}

/// Supported CPU architectures (host == guest for a hardware-virtualized VM).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// The architecture this binary is running on.
    pub fn current() -> Result<Arch> {
        match std::env::consts::ARCH {
            "x86_64" => Ok(Arch::X86_64),
            "aarch64" | "arm64" => Ok(Arch::Aarch64),
            other => bail!("unsupported CPU architecture: {other} (need x86_64 or aarch64)"),
        }
    }

    /// Canonical slug used in artifact filenames (`bsdkrun-agent.linux-<slug>`,
    /// `vmlinux-<ver>.<slug>`).
    pub fn slug(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    /// The `uname -m` / release name BSD image URLs use (`amd64`, `aarch64`).
    #[allow(dead_code)] // used by BSD-on-Linux paths
    pub fn bsd_machine(self) -> &'static str {
        match self {
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "aarch64",
        }
    }

    /// The OCI platform architecture (`linux/<oci>`).
    pub fn oci(self) -> &'static str {
        match self {
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "arm64",
        }
    }
}

/// Guest OS an agent is built for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuestOs {
    Linux,
    Freebsd,
    Netbsd,
}

impl GuestOs {
    pub fn slug(self) -> &'static str {
        match self {
            GuestOs::Linux => "linux",
            GuestOs::Freebsd => "freebsd",
            GuestOs::Netbsd => "netbsd",
        }
    }
}

/// Human name for the host OS (for messages).
#[allow(dead_code)] // used in OS-specific error messages
pub const OS_NAME: &str = if cfg!(target_os = "macos") {
    "macOS"
} else {
    "Linux"
};

/// A `Command` for a program that needs root privileges. On Linux it prefixes
/// `sudo` when not already root (loop mounts etc. need CAP_SYS_ADMIN) and sudo is
/// installed; on macOS the disk tools (`hdiutil`/`diskutil`) don't need root, so
/// it runs directly. When elevation is needed but sudo is missing, it runs the
/// program directly (which then fails with a clear permission error).
#[allow(dead_code)] // used by BSD-on-Linux disk tooling
pub fn root_command(program: &str) -> Command {
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: geteuid is always safe.
        if unsafe { libc::geteuid() } != 0 && sudo_available() {
            let mut c = Command::new("sudo");
            c.arg(program);
            return c;
        }
    }
    Command::new(program)
}

/// Whether `sudo` is installed and runnable.
#[cfg(not(target_os = "macos"))]
fn sudo_available() -> bool {
    Command::new("sudo")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Copy `src` to `dst` as a copy-on-write clone where the filesystem supports it,
/// falling back to a plain copy. `recursive` clones a directory tree.
#[cfg(target_os = "macos")]
pub fn cow_copy(src: &Path, dst: &Path, recursive: bool) -> Result<()> {
    // A recursive clone (a whole rootfs) goes through clonefile(2) directly: it
    // CoW-clones an ENTIRE directory tree in one syscall — ~10x faster than
    // `cp -Rc`, which clonefiles each of the thousands of nix-store files one by
    // one (~14s → ~1s for a nix image). Blocks are shared, so N machines from one
    // image cost ~one image on disk. Requires `dst` to not exist (callers ensure
    // this); falls back to `cp -Rc` / plain copy on any error (e.g. cross-device).
    if recursive {
        use std::os::unix::ffi::OsStrExt;
        if let (Ok(s), Ok(d)) = (
            std::ffi::CString::new(src.as_os_str().as_bytes()),
            std::ffi::CString::new(dst.as_os_str().as_bytes()),
        ) {
            // clonefile(const char *src, const char *dst, int flags)
            if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } == 0 {
                return Ok(());
            }
        }
    }
    // Fallback: `cp -c`/`-Rc` (per-file clonefile), then a plain copy.
    let flag = if recursive { "-Rc" } else { "-c" };
    if crate::fetch::run(Command::new("cp").arg(flag).arg(src).arg(dst), "cp (clone)").is_ok() {
        return Ok(());
    }
    plain_copy(src, dst, recursive)
}

#[cfg(not(target_os = "macos"))]
pub fn cow_copy(src: &Path, dst: &Path, recursive: bool) -> Result<()> {
    // Linux coreutils: reflink CoW on btrfs/XFS, else a full copy.
    let mut cmd = Command::new("cp");
    cmd.arg("--reflink=auto");
    if recursive {
        cmd.arg("-R");
    }
    cmd.arg(src).arg(dst);
    if crate::fetch::run(&mut cmd, "cp (clone)").is_ok() {
        return Ok(());
    }
    plain_copy(src, dst, recursive)
}

fn plain_copy(src: &Path, dst: &Path, recursive: bool) -> Result<()> {
    if recursive {
        let _ = std::fs::remove_dir_all(dst);
        crate::fetch::run(
            Command::new("cp").arg("-R").arg(src).arg(dst),
            "cp (copy dir)",
        )
    } else {
        let _ = std::fs::remove_file(dst);
        crate::fetch::run(Command::new("cp").arg(src).arg(dst), "cp (copy file)")
    }
}

/// Raise this process's open-file limit (`RLIMIT_NOFILE`) soft limit to the
/// hard limit.
///
/// virtio-fs is a **passthrough** filesystem: every file the guest holds open
/// costs one file descriptor in *this* process, the one serving the device. A
/// guest that opens thousands of files (a `nix` store operation is the extreme
/// case — SQLite plus a large substituter fan-out) therefore exhausts the host
/// process's fd table, and the guest sees the failure as a bewildering
/// `EMFILE`/"Too many open files" or SQLite's "unable to open database file"
/// — errors that point at the guest when the limit is actually ours.
///
/// macOS makes this easy to hit: `launchctl limit maxfiles` defaults to a soft
/// limit of **256** with an unlimited hard limit, and processes started by
/// launchd (i.e. the desktop app, and anything it spawns) inherit that 256 —
/// interactive shells often raise it, which is why the same command can work
/// from a terminal and fail from the GUI. An idle Linux microVM already holds
/// well over 100 fds, so 256 leaves almost no headroom.
///
/// `RLIM_INFINITY` is not a usable count, so cap the request at what the kernel
/// will actually allow per process (`kern.maxfilesperproc` on macOS). Best
/// effort: a failure here just leaves the inherited limit in place.
pub fn raise_fd_limit() {
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        let mut want = lim.rlim_max;
        // macOS refuses any setrlimit above kern.maxfilesperproc, and reports
        // rlim_max as RLIM_INFINITY — asking for infinity fails outright, so
        // clamp to the per-process ceiling the kernel advertises.
        #[cfg(target_os = "macos")]
        {
            let mut per_proc: libc::c_int = 0;
            let mut sz = std::mem::size_of::<libc::c_int>();
            let name = c"kern.maxfilesperproc";
            if libc::sysctlbyname(
                name.as_ptr(),
                &mut per_proc as *mut _ as *mut libc::c_void,
                &mut sz,
                std::ptr::null_mut(),
                0,
            ) == 0
                && per_proc > 0
            {
                want = want.min(per_proc as libc::rlim_t);
            }
        }
        if want <= lim.rlim_cur {
            return;
        }
        lim.rlim_cur = want;
        let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
    }
}
