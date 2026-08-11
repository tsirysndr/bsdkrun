//! Solo5 unikernels: finding one, and reading what it declares.
//!
//! A Solo5 unikernel (MirageOS is the best-known producer of them) is an ELF
//! binary run by a *tender* — `solo5-hvt`, which bsdkrun embeds and extracts.
//! Unlike every other guest here, the tender is the hypervisor front end: it
//! drives Hypervisor.framework or KVM in its own process, and libkrun is not
//! involved at all. See [`crate::commands::solo5`] for the run side.
//!
//! What this module does is read the two ELF notes every Solo5 binary carries:
//!
//!   * `ABI1` — which tender the binary was built for. Catching an `spt` or
//!     `virtio` unikernel here turns a baffling "Invalid ELF program header"
//!     from the tender into a sentence naming the actual mismatch.
//!   * `MFT1` — the *manifest*: the block and network devices the unikernel
//!     declares, by name. The tender requires that every declared device be
//!     attached on its command line and refuses to boot otherwise, so reading
//!     the manifest is what lets `bsdkrun solo5 unikernel.hvt` work with no
//!     device flags at all — the names come from the binary rather than from
//!     the user.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The devices a unikernel declares, in manifest order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Names of declared `MFT_DEV_NET_BASIC` devices. MirageOS's default is
    /// `service`.
    pub nets: Vec<String>,
    /// Names of declared `MFT_DEV_BLOCK_BASIC` devices.
    pub blocks: Vec<String>,
}

/// `MFT1`, little-endian — the manifest note's type.
const MFT1_NOTE_TYPE: u32 = 0x3154_464d;
/// `ABI1` — the ABI note's type.
const ABI1_NOTE_TYPE: u32 = 0x3149_4241;
/// Both notes carry this name (with its NUL, so `n_namesz` is 6).
const SOLO5_NOTE_NAME: &[u8] = b"Solo5\0";

/// `struct mft_entry`: `char name[68]`, `mft_type_t type`, then two unions and
/// a bool, padded out to an 8-byte boundary. Confirmed against the compiler
/// rather than assumed — the trailing padding is easy to get wrong.
const MFT_ENTRY_SIZE: usize = 104;
const MFT_NAME_SIZE: usize = 68;
/// `struct mft`: `version`, `entries`, then the entries themselves.
const MFT_ENTRIES_OFFSET: usize = 8;
/// `struct mft` is 8-aligned inside the note, but `struct mft1_nhdr` is only
/// 20 bytes — so the manifest starts 4 bytes *past* where the ELF note
/// descriptor does. This is what `MFT1_NOTE_ALIGN` exists to express; reading
/// the descriptor directly gets a manifest shifted by one `uint32_t`, which
/// parses as a plausible-looking but wrong device list.
const MFT_NOTE_PAD: usize = 4;

/// Device types, from `enum mft_type`.
const MFT_DEV_BLOCK_BASIC: u32 = 1;
const MFT_DEV_NET_BASIC: u32 = 2;
/// Entries at or above this are reserved and not attachable — a Solo5 binary
/// carries one such entry even when it declares no devices at all, so treating
/// it as a device would demand an impossible attachment.
const MFT_RESERVED_FIRST: u32 = 1 << 30;

/// `enum abi_target`. Only the hvt tender is embedded.
const HVT_ABI_TARGET: u32 = 1;

/// Resolve what to boot from a path that may be the unikernel itself or a
/// project directory.
///
/// `mirage build` leaves its output in `dist/`, so a bare `bsdkrun solo5` in a
/// MirageOS project finds `dist/<name>.hvt` without being told where to look.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }

    // `dist/` first: a project that has been built has its unikernel there,
    // and any stray `.hvt` in the project root is likely to be older.
    for dir in [path.join("dist"), path.to_path_buf()] {
        if let Some(found) = newest_hvt(&dir)? {
            return Ok(found);
        }
    }
    bail!(
        "no Solo5 unikernel (*.hvt) found in {} or {}/dist — build one first \
         (for MirageOS: `mirage configure -t hvt && make`)",
        path.display(),
        path.display()
    )
}

/// The most recently modified `*.hvt` in `dir`, if any. A directory that does
/// not exist is simply "none" — callers try several.
fn newest_hvt(dir: &Path) -> Result<Option<PathBuf>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("hvt") || !path.is_file() {
            continue;
        }
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    Ok(best.map(|(_, p)| p))
}

