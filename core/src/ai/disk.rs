//! Disks shared between agent sandboxes: the Docker store and the Nix store.
//!
//! An agent that pulls an image or realises a derivation should not have to do
//! it again in the next session, or in another agent's sandbox. Both stores are
//! content-addressed and both are enormous, so they live on their own disks
//! instead of inside each sandbox's disposable rootfs.
//!
//! ## Why one holder at a time
//!
//! These are ext4 images on virtio-blk. Two running guests mounting one
//! read-write **corrupts it** — ext4 has no idea another kernel is writing the
//! same blocks, and the damage surfaces later as unreadable images or a broken
//! Nix database, long after the session that caused it.
//!
//! So a shared disk is attached to at most one running sandbox at a time
//! ([`claim`]). A second concurrent sandbox boots without it and is told why:
//! it still works, it just starts from an empty store. Sharing across sessions
//! *over time* — which is what makes a rebuild fast — is unaffected, and that
//! is the case this is for.
//!
//! The holder is recorded rather than locked, and checked for liveness on the
//! next claim: a VM that is killed cannot release anything, and a lock nothing
//! can break is worse than no lock.
//!
//! ## What is *not* here
//!
//! A sandbox has no root block device. Its rootfs, its `$HOME` volume and its
//! workspace are all host directories shared over virtio-fs, bounded by the
//! host filesystem and nothing else — there is no per-sandbox size to raise,
//! and a sandbox that runs out of space has filled the host.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::db;

/// A store shared between sandboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shared {
    /// `/var/lib/docker` — images, layers and the build cache.
    Docker,
    /// `/nix` — the store and its database.
    Nix,
}

/// Both, in the order they are attached.
pub const ALL: &[Shared] = &[Shared::Docker, Shared::Nix];

impl Shared {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "docker" => Ok(Shared::Docker),
            "nix" => Ok(Shared::Nix),
            other => anyhow::bail!("unknown shared disk {other:?} (docker | nix)"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Shared::Docker => "docker",
            Shared::Nix => "nix",
        }
    }

    /// Where the guest mounts it.
    pub fn guest_path(self) -> &'static str {
        match self {
            Shared::Docker => "/var/lib/docker",
            Shared::Nix => "/nix",
        }
    }

    /// The size a disk is created at, when nothing has asked for one.
    ///
    /// Sparse, so this costs nothing until it is filled — the number is a
    /// ceiling, chosen high enough that nobody meets it by accident. Docker
    /// gets the larger one: images dwarf a Nix store in practice.
    pub fn default_size(self) -> &'static str {
        match self {
            Shared::Docker => "64G",
            Shared::Nix => "32G",
        }
    }

    pub fn image(self) -> Result<PathBuf> {
        Ok(dir()?.join(format!("{}.img", self.as_str())))
    }

    fn holder_file(self) -> Result<PathBuf> {
        Ok(dir()?.join(format!("{}.holder", self.as_str())))
    }
}

