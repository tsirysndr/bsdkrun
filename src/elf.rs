//! Flatten an aarch64 `vmlinux` ELF into a raw kernel `Image`.
//!
//! libkrun's aarch64 loader boots the raw arm64 `Image` format (a flat binary
//! with the `ARM\x64` header), not a `vmlinux` ELF — handing it an ELF fails
//! with `KernelFormatUnsupported`. The kernel build produces `Image` via
//! `objcopy -O binary vmlinux Image`; we do the same thing here in pure Rust
//! (no binutils dependency, and unlike `llvm-objcopy` it doesn't trip over the
//! kernel's `.rela.dyn`): lay each `PT_LOAD` segment's file bytes at its
//! physical address, relative to the lowest loaded address.

use anyhow::{bail, Context, Result};

/// Convert ELF64 (little-endian) bytes into a raw load image. Returns the image
/// bytes ready to pass to libkrun as `KRUN_KERNEL_FORMAT_RAW`.
pub fn elf_to_image(elf: &[u8]) -> Result<Vec<u8>> {
    if elf.len() < 64 || &elf[..4] != b"\x7fELF" {
        bail!("not an ELF file");
    }
    if elf[4] != 2 {
        bail!("not a 64-bit ELF (only ELF64 kernels are supported)");
    }
    if elf[5] != 1 {
        bail!("not a little-endian ELF");
    }

    let u16at = |off: usize| u16::from_le_bytes(elf[off..off + 2].try_into().unwrap());
    let u64at = |off: usize| u64::from_le_bytes(elf[off..off + 8].try_into().unwrap());

    let e_phoff = u64at(0x20) as usize;
    let e_phentsize = u16at(0x36) as usize;
    let e_phnum = u16at(0x38) as usize;
    if e_phentsize < 56 {
        bail!("unexpected ELF program-header size {e_phentsize}");
    }

    // Collect PT_LOAD segments that actually carry file bytes.
    struct Load {
        paddr: u64,
        offset: usize,
        filesz: usize,
    }
    let mut loads = Vec::new();
    for i in 0..e_phnum {
        let ph = e_phoff + i * e_phentsize;
        if ph + 56 > elf.len() {
            bail!("truncated ELF program headers");
        }
        let p_type = u32::from_le_bytes(elf[ph..ph + 4].try_into().unwrap());
        if p_type != 1 {
            continue; // not PT_LOAD
        }
        let offset = u64at(ph + 0x08) as usize;
        let paddr = u64at(ph + 0x18);
        let filesz = u64at(ph + 0x20) as usize;
        if filesz == 0 {
            continue; // pure .bss / note — nothing to place
        }
        if offset + filesz > elf.len() {
            bail!("ELF segment extends past end of file");
        }
        loads.push(Load {
            paddr,
            offset,
            filesz,
        });
    }
    if loads.is_empty() {
        bail!("ELF has no loadable segments");
    }

    let base = loads.iter().map(|l| l.paddr).min().unwrap();
    let end = loads
        .iter()
        .map(|l| l.paddr + l.filesz as u64)
        .max()
        .unwrap();
    let size = (end - base) as usize;

    let mut image = vec![0u8; size];
    for l in &loads {
        let dst = (l.paddr - base) as usize;
        image[dst..dst + l.filesz].copy_from_slice(&elf[l.offset..l.offset + l.filesz]);
    }

    // Sanity: a valid arm64 Image carries "ARM\x64" at offset 0x38.
    if image.len() < 0x40 || &image[0x38..0x3c] != b"ARM\x64" {
        bail!("flattened image lacks the arm64 boot magic — is this an aarch64 kernel?");
    }
    Ok(image)
}

/// Whether `bytes` looks like an ELF that needs flattening.
pub fn is_elf(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[..4] == b"\x7fELF"
}

/// Whether `bytes` is already a raw arm64 `Image` (magic at 0x38).
pub fn is_arm64_image(bytes: &[u8]) -> bool {
    bytes.len() >= 0x3c && &bytes[0x38..0x3c] == b"ARM\x64"
}

/// Load `path`, and if it's a vmlinux ELF, return its flattened arm64 image;
/// if it's already a raw Image, return it unchanged.
pub fn read_as_image(path: &std::path::Path) -> Result<Vec<u8>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading kernel {}", path.display()))?;
    if is_elf(&bytes) {
        elf_to_image(&bytes)
            .with_context(|| format!("converting {} to a raw Image", path.display()))
    } else if is_arm64_image(&bytes) {
        Ok(bytes)
    } else {
        bail!(
            "{} is neither an ELF vmlinux nor a raw arm64 Image",
            path.display()
        )
    }
}
