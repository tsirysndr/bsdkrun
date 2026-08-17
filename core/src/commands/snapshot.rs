//! Machine snapshots: capture a machine's disk state under a name, boot a new
//! machine from that state (`branch`), or put it back (`restore`/`rollback`).
//!
//! A snapshot is a **copy-on-write clone**, not a memory image: libkrun has no
//! save-VM API, so what is captured is what the guest wrote, not what it was
//! thinking. The clone costs no disk until one side diverges (`clonefile` on
//! APFS, `--reflink` on btrfs/XFS), which is what makes branching cheap enough
//! to do on a whim.
//!
//! What "disk state" means depends on the guest, and [`Payload`] is the one
//! place that knows:
//!
//! | Guest             | State                                                |
//! | ----------------- | ---------------------------------------------------- |
//! | Linux (OCI)       | the writable rootfs tree, served over virtio-fs      |
//! | FreeBSD / NetBSD  | one raw disk image holding the whole UFS             |
//! | Unikraft          | the unikernel + cmdline, plus any `--mount` host dirs |
//!
//! The boot half of `branch` lives in [`crate::commands::boot`], for the same
//! reason the flavor boot code does: everything here has to keep working in a
//! build that links no hypervisor.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use crate::{db, host, id, unikraft};

use super::guest::guest_os_kind;
use super::machines::{fullsync_file, quiesce_guest_for_snapshot};
use super::{machine_rootfs_dir, volume_dir};

/// A machine's on-disk state, resolved to the files that hold it.
pub(crate) enum Payload {
    /// Linux: the writable rootfs tree the guest sees as `/`.
    Rootfs(PathBuf),
    /// FreeBSD / NetBSD: the raw root disk (`root.raw` / `root.img`).
    Disk(PathBuf),
    /// Unikraft: the unikernel image, its boot spec, and the host directories
    /// it mounts — the only part of a unikernel that a guest can write to.
    Unikernel {
        kernel: PathBuf,
        spec: unikraft::BootSpec,
    },
}

/// Validate a snapshot name (used as a DB key and shown everywhere).
fn valid_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!("invalid snapshot name {name:?} — use letters, digits, '-', '_' or '.'");
    }
    Ok(())
}

/// Where a machine keeps the state a snapshot captures.
///
/// The rootfs is resolved the same way the boot path resolves it — via
/// [`machine_rootfs_dir`], which on macOS points at the case-sensitive store —
/// so a snapshot never silently misses a rootfs that is simply not where the
/// state dir would suggest.
pub(crate) fn machine_payload(vm: &db::MachineRow) -> Result<Payload> {
    let mdir = PathBuf::from(&vm.state_dir);
    let kind = guest_os_kind(&vm.kind, &vm.image);

    if kind == "unikraft" {
        let spec = unikraft::BootSpec::load(&mdir).with_context(|| {
            format!(
                "machine {} has no recorded unikernel to snapshot",
                short(&vm.id)
            )
        })?;
        return Ok(Payload::Unikernel {
            kernel: spec.kernel.clone(),
            spec,
        });
    }

    if kind == "linux" {
        let rootfs = match &vm.volume {
            Some(v) => volume_dir(v)?.join("rootfs"),
            None => machine_rootfs_dir(&vm.id, &mdir).join("rootfs"),
        };
        if !rootfs.exists() {
            anyhow::bail!("machine {} has no rootfs to snapshot", short(&vm.id));
        }
        return Ok(Payload::Rootfs(rootfs));
    }

    let base = match &vm.volume {
        Some(v) => volume_dir(v)?,
        None => mdir,
    };
    let disk = ["root.raw", "root.img"]
        .iter()
        .map(|f| base.join(f))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("machine {} has no disk to snapshot", short(&vm.id)))?;
    Ok(Payload::Disk(disk))
}

