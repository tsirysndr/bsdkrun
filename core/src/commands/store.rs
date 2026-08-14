//! `bsdkrun store` — the case-sensitive APFS sparsebundle behind image rootfs
//! trees and named volumes on macOS.

use anyhow::Result;

use crate::store;

/// Create the case-sensitive store, then move existing named volumes onto it.
/// The image cache is dropped rather than moved — see [`store::migrate`].
#[cfg(target_os = "macos")]
pub(crate) fn cmd_store_init(size: &str) -> Result<()> {
    store::ensure_no_running_machines()?;
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
