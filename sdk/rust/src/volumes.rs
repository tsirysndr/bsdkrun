//! Host-level volume operations.

use serde_json::Value;

use crate::error::Result;
use crate::process::run_checked;
use crate::types::VolumeInfo;

/// List persistent volumes.
pub fn list() -> Result<Vec<VolumeInfo>> {
    let res = run_checked(["volume", "ls", "--json"], "bsdkrun volume ls")?;
    let raw = if res.stdout.trim().is_empty() {
        "[]".to_string()
    } else {
        res.stdout
    };
    let rows: Value = serde_json::from_str(&raw)?;
    Ok(rows
        .as_array()
        .map(|rows| rows.iter().map(VolumeInfo::from_row).collect())
        .unwrap_or_default())
}

/// Remove one or more volumes (and their data). `force` removes them even
/// while referenced.
pub fn remove<S: AsRef<str>>(names: &[S], force: bool) -> Result<()> {
    let mut args = vec!["volume".to_string(), "rm".to_string()];
    if force {
        args.push("--force".to_string());
    }
    args.extend(names.iter().map(|n| n.as_ref().to_string()));
    run_checked(args, "bsdkrun volume rm")?;
    Ok(())
}
