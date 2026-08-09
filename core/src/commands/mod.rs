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

#[cfg(feature = "boot")]
pub mod boot;
pub mod flavor;
pub mod guest;
pub mod images;
pub mod machines;
#[cfg(feature = "boot")]
pub mod probe;
#[cfg(target_os = "macos")]
pub mod store;
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
