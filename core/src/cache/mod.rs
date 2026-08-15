//! `bsdkrun cache` — save a guest directory to a backing store under a key, and
//! restore it into any machine later.
//!
//! The shape is GitHub Actions': you save under an exact `--key`, and restore
//! by that key with `--restore-keys` prefixes as fallbacks, so a lockfile-hashed
//! key that misses still lands on the most recent compatible cache instead of
//! nothing.
//!
//! The archive is produced the same way `bsdkrun cp -r` moves a directory —
//! `tar` in the guest, streamed out over the exec agent — and compressed on the
//! host, so an image needs no compressor of its own. See [`archive`] for the
//! formats and [`config`] for where entries are stored.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub mod archive;
pub mod config;
pub mod s3;

use archive::Compression;
use config::Backend;

/// What a saved cache entry records about itself. Written beside the archive so
/// a restore knows how to unwrap it, and `ls` can describe it without reading
/// gigabytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// The exact key it was saved under.
    pub key: String,
    /// Guest path the tree came from — informational, and the default target
    /// when restoring without one.
    pub path: String,
    pub compression: Compression,
    /// Archive size in bytes.
    pub size: u64,
    /// Unix seconds when it was saved.
    pub created: u64,
    /// `sha256:…` over the archive, so a restore can tell a truncated download
    /// from a corrupt cache.
    pub digest: String,
}

impl Entry {
    /// Storage name shared by the archive and its metadata.
    ///
    /// A readable slug plus a hash of the full key: the slug keeps a store
    /// browsable, and the hash makes collisions impossible even though the slug
    /// throws away characters and length.
    pub fn storage_name(key: &str) -> String {
        let slug: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let slug: String = slug.trim_matches('-').chars().take(40).collect();
        let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
        if slug.is_empty() {
            hash[..16].to_string()
        } else {
            format!("{slug}-{}", &hash[..8])
        }
    }

    fn archive_name(&self) -> String {
        format!(
            "{}.{}",
            Entry::storage_name(&self.key),
            self.compression.extension()
        )
    }

    fn meta_name(&self) -> String {
        format!("{}.json", Entry::storage_name(&self.key))
    }
}

/// A place cache entries live. Disk and S3 differ only in how bytes move.
pub enum Store {
    Disk(PathBuf),
    S3(config::S3Config),
}

impl Store {
    /// Open the store the environment and config select.
    pub fn open() -> Result<Store> {
        match config::resolve()? {
            Backend::Disk => Ok(Store::Disk(disk_dir()?)),
            Backend::S3(cfg) => Ok(Store::S3(cfg)),
        }
    }

    /// One line naming where entries go, for `cache ls` and `doctor`.
    pub fn describe(&self) -> String {
        match self {
            Store::Disk(dir) => format!("host disk at {}", dir.display()),
            Store::S3(cfg) => format!("s3://{}/{}", cfg.bucket, cfg.prefix.trim_matches('/')),
        }
    }

    /// Publish an archive and its metadata. Metadata goes **last**, so a store
    /// never advertises an entry whose archive did not finish uploading — the
    /// same reason `oci.rs` writes its completion marker last.
    pub fn put_entry(&self, entry: &Entry, archive: &Path) -> Result<()> {
        let meta = serde_json::to_vec_pretty(entry)?;
        match self {
            Store::Disk(dir) => {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
                let dest = dir.join(entry.archive_name());
                std::fs::copy(archive, &dest)
                    .with_context(|| format!("writing {}", dest.display()))?;
                std::fs::write(dir.join(entry.meta_name()), meta)?;
            }
            Store::S3(cfg) => {
                s3::put_file(cfg, &entry.archive_name(), archive)?;
                s3::put_bytes(cfg, &entry.meta_name(), &meta)?;
            }
        }
        Ok(())
    }

    /// Fetch an entry's archive into `dest`. `Ok(false)` means it is not there.
    pub fn fetch_entry(&self, entry: &Entry, dest: &Path) -> Result<bool> {
        match self {
            Store::Disk(dir) => {
                let src = dir.join(entry.archive_name());
                if !src.exists() {
                    return Ok(false);
                }
                std::fs::copy(&src, dest).with_context(|| format!("reading {}", src.display()))?;
                Ok(true)
            }
            Store::S3(cfg) => s3::get_file(cfg, &entry.archive_name(), dest),
        }
    }

