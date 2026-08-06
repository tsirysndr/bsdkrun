//! Host resource stats for the status bar: CPU% and RAM, plus the real on-disk
//! footprint of all microVMs.
//!
//! Ported from the desktop app's `system.rs` so the web UI's status bar shows
//! the same numbers. The daemon is the only thing that can measure these for a
//! remote host — a browser has no view of the machine actually running the VMs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sysinfo::System;

#[derive(Debug, Clone)]
pub struct SystemStats {
    /// Host CPU usage, 0–100 (%).
    pub cpu: f32,
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

/// Sum actual allocated blocks (`st_blocks` × 512) under `path`.
///
/// Apparent size would be badly misleading here: APFS `clonefile`/reflink CoW
/// disks share blocks, so a dozen clones of a 2 GiB base cost far less than
/// 24 GiB and the status bar would otherwise report disk the host never used.
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

/// Total real disk usage of all microVMs plus persistent volumes, and how many
/// machine dirs exist.
fn vm_disk_usage() -> (u64, u32) {
    let base = state_dir();
    let machines = base.join("machines");
    let volumes = base.join("volumes");

    let count = std::fs::read_dir(&machines)
        .map(|it| it.flatten().filter(|e| e.path().is_dir()).count() as u32)
        .unwrap_or(0);

    (dir_real_size(&machines) + dir_real_size(&volumes), count)
}

/// A `System` kept between samples.
///
/// sysinfo computes CPU usage as a delta against the previous refresh, so a
/// fresh `System` every call would report 0% forever.
static SYS: Mutex<Option<System>> = Mutex::new(None);

/// Sample the host. Walks the state dir, so callers should treat it as blocking.
pub fn sample() -> SystemStats {
    let (cpu, mem_used, mem_total) = {
        let mut guard = SYS.lock().unwrap_or_else(|e| e.into_inner());
        let sys = guard.get_or_insert_with(System::new);
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        (
            sys.global_cpu_usage(),
            sys.used_memory(),
            sys.total_memory(),
        )
    };

    let (vm_disk, vm_count) = vm_disk_usage();
    SystemStats {
        cpu,
        mem_used,
        mem_total,
        vm_disk,
        vm_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_follows_the_cli_precedence() {
        // Not exercising the env (process-global, and tests run in parallel) —
        // just that the default lands under the documented path.
        let d = state_dir();
        assert!(
            d.ends_with("bsdkrun") || d.to_string_lossy().contains("bsdkrun"),
            "unexpected state dir: {}",
            d.display()
        );
    }

    #[test]
    fn sampling_twice_reports_plausible_memory() {
        let a = sample();
        let b = sample();
        assert!(a.mem_total > 0, "total memory should be known");
        assert_eq!(a.mem_total, b.mem_total);
        assert!(a.mem_used <= a.mem_total);
        assert!((0.0..=100.0).contains(&b.cpu), "cpu was {}", b.cpu);
    }

    #[test]
    fn a_missing_directory_measures_zero_rather_than_failing() {
        assert_eq!(dir_real_size(Path::new("/nonexistent-bsdkrun-state")), 0);
    }
}
