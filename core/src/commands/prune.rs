//! `bsdkrun prune` — reclaim disk from things nothing is using.
//!
//! Shaped after `docker system prune`, including the part that matters most:
//! it says exactly what it is about to delete, and how much that is worth,
//! before deleting anything. A reclaim command that just prints a number
//! afterwards gives you no moment to notice it was about to take the wrong VM.
//!
//! What is safe to remove is decided by the same checks the individual `rm`
//! commands use — [`super::images::image_users`], the running-machine test in
//! [`super::volumes::remove_volume`] — rather than a second opinion that could
//! disagree with them.
//!
//! ## What is kept, and why
//!
//! The **OCI layer cache** survives a default prune. It is pure cache, so it
//! looks like the obvious thing to drop — but it is exactly what makes
//! re-pulling the images this command just deleted cheap, it is already
//! bounded and LRU-evicted, and dropping it turns a 7-second re-pull back into
//! a 27-second one. `--all` includes it for when the disk really is gone.
//!
//! **Volumes** need `--volumes`, as in Docker: an image is re-downloadable and
//! a stopped machine is re-creatable, but a volume is the one thing here
//! holding data that came from somewhere else — an agent's login, a database.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::db;
use crate::oci::human_size;

/// One thing that can be removed, and what it is worth.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// What it is: `machine`, `image`, `volume`, `cache`, `layers`, `orphan`.
    pub kind: &'static str,
    /// The id or name to remove it by.
    pub id: String,
    /// What to show — a machine's name, an image's reference.
    pub label: String,
    pub bytes: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Plan {
    pub items: Vec<Item>,
}

impl Plan {
    pub fn total(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Item> + 'a {
        self.items.iter().filter(move |i| i.kind == kind)
    }
}

/// Whether a kind is in scope.
///
/// An empty `--only` means everything the other flags allow; naming kinds
/// narrows it to exactly those. Singular names, matching `Item::kind`, so the
/// selector and the report cannot drift apart.
pub const KINDS: &[&str] = &["machine", "image", "orphan", "volume", "cache", "layers"];

fn wanted(only: &[String], kind: &str) -> bool {
    only.is_empty() || only.iter().any(|k| k == kind)
}

