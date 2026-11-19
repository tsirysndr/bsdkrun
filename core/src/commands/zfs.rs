//! `bsdkrun zfs` — ZFS volumes: created, branched, attached, destroyed.
//!
//! Printing only; the work is in [`crate::zfs`], which the daemon reaches
//! through [`crate::api`] so both front ends run the same code.

use anyhow::Result;

use crate::{db, zfs};

use super::truncate;

pub(crate) fn cmd_create(name: &str, size: &str) -> Result<()> {
    let created = zfs::create(name, Some(size))?;
    println!("{created}");
    Ok(())
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_ls(volume: Option<&str>, json: bool) -> Result<()> {
    let volumes = zfs::list()?;
    if let Some(name) = volume {
        let Some(v) = volumes.iter().find(|v| v.name == name) else {
            anyhow::bail!("no such zfs volume: {name}");
        };
        if json {
            println!("{}", serde_json::to_string(&v.branches)?);
            return Ok(());
        }
        println!(
            "{:<24}  {:<24}  {:<28}  {}",
            "BRANCH", "ORIGIN", "MOUNTED", "CREATED"
        );
        for b in &v.branches {
            let mounted = v
                .attached_to
                .iter()
                .flat_map(|a| a.mounts.iter().map(move |m| (a, m)))
                .find(|(_, m)| m.branch == b.name)
                .map(|(a, m)| {
                    format!(
                        "{}:{}",
                        a.machine_name.as_deref().unwrap_or(&a.machine),
                        m.mountpoint
                    )
                })
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{:<24}  {:<24}  {:<28}  {}",
                truncate(&b.name, 24),
                truncate(b.origin.as_deref().unwrap_or("-"), 24),
                truncate(&mounted, 28),
                b.created_at
                    .as_deref()
                    .map_or_else(|| "-".to_string(), db::age),
            );
        }
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string(&volumes)?);
        return Ok(());
    }
    println!(
        "{:<20}  {:<9}  {:<10}  {:<8}  {:<24}  {}",
        "NAME", "DISK", "USED", "BRANCHES", "ATTACHED", "CREATED"
    );
    for v in &volumes {
        let attached = v
            .attached_to
            .first()
            .map(|a| {
                let who = a.machine_name.as_deref().unwrap_or(&a.machine);
                if a.running {
                    format!("{who} (running)")
                } else {
                    who.to_string()
                }
            })
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<20}  {:<9}  {:<10}  {:<8}  {:<24}  {}",
            truncate(&v.name, 20),
            crate::oci::human_size(v.size_bytes),
            v.size.as_deref().unwrap_or("-"),
            v.branches.len(),
            truncate(&attached, 24),
            v.created_at
                .as_deref()
                .map_or_else(|| "-".to_string(), db::age),
        );
    }
    Ok(())
}

pub(crate) fn cmd_branch(volume: &str, name: &str, from: Option<&str>) -> Result<()> {
    let created = zfs::branch(volume, name, from)?;
    println!("{created}");
    Ok(())
}

pub(crate) fn cmd_attach(target: &str, machine: &str, mount: Option<&str>) -> Result<()> {
    println!("{}", zfs::attach(target, machine, mount)?);
    Ok(())
}

pub(crate) fn cmd_detach(target: &str, machine: &str) -> Result<()> {
    println!("{}", zfs::detach(target, machine)?);
    Ok(())
}

pub(crate) fn cmd_destroy(targets: &[String], force: bool) -> Result<()> {
    let mut failed = false;
    for t in targets {
        match zfs::destroy(t, force) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("Error: {e:#}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
