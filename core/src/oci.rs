//! Minimal OCI registry client: pull a `linux/arm64` image from Docker Hub (or
//! any v2 registry) and extract its rootfs to a directory, along with the
//! image's runtime config (Entrypoint / Cmd / Env / WorkingDir / User).
//!
//! In keeping with the rest of bsdkrun, all HTTP goes through `curl` and layer
//! tarballs are unpacked with `tar` (bsdtar auto-detects gzip/zstd/xz) — no
//! async runtime or HTTP crate. `serde_json` parses the manifests and config.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::fetch::run;

/// An image's OCI runtime config — the bits we need to reconstruct its
/// entrypoint inside the guest.
#[derive(Debug, Default)]
pub struct ImageConfig {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub workdir: String,
    /// Image USER. Parsed but not yet honored — the guest runs as root for now.
    #[allow(dead_code)]
    pub user: String,
}

/// A pulled image: its extracted rootfs directory and runtime config.
pub struct Image {
    pub rootfs: PathBuf,
    pub config: ImageConfig,
    /// The image's content (config) digest, e.g. `sha256:…`.
    pub digest: String,
    /// Total size of the image's layers, in bytes.
    pub size: i64,
}

/// Media types we accept for a manifest request — both OCI and Docker schema2,
/// single-image and multi-arch index.
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

