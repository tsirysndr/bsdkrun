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
//!
//! **Volumes** ride in over virtio-fs — the only shared-directory transport
//! libkrun has (no virtio-9p) and the only Unikraft filesystem that can be
//! backed by a host directory. The host adds a share per `--mount` and names it
//! in a mount table on the kernel command line; see [`build_cmdline`], which is
//! where the awkward parts live.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tracing::debug;

use crate::fetch::cache_dir;

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

    let Some(shimmed) = crate::elf::shim_text_offset(&image).context("shimming unikernel entry")?
    else {
        // Links at the load address already (a stock Linux Image, or a Unikraft
        // build with no reserved hole) — nothing to shim.
        return Ok(Prepared {
            path: kernel.to_path_buf(),
            format: crate::krun::KRUN_KERNEL_FORMAT_RAW,
        });
    };
    debug!(len = shimmed.len(), "shimming unikernel entry");

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

/// A host directory shared into the unikernel over virtio-fs.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Volume {
    pub host: PathBuf,
    pub guest: String,
}

/// Parse a `HOST:GUEST` volume spec. `GUEST` must be absolute — Unikraft's
/// fstab parser rejects a relative mountpoint outright.
pub fn parse_volume(spec: &str) -> Result<Volume> {
    let (host, guest) = spec
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("--mount {spec:?} must be HOST:GUEST, e.g. ./data:/data"))?;
    if host.is_empty() || guest.is_empty() {
        bail!("--mount {spec:?} must be HOST:GUEST, e.g. ./data:/data");
    }
    if !guest.starts_with('/') {
        bail!("--mount {spec:?}: the guest path must be absolute (start with '/')");
    }
    let host = std::fs::canonicalize(host)
        .with_context(|| format!("--mount {spec:?}: host path {host:?} does not exist"))?;
    Ok(Volume {
        host,
        guest: guest.to_string(),
    })
}

/// The virtio-fs tag for the `i`th volume. Unikraft matches devices on this
/// (36 bytes, read from the device's config space), and it is also what the
/// `vfs.fstab` entry names as the source device.
pub fn volume_tag(i: usize) -> String {
    format!("vol{i}")
}

/// Build the full guest command line: the `vfs.fstab=[...]` mount table (a
/// *kernel* parameter) followed by the user's command line (the application's
/// `argv`).
///
/// Two details of Unikraft's parser make this fiddlier than it looks, and
/// getting either wrong mounts nothing while the guest still boots happily —
/// every open() simply fails:
///
///   * **`argv[0]` is skipped.** `lib/ukboot/early_init.c` hands the parser
///     `&argv[1]`, treating the first word as the program name. A command line
///     that *starts* with `vfs.fstab=` therefore has the mount table silently
///     eaten as the program name. So a program name always goes first.
///   * **`--` separates the two halves.** Everything before it is parsed as
///     kernel library parameters, everything after becomes the application's
///     `argv`. Without it, nothing is treated as a parameter at all.
///
/// The `--` also has to hold for what *libkrun* adds. On x86_64 there is no
/// device tree, so the guest finds its virtio devices only through the
/// `virtio_mmio.device=` parameters libkrun appends once they are attached —
/// after this cmdline is set. Those have to land in the parameter half too, so
/// this needs a libkrun that inserts them ahead of the stop sequence
/// (`Cmdline::insert_before_stop`); with a plain append they become application
/// `argv`, no virtio-fs device is ever registered, and the mount fails with
/// `-ENOENT` before `main` runs.
///
/// The user's command line keeps its usual meaning throughout — its first word
/// is the application's `argv[0]` — so it is split around the injected table.
///
/// The table format is `"<dev>:<mountpoint>:<fsdriver>"` entries separated by
/// whitespace and wrapped in brackets (`lib/posix-vfs-fstab`,
/// `lib/vfscore/automount.c`); `virtiofs` is the driver name registered by
/// `lib/ukfs-virtiofs`, and `<dev>` is the virtio-fs tag.
pub fn build_cmdline(user_cmdline: &str, vols: &[Volume], progname: &str) -> String {
    if vols.is_empty() {
        return user_cmdline.to_string();
    }
    let mut entries: Vec<String> = Vec::new();
    // A mountpoint has to exist before anything can be mounted on it, and a
    // freshly booted unikernel has no root filesystem at all — so mounting
    // /data fails with ENOENT before virtio-fs is ever consulted. Give it a
    // ramfs root first, unless a volume is itself taking "/".
    if !vols.iter().any(|v| v.guest == "/") {
        entries.push("\"ramfs:/:ramfs\"".into());
    }
    // `mkmp` (make mount point) creates the directory in that root; without it
    // the mount fails with ENOENT for exactly the same reason. The empty
    // fields between are the mount flags and fs-specific options.
    entries.extend(vols.iter().enumerate().map(|(i, v)| {
        if v.guest == "/" {
            format!("\"{}:/:virtiofs\"", volume_tag(i))
        } else {
            format!("\"{}:{}:virtiofs:::mkmp\"", volume_tag(i), v.guest)
        }
    }));
    let fstab = format!("vfs.fstab=[ {} ]", entries.join(" "));

    // Split the user's line into argv[0] and the rest; fall back to the image
    // name when they gave nothing, since the slot cannot be left empty.
    let user = user_cmdline.trim();
    let (argv0, rest) = match user.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest.trim()),
        None if !user.is_empty() => (user, ""),
        None => (progname, ""),
    };
    if rest.is_empty() {
        format!("{argv0} {fstab} --")
    } else {
        format!("{argv0} {fstab} -- {rest}")
    }
}

