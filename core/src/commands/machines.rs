//! The machine lifecycle: `ps`, `start`, `stop`, `update`, `rm` and `commit`.

use anyhow::{Context, Result};
use tracing::info;

#[cfg(target_os = "macos")]
use crate::store;
use crate::{agent, api, db, host};

use super::flavor::valid_flavor_name;
use super::guest::{guest_os_kind, reject_unikraft};
use super::truncate;
use super::volume_dir;

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_ps(all: bool, json: bool) -> Result<()> {
    // The listing (and the reconciliation of stale "running" rows) lives in
    // `api` so the daemon gets the same answer; here it is only rendered.
    let machines = api::list_machines(all)?;
    if json {
        println!("{}", serde_json::to_string(&machines)?);
        return Ok(());
    }
    println!(
        "{:<14}  {:<20}  {:<22}  {:<24}  {:<15}  {:<24}  {}",
        "ID", "NAME", "IMAGE", "STATUS", "CREATED", "PORTS", "COMMAND"
    );
    for m in machines {
        let created = m.created_at.unwrap_or_default();
        // Docker-style STATUS: "Up 5 minutes" / "Exited (0) 3 minutes ago".
        let status = if m.running {
            format!("Up {}", db::human_duration_since(&created))
        } else {
            let dur = m.finished_at.as_deref().map(db::human_duration_since);
            match (m.exit_code, dur) {
                (Some(c), Some(d)) => format!("Exited ({c}) {d} ago"),
                (Some(c), None) => format!("Exited ({c})"),
                (None, Some(d)) => format!("Exited {d} ago"),
                (None, None) => "Exited".to_string(),
            }
        };
        let ports = if m.ports.is_empty() {
            "-".to_string()
        } else {
            m.ports
                .iter()
                .map(|p| format!("{}:{}->{}/tcp", p.bind, p.host, p.guest))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{:<14}  {:<20}  {:<22}  {:<24}  {:<15}  {:<24}  {}",
            m.id,
            truncate(m.name.as_deref().unwrap_or("-"), 20),
            truncate(&m.image, 22),
            status,
            format!("{} ago", db::human_duration_since(&created)),
            truncate(&ports, 24),
            truncate(&m.command, 40)
        );
    }
    Ok(())
}

/// `bsdkrun stop <id>` — stop a running machine.
pub(crate) fn cmd_stop(id: &str) -> Result<()> {
    println!("{}", stop(id)?);
    Ok(())
}

/// Stop a running machine, returning what happened, for the caller to report.
///
/// The message is the value rather than a `println!` so the daemon can send it
/// back over its own transport instead of writing it to the daemon's stdout.
pub fn stop(id: &str) -> Result<String> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    match vm.pid {
        Some(pid) if db::pid_alive(pid) => {
            // A BSD guest must be cleanly powered off (UFS unmounted) so its
            // in-place disk stays consistent and runtime changes survive the next
            // `start` — killing the VMM mid-write tears the live UFS, and fsck
            // then discards recent writes. Fall back to SIGTERM if the clean
            // poweroff can't run (no agent) or the guest doesn't halt in time.
            let clean = is_bsd_machine(&vm) && graceful_poweroff_bsd(&vm);
            let code = if clean {
                Some(0)
            } else {
                // A Linux guest's rootfs writes land on the host FS, but any
                // `--attach-disk` block device buffers dirty pages in the guest —
                // flush them before the VMM is killed so its ext4 stays intact.
                sync_attached_disks(&vm);
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                // The process exits 128+SIGTERM on our signal handler; record that
                // so `ps` shows a Docker-style "Exited (143)".
                vm.exit_code.or(Some(128 + libc::SIGTERM as i64))
            };
            db.set_machine_status(&vm.id, "exited", code).ok();
            Ok(vm.id)
        }
        _ => {
            db.set_machine_status(&vm.id, "exited", vm.exit_code).ok();
            Ok(format!("{} is not running", vm.id))
        }
    }
}

/// `bsdkrun update <id> [--cpus N] [--mem M]` — change a machine's recorded
/// vCPU / memory. libkrun fixes VM resources at boot, so the new values apply on
/// the next `start` (an in-place restart), not to a currently-running guest.
pub(crate) fn cmd_update(id: &str, cpus: Option<u8>, mem: Option<u32>) -> Result<()> {
    println!("{}", update(id, cpus, mem)?);
    Ok(())
}

/// Record new vCPU/memory for a machine. Takes effect on the next `start`,
/// since libkrun fixes a VM's resources at boot.
pub fn update(id: &str, cpus: Option<u8>, mem: Option<u32>) -> Result<String> {
    if cpus.is_none() && mem.is_none() {
        anyhow::bail!("nothing to update — pass --cpus and/or --mem");
    }
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let new_cpus = cpus.map(i64::from).unwrap_or(vm.cpus).clamp(1, 255);
    let new_mem = mem.map(i64::from).unwrap_or(vm.mem).max(64);
    db.set_machine_resources(&vm.id, new_cpus, new_mem)?;
    let running = vm.pid.map(db::pid_alive).unwrap_or(false) && vm.status == "running";
    if running {
        info!(
            id = %vm.id,
            "resources updated ({new_cpus} vCPU / {new_mem} MiB) — restart the machine to apply"
        );
    } else {
        info!(id = %vm.id, "resources updated ({new_cpus} vCPU / {new_mem} MiB)");
    }
    Ok(vm.id)
}

