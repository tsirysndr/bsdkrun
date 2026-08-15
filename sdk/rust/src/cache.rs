//! Cached guest directories — [`Sandbox::cache`](crate::Sandbox::cache), plus
//! host-level listing.
//!
//! Entries are keyed, so a rebuild can pick up where the last one left off.
//! Where they live — host disk or S3 — is host configuration, not an SDK
//! concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or write
//! `~/.config/bsdkrun/cache.toml`.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::process::run;

/// An archive format a cache entry can be stored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    #[default]
    Gzip,
    Zstd,
    Estargz,
    None,
}

impl Compression {
    fn as_str(self) -> &'static str {
        match self {
            Compression::Gzip => "gzip",
            Compression::Zstd => "zstd",
            Compression::Estargz => "estargz",
            Compression::None => "none",
        }
    }
}

/// A stored cache entry, as `cache ls` reports it.
#[derive(Debug, Clone, Default)]
pub struct CacheEntry {
    /// The exact key it was saved under.
    pub key: String,
    /// Guest path the tree came from.
    pub path: String,
    pub compression: String,
    /// Archive size in bytes.
    pub size: u64,
    /// Unix seconds when it was saved.
    pub created: u64,
    /// `sha256:…` over the archive.
    pub digest: String,
}

impl CacheEntry {
    fn from_value(v: &Value) -> CacheEntry {
        CacheEntry {
            key: str_at(v, "key"),
            path: str_at(v, "path"),
            compression: str_at(v, "compression"),
            size: num_at(v, "size").unwrap_or(0),
            created: num_at(v, "created").unwrap_or(0),
            digest: str_at(v, "digest"),
        }
    }
}

/// What a restore did. A miss is not an error — check [`restored`](Self::restored).
#[derive(Debug, Clone, Default)]
pub struct RestoreResult {
    pub restored: bool,
    /// The key asked for.
    pub requested_key: String,
    /// The entry actually used. Differs from [`requested_key`](Self::requested_key)
    /// when a restore-key prefix matched, and is `None` on a miss.
    pub key: Option<String>,
    /// Guest path it was restored into.
    pub path: Option<String>,
    pub size: Option<u64>,
    pub compression: Option<String>,
    pub created: Option<u64>,
}

impl RestoreResult {
    fn from_value(v: &Value) -> RestoreResult {
        RestoreResult {
            restored: v.get("restored").and_then(Value::as_bool).unwrap_or(false),
            requested_key: str_at(v, "requested_key"),
            key: opt_str_at(v, "key"),
            path: opt_str_at(v, "path"),
            size: num_at(v, "size"),
            compression: opt_str_at(v, "compression"),
            created: num_at(v, "created"),
        }
    }
}

// Lenient accessors, matching `types.rs`: the SDK reads the CLI's JSON through
// `serde_json::Value` rather than deriving, so a field the CLI adds later never
// turns into a decode error.
fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn num_at(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// Save and restore guest directories under a key.
///
/// ```no_run
/// # use bsdkrun_sdk::{Sandbox, cache::Compression};
/// # fn main() -> bsdkrun_sdk::Result<()> {
/// let sbx = Sandbox::get("web")?;
/// let hit = sbx.cache().restore("deps-abc123", None, &["deps-".to_string()])?;
/// if !hit.restored {
///     sbx.exec(["npm", "ci"])?;
///     sbx.cache().save("/app/node_modules", "deps-abc123", Compression::Zstd, false)?;
/// }
/// # Ok(())
/// # }
/// ```
pub struct Cache {
    id: String,
}

impl Cache {
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Cache { id: id.into() }
    }

    /// Archive the guest directory at `path` under `key`.
    pub fn save(
        &self,
        path: &str,
        key: &str,
        compression: Compression,
        force: bool,
    ) -> Result<CacheEntry> {
        let mut args = vec![
            "cache".to_string(),
            "save".to_string(),
            format!("{}:{}", self.id, path),
            "--key".to_string(),
            key.to_string(),
            "--json".to_string(),
        ];
        if compression != Compression::Gzip {
            args.push("--compression".to_string());
            args.push(compression.as_str().to_string());
        }
        if force {
            args.push("--force".to_string());
        }
        Ok(CacheEntry::from_value(&json(&args, "bsdkrun cache save")?))
    }

    /// Restore a stored tree.
    ///
    /// `path` defaults to the directory the entry was saved from.
    /// `restore_keys` are prefixes tried in order when `key` misses; within a
    /// prefix the newest matching entry wins.
    pub fn restore(
        &self,
        key: &str,
        path: Option<&str>,
        restore_keys: &[String],
    ) -> Result<RestoreResult> {
        let target = match path {
            Some(p) => format!("{}:{}", self.id, p),
            None => self.id.clone(),
        };
        let mut args = vec![
            "cache".to_string(),
            "restore".to_string(),
            target,
            "--key".to_string(),
            key.to_string(),
            "--json".to_string(),
        ];
        if !restore_keys.is_empty() {
            args.push("--restore-keys".to_string());
            args.extend(restore_keys.iter().cloned());
        }
        Ok(RestoreResult::from_value(&json(
            &args,
            "bsdkrun cache restore",
        )?))
    }
}

/// Every stored cache entry, newest first.
pub fn list() -> Result<Vec<CacheEntry>> {
    let v = json(
        &["cache".to_string(), "ls".to_string(), "--json".to_string()],
        "bsdkrun cache ls",
    )?;
    Ok(v.as_array()
        .map(|rows| rows.iter().map(CacheEntry::from_value).collect())
        .unwrap_or_default())
}

/// Remove entries by key, or every one of them with `all`.
pub fn remove(keys: &[String], all: bool) -> Result<()> {
    let mut args = vec!["cache".to_string(), "rm".to_string()];
    if all {
        args.push("--all".to_string());
    } else {
        args.extend(keys.iter().cloned());
    }
    let res = run(args)?;
    if res.exit_code != 0 {
        return Err(Error::CommandFailed {
            exit_code: res.exit_code,
            stdout: res.stdout,
            stderr: res.stderr,
            command: "bsdkrun cache rm".to_string(),
        });
    }
    Ok(())
}

fn json(args: &[String], label: &str) -> Result<Value> {
    let res = run(args.to_vec())?;
    if res.exit_code != 0 {
        return Err(Error::CommandFailed {
            exit_code: res.exit_code,
            stdout: res.stdout,
            stderr: res.stderr,
            command: label.to_string(),
        });
    }
    serde_json::from_str(res.stdout.trim()).map_err(|e| Error::CommandFailed {
        exit_code: 0,
        stdout: res.stdout.clone(),
        stderr: format!("could not decode {label} output: {e}"),
        command: label.to_string(),
    })
}
