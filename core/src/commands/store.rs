//! `bsdkrun store` — the case-sensitive APFS sparsebundle behind image rootfs
//! trees and named volumes on macOS.

use anyhow::Result;

use crate::{db, store};

/// Create the case-sensitive store, then move existing named volumes onto it.
/// The image cache is dropped rather than moved — see [`store::migrate`].
#[cfg(target_os = "macos")]
pub(crate) fn cmd_store_init(size: &str) -> Result<()> {
    // A stale "running" row whose pid is gone must not block the migration, so
    // confirm liveness rather than trusting the recorded status.
    let running = db::Db::open()
        .and_then(|d| d.list_machines())
        .map(|ms| {
            ms.iter()
                .filter(|m| m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);
    if running > 0 {
        anyhow::bail!(
            "{running} machine(s) still running — stop them first, since their rootfs \
             moves onto the new store"
        );
    }
    store::create(size)?;
    store::migrate()?;
    println!("case-sensitive store ready at {}", store::root()?.display());
    println!("images will re-pull into it on next use; nix guests now work.");
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn cmd_store_status() -> Result<()> {
    println!("{}", store::describe()?);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn cmd_store_attach() -> Result<()> {
    store::attach()?;
    println!("{}", store::describe()?);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn cmd_store_detach(force: bool) -> Result<()> {
    store::detach(force)?;
    println!("store detached — images and volumes on it are unavailable until reattached");
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn cmd_store_rm(force: bool) -> Result<()> {
    if !store::exists() {
        println!("no store to remove");
        return Ok(());
    }
    if !force {
        anyhow::bail!(
            "removing the store destroys every image and named volume on it — pass -f to confirm"
        );
    }
    store::remove(true)?;
    println!("store removed; the default cache and volume directories are in use again");
    Ok(())
}

