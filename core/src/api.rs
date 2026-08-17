//! The engine's programmatic surface: what the subcommands do, as values.
//!
//! [`crate::commands`] prints; this module returns. Both go through the same
//! code, so `bsdkrun ps` and a daemon's `ListMachines` can never disagree —
//! which they could, and did, when the daemon's only way to ask was to run the
//! CLI and parse its `--json`.
//!
//! The types here are the canonical shapes. The CLI's `--json` output is
//! `serde_json` over exactly these structs, so the wire format has one
//! definition too.
//!
//! Booting is deliberately absent: it forks, and the child *becomes* the
//! machine. A caller that must not host VMs in its own process (the daemon)
//! runs [`crate::cli::Command`] in a supervisor process instead.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::commands::flavor::FLAVOR_BUILD_PREFIX;
use crate::commands::images::reconcile_bsd_images;
use crate::commands::volumes::volume_size;
use crate::{db, fetch, flavors, net};

// The write side. These live beside the subcommands that print their results,
// and are re-exported here so a caller has one place to look.
/// AI agent sandboxes. As with docker, the engine's types are already the wire
/// shapes, so nothing is re-declared here.
pub use crate::ai::{
    agents as ai_agents, sessions as ai_sessions, AgentInfo as AiAgent, Session as AiSession,
};
pub use crate::commands::flavor::{add_flavor, remove_flavor};
pub use crate::commands::guest::{guest_os_kind, interactive_term, is_bsd};
pub use crate::commands::images::remove_image;
pub use crate::commands::machines::{commit, remove_machine, stop, update};
pub use crate::commands::snapshot::{
    create as create_snapshot, remove as remove_snapshot, restore as restore_snapshot,
    rollback as rollback_snapshot, Restored,
};
pub use crate::commands::volumes::remove_volume;

/// Docker compatibility. The engine's types are already the wire shapes (they
/// are what `--json` prints), so nothing is re-declared here.
pub use crate::docker::{
    container_action as docker_container_action, container_logs as docker_container_logs,
    containers as docker_containers, status as docker_status, Container as DockerContainer,
    Status as DockerStatus,
};
pub use crate::network::{
    connect as connect_network, create as create_network, disconnect as disconnect_network,
    remove as remove_network, sync as sync_network,
};

/// A machine, with its recorded state reconciled against reality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub image: String,
    pub kind: String,
    pub command: String,
    /// "running" | "exited", after checking the pid is actually alive.
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i64>,
    pub pid: Option<i64>,
    pub detached: bool,
    pub cpus: Option<i64>,
    pub mem: Option<i64>,
    pub volume: Option<String>,
    pub state_dir: Option<String>,
    pub created_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub net_ip: Option<String>,
    /// Host↔guest TCP port forwards (`--port HOST:GUEST`), empty if none.
    #[serde(default)]
    pub ports: Vec<net::PortForward>,
    /// The snapshot this machine was branched from (`bsdkrun branch`), if any.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub id: String,
    pub reference: String,
    pub digest: Option<String>,
    pub size: i64,
    pub rootfs: Option<String>,
    pub created_at: Option<String>,
    /// Machines still using it. Only a dangling image (none) can be removed,
    /// so a UI can disable its delete button rather than let it fail.
    #[serde(default)]
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub guest: Option<String>,
    pub base: Option<String>,
    pub path: Option<String>,
    /// Human-readable ("2.3 GiB"), or `None` when it could not be measured.
    pub size: Option<String>,
    pub created_at: Option<String>,
    /// False for a directory found on disk that the database does not know
    /// about — created before volumes were recorded, or by hand.
    pub tracked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub name: String,
    pub subnet: String,
    pub gateway: String,
    pub members: i64,
    pub running: i64,
    pub up: bool,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flavor {
    pub name: String,
    /// "catalog" | "user" | "snapshot".
    pub source: String,
    /// "linux" | "freebsd" | "netbsd".
    pub kind: String,
    pub base: String,
    pub category: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub nix: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A machine snapshot: one machine's disk state, captured under a name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Short id (12 hex chars), like a machine's.
    pub id: String,
    pub name: String,
    pub machine_id: String,
    /// The machine's display name when the snapshot was taken. Empty if it had
    /// none — the snapshot outlives the machine, so this is a copy, not a join.
    pub machine_name: String,
    /// "linux" | "freebsd" | "netbsd" | "unikraft".
    pub kind: String,
    pub image: String,
    pub path: String,
    /// The snapshot the source machine was itself branched from, if any.
    pub parent: Option<String>,
    pub description: String,
    pub cpus: i64,
    pub mem: i64,
    pub ports: Vec<net::PortForward>,
    /// Human-readable ("2.3 GiB"), or `None` when it could not be measured.
    /// Snapshots are CoW clones, so this is what the data *would* cost, not
    /// what taking it cost.
    pub size: Option<String>,
    pub created_at: String,
}

