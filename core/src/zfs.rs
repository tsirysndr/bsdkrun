//! `bsdkrun zfs` — ZFS volumes for Linux, FreeBSD and NetBSD guests.
//!
//! A volume is one zpool on a sparse raw disk image under `<state>/zfs/`,
//! attached to a machine as virtio-blk and imported by the guest. Its
//! **branches** are datasets inside that pool: `main` is created with the
//! pool, and `bsdkrun zfs branch` snapshots a branch and `zfs clone`s it — a
//! copy-on-write fork that costs nothing until it diverges. That is ZFS's own
//! CoW, not the host filesystem's, which is the point: a branch shares blocks
//! with its parent at the block level, inside the guest, however the image is
//! stored on the host.
//!
//! The constraint that shapes everything here: **a pool is imported by one
//! kernel at a time.** The host has no ZFS, and libkrun cannot share a block
//! device between two VMs, so a volume — all of its branches — is held by at
//! most one machine, and bsdkrun refuses to record a second holder rather
//! than let two guests write the same pool. Different volumes go to different
//! machines freely; several branches of one volume mount in the same guest
//! at different paths.
//!
//! Where the ZFS commands actually run:
//!
//! - **Mounting** happens in the guest at boot. Linux does it from the
//!   generated init (so it is ordered before the entrypoint); the BSDs have no
//!   init of ours, so [`run_post_boot`] runs the same script through the guest
//!   agent once it answers — from a thread inside the VM process for a plain
//!   `-d`/`start`, or inline before a one-shot command.
//! - **Branching and destroying branches** need a kernel with the pool
//!   imported. If a running machine holds the volume, its agent does the work
//!   live; otherwise [`run_in_pool`] boots a throwaway FreeBSD helper with the
//!   image attached, runs the script, exports the pool and powers off.
//!
//! What each guest taught us (verified on arm64, see the probes in the commit):
//!
//! - **FreeBSD** has `zfs.ko` in base; `kldload zfs`. Attached disks are
//!   `/dev/vtbd1…`. `zpool import -d` takes a *device path* here; a directory
//!   of symlinks reports "invalid label".
//! - **NetBSD** names virtio disks from `ld4` (`ld0`–`ld3` are other buses),
//!   so the disk is found by position in `hw.disknames`, not by a fixed unit,
//!   and the whole-disk partition letter comes from `kern.rawpartition`.
//!   `zpool import -d` wants a *directory* here, the opposite of FreeBSD.
//!   `zpool` calls `modctl(MODCTL_LOAD)` itself and dies with EPERM at
//!   securelevel 1 before the kernel's autoload gets a look in, so the module
//!   must be loaded during `/etc/rc` via `/etc/modules.conf` — and the module
//!   must match the running kernel, which bsdkrun fetches from the CDN
//!   separately from the image. [`prepare_netbsd`] ships the matching
//!   `zfs`/`solaris` modules in and registers them; the machine then needs
//!   one restart.
//! - **Linux** needs `zpool` in the image; the kernel side is built in.
//!   vmlinux-builder has carried OpenZFS in every release since 7.3.5, the
//!   default kernel, so nothing is loaded or matched. The init reports
//!   clearly when `zpool` is missing instead of failing the mount silently.
//!
//! Pools created by the FreeBSD helper (or a Linux guest) are created with an
//! explicit feature set NetBSD's older implementation understands, so a volume
//! branched before it was ever attached still imports on every guest.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::{DiskSpec, ZfsDisk, ZfsMount};
use crate::commands::{load_attached_disks, save_attached_disks};
use crate::host::GuestOs;
use crate::{agent, db, fetch};

/// The branch every volume starts with.
pub const MAIN: &str = "main";

/// Image size when `create` is given none. Sparse, so it costs nothing until
/// the guest writes.
pub const DEFAULT_SIZE: &str = "10G";

/// Pool features enabled when a pool is created by FreeBSD or Linux. This is
/// the set NetBSD's ZFS (`zpool upgrade -v` there) supports; a pool created
/// with OpenZFS 2.x defaults carries `zstd_compress`, `log_spacemap` and
/// friends, and NetBSD refuses to import it. NetBSD itself creates with its
/// own defaults, which are exactly this list.
const PORTABLE_FEATURES: &[&str] = &[
    "async_destroy",
    "bookmarks",
    "embedded_data",
    "empty_bpobj",
    "enabled_txg",
    "extensible_dataset",
    "filesystem_limits",
    "hole_birth",
    "large_blocks",
    "lz4_compress",
    "multi_vdev_crash_dump",
    "spacemap_histogram",
];

/// File in a NetBSD machine's state dir once its ZFS modules are in place,
/// so `attach` does not boot it again just to find that out.
const NETBSD_READY_MARKER: &str = "zfs-netbsd-ready";

/// How long the post-boot hook waits for the guest agent before giving up on
/// the import (a BSD boots in ~15s; a cold image fetch happens before boot).
const AGENT_WAIT: std::time::Duration = std::time::Duration::from_secs(240);

// ---------------------------------------------------------------------------
// wire types
// ---------------------------------------------------------------------------

/// A ZFS volume as `bsdkrun zfs ls --json` reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZfsVolume {
    pub name: String,
    pub path: String,
    /// The image's nominal size (what the guest sees as the disk).
    pub size_bytes: u64,
    /// Human-readable space actually allocated on the host, e.g. "1.2 GiB";
    /// `None` when it could not be measured.
    pub size: Option<String>,
    pub branches: Vec<ZfsBranch>,
    /// The machine holding the pool, if any. At most one.
    pub attached_to: Vec<ZfsAttachment>,
    pub created_at: Option<String>,
}

/// A branch: a dataset of the volume's pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZfsBranch {
    pub name: String,
    /// The branch it was cloned from; `None` for `main`.
    pub origin: Option<String>,
    pub created_at: Option<String>,
}

/// Which machine holds a volume, and which branches it mounts where.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZfsAttachment {
    pub machine: String,
    #[serde(default)]
    pub machine_name: Option<String>,
    pub running: bool,
    pub mounts: Vec<ZfsMount>,
}

// ---------------------------------------------------------------------------
// names and paths
// ---------------------------------------------------------------------------

/// `<state>/zfs`, where the images live.
pub fn dir() -> Result<PathBuf> {
    Ok(db::state_dir()?.join("zfs"))
}

/// The image backing a volume.
pub fn image_path(name: &str) -> Result<PathBuf> {
    Ok(dir()?.join(format!("{name}.img")))
}

