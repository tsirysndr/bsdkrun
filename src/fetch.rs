//! `fetch` / `versions` subcommands: download a BSD arm64 VM image and prepare
//! it for booting under bsdkrun.
//!
//! Everything is done by shelling out to tools already present on macOS —
//! `curl` (download), `xz`/`gzip` (decompress), and `hdiutil`/`diskutil` (mount
//! the FAT ESP so we can drop a console hint on it). No extra Rust dependencies.
//!
//! Downloaded+decompressed images are cached under `$HOME` (see [`cache_dir`]),
//! so a version fetched before is never re-downloaded; `--dir` receives a hard
//! link (or symlink) to the cached file — no second multi-GiB copy.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use tracing::{info, warn};

const FREEBSD_VM_IMAGES: &str = "https://download.freebsd.org/releases/VM-IMAGES";
const NETBSD_PUB: &str = "https://cdn.netbsd.org/pub/NetBSD";
const NETBSD_DAILY_HEAD: &str = "https://nycdn.netbsd.org/pub/NetBSD-daily/HEAD/latest";

/// Guest operating systems bsdkrun can provision.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Os {
    Freebsd,
    Netbsd,
}

/// FreeBSD's `/boot/loader.conf` lives on UFS (macOS can't write it), but the
/// EFI loader reads this from the FAT ESP before mounting UFS. NetBSD needs no
/// such hint — its kernel auto-selects the PL011 UART as console.
const FREEBSD_LOADER_ENV: &str = "\
console=efi,eficom
boot_serial=YES
boot_multicons=YES
comconsole_speed=115200
efi_com_speed=115200
";

impl Os {
    fn slug(self) -> &'static str {
        match self {
            Os::Freebsd => "freebsd",
            Os::Netbsd => "netbsd",
        }
    }

    /// File extension of the *decompressed* image.
    fn raw_ext(self) -> &'static str {
        match self {
            Os::Freebsd => "raw",
            Os::Netbsd => "img",
        }
    }

    /// Compression of the published image.
    fn comp(self) -> Comp {
        match self {
            Os::Freebsd => Comp::Xz,
            Os::Netbsd => Comp::Gz,
        }
    }

    /// Version used when the user doesn't pass `--version`.
    fn default_version(self) -> Result<String> {
        match self {
            // Newest published release.
            Os::Freebsd => Ok(self
                .all_versions()?
                .pop()
                .ok_or_else(|| anyhow::anyhow!("no FreeBSD releases found"))?),
            // Releases (<=10.x) lack modern virtio-mmio, so they can't mount a
            // root disk under libkrun; -current is the one that works fully.
            Os::Netbsd => Ok("current".to_string()),
        }
    }

    fn image_url(self, version: &str) -> String {
        match self {
            Os::Freebsd => format!(
                "{FREEBSD_VM_IMAGES}/{version}-RELEASE/aarch64/Latest/\
                 FreeBSD-{version}-RELEASE-arm64-aarch64-ufs.raw.xz"
            ),
            Os::Netbsd if version == "current" => {
                format!("{NETBSD_DAILY_HEAD}/evbarm-aarch64/binary/gzimg/arm64.img.gz")
            }
            Os::Netbsd => {
                format!("{NETBSD_PUB}/NetBSD-{version}/evbarm-aarch64/binary/gzimg/arm64.img.gz")
            }
        }
    }

    /// Published release versions (ascending). `current`, where applicable, is
    /// not listed here — it's handled separately.
    fn all_versions(self) -> Result<Vec<String>> {
        let (url, prefix, suffix) = match self {
            Os::Freebsd => (format!("{FREEBSD_VM_IMAGES}/"), "", "-RELEASE/"),
            Os::Netbsd => (format!("{NETBSD_PUB}/"), "NetBSD-", "/"),
        };
        let listing = curl_text(&url)?;
        let mut versions: Vec<(u32, u32, String)> = listing
            .split("href=\"")
            .filter_map(|s| s.split('"').next())
            .filter_map(|s| s.strip_prefix(prefix)?.strip_suffix(suffix))
            .filter_map(|v| {
                let mut parts = v.split('.');
                let maj = parts.next()?.parse().ok()?;
                let min = parts.next()?.parse().ok()?;
                Some((maj, min, v.to_string()))
            })
            .collect();
        versions.sort();
        versions.dedup();
        Ok(versions.into_iter().map(|(_, _, v)| v).collect())
    }

    /// Write a console hint onto the image where the OS's bootloader needs one.
    fn prepare_console(self, raw: &Path) -> Result<()> {
        match self {
            Os::Freebsd => {
                info!("writing serial-console loader.env onto the image's ESP…");
                write_freebsd_loader_env(raw)
            }
            // NetBSD picks up the PL011 UART on its own.
            Os::Netbsd => Ok(()),
        }
    }

    /// Note anything the user should know about the chosen version.
    fn warn_if_unsupported(self, version: &str) {
        match self {
            Os::Netbsd if version != "current" => warn!(
                "NetBSD {version} is a release whose virtio-mmio driver is legacy-only: it boots \
                 (kernel + console work) but cannot see libkrun's virtio devices, so it can't \
                 mount its root disk. Use `--version current` for a fully working system."
            ),
            _ => {}
        }
    }
}

