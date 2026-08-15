//! Where cache entries are stored, and how to reach it.
//!
//! Resolution order is environment, then `cache.toml`, then the default — so CI
//! can point a run at a bucket with `BSDKRUN_CACHE_*` without writing a file,
//! and a workstation can set it once and forget.
//!
//! ```toml
//! # ~/.config/bsdkrun/cache.toml
//! backend = "s3"          # or "disk" (the default)
//!
//! [s3]
//! bucket   = "my-ci-cache"
//! region   = "us-east-1"
//! prefix   = "bsdkrun"                       # optional key prefix
//! endpoint = "https://<id>.r2.cloudflarestorage.com"  # optional: R2, MinIO, …
//! ```
//!
//! Credentials are **never** read from that file — they come from
//! `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (+ `AWS_SESSION_TOKEN`), so a
//! config you can commit never holds a secret.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Which store cache entries live in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// A directory on this host — the default, and the whole feature for a
    /// single machine.
    Disk,
    /// An S3-compatible bucket, for sharing a cache between hosts or CI runs.
    S3(S3Config),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct S3Config {
    pub bucket: String,
    #[serde(default = "default_region")]
    pub region: String,
    /// Key prefix inside the bucket, so one bucket can hold several projects.
    #[serde(default)]
    pub prefix: String,
    /// Override the endpoint for a non-AWS implementation (R2, MinIO, Ceph).
    #[serde(default)]
    pub endpoint: Option<String>,
}

fn default_region() -> String {
    "us-east-1".to_string()
}

#[derive(Debug, Default, serde::Deserialize)]
struct File {
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    s3: Option<S3Config>,
}

impl S3Config {
    /// Base URL for the bucket, honouring a custom endpoint.
    ///
    /// AWS is addressed virtual-host style (`bucket.s3.region.amazonaws.com`);
    /// a custom endpoint is addressed path style (`endpoint/bucket`), which is
    /// what MinIO defaults to and what R2 requires.
    pub fn base_url(&self) -> String {
        match &self.endpoint {
            Some(ep) => format!("{}/{}", ep.trim_end_matches('/'), self.bucket),
            None => format!("https://{}.s3.{}.amazonaws.com", self.bucket, self.region),
        }
    }

    /// Host header for signing — the URL's authority.
    pub fn host(&self) -> String {
        let url = self.base_url();
        let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(&url);
        after_scheme
            .split('/')
            .next()
            .unwrap_or(after_scheme)
            .to_string()
    }

    /// Full object key for a cache entry, including any configured prefix.
    pub fn object_key(&self, name: &str) -> String {
        match self.prefix.trim_matches('/') {
            "" => name.to_string(),
            p => format!("{p}/{name}"),
        }
    }
}

/// Resolve the backend from the environment, then `cache.toml`, then the
/// default.
pub fn resolve() -> Result<Backend> {
    let file = load_file();

    let chosen = env("BSDKRUN_CACHE_BACKEND")
        .or_else(|| file.backend.clone())
        .unwrap_or_else(|| "disk".to_string());

    match chosen.to_ascii_lowercase().as_str() {
        "disk" | "local" | "host" => Ok(Backend::Disk),
        "s3" => Ok(Backend::S3(s3_config(file.s3)?)),
        other => bail!("unknown cache backend {other:?} — expected `disk` or `s3`"),
    }
}

fn s3_config(from_file: Option<S3Config>) -> Result<S3Config> {
    let mut cfg = from_file.unwrap_or_default();
    if let Some(v) = env("BSDKRUN_CACHE_S3_BUCKET") {
        cfg.bucket = v;
    }
    if let Some(v) = env("BSDKRUN_CACHE_S3_REGION").or_else(|| env("AWS_REGION")) {
        cfg.region = v;
    }
    if let Some(v) = env("BSDKRUN_CACHE_S3_PREFIX") {
        cfg.prefix = v;
    }
    if let Some(v) = env("BSDKRUN_CACHE_S3_ENDPOINT") {
        cfg.endpoint = Some(v);
    }
    if cfg.region.is_empty() {
        cfg.region = default_region();
    }
    if cfg.bucket.is_empty() {
        bail!(
            "the S3 cache backend needs a bucket. Set BSDKRUN_CACHE_S3_BUCKET, or add one to {}:\n\
             \n  backend = \"s3\"\n  \n  [s3]\n  bucket = \"my-ci-cache\"\n  region = \"us-east-1\"",
            config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "~/.config/bsdkrun/cache.toml".to_string())
        );
    }
    Ok(cfg)
}

/// S3 credentials, from the environment only.
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

pub fn credentials() -> Result<Credentials> {
    let access_key = env("AWS_ACCESS_KEY_ID").context(
        "the S3 cache backend needs AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY in the \
         environment (they are deliberately not read from cache.toml, so the file stays \
         safe to commit)",
    )?;
    let secret_key = env("AWS_SECRET_ACCESS_KEY")
        .context("AWS_ACCESS_KEY_ID is set but AWS_SECRET_ACCESS_KEY is not")?;
    Ok(Credentials {
        access_key,
        secret_key,
        session_token: env("AWS_SESSION_TOKEN"),
    })
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// `$XDG_CONFIG_HOME/bsdkrun/cache.toml`, else `~/.config/bsdkrun/cache.toml`.
pub fn config_path() -> Result<PathBuf> {
    let base = match env("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?)
            .join(".config"),
    };
    Ok(base.join("bsdkrun").join("cache.toml"))
}

fn load_file() -> File {
    let Ok(path) = config_path() else {
        return File::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return File::default();
    };
    match toml::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("ignoring {}: {e}", path.display());
            File::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_is_virtual_host_addressed_and_custom_endpoints_are_path_addressed() {
        let aws = S3Config {
            bucket: "b".into(),
            region: "eu-west-1".into(),
            ..Default::default()
        };
        assert_eq!(aws.base_url(), "https://b.s3.eu-west-1.amazonaws.com");
        assert_eq!(aws.host(), "b.s3.eu-west-1.amazonaws.com");

        let r2 = S3Config {
            bucket: "b".into(),
            region: "auto".into(),
            endpoint: Some("https://acct.r2.cloudflarestorage.com/".into()),
            ..Default::default()
        };
        assert_eq!(r2.base_url(), "https://acct.r2.cloudflarestorage.com/b");
        assert_eq!(r2.host(), "acct.r2.cloudflarestorage.com");
    }

    #[test]
    fn a_prefix_is_applied_and_slashes_are_not_doubled() {
        let mut cfg = S3Config {
            bucket: "b".into(),
            ..Default::default()
        };
        assert_eq!(cfg.object_key("x.tar.gz"), "x.tar.gz");
        cfg.prefix = "/team/ci/".into();
        assert_eq!(cfg.object_key("x.tar.gz"), "team/ci/x.tar.gz");
    }

    /// The error has to say what to set; a bare "missing bucket" sends people
    /// looking for a flag that does not exist.
    #[test]
    fn a_bucketless_s3_config_explains_how_to_set_one() {
        let err = s3_config(Some(S3Config::default()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("BSDKRUN_CACHE_S3_BUCKET"), "{err}");
        assert!(err.contains("cache.toml"), "{err}");
    }
}