/// A name usable as both a zpool name and a dataset component. ZFS allows
/// more, but these are also shell-interpolated into the guest script, so
/// keep to what needs no quoting.
pub fn validate_name(what: &str, name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with(['-', '.']);
    if !ok {
        anyhow::bail!(
            "invalid {what} name {name:?}: use letters, digits, '-', '_' and '.', \
             not starting with '-' or '.'"
        );
    }
    // zpool rejects these as pool names; refusing them here keeps the error
    // readable instead of surfacing from inside a guest.
    if matches!(
        name,
        "mirror" | "raidz" | "raidz1" | "raidz2" | "raidz3" | "spare" | "log" | "cache" | "draid"
    ) || name.starts_with("c0")
    {
        anyhow::bail!("{name:?} is a name ZFS reserves; pick another");
    }
    Ok(())
}

/// Split `VOL[@BRANCH]`.
pub fn parse_target(target: &str) -> Result<(String, Option<String>)> {
    match target.split_once('@') {
        Some((v, b)) => {
            validate_name("volume", v)?;
            validate_name("branch", b)?;
            Ok((v.to_string(), Some(b.to_string())))
        }
        None => {
            validate_name("volume", target)?;
            Ok((target.to_string(), None))
        }
    }
}

/// Where a branch mounts when the caller names no path: `/mnt/<vol>` for
/// `main`, `/mnt/<vol>-<branch>` otherwise, so two branches of one volume in
/// the same guest never collide.
pub fn default_mountpoint(volume: &str, branch: &str) -> String {
    if branch == MAIN {
        format!("/mnt/{volume}")
    } else {
        format!("/mnt/{volume}-{branch}")
    }
}

// ---------------------------------------------------------------------------
// the guest script
// ---------------------------------------------------------------------------

/// POSIX-sh single-quote.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A shell expression for the `index`-th attached disk on this guest.
///
/// Linux: no root block device under virtio-fs, so attached disks are
/// `/dev/vda`, `/dev/vdb`, … in order. FreeBSD: root is `vtbd0`. NetBSD: the
/// virtio `ld` units start wherever the kernel's other `ld` buses leave off
/// (`ld4` on the arm64 GENERIC64 kernel), so take the disks `hw.disknames`
/// lists, in unit order, skipping the root.
pub fn device_expr(os: GuestOs, index: usize) -> String {
    let nth = index + 1;
    match os {
        GuestOs::Linux => format!("$(ls /dev/vd? 2>/dev/null | sort | sed -n '{nth}p')"),
        GuestOs::Freebsd => format!("/dev/vtbd{nth}"),
        // The root disk is the first `ld`; the attached ones follow it.
        GuestOs::Netbsd => format!(
            "$(d=$(sysctl -n hw.disknames | tr ' ' '\\n' | grep '^ld' | sort -k1.3n | sed -n '{after_root}p'); \
             [ -n \"$d\" ] && printf '/dev/%s%s' \"$d\" \"$(printf \"\\\\$(printf '%03o' $((97 + $(sysctl -n kern.rawpartition))))\")\")",
            after_root = index + 2,
        ),
    }
}

/// How the guest loads its ZFS module. NetBSD can only do it at boot (see the
/// module docs), so here it just reports.
fn module_load(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Linux => "modprobe zfs >/dev/null 2>&1 || true",
        GuestOs::Freebsd => "kldstat -q -m zfs || kldload zfs",
        GuestOs::Netbsd => {
            "modstat 2>/dev/null | grep -q '^zfs ' || { echo '[bsdkrun] zfs: the zfs module is not loaded; NetBSD loads it at boot from /etc/modules.conf — restart this machine'; return 1; }"
        }
    }
}

/// `zpool create` options for a portable pool (see [`PORTABLE_FEATURES`]).
/// NetBSD knows no `-o feature@` it does not already enable by default.
fn create_opts(os: GuestOs) -> String {
    // `cachefile=none`: the guest's zpool.cache must not remember this pool,
    // or the module auto-imports it at the next boot by the path it had last
    // time (UNAVAIL when that was a symlink dir, or another guest's name),
    // ahead of the script that knows the right device.
    let mut s = String::from("-f -o cachefile=none -O mountpoint=none -O canmount=noauto -m none");
    if os != GuestOs::Netbsd {
        s.push_str(" -d");
        for f in PORTABLE_FEATURES {
            s.push_str(&format!(" -o feature@{f}=enabled"));
        }
    }
    s
}

