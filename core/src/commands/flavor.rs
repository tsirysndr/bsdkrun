//! Flavors: the built-in catalog, user definitions in `flavors.toml`, saved
//! snapshots, and the build that turns a definition into a bootable rootfs.

use anyhow::Result;
use tracing::info;

use crate::cli::FlavorAddArgs;
use crate::{api, db, flavors, host};

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
    pub(crate) image: String,
    pub(crate) env: Vec<String>,
    pub(crate) ports: Vec<String>,
    pub(crate) nix: Vec<String>,
    pub(crate) provision: Vec<String>,
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
