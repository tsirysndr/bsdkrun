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
//!     a guest kernel built with virtio-fs (`CONFIG_VIRTIO_FS=y`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

use crate::fetch::{cache_dir, run};
use crate::oci::{self, ImageConfig};

/// Where the prebuilt aarch64 kernels are published, and the default release.
const KERNEL_RELEASE_BASE: &str = "https://github.com/tsirysndr/vmlinux-builder/releases/download";
pub const DEFAULT_KERNEL_VERSION: &str = "7.1.8";

/// Asset filename for a given vmlinux-builder release + host arch.
fn kernel_file(version: &str, arch: crate::host::Arch) -> String {
    format!("vmlinux-{version}.{}", arch.slug())
}

/// Download URL for a given vmlinux-builder release + host arch.
fn kernel_url(version: &str, arch: crate::host::Arch) -> String {
    format!(
        "{KERNEL_RELEASE_BASE}/{version}/{}",
        kernel_file(version, arch)
    )
}

/// libkrun kernel format for the host arch: aarch64 boots a raw `Image`, x86_64
/// loads the `vmlinux` ELF directly.
pub fn kernel_format() -> u32 {
    match crate::host::Arch::current() {
        Ok(crate::host::Arch::X86_64) => crate::krun::KRUN_KERNEL_FORMAT_ELF,
        _ => crate::krun::KRUN_KERNEL_FORMAT_RAW,
    }
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
    let arch = crate::host::Arch::current()?;
    let cache = cache_dir()?;
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("creating cache dir {}", cache.display()))?;

    // Resolve the source kernel (override or downloaded prebuilt). The cache
    // filename embeds the version + arch, so releases/arches coexist.
    let source = match override_path {
        Some(p) => {
            if !p.exists() {
                bail!("--kernel {} does not exist", p.display());
            }
            p
        }
        None => {
            let kernel = cache.join(kernel_file(version, arch));
            if kernel.exists() {
                info!(path = %kernel.display(), "using cached kernel");
            } else {
                let url = kernel_url(version, arch);
                info!(%url, "downloading kernel…");
                let tmp = cache.join(format!("{}.partial", kernel_file(version, arch)));
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
                        "downloading kernel {version} ({}) — is that vmlinux-builder release \
                         published for this arch?",
                        arch.slug()
                    )
                })?;
                std::fs::rename(&tmp, &kernel).context("moving kernel into cache")?;
            }
            kernel
        }
    };

    // x86_64 boots the vmlinux ELF directly (KRUN_KERNEL_FORMAT_ELF) — no
    // flattening. aarch64 needs a raw `Image`, so flatten the ELF (cached).
    if arch != crate::host::Arch::Aarch64 {
        return Ok(source);
    }
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

/// A host directory bind-mounted into the guest over virtio-fs (`--mount`).
pub struct BindMount {
    /// Absolute host directory to share.
    pub host: PathBuf,
    /// Absolute guest mount point.
    pub guest: String,
    /// Mount read-only in the guest.
    pub ro: bool,
}

/// virtio-fs tag for the i-th `--mount` share (matched between the host-side
/// `add_virtiofs` and the guest-side `mount` in the generated init).
pub fn mount_tag(i: usize) -> String {
    format!("bkm{i}")
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
    add_ip(&mut cmdline, net);
    cmdline
}

/// Path of our generated init inside a virtio-fs rootfs.
const VIRTIOFS_INIT: &str = "/.bsdkrun-init";

/// Kernel command line for the virtio-fs boot: mount the shared rootfs (tag
/// `/dev/root`) read-write and run our own init — bypassing libkrun's `init.krun`
/// (which only works with the bundled libkrunfw kernel).
pub fn virtiofs_cmdline(console: &str, net: bool) -> String {
    let mut cmdline =
        format!("console={console} root=/dev/root rootfstype=virtiofs rw init={VIRTIOFS_INIT}");
    add_ip(&mut cmdline, net);
    cmdline
}

/// Append kernel-level IP autoconfig (so the image needs no `ip`/`ifconfig`).
fn add_ip(cmdline: &mut String, net: bool) {
    if net {
        // A shared-network member boots with an assigned static IP (`BSDKRUN_NET_IP`);
        // a solo machine keeps the default .2.
        let guest = std::env::var("BSDKRUN_NET_IP")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| GUEST_IP.to_string());
        // ip=client::gw:netmask:hostname:device:autoconf:dns
        cmdline.push_str(&format!(
            " ip={guest}::{GATEWAY_IP}:{NETMASK}:bsdkrun:eth0:off:{GATEWAY_IP}"
        ));
    }
}