/// The script that imports (or, on a blank disk, creates) the pool and mounts
/// the requested branches. Idempotent: safe to run again on a guest that
/// already has the pool, which is how a branch is mounted live.
///
/// Defined as a function and then called, so `return` works inside it and a
/// failure never aborts the init or the command this is prepended to.
pub fn setup_script(os: GuestOs, index: usize, disk: &ZfsDisk) -> String {
    let pool = sq(&disk.volume);
    let dev = device_expr(os, index);
    let load = module_load(os);
    let create = create_opts(os);
    let mut s = format!(
        "bk_zfs_setup() {{\n\
         pool={pool}\n\
         dev=\"{dev}\"\n\
         if [ -z \"$dev\" ] || [ ! -e \"$dev\" ]; then echo \"[bsdkrun] zfs: no disk for pool $pool (expected attached disk #{n})\"; return 1; fi\n\
         {load}\n\
         command -v zpool >/dev/null 2>&1 || {{ echo \"[bsdkrun] zfs: this guest has no zpool; install ZFS userland (and a kernel with the module) to use pool $pool\"; return 1; }}\n\
         # A pool the module auto-imported from a stale zpool.cache at boot\n\
         # (an old vdev path) is UNAVAIL; export it and import it properly.\n\
         if zpool list -H -o name \"$pool\" >/dev/null 2>&1; then\n\
         \x20 case \"$(zpool list -H -o health \"$pool\" 2>/dev/null)\" in ONLINE|DEGRADED) ;; *) zpool export -f \"$pool\" >/dev/null 2>&1 ;; esac\n\
         fi\n\
         if zpool list -H -o name \"$pool\" >/dev/null 2>&1; then\n\
         \x20 zpool set cachefile=none \"$pool\" >/dev/null 2>&1\n\
         fi\n\
         if ! zpool list -H -o name \"$pool\" >/dev/null 2>&1; then\n\
         \x20 # An import can succeed from a cached config and still leave the\n\
         \x20 # pool UNAVAIL (its vdev path unopenable), so each form is checked\n\
         \x20 # for health and exported again if it did not actually work.\n\
         \x20 bk_zfs_import() {{\n\
         \x20   zpool import -f -N -o cachefile=none \"$@\" \"$pool\" >/dev/null 2>&1 || return 1\n\
         \x20   case \"$(zpool list -H -o health \"$pool\" 2>/dev/null)\" in ONLINE|DEGRADED) return 0 ;; esac\n\
         \x20   zpool export \"$pool\" >/dev/null 2>&1; return 1\n\
         \x20 }}\n\
         \x20 ok=0\n\
         \x20 # By device path (FreeBSD; a directory elsewhere), then the default\n\
         \x20 # scan (NetBSD's /dev, Linux's blkid, which also sees partitions).\n\
         \x20 bk_zfs_import -d \"$dev\" && ok=1\n\
         \x20 [ $ok = 0 ] && bk_zfs_import && ok=1\n\
         \x20 if [ $ok = 0 ]; then\n\
         \x20   # Last resort: a directory holding only this device. It stays, as\n\
         \x20   # the pool records the symlink as its vdev path; removing it\n\
         \x20   # made the pool UNAVAIL on NetBSD.\n\
         \x20   sd=/tmp/bsdkrun-zfs-$pool\n\
         \x20   mkdir -p \"$sd\" 2>/dev/null && ln -sf \"$dev\" \"$sd/$(basename \"$dev\")\" && bk_zfs_import -d \"$sd\" && ok=1\n\
         \x20 fi\n\
         \x20 if [ $ok = 0 ]; then\n\
         \x20   # Only a blank disk gets a new pool: a disk that already carries\n\
         \x20   # one (under another name, or one this guest cannot import) is\n\
         \x20   # reported, never overwritten.\n\
         \x20   base=$(basename \"$dev\")\n\
         \x20   if zpool import -d \"$dev\" 2>/dev/null | grep -q 'pool:' || zpool import 2>/dev/null | grep -q \"$base\"; then\n\
         \x20     echo \"[bsdkrun] zfs: $dev holds a pool that is not importable as $pool:\"; zpool import -d \"$dev\" 2>&1 | grep -E 'pool:|state:|status:' | head -3; return 1\n\
         \x20   fi\n\
         \x20   # Never format a disk something is mounted from: a wrong device\n\
         \x20   # index would otherwise take the root disk with it. Partitions\n\
         \x20   # share the disk's name prefix; NetBSD's wedges (dkN) do not, so\n\
         \x20   # ask dkctl which belong to it.\n\
         \x20   disk=$(echo \"$dev\" | sed 's/\\(ld[0-9]*\\)[a-z]$/\\1/')\n\
         \x20   if mount 2>/dev/null | grep -q \"^$disk\"; then echo \"[bsdkrun] zfs: refusing to create pool $pool on $dev: a filesystem is mounted from that disk\"; return 1; fi\n\
         \x20   for w in $(dkctl $(basename \"$disk\") listwedges 2>/dev/null | sed -n 's/^\\(dk[0-9]*\\):.*/\\1/p'); do\n\
         \x20     if mount 2>/dev/null | grep -q \"^/dev/$w \"; then echo \"[bsdkrun] zfs: refusing to create pool $pool on $dev: $w on that disk is mounted\"; return 1; fi\n\
         \x20   done\n\
         \x20   zpool create {create} \"$pool\" \"$dev\" || {{ echo \"[bsdkrun] zfs: could not create pool $pool on $dev\"; return 1; }}\n\
         \x20   zfs create -o canmount=noauto \"$pool/{main}\" || return 1\n\
         \x20 fi\n\
         fi\n",
        n = index + 1,
        main = MAIN,
    );
    for m in &disk.mounts {
        let ds = format!("$pool/{}", m.branch);
        let mp = sq(&m.mountpoint);
        s.push_str(&format!(
            "ds=\"{ds}\"; mp={mp}\n\
             if zfs list -H -o name \"$ds\" >/dev/null 2>&1; then\n\
             \x20 mkdir -p \"$mp\" 2>/dev/null\n\
             \x20 if zfs set mountpoint=\"$mp\" \"$ds\"; then\n\
             \x20   # `canmount` is not inherited, so a dataset made by hand may\n\
             \x20   # already be mounted by the `set` above; only mount if not.\n\
             \x20   [ \"$(zfs get -H -o value mounted \"$ds\" 2>/dev/null)\" = yes ] || zfs mount \"$ds\" || echo \"[bsdkrun] zfs: mount $ds at $mp failed\"\n\
             \x20 fi\n\
             else echo \"[bsdkrun] zfs: no branch {br} in pool $pool\"\n\
             fi\n",
            br = m.branch,
        ));
    }
    s.push_str("return 0\n}\nbk_zfs_setup\n");
    s
}

/// The script run by the NetBSD provisioning step, after the module archive
/// was unpacked to `/`: own the modules (the archive was made on the host,
/// and the kernel loads only root-owned files) and register the module for
/// `/etc/rc.d/modules`, which runs while securelevel is still 0.
fn netbsd_register_script(mach: &str, kver: &str) -> String {
    format!(
        "chown -R root:wheel /stand/{mach}/{kver}/modules 2>/dev/null; \
         grep -qx zfs /etc/modules.conf 2>/dev/null || echo zfs >> /etc/modules.conf; sync"
    )
}

// ---------------------------------------------------------------------------
// host-side bookkeeping
// ---------------------------------------------------------------------------

/// Every machine's recorded ZFS attachments: `(machine, disk index, spec)`.
fn attachments() -> Result<Vec<(db::MachineRow, usize, ZfsDisk)>> {
    let db = db::Db::open()?;
    let mut out = Vec::new();
    for m in db.list_machines()? {
        let vdir = Path::new(&m.state_dir);
        for (i, d) in load_attached_disks(vdir).into_iter().enumerate() {
            if let Some(z) = d.zfs {
                out.push((m.clone(), i, z));
            }
        }
    }
    Ok(out)
}

fn is_running(m: &db::MachineRow) -> bool {
    m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false)
}

/// The machine holding `volume`, with its disk index and attachment.
pub fn holder(volume: &str) -> Result<Option<(db::MachineRow, usize, ZfsDisk)>> {
    Ok(attachments()?
        .into_iter()
        .find(|(_, _, z)| z.volume == volume))
}

/// Make `volume` free for `machine` to take: a *running* holder is an error
/// (two kernels must never import one pool), a stopped one merely has its
/// record dropped — a stopped machine imports nothing, and a volume that a
/// finished one-shot command still "held" would otherwise be stuck.
fn displace_holder(volume: &str, machine: Option<&str>) -> Result<Option<String>> {
    let Some((other, i, _)) = holder(volume)? else {
        return Ok(None);
    };
    if Some(other.id.as_str()) == machine {
        return Ok(None);
    }
    if is_running(&other) {
        anyhow::bail!(
            "zfs volume {volume} is in use by running machine {} ({}); stop it or detach the volume first \
             (a pool is imported by one machine at a time)",
            other.id,
            other.name.as_deref().unwrap_or("-")
        );
    }
    let vdir = PathBuf::from(&other.state_dir);
    let mut disks = load_attached_disks(&vdir);
    if i < disks.len() {
        disks.remove(i);
    }
    save_attached_disks(&vdir, &disks);
    Ok(Some(other.id))
}

