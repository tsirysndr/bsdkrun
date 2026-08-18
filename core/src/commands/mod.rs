//! One module per group of subcommands, each holding what used to sit in the
//! CLI's `main.rs`.
//!
//! These still print — a `ps` here writes the same table it always did — so the
//! CLI is a thin pass-through. The daemon does not call them; it goes through
//! [`crate::api`], which returns the same information as data.
//!
//! Which is why a build without `boot` leaves most of them unused: the printing
//! layer exists for the CLI, and a build that cannot start a machine is not the
//! CLI. `api` and the helpers underneath are what such a build is for.
#![cfg_attr(not(feature = "boot"), allow(dead_code))]

pub mod ai;
#[cfg(feature = "boot")]
pub mod boot;
pub mod cache;
#[cfg(feature = "ci")]
pub mod ci;
pub mod cp;
pub mod docker;
pub mod doctor;
pub mod domains;
pub mod flavor;
pub mod guest;
pub mod images;
pub mod machines;
#[cfg(feature = "pack")]
pub mod pack;
#[cfg(feature = "boot")]
pub mod probe;
/// `bsdkrun prune` — reclaim disk from machines, images, volumes and caches
/// that nothing is using.
pub mod prune;
pub mod snapshot;
/// Needs `boot` as well as `solo5`: a build that cannot start a machine has no
/// `dispatch` to reach this from, and it shares the console and machine-record
/// machinery with the libkrun boot paths.
#[cfg(all(feature = "solo5", feature = "boot"))]
pub mod solo5;
#[cfg(target_os = "macos")]
pub mod store;
#[cfg(all(feature = "boot", feature = "tui"))]
pub mod tui;
pub mod volumes;

use std::path::PathBuf;

use anyhow::Result;

use crate::db;

/// Truncate a string to `n` display chars, adding an ellipsis if cut.
///
/// Shared by every table-printing subcommand, which is why it lives here rather
/// than beside any one of them.
pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

// ---------------------------------------------------------------------------
// paths
// ---------------------------------------------------------------------------
//
// Where a machine and a volume keep their files. These sit here rather than
// beside the boot code that mostly uses them because `commit` and the volume
// listing need them too, and neither should pull in a hypervisor.

/// Per-machine state dir (`<state>/machines/<id>`), falling back to a temp dir.
pub(crate) fn machine_dir_or_tmp(id: &str) -> std::path::PathBuf {
    let dir = db::machine_dir(id).unwrap_or_else(|_| std::env::temp_dir().join(id));
    std::fs::create_dir_all(&dir).ok();
    dir
}

/// Directory that will hold a machine's writable rootfs clone. On macOS with a
/// case-sensitive store set up this lives on the store (nix guests need that,
/// and the clone stays CoW because source and destination share a volume);
/// everywhere else it is the machine's own state dir, as before.
pub(crate) fn machine_rootfs_dir(id: &str, vdir: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(d) = crate::store::machine_rootfs_dir(id) {
            return d;
        }
    }
    let _ = id;
    vdir.to_path_buf()
}

/// File in a machine's state dir recording its `--attach-disk` specs, so
/// `start` can re-attach the same disks (the machines DB row doesn't carry
/// them).
const ATTACHED_DISKS_FILE: &str = "attached-disks.json";

/// Record `--attach-disk` specs in the machine's state dir for restarts. Paths
/// are made absolute (a later `start` runs from a different cwd). An empty
/// list clears the record.
pub(crate) fn save_attached_disks(vdir: &std::path::Path, disks: &[crate::cli::DiskSpec]) {
    let file = vdir.join(ATTACHED_DISKS_FILE);
    if disks.is_empty() {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let abs: Vec<crate::cli::DiskSpec> = disks
        .iter()
        .map(|d| crate::cli::DiskSpec {
            path: std::fs::canonicalize(&d.path).unwrap_or_else(|_| d.path.clone()),
            read_only: d.read_only,
            mount: d.mount.clone(),
        })
        .collect();
    match serde_json::to_vec(&abs) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&file, json) {
                tracing::warn!(file = %file.display(), "could not record attached disks: {e}");
            }
        }
        Err(e) => tracing::warn!("could not encode attached disks: {e}"),
    }
}

