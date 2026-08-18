//! `bsdkrun ci` — run tangled spindle workflows in microVMs.
//!
//! The tool itself is a Go binary (`ci/` at the repo root), embedded and
//! exec'd exactly as `pack` is — see that module for the full account of the
//! pattern, including why the extracted binary must be ad-hoc signed on
//! macOS (an entitled parent may only exec a validly signed child; unsigned
//! is SIGKILL before `main()`).
//!
//! Go rather than Rust for one decisive reason: the workflow schema and its
//! `when:` matching are imported from tangled.org/core itself, so a file
//! spindle accepts is a file `bsdkrun ci` accepts. Reimplementing someone
//! else's format here would drift within a month.
//!
//! The Go tool spins VMs through the bsdkrun Go SDK, which resolves its
//! binary from `$BSDKRUN_BIN` — set here to *this very executable*, so the
//! CI runner always drives the bsdkrun that launched it, not whatever an
//! unrelated PATH entry happens to be.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;

use crate::fetch::cache_dir;

/// The compiled `bsdkrun-ci` binary, if `core/build.rs` managed to build one.
/// Empty when the machine that ran `cargo build` had no Go toolchain.
#[derive(RustEmbed)]
#[folder = "src/ci-bin"]
struct CiBinary;

const BINARY_NAME: &str = "bsdkrun-ci";

/// Extract the embedded binary (if any), returning the path to it.
pub(crate) fn ci_binary() -> Result<std::path::PathBuf> {
    let embedded = CiBinary::get(BINARY_NAME).filter(|f| !f.data.is_empty());
    let Some(embedded) = embedded else {
        bail!(
            "this bsdkrun binary was built without ci support (no Go toolchain on the \
             machine that ran `cargo build`).\n\
             Install Go >= 1.25 and run `cargo build --release` again to enable `bsdkrun ci`."
        );
    };

    let dir = cache_dir()?.join("ci").join(crate::VERSION);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let bin = dir.join(BINARY_NAME);
    std::fs::write(&bin, &embedded.data).with_context(|| format!("writing {}", bin.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod +x {}", bin.display()))?;
    }
    #[cfg(target_os = "macos")]
    sign_adhoc(&bin)?;

    Ok(bin)
}

/// Extract and exec with `args`, replacing this process. Never returns on
/// success.
pub(crate) fn cmd_ci(args: &[String]) -> Result<()> {
    let bin = ci_binary()?;
    let this = std::env::current_exe().context("resolving the bsdkrun binary path")?;
    let err = Command::new(&bin)
        .args(args)
        // The SDK inside the Go tool boots VMs by shelling out to bsdkrun;
        // pinning it to this exact binary keeps a dev build driving the dev
        // build and a release driving the release.
        .env("BSDKRUN_BIN", this)
        .exec();
    Err(err).with_context(|| format!("running {}", bin.display()))
}

/// See `pack::sign_adhoc` — the same trap, the same fix.
#[cfg(target_os = "macos")]
fn sign_adhoc(bin: &std::path::Path) -> Result<()> {
    let out = Command::new("codesign")
        .args(["--force", "-s", "-"])
        .arg(bin)
        .output()
        .with_context(|| format!("running codesign on {}", bin.display()))?;
    if !out.status.success() {
        bail!(
            "codesign {} failed: {}",
            bin.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}
