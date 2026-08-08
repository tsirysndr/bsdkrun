//! `nanos` subcommand: boot a Nanos (NanoVMs) unikernel image.
//!
//! Nanos implements the Linux syscall ABI, so the guest is an ordinary static
//! Linux binary wrapped into a bootable image by `ops` (https://ops.city).
//! bsdkrun boots what ops built — it does not run ops itself, mirroring how
//! the `unikraft` command boots what kraft built.
//!
//! Boot method differs per host:
//!
//!   * **Linux/x86_64** — direct kernel load: ops stages a `kernel.img` (an
//!     ELF) per Nanos release under `~/.ops/<version>/`, and the image is the
//!     root disk. This is the same boot contract Firecracker uses, which
//!     Nanos supports officially. Experimental until the `e2e-nanos`
//!     workflow is green.
//!   * **macOS/arm64** — EFI boot of the image (built with `"Uefi": true`),
//!     with `KRUN_ACPI=1` so libkrun serves the ACPI tables Nanos requires
//!     (GIC via MADT, timer via GTDT, console via SPCR, virtio via DSDT).
//!     Known **not** to reach userspace at nanos 0.1.55: the upstream UEFI
//!     loader hangs identically under QEMU + stock edk2. The command exists
//!     so it lights up when upstream fixes it — see
//!     `examples/nanos-hello/README.md` for the full diagnosis.
//!   * **Linux/aarch64** — not bootable: the Nanos kernel links at
//!     `0x4040_0000`, below libkrun's direct-kernel RAM base (`0x8000_0000`),
//!     and there is no EFI path on Linux hosts.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Where ops keeps its state.
fn ops_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ops"))
}

/// Resolve what the user pointed at to an image file: a path is taken as-is,
/// a bare name is looked up in ops' image store (`~/.ops/images/<name>`).
pub fn resolve_image(arg: &str) -> Result<PathBuf> {
    let p = PathBuf::from(arg);
    if p.is_file() {
        return Ok(p);
    }
    if !arg.contains('/') {
        if let Some(ops) = ops_dir() {
            let candidate = ops.join("images").join(arg);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!(
        "{arg} is neither an image file nor a name in ~/.ops/images — build one \
         with `ops build <elf> -i <name>` (see examples/nanos-hello)"
    );
}

/// The newest Nanos kernel ops has staged (`~/.ops/<version>/kernel.img`),
/// for the Linux direct-kernel boot. Release dirs are plain versions on
/// x86_64 (`0.1.55`) and `-arm`-suffixed on arm; only same-arch dirs are
/// considered.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Linux boot path only
pub fn default_kernel() -> Result<PathBuf> {
    let ops = ops_dir().context("HOME is not set")?;
    let want_arm = matches!(crate::host::Arch::current()?, crate::host::Arch::Aarch64);
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&ops)
        .with_context(|| format!("reading {} — is ops installed?", ops.display()))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        if name.ends_with("-arm") != want_arm {
            continue;
        }
        let kernel = entry.path().join("kernel.img");
        if kernel.is_file() {
            candidates.push((name, kernel));
        }
    }
    // Version-ish sort: the plain string sort is right for dotted versions of
    // equal component counts, which is what ops produces.
    candidates.sort();
    candidates.pop().map(|(_, k)| k).ok_or_else(|| {
        anyhow::anyhow!(
            "no Nanos kernel found under {} — run `ops build` once (it stages \
             the kernel), or pass --kernel",
            ops.display()
        )
    })
}

/// What a nanos machine was booted from, saved in its state dir so
/// `bsdkrun start` can boot the same thing again.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BootSpec {
    pub image: PathBuf,
    /// Linux only; None on macOS (EFI boots need no kernel).
    pub kernel: Option<PathBuf>,
    pub cmdline: String,
}

impl BootSpec {
    fn file(vdir: &Path) -> PathBuf {
        vdir.join("nanos.json")
    }

    pub fn save(&self, vdir: &Path) -> Result<()> {
        let f = Self::file(vdir);
        std::fs::write(&f, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", f.display()))
    }

    pub fn load(vdir: &Path) -> Result<Self> {
        let f = Self::file(vdir);
        let bytes = std::fs::read(&f).with_context(|| {
            format!(
                "reading {} (booted by an older bsdkrun? boot the image again \
                 with `bsdkrun nanos`)",
                f.display()
            )
        })?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_is_not_a_file_errors_with_guidance() {
        let err = resolve_image("/definitely/not/there")
            .unwrap_err()
            .to_string();
        assert!(err.contains("ops build"), "{err}");
    }
}