/// Where a *snapshot* keeps the same state, so `branch` and `restore` read it
/// back the way `create` wrote it.
pub(crate) fn snapshot_payload(snap: &db::SnapshotRow) -> Result<Payload> {
    let dir = PathBuf::from(&snap.path);
    if snap.kind == "unikraft" {
        let spec = unikraft::BootSpec::load(&dir)
            .with_context(|| format!("snapshot {:?} is missing its unikernel spec", snap.name))?;
        return Ok(Payload::Unikernel {
            kernel: spec.kernel.clone(),
            spec,
        });
    }
    if snap.kind == "linux" {
        let rootfs = dir.join("rootfs");
        if !rootfs.exists() {
            anyhow::bail!("snapshot {:?} is missing its rootfs data", snap.name);
        }
        return Ok(Payload::Rootfs(rootfs));
    }
    let disk = ["disk.raw", "disk.img"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("snapshot {:?} is missing its disk data", snap.name))?;
    Ok(Payload::Disk(disk))
}

/// The file whose filesystem decides where the snapshot goes: the big one, so
/// a unikernel's small mounts follow its image rather than the other way round.
fn payload_source(payload: &Payload) -> &Path {
    match payload {
        Payload::Rootfs(p) | Payload::Disk(p) => p,
        Payload::Unikernel { kernel, .. } => kernel,
    }
}

/// Where a snapshot of `src` should live.
///
/// On the same filesystem as the data it clones, whenever one of our state
/// directories is: a snapshot taken across volumes is not a snapshot at all but
/// a byte-for-byte copy (`clonefile` fails `EXDEV`), which for a nix rootfs is
/// the difference between 50 ms and several minutes. On macOS that means the
/// case-sensitive store for a Linux rootfs, and `<state>` for a BSD disk.
fn snapshot_dir(src: &Path, id: &str) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(d) = crate::store::snapshots_dir() {
        candidates.push(d);
    }
    candidates.push(db::snapshots_dir()?);
    let base = candidates
        .iter()
        .find(|c| host::same_device(src, c))
        .unwrap_or(&candidates[candidates.len() - 1]);
    Ok(base.join(id))
}

/// Copy a machine's state into a snapshot directory.
fn capture(payload: &Payload, dir: &Path) -> Result<()> {
    match payload {
        Payload::Rootfs(src) => {
            // Push the guest's just-synced writes out of the host page cache:
            // virtio-fs passes them through to these files, and the clone is
            // extent-level, so an unflushed write is simply not in the snapshot.
            let _ = std::process::Command::new("sync").status();
            host::clone_or_copy_tree(src, &dir.join("rootfs"))
        }
        Payload::Disk(src) => {
            // Same trap, one level down: libkrun's virtio-blk writes sit in the
            // page cache, and `clonefile` shares on-disk extents — so without a
            // full fsync the snapshot silently misses recent changes even
            // though the file *reads* current.
            fullsync_file(src);
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("img");
            host::clone_or_copy_file(src, &dir.join(format!("disk.{ext}")))
        }
        Payload::Unikernel { kernel, spec } => {
            let _ = std::process::Command::new("sync").status();
            // Keep our own copy of the unikernel: the path the machine booted
            // from is the user's build directory, and a `kraft build` (or an
            // `rm -rf`) later would leave the snapshot unbootable.
            let kdst = dir.join("kernel");
            host::clone_or_copy_file(kernel, &kdst)
                .with_context(|| format!("copying unikernel {}", kernel.display()))?;
            let mut saved = unikraft::BootSpec {
                kernel: kdst,
                cmdline: spec.cmdline.clone(),
                initramfs: spec.initramfs.clone(),
                volumes: vec![],
            };
            // Each mounted host dir is guest-writable state, so it belongs in
            // the snapshot; the guest path stays as it was, only the host side
            // moves into the snapshot dir.
            for (i, v) in spec.volumes.iter().enumerate() {
                let dst = dir.join("volumes").join(i.to_string());
                std::fs::create_dir_all(dst.parent().unwrap())?;
                if v.host.exists() {
                    host::clone_or_copy_tree(&v.host, &dst).with_context(|| {
                        format!("copying mounted directory {}", v.host.display())
                    })?;
                } else {
                    std::fs::create_dir_all(&dst)?;
                }
                saved.volumes.push(unikraft::Volume {
                    host: dst,
                    guest: v.guest.clone(),
                });
            }
            saved.save(dir)
        }
    }
}