pub(crate) fn cmd_rm(ids: &[String], force: bool) -> Result<()> {
    for id in ids {
        println!("{}", remove_machine(id, force)?);
    }
    Ok(())
}

/// Remove one machine and its state. Refuses a running machine unless `force`,
/// which stops it first.
pub fn remove_machine(id: &str, force: bool) -> Result<String> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let running = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
    if running {
        if !force {
            anyhow::bail!("machine {} is running (stop it first, or use -f)", vm.id);
        }
        if let Some(pid) = vm.pid {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
    }
    // Remove the per-machine state dir (console log, sockets, CoW disk clone),
    // then drop the DB row. Rename-aside + background GC so a huge read-only
    // nix rootfs doesn't make `rm` (and the desktop's delete) hang.
    host::remove_dir_all_detached(&std::path::PathBuf::from(&vm.state_dir));
    // The rootfs clone may live on the case-sensitive store rather than in
    // the state dir; without this it would be orphaned there.
    #[cfg(target_os = "macos")]
    store::remove_machine_rootfs(&vm.id);
    db.delete_machine(&vm.id)?;
    // The machine's domain goes with it.
    crate::domains::sync_if_enabled();
    Ok(vm.id)
}

/// A default `LinuxArgs` for launching a flavor: image + resources + ports/env,
/// detached-with-no-command (persistent console) so the environment stays up.
#[allow(clippy::too_many_arguments)]
/// True for a BSD guest (FreeBSD/NetBSD), whose live UFS must be cleanly
/// unmounted before its disk is reused or cloned.
pub(crate) fn is_bsd_machine(vm: &db::MachineRow) -> bool {
    matches!(
        vm.kind.as_str(),
        "firmware" | "kernel" | "freebsd" | "netbsd"
    ) || vm.image.to_lowercase().starts_with("freebsd")
        || vm.image.to_lowercase().starts_with("netbsd")
}

/// Best-effort `sync` in a guest that has attached block disks, via its agent,
/// before the VMM is torn down. Journaled filesystems (ext4) then replay to a
/// consistent state even though the guest itself dies on SIGTERM.
pub(crate) fn sync_attached_disks(vm: &db::MachineRow) {
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    if super::load_attached_disks(&vdir).is_empty() {
        return;
    }
    let Some(port) = agent::read_port(&vdir) else {
        return;
    };
    let argv = ["sh".to_string(), "-c".to_string(), "sync".to_string()];
    let _ = agent::exec_quiet(port, &argv);
}

/// Cleanly power off a running BSD guest via its agent (`shutdown -p now`) and
/// wait for the VM process to exit, so its UFS is unmounted and the on-disk
/// image is left consistent. A live UFS killed mid-write is *torn* — the next
/// boot's `fsck` discards recent changes — and even a `sync` first isn't enough;
/// only a clean unmount is. Returns true if the guest powered off on its own.
/// Best-effort: false if it wasn't running, had no agent, or didn't exit in time.
pub(crate) fn graceful_poweroff_bsd(vm: &db::MachineRow) -> bool {
    let Some(pid) = vm.pid.filter(|p| db::pid_alive(*p)) else {
        return false;
    };
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let Some(port) = agent::read_port(&vdir) else {
        return false;
    };
    let argv = [
        "sh".to_string(),
        "-c".to_string(),
        "sync; nohup shutdown -p now >/dev/null 2>&1 & sleep 1".to_string(),
    ];
    let _ = agent::exec(port, &argv, &[], false);
    for _ in 0..80 {
        if !db::pid_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
}

/// Prepare a running guest for a consistent snapshot, via its agent.
///
/// - **Linux** (virtio-fs rootfs backed by host files): a `sync` flushes the
///   guest's page cache through to those host files — enough, since there's no
///   in-guest block filesystem to tear.
/// - **BSD** (a raw UFS disk): a mounted UFS is always flagged *dirty*, and a
///   running guest keeps writing, so cloning it yields a *torn* image that boots
///   into an `fsck` which discards recent changes — even a `sync` first isn't
///   enough (verified). The only reliable way to a clean, bootable image is a
///   **clean shutdown**, which unmounts UFS. So we power the guest off, wait for
///   it to exit, and snapshot the quiescent disk. The machine is left **stopped**
///   (start it again to resume).
///
/// Best-effort: a machine that's already stopped, or one without an agent, is
/// snapshotted as-is.
pub(crate) fn quiesce_guest_for_snapshot(vm: &db::MachineRow) {
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        return;
    }
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let Some(port) = agent::read_port(&vdir) else {
        tracing::warn!(
            id = %vm.id,
            "no guest agent to quiesce before snapshot — it may miss recent writes"
        );
        return;
    };

    if vm.kind == "linux" {
        let argv = ["sh".to_string(), "-c".to_string(), "sync; sync".to_string()];
        let _ = agent::exec(port, &argv, &[], false);
        info!(id = %vm.id, "flushed guest to disk before snapshot");
        return;
    }

    // BSD: clean poweroff so UFS is unmounted and the image is consistent.
    let _ = port; // readiness already checked; the helper re-reads the port
    info!(id = %vm.id, "powering off guest for a consistent BSD snapshot…");
    graceful_poweroff_bsd(vm);
    if let Ok(db) = db::Db::open() {
        let _ = db.set_machine_status(&vm.id, "exited", Some(0));
    }
    info!(
        id = %vm.id,
        "guest powered off — snapshotting its clean disk (machine left stopped; `start` to resume)"
    );
}