/// What `prune` would remove, without removing it.
///
/// Every candidate is sized here rather than at deletion time, because the
/// summary has to be truthful *before* the confirmation — a number produced
/// afterwards is not something anyone can decline.
pub fn plan(all: bool, volumes: bool, only: &[String]) -> Result<Plan> {
    let db = db::Db::open()?;
    let machines = db.list_machines()?;
    let mut plan = Plan::default();

    // Stopped machines. "Stopped" means the row says so *and* the pid is gone:
    // a crashed supervisor leaves a row claiming to run, and that machine is
    // not something to delete out from under someone.
    for m in machines.iter().filter(|_| wanted(only, "machine")) {
        let running = m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false);
        if running {
            continue;
        }
        let dir = PathBuf::from(&m.state_dir);
        plan.items.push(Item {
            kind: "machine",
            id: m.id.clone(),
            label: m.name.clone().unwrap_or_else(|| m.id.clone()),
            bytes: crate::host::dir_size(&dir),
        });
    }

    // Images no machine references. The same rule `image rm` enforces, so a
    // prune can never remove an image a machine still needs to boot.
    for image in db
        .list_images()?
        .into_iter()
        .filter(|_| wanted(only, "image"))
    {
        if !super::images::image_users(&image)?.is_empty() {
            continue;
        }
        // An image whose only users are machines this prune is about to remove
        // is still unused afterwards — but not before, so it is left for the
        // next run rather than deleted out of order.
        let rootfs = PathBuf::from(&image.rootfs);
        let bytes = if rootfs.exists() {
            crate::host::dir_size(&rootfs)
        } else {
            0
        };
        plan.items.push(Item {
            kind: "image",
            id: image.id.clone(),
            label: image.reference.clone(),
            bytes,
        });
    }

    // Extracted rootfs trees the database has forgotten, and the `.staging`
    // directories an interrupted pull leaves behind. Pure garbage: nothing can
    // reach them, and nothing will.
    let image_dirs: std::collections::HashSet<String> = db
        .list_images()?
        .iter()
        .filter_map(|i| i.rootfs.strip_suffix("/rootfs").map(str::to_string))
        .collect();
    if wanted(only, "orphan") {
        if let Ok(cache) = crate::fetch::oci_cache_dir() {
            if let Ok(entries) = std::fs::read_dir(&cache) {
                for e in entries.flatten() {
                    let path = e.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let name = e.file_name().to_string_lossy().into_owned();
                    let dir = path.display().to_string();
                    let Some(why) = orphan_reason(&name, &dir, &image_dirs) else {
                        continue;
                    };
                    plan.items.push(Item {
                        kind: "orphan",
                        id: dir,
                        label: format!("{name} ({why})"),
                        bytes: crate::host::dir_size(&path),
                    });
                }
            }
        }
    }

    // Volumes attached to no machine at all — not merely to no *running* one,
    // which is the weaker test `volume rm` applies. A volume a stopped machine
    // still points at is that machine's data.
    // Naming a kind with `--only` is the same permission `--volumes` grants,
    // so an explicit `--only volumes` does not also need the flag.
    if (volumes || only.iter().any(|k| k == "volume")) && wanted(only, "volume") {
        for v in db.list_volumes()? {
            if machines
                .iter()
                .any(|m| m.volume.as_deref() == Some(&v.name))
            {
                continue;
            }
            let dir = PathBuf::from(&v.path);
            plan.items.push(Item {
                kind: "volume",
                id: v.name.clone(),
                label: v.name.clone(),
                bytes: crate::host::dir_size(&dir),
            });
        }
    }

    if all || only.iter().any(|k| k == "cache" || k == "layers") {
        // Saved `bsdkrun cache` entries. Behind `--all` because a user put
        // them there deliberately, under a key they chose.
        if wanted(only, "cache") {
            if let Ok(store) = crate::cache::Store::open() {
                for entry in store.list().unwrap_or_default() {
                    plan.items.push(Item {
                        kind: "cache",
                        id: entry.key.clone(),
                        label: entry.key.clone(),
                        bytes: entry.size,
                    });
                }
            }
        }

        // The OCI layer cache — see the module docs for why this is not in the
        // default set.
        if wanted(only, "layers") {
            if let Ok(blobs) = crate::fetch::oci_cache_dir().map(|d| d.join("blobs")) {
                let bytes = crate::host::dir_size(&blobs);
                if bytes > 0 {
                    plan.items.push(Item {
                        kind: "layers",
                        id: blobs.display().to_string(),
                        label: "OCI layer cache".to_string(),
                        bytes,
                    });
                }
            }
        }
    }

    Ok(plan)
}

/// Why a directory in the OCI cache is garbage, or `None` if it is live.
///
/// Its own function because it decides what gets deleted, and a rule that can
/// only be exercised against a real cache directory is a rule nobody checks.
fn orphan_reason(
    name: &str,
    dir: &str,
    image_dirs: &std::collections::HashSet<String>,
) -> Option<&'static str> {
    // The layer cache is not an extracted image and is never garbage here;
    // `--all` removes it through its own entry.
    if name == "blobs" {
        return None;
    }
    // A pull that died part-way. It is named `.staging` precisely so it can
    // never be mistaken for a complete tree, so no image can reference it.
    if name.ends_with(".staging") {
        return Some("interrupted pull");
    }
    // Otherwise: live if any image row points at it.
    (!image_dirs.contains(dir)).then_some("no image row")
}

/// Print the summary that the confirmation is asking about.
fn summarize(plan: &Plan) {
    let group = |kind: &'static str, noun: &str| {
        let items: Vec<&Item> = plan.of_kind(kind).collect();
        if items.is_empty() {
            return;
        }
        let bytes: u64 = items.iter().map(|i| i.bytes).sum();
        println!("\n{} {noun} ({}):", items.len(), human_size(bytes));
        for i in items.iter().take(20) {
            println!(
                "  {:<28}  {}",
                super::truncate(&i.label, 28),
                human_size(i.bytes)
            );
        }
        if items.len() > 20 {
            println!("  … and {} more", items.len() - 20);
        }
    };

    group("machine", "stopped machine(s)");
    group("image", "unused image(s)");
    group("orphan", "orphaned directory(ies)");
    group("volume", "unused volume(s)");
    group("cache", "cache entry(ies)");
    group("layers", "layer cache");
}

