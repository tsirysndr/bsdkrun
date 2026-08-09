//! Flavors: the built-in catalog, user definitions in `flavors.toml`, saved
//! snapshots, and the build that turns a definition into a bootable rootfs.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::*;
use crate::net::PortForward;
use crate::{api, db, flavors, host, linux};

use super::boot::{
    boot_freebsd, boot_freebsd_disk, boot_linux_from, boot_netbsd, boot_netbsd_disk,
    repo_clone_argv, volume_dir,
};
use super::guest::guest_os_kind;

pub(crate) fn flavor_linux_args(
    image: String,
    detach: bool,
    cpus: u8,
    mem: u32,
    volume: Option<String>,
    ports: Vec<PortForward>,
    env: Vec<String>,
) -> LinuxArgs {
    LinuxArgs {
        image,
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach,
        initramfs: false,
        volume,
        mounts: vec![],
        entrypoint: None,
        env,
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig { cpus, mem },
        repo: None,
        command: vec![],
    }
}

/// Validate a flavor/snapshot name (used as a directory + DB key).
pub(crate) fn valid_flavor_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!("invalid flavor name {name:?} — use letters, digits, '-', '_' or '.'");
    }
    if flavors::find(name).is_some() {
        anyhow::bail!("{name:?} is a built-in catalog flavor name — pick another");
    }
    Ok(())
}

/// `bsdkrun flavors` — list saved snapshots + the built-in catalog.
#[allow(clippy::print_literal)]
pub(crate) fn cmd_flavors(json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(&api::list_flavors()?)?);
        return Ok(());
    }
    let db = db::Db::open()?;
    let snapshots = db.list_flavors().unwrap_or_default();
    let user = flavors::user_flavors();
    if !snapshots.is_empty() {
        println!("Your snapshots:");
        for f in &snapshots {
            println!("  {:<18}  {:<9}  {}", f.name, f.kind, f.description);
        }
        println!();
    }
    if !user.is_empty() {
        println!("Your flavors (flavors.toml):");
        println!(
            "  {:<14}  {:<9}  {:<8}  {}",
            "NAME", "CATEGORY", "METHOD", "DESCRIPTION"
        );
        for u in &user {
            println!(
                "  {:<14}  {:<9}  {:<8}  {}",
                u.name,
                u.category,
                u.method(),
                u.description
            );
        }
        println!();
    }
    println!("Catalog:");
    println!(
        "  {:<14}  {:<9}  {:<8}  {}",
        "NAME", "CATEGORY", "METHOD", "DESCRIPTION"
    );
    for c in flavors::catalog() {
        println!(
            "  {:<14}  {:<9}  {:<8}  {}",
            c.name,
            c.category,
            c.method(),
            c.description
        );
    }
    Ok(())
}