/// Pull `reference` (e.g. `alpine`, `alpine:3.20`, `docker.io/library/nginx`,
/// `ghcr.io/owner/name:tag`) for linux/arm64 and return its extracted rootfs +
/// config. The extracted rootfs is cached under the bsdkrun cache by the image's
/// content digest, so a repeat pull of the same image is essentially free.
pub fn pull(reference: &str) -> Result<Image> {
    let r = Ref::parse(reference)?;
    info!(registry = %r.endpoint, repo = %r.repository, reference = %r.reference, "resolving OCI image");

    let token = get_token(&r)?;

    // Resolve the reference to a concrete image manifest for the host arch.
    let oci_arch = crate::host::Arch::current()?.oci();
    let manifest = get_manifest(&r, &r.reference, &token)?;
    let image_manifest = if is_index(&manifest) {
        let digest = select_platform(&manifest, oci_arch).with_context(|| {
            format!("{reference} has no linux/{oci_arch} image in its manifest index")
        })?;
        info!(%digest, arch = oci_arch, "selected manifest");
        get_manifest(&r, &digest, &token)?
    } else {
        manifest
    };

    let config_digest = image_manifest["config"]["digest"]
        .as_str()
        .context("image manifest has no config.digest")?
        .to_string();
    let total_size: i64 = image_manifest["layers"]
        .as_array()
        .map(|ls| ls.iter().filter_map(|l| l["size"].as_i64()).sum())
        .unwrap_or(0);

    let layers = image_manifest["layers"]
        .as_array()
        .context("image manifest has no layers")?
        .clone();
    let layer_digests: Vec<String> = layers
        .iter()
        .filter_map(|l| l["digest"].as_str().map(str::to_string))
        .collect();

    // Content-addressed cache: identical image contents => identical config
    // digest => reuse the already-extracted rootfs.
    //
    // Reuse needs the completion marker, not just the tree: a pull that lost a
    // layer part-way (ENOSPC, a short download) still leaves a plausible-looking
    // rootfs behind, and the cache is keyed by content so every later run would
    // reuse it forever. A half-extracted rootfs boots into a kernel panic —
    // "Requested init /.bsdkrun-init failed (error -2)" — because ENOENT there
    // is the *interpreter* of our init's `#!/bin/sh`, not the init itself.
    let dir = crate::fetch::oci_cache_dir()?.join(digest_to_dirname(&config_digest));
    let rootfs = dir.join("rootfs");
    let config_path = dir.join("config.json");
    if rootfs.exists() && config_path.exists() {
        if cached_layers(&dir).as_deref() == Some(layer_digests.as_slice()) {
            info!(path = %rootfs.display(), "using cached rootfs");
            let cfg = std::fs::read(&config_path).context("reading cached image config")?;
            return Ok(Image {
                rootfs,
                config: parse_config(&serde_json::from_slice(&cfg)?)?,
                digest: config_digest,
                size: total_size,
            });
        }
        warn!(
            path = %dir.display(),
            "cached rootfs is incomplete or was extracted by an older bsdkrun — re-pulling"
        );
    }

    // Fresh pull: download the config blob and every layer, then extract.
    let config_blob = get_blob(&r, &config_digest, &token, None)?;
    let config_json: Value =
        serde_json::from_slice(&config_blob).context("parsing image config blob")?;
    let config = parse_config(&config_json)?;

    // Extract into a temp dir first, then atomically swap in — so an interrupted
    // pull never leaves a half-extracted rootfs looking cached.
    let staging = dir.with_extension("staging");
    let _ = std::fs::remove_dir_all(&staging);
    let staging_rootfs = staging.join("rootfs");
    std::fs::create_dir_all(&staging_rootfs)
        .with_context(|| format!("creating {}", staging_rootfs.display()))?;

    let total_bytes: u64 = layers.iter().filter_map(|l| l["size"].as_u64()).sum();
    let (mut cached_bytes, mut downloaded_bytes) = (0u64, 0u64);
    info!(
        count = layers.len(),
        total = %human_size(total_bytes),
        "pulling image layers"
    );
    for (i, layer) in layers.iter().enumerate() {
        let digest = layer["digest"]
            .as_str()
            .with_context(|| format!("layer {i} has no digest"))?;
        let size = layer["size"].as_u64().unwrap_or(0);
        // Layers are shared between images far more often than whole images
        // are: every flavor built `FROM node:24` has the same base layers, and
        // a tag that moves upstream usually changes one layer out of ten. The
        // cache is keyed by layer digest, so all of that is downloaded once.
        let blob = match cached_blob(digest, size) {
            Some(path) => {
                eprintln!(
                    "  layer {}/{}  {}  (cached)",
                    i + 1,
                    layers.len(),
                    human_size(size)
                );
                cached_bytes += size;
                path
            }
            None => {
                // A blank line keeps curl's in-place progress bar from being
                // overwritten by the next log line.
                eprintln!("  layer {}/{}  {}", i + 1, layers.len(), human_size(size));
                // Downloaded under a temporary name in the cache directory, so
                // the rename into place is atomic and on the same filesystem.
                let tmp = blob_dir()?.join(format!(".download-{}-{i}", std::process::id()));
                get_blob(&r, digest, &token, Some(&tmp))?;
                // A short blob means the transfer was cut off. bsdtar reports a
                // truncated archive as a *warning* (exit 1) and still extracts
                // the part it read, so without this check the missing files
                // would be baked into the cache.
                if size > 0 {
                    let got = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
                    if got != size {
                        let _ = std::fs::remove_file(&tmp);
                        bail!(
                            "layer {}/{} ({digest}) downloaded {} of {} — the transfer was cut short",
                            i + 1,
                            layers.len(),
                            human_size(got),
                            human_size(size)
                        );
                    }
                }
                downloaded_bytes += size;
                store_blob(&tmp, digest)?
            }
        };
        extract_layer(&blob, &staging_rootfs)
            .with_context(|| format!("extracting layer {i} ({digest})"))?;
        apply_whiteouts(&staging_rootfs)?;
        // Deliberately kept: deleting it here is what used to make the next
        // image re-download layers it already had.
    }

    std::fs::write(
        staging.join("config.json"),
        serde_json::to_vec_pretty(&config_json)?,
    )
    .context("caching image config")?;
    // Written last, and only on the success path: this is what marks the tree
    // complete for the cache check above.
    std::fs::write(
        staging.join(PULL_MARKER),
        serde_json::to_vec(&layer_digests)?,
    )
    .context("recording the extracted layers")?;

    // Swap staging into place.
    let _ = std::fs::remove_dir_all(&dir);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::rename(&staging, &dir)
        .with_context(|| format!("moving extracted image into cache at {}", dir.display()))?;

    if cached_bytes > 0 {
        info!(
            reused = %human_size(cached_bytes),
            downloaded = %human_size(downloaded_bytes),
            "layer cache hit"
        );
    }
    // After the pull, not before: evicting a layer this pull is about to reuse
    // would be the one moment the cache costs more than it saves.
    prune_blobs();

    info!(rootfs = %rootfs.display(), "image ready");
    Ok(Image {
        rootfs,
        config,
        digest: config_digest,
        size: total_size,
    })
}