/// Load the disks recorded by [`save_attached_disks`]. A disk whose image file
/// has since disappeared is skipped with a warning rather than failing the
/// whole restart.
pub(crate) fn load_attached_disks(vdir: &std::path::Path) -> Vec<crate::cli::DiskSpec> {
    let Ok(bytes) = std::fs::read(vdir.join(ATTACHED_DISKS_FILE)) else {
        return vec![];
    };
    let disks: Vec<crate::cli::DiskSpec> = serde_json::from_slice(&bytes).unwrap_or_default();
    disks
        .into_iter()
        .filter(|d| {
            let ok = d.path.exists();
            if !ok {
                tracing::warn!(
                    path = %d.path.display(),
                    "recorded attach-disk image is gone; restarting without it"
                );
            }
            ok
        })
        .collect()
}

/// File in a machine's state dir recording its `-e` environment, so `start`
/// re-applies it. The DB row doesn't carry it, and a Linux guest's entrypoint
/// often *is* its configuration: `docker:dind` picks TLS-on-2376 versus
/// plaintext-on-2375 purely from `DOCKER_TLS_CERTDIR`, so a restart that
/// dropped the variable silently came back on a different port.
const ENV_FILE: &str = "env.json";

/// Record `-e K=V` pairs for restarts. An empty list clears the record.
pub(crate) fn save_env(vdir: &std::path::Path, env: &[String]) {
    let file = vdir.join(ENV_FILE);
    if env.is_empty() {
        let _ = std::fs::remove_file(&file);
        return;
    }
    match serde_json::to_vec(env) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&file, json) {
                tracing::warn!(file = %file.display(), "could not record the guest env: {e}");
            }
        }
        Err(e) => tracing::warn!("could not encode the guest env: {e}"),
    }
}

/// Load what [`save_env`] wrote. Empty for a machine booted before this, or
/// one that never had `-e`.
pub(crate) fn load_env(vdir: &std::path::Path) -> Vec<String> {
    std::fs::read(vdir.join(ENV_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// File in a machine's state dir recording its `--mount` shares, so `start`
/// re-applies them. Losing a share on restart is worse than losing a flag: the
/// guest path still exists (the init, or Docker, creates it), so the mount is
/// silently *empty* rather than missing.
const MOUNTS_FILE: &str = "mounts.json";

/// Record `--mount HOST:GUEST[:ro]` specs for restarts, with the host side
/// made absolute (a later `start` runs from a different cwd).
pub(crate) fn save_mounts(vdir: &std::path::Path, mounts: &[crate::linux::BindMount]) {
    let file = vdir.join(MOUNTS_FILE);
    if mounts.is_empty() {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let specs: Vec<String> = mounts
        .iter()
        .map(|m| {
            let host = std::fs::canonicalize(&m.host).unwrap_or_else(|_| m.host.clone());
            let ro = if m.ro { ":ro" } else { "" };
            format!("{}:{}{ro}", host.display(), m.guest)
        })
        .collect();
    match serde_json::to_vec(&specs) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&file, json) {
                tracing::warn!(file = %file.display(), "could not record mounts: {e}");
            }
        }
        Err(e) => tracing::warn!("could not encode mounts: {e}"),
    }
}

/// Load what [`save_mounts`] wrote, as `--mount` specs. A share whose host
/// directory is gone is dropped with a warning rather than failing the restart.
pub(crate) fn load_mounts(vdir: &std::path::Path) -> Vec<String> {
    let specs: Vec<String> = std::fs::read(vdir.join(MOUNTS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    specs
        .into_iter()
        .filter(|spec| {
            let host = spec.rsplit_once(':').map(|(h, _)| h).unwrap_or(spec);
            let ok = std::path::Path::new(host).exists();
            if !ok {
                tracing::warn!(
                    spec,
                    "a recorded --mount host directory is gone; skipping it"
                );
            }
            ok
        })
        .collect()
}

/// The `--mount` specs recorded for a machine, for anything that wants to
/// *show* what is shared (`docker status`) rather than re-apply it.
pub fn machine_mounts(vdir: &std::path::Path) -> Vec<String> {
    load_mounts(vdir)
}

/// Resolve a `--volume NAME` to its directory under `<state>/volumes`, rejecting
/// names that could escape it.
pub(crate) fn volume_dir(name: &str) -> Result<PathBuf> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!("invalid volume name {name:?} — use letters, digits, '-', '_' or '.'");
    }
    Ok(db::volumes_dir()?.join(name))
}

/// The last path component, for display.
pub(crate) fn basename(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}