/// Read `path`'s manifest, checking first that it is an hvt unikernel at all.
pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading Solo5 unikernel {}", path.display()))?;
    if !crate::elf::is_elf(&bytes) {
        bail!(
            "{} is not an ELF binary, so it is not a Solo5 unikernel",
            path.display()
        );
    }

    match note_desc(&bytes, ABI1_NOTE_TYPE) {
        // The ABI note is `abi_target` then `abi_version`, both u32.
        Some(desc) if desc.len() >= 4 => {
            let target = u32::from_le_bytes(desc[..4].try_into().unwrap());
            if target != HVT_ABI_TARGET {
                bail!(
                    "{} is a Solo5 unikernel built for the {} tender, not hvt — \
                     rebuild it with `-t hvt` (MirageOS: `mirage configure -t hvt`)",
                    path.display(),
                    abi_target_name(target)
                );
            }
        }
        // No ABI note at all: an ELF, but not one Solo5 produced.
        _ => bail!(
            "{} carries no Solo5 ABI note — it is an ELF binary, but not a Solo5 unikernel",
            path.display()
        ),
    }

    let Some(desc) = note_desc(&bytes, MFT1_NOTE_TYPE) else {
        // Every Solo5 binary has one; if this one doesn't, the safe reading is
        // "declares nothing", and the tender will say so if that's wrong.
        tracing::warn!(
            "{} has no Solo5 manifest note; assuming it declares no devices",
            path.display()
        );
        return Ok(Manifest::default());
    };
    Ok(parse_manifest(desc))
}

/// Parse a `MFT1` note descriptor into the device names it declares.
///
/// Anything that doesn't line up is treated as "declares nothing" rather than
/// as an error: a future manifest version would otherwise make every unikernel
/// unbootable here, when the tender itself is the authority and will reject a
/// missing device with a precise message of its own.
fn parse_manifest(desc: &[u8]) -> Manifest {
    let mut mft = Manifest::default();
    if desc.len() < MFT_NOTE_PAD + MFT_ENTRIES_OFFSET {
        return mft;
    }
    let m = &desc[MFT_NOTE_PAD..];
    let version = u32::from_le_bytes(m[..4].try_into().unwrap());
    let entries = u32::from_le_bytes(m[4..8].try_into().unwrap()) as usize;
    // MFT_VERSION 1, MFT_MAX_ENTRIES 64.
    if version != 1 || entries > 64 {
        tracing::warn!("unrecognised Solo5 manifest (version {version}, {entries} entries)");
        return mft;
    }

    for i in 0..entries {
        let off = MFT_ENTRIES_OFFSET + i * MFT_ENTRY_SIZE;
        let Some(entry) = m.get(off..off + MFT_ENTRY_SIZE) else {
            tracing::warn!("Solo5 manifest is shorter than its {entries} entries claim");
            break;
        };
        let kind = u32::from_le_bytes(entry[MFT_NAME_SIZE..MFT_NAME_SIZE + 4].try_into().unwrap());
        // Every binary carries a reserved entry, device-less ones included.
        if kind >= MFT_RESERVED_FIRST {
            continue;
        }
        let name = entry[..MFT_NAME_SIZE]
            .split(|b| *b == 0)
            .next()
            .unwrap_or_default();
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        match kind {
            MFT_DEV_NET_BASIC => mft.nets.push(name.to_string()),
            MFT_DEV_BLOCK_BASIC => mft.blocks.push(name.to_string()),
            _ => tracing::warn!("ignoring Solo5 manifest device {name} of unknown type {kind}"),
        }
    }
    mft
}

fn abi_target_name(target: u32) -> &'static str {
    match target {
        1 => "hvt",
        2 => "spt",
        3 => "virtio",
        4 => "muen",
        5 => "genode",
        6 => "xen",
        _ => "unknown",
    }
}