/// Where downloaded layer blobs are kept, keyed by their digest.
///
/// Beside the extracted rootfs trees rather than inside one, because the whole
/// point is that a blob outlives the image that first pulled it.
fn blob_dir() -> Result<PathBuf> {
    let dir = crate::fetch::oci_cache_dir()?.join("blobs");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn blob_path(digest: &str) -> Result<PathBuf> {
    Ok(blob_dir()?.join(digest_to_dirname(digest)))
}

/// How much disk the blob cache may use before the oldest entries are dropped.
///
/// Layers are compressed, so this buys a lot of images. Override with
/// `BSDKRUN_OCI_BLOB_CACHE` (bytes); `0` disables the cache entirely, which is
/// the pre-cache behaviour for anyone who would rather have the disk.
const BLOB_CACHE_DEFAULT: u64 = 20 * 1024 * 1024 * 1024;

fn blob_cache_limit() -> u64 {
    std::env::var("BSDKRUN_OCI_BLOB_CACHE")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(BLOB_CACHE_DEFAULT)
}

/// A cached layer, if one is there and is the right size.
///
/// Size is a cheap check and a sound one here: a blob only enters the cache
/// after its digest has been verified, and it is renamed into place in one
/// step, so a truncated download is never visible under its final name.
fn cached_blob(digest: &str, size: u64) -> Option<PathBuf> {
    if blob_cache_limit() == 0 {
        return None;
    }
    let path = blob_path(digest).ok()?;
    let got = std::fs::metadata(&path).ok()?.len();
    if size > 0 && got != size {
        // A stale or half-written file under the final name should not be
        // trusted; drop it and let the caller re-download.
        let _ = std::fs::remove_file(&path);
        return None;
    }
    // Touched so the prune below evicts genuinely cold layers rather than the
    // ones that keep being reused. Best-effort: a cache whose LRU ordering is
    // slightly wrong still works, and a read-only store should not fail a pull.
    if let Ok(f) = std::fs::File::options().write(true).open(&path) {
        let _ = f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
    }
    Some(path)
}

/// Verify a downloaded blob against its digest and move it into the cache.
///
/// Verified on the way in, once, rather than on every hit: hashing a 500 MB
/// layer on each reuse would give back the time the cache is there to save.
fn store_blob(tmp: &Path, digest: &str) -> Result<PathBuf> {
    if let Some(want) = digest.strip_prefix("sha256:") {
        let mut file = std::fs::File::open(tmp)
            .with_context(|| format!("reading {} to verify it", tmp.display()))?;
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        std::io::copy(&mut file, &mut hasher).context("hashing the downloaded layer")?;
        let got = format!("{:x}", sha2::Digest::finalize(hasher));
        if got != want {
            let _ = std::fs::remove_file(tmp);
            bail!("layer {digest} hashed to sha256:{got} — the download is corrupt");
        }
    }
    let path = blob_path(digest)?;
    // Rename, so the blob is only ever visible under its final name complete.
    // The download already lands in this directory, so this is normally a
    // same-filesystem rename — but on macOS the cache is a sparsebundle, and a
    // caller handing us a path from anywhere else gets EXDEV. Copy to a
    // neighbouring temp file and rename that, rather than copying straight to
    // the final name and briefly publishing a partial blob.
    match std::fs::rename(tmp, &path) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let staging = path.with_extension("incoming");
            std::fs::copy(tmp, &staging)
                .with_context(|| format!("copying the layer into {}", staging.display()))?;
            std::fs::rename(&staging, &path)
                .with_context(|| format!("moving the layer into place at {}", path.display()))?;
            let _ = std::fs::remove_file(tmp);
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("moving the layer into the cache at {}", path.display()))
        }
    }
    Ok(path)
}

/// Drop the least recently used blobs until the cache fits its limit.
fn prune_blobs() {
    let limit = blob_cache_limit();
    let Ok(dir) = blob_dir() else { return };
    let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                if !m.is_file() {
                    return None;
                }
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect(),
        Err(_) => return,
    };
    let total: u64 = entries.iter().map(|(_, len, _)| len).sum();
    if total <= limit {
        return;
    }
    // Oldest first.
    entries.sort_by_key(|(mtime, _, _)| *mtime);
    let mut freed = 0u64;
    for (_, len, path) in entries {
        if total - freed <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed += len;
        }
    }
    info!(
        freed = %human_size(freed),
        limit = %human_size(limit),
        "pruned the OCI layer cache"
    );
}