// --- persistent volumes (writable virtio-fs root) ---------------------------
//
// A named volume is just a persistent, writable rootfs (like a Docker volume):
// the first use CoW-clones the base image into the volume dir; later boots reuse
// it as-is, so the guest's changes survive reboots. libkrun serves it as the
// virtio-fs root (`/dev/root`) exactly like the per-machine clone.
//
// We deliberately do NOT layer overlayfs over virtio-fs here: on Linux/KVM
// overlayfs rejects a virtio-fs upperdir/workdir (it can't set the UUID xattr or
// create the work dir), so the overlay silently drops to read-only and init
// can't exec — a kernel panic ("Attempted to kill init!"). A plain writable
// virtio-fs root behaves identically on macOS and Linux.

/// Prepare a persistent, writable virtio-fs root backed by a named volume.
///
/// First use CoW-clones the cached image into `<volume_dir>/rootfs`; later uses
/// keep it (the guest's changes persist). We always (re)inject our generated
/// init + exec agent so a bsdkrun upgrade ships new ones into existing volumes.
pub fn prepare_volume_root(
    cached_rootfs: &Path,
    ep: &Entrypoint,
    net: bool,
    persistent: bool,
    volume_dir: &Path,
    mounts: &[BindMount],
) -> Result<PathBuf> {
    let root = volume_dir.join("rootfs");
    // First use: CoW-clone the image in. Reuse: keep the persisted rootfs so the
    // guest's changes from previous boots are still there.
    if !root.exists() {
        std::fs::create_dir_all(volume_dir)
            .with_context(|| format!("creating {}", volume_dir.display()))?;
        crate::host::cow_copy(cached_rootfs, &root, true)?;
    }
    // Always refresh the bsdkrun-managed init + agent (never the user's data).
    oci::write_rootfs_file(
        &root,
        VIRTIOFS_INIT.trim_start_matches('/'),
        generate_init(ep, net, persistent, mounts).as_bytes(),
        0o755,
    )?;
    crate::agent::inject_linux(&root)?;
    Ok(root)
}