/// `bsdkrun zfs ls`.
pub fn list() -> Result<Vec<ZfsVolume>> {
    let db = db::Db::open()?;
    let held = attachments()?;
    let mut out = Vec::new();
    for v in db.list_zfs_volumes()? {
        let branches = db
            .list_zfs_branches(&v.name)?
            .into_iter()
            .map(|b| ZfsBranch {
                name: b.name,
                origin: b.origin,
                created_at: Some(b.created_at),
            })
            .collect();
        let attached_to = held
            .iter()
            .filter(|(_, _, z)| z.volume == v.name)
            .map(|(m, _, z)| ZfsAttachment {
                machine: m.id.clone(),
                machine_name: m.name.clone(),
                running: is_running(m),
                mounts: z.mounts.clone(),
            })
            .collect();
        let size_bytes = std::fs::metadata(&v.path)
            .map(|m| m.len())
            .unwrap_or(v.size as u64);
        let size = Some(crate::commands::volumes::volume_size(&v.path)).filter(|s| s != "-");
        out.push(ZfsVolume {
            name: v.name,
            path: v.path,
            size_bytes,
            size,
            branches,
            attached_to,
            created_at: Some(v.created_at),
        });
    }
    Ok(out)
}

/// `bsdkrun zfs create NAME [--size S]`: a sparse image and the `main` branch
/// record. The pool itself is made by the first guest to import the disk (or
/// by the helper, if the volume is branched first).
pub fn create(name: &str, size: Option<&str>) -> Result<String> {
    validate_name("volume", name)?;
    let db = db::Db::open()?;
    if db.find_zfs_volume(name)?.is_some() {
        anyhow::bail!("zfs volume {name:?} already exists");
    }
    let size = size.unwrap_or(DEFAULT_SIZE);
    let bytes = fetch::parse_size(size)?;
    // ZFS wants at least 64 MiB per vdev; a smaller pool fails with a message
    // about the device being too small, from inside a guest.
    if bytes < 64 << 20 {
        anyhow::bail!("--size must be at least 64M for a zpool");
    }
    let path = image_path(name)?;
    if path.exists() {
        anyhow::bail!(
            "{} already exists but is not recorded; remove it or pick another name",
            path.display()
        );
    }
    std::fs::create_dir_all(dir()?).context("creating the zfs volume dir")?;
    let f = std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    f.set_len(bytes)
        .with_context(|| format!("sizing {}", path.display()))?;
    db.upsert_zfs_volume(name, &path.to_string_lossy(), bytes as i64)?;
    db.add_zfs_branch(name, MAIN, None)?;
    Ok(name.to_string())
}

/// A recorded volume, or a clear error.
fn volume_row(name: &str) -> Result<db::ZfsVolumeRow> {
    validate_name("volume", name)?;
    db::Db::open()?
        .find_zfs_volume(name)?
        .ok_or_else(|| anyhow::anyhow!("no such zfs volume: {name} (see `bsdkrun zfs ls`)"))
}

/// `bsdkrun zfs branch VOL NEW [--from BRANCH]`: `zfs snapshot` + `zfs clone`
/// inside the pool, through whichever kernel can reach it.
pub fn branch(volume: &str, name: &str, from: Option<&str>) -> Result<String> {
    validate_name("branch", name)?;
    let from = from.unwrap_or(MAIN);
    validate_name("branch", from)?;
    let v = volume_row(volume)?;
    let db = db::Db::open()?;
    let branches = db.list_zfs_branches(volume)?;
    if branches.iter().any(|b| b.name == name) {
        anyhow::bail!("branch {name:?} already exists in {volume}");
    }
    if !branches.iter().any(|b| b.name == from) {
        anyhow::bail!(
            "no branch {from:?} in {volume} to branch from (see `bsdkrun zfs ls {volume}`)"
        );
    }
    // The snapshot is named after the branch it seeds, so destroying the
    // branch knows what to clean up.
    let script = format!(
        "zfs list -H -t snapshot -o name \"$pool/{from}@{name}\" >/dev/null 2>&1 || zfs snapshot \"$pool/{from}@{name}\" || exit 1\n\
         zfs clone -o canmount=noauto \"$pool/{from}@{name}\" \"$pool/{name}\" || exit 1\n\
         echo branched"
    );
    run_in_pool(&v, &script)?;
    db.add_zfs_branch(volume, name, Some(from))?;
    Ok(format!("{volume}@{name}"))
}

