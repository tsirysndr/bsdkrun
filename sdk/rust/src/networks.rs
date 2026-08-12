//! Global-network operations — reach machines by name on a shared subnet.

use serde_json::Value;

use crate::error::Result;
use crate::process::run_checked;
use crate::sandbox::Sandbox;
use crate::types::{NetworkInfo, SandboxInfo};

/// List global networks and their member counts.
pub fn list() -> Result<Vec<NetworkInfo>> {
    let res = run_checked(["network", "ls", "--json"], "bsdkrun network ls")?;
    let raw = if res.stdout.trim().is_empty() {
        "[]".to_string()
    } else {
        res.stdout
    };
    let rows: Value = serde_json::from_str(&raw)?;
    Ok(rows
        .as_array()
        .map(|rows| rows.iter().map(NetworkInfo::from_row).collect())
        .unwrap_or_default())
}

/// Create a global network (starts its shared switch).
pub fn create(name: &str) -> Result<()> {
    run_checked(["network", "create", name], "bsdkrun network create")?;
    Ok(())
}

/// Remove one or more networks. `force` allows removal with live members.
pub fn remove<S: AsRef<str>>(names: &[S], force: bool) -> Result<()> {
    let mut args = vec!["network".to_string(), "rm".to_string()];
    if force {
        args.push("--force".to_string());
    }
    args.extend(names.iter().map(|n| n.as_ref().to_string()));
    run_checked(args, "bsdkrun network rm")?;
    Ok(())
}

/// Join or switch a machine (by id or name) to a network (next start).
pub fn connect(machine: &str, network: &str) -> Result<()> {
    run_checked(
        ["network", "connect", machine, network],
        "bsdkrun network connect",
    )?;
    Ok(())
}

/// Detach a machine from its network. Applies on its next start.
pub fn disconnect(machine: &str) -> Result<()> {
    run_checked(
        ["network", "disconnect", machine],
        "bsdkrun network disconnect",
    )?;
    Ok(())
}

/// Refresh members' `/etc/hosts` so peers resolve by name (notably NetBSD).
pub fn sync(network: &str) -> Result<()> {
    run_checked(["network", "sync", network], "bsdkrun network sync")?;
    Ok(())
}

/// The machines currently attached to `network` (running or stopped).
pub fn members(network: &str) -> Result<Vec<SandboxInfo>> {
    Ok(Sandbox::list(true)?
        .into_iter()
        .filter(|m| m.network.as_deref() == Some(network))
        .collect())
}