enum Comp {
    Xz,
    Gz,
}

impl Comp {
    /// Filename suffix of the compressed download (empty when uncompressed).
    fn ext(&self) -> &'static str {
        match self {
            Comp::Xz => "xz",
            Comp::Gz => "gz",
        }
    }

    /// Decompress `file` in place (removing the compressed original).
    fn decompress(&self, file: &Path) -> Result<()> {
        let bin = match self {
            Comp::Xz => "xz",
            Comp::Gz => "gzip",
        };
        run(
            Command::new(bin).arg("-d").arg("-f").arg(file),
            &format!("{bin} (decompress)"),
        )
    }
}

/// Download + prepare an image. Returns the path to the ready raw image.
pub fn fetch(os: Os, version: Option<String>, dir: &Path, force: bool) -> Result<PathBuf> {
    let version = match version {
        Some(v) => v,
        None => {
            info!("resolving default version…");
            os.default_version()?
        }
    };
    info!(os = os.slug(), version = %version, "selected");
    os.warn_if_unsupported(&version);

    let cache = cache_dir()?;
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("creating cache dir {}", cache.display()))?;

    let base = format!("{}-{version}", os.slug());
    let raw = cache.join(format!("{base}.{}", os.raw_ext()));
    let comp = cache.join(format!("{base}.{}.{}", os.raw_ext(), os.comp().ext()));

    if raw.exists() && !force {
        info!(path = %raw.display(), "using cached image (already downloaded)");
    } else {
        let url = os.image_url(&version);
        info!(%url, "downloading (this is a few hundred MiB)…");
        let _ = std::fs::remove_file(&comp); // drop any stale/partial download
        run(
            Command::new("curl")
                .args(["-L", "--fail", "--progress-bar", "-o"])
                .arg(&comp)
                .arg(&url),
            "curl (download image)",
        )
        .with_context(|| format!("downloading {url} — is that version published?"))?;

        info!("decompressing (expands to a couple GiB)…");
        os.comp().decompress(&comp)?;
    }

    os.prepare_console(&raw)?;

    let ready = materialize(&raw, dir)?;

    // Record the fetched BSD image so `bsdkrun images` lists it alongside OCI
    // images. Keyed by path (unique), so a re-fetch just updates the row.
    let size = std::fs::metadata(&ready).map(|m| m.len() as i64).unwrap_or(0);
    crate::db::record_image(
        &format!("{}-{version}", os.slug()),
        &format!("file:{}", ready.display()),
        size,
        &ready.to_string_lossy(),
    );

    info!(image = %ready.display(), "ready — boot it with: bsdkrun firmware --firmware images/KRUN_EFI.fd --disk {}", ready.display());
    Ok(ready)
}