/// `bsdkrun zfs attach VOL[@BRANCH] MACHINE [--mount PATH]`.
///
/// Records the attachment in the machine's `attached-disks.json`, which is
/// what `start` re-attaches. A running machine that already holds the pool
/// gets the branch mounted live; one that does not cannot be given a disk
/// without a restart, and says so.
pub fn attach(target: &str, machine: &str, mount: Option<&str>) -> Result<String> {
    let (volume, br) = parse_target(target)?;
    let br = br.unwrap_or_else(|| MAIN.to_string());
    let v = volume_row(&volume)?;
    let db = db::Db::open()?;
    if !db.list_zfs_branches(&volume)?.iter().any(|b| b.name == br) {
        anyhow::bail!("no branch {br:?} in {volume} (see `bsdkrun zfs ls {volume}`)");
    }
    let vm = db.find_machine(machine)?;
    let kind = crate::commands::guest::guest_os_kind(&vm.kind, &vm.image);
    if !matches!(kind, "linux" | "freebsd" | "netbsd") {
        anyhow::bail!(
            "zfs volumes attach to Linux, FreeBSD and NetBSD machines, not a {kind} guest"
        );
    }
    let mountpoint = match mount {
        Some(m) if m.starts_with('/') => m.to_string(),
        Some(m) => anyhow::bail!("--mount must be an absolute guest path, got {m:?}"),
        None => default_mountpoint(&volume, &br),
    };

    // One holder per pool: a running one blocks, a stopped one is displaced.
    let moved_from = displace_holder(&volume, Some(&vm.id))?;
    if let Some(prev) = &moved_from {
        eprintln!("{volume} was attached to stopped machine {prev}; moving it");
    }

    let vdir = PathBuf::from(&vm.state_dir);
    let mut disks = load_attached_disks(&vdir);
    let running = is_running(&vm);
    let existing = disks
        .iter()
        .position(|d| d.zfs.as_ref().map(|z| z.volume == volume).unwrap_or(false));
    let index = match existing {
        Some(i) => {
            let z = disks[i].zfs.as_mut().unwrap();
            if let Some(m) = z.mounts.iter_mut().find(|m| m.branch == br) {
                m.mountpoint = mountpoint.clone();
            } else {
                z.mounts.push(ZfsMount {
                    branch: br.clone(),
                    mountpoint: mountpoint.clone(),
                });
            }
            i
        }
        None => {
            if running {
                anyhow::bail!(
                    "machine {} is running and a disk cannot be hot-plugged; stop it, \
                     attach, then `bsdkrun start {}`",
                    vm.id,
                    vm.id
                );
            }
            disks.push(DiskSpec {
                path: PathBuf::from(&v.path),
                read_only: false,
                mount: None,
                zfs: Some(ZfsDisk {
                    volume: volume.clone(),
                    mounts: vec![ZfsMount {
                        branch: br.clone(),
                        mountpoint: mountpoint.clone(),
                    }],
                }),
            });
            disks.len() - 1
        }
    };
    // A stopped NetBSD machine gets its modules now, so the next `start`
    // imports the pool rather than asking for one more restart. Done before
    // the record is written: the preparation boot would otherwise find it
    // and run the same provisioning from its own post-boot hook, concurrently.
    if !running && kind == "netbsd" && existing.is_none() {
        if let Err(e) = prepare_netbsd_machine(&vm) {
            eprintln!("warning: could not prepare NetBSD for ZFS now ({e:#}); the next start will");
        }
    }
    save_attached_disks(&vdir, &disks);

    if running {
        // The pool is already imported in this guest: mount the branch now.
        let os = guest_os(kind);
        let port = agent::read_port(&vdir)
            .ok_or_else(|| anyhow::anyhow!("machine {} has no agent port recorded", vm.id))?;
        let spec = disks[index].zfs.clone().unwrap();
        let (code, out, err) = exec_capture(port, &setup_script(os, index, &spec))?;
        if code != 0 {
            anyhow::bail!("mounting {target} in {}: {}", vm.id, (out + &err).trim());
        }
        return Ok(format!("{target} mounted at {mountpoint} in {}", vm.id));
    }

    Ok(format!(
        "{target} attached to {} at {mountpoint}; takes effect on `bsdkrun start {}`",
        vm.id, vm.id
    ))
}

/// `bsdkrun zfs detach VOL[@BRANCH] MACHINE`. Without a branch the whole
/// pool goes; a running machine unmounts (and exports) live, and the disk
/// itself leaves on the next start.
pub fn detach(target: &str, machine: &str) -> Result<String> {
    let (volume, br) = parse_target(target)?;
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;
    let vdir = PathBuf::from(&vm.state_dir);
    let mut disks = load_attached_disks(&vdir);
    let Some(i) = disks
        .iter()
        .position(|d| d.zfs.as_ref().map(|z| z.volume == volume).unwrap_or(false))
    else {
        anyhow::bail!("zfs volume {volume} is not attached to machine {}", vm.id);
    };
    let running = is_running(&vm);
    let kind = crate::commands::guest::guest_os_kind(&vm.kind, &vm.image);
    let live = |script: &str| -> Result<()> {
        if !running {
            return Ok(());
        }
        let Some(port) = agent::read_port(&vdir) else {
            return Ok(());
        };
        let (code, out, err) = exec_capture(port, script)?;
        if code != 0 {
            anyhow::bail!("{}", (out + &err).trim());
        }
        Ok(())
    };
    let msg = match br {
        Some(b) => {
            let z = disks[i].zfs.as_mut().unwrap();
            let Some(pos) = z.mounts.iter().position(|m| m.branch == b) else {
                anyhow::bail!("branch {b} of {volume} is not mounted in machine {}", vm.id);
            };
            z.mounts.remove(pos);
            live(&format!(
                "zfs unmount '{volume}/{b}' 2>&1 || true; zfs set canmount=noauto '{volume}/{b}' 2>/dev/null; true"
            ))
            .with_context(|| format!("unmounting {volume}@{b} in {}", vm.id))?;
            if z.mounts.is_empty() {
                disks.remove(i);
                let _ = kind;
                format!(
                    "{volume}@{b} detached from {}; the disk leaves on its next start",
                    vm.id
                )
            } else {
                format!("{volume}@{b} detached from {}", vm.id)
            }
        }
        None => {
            disks.remove(i);
            live(&format!(
                "zfs unmount -a 2>/dev/null; zpool export '{volume}' 2>&1 || {{ echo \"pool {volume} is busy; stop what uses it and retry\"; exit 1; }}"
            ))
            .with_context(|| format!("exporting {volume} from {}", vm.id))?;
            if running {
                format!(
                    "{volume} exported from {}; the disk leaves on its next start",
                    vm.id
                )
            } else {
                format!("{volume} detached from {}", vm.id)
            }
        }
    };
    save_attached_disks(&vdir, &disks);
    Ok(msg)
}

/// `bsdkrun zfs destroy VOL[@BRANCH] [--force]`. A branch is `zfs destroy`ed
/// inside the pool (ZFS refuses while other branches are cloned from it); a
/// volume is deleted outright, refused while attached unless forced, and
/// never while its holder runs.
pub fn destroy(target: &str, force: bool) -> Result<String> {
    let (volume, br) = parse_target(target)?;
    let v = volume_row(&volume)?;
    let db = db::Db::open()?;
    let held = holder(&volume)?;
    match br {
        Some(b) => {
            if b == MAIN {
                anyhow::bail!(
                    "{volume}@main is the volume's root branch; destroy the volume instead"
                );
            }
            if !db.list_zfs_branches(&volume)?.iter().any(|x| x.name == b) {
                anyhow::bail!("no branch {b:?} in {volume}");
            }
            if let Some((m, i, z)) = &held {
                if z.mounts.iter().any(|x| x.branch == b) {
                    if !force {
                        anyhow::bail!(
                            "{volume}@{b} is mounted in machine {} (detach it first, or use --force)",
                            m.id
                        );
                    }
                    let vdir = PathBuf::from(&m.state_dir);
                    let mut disks = load_attached_disks(&vdir);
                    disks[*i]
                        .zfs
                        .as_mut()
                        .unwrap()
                        .mounts
                        .retain(|x| x.branch != b);
                    save_attached_disks(&vdir, &disks);
                }
            }
            let script = format!(
                "zfs unmount \"$pool/{b}\" >/dev/null 2>&1; zfs destroy -r \"$pool/{b}\" || exit 1\n\
                 for s in $(zfs list -H -t snapshot -o name -r \"$pool\" 2>/dev/null | grep '@{b}$'); do zfs destroy \"$s\"; done\n\
                 echo destroyed"
            );
            run_in_pool(&v, &script)?;
            db.remove_zfs_branch(&volume, &b)?;
            Ok(format!("{volume}@{b}"))
        }
        None => {
            if let Some((m, i, _)) = &held {
                if is_running(m) {
                    anyhow::bail!(
                        "zfs volume {volume} is in use by running machine {}; stop it first",
                        m.id
                    );
                }
                if !force {
                    anyhow::bail!(
                        "zfs volume {volume} is attached to machine {} (detach it first, or use --force)",
                        m.id
                    );
                }
                let vdir = PathBuf::from(&m.state_dir);
                let mut disks = load_attached_disks(&vdir);
                disks.remove(*i);
                save_attached_disks(&vdir, &disks);
            }
            let path = Path::new(&v.path);
            if path.exists() {
                std::fs::remove_file(path)
                    .with_context(|| format!("removing {}", path.display()))?;
            }
            db.remove_zfs_volume(&volume)?;
            Ok(volume)
        }
    }
}