/// What a unikraft machine was booted from, saved in its state dir so
/// `bsdkrun start` can boot the same image again (nothing else records it: the
/// DB row only carries the image's display name).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BootSpec {
    pub kernel: PathBuf,
    /// The cmdline as the user gave it, WITHOUT the generated `vfs.fstab`
    /// fragment — that is re-derived from `volumes` on each boot, so a restart
    /// does not append a second copy.
    pub cmdline: String,
    pub initramfs: Option<PathBuf>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(guest: &str) -> Volume {
        Volume {
            host: PathBuf::from("/tmp/x"),
            guest: guest.into(),
        }
    }

    /// Without volumes the user's line is passed through untouched — no mount
    /// table, and no `--` that would turn their first word into a parameter.
    #[test]
    fn no_volumes_leaves_the_cmdline_alone() {
        assert_eq!(build_cmdline("app -v", &[], "img"), "app -v");
        assert_eq!(build_cmdline("", &[], "img"), "");
    }

    /// The three things that silently mount nothing if they are wrong: a
    /// program name first (the parser skips argv[0]), `--` before the app's
    /// args, and `mkmp` so the mountpoint gets created.
    #[test]
    fn volumes_build_a_mount_table_after_a_program_name() {
        assert_eq!(
            build_cmdline("", &[vol("/data")], "myimg"),
            "myimg vfs.fstab=[ \"ramfs:/:ramfs\" \"vol0:/data:virtiofs:::mkmp\" ] --"
        );
    }

    /// The user's own line keeps its meaning: first word stays argv[0], the
    /// rest lands after the separator.
    #[test]
    fn the_users_argv0_is_preserved_around_the_table() {
        assert_eq!(
            build_cmdline("app one two", &[vol("/data")], "img"),
            "app vfs.fstab=[ \"ramfs:/:ramfs\" \"vol0:/data:virtiofs:::mkmp\" ] -- one two"
        );
    }

    /// A volume mounted at / IS the root, so no ramfs is added and it needs no
    /// mkmp — the mountpoint it wants already exists by definition.
    #[test]
    fn a_volume_at_the_root_replaces_the_ramfs() {
        assert_eq!(
            build_cmdline("", &[vol("/")], "img"),
            "img vfs.fstab=[ \"vol0:/:virtiofs\" ] --"
        );
    }

    /// Tags are positional and must line up with the virtio-fs devices added
    /// in the same order.
    #[test]
    fn each_volume_gets_its_own_tag() {
        let out = build_cmdline("", &[vol("/a"), vol("/b")], "img");
        assert!(out.contains("\"vol0:/a:virtiofs:::mkmp\""), "{out}");
        assert!(out.contains("\"vol1:/b:virtiofs:::mkmp\""), "{out}");
    }

    #[test]
    fn volume_specs_are_validated() {
        assert!(parse_volume("/tmp:relative").is_err());
        assert!(parse_volume("noguest").is_err());
        assert!(parse_volume("/tmp/definitely-not-there-12345:/x").is_err());
    }
}
