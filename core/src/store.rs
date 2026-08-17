//! Case-sensitive backing store for guest rootfs trees. **macOS only** — this
//! module is not compiled on other hosts.
//!
//! macOS formats the boot volume as **case-insensitive** APFS. That is fine for
//! most guests, but it silently corrupts Linux trees whose paths differ only by
//! case. That includes nix (`Pod/` and `pod/`) and Linux kernel sources
//! (`ipt_ECN.h` and `ipt_ecn.h`). Extracting or building into a case-insensitive
//! directory collapses them and produces confusing failures such as
//!
//! ```text
//! error: creating directory ".../perl-5.42.0/lib/perl5/5.42.0/pod": File exists
//! ```
//!
//! The fix is a real case-sensitive filesystem. We keep one as an **APFS
//! sparsebundle** — a disk image that needs no admin rights, allocates blocks
//! only as they are used, and is removed by deleting a single directory. It is
//! attached at `<cache>/store` and holds both trees the guest writes into:
//!
//! ```text
//! <cache>/store.sparsebundle     the image file (Case-sensitive APFS)
//! <cache>/store/                 its mountpoint
//! <cache>/store/oci/             extracted base-image rootfs trees
//! <cache>/store/volumes/         named persistent volumes
//! ```
//!
//! Both live on the *same* volume deliberately: `--volume` CoW-clones a base
//! rootfs with `clonefile`, which fails `EXDEV` across volumes and would fall
//! back to copying hundreds of megabytes.
//!
//! Linux hosts need none of this — ext4/xfs/btrfs are case-sensitive already —
//! so the module is `#[cfg]`'d out there, along with the `store` subcommand,
//! and the historical cache/state paths are used unchanged.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

/// Default capacity for a new sparsebundle. This is a *ceiling*, not an
/// allocation — a fresh 200 GiB bundle occupies ~24 MB on disk — so it is sized
/// generously: a nix build of a moderate Rust project can pull tens of GB of
/// store paths, and growing later means a separate `hdiutil resize`.
pub const DEFAULT_SIZE: &str = "200g";

/// Path of the sparsebundle image itself.
pub fn bundle_path() -> Result<PathBuf> {
    Ok(crate::fetch::cache_dir()?.join("store.sparsebundle"))
}

/// Mountpoint the sparsebundle is attached at.
pub fn root() -> Result<PathBuf> {
    Ok(crate::fetch::cache_dir()?.join("store"))
}

/// Does `dir` sit on a case-sensitive filesystem? Probes by creating a file and
/// asking for it back under a different case — the only reliable answer, since
/// case-sensitivity is a per-volume format choice, not something `statfs`
/// reports portably. A directory we cannot write to is reported as
/// case-insensitive so callers stay on the safe path.
pub fn is_case_sensitive(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".bsdkrun-case-probe-A");
    let other = dir.join(".bsdkrun-case-probe-a");
    // Clear both spellings first: a leftover lowercase probe from an earlier run
    // would make a case-sensitive volume look insensitive.
    let _ = std::fs::remove_file(&probe);
    let _ = std::fs::remove_file(&other);
    if std::fs::write(&probe, b"").is_err() {
        return false;
    }
    let sensitive = !other.exists();
    let _ = std::fs::remove_file(&probe);
    sensitive
}

/// Is the store present (the bundle exists) — regardless of whether it is
/// currently attached?
pub fn exists() -> bool {
    bundle_path().map(|p| p.is_dir()).unwrap_or(false)
}

/// Is the store attached *and* actually case-sensitive right now? Both halves
/// matter: an unattached mountpoint is just an empty dir on the boot volume,
/// which would quietly reintroduce the bug we are avoiding.
pub fn is_active() -> bool {
    match root() {
        Ok(r) => is_mounted(&r) && is_case_sensitive(&r),
        Err(_) => false,
    }
}