// ---------------------------------------------------------------------------
// running ZFS commands against a pool
// ---------------------------------------------------------------------------

fn guest_os(kind: &str) -> GuestOs {
    match kind {
        "freebsd" => GuestOs::Freebsd,
        "netbsd" => GuestOs::Netbsd,
        _ => GuestOs::Linux,
    }
}

/// Run a script in a guest, returning `(exit code, stdout, stderr)`.
fn exec_capture(port: u16, script: &str) -> Result<(i32, String, String)> {
    let argv = ["sh".to_string(), "-c".to_string(), script.to_string()];
    let mut out = Vec::new();
    let (code, err) = agent::exec_stream(port, &argv, None, &mut out)?;
    Ok((code, String::from_utf8_lossy(&out).into_owned(), err))
}

/// Run `script` with the pool imported as `$pool`: in the running machine
/// that holds it, or in a throwaway FreeBSD helper booted for the purpose.
fn run_in_pool(v: &db::ZfsVolumeRow, script: &str) -> Result<String> {
    if let Some((m, i, z)) = holder(&v.name)? {
        if is_running(&m) {
            let kind = crate::commands::guest::guest_os_kind(&m.kind, &m.image);
            let vdir = PathBuf::from(&m.state_dir);
            let port = agent::read_port(&vdir).ok_or_else(|| {
                anyhow::anyhow!(
                    "machine {} holds {} but has no agent port recorded",
                    m.id,
                    v.name
                )
            })?;
            let setup = setup_script(
                guest_os(kind),
                i,
                &ZfsDisk {
                    volume: z.volume,
                    mounts: vec![],
                },
            );
            let full = format!("{setup}\n[ $? = 0 ] || exit 1\n{script}");
            let (code, out, err) = exec_capture(port, &full)?;
            if code != 0 {
                anyhow::bail!("in machine {}: {}", m.id, (out + &err).trim());
            }
            return Ok(out);
        }
    }
    helper_run(v, script)
}

/// A command that runs the `bsdkrun` CLI, for the boots done out of process
/// (the helper, a NetBSD preparation). This executable by default; the daemon
/// links this crate from a different binary and points `BSDKRUN_BIN` at its
/// supervisor, with `BSDKRUN_BIN_ARGS` carrying the subcommand (`cli`) that
/// makes it accept a bsdkrun command line.
fn cli_command() -> Result<std::process::Command> {
    let exe = match std::env::var_os("BSDKRUN_BIN").filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe().context("locating bsdkrun for the zfs helper")?,
    };
    let mut cmd = std::process::Command::new(exe);
    if let Ok(pre) = std::env::var("BSDKRUN_BIN_ARGS") {
        cmd.args(pre.split_whitespace());
    }
    Ok(cmd)
}

/// Boot FreeBSD with the volume's image attached, run `script` with the pool
/// imported, export it, and power off. FreeBSD because it ships ZFS in base
/// and bsdkrun bundles its image with the agent; the pool is created with a
/// NetBSD-compatible feature set if this is its first import.
fn helper_run(v: &db::ZfsVolumeRow, script: &str) -> Result<String> {
    let name = format!("zfs-helper-{}", crate::id::short_id());
    eprintln!(
        "no running machine holds {}; booting a FreeBSD helper to run ZFS (~15s)…",
        v.name
    );
    let setup = setup_script(
        GuestOs::Freebsd,
        0,
        &ZfsDisk {
            volume: v.name.clone(),
            mounts: vec![],
        },
    );
    let full = format!(
        "{setup}\n[ $? = 0 ] || exit 1\n{script}\nrc=$?\nzpool export \"$pool\" >/dev/null 2>&1\nexit $rc"
    );
    let mut cmd = cli_command()?;
    cmd.args(["freebsd", "--attach-disk"])
        .arg(&v.path)
        .args(["--name", &name, "--", "sh", "-c", &full])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    if std::env::var_os("RUST_LOG").is_none() {
        // The helper's boot chatter is noise to a `zfs branch`; keep warnings.
        cmd.env("RUST_LOG", "warn");
    }
    let out = cmd
        .output()
        .map_err(|e| fetch::spawn_error(&cmd, "the zfs helper boot", e))?;
    // The one-shot boot leaves an exited machine row and its disk clone.
    let _ = crate::commands::machines::remove_machine(&name, true);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        anyhow::bail!("zfs helper failed ({}): {}", out.status, stdout.trim());
    }
    Ok(stdout)
}

// ---------------------------------------------------------------------------
// NetBSD: modules that match the kernel
// ---------------------------------------------------------------------------

/// What the guest reports about itself: `(uname -r, uname -m)`.
fn netbsd_uname(port: u16) -> Result<(String, String)> {
    let (code, out, err) = exec_capture(port, "uname -r; uname -m")?;
    if code != 0 {
        anyhow::bail!("uname in the guest failed: {}", err.trim());
    }
    let mut lines = out.lines();
    let kver = lines.next().unwrap_or("").trim().to_string();
    let mach = lines.next().unwrap_or("").trim().to_string();
    if kver.is_empty() || mach.is_empty() {
        anyhow::bail!("could not read the guest's kernel version");
    }
    Ok((kver, mach))
}

