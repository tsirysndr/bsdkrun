//! `linux` subcommand: run an OCI image as a microVM.
//!
//! Flow: fetch a prebuilt aarch64 `vmlinux` (cached), pull the requested OCI
//! image and extract its rootfs (see [`crate::oci`]), then boot it one of two
//! ways:
//!
//!   * **initramfs** (default) — pack the rootfs into a cpio and boot it as the
//!     kernel's initramfs, with a generated `/init` that sets up mounts +
//!     networking and runs the image's entrypoint. Works with a stock
//!     Firecracker-style kernel (no virtio-fs needed).
//!   * **virtio-fs** (`--virtiofs`) — share the extracted rootfs directly via
//!     `krun_set_root`, letting libkrun's own init run the entrypoint. Requires
//!     a guest kernel built with `CONFIG_FUSE_FS=y` / virtio-fs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::fetch::{cache_dir, run};
use crate::oci::{self, ImageConfig};

/// Where the prebuilt aarch64 kernels are published, and the default release.
const KERNEL_RELEASE_BASE: &str = "https://github.com/tsirysndr/vmlinux-builder/releases/download";
pub const DEFAULT_KERNEL_VERSION: &str = "7.1.5";

/// Asset filename for a given vmlinux-builder release.
fn kernel_file(version: &str) -> String {
    format!("vmlinux-{version}.aarch64")
}

/// Download URL for a given vmlinux-builder release.
fn kernel_url(version: &str) -> String {
    format!("{KERNEL_RELEASE_BASE}/{version}/{}", kernel_file(version))
}

/// gvproxy's fixed guest lease + gateway (see [`crate::net`]).
const GUEST_IP: &str = "192.168.127.2";
const GATEWAY_IP: &str = "192.168.127.1";
const NETMASK: &str = "255.255.255.0";

/// Ensure a bootable kernel is available and return the path to a raw arm64
/// `Image` (libkrun's aarch64 loader needs `Image`, not a `vmlinux` ELF).
///
/// Uses the `--kernel` override, or the prebuilt vmlinux-builder release named
/// by `version` (downloaded + cached on first use). If the resolved file is an
/// ELF, it's flattened to an `Image` and cached.
pub fn ensure_kernel(override_path: Option<PathBuf>, version: &str) -> Result<PathBuf> {
    let cache = cache_dir()?;
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("creating cache dir {}", cache.display()))?;

    // Resolve the source kernel (override or downloaded prebuilt). The cache
    // filename embeds the version, so different releases coexist.
    let source = match override_path {
        Some(p) => {
            if !p.exists() {
                bail!("--kernel {} does not exist", p.display());
            }
            p
        }
        None => {
            let kernel = cache.join(kernel_file(version));
            if kernel.exists() {
                info!(path = %kernel.display(), "using cached kernel");
            } else {
                let url = kernel_url(version);
                info!(%url, "downloading kernel…");
                let tmp = cache.join(format!("{}.partial", kernel_file(version)));
                let _ = std::fs::remove_file(&tmp);
                run(
                    Command::new("curl")
                        .args(["-L", "--fail", "--progress-bar", "-o"])
                        .arg(&tmp)
                        .arg(&url),
                    "curl (download kernel)",
                )
                .with_context(|| {
                    format!(
                        "downloading kernel {version} — is that vmlinux-builder release published?"
                    )
                })?;
                std::fs::rename(&tmp, &kernel).context("moving kernel into cache")?;
            }
            kernel
        }
    };

    // If it's already a raw Image, use it directly; otherwise flatten the ELF to
    // a cached Image (keyed by the source's name so overrides don't collide).
    let bytes = std::fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
    if crate::elf::is_arm64_image(&bytes) {
        return Ok(source);
    }
    let stem = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kernel".to_string());
    let image_path = cache.join(format!("{stem}.Image"));
    // Rebuild the Image if missing or older than its source.
    let stale = match (std::fs::metadata(&image_path), std::fs::metadata(&source)) {
        (Ok(img), Ok(src)) => match (img.modified(), src.modified()) {
            (Ok(i), Ok(s)) => i < s,
            _ => true,
        },
        _ => true,
    };
    if !stale {
        info!(path = %image_path.display(), "using cached kernel Image");
        return Ok(image_path);
    }
    info!("converting vmlinux ELF to a raw arm64 Image…");
    let image = crate::elf::read_as_image(&source)?;
    std::fs::write(&image_path, &image)
        .with_context(|| format!("writing {}", image_path.display()))?;
    Ok(image_path)
}

/// The resolved entrypoint to run in the guest.
pub struct Entrypoint {
    /// argv (argv[0] is the program). Never empty.
    pub argv: Vec<String>,
    /// Environment as `KEY=VALUE` entries.
    pub env: Vec<String>,
    /// Working directory (empty = image default / `/`).
    pub workdir: String,
}