/// Ask before deleting, exactly as `docker system prune` does.
///
/// Defaults to no on an empty answer, and refuses rather than assumes when
/// there is no terminal to ask at — a prune in a script that did not pass
/// `--force` was not an instruction to delete anything.
fn confirm() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("refusing to prune without a terminal to confirm at — pass --force");
    }
    print!("\nAre you sure you want to continue? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading the confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// `bsdkrun prune`.
pub(crate) fn cmd_prune(
    all: bool,
    volumes: bool,
    only: &[String],
    force: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    // Checked before anything is scanned: a typo'd kind would otherwise
    // silently select nothing and read as "there is nothing to prune".
    for kind in only {
        if !KINDS.contains(&kind.as_str()) {
            anyhow::bail!(
                "unknown --only kind {kind:?} (one of: {})",
                KINDS.join(", ")
            );
        }
    }
    let plan = plan(all, volumes, only)?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "items": plan.items,
                "total": plan.total(),
                "removed": !dry_run && force,
            }))?
        );
        if dry_run || !force {
            return Ok(());
        }
        return remove(&plan, true);
    }

    if plan.is_empty() {
        println!("Nothing to prune.");
        return Ok(());
    }

    println!("This will remove:");
    summarize(&plan);
    println!("\nTotal reclaimable space: {}", human_size(plan.total()));
    if !only.is_empty() {
        println!("(--only {}: nothing else is considered)", only.join(", "));
    }
    if !all && only.is_empty() {
        println!(
            "(the OCI layer cache and saved cache entries are kept — add --all to include them)"
        );
    }
    if !volumes && only.is_empty() {
        println!("(volumes are kept — add --volumes to include unused ones)");
    }

    if dry_run {
        println!("\n--dry-run: nothing was removed.");
        return Ok(());
    }
    if !force && !confirm()? {
        println!("Cancelled.");
        return Ok(());
    }
    remove(&plan, false)
}

/// Do the deletions, reporting what actually went.
///
/// A failure on one item is reported and skipped rather than aborting: a
/// volume held by a machine that started while the prompt was open should not
/// strand the rest of the reclaim.
fn remove(plan: &Plan, quiet: bool) -> Result<()> {
    let mut freed = 0u64;
    let mut failed = 0usize;

    for item in &plan.items {
        let outcome = match item.kind {
            "machine" => super::machines::remove_machine(&item.id, false).map(|_| ()),
            "image" => super::images::remove_image(&item.id, false).map(|_| ()),
            "volume" => super::volumes::remove_volume(&item.id, false).map(|_| ()),
            "cache" => crate::cache::Store::open()
                .and_then(|s| s.remove(&item.id))
                .map(|_| ()),
            "orphan" | "layers" => {
                crate::host::force_remove_dir_all(std::path::Path::new(&item.id));
                Ok(())
            }
            other => Err(anyhow::anyhow!("unknown prune kind {other}")),
        };
        match outcome {
            Ok(()) => freed += item.bytes,
            Err(e) => {
                failed += 1;
                if !quiet {
                    eprintln!("  skipped {}: {e:#}", item.label);
                }
            }
        }
    }

    if !quiet {
        println!("\nTotal reclaimed space: {}", human_size(freed));
        if failed > 0 {
            println!("{failed} item(s) could not be removed and were left alone.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphans_are_only_the_trees_nothing_points_at() {
        let mut live = std::collections::HashSet::new();
        live.insert("/cache/sha256-live".to_string());

        // Referenced by an image row: keep, whatever else is true.
        assert_eq!(
            orphan_reason("sha256-live", "/cache/sha256-live", &live),
            None
        );
        // Not referenced: garbage.
        assert_eq!(
            orphan_reason("sha256-gone", "/cache/sha256-gone", &live),
            Some("no image row")
        );
        // A half-finished pull, even if its prefix matches a live tree.
        assert_eq!(
            orphan_reason("sha256-live.staging", "/cache/sha256-live.staging", &live),
            Some("interrupted pull")
        );
        // The layer cache is never swept up as an orphan.
        assert_eq!(orphan_reason("blobs", "/cache/blobs", &live), None);
    }

    #[test]
    fn a_plan_totals_its_items() {
        let plan = Plan {
            items: vec![
                Item {
                    kind: "image",
                    id: "a".into(),
                    label: "a".into(),
                    bytes: 100,
                },
                Item {
                    kind: "machine",
                    id: "b".into(),
                    label: "b".into(),
                    bytes: 50,
                },
            ],
        };
        assert_eq!(plan.total(), 150);
        assert_eq!(plan.of_kind("image").count(), 1);
        assert!(!plan.is_empty());
        assert!(Plan::default().is_empty());
    }
}
