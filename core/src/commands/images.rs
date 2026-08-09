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

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
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