/// Find the descriptor of the first `Solo5` ELF note of `want_type`.
///
/// Walks `PT_NOTE` segments by hand rather than sections: a note is reachable
/// from both, but the program headers are what the tender itself reads, so
/// this agrees with it on a stripped binary.
fn note_desc(elf: &[u8], want_type: u32) -> Option<&[u8]> {
    if elf.len() < 64 || elf[4] != 2 || elf[5] != 1 {
        return None; // not ELF64 little-endian
    }
    let u16at = |off: usize| -> Option<u16> {
        Some(u16::from_le_bytes(elf.get(off..off + 2)?.try_into().ok()?))
    };
    let u32at = |off: usize| -> Option<u32> {
        Some(u32::from_le_bytes(elf.get(off..off + 4)?.try_into().ok()?))
    };
    let u64at = |off: usize| -> Option<u64> {
        Some(u64::from_le_bytes(elf.get(off..off + 8)?.try_into().ok()?))
    };

    let e_phoff = u64at(0x20)? as usize;
    let e_phentsize = u16at(0x36)? as usize;
    let e_phnum = u16at(0x38)? as usize;
    if e_phentsize < 56 {
        return None;
    }

    for i in 0..e_phnum {
        let ph = e_phoff.checked_add(i.checked_mul(e_phentsize)?)?;
        if u32at(ph)? != 4 {
            continue; // not PT_NOTE
        }
        let offset = u64at(ph + 0x08)? as usize;
        let filesz = u64at(ph + 0x20)? as usize;
        let seg = elf.get(offset..offset.checked_add(filesz)?)?;

        // Notes are packed back to back: a 12-byte header, the name padded to
        // 4 bytes, then the descriptor padded to 4.
        let mut p = 0usize;
        while p + 12 <= seg.len() {
            let namesz = u32::from_le_bytes(seg.get(p..p + 4)?.try_into().ok()?) as usize;
            let descsz = u32::from_le_bytes(seg.get(p + 4..p + 8)?.try_into().ok()?) as usize;
            let ntype = u32::from_le_bytes(seg.get(p + 8..p + 12)?.try_into().ok()?);
            let name = seg.get(p + 12..p + 12 + namesz)?;
            let desc_at = p + 12 + namesz.next_multiple_of(4);
            let desc = seg.get(desc_at..desc_at + descsz)?;
            if ntype == want_type && name == SOLO5_NOTE_NAME {
                return Some(desc);
            }
            p = desc_at + descsz.next_multiple_of(4);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `MFT1` descriptor the way the linker lays one out, so the
    /// parser is tested against the real shape — including the four bytes of
    /// padding that separate the ELF descriptor from `struct mft`.
    fn mft_desc(entries: &[(&str, u32)]) -> Vec<u8> {
        let mut desc = vec![0u8; MFT_NOTE_PAD];
        desc.extend_from_slice(&1u32.to_le_bytes()); // version
        desc.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (name, kind) in entries {
            let mut entry = vec![0u8; MFT_ENTRY_SIZE];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            entry[MFT_NAME_SIZE..MFT_NAME_SIZE + 4].copy_from_slice(&kind.to_le_bytes());
            desc.extend_from_slice(&entry);
        }
        desc
    }

    #[test]
    fn reads_declared_devices_in_order() {
        let desc = mft_desc(&[
            ("service", MFT_DEV_NET_BASIC),
            ("storage", MFT_DEV_BLOCK_BASIC),
            ("mgmt", MFT_DEV_NET_BASIC),
        ]);
        assert_eq!(
            parse_manifest(&desc),
            Manifest {
                nets: vec!["service".into(), "mgmt".into()],
                blocks: vec!["storage".into()],
            }
        );
    }

    /// Every Solo5 binary carries a reserved entry, so a unikernel that
    /// declares no devices still reports `entries = 1`. Attaching anything for
    /// it would be a device the tender never asked for.
    #[test]
    fn ignores_the_reserved_entry() {
        let desc = mft_desc(&[("", MFT_RESERVED_FIRST)]);
        assert_eq!(parse_manifest(&desc), Manifest::default());
    }

    /// A manifest this code does not understand must not make the unikernel
    /// unbootable — the tender is the authority on its own devices.
    #[test]
    fn unknown_version_declares_nothing() {
        let mut desc = mft_desc(&[("service", MFT_DEV_NET_BASIC)]);
        desc[MFT_NOTE_PAD..MFT_NOTE_PAD + 4].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(parse_manifest(&desc), Manifest::default());
    }

    /// A truncated note yields what it does hold rather than panicking.
    #[test]
    fn truncated_manifest_is_not_fatal() {
        let mut desc = mft_desc(&[
            ("service", MFT_DEV_NET_BASIC),
            ("storage", MFT_DEV_BLOCK_BASIC),
        ]);
        desc.truncate(desc.len() - 20);
        assert_eq!(
            parse_manifest(&desc),
            Manifest {
                nets: vec!["service".into()],
                blocks: vec![],
            }
        );
    }
}