/// Combine the image config with any user overrides, following Docker's
/// entrypoint/cmd rules:
///   * `--entrypoint` replaces the image Entrypoint.
///   * positional args after the image replace the image Cmd.
///   * final argv = entrypoint ++ cmd; if empty, fall back to `/bin/sh`.
pub fn resolve_entrypoint(
    cfg: &ImageConfig,
    entrypoint_override: Option<&str>,
    cmd_override: &[String],
) -> Entrypoint {
    let entrypoint = match entrypoint_override {
        Some(e) => vec![e.to_string()],
        None => cfg.entrypoint.clone(),
    };
    let cmd = if !cmd_override.is_empty() {
        cmd_override.to_vec()
    } else {
        cfg.cmd.clone()
    };
    let mut argv: Vec<String> = entrypoint.into_iter().chain(cmd).collect();
    if argv.is_empty() {
        argv.push("/bin/sh".to_string());
    }

    // Ensure PATH is set — minimal images sometimes omit it from the config.
    let mut env = cfg.env.clone();
    if !env.iter().any(|e| e.starts_with("PATH=")) {
        env.push("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string());
    }

    Entrypoint {
        argv,
        env,
        workdir: cfg.workdir.clone(),
    }
}

/// Kernel command line for booting the initramfs. `console` is the guest console
/// device (e.g. `ttyS0`); `net` adds kernel-level IP autoconfig so the image
/// needs no `ip`/`ifconfig` of its own.
pub fn kernel_cmdline(console: &str, net: bool) -> String {
    let mut cmdline = format!("console={console} rdinit=/init");
    if net {
        // ip=client::gw:netmask:hostname:device:autoconf:dns
        cmdline.push_str(&format!(
            " ip={GUEST_IP}::{GATEWAY_IP}:{NETMASK}:bsdkrun:eth0:off:{GATEWAY_IP}"
        ));
    }
    cmdline
}

/// Build the initramfs: a cpio of the extracted rootfs, with a generated `/init`
/// appended as a second cpio segment (so the shared, content-addressed rootfs
/// cache is never mutated). Returns the path to the `.cpio.gz`.
///
/// `persistent` makes `/init` re-spawn the workload in a loop instead of powering
/// off when it exits — used for detached machines with no explicit command, so
/// exiting an attached shell gives a fresh one rather than stopping the machine.
pub fn build_initramfs(
    rootfs: &Path,
    ep: &Entrypoint,
    net: bool,
    persistent: bool,
) -> Result<PathBuf> {
    let work = std::env::temp_dir().join(format!("bsdkrun-linux-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;

    // Segment 2 staging: just our generated /init.
    let initstage = work.join("initstage");
    std::fs::create_dir_all(&initstage)?;
    oci::write_rootfs_file(
        &initstage,
        "init",
        generate_init(ep, net, persistent).as_bytes(),
        0o755,
    )?;

    let part_rootfs = work.join("rootfs.cpio.gz");
    let part_init = work.join("init.cpio.gz");
    cpio_gz(rootfs, ".", &part_rootfs)?;
    cpio_gz(&initstage, "./init", &part_init)?;

    // Concatenate the two gzip'd cpio archives — the kernel unpacks segments in
    // order, so /init (last) wins over any /init the image might carry.
    let out = work.join("initramfs.cpio.gz");
    concat_files(&[&part_rootfs, &part_init], &out)?;
    let _ = std::fs::remove_file(&part_rootfs);
    let _ = std::fs::remove_file(&part_init);
    Ok(out)
}

/// Run `find <list> | cpio -o -H newc | gzip` from `dir`, writing to `out`.
/// `list` is the argument to `find` (e.g. `.`) or a literal path echoed in.
fn cpio_gz(dir: &Path, list: &str, out: &Path) -> Result<()> {
    let dir = dir
        .to_str()
        .context("rootfs path is not valid UTF-8")?
        .replace('\'', "'\\''");
    let out_s = out
        .to_str()
        .context("output path is not valid UTF-8")?
        .replace('\'', "'\\''");
    // For a single explicit entry we `echo` it; for a tree we `find`.
    let producer = if list == "." {
        "find . -print".to_string()
    } else {
        format!("printf '%s\\n' '{}'", list.replace('\'', "'\\''"))
    };
    let script = format!(
        "cd '{dir}' && {producer} | cpio -o -H newc --quiet 2>/dev/null | gzip -1 > '{out_s}'"
    );
    run(
        Command::new("sh").arg("-c").arg(&script),
        "cpio (pack initramfs)",
    )
}

fn concat_files(parts: &[&Path], out: &Path) -> Result<()> {
    use std::io::Write;
    let mut f =
        std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
    for p in parts {
        let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        f.write_all(&bytes)
            .with_context(|| format!("appending {}", p.display()))?;
    }
    Ok(())
}

/// Generate the `/init` (PID 1) shell script for the initramfs boot: mount the
/// core pseudo-filesystems, point DNS at gvproxy, run the entrypoint, then power
/// the VM off cleanly (via magic-sysrq — no shutdown binary needed in the image).
fn generate_init(ep: &Entrypoint, net: bool, persistent: bool) -> String {
    let mut s = String::from("#!/bin/sh\n# generated by bsdkrun\n");
    s.push_str("mkdir -p /proc /sys /dev 2>/dev/null\n");
    s.push_str("mount -t proc proc /proc 2>/dev/null\n");
    s.push_str("mount -t sysfs sysfs /sys 2>/dev/null\n");
    s.push_str("mount -t devtmpfs dev /dev 2>/dev/null\n");
    if net {
        s.push_str(&format!(
            "echo 'nameserver {GATEWAY_IP}' > /etc/resolv.conf 2>/dev/null\n"
        ));
    }
    for e in &ep.env {
        if let Some((k, v)) = e.split_once('=') {
            s.push_str(&format!("export {k}={}\n", sh_quote(v)));
        }
    }
    if !ep.workdir.is_empty() {
        s.push_str(&format!("cd {} 2>/dev/null\n", sh_quote(&ep.workdir)));
    }
    // Run the workload in its OWN session with the console as controlling
    // terminal, so Ctrl-C and job control actually reach it. Without this the
    // program shares PID 1's session — which has no controlling tty — so the
    // console's INTR char (^C) has no foreground process group to signal, and
    // the shell reports "can't access tty; job control turned off".
    //
    // `setsid -c` acquires the controlling tty. On util-linux `-w` makes setsid
    // wait for the child (we probe for it with a harmless `true`); on busybox,
    // `setsid -c` exec's the program in place, so it waits inherently. If setsid
    // is absent we fall back to a plain run (no job control, but it still runs).
    let cmd: Vec<String> = ep.argv.iter().map(|a| sh_quote(a)).collect();
    s.push_str(&format!("set -- {}\n", cmd.join(" ")));
    // `run` executes "$@" in its own session with the console as controlling
    // terminal (via setsid -c), so Ctrl-C and job control reach it.
    s.push_str(
        "run() {\n\
        \tif command -v setsid >/dev/null 2>&1; then\n\
        \t\tif setsid -w -c true 2>/dev/null; then setsid -w -c \"$@\"; else setsid -c \"$@\"; fi\n\
        \telse\n\
        \t\t\"$@\"\n\
        \tfi\n\
        }\n",
    );
    if persistent {
        // Detached machine with no explicit command: keep a console shell alive
        // across exits. When the shell exits we emit an (invisible OSC) marker so
        // an attached `shell` client detaches back to the host, then respawn a
        // fresh shell for the next attach. The machine ends only via `stop`.
        s.push_str(
            "while : ; do run \"$@\"; printf '\\033]6666;bsdkrun-exit\\007'; set -- /bin/sh ; done\n",
        );
    } else {
        // Run the workload once, then power the machine off cleanly (magic-sysrq
        // needs no shutdown binary in the image); fall back to a shell rather
        // than panic-looping if that fails.
        s.push_str("run \"$@\"\n");
        s.push_str("code=$?\n");
        s.push_str("echo \"[bsdkrun] entrypoint exited (status $code); powering off\"\n");
        s.push_str("sync\n");
        s.push_str(
            "poweroff -f 2>/dev/null || halt -f 2>/dev/null || \
             echo o > /proc/sysrq-trigger 2>/dev/null\n",
        );
        s.push_str("exec /bin/sh\n");
    }
    s
}

/// POSIX-sh single-quote a string.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Log a friendly hint about memory sizing for the initramfs path.
pub fn warn_initramfs_memory(rootfs: &Path, mem_mib: u32) {
    if let Ok(size) = dir_size(rootfs) {
        let rootfs_mib = size / (1024 * 1024);
        // The whole rootfs lives in RAM; leave headroom for the kernel + work.
        if rootfs_mib + 128 > mem_mib as u64 {
            warn!(
                rootfs_mib,
                mem_mib,
                "the rootfs may not fit in RAM — the initramfs is loaded entirely into guest \
                 memory; consider a larger --mem (or --virtiofs)"
            );
        }
    }
}

fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)?.filter_map(|e| e.ok()) {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() && !meta.is_symlink() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    Ok(total)
}