/// Grow a raw disk image to `size` (e.g. "8G"). Only ever enlarges the file.
///
/// NetBSD's arm64 image expands its root filesystem to fill the new space
/// automatically on the next boot (its root partition is last on the disk and
/// `resize_root` runs on boot) — no in-guest steps needed. FreeBSD's image
/// won't: its UFS root is followed by swap, so the trailing space isn't
/// adjacent to root (you'd need to repartition + `growfs` by hand).
pub fn grow(disk: &Path, size: &str) -> Result<()> {
    let target = parse_size(size)?;
    let meta = std::fs::metadata(disk).with_context(|| format!("stat {}", disk.display()))?;
    let current = meta.len();
    if target <= current {
        bail!(
            "--size {size} ({target} bytes) is not larger than the current image \
             ({current} bytes); grow only enlarges disks"
        );
    }
    // Growing follows hard links, so a cache-backed image would grow too.
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() > 1 {
        warn!(
            "{} has {} hard links (e.g. into ~/.cache/bsdkrun) — growing enlarges all of them",
            disk.display(),
            meta.nlink()
        );
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(disk)
        .with_context(|| format!("opening {}", disk.display()))?
        .set_len(target)
        .with_context(|| format!("growing {}", disk.display()))?;

    info!(from = current, to = target, image = %disk.display(), "grew image");
    info!(
        "NetBSD expands its root filesystem to fill the new space on next boot. \
         (FreeBSD's root is followed by swap, so it won't auto-grow.)"
    );
    Ok(())
}

/// Parse a size like `8G`, `4096M`, `1.5g`, or a plain byte count.
fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, mult): (&str, u64) = match s.chars().last() {
        Some('G' | 'g') => (&s[..s.len() - 1], 1 << 30),
        Some('M' | 'm') => (&s[..s.len() - 1], 1 << 20),
        Some('K' | 'k') => (&s[..s.len() - 1], 1 << 10),
        _ => (s, 1),
    };
    let val: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid size: {s:?} (try e.g. 8G, 4096M)"))?;
    if val <= 0.0 {
        bail!("size must be positive: {s:?}");
    }
    Ok((val * mult as f64) as u64)
}

/// Print the available builds for `os`.
pub fn list_versions(os: Os) -> Result<()> {
    let versions = os.all_versions()?;
    match os {
        Os::Freebsd => {
            let latest = versions.last().cloned().unwrap_or_default();
            println!("Available FreeBSD arm64 releases:");
            for v in &versions {
                let tag = if *v == latest { "  (latest)" } else { "" };
                println!("  {v}{tag}");
            }
        }
        Os::Netbsd => {
            println!("Available NetBSD arm64 builds:");
            println!("  current  (recommended — modern virtio-mmio; boots to root under libkrun)");
            for v in versions.iter().rev() {
                println!("  {v:<7}  (release; boots but no root disk under libkrun — legacy virtio-mmio)");
            }
        }
    }
    Ok(())
}

fn curl_text(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args(["-sL", "--fail", "--max-time", "30", url])
        .output()
        .context("running curl")?;
    if !out.status.success() {
        bail!("curl failed to fetch {url}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Cache directory for downloaded images: `$BSDKRUN_CACHE`, else
/// `$XDG_CACHE_HOME/bsdkrun`, else `$HOME/.cache/bsdkrun`.
pub(crate) fn cache_dir() -> Result<PathBuf> {
    if let Ok(c) = std::env::var("BSDKRUN_CACHE") {
        if !c.is_empty() {
            return Ok(PathBuf::from(c));
        }
    }
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("bsdkrun"));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".cache").join("bsdkrun"))
}

