//! `unikraft` subcommand: boot a Unikraft unikernel as a microVM.
//!
//! Unikraft images built for Firecracker (`kraft build --plat fc`) are exactly
//! what libkrun wants — the `fc` platform links at `0x8000_0000`, which is where
//! libkrun's aarch64 loader puts a raw image, and boots via the Linux boot
//! protocol (`x0` = device tree), which is what libkrun sets up. Two details
//! still need handling on the host side:
//!
//!   * **`text_offset`.** libkrun writes a raw image at `0x8000_0000` and enters
//!     it there, ignoring the arm64 `Image` header. Unikraft reserves the first
//!     megabyte of RAM for the DTB, so its header asks to be loaded at
//!     `0x8000_0000 + 0xf_ffc0` and it is *not* relocatable (no `LIBUKRELOC`) —
//!     entering at `0x8000_0000` runs into the reserved hole. [`prepare`] shims
//!     this: it front-pads the image by `text_offset` so every byte lands at its
//!     link address, and writes a single `b` instruction at offset 0 so libkrun's
//!     fixed entry jumps to the real one (a branch preserves `x0`).
//!   * **Console.** libkrun's aarch64 console is a PL011, Firecracker's is an
//!     ns16550, so a stock `fc/arm64` build boots silently. Build with
//!     `CONFIG_LIBPL011=y` + `CONFIG_LIBPL011_EARLY_CONSOLE=y` — both drivers
//!     probe the device tree, so enabling PL011 alongside the default costs
//!     nothing and makes one image boot under either VMM.
//!
//! On x86_64 the `fc` build is an ELF that libkrun's ELF loader boots directly
//! via the same Linux protocol, so no shim is needed.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::fetch::cache_dir;

/// Where libkrun's aarch64 loader writes a raw kernel image, and enters it.
const AARCH64_LOAD_ADDR: u64 = 0x8000_0000;

/// Range of an AArch64 `b` (26-bit signed word offset): ±128 MiB.
const MAX_BRANCH: u64 = 128 << 20;

/// A unikernel image ready to hand to `krun_set_kernel`.
pub struct Prepared {
    /// The image to boot — the input itself, or a shimmed copy in the cache.
    pub path: PathBuf,
    /// `KRUN_KERNEL_FORMAT_*` for [`Self::path`].
    pub format: u32,
}

/// Prepare `kernel` for booting: on aarch64, flatten an ELF to a raw `Image` if
/// needed and apply the `text_offset` shim (see the module docs); on x86_64,
/// hand the ELF to libkrun's loader as-is.
pub fn prepare(kernel: &Path) -> Result<Prepared> {
    if matches!(crate::host::Arch::current()?, crate::host::Arch::X86_64) {
        let bytes = std::fs::read(kernel)
            .with_context(|| format!("reading unikernel {}", kernel.display()))?;
        if !crate::elf::is_elf(&bytes) {
            bail!(
                "{} is not an ELF — an x86_64 Unikraft image is the ELF that \
                 `kraft build --plat fc --arch x86_64` writes to .unikraft/build/",
                kernel.display()
            );
        }
        return Ok(Prepared {
            path: kernel.to_path_buf(),
            format: crate::krun::KRUN_KERNEL_FORMAT_ELF,
        });
    }

    // aarch64: `read_as_image` takes either the raw `Image` or the `.dbg` ELF
    // beside it (same bytes once flattened) and gives us a raw image.
    let image = crate::elf::read_as_image(kernel)?;
    let text_offset = u64::from_le_bytes(image[8..16].try_into().unwrap());

    if text_offset == 0 {
        // Links at the load address already (a stock Linux Image, or a Unikraft
        // build with no reserved hole) — nothing to shim.
        return Ok(Prepared {
            path: kernel.to_path_buf(),
            format: crate::krun::KRUN_KERNEL_FORMAT_RAW,
        });
    }
    if text_offset % 4 != 0 {
        bail!("unikernel text_offset {text_offset:#x} is not instruction-aligned");
    }
    if text_offset >= MAX_BRANCH {
        bail!(
            "unikernel wants to load {text_offset:#x} past {AARCH64_LOAD_ADDR:#x}, which is \
             out of range of the entry branch bsdkrun writes ({MAX_BRANCH:#x})"
        );
    }

    let mut shimmed = vec![0u8; text_offset as usize];
    // b <text_offset>: imm26 is the offset in words, and the branch is the
    // first thing libkrun's fixed entry point executes.
    let branch = 0x1400_0000u32 | ((text_offset / 4) as u32 & 0x03FF_FFFF);
    shimmed[..4].copy_from_slice(&branch.to_le_bytes());
    shimmed.extend_from_slice(&image);
    debug!(
        text_offset,
        branch = format!("{branch:#010x}"),
        "shimming unikernel entry"
    );

    let out = cache_dir()?.join("unikraft").join(format!(
        "{}-{:016x}.img",
        kernel
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unikernel".into()),
        fnv1a(&shimmed),
    ));
    if !out.exists() {
        std::fs::create_dir_all(out.parent().unwrap())
            .with_context(|| format!("creating {}", out.parent().unwrap().display()))?;
        // Write-then-rename: two `bsdkrun unikraft` runs racing on the same
        // image must never see a half-written one.
        let tmp = out.with_extension("img.tmp");
        std::fs::write(&tmp, &shimmed).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &out).with_context(|| format!("renaming to {}", out.display()))?;
    }
    Ok(Prepared {
        path: out,
        format: crate::krun::KRUN_KERNEL_FORMAT_RAW,
    })
}

/// FNV-1a, to name a shimmed image after its contents (so a rebuilt unikernel
/// gets a fresh cache entry instead of silently booting the previous one).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Resolve what the user pointed at to a unikernel image: a file is taken as-is,
/// a `kraft` project directory is searched for the image its build produced.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        bail!("{} does not exist", path.display());
    }
    let build = path.join(".unikraft").join("build");
    if !build.is_dir() {
        bail!(
            "{} has no .unikraft/build — build the unikernel first, e.g. \
             `kraft build --plat fc --arch {}`",
            path.display(),
            crate::host::Arch::current()?.uk_slug(),
        );
    }
    let want = format!("_fc-{}", crate::host::Arch::current()?.uk_slug());
    // The build dir holds the image plus its debug/aux siblings (`.dbg`, `.cmd`,
    // `.bootinfo`, …); the extension-less name is the bootable one.
    let mut found: Vec<PathBuf> = std::fs::read_dir(&build)
        .with_context(|| format!("reading {}", build.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_none()
                && p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(&want))
                    .unwrap_or(false)
        })
        .collect();
    found.sort();
    match found.len() {
        0 => bail!(
            "no {} image in {} — build one with `kraft build --plat fc --arch {}`",
            want.trim_start_matches('_'),
            build.display(),
            crate::host::Arch::current()?.uk_slug(),
        ),
        1 => Ok(found.remove(0)),
        _ => bail!(
            "{} holds several unikernels ({}); pass the one to boot directly",
            build.display(),
            found
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
}

/// What a unikraft machine was booted from, saved in its state dir so
/// `bsdkrun start` can boot the same image again (nothing else records it: the
/// DB row only carries the image's display name).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BootSpec {
    pub kernel: PathBuf,
    pub cmdline: String,
    pub initramfs: Option<PathBuf>,
}

impl BootSpec {
    fn file(vdir: &Path) -> PathBuf {
        vdir.join("unikraft.json")
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
                "reading {} (booted by an older \
                 bsdkrun? boot the unikernel again with `bsdkrun unikraft`)",
                f.display()
            )
        })?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}