    /// Every entry in the store, newest first.
    pub fn list(&self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        match self {
            Store::Disk(dir) => {
                let Ok(rd) = std::fs::read_dir(dir) else {
                    return Ok(out);
                };
                for e in rd.filter_map(|e| e.ok()) {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue;
                    }
                    if let Ok(bytes) = std::fs::read(&p) {
                        if let Ok(entry) = serde_json::from_slice::<Entry>(&bytes) {
                            out.push(entry);
                        }
                    }
                }
            }
            Store::S3(cfg) => {
                for name in s3::list(cfg, "")? {
                    if !name.ends_with(".json") {
                        continue;
                    }
                    if let Some(bytes) = s3::get_bytes(cfg, &name)? {
                        if let Ok(entry) = serde_json::from_slice::<Entry>(&bytes) {
                            out.push(entry);
                        }
                    }
                }
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.created));
        Ok(out)
    }

    /// Metadata for one exact key.
    fn get(&self, key: &str) -> Result<Option<Entry>> {
        let name = format!("{}.json", Entry::storage_name(key));
        let bytes = match self {
            Store::Disk(dir) => std::fs::read(dir.join(&name)).ok(),
            Store::S3(cfg) => s3::get_bytes(cfg, &name)?,
        };
        Ok(bytes.and_then(|b| serde_json::from_slice(&b).ok()))
    }

    /// Remove an entry. Returns whether there was one.
    pub fn remove(&self, key: &str) -> Result<bool> {
        let Some(entry) = self.get(key)? else {
            return Ok(false);
        };
        match self {
            Store::Disk(dir) => {
                let _ = std::fs::remove_file(dir.join(entry.archive_name()));
                let _ = std::fs::remove_file(dir.join(entry.meta_name()));
            }
            Store::S3(cfg) => {
                s3::delete(cfg, &entry.archive_name())?;
                s3::delete(cfg, &entry.meta_name())?;
            }
        }
        Ok(true)
    }
}

/// Resolve `key`, then each `restore_keys` prefix in turn.
///
/// Exact match wins. Otherwise the prefixes are tried **in the order given** —
/// they are a preference list, not a set — and within one prefix the newest
/// matching entry wins. That is GitHub Actions' rule, and the reason a
/// lockfile-hashed key can fall back to "any cache for this OS" instead of
/// rebuilding from nothing.
pub fn resolve_key(store: &Store, key: &str, restore_keys: &[String]) -> Result<Option<Entry>> {
    if let Some(hit) = store.get(key)? {
        return Ok(Some(hit));
    }
    if restore_keys.is_empty() {
        return Ok(None);
    }
    let all = store.list()?; // already newest-first
    for prefix in restore_keys {
        if let Some(hit) = all.iter().find(|e| e.key.starts_with(prefix.as_str())) {
            return Ok(Some(hit.clone()));
        }
    }
    Ok(None)
}

/// Default disk store: `<cache>/caches`, beside the image cache.
fn disk_dir() -> Result<PathBuf> {
    Ok(crate::fetch::cache_dir()?.join("caches"))
}

/// Compress a tar stream from `tar_source` into a new archive file.
///
/// Returns the temp file holding it, its size and its digest — the caller
/// uploads it and then throws it away.
pub fn write_archive<F>(
    compression: Compression,
    tar_source: F,
) -> Result<(tempfile::NamedTempFile, u64, String)>
where
    F: FnOnce(&mut dyn Write) -> Result<()>,
{
    let tmp = tempfile::NamedTempFile::new().context("creating a temporary archive")?;
    let file = tmp.reopen()?;
    let mut sink = archive::Sink::new(std::io::BufWriter::new(file), compression)?;
    tar_source(&mut sink)?;
    sink.finish()?.flush()?;

    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(tmp.path())?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((tmp, size, format!("sha256:{:x}", hasher.finalize())))
}

/// Decompress an archive into a plain tar, ready to stream into the guest.
///
/// eStargz's TOC and landmark are stripped here rather than in the guest: they
/// are real tar members, so a guest `tar -xf` would otherwise drop a
/// `stargz.index.json` into the restored directory.
pub fn open_archive(path: &Path, compression: Compression) -> Result<Box<dyn Read + Send>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("opening the cache archive {}", path.display()))?;
    if compression == Compression::Estargz {
        let mut stripped = tempfile::tempfile()?;
        archive::strip_estargz(archive::reader(file, compression)?, &mut stripped)?;
        use std::io::Seek;
        stripped.seek(std::io::SeekFrom::Start(0))?;
        return Ok(Box::new(stripped));
    }
    archive::reader(file, compression)
}

/// Verify a fetched archive against the digest its metadata recorded.
pub fn verify(path: &Path, expected: &str) -> Result<()> {
    if expected.is_empty() {
        return Ok(());
    }
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = format!("sha256:{:x}", hasher.finalize());
    if got != expected {
        bail!("the cache archive is corrupt: expected {expected}, got {got}");
    }
    Ok(())
}