/// Whether the guest can already use ZFS: module loaded, or at least on disk
/// where the kernel will find it at the next boot.
fn netbsd_state(port: u16, mach: &str, kver: &str) -> Result<(bool, bool)> {
    let (_, out, _) = exec_capture(
        port,
        &format!(
            "modstat 2>/dev/null | grep -q '^zfs ' && echo loaded; \
             [ -f /stand/{mach}/{kver}/modules/zfs/zfs.kmod ] && grep -qx zfs /etc/modules.conf 2>/dev/null && echo staged"
        ),
    )?;
    Ok((out.contains("loaded"), out.contains("staged")))
}

/// Make a NetBSD guest able to load ZFS at its next boot, through its agent
/// on `port`. Returns whether anything had to be done (so the caller can say
/// a restart is needed).
pub fn prepare_netbsd(port: u16, version: Option<&str>) -> Result<bool> {
    let (kver, mach) = netbsd_uname(port)?;
    let (loaded, staged) = netbsd_state(port, &mach, &kver)?;
    if loaded || staged {
        return Ok(false);
    }
    let archive = fetch::fetch_netbsd_zfs_modules(version, &kver, &mach)?;
    let file =
        std::fs::File::open(&archive).with_context(|| format!("opening {}", archive.display()))?;
    let argv = [
        "sh".to_string(),
        "-c".to_string(),
        "tar -xzpf - -C /".to_string(),
    ];
    let mut sink = Vec::new();
    let (code, err) = agent::exec_stream(port, &argv, Some(Box::new(file)), &mut sink)?;
    if code != 0 {
        anyhow::bail!("unpacking the zfs modules in the guest: {}", err.trim());
    }
    let (code, out, err) = exec_capture(port, &netbsd_register_script(&mach, &kver))?;
    if code != 0 {
        anyhow::bail!("registering the zfs module: {}", (out + &err).trim());
    }
    Ok(true)
}

