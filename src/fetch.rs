//! `fetch` subcommand: download a FreeBSD arm64 VM image and prepare it for
//! booting under bsdkrun (writes the serial-console `loader.env` onto its ESP).
//!
//! Everything is done by shelling out to tools already present on macOS —
//! `curl` (download), `xz` (decompress), and `hdiutil`/`diskutil` (mount the FAT
//! ESP so we can drop `loader.env` on it). No extra Rust dependencies.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

const MIRROR: &str = "https://download.freebsd.org/releases/VM-IMAGES";

/// The console hint written to `/efi/freebsd/loader.env` on the image's ESP.
/// FreeBSD's `/boot/loader.conf` lives on UFS (macOS can't write it), but the
/// EFI loader reads this file from the FAT ESP before mounting UFS.
const LOADER_ENV: &str = "\
console=efi,eficom
boot_serial=YES
boot_multicons=YES
comconsole_speed=115200
efi_com_speed=115200
";

/// Download + prepare a FreeBSD image. Returns the path to the ready raw image.
pub fn fetch(version: Option<String>, dir: &Path, force: bool) -> Result<PathBuf> {
    let version = match version {
        Some(v) => v,
        None => {
            info!("resolving latest FreeBSD release…");
            latest_version()?
        }
    };
    info!(version = %version, "FreeBSD version");

    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating output dir {}", dir.display()))?;

    let raw = dir.join(format!("freebsd-{version}.raw"));
    let xz = dir.join(format!("freebsd-{version}.raw.xz"));

    if raw.exists() && !force {
        info!(path = %raw.display(), "image already downloaded (pass --force to re-download)");
    } else {
        let url = image_url(&version);
        info!(%url, "downloading (this is a few hundred MiB)…");
        run(
            Command::new("curl")
                .args(["-L", "--fail", "--progress-bar", "-o"])
                .arg(&xz)
                .arg(&url),
            "curl (download image)",
        )
        .with_context(|| format!("downloading {url} — is that version published?"))?;

        info!("decompressing with xz (expands to several GiB)…");
        // `xz -d -f foo.raw.xz` removes the .xz and produces `foo.raw`.
        run(
            Command::new("xz").arg("-d").arg("-f").arg(&xz),
            "xz (decompress)",
        )?;
    }

    info!("writing serial-console loader.env onto the image's ESP…");
    prepare_console(&raw)?;

    info!(image = %raw.display(), "ready — boot it with: bsdkrun firmware --firmware images/KRUN_EFI.fd --disk {}", raw.display());
    Ok(raw)
}

/// All FreeBSD `X.Y` releases published under VM-IMAGES, sorted ascending.
pub fn all_versions() -> Result<Vec<String>> {
    let listing = curl_text(&format!("{MIRROR}/"))?;
    let mut versions: Vec<(u32, u32, String)> = listing
        .split("href=\"")
        .filter_map(|s| s.split('"').next())
        .filter_map(|s| s.strip_suffix("-RELEASE/"))
        .filter_map(|v| {
            let mut parts = v.split('.');
            let maj = parts.next()?.parse().ok()?;
            let min = parts.next()?.parse().ok()?;
            Some((maj, min, v.to_string()))
        })
        .collect();
    versions.sort();
    if versions.is_empty() {
        bail!("no FreeBSD releases found at {MIRROR}/");
    }
    Ok(versions.into_iter().map(|(_, _, v)| v).collect())
}

/// The highest `X.Y` release currently published.
pub fn latest_version() -> Result<String> {
    all_versions()?
        .pop()
        .ok_or_else(|| anyhow::anyhow!("no FreeBSD releases found at {MIRROR}/"))
}

/// Print the available FreeBSD releases (latest last, marked).
pub fn list_versions() -> Result<()> {
    let versions = all_versions()?;
    let latest = versions.last().cloned().unwrap_or_default();
    println!("Available FreeBSD arm64 releases:");
    for v in &versions {
        if *v == latest {
            println!("  {v}  (latest)");
        } else {
            println!("  {v}");
        }
    }
    Ok(())
}

fn image_url(version: &str) -> String {
    format!(
        "{MIRROR}/{version}-RELEASE/aarch64/Latest/\
         FreeBSD-{version}-RELEASE-arm64-aarch64-ufs.raw.xz"
    )
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

/// Mount the image's FAT ESP and write `loader.env` onto it, then detach.
fn prepare_console(raw: &Path) -> Result<()> {
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
    // Whole disk (for detach): strip a trailing "sN".
    let whole_disk = esp_dev
        .rfind('s')
        .map(|i| &esp_dev[..i])
        .unwrap_or(&esp_dev)
        .to_string();

    // Everything after attach must be balanced by a detach, so wrap the work.
    let result = write_loader_env(&esp_dev);

    // Always detach, even on error.
    let _ = Command::new("hdiutil")
        .arg("detach")
        .arg(&whole_disk)
        .output();
    result
}

fn write_loader_env(esp_dev: &str) -> Result<()> {
    let mount = std::env::temp_dir().join(format!("bsdkrun-esp-{}", std::process::id()));
    std::fs::create_dir_all(&mount).context("creating ESP mountpoint")?;

    run(
        Command::new("diskutil")
            .arg("mount")
            .arg("-mountPoint")
            .arg(&mount)
            .arg(esp_dev),
        "diskutil mount (ESP)",
    )?;

    let write_result = (|| -> Result<()> {
        let dir = mount.join("EFI/freebsd");
        std::fs::create_dir_all(&dir).context("creating EFI/freebsd on ESP")?;
        std::fs::write(dir.join("loader.env"), LOADER_ENV).context("writing loader.env")?;
        // Clean up AppleDouble junk macOS sprinkles on the FAT volume.
        let _ = Command::new("dot_clean").arg(&mount).output();
        for junk in [".fseventsd", "EFI/._freebsd", "EFI/freebsd/._loader.env"] {
            let _ = std::fs::remove_dir_all(mount.join(junk));
            let _ = std::fs::remove_file(mount.join(junk));
        }
        Ok(())
    })();

    // Always unmount.
    if let Err(e) = run(
        Command::new("diskutil").arg("unmount").arg(&mount),
        "diskutil unmount (ESP)",
    ) {
        warn!("failed to unmount ESP cleanly: {e}");
    }
    let _ = std::fs::remove_dir(&mount);

    write_result
}

/// Run a command, streaming its stdout/stderr, and error if it fails.
fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {what}"))?;
    if !status.success() {
        bail!("{what} exited with {status}");
    }
    Ok(())
}