/// Expose the cached image inside `dir` without copying its bytes — a hard link
/// when possible, else a symlink. Returns the usable path. If `dir` already is
/// the cache directory, the cached path is returned as-is.
fn materialize(cached: &Path, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating output dir {}", dir.display()))?;
    let name = cached
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("bad cached path {}", cached.display()))?;
    let out = dir.join(name);

    let same_dir = match (
        std::fs::canonicalize(dir),
        cached.parent().map(std::fs::canonicalize),
    ) {
        (Ok(a), Some(Ok(b))) => a == b,
        _ => out == *cached,
    };
    if same_dir {
        return Ok(cached.to_path_buf());
    }

    let _ = std::fs::remove_file(&out);
    if std::fs::hard_link(cached, &out).is_err() {
        std::os::unix::fs::symlink(cached, &out)
            .with_context(|| format!("linking {} -> {}", out.display(), cached.display()))?;
    }
    Ok(out)
}

/// Mount the image's FAT ESP and write FreeBSD's `loader.env` onto it.
fn write_freebsd_loader_env(raw: &Path) -> Result<()> {
    // Attach the raw image without mounting, and find the EFI (FAT) slice.
    let out = Command::new("hdiutil")
        .args([
            "attach",
            "-imagekey",
            "diskimage-class=CRawDiskImage",
            "-nomount",
        ])
        .arg(raw)
        .output()
        .context("hdiutil attach")?;
    if !out.status.success() {
        bail!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let esp_dev = listing
        .lines()
        .find(|l| l.contains("EFI") && !l.contains("GUID"))
        .and_then(|l| l.split_whitespace().next())
        .ok_or_else(|| anyhow::anyhow!("no EFI partition found in {}", raw.display()))?
        .to_string();
    let whole_disk = esp_dev
        .rfind('s')
        .map(|i| &esp_dev[..i])
        .unwrap_or(&esp_dev)
        .to_string();

    // Everything after attach must be balanced by a detach, so wrap the work.
    let result = mount_and_write(&esp_dev);
    let _ = Command::new("hdiutil")
        .arg("detach")
        .arg(&whole_disk)
        .output();
    result
}

fn mount_and_write(esp_dev: &str) -> Result<()> {
    let mount = std::env::temp_dir().join(format!("bsdkrun-esp-{}", std::process::id()));
    std::fs::create_dir_all(&mount).context("creating ESP mountpoint")?;

    run(
        Command::new("diskutil")
            .arg("mount")
            .arg("-mountPoint")
            .arg(&mount)
            .arg(esp_dev)
            // Keep diskutil's chatter off stdout (the `freebsd` shortcut prints
            // the machine id there).
            .stdout(std::process::Stdio::null()),
        "diskutil mount (ESP)",
    )?;

    let write_result = (|| -> Result<()> {
        let dir = mount.join("EFI/freebsd");
        std::fs::create_dir_all(&dir).context("creating EFI/freebsd on ESP")?;
        std::fs::write(dir.join("loader.env"), FREEBSD_LOADER_ENV).context("writing loader.env")?;
        let _ = Command::new("dot_clean").arg(&mount).output();
        for junk in [".fseventsd", "EFI/._freebsd", "EFI/freebsd/._loader.env"] {
            let _ = std::fs::remove_dir_all(mount.join(junk));
            let _ = std::fs::remove_file(mount.join(junk));
        }
        Ok(())
    })();

    if let Err(e) = run(
        Command::new("diskutil")
            .arg("unmount")
            .arg(&mount)
            .stdout(std::process::Stdio::null()),
        "diskutil unmount (ESP)",
    ) {
        warn!("failed to unmount ESP cleanly: {e}");
    }
    let _ = std::fs::remove_dir(&mount);

    write_result
}

/// Run a command, streaming its stdout/stderr, and error if it fails.
pub(crate) fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {what}"))?;
    if !status.success() {
        bail!("{what} exited with {status}");
    }
    Ok(())
}