/// Force a file's dirty pages all the way to permanent storage, so a subsequent
/// APFS `clonefile` (which shares on-disk *extents*) captures the latest writes.
/// libkrun's virtio-blk writes land in the host page cache; a plain `sync(2)`
/// only *schedules* the flush, so we use `F_FULLFSYNC`, which macOS guarantees
/// hits the disk. Opening a second read fd is enough — fsync flushes the whole
/// inode's dirty pages, including the ones libkrun wrote through its own fd.
#[cfg(target_os = "macos")]
pub(crate) fn fullsync_file(path: &std::path::Path) {
    use std::os::unix::io::AsRawFd;
    if let Ok(f) = std::fs::File::open(path) {
        // SAFETY: fcntl(F_FULLFSYNC) on a valid fd is always safe.
        unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC);
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn fullsync_file(path: &std::path::Path) {
    use std::os::unix::io::AsRawFd;
    if let Ok(f) = std::fs::File::open(path) {
        // SAFETY: fsync on a valid fd is always safe.
        unsafe {
            libc::fsync(f.as_raw_fd());
        }
    }
}

/// `bsdkrun commit <id> <name>` — snapshot a machine's current rootfs/disk into a
/// named flavor (a CoW clone), like `docker commit`. Boot it with `flavor run`.
pub(crate) fn cmd_commit(id: &str, name: &str, description: &str) -> Result<()> {
    println!("{}", commit(id, name, description)?);
    Ok(())
}

/// Snapshot a machine's rootfs/disk into a named flavor, returning the name.
pub fn commit(id: &str, name: &str, description: &str) -> Result<String> {
    valid_flavor_name(name)?;
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    reject_unikraft(&vm, "snapshot")?;
    let dir = db::flavors_dir()?.join(name);
    if dir.exists() || db.find_flavor(name)?.is_some() {
        anyhow::bail!("flavor {name:?} already exists (remove it first)");
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating flavor dir {}", dir.display()))?;

    // Make the guest's disk/rootfs consistent before cloning: flush a Linux
    // guest, or cleanly power off a BSD guest (a live UFS can't be cloned
    // consistently). See [`quiesce_guest_for_snapshot`].
    quiesce_guest_for_snapshot(&vm);

    let mdir = std::path::PathBuf::from(&vm.state_dir);
    if vm.kind == "linux" {
        // The machine's writable rootfs (per-machine clone, or its volume).
        let src = match &vm.volume {
            Some(v) => volume_dir(v)?.join("rootfs"),
            None => mdir.join("rootfs"),
        };
        if !src.exists() {
            let _ = std::fs::remove_dir_all(&dir);
            anyhow::bail!("machine {} has no rootfs to snapshot", vm.id);
        }
        // Flush the guest's just-synced writes (virtio-fs passes them through to
        // these host files) to disk before the extent-level clone.
        let _ = std::process::Command::new("sync").status();
        host::cow_copy(&src, &dir.join("rootfs"), true)?;
    } else {
        // BSD: snapshot the raw disk (`root.<ext>`), from the volume or state dir.
        let base = match &vm.volume {
            Some(v) => volume_dir(v)?,
            None => mdir.clone(),
        };
        let disk = ["root.raw", "root.img"]
            .iter()
            .map(|f| base.join(f))
            .find(|p| p.exists())
            .ok_or_else(|| {
                let _ = std::fs::remove_dir_all(&dir);
                anyhow::anyhow!("machine {} has no disk to snapshot", vm.id)
            })?;
        // Flush the guest's just-synced writes from the host page cache to disk
        // so `clonefile` (extent-level) captures them — without this the snapshot
        // silently misses recent changes even though the file *reads* current.
        fullsync_file(&disk);
        let ext = disk.extension().and_then(|e| e.to_str()).unwrap_or("img");
        host::cow_copy(&disk, &dir.join(format!("disk.{ext}")), false)?;
    }

    db.upsert_flavor(
        name,
        // Store the guest-OS label (freebsd/netbsd/linux), not the internal boot
        // mode (firmware/kernel), so the UI labels it right and `run` knows how
        // to boot it.
        guest_os_kind(&vm.kind, &vm.image),
        &vm.image,
        &dir.to_string_lossy(),
        description,
    )?;
    Ok(name.to_string())
}
