//! `osv` subcommand: boot an [OSv](https://github.com/cloudius-systems/osv)
//! unikernel image.
//!
//! OSv is a POSIX-ish unikernel: the guest is an unmodified Linux shared object
//! that OSv links against its own libc at runtime. bsdkrun boots what OSv (or
//! `capstan`) built — it does not build images itself, mirroring how `unikraft`
//! and `nanos` boot what `kraft`/`ops` produced.
//!
//! # Why this is a separate command and not `bsdkrun kernel`
//!
//! OSv/aarch64 drives a **PL011** for its console, so it needs libkrun's
//! explicit serial rather than the implicit virtio-console `bsdkrun kernel`
//! wires up for the BSDs. Booting an OSv image through `kernel` produces a
//! silent VM that looks hung but is actually running fine with nowhere to
//! write.
//!
//! # Boot method
//!
//! The two architectures OSv supports boot by completely different routes, so
//! [`Image::probe`] sniffs which one it was handed rather than trusting a flag.
//!
//! ## x86_64 — PVH
//!
//! OSv's x86_64 loader is an ELF carrying the Xen `PHYS32_ENTRY` note, i.e. it
//! boots via the [x86/HVM direct boot ABI](https://xenbits.xen.org/docs/unstable/misc/pvh.html)
//! — the same ABI NetBSD's `MICROVM` and FreeBSD's `FIRECRACKER` kernels use,
//! and the reason this repo carries a PVH-enabled libkrun fork. We hand libkrun
//! the ELF and set `KRUN_PVH=1`; the loader honours the note and enters in
//! 32-bit protected mode. There is no filesystem inside the ELF, so an
//! application image has to be attached separately as a disk.
//!
//! ## aarch64 — raw arm64 Image
//!
//! OSv's aarch64 `loader.img` is already a raw arm64 `Image` — the format
//! libkrun's `KRUN_KERNEL_FORMAT_RAW` loader expects — so nothing has to be
//! reshaped the way [`crate::elf`] reshapes a `vmlinux`. The layout, from
//! OSv's `arch/aarch64/preboot.S` and the `loader.img` rule in its Makefile:
//!
//! ```text
//!   0x00000  preboot, starting with the Linux arm64 Image header
//!   0x00010  image_size (patched in by OSv's scripts/imgedit.py)
//!   0x00038  "ARM\x64" magic
//!   0x00200  the kernel command line, NUL-terminated
//!   0x10000  loader-stripped.elf
//! ```
//!
//! libkrun writes that blob into guest RAM, enters it with the FDT address in
//! `x0`, and OSv's `prestart` — which is entirely PC-relative — parses the
//! appended ELF and jumps into it.
//!
//! The catch is the `text_offset` of `0x80000` in the header: libkrun enters a
//! raw image at the load address and ignores that field, so OSv would run at the
//! very base of RAM instead of `base + 0x80000`, collide with the memory it is
//! about to hand out, and die before it can say anything. Booting one through
//! `bsdkrun kernel` looks exactly like a hang. [`crate::elf::shim_text_offset`]
//! front-pads the image to put every byte at its linked address; the same shim
//! the `unikraft` path needs, for the same reason.
//!
//! # GIC version (aarch64 only)
//!
//! OSv only grew a GICv3 driver after its v0.57.0 release, so the released
//! aarch64 kernel aborts with "failed to get GICv2 information from dtb" against
//! libkrun's default GICv3. We therefore ask libkrun for a GICv2 by default
//! (`KRUN_GIC=v2`); `--gic v3` selects the GICv3 for a kernel built from OSv
//! master, which prefers it. x86_64 has no GIC and ignores this entirely.
//!
//! # Images with a filesystem
//!
//! A `capstan`-composed image is the loader followed by a ZFS or ROFS
//! partition, so the same file is both the kernel and the root disk. We slice
//! the leading `image_size` bytes out into the machine's state dir to hand
//! libkrun as the kernel — libkrun reads the whole kernel file into guest RAM,
//! so handing it a multi-gigabyte disk image would balloon the VM — and attach
//! the file itself as virtio-blk.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Offset of the 64-bit `image_size` field in the arm64 Image header.
const IMAGE_SIZE_OFFSET: usize = 0x10;
/// Offset of the `ARM\x64` magic in the arm64 Image header.
const MAGIC_OFFSET: usize = 0x38;
const ARM64_MAGIC: &[u8; 4] = b"ARM\x64";
/// Offset of OSv's command line within its `loader.img`, per `preboot.S`.
/// capstan writes the command line here too (`util.SetCmdLine`).
const CMDLINE_OFFSET: usize = 0x200;
/// Longest command line we will read back out of an image.
const CMDLINE_MAX: usize = 0x400 - CMDLINE_OFFSET;
/// Enough of the image to cover the whole header plus the command line.
const HEADER_LEN: usize = 0x400;