/// Prepare a per-machine, writable virtio-fs root: clone the cached rootfs into
/// the machine's state dir and drop in our generated init at `VIRTIOFS_INIT`.
///
/// `cp -Rc` uses APFS `clonefile(2)` where available — an instant copy-on-write
/// clone that costs no disk until the guest writes — so the shared image cache
/// stays pristine and machines are isolated (unlike sharing one rw dir). It
/// still serves the rootfs from disk, so there's no initramfs RAM load.
pub fn prepare_virtiofs_root(
    cached_rootfs: &Path,
    ep: &Entrypoint,
    net: bool,
    persistent: bool,
    machine_dir: &Path,
    mounts: &[BindMount],
) -> Result<PathBuf> {
    std::fs::create_dir_all(machine_dir)
        .with_context(|| format!("creating {}", machine_dir.display()))?;
    let root = machine_dir.join("rootfs");
    // Reuse an intact per-machine rootfs (a restart of the SAME id) instead of
    // re-cloning the whole image — cloning a large read-only nix rootfs takes
    // ~20s, which is what made `start`/Play feel like it "spins forever". It's
    // reusable when it exists, carries the image content (/nix or /bin), and is
    // NOT a broken rootfs/rootfs nesting from an earlier failure. Fresh runs (new
    // id → no rootfs) and broken clones fall through and clone.
    // A restart boots the machine's OWN rootfs by passing it as the source. Never
    // rename/clone in that case — the "free the target then clone" path below
    // would delete the source and lose all data. Detect it (same path) and reuse
    // unconditionally, independent of the content heuristic, so restart can never
    // fall back to re-cloning the base image.
    let booting_own_rootfs = root.symlink_metadata().is_ok()
        && std::fs::canonicalize(cached_rootfs).ok() == std::fs::canonicalize(&root).ok();
    let reusable = booting_own_rootfs
        || (root.symlink_metadata().is_ok()
            && root.join("rootfs").symlink_metadata().is_err()
            && (root.join("nix").exists() || root.join("bin").exists()));
    if !reusable {
        // Free the target path RELIABLY before cloning. `cp -Rc SRC DST` copies
        // INTO DST when DST exists (→ rootfs/rootfs nesting → init not found →
        // kernel panic), and a nix rootfs's read-only /nix/store can make a
        // recursive delete flake — so rename any stale clone aside (rename needs
        // write only on the parent, which we own), GC it best-effort, then clone.
        if root.symlink_metadata().is_ok() {
            let trash = machine_dir.join(format!(".rootfs.trash.{}", std::process::id()));
            if std::fs::rename(&root, &trash).is_ok() {
                crate::host::force_remove_dir_all_async(&trash);
            } else {
                crate::host::force_remove_dir_all(&root);
            }
        }
        // Copy-on-write clone (APFS clonefile / Linux reflink); plain-copy fallback.
        crate::host::cow_copy(cached_rootfs, &root, true)?;
    }

    // (Re)write our init + agent every boot (cheap; also picks up bsdkrun
    // upgrades even on a reused rootfs). The init handles mounts, net, exec.
    oci::write_rootfs_file(
        &root,
        VIRTIOFS_INIT.trim_start_matches('/'),
        generate_init(ep, net, persistent, mounts).as_bytes(),
        0o755,
    )?;
    crate::agent::inject_linux(&root)?;
    Ok(root)
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
    mounts: &[BindMount],
) -> Result<PathBuf> {
    let work = std::env::temp_dir().join(format!("bsdkrun-linux-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;

    // Segment 2 staging: our generated /init plus the exec agent.
    let initstage = work.join("initstage");
    std::fs::create_dir_all(&initstage)?;
    oci::write_rootfs_file(
        &initstage,
        "init",
        generate_init(ep, net, persistent, mounts).as_bytes(),
        0o755,
    )?;
    crate::agent::inject_linux(&initstage)?;

    let part_rootfs = work.join("rootfs.cpio.gz");
    let part_init = work.join("init.cpio.gz");
    cpio_gz(rootfs, ".", &part_rootfs)?;
    cpio_gz(&initstage, ".", &part_init)?;

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
fn generate_init(ep: &Entrypoint, net: bool, persistent: bool, mounts: &[BindMount]) -> String {
    let mut s = String::from("#!/bin/sh\n# generated by bsdkrun\n");
    // PATH FIRST: the kernel execs init with an EMPTY environment, so mkdir /
    // mount / mknod are unfindable until we set one. FHS images keep them in
    // /bin,/sbin; nix-based images (nixos/nix) keep EVERY binary under
    // /nix/store, reachable only through the profile dirs. Prepend the image's
    // own configured PATH, then add both sets of fallbacks.
    {
        let img_path = ep
            .env
            .iter()
            .find_map(|e| e.strip_prefix("PATH="))
            .unwrap_or("");
        let mut path = String::new();
        if !img_path.is_empty() {
            path.push_str(img_path);
            path.push(':');
        }
        path.push_str(
            "/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:\
             /nix/var/nix/profiles/default/sbin:/root/.nix-profile/bin:\
             /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        );
        s.push_str(&format!("export PATH={}\n", sh_quote(&path)));
    }
    // Locate util-linux tools (mount, setsid, …) that nix-based images keep off
    // PATH entirely — they live only in a /nix/store/…util-linux…/bin dir. FHS
    // images have them on PATH, so `command -v` finds them first. Returns an
    // absolute path (or empty). `command -v` is a builtin, so this is safe to
    // call before /dev/null exists (no stderr redirect needed).
    s.push_str(
        "_find() {\n\
         \t_p=\"$(command -v \"$1\")\"\n\
         \tif [ -z \"$_p\" ]; then\n\
         \t\tfor _c in /sbin/\"$1\" /bin/\"$1\" /nix/store/*util-linux*/bin/\"$1\" /nix/store/*/bin/\"$1\"; do\n\
         \t\t\tif [ -x \"$_c\" ]; then _p=\"$_c\"; break; fi\n\
         \t\tdone\n\
         \tfi\n\
         \tprintf '%s' \"$_p\"\n\
         }\n",
    );
    // Wrap mount(8) in a shell function so every call below routes through the
    // binary we found (nix images don't put it on PATH).
    s.push_str("_mount=\"$(_find mount)\"\n");
    s.push_str(
        "mount() { if [ -n \"$_mount\" ]; then \"$_mount\" \"$@\"; else echo '[bsdkrun] no mount(8) found' >&2; return 1; fi; }\n",
    );
    // CHICKEN-AND-EGG: these bootstrap mounts must NOT redirect to /dev/null,
    // because /dev/null does not exist until devtmpfs is mounted below. In POSIX
    // shells a failed redirection ABORTS the command, so `... 2>/dev/null` here
    // would silently skip the very mounts that create /dev — leaving /proc, /sys
    // and /dev all unmounted. That breaks anything reading /proc/self/exe (e.g.
    // `nix`) and every later `2>/dev/null` in this script. Some images (NixOS)
    // ship an empty /dev and the kernel's devtmpfs auto-mount fails
    // ("devtmpfs: error mounting -2"), so we also create the essential nodes by
    // hand as a fallback.
    s.push_str("mkdir -p /proc /sys /dev\n");
    // Mount /proc FIRST so /proc/mounts exists, then only mount what the kernel
    // hasn't already auto-mounted. FHS images (alpine/debian/…) arrive with /dev
    // already on devtmpfs, so a blind re-mount errors "dev already mounted or
    // mount point busy". We can't hide that with `2>/dev/null` (chicken-and-egg:
    // /dev/null may not exist yet), so guard with a pure-shell mountpoint check
    // that reads /proc/mounts — no grep/mountpoint binary needed.
    s.push_str("[ -e /proc/mounts ] || mount -t proc proc /proc\n");
    s.push_str(
        "is_mounted() { while read -r _ _mp _; do [ \"$_mp\" = \"$1\" ] && return 0; done < /proc/mounts; return 1; }\n",
    );
    s.push_str("is_mounted /sys || mount -t sysfs sysfs /sys\n");
    s.push_str("is_mounted /dev || mount -t devtmpfs dev /dev\n");
    // cgroup2 (unified hierarchy). On a real host this arrives for free —
    // systemd mounts it, or a container inherits it bind-mounted in from the
    // host. Here the guest kernel IS the whole machine, so nothing mounts it
    // unless we do. Without this, any workload that manages its own cgroups
    // (dockerd/containerd chief among them) dies at startup with "devices
    // cgroup isn't mounted" — the daemon falls back to probing for a legacy
    // cgroup v1 hierarchy, finds nothing mounted at all, and gives up.
    s.push_str("mkdir -p /sys/fs/cgroup\n");
    s.push_str("is_mounted /sys/fs/cgroup || mount -t cgroup2 none /sys/fs/cgroup 2>/dev/null\n");
    // Delegate a leaf cgroup to the workload instead of leaving it at the true
    // root. The root cgroup permanently holds every kernel thread (they can
    // never be moved out — that's fundamental cgroup semantics, not a config
    // issue), so anything that tries to enable controllers there fails with
    // EINVAL forever. On a real Docker host this is a non-issue: `--privileged`
    // gives the container its own cgroup NAMESPACE, so /sys/fs/cgroup already
    // shows a kernel-thread-free subtree. We can't unshare(CLONE_NEWCGROUP) —
    // busybox's `unshare` in these images has no -C/--cgroup support — so we
    // get the same effect with a plain bind mount: move ourselves into a child
    // cgroup, then bind that child over /sys/fs/cgroup. Every process we
    // subsequently start (the whole workload) inherits the move; the true root
    // — and its kernel threads — becomes unreachable through this path.
    s.push_str("mkdir -p /sys/fs/cgroup/machine\n");
    s.push_str("echo $$ > /sys/fs/cgroup/machine/cgroup.procs 2>/dev/null\n");
    s.push_str("mount --bind /sys/fs/cgroup/machine /sys/fs/cgroup 2>/dev/null\n");
    s.push_str("[ -e /dev/null ] || mknod -m 666 /dev/null c 1 3\n");
    s.push_str("[ -e /dev/zero ] || mknod -m 666 /dev/zero c 1 5\n");
    s.push_str("[ -e /dev/console ] || mknod -m 600 /dev/console c 5 1\n");
    s.push_str("[ -e /dev/tty ] || mknod -m 666 /dev/tty c 5 0\n");
    // devpts is required for openpty (used by `exec -t` / `shell` in the agent).
    s.push_str("mkdir -p /dev/pts 2>/dev/null\n");
    s.push_str("mount -t devpts devpts /dev/pts 2>/dev/null\n");
    // devtmpfs supplies only real device nodes — the /dev/fd, /dev/stdin,
    // /dev/stdout and /dev/stderr symlinks are userspace convention, normally
    // created by udev/systemd, which never runs here. Without /dev/fd, bash
    // process substitution `<(…)` silently produces a path nothing can open:
    // nix's stdenv setup.sh uses it, and every build dies with
    // "/dev/fd/63: No such file or directory". These are plain symlinks into
    // /proc/self/fd, so they cost nothing and must come after /proc is mounted.
    s.push_str("[ -e /dev/fd ] || ln -s /proc/self/fd /dev/fd 2>/dev/null\n");
    s.push_str("[ -e /dev/stdin ] || ln -s /proc/self/fd/0 /dev/stdin 2>/dev/null\n");
    s.push_str("[ -e /dev/stdout ] || ln -s /proc/self/fd/1 /dev/stdout 2>/dev/null\n");
    s.push_str("[ -e /dev/stderr ] || ln -s /proc/self/fd/2 /dev/stderr 2>/dev/null\n");
    // Set the clock when the guest has none: aarch64 microVMs get no RTC, so
    // the kernel boots at epoch 0 (Jan 1 1970) and ALL TLS fails ("certificate
    // not valid yet") — apk/apt, tailscale downloads, everything. This script
    // is generated at boot, so the host's wall clock (UTC, in POSIX
    // MMDDhhmmYYYY.ss form — the one form busybox and coreutils both accept)
    // is baked in; skipped when a working clock source (e.g. kvmclock on
    // x86_64) already set a sane time.
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (y, mo, d, h, mi, sec) = utc_parts(now);
        s.push_str(&format!(
            "if command -v date >/dev/null 2>&1; then [ \"$(date +%s 2>/dev/null)\" -lt 1000000000 ] && date -u {mo:02}{d:02}{h:02}{mi:02}{y:04}.{sec:02} >/dev/null 2>&1; fi\n"
        ));
    }
    // Bind-mounted host directories (`--mount HOST:GUEST[:ro]`), over virtio-fs.
    for (i, m) in mounts.iter().enumerate() {
        let g = sh_quote(&m.guest);
        let opt = if m.ro { " -o ro" } else { "" };
        s.push_str(&format!("mkdir -p {g} 2>/dev/null\n"));
        s.push_str(&format!(
            "mount -t virtiofs{opt} {} {g} || echo '[bsdkrun] mount {g} failed'\n",
            mount_tag(i)
        ));
    }
    if net {
        // On a global network, add a `search <network>` domain so bare peer names
        // (e.g. `ping db`) resolve against the network's gvproxy DNS zone.
        let search = std::env::var("BSDKRUN_NET_NAME")
            .ok()
            .filter(|n| !n.is_empty())
            .map(|n| format!("search {n}\\n"))
            .unwrap_or_default();
        s.push_str(&format!(
            "printf '{search}nameserver {GATEWAY_IP}\\n' > /etc/resolv.conf 2>/dev/null\n"
        ));
        // The kernel's `ip=` autoconfig (see add_ip) hardcodes the guest's own
        // hostname to "bsdkrun", but gvproxy's DNS has no record for it (that
        // only happens for named --network members). Anything that resolves
        // its own hostname — e.g. dind's entrypoint doing `hostname`/`getent
        // hosts $(hostname)` for its TLS cert CN — fails with "Host not
        // found" without a local /etc/hosts entry.
        let guest = std::env::var("BSDKRUN_NET_IP")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| GUEST_IP.to_string());
        s.push_str(&format!(
            "printf '127.0.0.1 localhost\\n{guest} bsdkrun\\n' >> /etc/hosts 2>/dev/null\n"
        ));
    }
    // systemd handoff: `bsdkrun-agent systemd setup` installs systemd, writes
    // an agent unit, and drops this marker — from then on the guest boots a
    // full systemd system. exec keeps it PID 1 (which systemd requires); the
    // agent is NOT pre-started here in that case, its unit owns it (two agents
    // would fight over the TCP port). Everything after this line is the plain
    // bsdkrun init path.
    s.push_str(
        "if [ -f /etc/bsdkrun-systemd ]; then\n\
         \tfor sd in /lib/systemd/systemd /usr/lib/systemd/systemd; do\n\
         \t\tif [ -x \"$sd\" ]; then\n\
         \t\t\techo '[bsdkrun] handing PID 1 to systemd'\n\
         \t\t\texec \"$sd\"\n\
         \t\tfi\n\
         \tdone\n\
         \techo '[bsdkrun] /etc/bsdkrun-systemd set but no systemd binary found; continuing'\n\
         fi\n",
    );
    for e in &ep.env {
        if let Some((k, v)) = e.split_once('=') {
            // PATH is already exported (enriched with FHS + nix fallbacks) at the
            // top; don't overwrite it with the image's narrower value.
            if k == "PATH" {
                continue;
            }
            s.push_str(&format!("export {k}={}\n", sh_quote(v)));
        }
    }
    // Neutralize nix's build users. The nixos/nix image ships
    // `build-users-group = nixbld`, so nix — itself running as root — drops each
    // *builder* to a nixbld uid. That is wrong for a microVM in two ways.
    //
    // On a Linux host, libkrun's virtio-fs backend honours the guest's
    // credentials per operation, but the server process is unprivileged and
    // cannot act as a nixbld uid against a store owned by the user who launched
    // us, so any build that writes to the store dies with
    // `creating directory ".../user-environment": Permission denied` — while
    // plain `nix-store --add` (done by nix itself, not a build user) succeeds,
    // which makes the failure look arbitrary. On macOS the backend performs
    // every operation as the host user regardless of guest uid, so it does not
    // bite there; the setting is still pointless.
    //
    // Build users exist to isolate builders from each other and from the store
    // on a shared multi-user machine. A microVM is already that isolation, and
    // it is single-user, so clearing this gives up nothing.
    //
    // Done via NIX_CONFIG rather than by editing /etc/nix/nix.conf, because in
    // this image that path is a symlink into the read-only /nix/store and
    // cannot be written. Skipped when the image (or `-e`) already set
    // NIX_CONFIG, and when there is no nix store to care about.
    if !ep.env.iter().any(|e| e.starts_with("NIX_CONFIG=")) {
        s.push_str("if [ -d /nix/store ]; then export NIX_CONFIG='build-users-group ='; fi\n");
    }
    // Default HOME when the image didn't set one. The kernel launches init with
    // no HOME, so it stays empty/`/` — nix then warns "$HOME ('/') is not owned
    // by you, falling back to … ('/root')". Guests run as root, so derive HOME
    // from USER (root → /root) unless the image already declared it.
    if !ep.env.iter().any(|e| e.starts_with("HOME=")) {
        let user = ep
            .env
            .iter()
            .find_map(|e| e.strip_prefix("USER="))
            .unwrap_or("root");
        let home = if user.is_empty() || user == "root" {
            "/root".to_string()
        } else {
            format!("/home/{user}")
        };
        s.push_str(&format!("export HOME={}\n", sh_quote(&home)));
    }
    // Start the exec agent (TCP; for `exec`/`shell`) in the background — AFTER
    // the env + HOME exports above, so the agent (and every command it spawns
    // for `exec`/`shell`) inherits them. Starting it earlier left agent-run
    // shells with HOME=/ (nix: "$HOME ('/') is not owned by you").
    s.push_str("[ -x /sbin/bsdkrun-agent ] && /sbin/bsdkrun-agent >/dev/null 2>&1 &\n");
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
    // setsid is util-linux, which nix images keep off PATH (like mount), so
    // resolve it via _find rather than a bare `command -v` — otherwise the
    // interactive shell gets "cannot set terminal process group / no job control".
    s.push_str("_setsid=\"$(_find setsid)\"\n");
    let cmd: Vec<String> = ep.argv.iter().map(|a| sh_quote(a)).collect();
    s.push_str(&format!("set -- {}\n", cmd.join(" ")));
    // `run` executes "$@" in its own session with the console as controlling
    // terminal (via setsid -c), so Ctrl-C and job control reach it.
    s.push_str(
        "run() {\n\
        \tif [ -n \"$_setsid\" ]; then\n\
        \t\tif \"$_setsid\" -w -c true 2>/dev/null; then \"$_setsid\" -w -c \"$@\"; else \"$_setsid\" -c \"$@\"; fi\n\
        \telse\n\
        \t\t\"$@\"\n\
        \tfi\n\
        }\n",
    );
    if persistent {
        // Detached machine with no explicit command: keep the console shell alive
        // across exits. Re-run the *same* command each time (so the image's real
        // shell + prompt is preserved, not a bare /bin/sh), emitting an invisible
        // OSC marker on exit so an attached `shell` detaches back to the host.
        // The machine ends only via `stop`.
        s.push_str("while : ; do run \"$@\"; printf '\\033]6666;bsdkrun-exit\\007'; done\n");
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

/// Split a unix timestamp into UTC (year, month, day, hour, min, sec) — the
/// civil-from-days algorithm (Howard Hinnant), no chrono dependency needed.
fn utc_parts(epoch: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = epoch / 86_400;
    let secs = epoch % 86_400;
    let (h, mi, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u64;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u64;
    let y = (if m <= 2 { y + 1 } else { y }) as u64;
    (y, m, d, h, mi, s)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ep() -> Entrypoint {
        Entrypoint {
            argv: vec!["/bin/bash".into()],
            env: vec!["PATH=/root/.nix-profile/bin:/nix/var/nix/profiles/default/bin".into()],
            workdir: String::new(),
        }
    }

    #[test]
    fn init_resolves_offpath_util_linux_tools() {
        let s = generate_init(&ep(), true, false, &[]);
        // PATH is exported before any command that needs mkdir/mount/mknod.
        let path_pos = s.find("export PATH=").expect("PATH exported");
        let mount_pos = s.find("mount -t proc").expect("proc mount present");
        assert!(path_pos < mount_pos, "PATH must be set before the mounts");
        // The image's own PATH is folded in, plus nix + FHS fallbacks.
        assert!(s.contains("/root/.nix-profile/bin"));
        assert!(s.contains("/nix/var/nix/profiles/default/bin"));
        assert!(s.contains(":/sbin:/bin"));
        // mount(8) and setsid are resolved via the nix-store glob, not bare PATH.
        assert!(s.contains("_find()"));
        assert!(s.contains("/nix/store/*util-linux*/bin/"));
        assert!(s.contains("_mount=\"$(_find mount)\""));
        assert!(s.contains("_setsid=\"$(_find setsid)\""));
        // run() uses the resolved setsid, not a bare `command -v setsid`.
        assert!(s.contains("\"$_setsid\" -w -c"));
        assert!(!s.contains("command -v setsid"));
        // devtmpfs fallback nodes are created if the auto-mount left /dev empty.
        assert!(s.contains("mknod -m 666 /dev/null"));
        // HOME defaults to /root (image set USER=root, no HOME) so nix stops
        // warning "$HOME ('/') is not owned by you".
        assert!(s.contains("export HOME='/root'"));
    }

    #[test]
    fn init_mounts_cgroup2() {
        let s = generate_init(&ep(), true, false, &[]);
        // Required before any workload that manages its own cgroups (dockerd,
        // containerd) can start — otherwise it fails with "devices cgroup
        // isn't mounted", since the guest kernel is the whole machine and
        // nothing else mounts cgroupfs for it.
        assert!(s.contains("mkdir -p /sys/fs/cgroup"));
        assert!(s.contains("mount -t cgroup2 none /sys/fs/cgroup"));
        // Mounted after /sys (which cgroup2 sits under) but before the
        // workload's entrypoint runs.
        let sys_pos = s.find("mount -t sysfs sysfs /sys").expect("sysfs mount");
        let cgroup_pos = s.find("mount -t cgroup2").expect("cgroup2 mount");
        assert!(sys_pos < cgroup_pos);
    }

    #[test]
    fn init_delegates_a_leaf_cgroup_to_the_workload() {
        let s = generate_init(&ep(), true, false, &[]);
        // The root cgroup permanently holds kernel threads, which can never be
        // migrated out — so anything (e.g. dockerd/dind) that waits for the
        // root cgroup to empty before enabling controllers would spin forever.
        // We move PID 1 into a child cgroup and bind it over /sys/fs/cgroup so
        // the workload only ever sees a kernel-thread-free subtree.
        assert!(s.contains("mkdir -p /sys/fs/cgroup/machine"));
        assert!(s.contains("echo $$ > /sys/fs/cgroup/machine/cgroup.procs"));
        assert!(s.contains("mount --bind /sys/fs/cgroup/machine /sys/fs/cgroup"));
        // Delegated after the cgroup2 mount exists, before the workload runs.
        let cgroup2_pos = s.find("mount -t cgroup2").expect("cgroup2 mount");
        let bind_pos = s.find("mount --bind").expect("bind mount");
        assert!(cgroup2_pos < bind_pos);
    }

    #[test]
    fn init_registers_own_hostname_in_etc_hosts() {
        let s = generate_init(&ep(), true, false, &[]);
        // The kernel's `ip=` autoconfig hardcodes the guest hostname to
        // "bsdkrun" (see add_ip), but gvproxy's DNS has no record for it, so
        // anything resolving its own hostname (e.g. dind's entrypoint) would
        // otherwise fail with "Host not found".
        assert!(s.contains(&format!("{GUEST_IP} bsdkrun")));
        assert!(s.contains(">> /etc/hosts"));
        // No net, no kernel-assigned hostname to register.
        let s = generate_init(&ep(), false, false, &[]);
        assert!(!s.contains("/etc/hosts"));
    }

    #[test]
    fn image_home_is_respected() {
        let mut e = ep();
        e.env.push("HOME=/home/dev".into());
        let s = generate_init(&e, false, false, &[]);
        // Don't override an image-provided HOME with our default.
        assert!(!s.contains("export HOME='/root'"));
        assert!(s.contains("export HOME='/home/dev'"));
    }

    #[test]
    fn bootstrap_mounts_do_not_redirect_to_devnull() {
        let s = generate_init(&ep(), false, false, &[]);
        // /dev/null does not exist yet at these lines; a redirect would abort them.
        assert!(!s.contains("mount -t proc proc /proc 2>/dev/null"));
        assert!(!s.contains("mount -t devtmpfs dev /dev 2>/dev/null"));
        // /dev is only mounted if not already mounted, so FHS images (alpine)
        // whose kernel auto-mounted devtmpfs don't error "dev already mounted".
        assert!(s.contains("is_mounted /dev || mount -t devtmpfs dev /dev"));
        assert!(s.contains("is_mounted /sys || mount -t sysfs sysfs /sys"));
        // The mountpoint check reads /proc/mounts in pure shell — no grep needed.
        assert!(s.contains("< /proc/mounts"));
    }

    #[test]
    fn init_neutralizes_nix_build_users() {
        let s = generate_init(&ep(), false, false, &[]);
        // Guarded on /nix/store so non-nix images are untouched, and exported
        // before the agent starts so `exec`/`shell` inherit it.
        assert!(
            s.contains("if [ -d /nix/store ]; then export NIX_CONFIG='build-users-group ='; fi")
        );
        let cfg = s.find("NIX_CONFIG").expect("NIX_CONFIG exported");
        let agent = s.find("/sbin/bsdkrun-agent").expect("agent started");
        assert!(cfg < agent, "NIX_CONFIG must be exported before the agent");
    }

    #[test]
    fn image_nix_config_is_respected() {
        let mut e = ep();
        e.env.push("NIX_CONFIG=cores = 4".into());
        let s = generate_init(&e, false, false, &[]);
        // Don't clobber a value the image (or `-e`) already set.
        assert!(!s.contains("export NIX_CONFIG='build-users-group ='"));
        assert!(s.contains("export NIX_CONFIG='cores = 4'"));
    }

    #[test]
    fn init_creates_dev_fd_symlinks() {
        let s = generate_init(&ep(), false, false, &[]);
        // udev/systemd never runs here, so /init must create the /dev/fd family
        // itself — without it bash process substitution `<(…)` yields an
        // unopenable /dev/fd/N and every nix stdenv build fails.
        for (link, target) in [
            ("/dev/fd", "/proc/self/fd"),
            ("/dev/stdin", "/proc/self/fd/0"),
            ("/dev/stdout", "/proc/self/fd/1"),
            ("/dev/stderr", "/proc/self/fd/2"),
        ] {
            // Guarded so images that already ship the link aren't disturbed.
            assert!(
                s.contains(&format!("[ -e {link} ] || ln -s {target} {link}")),
                "{link} -> {target}"
            );
        }
        // They are symlinks into /proc/self/fd, so /proc must be mounted first.
        let proc_pos = s.find("mount -t proc").expect("proc mount present");
        let fd_pos = s.find("ln -s /proc/self/fd /dev/fd").expect("/dev/fd link");
        assert!(proc_pos < fd_pos, "/proc must be mounted before /dev/fd");
    }
}