/// A parsed image reference broken into what the registry API needs.
struct Ref {
    /// HTTPS registry endpoint host (e.g. `registry-1.docker.io`).
    endpoint: String,
    /// Repository path (e.g. `library/alpine`).
    repository: String,
    /// Tag or digest to resolve (e.g. `latest`, `sha256:…`).
    reference: String,
}

impl Ref {
    fn parse(s: &str) -> Result<Self> {
        if s.is_empty() {
            bail!("empty image reference");
        }
        // Separate an optional digest (`@sha256:…`) or tag (`:tag`). A ':' is
        // only a tag when it comes after the last '/', so a `host:port` prefix
        // isn't mistaken for one.
        let (name, reference) = if let Some((n, d)) = s.split_once('@') {
            (n.to_string(), d.to_string())
        } else {
            let last_slash = s.rfind('/').map(|i| i + 1).unwrap_or(0);
            match s[last_slash..].split_once(':') {
                Some((_, tag)) => {
                    let cut = last_slash + s[last_slash..].find(':').unwrap();
                    (s[..cut].to_string(), tag.to_string())
                }
                None => (s.to_string(), "latest".to_string()),
            }
        };

        // Split an optional registry host off the front. A first component is a
        // registry when it looks like a hostname (has '.'/' :' or is localhost).
        let (host, repository) = match name.split_once('/') {
            Some((h, rest)) if h == "localhost" || h.contains('.') || h.contains(':') => {
                (h.to_string(), rest.to_string())
            }
            _ => ("docker.io".to_string(), name.clone()),
        };

        // Docker Hub quirks: the API host differs from the friendly name, and
        // single-name repos live under `library/`.
        let (endpoint, repository) = if host == "docker.io" || host == "index.docker.io" {
            let repo = if repository.contains('/') {
                repository
            } else {
                format!("library/{repository}")
            };
            ("registry-1.docker.io".to_string(), repo)
        } else {
            (host, repository)
        };

        Ok(Ref {
            endpoint,
            repository,
            reference,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("https://{}/v2/{}/{}", self.endpoint, self.repository, path)
    }
}

/// A raw HTTP response captured from `curl`.
struct Resp {
    code: u16,
    headers: String,
    body: Vec<u8>,
}

/// Perform a GET with `curl`, capturing status, headers, and body. `accept` and
/// `token` add the corresponding request headers when present. Follows
/// redirects (curl drops the Authorization header on cross-host redirects, which
/// is exactly right for blob downloads that redirect to a signed CDN URL).
fn curl_get(url: &str, accept: Option<&str>, token: Option<&str>) -> Result<Resp> {
    let tmp = std::env::temp_dir();
    let body_file = tmp.join(format!("bsdkrun-oci-body-{}", std::process::id()));
    let hdr_file = tmp.join(format!("bsdkrun-oci-hdr-{}", std::process::id()));

    let mut c = Command::new("curl");
    c.args(["-s", "-L", "--max-time", "300", "-o"])
        .arg(&body_file)
        .arg("-D")
        .arg(&hdr_file)
        .arg("-w")
        .arg("%{http_code}");
    if let Some(a) = accept {
        c.arg("-H").arg(format!("Accept: {a}"));
    }
    if let Some(t) = token {
        c.arg("-H").arg(format!("Authorization: Bearer {t}"));
    }
    c.arg(url);

    let out = c.output().context("running curl")?;
    if !out.status.success() {
        bail!("curl failed for {url}");
    }
    let code = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    let headers = std::fs::read_to_string(&hdr_file).unwrap_or_default();
    let body = std::fs::read(&body_file).unwrap_or_default();
    let _ = std::fs::remove_file(&body_file);
    let _ = std::fs::remove_file(&hdr_file);
    Ok(Resp {
        code,
        headers,
        body,
    })
}

/// Obtain a bearer token for pulling `r.repository`, if the registry wants one.
/// Public images on Docker Hub still require an anonymous token.
fn get_token(r: &Ref) -> Result<Option<String>> {
    // A request to the API root triggers the auth challenge with the realm.
    let probe = curl_get(&format!("https://{}/v2/", r.endpoint), None, None)?;
    if probe.code != 401 {
        return Ok(None); // no auth required
    }
    let challenge = probe
        .headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("www-authenticate:"))
        .context("registry returned 401 without a Www-Authenticate header")?;
    let realm = auth_param(challenge, "realm").context("auth challenge has no realm")?;
    let service = auth_param(challenge, "service").unwrap_or_default();

    let token_url = format!(
        "{realm}?service={service}&scope=repository:{}:pull",
        r.repository
    );
    let resp = curl_get(&token_url, None, None)?;
    if resp.code != 200 {
        bail!("token request to {realm} failed (HTTP {})", resp.code);
    }
    let json: Value = serde_json::from_slice(&resp.body).context("parsing token response")?;
    // Registries return the token as "token" and/or "access_token".
    let token = json["token"]
        .as_str()
        .or_else(|| json["access_token"].as_str())
        .context("token response had no token")?
        .to_string();
    Ok(Some(token))
}

/// Extract a `key="value"` parameter from a `Bearer …` challenge line.
fn auth_param(header: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = header.find(&needle)? + needle.len();
    let rest = &header[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn get_manifest(r: &Ref, reference: &str, token: &Option<String>) -> Result<Value> {
    // Retried on 5xx with growing waits, for one registry in particular:
    // nixery builds an image server-side on its first request, and a large
    // dependency set (a CI toolchain, say) takes longer than its gateway
    // timeout — the 504 arrives while the build keeps going, and the retry
    // finds it cached. Bounded, because a registry that is actually down
    // should fail in under two minutes, not spin forever.
    let url = r.url(&format!("manifests/{reference}"));
    let mut resp = curl_get(&url, Some(MANIFEST_ACCEPT), token.as_deref())?;
    for (attempt, wait) in [15u64, 30, 45].iter().enumerate() {
        if resp.code < 500 {
            break;
        }
        eprintln!(
            "  registry answered HTTP {} — likely still building the image;              retry {}/3 in {wait}s",
            resp.code,
            attempt + 1
        );
        std::thread::sleep(std::time::Duration::from_secs(*wait));
        resp = curl_get(&url, Some(MANIFEST_ACCEPT), token.as_deref())?;
    }
    if resp.code != 200 {
        bail!(
            "fetching manifest {reference} failed (HTTP {}): {}",
            resp.code,
            String::from_utf8_lossy(&resp.body).trim()
        );
    }
    serde_json::from_slice(&resp.body).context("parsing manifest JSON")
}

/// Download a blob. With `to`, streams it to that path and returns empty bytes;
/// otherwise returns the blob in memory (used for the small config blob).
fn get_blob(r: &Ref, digest: &str, token: &Option<String>, to: Option<&Path>) -> Result<Vec<u8>> {
    let url = r.url(&format!("blobs/{digest}"));
    match to {
        Some(path) => {
            let mut c = Command::new("curl");
            // `--progress-bar` shows a live bar + percentage on stderr as the
            // layer downloads (the layer size is logged separately by the caller).
            c.args(["--progress-bar", "-L", "--fail", "--max-time", "600", "-o"])
                .arg(path);
            if let Some(t) = token {
                c.arg("-H").arg(format!("Authorization: Bearer {t}"));
            }
            c.arg(&url);
            run(&mut c, &format!("curl (blob {digest})"))?;
            Ok(Vec::new())
        }
        None => {
            let resp = curl_get(&url, None, token.as_deref())?;
            if resp.code != 200 {
                bail!("fetching blob {digest} failed (HTTP {})", resp.code);
            }
            Ok(resp.body)
        }
    }
}

fn is_index(manifest: &Value) -> bool {
    let mt = manifest["mediaType"].as_str().unwrap_or_default();
    mt.contains("manifest.list") || mt.contains("image.index") || manifest["manifests"].is_array()
}

/// From a multi-arch index, pick the `linux/<arch>` image manifest digest. For
/// arm64, prefer the `v8` variant when several entries exist.
fn select_platform(index: &Value, arch: &str) -> Result<String> {
    let manifests = index["manifests"]
        .as_array()
        .context("manifest index has no manifests array")?;
    let matches = manifests.iter().filter(|m| {
        let p = &m["platform"];
        p["architecture"].as_str() == Some(arch) && p["os"].as_str() == Some("linux")
    });
    // Prefer variant v8 (arm64) or unspecified over odd variants.
    let chosen = matches
        .clone()
        .find(|m| matches!(m["platform"]["variant"].as_str(), Some("v8") | None))
        .or_else(|| matches.clone().next());
    chosen
        .and_then(|m| m["digest"].as_str())
        .map(|s| s.to_string())
        .with_context(|| format!("no linux/{arch} manifest in index"))
}

/// Parse the image config JSON into our `ImageConfig`.
fn parse_config(json: &Value) -> Result<ImageConfig> {
    let cfg = &json["config"];
    let str_list = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(ImageConfig {
        entrypoint: str_list(&cfg["Entrypoint"]),
        cmd: str_list(&cfg["Cmd"]),
        env: str_list(&cfg["Env"]),
        workdir: cfg["WorkingDir"].as_str().unwrap_or_default().to_string(),
        user: cfg["User"].as_str().unwrap_or_default().to_string(),
    })
}

/// `sha256:abcd…` -> `sha256-abcd…`, safe as a directory name.
fn digest_to_dirname(digest: &str) -> String {
    digest.replace(':', "-")
}

/// Format a byte count as a human-readable size (e.g. `3.2 MiB`).
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// Records the layer digests an image cache dir was built from. Its presence is
/// what makes the tree reusable — see the cache check in `pull`.
const PULL_MARKER: &str = "layers.json";

/// Layer digests recorded for an extracted image, or `None` if the marker is
/// missing or unreadable (an interrupted pull, or a pre-marker cache dir).
fn cached_layers(dir: &Path) -> Option<Vec<String>> {
    serde_json::from_slice(&std::fs::read(dir.join(PULL_MARKER)).ok()?).ok()
}

/// bsdtar messages that mean files were *lost*, not merely skipped. It reports
/// all of these at exit 1, the same code it uses for the benign unprivileged
/// skips, so the code alone can't tell "couldn't chown" from "disk filled up
/// half-way through the layer".
const FATAL_TAR_ERRORS: &[&str] = &[
    "No space left",
    "truncated",
    "Unexpected EOF",
    "Unrecognized archive format",
    "Damaged",
    "Write error",
    "Input/output error",
    "Read-only file system",
];

/// Did tar's stderr report lost data rather than a skipped attribute?
fn tar_lost_data(stderr: &str) -> bool {
    FATAL_TAR_ERRORS.iter().any(|m| stderr.contains(m))
}

/// Unpack a layer tarball into `rootfs`. bsdtar auto-detects the compression;
/// `-p` preserves permissions (ownership is skipped automatically when not
/// root, which is fine for a single-user microVM rootfs).
fn extract_layer(blob: &Path, rootfs: &Path) -> Result<()> {
    let mut c = Command::new("tar");
    c.arg("-xpf").arg(blob).arg("-C").arg(rootfs);
    // Some layers carry device nodes / setuid bits we can't recreate unprivileged;
    // let tar skip those rather than fail the whole extraction.
    let out = c.output().context("spawning tar")?;
    if !out.status.success() {
        // bsdtar warns (exit 1) on skipped entries but still extracts; treat a
        // hard failure (exit >1) as fatal — and exit 1 too when the message says
        // data was lost rather than skipped, because tar exits 1 either way and
        // the caller would otherwise cache the partial rootfs forever.
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if code > 1 || tar_lost_data(&stderr) {
            bail!(
                "tar failed to extract layer (exit {code}): {}",
                stderr.trim()
            );
        }
        warn!(
            "tar reported skipped entries while extracting a layer (unprivileged): {}",
            stderr.trim()
        );
    }
    Ok(())
}

/// Apply OCI whiteouts left by the most recently extracted layer.
///
/// `.wh.<name>` deletes `<name>` from the layers below; `.wh..wh..opq` marks its
/// directory opaque (hide everything inherited from below). We delete the marked
/// targets and remove the marker files themselves.
fn apply_whiteouts(root: &Path) -> Result<()> {
    fn walk(dir: &Path) -> Result<()> {
        let entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return Ok(()),
        };
        for e in &entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name == ".wh..wh..opq" {
                // Opaque dir: drop every non-marker sibling.
                for sib in &entries {
                    let sname = sib.file_name();
                    if !sname.to_string_lossy().starts_with(".wh.") {
                        remove_any(&sib.path());
                    }
                }
                let _ = std::fs::remove_file(e.path());
            } else if let Some(target) = name.strip_prefix(".wh.") {
                remove_any(&dir.join(target));
                let _ = std::fs::remove_file(e.path());
            }
        }
        // Recurse into surviving subdirectories.
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() && !p.is_symlink() {
                    walk(&p)?;
                }
            }
        }
        Ok(())
    }
    walk(root)
}