/// Which interrupt controller to ask libkrun for.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Gic {
    /// The default: OSv only grew a GICv3 driver after v0.57.0.
    #[default]
    V2,
    V3,
}

impl Gic {
    /// The value libkrun's `KRUN_GIC` reads.
    pub fn krun_value(self) -> &'static str {
        match self {
            Gic::V2 => "v2",
            Gic::V3 => "v3",
        }
    }
}

impl std::str::FromStr for Gic {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "v2" | "2" => Ok(Gic::V2),
            "v3" | "3" => Ok(Gic::V3),
            other => bail!("unknown GIC version {other} (expected v2 or v3)"),
        }
    }
}

/// The parts of an OSv image bsdkrun cares about.
#[derive(Debug, PartialEq, Eq)]
pub struct ImageHeader {
    /// Bytes of kernel, from the start of the image. Covers BSS, so it can
    /// exceed the file length.
    pub image_size: u64,
    /// The command line baked into the image, if there is one.
    pub cmdline: Option<String>,
}

/// Parse the arm64 Image header at the front of an OSv image.
pub fn parse_header(bytes: &[u8]) -> Result<ImageHeader> {
    if bytes.len() < MAGIC_OFFSET + 4 {
        bail!("too short to be an OSv image ({} bytes)", bytes.len());
    }
    if &bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4] != ARM64_MAGIC {
        bail!(
            "no arm64 Image magic at 0x{MAGIC_OFFSET:x} — this is not an OSv \
             aarch64 loader.img. An OSv x86_64 image cannot boot here: a \
             hardware-virtualized guest runs the host's architecture, so on \
             Apple Silicon you need the aarch64 build (e.g. \
             osv-loader-microvm.qemu.aarch64)"
        );
    }
    let image_size = u64::from_le_bytes(
        bytes[IMAGE_SIZE_OFFSET..IMAGE_SIZE_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    if image_size == 0 {
        bail!(
            "the image header declares a zero image_size — OSv's build patches \
             this field in via scripts/imgedit.py, so this image looks unfinished"
        );
    }

    // The command line sits at a fixed offset and is NUL-terminated.
    let cmdline = bytes
        .get(CMDLINE_OFFSET..)
        .map(|tail| &tail[..tail.len().min(CMDLINE_MAX)])
        .and_then(|tail| {
            let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
            std::str::from_utf8(&tail[..end]).ok()
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Ok(ImageHeader {
        image_size,
        cmdline,
    })
}

/// What kind of OSv image we were handed, and therefore how to boot it.
#[derive(Debug, PartialEq, Eq)]
pub enum Image {
    /// An aarch64 `loader.img`: a raw arm64 Image, possibly with a filesystem
    /// appended (a capstan-composed application image).
    Arm64 { header: ImageHeader },
    /// An x86_64 loader ELF, entered through its PVH `PHYS32_ENTRY` note. The
    /// ELF is kernel only — any filesystem comes from a separate disk.
    ElfPvh,
}

/// The first bytes of an ELF file.
const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
/// The first bytes of a QCOW2 file. capstan's repository stores composed images
/// in this format, which libkrun cannot read.
const QCOW2_MAGIC: &[u8; 4] = b"QFI\xfb";
/// `e_machine` for x86-64, at offset 0x12 of an ELF header.
const EM_X86_64: u16 = 62;

/// Decide how to boot the image at `path` from its contents. Sniffing beats a
/// flag here: an OSv release ships both flavours side by side, with names that
/// differ only in a suffix, and getting it wrong produces a silent dead VM.
pub fn probe(path: &Path) -> Result<Image> {
    let head = read_head(path, HEADER_LEN)?;
    if head.len() >= 4 && &head[..4] == QCOW2_MAGIC {
        bail!(
            "{} is a QCOW2 image, which libkrun cannot read — convert it first:\n  \
             qemu-img convert -O raw {} disk.raw\n\
             (capstan's repository stores composed images as QCOW2; `capstan run \
             -p bsdkrun` does this conversion for you)",
            path.display(),
            path.display(),
        );
    }
    if head.len() >= 4 && &head[..4] == ELF_MAGIC {
        if head.len() < 0x14 {
            bail!("{} is a truncated ELF", path.display());
        }
        let machine = u16::from_le_bytes(head[0x12..0x14].try_into().unwrap());
        if machine != EM_X86_64 {
            bail!(
                "{} is an ELF for e_machine {machine}, which bsdkrun has no OSv \
                 boot path for — pass the x86_64 loader ELF, or the aarch64 \
                 loader.img (not its ELF)",
                path.display()
            );
        }
        if !has_pvh_note(path)? {
            bail!(
                "{} carries no Xen PHYS32_ENTRY note, so libkrun cannot enter it \
                 via PVH — is this really an OSv loader ELF?",
                path.display()
            );
        }
        return Ok(Image::ElfPvh);
    }
    Ok(Image::Arm64 {
        header: parse_header(&head)
            .with_context(|| format!("reading OSv image {}", path.display()))?,
    })
}

/// Whether an ELF advertises the Xen `PHYS32_ENTRY` note (`XEN_ELFNOTE_PHYS32_ENTRY`,
/// type 18) that libkrun's PVH path keys off.
fn has_pvh_note(path: &Path) -> Result<bool> {
    const PT_NOTE: u32 = 4;
    const XEN_ELFNOTE_PHYS32_ENTRY: u32 = 18;

    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() < 64 {
        return Ok(false);
    }
    let u16at = |off: usize| u16::from_le_bytes(bytes[off..off + 2].try_into().unwrap());
    let u32at = |off: usize| u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
    let u64at = |off: usize| u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());

    let e_phoff = u64at(0x20) as usize;
    let e_phentsize = u16at(0x36) as usize;
    let e_phnum = u16at(0x38) as usize;
    if e_phentsize < 56 {
        return Ok(false);
    }
    for i in 0..e_phnum {
        let ph = e_phoff + i * e_phentsize;
        if ph + 56 > bytes.len() || u32at(ph) != PT_NOTE {
            continue;
        }
        let off = u64at(ph + 0x08) as usize;
        let filesz = u64at(ph + 0x20) as usize;
        let Some(notes) = bytes.get(off..off + filesz) else {
            continue;
        };
        // Each note is namesz/descsz/type, then the 4-byte-aligned name and desc.
        let mut pos = 0usize;
        while pos + 12 <= notes.len() {
            let namesz = u32::from_le_bytes(notes[pos..pos + 4].try_into().unwrap()) as usize;
            let descsz = u32::from_le_bytes(notes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let ntype = u32::from_le_bytes(notes[pos + 8..pos + 12].try_into().unwrap());
            let name_end = pos + 12 + namesz;
            if name_end > notes.len() {
                break;
            }
            let name = notes[pos + 12..name_end].strip_suffix(b"\0").unwrap_or(&[]);
            if name == b"Xen" && ntype == XEN_ELFNOTE_PHYS32_ENTRY {
                return Ok(true);
            }
            pos = name_end.next_multiple_of(4) + descsz.next_multiple_of(4);
        }
    }
    Ok(false)
}

fn read_head(path: &Path, len: usize) -> Result<Vec<u8>> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = vec![0u8; len];
    let n = read_up_to(&mut file, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn read_up_to(file: &mut impl std::io::Read, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Resolve what the user pointed at to an image file.
pub fn resolve_image(arg: &str) -> Result<PathBuf> {
    let p = PathBuf::from(arg);
    if p.is_file() {
        return Ok(p);
    }
    bail!(
        "{arg} is not a file — pass an OSv aarch64 image, either a loader \
         (e.g. osv-loader-microvm.qemu.aarch64 from an OSv release) or an \
         image composed by capstan"
    );
}

/// How many bytes of `path` are kernel. `image_size` counts BSS, which has no
/// bytes in the file, so a loader-only image reports more than it contains.
pub fn kernel_len(path: &Path, header: &ImageHeader) -> Result<u64> {
    let file_len = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .len();
    Ok(header.image_size.min(file_len))
}

/// Whether the image carries a filesystem after the kernel, i.e. whether it is
/// a composed application image rather than a bare loader.
pub fn has_filesystem(path: &Path, header: &ImageHeader) -> Result<bool> {
    let file_len = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .len();
    Ok(file_len > header.image_size)
}

/// Copy the leading kernel out of `image` into `dest`, ready for
/// `krun_set_kernel`.
///
/// Two things happen here:
///
///   * The kernel is sliced off the front. libkrun reads the entire kernel file
///     into guest RAM, so a composed image with a multi-gigabyte filesystem must
///     not be handed over whole.
///   * [`crate::elf::shim_text_offset`] is applied. OSv's header asks to be
///     loaded at `+0x80000`, and libkrun enters a raw image at the load address
///     regardless — without the shim OSv runs at the very base of RAM, where it
///     collides with the memory it is about to hand out, and dies before it can
///     say so.
pub fn extract_kernel(image: &Path, header: &ImageHeader, dest: &Path) -> Result<PathBuf> {
    let len = kernel_len(image, header)?;
    let mut src =
        std::fs::File::open(image).with_context(|| format!("opening {}", image.display()))?;
    let mut kernel = vec![0u8; len as usize];
    let n = read_up_to(&mut src, &mut kernel)?;
    kernel.truncate(n);

    let bytes = match crate::elf::shim_text_offset(&kernel)
        .with_context(|| format!("shimming {}", image.display()))?
    {
        Some(shimmed) => shimmed,
        None => kernel,
    };

    std::fs::write(dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest.to_path_buf())
}

/// The command line to hand libkrun.
///
/// OSv reads `/chosen/bootargs` out of the FDT in preference to the copy baked
/// into the image, so whatever we pass here wins. An empty `--cmdline` means
/// "keep what the image was built with", which we can only honour by echoing
/// the baked-in string back.
/// An x86_64 loader ELF has no baked-in command line, so `header` is `None`
/// there and an empty request stays empty.
pub fn effective_cmdline(requested: &str, header: Option<&ImageHeader>) -> String {
    if !requested.trim().is_empty() {
        return requested.to_string();
    }
    header.and_then(|h| h.cmdline.clone()).unwrap_or_default()
}

/// What an osv machine was booted from, saved in its state dir so
/// `bsdkrun start` can boot the same thing again.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BootSpec {
    pub image: PathBuf,
    pub cmdline: String,
    pub gic: Gic,
}

impl BootSpec {
    fn file(vdir: &Path) -> PathBuf {
        vdir.join("osv.json")
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
                 with `bsdkrun osv`)",
                f.display()
            )
        })?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic OSv image header: magic, image_size, and a command line.
    fn header_bytes(image_size: u64, cmdline: &str) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_LEN];
        b[IMAGE_SIZE_OFFSET..IMAGE_SIZE_OFFSET + 8].copy_from_slice(&image_size.to_le_bytes());
        b[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(ARM64_MAGIC);
        b[CMDLINE_OFFSET..CMDLINE_OFFSET + cmdline.len()].copy_from_slice(cmdline.as_bytes());
        b
    }

    #[test]
    fn parses_size_and_cmdline() {
        let h = parse_header(&header_bytes(0x380000, "--nomount tests/tst-hello.so")).unwrap();
        assert_eq!(h.image_size, 0x380000);
        assert_eq!(h.cmdline.as_deref(), Some("--nomount tests/tst-hello.so"));
    }

    #[test]
    fn an_image_with_no_cmdline_reports_none() {
        let h = parse_header(&header_bytes(0x1000, "")).unwrap();
        assert_eq!(h.cmdline, None);
    }

    #[test]
    fn rejects_a_non_arm64_image_with_an_actionable_message() {
        let mut b = header_bytes(0x1000, "");
        b[MAGIC_OFFSET] = b'X';
        let err = parse_header(&b).unwrap_err().to_string();
        assert!(err.contains("aarch64"), "{err}");
    }

    #[test]
    fn rejects_an_unpatched_image_size() {
        let err = parse_header(&header_bytes(0, "")).unwrap_err().to_string();
        assert!(err.contains("imgedit"), "{err}");
    }

    #[test]
    fn rejects_something_far_too_short() {
        assert!(parse_header(&[0u8; 8]).is_err());
    }

    #[test]
    fn an_explicit_cmdline_overrides_the_baked_in_one() {
        let h = parse_header(&header_bytes(0x1000, "baked")).unwrap();
        assert_eq!(effective_cmdline("explicit", Some(&h)), "explicit");
        assert_eq!(effective_cmdline("   ", Some(&h)), "baked");
        assert_eq!(effective_cmdline("", Some(&h)), "baked");
        // An x86_64 ELF has nothing baked in to fall back to.
        assert_eq!(effective_cmdline("", None), "");
        assert_eq!(effective_cmdline("explicit", None), "explicit");
    }

    #[test]
    fn gic_parses_both_spellings_and_defaults_are_distinct() {
        use std::str::FromStr;
        assert_eq!(Gic::from_str("v2").unwrap(), Gic::V2);
        assert_eq!(Gic::from_str("2").unwrap(), Gic::V2);
        assert_eq!(Gic::from_str("V3").unwrap(), Gic::V3);
        assert_eq!(Gic::from_str(" 3 ").unwrap(), Gic::V3);
        assert!(Gic::from_str("v4").is_err());
        assert_eq!(Gic::V2.krun_value(), "v2");
        assert_eq!(Gic::V3.krun_value(), "v3");
    }

    #[test]
    fn a_loader_only_image_is_not_treated_as_having_a_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("loader.img");
        // image_size counts BSS, so it exceeds the bytes actually in the file.
        let mut bytes = header_bytes(0x380000, "");
        bytes.resize(0x2000, 0);
        std::fs::write(&img, &bytes).unwrap();

        let Image::Arm64 { header: h } = probe(&img).unwrap() else {
            panic!("expected an arm64 image")
        };
        assert!(!has_filesystem(&img, &h).unwrap());
        // The kernel is clamped to what the file actually holds, so we never
        // try to read past the end of it.
        assert_eq!(kernel_len(&img, &h).unwrap(), 0x2000);
    }

    #[test]
    fn a_composed_image_yields_a_kernel_slice_and_a_disk() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("app.img");
        let kernel_size = 0x1000u64;
        let mut bytes = header_bytes(kernel_size, "/app.so");
        bytes.resize(kernel_size as usize, 0xAA);
        // A "filesystem" after the kernel.
        bytes.extend(std::iter::repeat_n(0xBB, 0x4000));
        std::fs::write(&img, &bytes).unwrap();

        let Image::Arm64 { header: h } = probe(&img).unwrap() else {
            panic!("expected an arm64 image")
        };
        assert!(has_filesystem(&img, &h).unwrap());
        assert_eq!(kernel_len(&img, &h).unwrap(), kernel_size);

        let kernel = dir.path().join("kernel.img");
        extract_kernel(&img, &h, &kernel).unwrap();
        let extracted = std::fs::read(&kernel).unwrap();
        // Exactly the kernel, and none of the filesystem behind it.
        assert_eq!(extracted.len() as u64, kernel_size);
        assert_eq!(&extracted[MAGIC_OFFSET..MAGIC_OFFSET + 4], ARM64_MAGIC);
        assert!(!extracted.contains(&0xBB));
    }

    /// A minimal x86_64 ELF with a single PT_NOTE segment, optionally carrying
    /// the Xen PHYS32_ENTRY note libkrun's PVH path looks for.
    fn elf_bytes(machine: u16, with_pvh: bool) -> Vec<u8> {
        const EHDR: usize = 64;
        const PHDR: usize = 56;
        let notes_off = EHDR + PHDR;

        // namesz=4 ("Xen\0"), descsz=8, type=18, name, desc.
        let mut notes = Vec::new();
        let ntype: u32 = if with_pvh { 18 } else { 1 };
        notes.extend(4u32.to_le_bytes());
        notes.extend(8u32.to_le_bytes());
        notes.extend(ntype.to_le_bytes());
        notes.extend(b"Xen\0");
        notes.extend(0x2c535eu64.to_le_bytes());

        let mut b = vec![0u8; notes_off + notes.len()];
        b[..4].copy_from_slice(ELF_MAGIC);
        b[4] = 2; // ELFCLASS64
        b[5] = 1; // little-endian
        b[0x10..0x12].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        b[0x12..0x14].copy_from_slice(&machine.to_le_bytes());
        b[0x20..0x28].copy_from_slice(&(EHDR as u64).to_le_bytes()); // e_phoff
        b[0x36..0x38].copy_from_slice(&(PHDR as u16).to_le_bytes()); // e_phentsize
        b[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes()); // e_phnum

        let ph = EHDR;
        b[ph..ph + 4].copy_from_slice(&4u32.to_le_bytes()); // PT_NOTE
        b[ph + 0x08..ph + 0x10].copy_from_slice(&(notes_off as u64).to_le_bytes());
        b[ph + 0x20..ph + 0x28].copy_from_slice(&(notes.len() as u64).to_le_bytes());
        b[notes_off..].copy_from_slice(&notes);
        b
    }

    fn write_tmp(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn probe_recognises_an_x86_64_pvh_loader() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tmp(&dir, "loader.elf", &elf_bytes(EM_X86_64, true));
        assert_eq!(probe(&p).unwrap(), Image::ElfPvh);
    }

    #[test]
    fn probe_recognises_an_aarch64_loader_img() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tmp(&dir, "loader.img", &header_bytes(0x380000, "/hello.so"));
        match probe(&p).unwrap() {
            Image::Arm64 { header } => {
                assert_eq!(header.image_size, 0x380000);
                assert_eq!(header.cmdline.as_deref(), Some("/hello.so"));
            }
            other => panic!("expected an arm64 image, got {other:?}"),
        }
    }

    #[test]
    fn probe_rejects_an_elf_without_a_pvh_note() {
        // OSv's *aarch64* ELF is the trap here: it is a valid ELF that libkrun
        // cannot enter, and the loader.img is what should be passed instead.
        let dir = tempfile::tempdir().unwrap();
        let p = write_tmp(&dir, "loader.elf", &elf_bytes(EM_X86_64, false));
        let err = probe(&p).unwrap_err().to_string();
        assert!(err.contains("PHYS32_ENTRY"), "{err}");
    }

    #[test]
    fn probe_rejects_an_elf_for_the_wrong_machine() {
        const EM_AARCH64: u16 = 183;
        let dir = tempfile::tempdir().unwrap();
        let p = write_tmp(&dir, "loader.elf", &elf_bytes(EM_AARCH64, true));
        let err = probe(&p).unwrap_err().to_string();
        assert!(err.contains("loader.img"), "{err}");
    }

    #[test]
    fn resolve_image_rejects_a_missing_path_with_guidance() {
        let err = resolve_image("/definitely/not/there")
            .unwrap_err()
            .to_string();
        assert!(err.contains("capstan"), "{err}");
    }
}