/// Prepare a *stopped* NetBSD machine: boot it, provision, stop it again.
/// The boot goes through a child `bsdkrun start` because a boot in this
/// process would print its id and, for a one-shot, exit.
fn prepare_netbsd_machine(vm: &db::MachineRow) -> Result<()> {
    // Once per machine: the modules live on its root disk from then on.
    let marker = Path::new(&vm.state_dir).join(NETBSD_READY_MARKER);
    if marker.exists() {
        return Ok(());
    }
    eprintln!(
        "preparing NetBSD machine {} for ZFS (one-time: ships the kernel's zfs module)…",
        vm.id
    );
    let mut cmd = cli_command()?;
    cmd.args(["start", &vm.id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());
    if std::env::var_os("RUST_LOG").is_none() {
        cmd.env("RUST_LOG", "warn");
    }
    let status = cmd
        .status()
        .map_err(|e| fetch::spawn_error(&cmd, "starting the machine", e))?;
    if !status.success() {
        anyhow::bail!("could not start machine {} to prepare it", vm.id);
    }
    let vdir = PathBuf::from(&vm.state_dir);
    let result = (|| -> Result<bool> {
        let port = wait_for_agent(&vdir)?;
        prepare_netbsd(port, None)
    })();
    let _ = crate::commands::machines::stop(&vm.id);
    if result.is_ok() {
        let _ = std::fs::write(&marker, b"");
    }
    result.map(|_| ())
}

/// Poll the agent port a boot recorded under `vdir` until the agent answers.
fn wait_for_agent(vdir: &Path) -> Result<u16> {
    let deadline = std::time::Instant::now() + AGENT_WAIT;
    loop {
        if let Some(port) = agent::read_port(vdir) {
            if agent::ping(port) {
                return Ok(port);
            }
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!(
                "the guest agent did not come up within {}s",
                AGENT_WAIT.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

// ---------------------------------------------------------------------------
// the post-boot hook (BSD guests)
// ---------------------------------------------------------------------------

/// What a BSD boot registers for the hook: the guest, its pinned NetBSD
/// version (for the module fetch), and the ZFS disks by attach index.
#[derive(Clone, Debug)]
pub struct PostBoot {
    pub os: GuestOs,
    pub version: Option<String>,
    pub disks: Vec<(usize, ZfsDisk)>,
}

static POST_BOOT: Mutex<Option<PostBoot>> = Mutex::new(None);

/// Register the ZFS attachments of the machine about to boot. The boot
/// machinery picks this up (see `run_machine`) and runs [`run_post_boot`]
/// once the agent is up. No-op when no disk is a ZFS volume.
pub fn register_post_boot(os: GuestOs, version: Option<&str>, disks: &[DiskSpec]) {
    let zfs: Vec<(usize, ZfsDisk)> = disks
        .iter()
        .enumerate()
        .filter_map(|(i, d)| d.zfs.clone().map(|z| (i, z)))
        .collect();
    if zfs.is_empty() {
        return;
    }
    *POST_BOOT.lock().unwrap() = Some(PostBoot {
        os,
        version: version.map(str::to_string),
        disks: zfs,
    });
}

/// Take what [`register_post_boot`] left, so exactly one path runs it.
pub fn take_post_boot() -> Option<PostBoot> {
    POST_BOOT.lock().unwrap().take()
}

/// Wait for the guest agent, then import and mount every registered volume.
/// Failures are reported through `notice` (stderr in the foreground, the
/// machine's boot log when detached) and never abort the boot: a guest whose
/// pool did not come up is still a guest you can log in to and look at.
pub fn run_post_boot(vdir: &Path, machine_id: &str, pb: &PostBoot) {
    let notice = |msg: &str| {
        eprintln!("[bsdkrun] zfs: {msg}");
        tracing::warn!(machine = machine_id, "zfs: {msg}");
    };
    let port = match wait_for_agent(vdir) {
        Ok(p) => p,
        Err(e) => {
            notice(&format!("{e:#}; volumes were not imported"));
            return;
        }
    };
    if pb.os == GuestOs::Netbsd {
        match prepare_netbsd(port, pb.version.as_deref()) {
            Ok(false) => {}
            Ok(true) => {
                notice(
                    "NetBSD now loads ZFS at boot; restart this machine (bsdkrun stop/start) to import its volumes",
                );
                return;
            }
            Err(e) => {
                notice(&format!("could not prepare NetBSD for ZFS: {e:#}"));
                return;
            }
        }
    }
    for (index, disk) in &pb.disks {
        let script = setup_script(pb.os, *index, disk);
        match exec_capture(port, &script) {
            Ok((0, out, _)) => {
                for line in out.lines().filter(|l| l.starts_with("[bsdkrun]")) {
                    notice(line.trim_start_matches("[bsdkrun] zfs: "));
                }
                tracing::info!(machine = machine_id, volume = %disk.volume, "zfs volume imported");
            }
            Ok((code, out, err)) => notice(&format!(
                "importing {} failed (exit {code}): {}",
                disk.volume,
                (out + &err).trim()
            )),
            Err(e) => notice(&format!("importing {}: {e:#}", disk.volume)),
        }
    }
}

/// Run the hook on a thread inside the VM process, for boots nobody waits on
/// (`-d`, `start`, a foreground console). The thread dies with the process.
pub fn spawn_post_boot(vdir: PathBuf, machine_id: String, pb: PostBoot) {
    std::thread::Builder::new()
        .name("bsdkrun-zfs".into())
        .spawn(move || run_post_boot(&vdir, &machine_id, &pb))
        .ok();
}

/// Turn `--zfs VOL[@BRANCH][:/mount]` flags into disk specs, one disk per
/// volume, merging several branches of one volume. Validates against the
/// records so a typo fails before the boot, not inside the guest.
pub fn disks_from_flags(specs: &[crate::cli::ZfsAttachSpec]) -> Result<Vec<DiskSpec>> {
    let mut out: Vec<DiskSpec> = Vec::new();
    for s in specs {
        let v = volume_row(&s.volume)?;
        let db = db::Db::open()?;
        if !db
            .list_zfs_branches(&s.volume)?
            .iter()
            .any(|b| b.name == s.branch)
        {
            anyhow::bail!(
                "no branch {:?} in zfs volume {} (see `bsdkrun zfs ls {}`)",
                s.branch,
                s.volume,
                s.volume
            );
        }
        if let Some(prev) = displace_holder(&s.volume, None)? {
            eprintln!(
                "{} was attached to stopped machine {prev}; moving it",
                s.volume
            );
        }
        let mount = ZfsMount {
            branch: s.branch.clone(),
            mountpoint: s
                .mountpoint
                .clone()
                .unwrap_or_else(|| default_mountpoint(&s.volume, &s.branch)),
        };
        match out.iter_mut().find(|d| {
            d.zfs
                .as_ref()
                .map(|z| z.volume == s.volume)
                .unwrap_or(false)
        }) {
            Some(d) => d.zfs.as_mut().unwrap().mounts.push(mount),
            None => out.push(DiskSpec {
                path: PathBuf::from(&v.path),
                read_only: false,
                mount: None,
                zfs: Some(ZfsDisk {
                    volume: s.volume.clone(),
                    mounts: vec![mount],
                }),
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(mounts: &[(&str, &str)]) -> ZfsDisk {
        ZfsDisk {
            volume: "data".into(),
            mounts: mounts
                .iter()
                .map(|(b, m)| ZfsMount {
                    branch: b.to_string(),
                    mountpoint: m.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn names() {
        assert!(validate_name("volume", "data").is_ok());
        assert!(validate_name("volume", "my-db_2.0").is_ok());
        assert!(validate_name("volume", "").is_err());
        assert!(validate_name("volume", "-x").is_err());
        assert!(validate_name("volume", "a b").is_err());
        assert!(validate_name("volume", "a/b").is_err());
        assert!(validate_name("volume", "mirror").is_err());
    }

    #[test]
    fn targets() {
        assert_eq!(parse_target("data").unwrap(), ("data".into(), None));
        assert_eq!(
            parse_target("data@feat").unwrap(),
            ("data".into(), Some("feat".into()))
        );
        assert!(parse_target("data@").is_err());
        assert!(parse_target("@x").is_err());
    }

    #[test]
    fn mountpoints() {
        assert_eq!(default_mountpoint("data", "main"), "/mnt/data");
        assert_eq!(default_mountpoint("data", "feat"), "/mnt/data-feat");
    }

    /// The device each guest looks for is the attach index, not a fixed name:
    /// FreeBSD counts from the root disk, NetBSD from whatever `hw.disknames`
    /// lists, Linux from `vda`.
    #[test]
    fn device_lookup_per_guest() {
        assert_eq!(device_expr(GuestOs::Freebsd, 0), "/dev/vtbd1");
        assert_eq!(device_expr(GuestOs::Freebsd, 2), "/dev/vtbd3");
        assert!(device_expr(GuestOs::Linux, 0).contains("sed -n '1p'"));
        // NetBSD's first `ld` is the root disk: attached disk #2 is the third.
        let nb = device_expr(GuestOs::Netbsd, 1);
        assert!(nb.contains("hw.disknames"));
        assert!(nb.contains("kern.rawpartition"));
        assert!(nb.contains("sed -n '3p'"));
        assert!(device_expr(GuestOs::Netbsd, 0).contains("sed -n '2p'"));
    }

    #[test]
    fn script_imports_before_creating_and_mounts_each_branch() {
        let s = setup_script(
            GuestOs::Freebsd,
            0,
            &disk(&[("main", "/mnt/data"), ("feat", "/mnt/data-feat")]),
        );
        let import = s.find("bk_zfs_import -d \"$dev\"").unwrap();
        let create = s.find("zpool create").unwrap();
        assert!(
            import < create,
            "a disk that holds the pool is imported, never recreated"
        );
        assert!(s.contains("kldload zfs"));
        assert!(s.contains("zfs create -o canmount=noauto \"$pool/main\""));
        assert!(s.contains("ds=\"$pool/main\"; mp='/mnt/data'"));
        assert!(s.contains("ds=\"$pool/feat\"; mp='/mnt/data-feat'"));
        // Portable feature set: NetBSD must be able to import a pool FreeBSD made.
        assert!(s.contains("zpool create -f -o cachefile=none -O mountpoint=none -O canmount=noauto -m none -d -o feature@"));
        assert!(s.contains("zpool import -f -N -o cachefile=none"));
        assert!(s.contains("-o feature@lz4_compress=enabled"));
        assert!(!s.contains("zstd"));
    }

    /// The create path refuses a disk anything is mounted from, so an off-by-one
    /// in the device lookup cannot format a root disk.
    #[test]
    fn create_is_guarded_against_mounted_disks() {
        let s = setup_script(GuestOs::Netbsd, 0, &disk(&[]));
        let guard = s.find("refusing to create pool").unwrap();
        let create = s.find("zpool create").unwrap();
        assert!(guard < create);
        assert!(s.contains("listwedges"));
    }

    #[test]
    fn netbsd_creates_with_its_own_defaults_and_cannot_load_live() {
        let s = setup_script(GuestOs::Netbsd, 0, &disk(&[]));
        assert!(!s.contains("feature@"));
        assert!(s.contains("/etc/modules.conf"));
        assert!(!s.contains("modload"));
    }

    #[test]
    fn mountpoints_are_shell_quoted() {
        let s = setup_script(GuestOs::Linux, 0, &disk(&[("main", "/mnt/it's")]));
        assert!(s.contains("mp='/mnt/it'\\''s'"));
    }
}
