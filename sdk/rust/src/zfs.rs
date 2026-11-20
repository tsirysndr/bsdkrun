//! ZFS volumes: one zpool per volume, branched copy-on-write in the guest.

use serde_json::Value;

use crate::error::Result;
use crate::process::run_checked;
use crate::types::{ZfsBranch, ZfsVolumeInfo};

fn rows(stdout: String) -> Result<Vec<Value>> {
    let raw = if stdout.trim().is_empty() {
        "[]".to_string()
    } else {
        stdout
    };
    let v: Value = serde_json::from_str(&raw)?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// List ZFS volumes, with their branches and holder.
pub fn list() -> Result<Vec<ZfsVolumeInfo>> {
    let res = run_checked(["zfs", "ls", "--json"], "bsdkrun zfs ls")?;
    Ok(rows(res.stdout)?
        .iter()
        .map(ZfsVolumeInfo::from_row)
        .collect())
}

/// The branches of one volume.
pub fn branches(volume: &str) -> Result<Vec<ZfsBranch>> {
    let res = run_checked(["zfs", "ls", volume, "--json"], "bsdkrun zfs ls")?;
    Ok(rows(res.stdout)?.iter().map(ZfsBranch::from_row).collect())
}

/// Create a volume: a sparse image that becomes a zpool with a `main` branch
/// the first time a guest imports it. `size` like `"10G"` (the default).
pub fn create(name: &str, size: Option<&str>) -> Result<String> {
    let mut args = vec!["zfs".to_string(), "create".to_string(), name.to_string()];
    if let Some(s) = size {
        args.push("--size".to_string());
        args.push(s.to_string());
    }
    Ok(run_checked(args, "bsdkrun zfs create")?
        .stdout
        .trim()
        .to_string())
}

/// Fork a branch copy-on-write (`zfs snapshot` + `zfs clone`) from `from`
/// (`main` when `None`). Runs in the machine holding the volume or in a
/// short-lived FreeBSD helper, so it can take as long as a BSD boot. Returns
/// the new `VOL@BRANCH`.
pub fn branch(volume: &str, name: &str, from: Option<&str>) -> Result<String> {
    let mut args = vec![
        "zfs".to_string(),
        "branch".to_string(),
        volume.to_string(),
        name.to_string(),
    ];
    if let Some(f) = from {
        args.push("--from".to_string());
        args.push(f.to_string());
    }
    Ok(run_checked(args, "bsdkrun zfs branch")?
        .stdout
        .trim()
        .to_string())
}

/// Attach `VOL` or `VOL@BRANCH` to a machine at `mount` (`/mnt/VOL`, or
/// `/mnt/VOL-BRANCH`, when `None`). A pool is imported by one machine at a
/// time; a stopped machine picks it up on start, a running holder mounts the
/// branch live. Returns the CLI's one-line report.
pub fn attach(target: &str, machine: &str, mount: Option<&str>) -> Result<String> {
    let mut args = vec![
        "zfs".to_string(),
        "attach".to_string(),
        target.to_string(),
        machine.to_string(),
    ];
    if let Some(m) = mount {
        args.push("--mount".to_string());
        args.push(m.to_string());
    }
    Ok(run_checked(args, "bsdkrun zfs attach")?
        .stdout
        .trim()
        .to_string())
}

/// Detach one branch (`VOL@BRANCH`) or the whole volume (`VOL`) from a machine.
pub fn detach(target: &str, machine: &str) -> Result<String> {
    let args = ["zfs", "detach", target, machine];
    Ok(run_checked(args, "bsdkrun zfs detach")?
        .stdout
        .trim()
        .to_string())
}

/// Destroy branches (`VOL@BRANCH`) or whole volumes (`VOL`). `force` destroys
/// a volume a stopped machine still has attached.
pub fn destroy<S: AsRef<str>>(targets: &[S], force: bool) -> Result<()> {
    let mut args = vec!["zfs".to_string(), "destroy".to_string()];
    if force {
        args.push("--force".to_string());
    }
    args.extend(targets.iter().map(|t| t.as_ref().to_string()));
    run_checked(args, "bsdkrun zfs destroy")?;
    Ok(())
}