/// `bsdkrun flavor rm <name>...` — remove saved snapshot flavors.
pub(crate) fn cmd_flavor_rm(names: &[String], _force: bool) -> Result<()> {
    let mut failed = false;
    for name in names {
        match remove_flavor(name) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("Error: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// Remove one saved snapshot, or a user flavor from `flavors.toml`. Built-in
/// catalog entries are not removable.
pub fn remove_flavor(name: &str) -> Result<String> {
    if flavors::find(name).is_some() {
        anyhow::bail!("{name:?} is a built-in catalog flavor (can't remove)");
    }
    let db = db::Db::open()?;
    match db.find_flavor(name)? {
        Some(f) => {
            // Rename-aside + background GC so a large (nix) snapshot rootfs
            // doesn't make the delete hang, same as `rm`.
            host::remove_dir_all_detached(&std::path::PathBuf::from(&f.path));
            db.remove_flavor(name).ok();
            Ok(name.to_string())
        }
        // Not a snapshot — try a user (flavors.toml) flavor.
        None => match flavors::remove_user_flavor(name)? {
            true => Ok(name.to_string()),
            false => anyhow::bail!("no such flavor: {name}"),
        },
    }
}

/// `bsdkrun flavor add <name> --base <ref> …` — define a custom flavor in the
/// writable `flavors.toml`.
pub(crate) fn cmd_flavor_add(a: FlavorAddArgs) -> Result<()> {
    println!("{}", add_flavor(a)?);
    Ok(())
}

/// Define a custom flavor in the writable `flavors.toml`, returning its name.
pub fn add_flavor(a: FlavorAddArgs) -> Result<String> {
    let ok = !a.name.is_empty()
        && a.name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        anyhow::bail!(
            "invalid flavor name {:?} — use letters, digits, '-', '_' or '.'",
            a.name
        );
    }
    if a.base.trim().is_empty() {
        anyhow::bail!("--base is required (an OCI image ref, or `freebsd`/`netbsd`)");
    }
    let name = a.name.clone();
    let flavor = flavors::UserFlavor {
        name: a.name,
        category: if a.category.is_empty() {
            "custom".into()
        } else {
            a.category
        },
        description: a.description,
        base: a.base,
        ports: a.ports,
        env: a.env,
        nix: a.nix,
        provision: a.provision,
    };
    let path = flavors::upsert_user_flavor(flavor)?;
    info!(flavor = %name, file = %path.display(), "saved flavor");
    Ok(name)
}

/// A resolved Linux flavor: the base image plus its defaults and provisioning
/// steps, from either the built-in catalog or a user `flavors.toml`.
pub(crate) struct LinuxFlavorSpec {
    image: String,
    env: Vec<String>,
    ports: Vec<String>,
    nix: Vec<String>,
    provision: Vec<String>,
}

/// Resolve a Linux flavor (catalog or user) by name. Returns `None` for a BSD
/// flavor or an unknown name.
pub(crate) fn resolve_linux_flavor(name: &str) -> Option<LinuxFlavorSpec> {
    if let Some(c) = flavors::find(name) {
        if c.kind() != "linux" {
            return None;
        }
        return Some(LinuxFlavorSpec {
            image: c.image().to_string(),
            env: c.env.iter().map(|s| s.to_string()).collect(),
            ports: c.ports.iter().map(|s| s.to_string()).collect(),
            nix: c.nix.iter().map(|s| s.to_string()).collect(),
            provision: c.provision.iter().map(|s| s.to_string()).collect(),
        });
    }
    let u = flavors::find_user(name)?;
    if u.kind() != "linux" {
        return None;
    }
    Some(LinuxFlavorSpec {
        image: u.base.clone(),
        env: u.env,
        ports: u.ports,
        nix: u.nix,
        provision: u.provision,
    })
}

/// A stable short cache key for a flavor build, derived from the base image ref
/// and the exact provisioning steps — like a Docker build cache keyed by its
/// instructions. Any change to the base or a step yields a new key (cache miss).
pub(crate) fn flavor_build_key(image: &str, nix: &[String], provision: &[String]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    image.hash(&mut h);
    0xFFu8.hash(&mut h);
    for n in nix {
        n.hash(&mut h);
        0x01u8.hash(&mut h);
    }
    0xFEu8.hash(&mut h);
    for p in provision {
        p.hash(&mut h);
        0x02u8.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// The reserved volume that holds a built flavor's provisioned rootfs (the
/// "cache layer"). Hidden from `volume ls`.
pub const FLAVOR_BUILD_PREFIX: &str = "bsdkrun-build-";
pub(crate) fn flavor_build_volume(key: &str) -> String {
    format!("{FLAVOR_BUILD_PREFIX}{key}")
}

/// A short filesystem-safe label for a flavor name (for guest `echo`s).
pub(crate) fn safe_label(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

/// Build the single guest command that provisions a flavor: Nix package installs
/// (via the Determinate Systems installer on the OCI base) then the flavor's
/// shell steps, in order, under a login shell. `None` ⇒ nothing to provision.
pub(crate) fn flavor_provision_argv(
    name: &str,
    nix: &[String],
    provision: &[String],
) -> Option<Vec<String>> {
    if nix.is_empty() && provision.is_empty() {
        return None;
    }
    let label = safe_label(name);
    let mut lines: Vec<String> = vec![format!("echo '==> provisioning {label}'")];
    if !nix.is_empty() {
        lines.push("echo '==> installing Nix (Determinate Systems)'".into());
        lines.push(
            "command -v nix >/dev/null 2>&1 || curl --proto '=https' --tlsv1.2 -sSf -L \
             https://install.determinate.systems/nix | sh -s -- install linux \
             --no-confirm --init none"
                .into(),
        );
        lines.push(
            ". /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh 2>/dev/null || \
             export PATH=/nix/var/nix/profiles/default/bin:$PATH"
                .into(),
        );
        let pkgs = nix
            .iter()
            .map(|p| format!("nixpkgs#{p}"))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!("nix profile install {pkgs}"));
    }
    for step in provision {
        lines.push(step.clone());
    }
    lines.push(format!("echo '==> {label} ready'"));
    Some(vec!["sh".into(), "-lc".into(), lines.join("\n")])
}

/// Ensure a flavor's provisioned rootfs is built and cached, returning its path.
/// Cache hit → returns immediately (instant, no re-provisioning). Miss → runs
/// the provisioning build in a child `bsdkrun` process (streaming its progress)
/// and records the result so every later launch just clones it.
pub(crate) fn ensure_flavor_built(
    spec: &LinuxFlavorSpec,
    name: &str,
    cpus: u8,
    mem: u32,
) -> Result<PathBuf> {
    let key = flavor_build_key(&spec.image, &spec.nix, &spec.provision);
    let vol = flavor_build_volume(&key);
    let voldir = volume_dir(&vol)?;
    let rootfs = voldir.join("rootfs");
    let marker = voldir.join(".provisioned");
    if marker.exists() && rootfs.exists() {
        info!(flavor = name, key = %key, "using cached flavor build");
        return Ok(rootfs);
    }
    // Cache miss: build in a CHILD process. Provisioning ends in `process::exit`
    // (see `run_guest_command`), so it must not run in this process — we need to
    // survive it to clone + boot the real machine afterwards.
    info!(flavor = name, key = %key, "building flavor (first launch)…");
    host::force_remove_dir_all(&voldir); // clear any half-built remnant
    let exe = std::env::current_exe().context("locating bsdkrun for the flavor build")?;
    let status = std::process::Command::new(exe)
        .args([
            "flavor",
            "__build",
            name,
            "--key",
            &key,
            "--cpus",
            &cpus.to_string(),
            "--mem",
            &mem.to_string(),
        ])
        .status()
        .context("spawning the flavor build")?;
    if !status.success() {
        host::force_remove_dir_all(&voldir);
        anyhow::bail!("provisioning {name} failed (see the output above)");
    }
    if !rootfs.exists() {
        host::force_remove_dir_all(&voldir);
        anyhow::bail!("the flavor build produced no rootfs for {name}");
    }
    std::fs::write(&marker, key.as_bytes()).ok();
    Ok(rootfs)
}

/// Hidden `bsdkrun flavor __build` — the child that provisions a flavor into its
/// build volume, then powers the builder off. Not for direct use.
pub(crate) fn cmd_flavor_build(name: &str, key: &str, cpus: u8, mem: u32) -> Result<()> {
    let spec = resolve_linux_flavor(name)
        .ok_or_else(|| anyhow::anyhow!("no such Linux flavor to build: {name}"))?;
    let argv = flavor_provision_argv(name, &spec.nix, &spec.provision)
        .ok_or_else(|| anyhow::anyhow!("{name} has nothing to provision"))?;
    let vol = flavor_build_volume(key);

    // Boot a builder whose root is the persistent build volume. A trivial
    // keep-alive is the main process so the VM (and its agent) stay up on any
    // base image while provisioning runs; `run_machine` powers it off when the
    // provisioning command finishes (detach=false ⇒ keep_running=false).
    let largs = LinuxArgs {
        image: spec.image.clone(),
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach: false,
        initramfs: false,
        volume: Some(vol),
        mounts: vec![],
        entrypoint: None,
        env: spec.env.clone(),
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports: vec![],
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig { cpus, mem },
        repo: None,
        command: vec![
            "sh".into(),
            "-c".into(),
            "while :; do sleep 86400; done".into(),
        ],
    };
    boot_linux_from(largs, None, &argv)
}

/// `bsdkrun flavor build <name>` — pre-build a flavor's provisioned rootfs into
/// the cache so a later `run` is instant. Streams provisioning output.
pub(crate) fn cmd_flavor_prebuild(name: &str, cpus: u8, mem: u32, force: bool) -> Result<()> {
    let Some(spec) = resolve_linux_flavor(name) else {
        anyhow::bail!("no such Linux flavor to build: {name} (see `bsdkrun flavors`)");
    };
    if spec.nix.is_empty() && spec.provision.is_empty() {
        println!("{name}: nothing to build (no provisioning steps)");
        return Ok(());
    }
    if force {
        // Drop the cached build so it's rebuilt from scratch.
        let key = flavor_build_key(&spec.image, &spec.nix, &spec.provision);
        if let Ok(dir) = volume_dir(&flavor_build_volume(&key)) {
            host::force_remove_dir_all(&dir);
        }
    }
    let built = ensure_flavor_built(&spec, name, cpus, mem)?;
    info!(flavor = name, rootfs = %built.display(), "flavor built");
    println!("{name}");
    Ok(())
}

/// `bsdkrun flavor run <name>` — boot a machine from a catalog/user flavor or a
/// saved snapshot. Provisioned flavors are built once (cached) then cloned.
pub(crate) fn cmd_flavor_run(args: FlavorRunArgs) -> Result<()> {
    let db = db::Db::open()?;

    // Optional `--repo` clones a repo into the machine after boot (cd on shell).
    let repo_argv = args
        .repo
        .as_deref()
        .and_then(repo_clone_argv)
        .unwrap_or_default();

    // A saved snapshot (from `commit`) wins over any catalog/user name.
    if let Some(f) = db.find_flavor(&args.name)? {
        // Normalize (old snapshots stored the boot mode: firmware/kernel).
        let osk = guest_os_kind(&f.kind, &f.base);
        if osk == "linux" {
            let rootfs = std::path::PathBuf::from(&f.path).join("rootfs");
            if !rootfs.exists() {
                anyhow::bail!("snapshot {:?} is missing its rootfs data", f.name);
            }
            let largs = flavor_linux_args(
                f.base.clone(),
                args.detach,
                args.vm.cpus,
                args.vm.mem,
                args.volume,
                args.ports,
                vec![],
            );
            return boot_linux_from(largs, Some(rootfs), &repo_argv);
        }

        // BSD snapshot: boot from its saved root disk (`disk.raw` / `disk.img`).
        let disk = ["disk.raw", "disk.img"]
            .iter()
            .map(|n| std::path::PathBuf::from(&f.path).join(n))
            .find(|p| p.exists())
            .ok_or_else(|| anyhow::anyhow!("snapshot {:?} is missing its disk data", f.name))?;
        let bargs = BsdArgs {
            version: None,
            firmware: None,
            force: false,
            attach_disk: vec![],
            disk_size: None,
            run: RunConfig {
                detach: args.detach,
                persist: false,
                volume: args.volume,
            },
            net: NetConfig {
                no_net: false,
                ports: args.ports,
                mac: None,
                network: None,
                name: None,
            },
            vm: VmConfig {
                cpus: args.vm.cpus,
                mem: args.vm.mem,
            },
            verbose: false,
            repo: None,
            command: repo_argv,
        };
        return if osk == "netbsd" {
            boot_netbsd_disk(bargs, Some(disk))
        } else {
            boot_freebsd_disk(bargs, Some(disk))
        };
    }

    // A Linux flavor (catalog or user): build-once-then-clone.
    if let Some(spec) = resolve_linux_flavor(&args.name) {
        let mut ports: Vec<PortForward> = args.ports;
        for p in &spec.ports {
            if let Ok(pf) = p.parse::<PortForward>() {
                ports.push(pf);
            }
        }
        let largs = flavor_linux_args(
            spec.image.clone(),
            args.detach,
            args.vm.cpus,
            args.vm.mem,
            args.volume,
            ports,
            spec.env.clone(),
        );
        // Provisioned flavors boot from a cached, pre-provisioned rootfs; plain
        // ones boot the base image directly.
        let has_provisioning = !spec.nix.is_empty() || !spec.provision.is_empty();
        if has_provisioning {
            let built = ensure_flavor_built(&spec, &args.name, args.vm.cpus, args.vm.mem)?;
            return boot_linux_from(largs, Some(built), &repo_argv);
        }
        return boot_linux_from(largs, None, &repo_argv);
    }

    // A BSD catalog flavor (no provisioning/cache — boots the bundled image).
    let Some(c) = flavors::find(&args.name) else {
        anyhow::bail!("no such flavor: {} (see `bsdkrun flavors`)", args.name);
    };
    let mut ports: Vec<PortForward> = args.ports;
    for p in c.ports {
        if let Ok(pf) = p.parse::<PortForward>() {
            ports.push(pf);
        }
    }
    let bargs = BsdArgs {
        version: None,
        firmware: None,
        force: false,
        attach_disk: vec![],
        disk_size: None,
        run: RunConfig {
            detach: args.detach,
            persist: false,
            volume: args.volume,
        },
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig {
            cpus: args.vm.cpus,
            mem: args.vm.mem,
        },
        verbose: false,
        repo: None,
        // On BSD the post-boot command IS the repo clone (if any).
        command: repo_argv,
    };
    match c.base {
        flavors::Base::Freebsd => boot_freebsd(bargs),
        flavors::Base::Netbsd => boot_netbsd(bargs),
        flavors::Base::Oci(_) => unreachable!("OCI flavors handled above"),
    }
}
