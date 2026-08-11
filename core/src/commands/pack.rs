//! `bsdkrun pack` — detect a project's language, build it with BuildKit, and
//! generate a Kraftfile so `kraft build` (and then `bsdkrun unikraft .`) can
//! turn it into a bootable unikernel.
//!
//! The tool itself is a Go binary (`pack/` at the repo root): `core/build.rs`
//! compiles it for the host triple and embeds it here with `rust_embed`. This
//! command extracts it once and `exec`s it, inheriting stdio so its (bubbletea)
//! TUI owns the terminal directly, forwarding every argument after `pack`
//! untouched — `bsdkrun pack` has no flag surface of its own to keep in sync
//! with the Go binary's.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;

use crate::fetch::cache_dir;

/// The compiled `bsdkrun-pack` binary, if `core/build.rs` managed to build one
/// (it needs `go` on the machine that ran `cargo build`). Empty when it
/// didn't — see `build.rs::ensure_pack_binary`.
#[derive(RustEmbed)]
#[folder = "src/pack-bin"]
struct PackBinary;

const BINARY_NAME: &str = "bsdkrun-pack";

/// Extract the embedded binary (if any) and exec it with `args`, replacing
/// this process. Never returns on success.
pub(crate) fn cmd_pack(args: &[String]) -> Result<()> {
    let embedded = PackBinary::get(BINARY_NAME).filter(|f| !f.data.is_empty());
    let Some(embedded) = embedded else {
        bail!(
            "this bsdkrun binary was built without pack support (no Go toolchain on the \
             machine that ran `cargo build`).\n\
             Install Go >= 1.22 and run `cargo build --release` again to enable `bsdkrun pack`."
        );
    };

    let dir = cache_dir()?.join("pack").join(crate::VERSION);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let bin = dir.join(BINARY_NAME);
    // Always rewrite: the embedded bytes come from this exact binary, and a
    // memcpy + write is cheap enough that there is no staleness window worth
    // guarding against (unlike agent.rs's downloads, there's no network cost
    // to avoid here).
    std::fs::write(&bin, &embedded.data).with_context(|| format!("writing {}", bin.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod +x {}", bin.display()))?;
    }
    #[cfg(target_os = "macos")]
    sign_adhoc(&bin)?;

    let err = Command::new(&bin).args(args).exec();
    // exec() only returns on failure.
    Err(err).with_context(|| format!("running {}", bin.display()))
}

/// Ad-hoc codesign a freshly-extracted binary before `exec`ing into it.
///
/// `exec()` (`execve`) replaces this process's image in place rather than
/// forking — so once `bsdkrun` itself carries the hypervisor entitlement
/// (`make sign-release`; see the top-level README's "Hypervisor entitlement"
/// section), macOS's AMFI/hardened-runtime enforcement requires whatever it
/// execs into to also carry a valid signature. A plain `go build` output has
/// none ("no CMS blob"), and the kernel SIGKILLs it (exit 137) before main()
/// ever runs — silently and without a crash report pointing at the real
/// cause. `bsdkrun-pack` needs no entitlements of its own (it never touches
/// libkrun), so an ad-hoc signature is sufficient.
#[cfg(target_os = "macos")]
fn sign_adhoc(bin: &std::path::Path) -> Result<()> {
    let status = Command::new("codesign")
        .args(["--force", "-s", "-"])
        .arg(bin)
        .status()
        .with_context(|| format!("running codesign on {}", bin.display()))?;
    if !status.success() {
        bail!("codesign {} failed: {status}", bin.display());
    }
    Ok(())
}