fn remove_any(p: &Path) {
    if p.is_dir() && !p.is_symlink() {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// Write a file into the rootfs, creating parent dirs. Used by the caller to
/// drop in the generated init.
pub fn write_rootfs_file(rootfs: &Path, rel: &str, contents: &[u8], mode: u32) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;
    let path = rootfs.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        // Nix-built images (nixery.dev among them) ship directories mode
        // 0555 — including `/` itself — so creating a file in the cloned
        // rootfs fails with EACCES. The clone is this machine's private
        // copy; making its directories writable changes nothing the guest
        // can observe that matters, and the alternative is a boot that
        // dies on `.bsdkrun-init` with a bare "Permission denied".
        if let Ok(meta) = std::fs::metadata(parent) {
            let mode = meta.permissions().mode();
            if mode & 0o200 == 0 {
                let _ =
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(mode | 0o700));
            }
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(mode)
        .open(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    f.write_all(contents)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tar exits 1 for both "couldn't chown" and "the disk filled up", so the
    /// message is the only thing separating a cacheable rootfs from a ruined one.
    #[test]
    fn benign_tar_warnings_are_not_treated_as_lost_data() {
        for s in [
            "tar: Can't set user=0/group=0 for bin/busybox",
            "tar: Can't restore time for etc/hosts",
            "tar: dev/console: Can't create special file",
            "",
        ] {
            assert!(!tar_lost_data(s), "{s:?} should be tolerated");
        }
    }

    #[test]
    fn tar_errors_that_lose_files_are_fatal() {
        for s in [
            "bin/busybox: truncated gzip input: Unknown error: -1",
            "tar: Write error: No space left on device",
            "tar: Unrecognized archive format",
            "tar: Unexpected EOF in archive",
        ] {
            assert!(tar_lost_data(s), "{s:?} should be fatal");
        }
    }

    /// An image cache dir is only reusable when the marker lists exactly the
    /// layers the manifest asks for — that is what stops a half-extracted rootfs
    /// from being served forever under its content-addressed name.
    #[test]
    fn cached_layers_reads_back_what_a_pull_recorded() {
        let dir = std::env::temp_dir().join(format!("bsdkrun-oci-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(cached_layers(&dir), None, "no marker => not reusable");

        let digests = vec!["sha256:aaa".to_string(), "sha256:bbb".to_string()];
        std::fs::write(dir.join(PULL_MARKER), serde_json::to_vec(&digests).unwrap()).unwrap();
        assert_eq!(cached_layers(&dir).as_deref(), Some(digests.as_slice()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A blob only enters the cache after its digest checks out — the whole
    /// reason a later hit can trust a cheap size comparison.
    #[test]
    fn a_corrupt_layer_is_rejected_before_it_is_cached() {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-blob-{}", std::process::id()));
        std::fs::write(&tmp, b"not what the digest says").unwrap();
        // sha256 of something else entirely.
        let wrong = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let err = store_blob(&tmp, wrong).unwrap_err().to_string();
        assert!(err.contains("the download is corrupt"), "{err}");
        assert!(
            !tmp.exists(),
            "the bad download should be removed, not left behind"
        );
    }

    #[test]
    fn a_matching_digest_is_accepted() {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-blob-ok-{}", std::process::id()));
        let body = b"hello layer";
        std::fs::write(&tmp, body).unwrap();
        let mut h = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut h, body);
        let digest = format!("sha256:{:x}", sha2::Digest::finalize(h));
        let path = store_blob(&tmp, &digest).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        let _ = std::fs::remove_file(path);
    }
}