/// Unix seconds, for an entry's `created`.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, created: u64) -> Entry {
        Entry {
            key: key.to_string(),
            path: "/root/.cargo".into(),
            compression: Compression::Gzip,
            size: 1,
            created,
            digest: String::new(),
        }
    }

    fn disk_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::Disk(dir.path().to_path_buf());
        (dir, store)
    }

    /// A key is arbitrary user text — a lockfile hash, a branch name, a path.
    /// The name it maps to has to be a safe filename *and* an S3 key, and two
    /// different keys must never collide onto one.
    #[test]
    fn storage_names_are_safe_and_collision_free() {
        let name = Entry::storage_name("deps/linux amd64:v2");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'),
            "{name}"
        );
        assert_ne!(
            Entry::storage_name("deps/linux"),
            Entry::storage_name("deps-linux"),
            "keys that slugify the same must still differ"
        );
        // Long keys stay bounded rather than producing an unusable filename.
        assert!(Entry::storage_name(&"x".repeat(500)).len() <= 49);
    }

    #[test]
    fn an_empty_or_unslugifiable_key_still_gets_a_name() {
        assert!(!Entry::storage_name("").is_empty());
        assert!(!Entry::storage_name("///").is_empty());
    }

    #[test]
    fn saving_then_listing_round_trips_the_metadata() {
        let (_dir, store) = disk_store();
        let archive = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(archive.path(), b"not really a tar").unwrap();

        let e = entry("deps-v1", 100);
        store.put_entry(&e, archive.path()).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "deps-v1");
        assert_eq!(listed[0].path, "/root/.cargo");

        let dest = tempfile::NamedTempFile::new().unwrap();
        assert!(store.fetch_entry(&e, dest.path()).unwrap());
        assert_eq!(std::fs::read(dest.path()).unwrap(), b"not really a tar");
    }

    #[test]
    fn removing_takes_both_the_archive_and_its_metadata() {
        let (dir, store) = disk_store();
        let archive = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(archive.path(), b"x").unwrap();
        store.put_entry(&entry("k", 1), archive.path()).unwrap();

        assert!(store.remove("k").unwrap());
        assert!(
            !store.remove("k").unwrap(),
            "removing twice is not an error"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// The fallback rule is the whole reason keys are worth having: an exact
    /// miss should still find the newest compatible entry.
    #[test]
    fn restore_keys_fall_back_in_order_and_prefer_the_newest() {
        let (_dir, store) = disk_store();
        let archive = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(archive.path(), b"x").unwrap();
        for (key, created) in [
            ("deps-linux-aaa", 100),
            ("deps-linux-bbb", 300),
            ("deps-macos-ccc", 200),
        ] {
            store
                .put_entry(&entry(key, created), archive.path())
                .unwrap();
        }

        // Exact hit wins outright.
        let hit = resolve_key(&store, "deps-linux-aaa", &["deps-".into()])
            .unwrap()
            .unwrap();
        assert_eq!(hit.key, "deps-linux-aaa");

        // A miss falls back to the newest entry under the prefix.
        let hit = resolve_key(&store, "deps-linux-zzz", &["deps-linux-".into()])
            .unwrap()
            .unwrap();
        assert_eq!(hit.key, "deps-linux-bbb");

        // Prefixes are a preference list, tried in order.
        let hit = resolve_key(
            &store,
            "nope",
            &["deps-macos-".into(), "deps-linux-".into()],
        )
        .unwrap()
        .unwrap();
        assert_eq!(hit.key, "deps-macos-ccc");

        // No prefix matches, and no restore keys at all.
        assert!(resolve_key(&store, "nope", &["other-".into()])
            .unwrap()
            .is_none());
        assert!(resolve_key(&store, "nope", &[]).unwrap().is_none());
    }

    #[test]
    fn a_corrupt_archive_is_caught_by_its_digest() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"hello").unwrap();
        let good = format!("sha256:{:x}", Sha256::digest(b"hello"));
        verify(f.path(), &good).unwrap();
        verify(f.path(), "").unwrap(); // no digest recorded: nothing to check

        let err = verify(f.path(), "sha256:deadbeef").unwrap_err().to_string();
        assert!(err.contains("corrupt"), "{err}");
    }

    /// Metadata is written after the archive so an interrupted save leaves an
    /// invisible entry rather than one that resolves to a partial file.
    #[test]
    fn an_archive_without_metadata_is_not_listed() {
        let (dir, store) = disk_store();
        std::fs::write(dir.path().join("orphan-12345678.tar.gz"), b"x").unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