/// Is `path` a mountpoint? Compares its device id with its parent's — a mounted
/// filesystem is by definition on a different device than the directory it
/// covers.
fn is_mounted(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(me), Some(parent)) = (path.metadata(), path.parent()) else {
        return false;
    };
    match parent.metadata() {
        Ok(p) => me.dev() != p.dev(),
        Err(_) => false,
    }
}

/// Directory for extracted OCI rootfs trees: inside the store when it is
/// active, else the historical `<cache>/oci`.
pub fn oci_dir() -> Result<PathBuf> {
    if is_active() {
        return Ok(root()?.join("oci"));
    }
    Ok(crate::fetch::cache_dir()?.join("oci"))
}

/// Directory for named persistent volumes: inside the store when it is active,
/// else the historical `<state>/volumes`.
pub fn volumes_dir() -> Result<PathBuf> {
    if is_active() {
        return Ok(root()?.join("volumes"));
    }
    crate::db::volumes_dir_default()
}

/// Where machine snapshots live when the store is active.
///
/// A snapshot is a `clonefile` of a machine's rootfs, and `clonefile` fails
/// `EXDEV` across volumes — a snapshot taken into `<state>/snapshots` on the
/// boot volume would silently degrade to a byte-for-byte copy of a multi-GiB
/// nix rootfs. Keeping snapshots on the same volume as the rootfs keeps them
/// instant and free.
pub fn snapshots_dir() -> Option<PathBuf> {
    if !is_active() {
        return None;
    }
    let dir = root().ok()?.join("snapshots");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Where a machine's writable rootfs clone lives when the store is active.
///
/// Without `--volume` the guest's root is a per-machine CoW clone of the base
/// image, and it is the tree the guest actually writes into — so it needs the
/// case-sensitive volume just as much as the image cache does. Keeping it here
/// rather than under `<state>/machines/<id>` also keeps the clone *intra*-volume,
/// so `clonefile` still works and a fresh machine costs no extra disk; cloning
/// across to the boot volume would copy the whole rootfs byte for byte.
///
/// Only the rootfs moves. Console logs and sockets stay in the machine's state
/// dir, which is where the rest of bsdkrun expects them.
pub fn machine_rootfs_dir(id: &str) -> Option<PathBuf> {
    if !is_active() {
        return None;
    }
    let dir = root().ok()?.join("machines").join(id);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Drop a machine's rootfs from the store. Paired with removing its state dir —
/// otherwise `bsdkrun rm` would leave the (large) clone orphaned on the store.
pub fn remove_machine_rootfs(id: &str) {
    if !is_active() {
        return;
    }
    if let Ok(r) = root() {
        crate::host::force_remove_dir_all(&r.join("machines").join(id));
    }
}

/// Attach the sparsebundle at its mountpoint. No-op when already attached.
pub fn attach() -> Result<()> {
    let (bundle, mnt) = (bundle_path()?, root()?);
    if is_mounted(&mnt) {
        return Ok(());
    }
    if !bundle.is_dir() {
        bail!(
            "no case-sensitive store at {} — create one with `bsdkrun store init`",
            bundle.display()
        );
    }
    std::fs::create_dir_all(&mnt).with_context(|| format!("creating {}", mnt.display()))?;
    // -nobrowse keeps it out of Finder's sidebar (it is bsdkrun's plumbing, not
    // a user disk); -noverify skips the checksum pass, which on a multi-GB
    // bundle would add seconds to every command that auto-attaches.
    let out = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-noverify", "-quiet", "-mountpoint"])
        .arg(&mnt)
        .arg(&bundle)
        .output()
        .context("running hdiutil attach")?;
    if !out.status.success() {
        bail!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Attach the store if it exists but is not mounted. Called on every startup so
/// the store survives a reboot without the user thinking about it; silent on
/// success and merely a warning on failure, since most commands do not need it.
pub fn auto_attach() {
    if !exists() {
        return;
    }
    let Ok(mnt) = root() else { return };
    if is_mounted(&mnt) {
        return;
    }
    if let Err(e) = attach() {
        warn!("could not attach the case-sensitive store: {e:#}");
    }
}

/// Ensure Linux OCI trees have case-sensitive backing storage.
///
/// The default macOS boot volume is usually case-insensitive. OCI images and
/// guest workloads can contain names that differ only by case (the Linux
/// kernel has several), so silently falling back to the historical cache can
/// corrupt a checkout. Create the sparse case-sensitive store automatically on
/// first Linux use. Existing stores are attached and verified.
pub fn ensure_linux_storage() -> Result<()> {
    let cache = crate::fetch::cache_dir()?;
    let state = crate::db::state_dir()?;
    if is_case_sensitive(&cache) && is_case_sensitive(&state) {
        return Ok(());
    }
    if exists() {
        attach()?;
        if is_active() {
            return Ok(());
        }
        bail!("the bsdkrun store is attached but is not case-sensitive");
    }

    ensure_no_running_machines()?;
    eprintln!(
        "initializing case-sensitive Linux storage at {} ({} sparse capacity)",
        bundle_path()?.display(),
        DEFAULT_SIZE
    );
    create(DEFAULT_SIZE)?;
    migrate()?;
    eprintln!(
        "case-sensitive Linux storage ready at {}",
        root()?.display()
    );
    Ok(())
}

/// Storage migration must never move a volume that a live VM is using.
pub(crate) fn ensure_no_running_machines() -> Result<()> {
    let running = crate::db::Db::open()
        .and_then(|db| db.list_machines())
        .map(|machines| {
            machines
                .iter()
                .filter(|machine| {
                    machine.status == "running"
                        && machine.pid.map(crate::db::pid_alive).unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    if running > 0 {
        bail!(
            "{running} machine(s) still running — stop them before bsdkrun initializes its \
             case-sensitive Linux store"
        );
    }
    Ok(())
}

/// Detach the sparsebundle. `force` unmounts even with open files.
pub fn detach(force: bool) -> Result<()> {
    let mnt = root()?;
    if !is_mounted(&mnt) {
        return Ok(());
    }
    let mut cmd = Command::new("hdiutil");
    cmd.arg("detach").arg(&mnt).arg("-quiet");
    if force {
        cmd.arg("-force");
    }
    let out = cmd.output().context("running hdiutil detach")?;
    if !out.status.success() {
        bail!(
            "hdiutil detach failed: {} (a running machine may still be using it — \
             stop it, or retry with --force)",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Create the sparsebundle and attach it. Errors if one already exists so an
/// accidental re-run can never discard a populated store.
pub fn create(size: &str) -> Result<()> {
    let bundle = bundle_path()?;
    if bundle.is_dir() {
        bail!(
            "a store already exists at {} — remove it with `bsdkrun store rm` first",
            bundle.display()
        );
    }
    std::fs::create_dir_all(crate::fetch::cache_dir()?)?;
    let out = Command::new("hdiutil")
        .args(["create", "-size", size, "-type", "SPARSEBUNDLE"])
        .args(["-fs", "Case-sensitive APFS"])
        .args(["-volname", "bsdkrun-store", "-quiet"])
        .arg(&bundle)
        .output()
        .context("running hdiutil create")?;
    if !out.status.success() {
        bail!(
            "hdiutil create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    attach()?;
    // Verify rather than trust: if the volume came back case-insensitive the
    // store is useless, and failing here beats corrupting a nix store later.
    let mnt = root()?;
    if !is_case_sensitive(&mnt) {
        bail!(
            "the new store at {} is not case-sensitive — refusing to use it",
            mnt.display()
        );
    }
    std::fs::create_dir_all(mnt.join("oci"))?;
    std::fs::create_dir_all(mnt.join("volumes"))?;
    Ok(())
}

/// Delete the store entirely (detaching first).
pub fn remove(force: bool) -> Result<()> {
    detach(force)?;
    let bundle = bundle_path()?;
    if bundle.is_dir() {
        std::fs::remove_dir_all(&bundle)
            .with_context(|| format!("removing {}", bundle.display()))?;
    }
    let mnt = root()?;
    // Only the (now empty) mountpoint is left; remove_dir refuses if attaching
    // silently failed and real files are sitting there.
    let _ = std::fs::remove_dir(&mnt);
    Ok(())
}

/// Move existing named volumes onto the store.
///
/// Volumes hold guest state the user expects to keep, so they are copied.
/// The OCI cache deliberately is **not** migrated: every tree in it was
/// extracted on a case-insensitive filesystem and may already have had
/// case-colliding paths merged into one, so copying it would carry that
/// corruption onto the new volume. It is a cache — dropping it just means the
/// next run re-pulls, this time correctly.
pub fn migrate() -> Result<()> {
    let mnt = root()?;
    let old_volumes = crate::db::volumes_dir_default()?;
    let new_volumes = mnt.join("volumes");
    std::fs::create_dir_all(&new_volumes)?;
    if old_volumes.is_dir() {
        for entry in std::fs::read_dir(&old_volumes)? {
            let entry = entry?;
            let dst = new_volumes.join(entry.file_name());
            if dst.exists() {
                continue;
            }
            info!(volume = %entry.file_name().to_string_lossy(), "migrating volume onto the store");
            // Always cross-volume, so skip the clone attempt entirely.
            crate::host::plain_copy_tree(&entry.path(), &dst)?;
        }
        crate::host::force_remove_dir_all(&old_volumes);
    }

    let old_oci = crate::fetch::cache_dir()?.join("oci");
    if old_oci.is_dir() {
        info!("dropping the old image cache (extracted case-insensitively; images will re-pull)");
        crate::host::force_remove_dir_all(&old_oci);
    }
    Ok(())
}

/// One-line human summary used by `bsdkrun store status`.
pub fn describe() -> Result<String> {
    let (bundle, mnt) = (bundle_path()?, root()?);
    if !bundle.is_dir() {
        let cache = crate::fetch::cache_dir()?;
        let state = crate::db::state_dir()?;
        return Ok(format!(
            "no store — image cache {} is {}; machine state {} is {}",
            cache.display(),
            if is_case_sensitive(&cache) {
                "case-sensitive"
            } else {
                "case-INSENSITIVE"
            },
            state.display(),
            if is_case_sensitive(&state) {
                "case-sensitive"
            } else {
                "case-INSENSITIVE (Linux guests require the sparse store)"
            }
        ));
    }
    if !is_mounted(&mnt) {
        return Ok(format!(
            "store exists at {} but is not attached — run `bsdkrun store attach`",
            bundle.display()
        ));
    }
    Ok(format!(
        "attached at {} ({}, on-disk {})",
        mnt.display(),
        if is_case_sensitive(&mnt) {
            "case-sensitive"
        } else {
            "case-INSENSITIVE — unexpected"
        },
        bundle_size(&bundle)
    ))
}

/// Actual space the sparsebundle occupies (not its capacity ceiling).
fn bundle_size(bundle: &Path) -> String {
    Command::new("du")
        .args(["-sh"])
        .arg(bundle)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_probe_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join("bsdkrun-store-probe-test");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = is_case_sensitive(&dir);
        assert!(!dir.join(".bsdkrun-case-probe-A").exists());
        assert!(!dir.join(".bsdkrun-case-probe-a").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dir_that_cannot_be_created_is_not_case_sensitive() {
        // /dev/null is a file, so create_dir_all beneath it must fail — the
        // probe has to report "insensitive" rather than panic.
        assert!(!is_case_sensitive(Path::new("/dev/null/nope")));
    }

    #[test]
    fn an_unattached_mountpoint_is_not_a_mountpoint() {
        // A plain directory on the boot volume shares its parent's device id,
        // so it must never be mistaken for an attached store.
        let dir = std::env::temp_dir().join("bsdkrun-store-notmnt");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_mounted(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
