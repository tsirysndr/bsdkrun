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

/// Recursive apparent size of a directory tree, symlinks not followed.
///
/// Best-effort: an unreadable subdirectory contributes nothing rather than
/// failing the caller, because everything asking this is producing a report —
/// a number that is slightly low beats an error where a size was wanted.
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() && !e.path().is_symlink() {
                stack.push(e.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

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

    /// The architecture name Unikraft/`kraft` uses (`kraft build --arch <uk>`,
    /// and the `<name>_fc-<uk>` image it writes).
    pub fn uk_slug(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "arm64",
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

/// Copy without attempting a CoW clone first. Use when the copy is *known* to
/// cross filesystems (migrating onto the case-sensitive store, say): clonefile
/// fails `EXDEV` there, and `cow_copy`'s fallback chain would print an alarming
/// "cp: … clonefile failed: Cross-device link" on the way to succeeding.
pub fn plain_copy_tree(src: &Path, dst: &Path) -> Result<()> {
    plain_copy(src, dst, true)
}

/// The filesystem a path is on, walking up to the first ancestor that exists —
/// so this answers for a destination that has not been created yet.
pub fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let mut p = path;
    loop {
        if let Ok(md) = std::fs::symlink_metadata(p) {
            return Some(md.dev());
        }
        p = p.parent()?;
    }
}

/// Whether a CoW clone between these two paths can work at all.
///
/// `clonefile`/`--reflink` share *extents*, which cannot cross a filesystem:
/// the copy silently degrades to a byte-for-byte one, and on macOS `cp -Rc`
/// announces it with an alarming "clonefile failed: Cross-device link" on the
/// way to succeeding. Callers that know a copy leaves the volume should use
/// [`plain_copy_tree`] instead and skip the noise.
pub fn same_device(a: &Path, b: &Path) -> bool {
    match (device_of(a), device_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// CoW-clone a tree when the two ends share a filesystem, else copy it plainly.
pub fn clone_or_copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if same_device(src, dst) {
        return cow_copy(src, dst, true);
    }
    plain_copy_tree(src, dst)
}

/// [`clone_or_copy_tree`] for a single file.
pub fn clone_or_copy_file(src: &Path, dst: &Path) -> Result<()> {
    if same_device(src, dst) {
        return cow_copy(src, dst, false);
    }
    plain_copy(src, dst, false)
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

/// The KVM device node every hardware-virtualized VM on Linux needs.
#[cfg(target_os = "linux")]
const KVM_DEV: &str = "/dev/kvm";

/// The KVM userspace API version libkrun (and every other VMM) expects.
/// `KVM_GET_API_VERSION` has returned 12 since Linux 2.6.22 and is stable.
#[cfg(target_os = "linux")]
const KVM_API_VERSION: i32 = 12;

/// `_IO(KVMIO, 0x00)` — `KVM_GET_API_VERSION`, the one ioctl that's safe to
/// issue on a bare `/dev/kvm` fd (it takes no argument and creates nothing).
#[cfg(target_os = "linux")]
const KVM_GET_API_VERSION: u64 = 0xAE00;

/// Why `/dev/kvm` isn't usable. Kept separate from the message so the advice
/// can be built (and tested) from host facts the probe gathers.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
pub enum KvmProblem {
    /// No device node at all — module not loaded, no virt extensions, or a
    /// container that wasn't given the device.
    Missing,
    /// Something is at `/dev/kvm`, but it isn't a character device.
    NotADevice,
    /// The node exists but this user can't open it read-write (EACCES/EPERM).
    Permission,
    /// EBUSY — another hypervisor holds the CPU's virtualization extensions.
    Busy,
    /// Any other open(2) failure, or an ioctl that didn't come back.
    OpenFailed(String),
    /// It opened, but it isn't speaking the KVM API we expect.
    ApiVersion(i32),
}

/// Verify this host can actually run a hardware-virtualized machine, so a
/// missing/inaccessible `/dev/kvm` surfaces as one actionable sentence instead
/// of libkrun failing deep inside `krun_create_ctx` with a bare errno.
///
/// A no-op on macOS, where Hypervisor.framework is gated by an entitlement
/// rather than a device node.
#[cfg(target_os = "linux")]
pub fn check_kvm() -> Result<()> {
    match probe_kvm() {
        Ok(version) => {
            tracing::debug!(api_version = version, "/dev/kvm is usable");
            Ok(())
        }
        Err(problem) => bail!("{}", kvm_advice(&problem, &KvmFacts::gather())),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn check_kvm() -> Result<()> {
    Ok(())
}

/// One-line summary of the KVM state for `bsdkrun probe`. `None` off Linux.
pub fn kvm_summary() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // The advice text already names the device, so only the ok line needs it.
        Some(match probe_kvm() {
            Ok(version) => format!("{KVM_DEV}: ok (KVM API version {version})"),
            Err(problem) => kvm_advice(&problem, &KvmFacts::gather()),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Everything `bsdkrun kvm` reports: the verdict, plus the host facts that
/// explain it. Gathered in one pass so the command prints a consistent picture.
#[cfg(target_os = "linux")]
pub struct KvmStatus {
    pub device: &'static str,
    /// `Ok(api_version)` when a machine can boot here.
    pub result: std::result::Result<i32, KvmProblem>,
    pub facts: KvmFacts,
}

#[cfg(target_os = "linux")]
impl KvmStatus {
    pub fn gather() -> KvmStatus {
        KvmStatus {
            device: KVM_DEV,
            result: probe_kvm(),
            facts: KvmFacts::gather(),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// The fix for whatever is wrong, or `None` when nothing is.
    pub fn advice(&self) -> Option<String> {
        self.result
            .as_ref()
            .err()
            .map(|p| kvm_advice(p, &self.facts))
    }

    /// Short verdict for the first line of the report.
    pub fn headline(&self) -> String {
        match &self.result {
            Ok(v) => format!("ok (KVM API version {v})"),
            Err(KvmProblem::Missing) => "missing".to_string(),
            Err(KvmProblem::NotADevice) => "not a character device".to_string(),
            Err(KvmProblem::Permission) => "permission denied".to_string(),
            Err(KvmProblem::Busy) => "busy".to_string(),
            Err(KvmProblem::OpenFailed(e)) => format!("unusable ({e})"),
            Err(KvmProblem::ApiVersion(v)) => format!("unexpected API version {v}"),
        }
    }
}

/// Open `/dev/kvm` read-write and ask it its API version. Read-write because
/// that's how a VMM opens it — a read-only-capable node would still fail at
/// boot, so checking anything less would pass a host that can't run a machine.
#[cfg(target_os = "linux")]
fn probe_kvm() -> std::result::Result<i32, KvmProblem> {
    probe_kvm_at(Path::new(KVM_DEV))
}

#[cfg(target_os = "linux")]
fn probe_kvm_at(dev: &Path) -> std::result::Result<i32, KvmProblem> {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::io::AsRawFd;

    match std::fs::metadata(dev) {
        Ok(md) if !md.file_type().is_char_device() => return Err(KvmProblem::NotADevice),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(KvmProblem::Missing),
        Err(e) if e.raw_os_error() == Some(libc::EACCES) => return Err(KvmProblem::Permission),
        Err(e) => return Err(KvmProblem::OpenFailed(e.to_string())),
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dev)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EPERM) => KvmProblem::Permission,
            Some(libc::EBUSY) => KvmProblem::Busy,
            _ => KvmProblem::OpenFailed(e.to_string()),
        })?;

    // SAFETY: a valid fd we own, and KVM_GET_API_VERSION takes no argument. The
    // request number is arch-independent (`_IO(KVMIO, 0x00)` — verified 0xAE00
    // on both x86_64 and aarch64 kernel headers), so this is the same on arm64.
    let version = unsafe { libc::ioctl(file.as_raw_fd(), KVM_GET_API_VERSION as _) };
    if version < 0 {
        return Err(KvmProblem::OpenFailed(format!(
            "KVM_GET_API_VERSION: {}",
            std::io::Error::last_os_error()
        )));
    }
    if version != KVM_API_VERSION {
        return Err(KvmProblem::ApiVersion(version));
    }
    Ok(version)
}

/// Host facts that decide *which* advice a `/dev/kvm` failure deserves —
/// gathered once, passed in, so the message logic stays a pure function.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct KvmFacts {
    /// `vmx` (Intel) / `svm` (AMD) in `/proc/cpuinfo`; `None` on arm64, where
    /// virtualization isn't advertised as a CPU flag.
    pub cpu_virt_flag: Option<&'static str>,
    /// A `kvm_intel` / `kvm_amd` / `kvm` module is loaded.
    pub module_loaded: bool,
    /// We're inside a container, so the node most likely just wasn't passed in.
    pub in_container: bool,
    /// Group that owns the node, and its mode — for the permission hint.
    pub owner_group: Option<String>,
    pub mode: Option<u32>,
}

#[cfg(target_os = "linux")]
impl KvmFacts {
    fn gather() -> KvmFacts {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let (owner_group, mode) = kvm_ownership();
        KvmFacts {
            cpu_virt_flag: cpu_virt_flag(&cpuinfo),
            module_loaded: ["kvm_intel", "kvm_amd", "kvm"]
                .iter()
                .any(|m| Path::new(&format!("/sys/module/{m}")).exists()),
            in_container: Path::new("/.dockerenv").exists()
                || Path::new("/run/.containerenv").exists(),
            owner_group,
            mode,
        }
    }
}

/// The virtualization flag `/proc/cpuinfo` advertises, if any. Only meaningful
/// on x86 — arm64 kernels don't list one, so `None` there says nothing.
#[cfg(target_os = "linux")]
fn cpu_virt_flag(cpuinfo: &str) -> Option<&'static str> {
    let flags = cpuinfo
        .lines()
        .find(|l| l.starts_with("flags") || l.starts_with("Features"))?;
    let has = |f: &str| flags.split_whitespace().any(|w| w == f);
    if has("vmx") {
        Some("vmx")
    } else if has("svm") {
        Some("svm")
    } else {
        None
    }
}

/// The owning group name and permission bits of `/dev/kvm`, for the "you're not
/// in the right group" hint. Distros hand the node to `kvm` (Debian/Ubuntu) or
/// `libvirt`/`wheel` (others), so quote the real one rather than guessing.
#[cfg(target_os = "linux")]
fn kvm_ownership() -> (Option<String>, Option<u32>) {
    use std::os::unix::fs::MetadataExt;
    let Ok(md) = std::fs::metadata(KVM_DEV) else {
        return (None, None);
    };
    let group = std::fs::read_to_string("/etc/group")
        .ok()
        .and_then(|g| group_name(&g, md.gid()));
    (group, Some(md.mode() & 0o777))
}

/// Look a gid up in `/etc/group` contents (`name:passwd:gid:members`).
#[cfg(target_os = "linux")]
fn group_name(etc_group: &str, gid: u32) -> Option<String> {
    etc_group.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let found: u32 = fields.nth(1)?.parse().ok()?;
        (found == gid).then(|| name.to_string())
    })
}

/// Turn a probe failure plus host facts into the message the user sees:
/// what's wrong, and the specific command that fixes it on *this* host.
#[cfg(target_os = "linux")]
fn kvm_advice(problem: &KvmProblem, facts: &KvmFacts) -> String {
    let x86 = cfg!(target_arch = "x86_64");
    match problem {
        KvmProblem::Missing => {
            let hint = if facts.in_container {
                "This looks like a container — start it with `--device /dev/kvm` \
                 (and make sure the host itself has KVM)."
                    .to_string()
            } else if x86 && facts.cpu_virt_flag.is_none() {
                "This CPU advertises no virtualization extensions (no `vmx`/`svm` in \
                 /proc/cpuinfo). Enable VT-x / AMD-V in the BIOS/UEFI — or, if this \
                 machine is itself a VM, enable nested virtualization on its host."
                    .to_string()
            } else if !x86 {
                // arm64 kernels build KVM in rather than shipping a module, so
                // modprobe advice would be wrong here: a missing node means the
                // kernel didn't boot at EL2 (the common cause being a guest on a
                // host without nested virtualization — GitHub's arm64 runners,
                // and Apple silicon before M3, are exactly this).
                "KVM is built into arm64 kernels, so a missing node means the kernel \
                 didn't come up at EL2 — usually because this machine is itself a VM \
                 on a host without nested virtualization. Check `dmesg | grep -i kvm`."
                    .to_string()
            } else if !facts.module_loaded {
                let module = match facts.cpu_virt_flag {
                    Some("svm") => "kvm_amd",
                    _ => "kvm_intel",
                };
                format!("The KVM module isn't loaded — try `sudo modprobe {module}`.")
            } else {
                "The KVM module is loaded but the device node is missing — check udev \
                 (`sudo udevadm trigger`) and `dmesg | grep -i kvm`."
                    .to_string()
            };
            format!("{KVM_DEV} does not exist, so this host can't run a machine.\n{hint}")
        }
        KvmProblem::NotADevice => format!(
            "{KVM_DEV} exists but is not a character device — something has replaced the \
             KVM node. Remove it and reload the kvm module."
        ),
        KvmProblem::Permission => {
            let group = facts.owner_group.as_deref().unwrap_or("kvm");
            let mode = facts
                .mode
                .map(|m| format!(" (mode {m:04o}, group `{group}`)"))
                .unwrap_or_default();
            format!(
                "{KVM_DEV} exists but this user can't open it read-write{mode}.\n\
                 Join the owning group, then start a new login session:\n    \
                 sudo usermod -aG {group} $USER\n\
                 `newgrp {group}` picks it up in this shell without logging out."
            )
        }
        KvmProblem::Busy => format!(
            "{KVM_DEV} is busy — another hypervisor (VirtualBox, VMware) is holding the \
             CPU's virtualization extensions. Stop it and retry."
        ),
        KvmProblem::OpenFailed(e) => format!("{KVM_DEV} could not be opened: {e}"),
        KvmProblem::ApiVersion(v) => {
            format!("{KVM_DEV} reports KVM API version {v}, but {KVM_API_VERSION} is required.")
        }
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
/// Linux needs a **much** higher ceiling than macOS, because the two
/// virtio-fs backends represent an inode differently. libkrun's macOS
/// passthrough keeps a path (`InodeHandle::Path`) and opens on demand, so an
/// idle-to-busy guest sits around a couple hundred fds. Its Linux passthrough
/// keeps an `O_PATH` **file descriptor per inode**, pinned in the inode map
/// until the guest sends FUSE `forget` — so the cost scales with the number of
/// inodes the guest has *ever looked up*, not with what it currently has open.
/// Walking a large tree (`nix-store --verify` over a whole store) touches
/// hundreds of thousands of inodes and exhausts the table on its own.
///
/// `RLIM_INFINITY` is not a usable count, so cap the request at what the kernel
/// will actually allow per process (`kern.maxfilesperproc` on macOS,
/// `/proc/sys/fs/nr_open` on Linux). Best effort throughout: a failure here
/// just leaves the inherited limit in place.
pub fn raise_fd_limit() {
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        let mut want = lim.rlim_max;
        // Linux lets a privileged process (CAP_SYS_RESOURCE, i.e. root) raise
        // the HARD limit as well, up to /proc/sys/fs/nr_open — commonly
        // 1048576, against a hard limit that is often only 65536. Unprivileged
        // runs get EPERM and fall through to the soft-to-hard raise below, so
        // this only ever helps.
        #[cfg(target_os = "linux")]
        {
            if let Some(nr_open) = std::fs::read_to_string("/proc/sys/fs/nr_open")
                .ok()
                .and_then(|s| s.trim().parse::<libc::rlim_t>().ok())
            {
                if nr_open > lim.rlim_max {
                    let hard = libc::rlimit {
                        rlim_cur: nr_open,
                        rlim_max: nr_open,
                    };
                    if libc::setrlimit(libc::RLIMIT_NOFILE, &hard) == 0 {
                        return;
                    }
                }
                want = want.min(nr_open);
            }
        }
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

#[cfg(all(test, target_os = "linux"))]
mod kvm_tests {
    use super::*;

    /// A CPU with no virt extensions is the one case where no amount of
    /// modprobe-ing helps, so it must win over the "module not loaded" advice.
    #[test]
    fn missing_node_on_a_cpu_without_virt_extensions_points_at_the_bios() {
        let facts = KvmFacts {
            cpu_virt_flag: None,
            module_loaded: false,
            ..KvmFacts::default()
        };
        let msg = kvm_advice(&KvmProblem::Missing, &facts);
        if cfg!(target_arch = "x86_64") {
            assert!(msg.contains("nested virtualization"), "{msg}");
            assert!(!msg.contains("modprobe"), "{msg}");
        }
    }

    #[test]
    fn missing_node_on_an_amd_host_names_the_amd_module() {
        let facts = KvmFacts {
            cpu_virt_flag: Some("svm"),
            module_loaded: false,
            ..KvmFacts::default()
        };
        let msg = kvm_advice(&KvmProblem::Missing, &facts);
        if cfg!(target_arch = "x86_64") {
            assert!(msg.contains("modprobe kvm_amd"), "{msg}");
        }
    }

    /// In a container the node is almost never missing for a host-level
    /// reason — it just wasn't passed through, and modprobe advice misleads.
    #[test]
    fn missing_node_in_a_container_says_to_pass_the_device_through() {
        let facts = KvmFacts {
            in_container: true,
            cpu_virt_flag: None,
            ..KvmFacts::default()
        };
        let msg = kvm_advice(&KvmProblem::Missing, &facts);
        assert!(msg.contains("--device /dev/kvm"), "{msg}");
    }

    /// The fix is distro-specific: quote the group that actually owns the node.
    #[test]
    fn permission_advice_names_the_owning_group() {
        let facts = KvmFacts {
            owner_group: Some("libvirt".into()),
            mode: Some(0o660),
            ..KvmFacts::default()
        };
        let msg = kvm_advice(&KvmProblem::Permission, &facts);
        assert!(msg.contains("usermod -aG libvirt"), "{msg}");
        assert!(msg.contains("0660"), "{msg}");
    }

    /// arm64 has no loadable kvm module, so `modprobe` advice would send the
    /// user down a dead end — the real cause is a kernel that never reached EL2.
    #[test]
    #[cfg(target_arch = "aarch64")]
    fn missing_node_on_arm64_talks_about_el2_not_modprobe() {
        let msg = kvm_advice(&KvmProblem::Missing, &KvmFacts::default());
        assert!(msg.contains("EL2"), "{msg}");
        assert!(!msg.contains("modprobe"), "{msg}");
    }

    /// Classification must not depend on a usable `/dev/kvm` — these runs on
    /// KVM-less machines (GitHub's arm64 runners) too, so they use temp paths.
    #[test]
    fn probe_classifies_an_absent_node_and_a_non_device() {
        let dir = std::env::temp_dir().join(format!("bsdkrun-kvm-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let absent = dir.join("nope");
        assert_eq!(probe_kvm_at(&absent), Err(KvmProblem::Missing));

        let regular = dir.join("regular");
        std::fs::write(&regular, b"not a device").unwrap();
        assert_eq!(probe_kvm_at(&regular), Err(KvmProblem::NotADevice));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn group_name_resolves_a_gid_from_etc_group() {
        let etc = "root:x:0:\nkvm:x:104:alice\nusers:x:100:\n";
        assert_eq!(group_name(etc, 104).as_deref(), Some("kvm"));
        assert_eq!(group_name(etc, 999), None);
    }

    #[test]
    fn cpu_virt_flag_reads_the_flags_line() {
        let intel = "processor\t: 0\nflags\t\t: fpu vme vmx smx est\n";
        let amd = "processor\t: 0\nflags\t\t: fpu vme svm npt\n";
        let none = "processor\t: 0\nflags\t\t: fpu vme de pse\n";
        assert_eq!(cpu_virt_flag(intel), Some("vmx"));
        assert_eq!(cpu_virt_flag(amd), Some("svm"));
        assert_eq!(cpu_virt_flag(none), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of `raise_fd_limit` is that an inherited-low soft limit
    /// does not survive startup — launchd hands GUI processes 256, and a
    /// virtio-fs guest blows through that immediately.
    #[test]
    fn raise_fd_limit_lifts_a_lowered_soft_limit() {
        unsafe {
            let mut orig: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut orig), 0);
            // Nothing to prove if the hard limit is already tiny.
            if orig.rlim_max <= 256 {
                return;
            }

            // 256 is deliberately generous enough for the other tests running
            // concurrently in this process — the limit is process-wide.
            let low = libc::rlimit {
                rlim_cur: 256,
                rlim_max: orig.rlim_max,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &low), 0);

            raise_fd_limit();

            let mut after: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut after), 0);
            let raised = after.rlim_cur;
            let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &orig);

            assert!(
                raised > 256,
                "soft limit should have been raised above 256, got {raised}"
            );
            // And it must be a usable count rather than RLIM_INFINITY, which
            // macOS rejects outright.
            assert_ne!(raised, libc::RLIM_INFINITY);
        }
    }
}