/// Every snapshot, newest first; or one machine's when `machine` is given.
pub fn list_snapshots(machine: Option<&str>) -> Result<Vec<Snapshot>> {
    Ok(crate::commands::snapshot::list(machine)?
        .iter()
        .map(snapshot)
        .collect())
}

/// One snapshot by name, id, or unique id prefix.
pub fn find_snapshot(key: &str) -> Result<Option<Snapshot>> {
    Ok(db::Db::open()?.find_snapshot(key)?.map(|s| snapshot(&s)))
}

/// A stored snapshot row as the wire shape. Sizing is deliberately not done
/// here — `du` over a rootfs is slow, and a listing renders many rows.
pub fn snapshot(s: &db::SnapshotRow) -> Snapshot {
    Snapshot {
        id: s.id.clone(),
        name: s.name.clone(),
        machine_id: s.machine_id.clone(),
        machine_name: s.machine_name.clone(),
        kind: s.kind.clone(),
        image: s.image.clone(),
        path: s.path.clone(),
        parent: s.parent.clone(),
        description: s.description.clone(),
        cpus: s.cpus,
        mem: s.mem,
        ports: s.ports.as_deref().map(net::parse_ports).unwrap_or_default(),
        size: None,
        created_at: s.created_at.clone(),
    }
}

/// A downloadable BSD release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub version: String,
    /// The one to pick by default. For NetBSD that is `current`, not the
    /// newest number — the numbered releases boot without a root disk under
    /// libkrun (legacy virtio-mmio).
    pub latest: bool,
}

/// Every machine, newest first, with each row's `running` recomputed.
///
/// A row that claims to be running but whose process is gone is *corrected* in
/// the database on the way past, which is why this both reads and writes.
/// `all` includes stopped machines, like `ps -a`.
pub fn list_machines(all: bool) -> Result<Vec<Machine>> {
    let db = db::Db::open()?;
    let mut out = Vec::new();
    for m in db.list_machines()? {
        let running = m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false);
        if m.status == "running" && !running {
            db.set_machine_status(&m.id, "exited", m.exit_code).ok();
        }
        if !all && !running {
            continue;
        }
        out.push(Machine {
            id: m.id,
            name: m.name,
            image: m.image,
            kind: m.kind,
            command: m.command,
            status: if running { "running" } else { "exited" }.to_string(),
            running,
            exit_code: m.exit_code,
            pid: m.pid,
            detached: m.detached,
            cpus: Some(m.cpus),
            mem: Some(m.mem),
            volume: m.volume,
            state_dir: Some(m.state_dir),
            created_at: Some(m.created_at),
            finished_at: m.finished_at,
            network: m.network,
            net_ip: m.net_ip,
            ports: m.ports.as_deref().map(net::parse_ports).unwrap_or_default(),
            origin: m.origin,
        });
    }
    Ok(out)
}

/// Find a machine by id, name, or unique id prefix — the same three ways the
/// CLI accepts one on the command line.
pub fn find_machine(id: &str) -> Result<Option<Machine>> {
    let machines = list_machines(true)?;
    Ok(machines.into_iter().find(|m| {
        m.id == id || m.name.as_deref() == Some(id) || (!id.is_empty() && m.id.starts_with(id))
    }))
}

pub fn list_images() -> Result<Vec<Image>> {
    // Pick up any BSD disk images sitting in the cache that predate the
    // database, so a listing is complete however the image got here.
    reconcile_bsd_images();
    let db = db::Db::open()?;
    // One machine listing for the whole image list: `image_users` per image
    // would re-read the table once per row.
    let machines = db.list_machines().unwrap_or_default();
    Ok(db
        .list_images()?
        .into_iter()
        .map(|im| Image {
            used_by: machines
                .iter()
                .filter(|m| m.image == im.reference || m.image == im.id)
                .map(|m| m.name.clone().unwrap_or_else(|| m.id.clone()))
                .collect(),
            id: im.id,
            reference: im.reference,
            digest: Some(im.digest),
            size: im.size,
            rootfs: Some(im.rootfs),
            created_at: Some(im.created_at),
        })
        .collect())
}