/// Put a snapshot's state back where a machine boots from, replacing what is
/// there. The machine must not be running.
fn write_back(payload: &Payload, vm: &db::MachineRow) -> Result<()> {
    let mdir = PathBuf::from(&vm.state_dir);
    match payload {
        Payload::Rootfs(src) => {
            let dst = match &vm.volume {
                Some(v) => volume_dir(v)?.join("rootfs"),
                None => machine_rootfs_dir(&vm.id, &mdir).join("rootfs"),
            };
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Rename-aside + background GC: `rm -rf` of a read-only nix rootfs
            // takes long enough that a restore would look hung.
            host::remove_dir_all_detached(&dst);
            host::clone_or_copy_tree(src, &dst)
        }
        Payload::Disk(src) => {
            let base = match &vm.volume {
                Some(v) => volume_dir(v)?,
                None => mdir,
            };
            // Restore under the name the machine already boots from, so `start`
            // finds it without any change to the DB row.
            let dst = ["root.raw", "root.img"]
                .iter()
                .map(|f| base.join(f))
                .find(|p| p.exists())
                .unwrap_or_else(|| {
                    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("img");
                    base.join(format!("root.{ext}"))
                });
            std::fs::create_dir_all(&base)?;
            let _ = std::fs::remove_file(&dst);
            host::clone_or_copy_file(src, &dst)
        }
        Payload::Unikernel { kernel, spec } => {
            // The machine's mounts point at the user's own directories; putting
            // a snapshot back means overwriting those, which is the whole point
            // of `restore` — but only for the mounts the snapshot has.
            let live = unikraft::BootSpec::load(&mdir).ok();
            let mut restored = unikraft::BootSpec {
                kernel: kernel.clone(),
                cmdline: spec.cmdline.clone(),
                initramfs: spec.initramfs.clone(),
                volumes: vec![],
            };
            for (i, v) in spec.volumes.iter().enumerate() {
                // Prefer the machine's current host dir (restore in place);
                // fall back to the snapshot's own copy when the machine no
                // longer records that mount.
                let dst = live
                    .as_ref()
                    .and_then(|l| l.volumes.get(i))
                    .map(|l| l.host.clone());
                match dst {
                    Some(dst) => {
                        host::remove_dir_all_detached(&dst);
                        host::clone_or_copy_tree(&v.host, &dst).with_context(|| {
                            format!("restoring mounted directory {}", dst.display())
                        })?;
                        restored.volumes.push(unikraft::Volume {
                            host: dst,
                            guest: v.guest.clone(),
                        });
                    }
                    None => restored.volumes.push(v.clone()),
                }
            }
            restored.save(&mdir)
        }
    }
}

/// The first free `<machine><suffix>-<n>`, so `bsdkrun snapshot web` never has
/// to be given a name to be useful.
fn auto_name(db: &db::Db, vm: &db::MachineRow, suffix: &str) -> String {
    let base: String = vm
        .name
        .clone()
        .unwrap_or_else(|| short(&vm.id).to_string())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let base = if base.is_empty() {
        format!("snapshot{suffix}")
    } else {
        format!("{base}{suffix}")
    };
    for n in 1.. {
        let candidate = format!("{base}-{n}");
        if db.find_snapshot(&candidate).ok().flatten().is_none() {
            return candidate;
        }
    }
    unreachable!("the loop returns on the first free name")
}

