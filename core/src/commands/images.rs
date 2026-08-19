//! `bsdkrun images` — every image this host has pulled or fetched.

use anyhow::Result;

use crate::{api, db, fetch, oci};

use super::truncate;

/// Record any BSD disk images sitting in the cache that aren't in the DB yet, so
/// `images` lists them (they may predate the DB, or be from another checkout).
pub(crate) fn reconcile_bsd_images() {
    let Ok(cache) = fetch::cache_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&cache) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Fetched images are named `<os>-<version>.<ext>` (freebsd → raw, netbsd → img).
        let is_bsd = (name.starts_with("freebsd-") && name.ends_with(".raw"))
            || (name.starts_with("netbsd-") && name.ends_with(".img"));
        if !is_bsd {
            continue;
        }
        let path = entry.path();
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        let reference = name.trim_end_matches(".raw").trim_end_matches(".img");
        db::record_image(
            reference,
            &format!("file:{}", path.display()),
            size,
            &path.to_string_lossy(),
        );
    }
}

/// Which machines still reference an image, by id and by reference.
///
/// A machine's row records the *reference* it booted (`alpine`, `disk.raw`),
/// so both are checked — removing the rootfs from under a stopped machine
/// would leave it unable to start with no hint as to why.
pub fn image_users(image: &db::ImageRow) -> Result<Vec<String>> {
    let db = db::Db::open()?;
    Ok(db
        .list_machines()?
        .into_iter()
        .filter(|m| m.image == image.reference || m.image == image.id)
        .map(|m| m.name.unwrap_or(m.id))
        .collect())
}

/// Remove an image and its extracted rootfs.
///
/// Only a **dangling** image — one no machine references — can go. The rootfs
/// is shared by every machine cloned from it, so deleting one still in use is
/// not a mess to warn about, it is a machine that silently cannot boot.
pub fn remove_image(id: &str, force: bool) -> Result<String> {
    let db = db::Db::open()?;
    let image = db
        .find_image(id)?
        .ok_or_else(|| anyhow::anyhow!("no such image: {id}"))?;

    let users = image_users(&image)?;
    if !users.is_empty() && !force {
        anyhow::bail!(
            "{} is in use by {} ({}) — remove those machines first",
            image.reference,
            users.len(),
            users.join(", ")
        );
    }

    // Rename-aside + background GC: an extracted nix rootfs takes long enough
    // to delete that the UI's button would look hung.
    if !image.rootfs.is_empty() {
        let path = std::path::PathBuf::from(&image.rootfs);
        // Only bsdkrun's own cache is ours to delete. A `file:`-backed BSD
        // image the user fetched elsewhere is left alone.
        if path.exists() {
            crate::host::remove_dir_all_detached(&path);
        }
    }
    db.remove_image(&image.id)?;
    Ok(image.reference)
}

/// `bsdkrun image rm <id>...`
pub(crate) fn cmd_image_rm(ids: &[String], force: bool) -> Result<()> {
    let mut failed = false;
    for id in ids {
        match remove_image(id, force) {
            Ok(reference) => println!("{reference}"),
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

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
/// Pull (or reuse from cache) and report where the rootfs landed plus the
/// image's runtime config — what lets tooling on top (the CI runner's drone
/// plugin support) execute an image's entrypoint without a container
/// runtime: the rootfs is a directory, the config says what to run in it.
pub(crate) fn cmd_image_pull(reference: &str, json: bool) -> Result<()> {
    let img = crate::oci::pull(reference)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "rootfs": img.rootfs,
                "digest": img.digest,
                "entrypoint": img.config.entrypoint,
                "cmd": img.config.cmd,
                "env": img.config.env,
                "workdir": img.config.workdir,
            })
        );
    } else {
        println!("{}", img.rootfs.display());
    }
    Ok(())
}

pub(crate) fn cmd_images(json: bool) -> Result<()> {
    let images = api::list_images()?;
    if json {
        println!("{}", serde_json::to_string(&images)?);
        return Ok(());
    }
    println!(
        "{:<14}  {:<32}  {:<10}  {}",
        "ID", "REFERENCE", "SIZE", "CREATED"
    );
    for im in images {
        println!(
            "{:<14}  {:<32}  {:<10}  {}",
            im.id,
            truncate(&im.reference, 32),
            oci::human_size(im.size.max(0) as u64),
            im.created_at
                .as_deref()
                .map_or_else(|| "-".to_string(), db::age)
        );
    }
    Ok(())
}
