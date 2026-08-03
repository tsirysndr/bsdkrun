//! Host resource stats for the Docker-Desktop-style status bar: host CPU% and
//! RAM (via sysinfo), plus the *real* on-disk footprint of all microVMs.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Serialize)]
pub struct SystemStats {
    /// Host CPU usage, 0–100 (%).
    pub cpu: f32,
    /// Host memory in bytes.
    pub mem_used: u64,
    pub mem_total: u64,
    /// Real (CoW/sparse-aware) bytes used by all microVM state dirs + volumes.
    pub vm_disk: u64,
    /// Number of machine state dirs on disk.
    pub vm_count: u32,
}

/// bsdkrun's state dir: `$BSDKRUN_STATE`, else `$XDG_STATE_HOME/bsdkrun`, else
/// `$HOME/.local/state/bsdkrun` (mirrors the CLI).
fn state_dir() -> PathBuf {
    if let Ok(s) = std::env::var("BSDKRUN_STATE") {
        if !s.is_empty() {
            return PathBuf::from(s);
        }
    }
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("bsdkrun");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/state/bsdkrun")
}

/// Sum actual allocated blocks (st_blocks × 512) under `path`. Unlike apparent
/// size this reflects real usage — APFS `clonefile`/reflink CoW disks share
/// blocks, so a dozen clones of a 2 GiB base cost far less than 24 GiB.
#[cfg(unix)]
fn dir_real_size(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&p) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(md) = e.metadata() {
                total += md.blocks() * 512;
            }
        }
    }
    total
}

#[cfg(not(unix))]
fn dir_real_size(_path: &Path) -> u64 {
    0
}

/// Total real disk usage of all microVMs (their per-machine state dirs) plus
/// persistent volumes, and how many machine dirs exist. Runs blocking IO.
pub fn vm_disk_usage() -> (u64, u32) {
    let base = state_dir();
    let machines = base.join("machines");
    let volumes = base.join("volumes");

    let count = std::fs::read_dir(&machines)
        .map(|it| it.flatten().filter(|e| e.path().is_dir()).count() as u32)
        .unwrap_or(0);

    let bytes = dir_real_size(&machines) + dir_real_size(&volumes);
    (bytes, count)
}