/// Named volumes: the recorded ones, plus any untracked directory sitting in
/// the volumes dir. Reserved flavor-build volumes are left out — they are a
/// build cache, not something a user owns.
pub fn list_volumes() -> Result<Vec<Volume>> {
    let db = db::Db::open()?;
    let rows: Vec<_> = db
        .list_volumes()?
        .into_iter()
        .filter(|v| !v.name.starts_with(FLAVOR_BUILD_PREFIX))
        .collect();
    let tracked: std::collections::HashSet<String> = rows.iter().map(|v| v.name.clone()).collect();

    let mut out: Vec<Volume> = rows
        .into_iter()
        .map(|v| Volume {
            size: measured(volume_size(&v.path)),
            name: v.name,
            guest: Some(v.kind),
            base: Some(v.base),
            path: Some(v.path),
            created_at: Some(v.created_at),
            tracked: true,
        })
        .collect();

    if let Ok(entries) = std::fs::read_dir(db::volumes_dir()?) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir()
                && !tracked.contains(&name)
                && !name.starts_with(FLAVOR_BUILD_PREFIX)
            {
                let path = e.path().to_string_lossy().into_owned();
                out.push(Volume {
                    name,
                    guest: None,
                    base: None,
                    size: measured(volume_size(&path)),
                    path: Some(path),
                    created_at: None,
                    tracked: false,
                });
            }
        }
    }
    Ok(out)
}

/// `du` reports "-" when it cannot measure a volume; absence says that more
/// clearly to a caller than the placeholder does.
fn measured(size: String) -> Option<String> {
    Some(size).filter(|s| s != "-")
}

pub fn list_networks() -> Result<Vec<Network>> {
    let db = db::Db::open()?;
    let mut out = Vec::new();
    for n in db.list_networks()? {
        let members = db.network_members(&n.name).unwrap_or_default();
        let running = members
            .iter()
            .filter(|(_, _, pid)| pid.map(db::pid_alive).unwrap_or(false))
            .count();
        out.push(Network {
            name: n.name,
            subnet: n.subnet,
            gateway: n.gateway,
            members: members.len() as i64,
            running: running as i64,
            up: n.pid.map(db::pid_alive).unwrap_or(false),
            created_at: Some(n.created_at),
        });
    }
    Ok(out)
}

/// Everything bootable by name: this host's snapshots, the user's
/// `flavors.toml`, and the built-in catalog.
pub fn list_flavors() -> Result<Vec<Flavor>> {
    let db = db::Db::open()?;
    let mut out = Vec::new();
    for f in db.list_flavors().unwrap_or_default() {
        out.push(Flavor {
            // Normalize legacy boot-mode kinds (firmware/kernel) for display.
            kind: crate::commands::guest::guest_os_kind(&f.kind, &f.base).to_string(),
            name: f.name,
            source: "snapshot".to_string(),
            base: f.base,
            category: "snapshot".to_string(),
            method: "snapshot".to_string(),
            description: f.description,
            ports: vec![],
            nix: vec![],
            created_at: Some(f.created_at),
        });
    }
    for c in flavors::catalog() {
        out.push(Flavor {
            name: c.name.to_string(),
            source: "catalog".to_string(),
            kind: c.kind().to_string(),
            base: c.image().to_string(),
            category: c.category.to_string(),
            method: c.method().to_string(),
            description: c.description.to_string(),
            ports: c.ports.iter().map(|p| p.to_string()).collect(),
            nix: c.nix.iter().map(|n| n.to_string()).collect(),
            created_at: None,
        });
    }
    for u in flavors::user_flavors() {
        out.push(Flavor {
            kind: u.kind().to_string(),
            method: u.method().to_string(),
            name: u.name,
            source: "user".to_string(),
            base: u.base,
            category: u.category,
            description: u.description,
            ports: u.ports,
            nix: u.nix,
            created_at: None,
        });
    }
    Ok(out)
}

/// The releases `bsdkrun fetch` can download for `os`.
///
/// Falls back to the built-in list when the CDN is unreachable, so a version
/// picker never empties out just because a mirror is slow.
pub fn list_versions(os: fetch::Os) -> Vec<Version> {
    let versions = os.all_versions().unwrap_or_else(|_| os.fallback_versions());
    match os {
        fetch::Os::Freebsd => {
            let latest = versions.last().cloned().unwrap_or_default();
            versions
                .into_iter()
                .map(|v| Version {
                    latest: v == latest,
                    version: v,
                })
                .collect()
        }
        // `current` is the recommended NetBSD build, not the newest number: the
        // numbered releases boot but find no root disk under libkrun.
        fetch::Os::Netbsd => std::iter::once(Version {
            version: "current".to_string(),
            latest: true,
        })
        .chain(versions.into_iter().rev().map(|v| Version {
            version: v,
            latest: false,
        }))
        .collect(),
    }
}
