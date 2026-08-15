//! `bsdkrun cache` — the CLI over [`crate::cache`].

use anyhow::{bail, Context, Result};

use crate::cache::{self, archive::Compression, Entry, Store};

use super::guest::{agent_error, agent_target};
use super::truncate;

/// What `--json` prints for a save or a restore.
///
/// A restore's headline fact is whether it *hit*, and on a fallback which key
/// it actually landed on — neither of which a caller should have to read out of
/// a human sentence on stderr. The SDKs are built on this.
#[derive(serde::Serialize)]
struct Outcome<'a> {
    /// Restore only: whether anything was found. Omitted for a save, where it
    /// would be a field with no meaning.
    #[serde(skip_serializing_if = "Option::is_none")]
    restored: Option<bool>,
    /// The key asked for.
    requested_key: &'a str,
    /// The entry actually used, when there was one. Differs from
    /// `requested_key` when a `--restore-keys` prefix matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compression: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<u64>,
}

/// Save the guest directory at `path` under `key`.
pub(crate) fn cmd_save(
    id: &str,
    path: &str,
    key: &str,
    compression: Compression,
    force: bool,
    json: bool,
) -> Result<()> {
    if key.trim().is_empty() {
        bail!("a cache entry needs a --key");
    }
    let store = Store::open()?;
    if !force {
        if let Some(existing) = cache::resolve_key(&store, key, &[])? {
            bail!(
                "{key:?} is already cached ({} from {}, saved {}). Pass --force to replace it, \
                 or use a key that names this exact content — a lockfile hash, say.",
                human_size(existing.size),
                existing.path,
                ago(existing.created)
            );
        }
    }

    let (vm, port) = agent_target(id)?;
    if !json {
        eprintln!("saving {}:{path} as {key:?} ({compression})", vm.id);
    }

    let (tmp, size, digest) = cache::write_archive(compression, |sink| {
        super::cp::stream_dir_out(&vm.id, port, path, sink).map_err(|e| agent_error(&vm.kind, e))
    })?;

    let entry = Entry {
        key: key.to_string(),
        path: path.to_string(),
        compression,
        size,
        created: cache::now(),
        digest,
    };
    store.put_entry(&entry, tmp.path())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Outcome {
                restored: None,
                requested_key: key,
                key: Some(key),
                path: Some(path),
                size: Some(size),
                compression: Some(compression.to_string()),
                created: Some(entry.created),
            })?
        );
    } else {
        eprintln!(
            "cached {key:?} — {} to {}",
            human_size(size),
            store.describe()
        );
    }
    Ok(())
}

/// Restore a cached tree into a machine.
pub(crate) fn cmd_restore(
    id: &str,
    path: Option<&str>,
    key: &str,
    restore_keys: &[String],
    json: bool,
) -> Result<()> {
    let store = Store::open()?;
    let Some(entry) = cache::resolve_key(&store, key, restore_keys)? else {
        // A cache miss is the normal case on a first run, not a failure — the
        // caller builds from scratch and saves afterwards. Exit 0 and say so,
        // the way actions/cache does, so `cache restore || true` is unnecessary.
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&Outcome {
                    restored: Some(false),
                    requested_key: key,
                    key: None,
                    path: None,
                    size: None,
                    compression: None,
                    created: None,
                })?
            );
        } else {
            eprintln!(
                "cache miss for {key:?}{} — nothing restored",
                if restore_keys.is_empty() {
                    String::new()
                } else {
                    format!(" (and {} restore-key prefix(es))", restore_keys.len())
                }
            );
        }
        return Ok(());
    };
    if entry.key != key && !json {
        eprintln!("cache miss for {key:?}; restoring {:?} instead", entry.key);
    }

    let target = path.unwrap_or(&entry.path);
    let (vm, port) = agent_target(id)?;

    let tmp = tempfile::NamedTempFile::new().context("creating a temporary archive")?;
    if !store.fetch_entry(&entry, tmp.path())? {
        bail!(
            "the metadata for {:?} is in {} but its archive is gone — remove it with \
             `bsdkrun cache rm {}`",
            entry.key,
            store.describe(),
            entry.key
        );
    }
    cache::verify(tmp.path(), &entry.digest)?;

    let stream = cache::open_archive(tmp.path(), entry.compression)?;
    super::cp::stream_dir_in(&vm.id, port, target, stream).map_err(|e| agent_error(&vm.kind, e))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Outcome {
                restored: Some(true),
                requested_key: key,
                key: Some(&entry.key),
                path: Some(target),
                size: Some(entry.size),
                compression: Some(entry.compression.to_string()),
                created: Some(entry.created),
            })?
        );
    } else {
        eprintln!(
            "restored {:?} ({}, saved {}) into {}:{target}",
            entry.key,
            human_size(entry.size),
            ago(entry.created),
            vm.id
        );
    }
    Ok(())
}

pub(crate) fn cmd_ls(json: bool) -> Result<()> {
    let store = Store::open()?;
    let entries = store.list()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("no cache entries in {}", store.describe());
        return Ok(());
    }
    println!(
        "{:<32} {:<24} {:>10} {:<9} SAVED",
        "KEY", "PATH", "SIZE", "FORMAT"
    );
    for e in entries {
        println!(
            "{:<32} {:<24} {:>10} {:<9} {}",
            truncate(&e.key, 32),
            truncate(&e.path, 24),
            human_size(e.size),
            e.compression,
            ago(e.created)
        );
    }
    Ok(())
}

pub(crate) fn cmd_rm(keys: &[String], all: bool) -> Result<()> {
    let store = Store::open()?;
    let owned: Vec<String>;
    let keys = if all {
        owned = store.list()?.into_iter().map(|e| e.key).collect();
        if owned.is_empty() {
            println!("no cache entries in {}", store.describe());
            return Ok(());
        }
        &owned
    } else {
        keys
    };
    for key in keys {
        if store.remove(key)? {
            println!("{key}");
        } else {
            eprintln!("no cache entry for {key:?}");
        }
    }
    Ok(())
}

fn human_size(bytes: u64) -> String {
    crate::oci::human_size(bytes)
}

/// A rough "3 hours ago", matching how `ps` renders ages.
fn ago(unix: u64) -> String {
    let now = cache::now();
    let secs = now.saturating_sub(unix);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} minutes ago", secs / 60),
        3600..=86_399 => format!("{} hours ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_as_prose() {
        let now = cache::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 120), "2 minutes ago");
        assert_eq!(ago(now - 7200), "2 hours ago");
        assert_eq!(ago(now - 3 * 86_400), "3 days ago");
    }
}