/// Where the shared disks live: beside the machines, not inside one, because
/// they outlive every sandbox that uses them.
fn dir() -> Result<PathBuf> {
    let dir = db::state_dir()?.join("ai-shared");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Set to `1` to boot sandboxes with no shared disks at all.
///
/// The escape hatch for the case this design cannot serve: two sandboxes that
/// both need Docker at the same time, where neither should be the one that
/// silently goes without.
pub const DISABLE_ENV: &str = "BSDKRUN_AI_NO_SHARED_DISKS";

fn disabled() -> bool {
    std::env::var(DISABLE_ENV).is_ok_and(|v| v != "0" && !v.is_empty())
}

/// Create the image if missing, or grow it to `size`.
///
/// Sparse. [`crate::fetch::grow`] refuses to shrink, which is the behaviour
/// wanted here too — a smaller number would cut a live filesystem in half.
pub fn ensure(what: Shared, size: &str) -> Result<PathBuf> {
    let path = what.image()?;
    if !path.exists() {
        std::fs::File::create(&path).with_context(|| {
            format!(
                "creating the shared {} disk {}",
                what.as_str(),
                path.display()
            )
        })?;
    }
    // `grow` errors rather than no-ops when the image is already at least this
    // big, which is the normal case on every boot after the first — treating it
    // as a failure meant the disk was silently never claimed. The Docker engine
    // VM documents the same trap; this is the same tolerance.
    match crate::fetch::grow(&path, size) {
        Ok(()) => {}
        Err(e) if path.metadata().map(|m| m.len()).unwrap_or(0) > 0 => {
            tracing::debug!("shared {} disk not grown: {e:#}", what.as_str());
        }
        Err(e) => return Err(e),
    }
    Ok(path)
}

/// The machine currently holding a disk, if one is still running.
///
/// A recorded holder that is no longer running is stale and reported as free —
/// a VM that was killed had no chance to clean up after itself.
pub fn holder(what: Shared) -> Option<String> {
    let id = std::fs::read_to_string(what.holder_file().ok()?).ok()?;
    let id = id.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let db = db::Db::open().ok()?;
    let vm = db.find_machine(&id).ok()?;
    let alive = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
    alive.then_some(id)
}

/// Claim a disk for `machine_id`, or report who has it.
///
/// Returns the image path when the claim succeeds. `Ok(None)` means another
/// running sandbox holds it — a normal outcome, not an error: the caller boots
/// without it.
pub fn claim(what: Shared, machine_id: &str) -> Result<Option<PathBuf>> {
    if let Some(current) = holder(what) {
        if current != machine_id {
            return Ok(None);
        }
    }
    // Sized on first use so a sandbox gets the benefit without a setup step.
    let path = ensure(what, what.default_size())?;
    std::fs::write(what.holder_file()?, machine_id)
        .with_context(|| format!("recording the holder of the shared {} disk", what.as_str()))?;
    Ok(Some(path))
}

/// The disks a sandbox got, and the ones another sandbox is holding.
pub type Claimed = (Vec<(Shared, PathBuf)>, Vec<(Shared, String)>);

/// The disks this sandbox should attach, and the ones it cannot have.
///
/// Both are returned because the second half is worth saying out loud: a
/// sandbox quietly missing its Docker cache looks like the cache does not work.
pub fn claim_all(machine_id: &str) -> Claimed {
    if disabled() {
        info!("{DISABLE_ENV} is set — booting the sandbox with no shared disks");
        return (Vec::new(), Vec::new());
    }
    let mut got = Vec::new();
    let mut busy = Vec::new();
    for &what in ALL {
        match claim(what, machine_id) {
            Ok(Some(path)) => got.push((what, path)),
            Ok(None) => {
                if let Some(h) = holder(what) {
                    busy.push((what, h));
                }
            }
            // A disk that cannot be created is not a reason to refuse to boot:
            // the sandbox works without it, just without a warm store.
            Err(e) => warn!("shared {} disk unavailable: {e:#}", what.as_str()),
        }
    }
    (got, busy)
}

/// Free and total bytes on the filesystem holding `path`.
///
/// The binding constraint for everything here, and easy to miss: the rootfs,
/// the home volumes and the workspaces are host directories with no size of
/// their own, and a *sparse* disk's ceiling is only reachable while the host
/// still has room under it. A 64 GiB Docker disk on a host with 5 GiB free
/// fills after 5.
pub fn host_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` is a valid NUL-terminated path and `st` is fully written
        // by a successful statvfs; the return code is checked before it is read.
        unsafe {
            let mut st: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut st) != 0 {
                return None;
            }
            // `f_frsize` is the fragment size the block counts are in; `f_bsize`
            // is the preferred I/O size and is the wrong multiplier here.
            let unit = if st.f_frsize > 0 {
                st.f_frsize as u64
            } else {
                st.f_bsize as u64
            };
            // `f_bavail`, not `f_bfree`: the reserved blocks are not ours.
            Some((st.f_bavail as u64 * unit, st.f_blocks as u64 * unit))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Free space on the host filesystem the shared disks live on.
pub fn host_free() -> Option<(u64, u64)> {
    host_space(&dir().ok()?)
}

/// One shared disk, as reported by `bsdkrun ai disk`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub name: &'static str,
    pub guest_path: &'static str,
    pub path: String,
    pub exists: bool,
    /// Apparent size — the ceiling, since the image is sparse.
    pub size: u64,
    /// What it actually occupies on the host.
    pub used: u64,
    /// The running sandbox currently holding it, if any.
    pub held_by: Option<String>,
    /// Room left inside the disk: its ceiling minus what it has written.
    pub free: u64,
    /// What the disk can *actually* still grow into — the smaller of its own
    /// headroom and the host's free space. This is the number that runs out.
    pub effective_free: u64,
}

pub fn status() -> Result<Vec<Status>> {
    let host = host_free();
    let mut out = Vec::new();
    for &what in ALL {
        let path = what.image()?;
        let meta = path.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let used = allocated(&path).unwrap_or(0);
        let free = size.saturating_sub(used);
        out.push(Status {
            name: what.as_str(),
            guest_path: what.guest_path(),
            exists: meta.is_some(),
            size,
            used,
            free,
            effective_free: free.min(host.unwrap_or((free, 0)).0),
            held_by: holder(what),
            path: path.display().to_string(),
        });
    }
    Ok(out)
}

/// Bytes a sparse file actually occupies — the number that matters, since the
/// apparent size is a ceiling nobody paid for.
fn allocated(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // `blocks` is in 512-byte units by POSIX convention, whatever the
        // filesystem's own block size is.
        Some(std::fs::metadata(path).ok()?.blocks() * 512)
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path).ok().map(|m| m.len())
    }
}

/// What one sandbox occupies on the host, by where it lives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub running: bool,
    /// The per-sandbox rootfs — disposable; a fresh session rebuilds it.
    pub rootfs: u64,
    /// The agent's home volume, shared by every session of that agent.
    pub home: u64,
    pub workspace: Option<String>,
}

/// Per-sandbox usage.
///
/// The workspace is reported as a path and never measured: it is your own
/// project directory, its size is not something a sandbox did, and walking it
/// could take longer than the command.
pub fn usage() -> Result<Vec<Usage>> {
    let mut out = Vec::new();
    for s in super::sessions()? {
        let vdir = db::state_dir()?.join("machines").join(&s.id);
        let home = super::home_dir(&s.agent)
            .map(|p| crate::host::dir_size(&p))
            .unwrap_or(0);
        out.push(Usage {
            rootfs: crate::host::dir_size(&vdir),
            home,
            id: s.id,
            name: s.name,
            agent: s.agent,
            running: s.running,
            workspace: s.workspace,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_disks_round_trip_through_their_names() {
        for &w in ALL {
            assert_eq!(Shared::parse(w.as_str()).unwrap(), w);
        }
        assert!(Shared::parse("everything").is_err());
    }

    /// The mount points are the whole point: a Docker disk mounted anywhere
    /// but `/var/lib/docker` caches nothing.
    #[test]
    fn the_mount_points_are_the_real_store_paths() {
        assert_eq!(Shared::Docker.guest_path(), "/var/lib/docker");
        assert_eq!(Shared::Nix.guest_path(), "/nix");
    }

    #[test]
    fn dir_size_adds_up_and_survives_a_missing_directory() {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-disk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("a"), vec![0u8; 1000]).unwrap();
        std::fs::write(tmp.join("sub/b"), vec![0u8; 2000]).unwrap();
        assert_eq!(crate::host::dir_size(&tmp), 3000);
        assert_eq!(crate::host::dir_size(Path::new("/no/such/directory")), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
