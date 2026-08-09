//! The machine lifecycle: `ps`, `start`, `stop`, `update`, `rm` and `commit`.

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::*;
#[cfg(target_os = "macos")]
use crate::store;
use crate::{agent, api, db, host, id, linux, names, nanos, osv, unikraft};

use super::boot::{
    boot_freebsd, boot_freebsd_disk, boot_linux, boot_linux_from, boot_nanos_image, boot_netbsd,
    boot_netbsd_disk, boot_osv_image, boot_unikraft_image, machine_dir_or_tmp, volume_dir,
};
use super::flavor::valid_flavor_name;
use super::guest::{guest_os_kind, reject_unikraft};
use super::truncate;

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
        "{:<14}  {:<20}  {:<22}  {:<24}  {:<15}  {}",
        "ID", "NAME", "IMAGE", "STATUS", "CREATED", "COMMAND"
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
        println!(
            "{:<14}  {:<20}  {:<22}  {:<24}  {:<15}  {}",
            m.id,
            truncate(m.name.as_deref().unwrap_or("-"), 20),
            truncate(&m.image, 22),
            status,
            format!("{} ago", db::human_duration_since(&created)),
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

/// Restart a stopped machine *in place*: re-boot the recorded image / resources
/// / volume under the SAME id (like `docker start`), rather than minting a new
/// machine. Detached. The guest OS is inferred from the recorded image ref.
pub(crate) fn cmd_start(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;

    // Already up? Nothing to do.
    if vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false) {
        println!("{}", vm.id);
        return Ok(());
    }

    let cpus = vm.cpus.clamp(1, 255) as u8;
    let mem = vm.mem.max(64) as u32;
    let volume = vm.volume.clone();

    // Resume a BSD machine from ITS OWN root disk, not a re-fetched base. The
    // per-machine working disk (a CoW clone of the original base — a fetched
    // image OR a committed snapshot) lives in the state dir as `root.<ext>`.
    // Re-cloning a base here would silently replace the disk: for a snapshot
    // flavor that means booting the DEFAULT image over the user's snapshot data
    // (data loss). So when that disk exists, boot it IN PLACE (persist) — which
    // also preserves runtime changes across stop/start, like `docker start`.
    // Volume-backed machines already resume their volume disk, so skip those.
    let existing_disk = if vm.volume.is_none() {
        let vdir = machine_dir_or_tmp(&vm.id);
        ["root.raw", "root.img", "root.qcow2"]
            .iter()
            .map(|n| vdir.join(n))
            .find(|p| p.exists())
    } else {
        None
    };

    // The DB row is left in place (the boot re-records it via INSERT OR REPLACE),
    // so it flips exited→running rather than vanishing from `ps`. NB: don't wipe
    // the whole state dir here — that means an `rm -rf` of the old read-only nix
    // rootfs, which is slow enough to make Play "spin forever". The Linux boot
    // path (prepare_linux_root) renames the stale rootfs aside and GC's it in the
    // background, so the restart returns promptly. Old sockets/logs/port files
    // are simply overwritten on the new boot.

    // The next boot picks up this id + name instead of generating fresh ones.
    id::set_override(&vm.id);
    if let Some(name) = &vm.name {
        names::set_override(name);
    }

    // Re-join the recorded global network on restart (its membership is stored
    // in the DB and edited via `network connect/disconnect`). Reuse the member
    // name so the derived MAC — and thus the BSD DHCP lease — stays stable; hint
    // the previously-assigned IP so a plain restart keeps its address.
    if let Some(ip) = vm.net_ip.as_deref().filter(|s| !s.is_empty()) {
        std::env::set_var("BSDKRUN_NET_PREF_IP", ip);
    }
    let net = NetConfig {
        no_net: false,
        ports: vec![],
        mac: None,
        network: vm.network.clone(),
        name: vm.name.clone(),
    };
    let vmcfg = VmConfig { cpus, mem };

    // Detect the guest from the recorded kind first (the image ref is unreliable
    // for a snapshot machine — its image is `disk.img`/`disk.raw`, not a
    // `netbsd-*`/`freebsd-*` name). FreeBSD records `firmware` (macOS EFI) or
    // `freebsd` (Linux PVH); NetBSD records `netbsd` (or legacy `kernel`).
    let reference = vm.image.to_lowercase();
    let is_freebsd =
        matches!(vm.kind.as_str(), "firmware" | "freebsd") || reference.starts_with("freebsd");
    let is_netbsd =
        matches!(vm.kind.as_str(), "kernel" | "netbsd") || reference.starts_with("netbsd");

    if vm.kind == "linux" {
        let largs = LinuxArgs {
            image: vm.image.clone(),
            kernel: None,
            kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
            detach: true,
            initramfs: false,
            volume: volume.clone(),
            mounts: vec![],
            entrypoint: None,
            env: vec![],
            console: "hvc0".to_string(),
            net,
            vm: vmcfg,
            repo: None,
            command: vec![], // persistent restart — keep a console shell alive
        };
        // Resume the machine's OWN rootfs (which holds its snapshot + runtime
        // changes) by passing it as the boot source, so restart never re-clones
        // the base OCI image and loses data. Reuse any intact, non-empty rootfs —
        // NOT gated on /bin|/nix, so images without those top-level dirs still
        // resume. Volume machines resume their volume rootfs already; a missing or
        // broken (nested) dir falls back to the base image.
        let own_rootfs = machine_dir_or_tmp(&vm.id).join("rootfs");
        let intact = own_rootfs.symlink_metadata().is_ok()
            && !own_rootfs.join("rootfs").exists()
            && std::fs::read_dir(&own_rootfs)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        if volume.is_none() && intact {
            boot_linux_from(largs, Some(own_rootfs), &[])
        } else {
            boot_linux(largs)
        }
    } else if vm.kind == "nanos" {
        // Re-boot the machine's own cloned disk in place when it survives (so
        // TFS state persists across stop/start, like the BSDs); fall back to
        // a fresh clone of the original image.
        let spec = nanos::BootSpec::load(std::path::Path::new(&vm.state_dir))?;
        let reuse = existing_disk.is_some();
        boot_nanos_image(
            &spec.image,
            spec.kernel.as_deref(),
            &spec.cmdline,
            None,
            true,
            reuse,
            net,
            vmcfg,
            existing_disk,
        )
    } else if vm.kind == "osv" {
        // Re-boot the machine's own cloned disk in place when it survives, so
        // filesystem writes persist across stop/start like the BSDs; fall back
        // to a fresh clone of the original image.
        let spec = osv::BootSpec::load(std::path::Path::new(&vm.state_dir))?;
        let reuse = existing_disk.is_some();
        boot_osv_image(
            &spec.image,
            &spec.cmdline,
            spec.gic,
            None,
            false,
            reuse,
            volume.as_deref(),
            &[],
            true,
            net,
            vmcfg,
            existing_disk,
        )
    } else if vm.kind == "unikraft" {
        // Nothing to resume — a unikernel has no disk — so this just boots the
        // same image again, from the spec saved beside its console log.
        let spec = unikraft::BootSpec::load(std::path::Path::new(&vm.state_dir))?;
        boot_unikraft_image(
            &spec.kernel,
            &spec.cmdline,
            spec.initramfs.as_deref(),
            true,
            net,
            vmcfg,
            &spec.volumes,
        )
    } else if is_freebsd || is_netbsd {
        // Boot the machine's own disk in place when it exists (see above), so a
        // snapshot machine keeps its data; otherwise fall back to the base image.
        let reuse = existing_disk.is_some();
        let args = BsdArgs {
            version: None, // bundled image (as originally booted)
            firmware: None,
            force: false,
            attach_disk: vec![],
            disk_size: None,
            run: RunConfig {
                detach: true,
                persist: reuse, // in-place boot of the existing disk (no re-clone)
                volume,
            },
            net,
            vm: vmcfg,
            verbose: false,
            repo: None,
            command: vec![],
        };
        let result = match (is_freebsd, existing_disk) {
            (true, Some(d)) => boot_freebsd_disk(args, Some(d)),
            (true, None) => boot_freebsd(args),
            (false, Some(d)) => boot_netbsd_disk(args, Some(d)),
            (false, None) => boot_netbsd(args),
        };
        // Booting the in-place disk relabels the row's image to `root.<ext>`;
        // restore the original label so `ps` still shows what it was booted from.
        if reuse && result.is_ok() {
            db.set_machine_image(&vm.id, &vm.image).ok();
        }
        result
    } else {
        // Put the id back for any future attempt and report clearly.
        anyhow::bail!(
            "don't know how to restart a {:?} machine ({}); start it with `run` instead",
            vm.kind,
            vm.image
        );
    }
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
