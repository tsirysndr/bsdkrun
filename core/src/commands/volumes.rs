//! `bsdkrun volume` — named volumes, listed and removed.

use anyhow::{Context, Result};

use std::path::PathBuf;

use crate::{api, db, oci};

use super::truncate;

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_volume_ls(json: bool) -> Result<()> {
    // Untracked on-disk volumes and the flavor-build filter are handled by
    // `api`, so the daemon sees the same set this table does.
    let volumes = api::list_volumes()?;
    if json {
        println!("{}", serde_json::to_string(&volumes)?);
        return Ok(());
    }
    println!(
        "{:<20}  {:<9}  {:<28}  {:<10}  {}",
        "NAME", "GUEST", "BASE", "SIZE", "CREATED"
    );
    for v in &volumes {
        println!(
            "{:<20}  {:<9}  {:<28}  {:<10}  {}",
            truncate(&v.name, 20),
            v.guest.as_deref().unwrap_or("-"),
            truncate(v.base.as_deref().unwrap_or("-"), 28),
            v.size.as_deref().unwrap_or("-"),
            v.created_at
                .as_deref()
                .map_or_else(|| "-".to_string(), db::age),
        );
    }
    Ok(())
}

/// On-disk size of a volume via `du -sk` (counts allocated blocks, so CoW-shared
/// data isn't double-counted); "-" if it can't be determined.
pub(crate) fn volume_size(path: &str) -> String {
    let out = std::process::Command::new("du")
        .args(["-sk", path])
        .output()
        .ok();
    out.filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .map(|kb| oci::human_size(kb * 1024))
        .unwrap_or_else(|| "-".to_string())
}

pub(crate) fn cmd_volume_rm(names: &[String], force: bool) -> Result<()> {
    let mut failed = false;
    for name in names {
        match remove_volume(name, force) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("Error: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// Delete one volume's directory and its row. Refuses a volume attached to a
/// running machine unless `force`.
pub fn remove_volume(name: &str, force: bool) -> Result<String> {
    let db = db::Db::open()?;
    let row = db.find_volume(name)?;
    let dir = match &row {
        Some(r) => PathBuf::from(&r.path),
        None => db::volumes_dir()?.join(name),
    };
    if row.is_none() && !dir.exists() {
        anyhow::bail!("no such volume: {name}");
    }
    // Volumes currently attached to a running machine.
    let in_use = db
        .list_machines()?
        .into_iter()
        .filter(|m| m.pid.map(db::pid_alive).unwrap_or(false))
        .any(|m| m.volume.as_deref() == Some(name));
    if in_use && !force {
        anyhow::bail!("volume {name:?} is in use by a running machine (use --force)");
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    db.remove_volume(name).ok();
    Ok(name.to_string())
}