/// A machine id, shortened for messages the way Docker shortens one.
fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// `bsdkrun snapshot <id> [name]` — capture a machine's state under a name.
///
/// The guest is quiesced first (a Linux guest is flushed; a BSD guest is
/// powered off, because a live UFS cannot be cloned consistently — see
/// [`quiesce_guest_for_snapshot`]), so a BSD machine is left **stopped**.
pub fn create(machine: &str, name: Option<&str>, description: &str) -> Result<db::SnapshotRow> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;

    let name = match name {
        Some(n) => {
            valid_name(n)?;
            n.to_string()
        }
        None => auto_name(&db, &vm, ""),
    };
    if db.find_snapshot(&name)?.is_some() {
        anyhow::bail!("snapshot {name:?} already exists (remove it, or pick another name)");
    }

    // Resolve the state BEFORE touching the guest: a machine with nothing to
    // snapshot should fail without having been powered off first.
    let payload = machine_payload(&vm)?;
    quiesce_guest_for_snapshot(&vm);
    // The quiesce may have powered a BSD guest off and written new blocks; the
    // payload paths are unchanged, but re-read the spec so a unikernel snapshot
    // picks up whatever the guest last wrote to its mounts.
    let payload = machine_payload(&vm).unwrap_or(payload);

    let sid = id::short_id();
    let dir = snapshot_dir(payload_source(&payload), &sid)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating snapshot dir {}", dir.display()))?;

    if let Err(e) = capture(&payload, &dir) {
        host::force_remove_dir_all(&dir);
        return Err(e);
    }

    let row = db::SnapshotRow {
        id: sid,
        name,
        machine_id: vm.id.clone(),
        machine_name: vm.name.clone().unwrap_or_default(),
        // Store the guest OS, not the boot mode (`firmware`/`kernel`), so the
        // UIs label it and `branch` boots it without re-deriving anything.
        kind: guest_os_kind(&vm.kind, &vm.image).to_string(),
        image: vm.image.clone(),
        path: dir.to_string_lossy().into_owned(),
        // A snapshot of a branched machine records where that branch came from,
        // which is what lets the UIs draw a lineage rather than a flat list.
        parent: vm.origin.clone(),
        description: description.to_string(),
        cpus: vm.cpus,
        mem: vm.mem,
        ports: vm.ports.clone(),
        created_at: String::new(), // set by the insert
    };
    db.upsert_snapshot(&row)?;
    info!(snapshot = %row.name, machine = %short(&vm.id), "snapshot created");
    Ok(row)
}

/// Every snapshot, or one machine's, newest first.
pub fn list(machine: Option<&str>) -> Result<Vec<db::SnapshotRow>> {
    let db = db::Db::open()?;
    match machine {
        Some(m) => {
            let vm = db.find_machine(m)?;
            db.machine_snapshots(&vm.id)
        }
        None => db.list_snapshots(),
    }
}

/// Look up a snapshot by name, id, or unique id prefix.
pub fn find(key: &str) -> Result<db::SnapshotRow> {
    db::Db::open()?
        .find_snapshot(key)?
        .ok_or_else(|| anyhow::anyhow!("no such snapshot: {key} (see `bsdkrun snapshots`)"))
}

/// Remove a snapshot and its data.
pub fn remove(key: &str) -> Result<String> {
    let db = db::Db::open()?;
    let snap = db
        .find_snapshot(key)?
        .ok_or_else(|| anyhow::anyhow!("no such snapshot: {key}"))?;
    // Rename-aside + background GC, so removing a multi-GiB rootfs snapshot
    // returns immediately instead of making the UI's delete button hang.
    host::remove_dir_all_detached(&PathBuf::from(&snap.path));
    db.remove_snapshot(&snap.id)?;
    Ok(snap.name)
}

/// What a restore did, so a caller can report it without re-querying.
pub struct Restored {
    pub machine: String,
    pub snapshot: String,
    /// The automatic safety snapshot taken before overwriting, if any.
    pub backup: Option<String>,
    /// Whether the machine was running and had to be stopped.
    pub stopped: bool,
}

/// `bsdkrun restore <id> <snapshot>` — put a machine's state back to a snapshot.
///
/// The machine must be stopped: a running guest holds the very files being
/// replaced. `force` stops it first, the way `rm -f` does. Unless `backup` is
/// false, the state being overwritten is itself snapshotted first — it is a
/// CoW clone, so the safety net is free, and without it a mistyped restore is
/// unrecoverable.
pub fn restore(machine: &str, snapshot: &str, force: bool, backup: bool) -> Result<Restored> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;
    let snap = db
        .find_snapshot(snapshot)?
        .ok_or_else(|| anyhow::anyhow!("no such snapshot: {snapshot} (see `bsdkrun snapshots`)"))?;

    // Restoring one guest's disk into another's is not a mistake we can make
    // safe: a FreeBSD UFS image is not a Linux rootfs tree.
    let vm_kind = guest_os_kind(&vm.kind, &vm.image);
    if snap.kind != vm_kind {
        anyhow::bail!(
            "snapshot {:?} is a {} snapshot, but {} is a {} machine",
            snap.name,
            snap.kind,
            short(&vm.id),
            vm_kind
        );
    }

    let running = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
    if running && !force {
        anyhow::bail!(
            "machine {} is running (stop it first, or use -f)",
            short(&vm.id)
        );
    }
    if running {
        // A clean stop, not a kill: a BSD guest's UFS has to be unmounted, and
        // a Linux guest's writes have to reach the host files we are about to
        // replace — otherwise the ones still in flight land on top of the
        // restored tree.
        super::machines::stop(&vm.id)?;
    }

    let backup = if backup {
        let name = auto_name(&db, &vm, "-pre-restore");
        match create(
            &vm.id,
            Some(&name),
            &format!("state before restoring {}", snap.name),
        ) {
            Ok(b) => Some(b.name),
            // A machine with nothing to back up (a fresh branch, say) should
            // still be restorable; the restore itself is what matters.
            Err(e) => {
                tracing::warn!("could not take a safety snapshot before restoring: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let payload = snapshot_payload(&snap)?;
    write_back(&payload, &vm)?;
    // A restored BSD machine boots the restored disk, which may have come from
    // a different image than the row records; keep the row pointing at what is
    // actually on disk so `start` picks the right guest.
    if vm.image != snap.image {
        db.set_machine_image(&vm.id, &snap.image).ok();
    }
    info!(machine = %short(&vm.id), snapshot = %snap.name, "restored");
    Ok(Restored {
        machine: vm.id,
        snapshot: snap.name,
        backup,
        stopped: running,
    })
}

/// `bsdkrun rollback <id>` — restore a machine to its most recent snapshot.
///
/// The common case of [`restore`]: "undo whatever I just did", without having
/// to look up what the last snapshot was called.
pub fn rollback(machine: &str, force: bool, backup: bool) -> Result<Restored> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;
    let latest = db
        .machine_snapshots(&vm.id)?
        .into_iter()
        // A safety snapshot taken by a previous restore is a rollback target
        // like any other: rolling back twice should walk back two steps.
        .next()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "machine {} has no snapshots to roll back to (take one with `bsdkrun snapshot {}`)",
                short(&vm.id),
                short(&vm.id)
            )
        })?;
    restore(&vm.id, &latest.id, force, backup)
}

// ---------------------------------------------------------------------------
// printing
// ---------------------------------------------------------------------------

pub(crate) fn cmd_create(
    machine: &str,
    name: Option<&str>,
    description: &str,
    json: bool,
) -> Result<()> {
    let row = create(machine, name, description)?;
    if json {
        println!("{}", serde_json::to_string(&crate::api::snapshot(&row))?);
    } else {
        println!("{}", row.name);
    }
    Ok(())
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_ls(machine: Option<&str>, json: bool) -> Result<()> {
    let rows = list(machine)?;
    if json {
        let out: Vec<_> = rows.iter().map(crate::api::snapshot).collect();
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No snapshots yet — take one with `bsdkrun snapshot <machine> [name]`.");
        return Ok(());
    }
    println!(
        "{:<20}  {:<14}  {:<20}  {:<9}  {:<12}  {}",
        "NAME", "ID", "MACHINE", "GUEST", "CREATED", "DESCRIPTION"
    );
    for s in rows {
        let machine = if s.machine_name.is_empty() {
            short(&s.machine_id).to_string()
        } else {
            s.machine_name.clone()
        };
        println!(
            "{:<20}  {:<14}  {:<20}  {:<9}  {:<12}  {}",
            super::truncate(&s.name, 20),
            s.id,
            super::truncate(&machine, 20),
            s.kind,
            db::age(&s.created_at),
            super::truncate(&s.description, 40)
        );
    }
    Ok(())
}

pub(crate) fn cmd_rm(keys: &[String]) -> Result<()> {
    let mut failed = false;
    for key in keys {
        match remove(key) {
            Ok(name) => println!("{name}"),
            Err(e) => {
                eprintln!("Error: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn cmd_restore(machine: &str, snapshot: &str, force: bool, backup: bool) -> Result<()> {
    report(restore(machine, snapshot, force, backup)?)
}

pub(crate) fn cmd_rollback(machine: &str, force: bool, backup: bool) -> Result<()> {
    report(rollback(machine, force, backup)?)
}

/// A restore leaves the machine stopped, so say so — and say how to start it,
/// since that is always the next thing the user wants.
fn report(r: Restored) -> Result<()> {
    println!("{} restored to {}", short(&r.machine), r.snapshot);
    if let Some(b) = &r.backup {
        println!("  previous state saved as {b}");
    }
    println!("  start it with: bsdkrun start {}", short(&r.machine));
    Ok(())
}
