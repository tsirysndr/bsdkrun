//! Global networks — an optional shared L2 subnet machines can join to reach
//! each other by IP and by name (docker-compose-style service discovery).
//!
//! A network is one long-lived **shared gvproxy** switch (a control socket with
//! a `/connect` endpoint). Members join it via a per-machine frame bridge (see
//! [`crate::net::start_network_bridge`]) instead of spawning their own isolated
//! gvproxy, so they share `192.168.127.0/24`: the gateway is `.1`, members get
//! `.2`, `.3`, …. Internal DNS uses the gvproxy's own resolver (guests already
//! query `.1`), with a zone named for the network.
//!
//! Without `--network`, machines keep the default isolated per-machine gvproxy.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::net::PortForward;
use crate::{agent, db, net};

/// The shared subnet every network uses (each network has its own gvproxy, so
/// they don't collide). Gateway `.1`; members are handed `.2`..`.254`.
const SUBNET: &str = "192.168.127.0/24";
const GATEWAY: &str = "192.168.127.1";

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `bsdkrun network create <name>` — create a network + start its shared gvproxy.
pub fn cmd_create(name: &str) -> Result<()> {
    if !valid_name(name) {
        anyhow::bail!("invalid network name {name:?} — use letters, digits, '-', '_' or '.'");
    }
    let db = db::Db::open()?;
    if db.find_network(name)?.is_some() {
        anyhow::bail!("network {name:?} already exists");
    }
    let dir = db::networks_dir()?.join(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let control = dir.join("control.sock");
    let pid = spawn_gvproxy(&dir, &control)?;
    db.create_network(
        name,
        SUBNET,
        GATEWAY,
        &control.to_string_lossy(),
        &dir.to_string_lossy(),
        Some(pid as i64),
    )?;
    info!(
        network = name,
        subnet = SUBNET,
        gateway = GATEWAY,
        "created network"
    );
    println!("{name}");
    Ok(())
}

/// `bsdkrun network ls` — list networks + their running member count.
#[allow(clippy::print_literal)]
pub fn cmd_ls(json: bool) -> Result<()> {
    let db = db::Db::open()?;
    let nets = db.list_networks()?;
    if json {
        let mut out = Vec::new();
        for n in &nets {
            let members = db.network_members(&n.name).unwrap_or_default();
            let running = members
                .iter()
                .filter(|(_, _, pid)| pid.map(db::pid_alive).unwrap_or(false))
                .count();
            out.push(serde_json::json!({
                "name": n.name, "subnet": n.subnet, "gateway": n.gateway,
                "members": members.len(), "running": running,
                "up": n.pid.map(db::pid_alive).unwrap_or(false),
                "created_at": n.created_at,
            }));
        }
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    println!(
        "{:<18}  {:<20}  {:<9}  {:<7}  MEMBERS",
        "NAME", "SUBNET", "GATEWAY", "STATUS"
    );
    for n in &nets {
        let members = db.network_members(&n.name).unwrap_or_default();
        let running = members
            .iter()
            .filter(|(_, _, pid)| pid.map(db::pid_alive).unwrap_or(false))
            .count();
        let status = if n.pid.map(db::pid_alive).unwrap_or(false) {
            "up"
        } else {
            "down"
        };
        println!(
            "{:<18}  {:<20}  {:<9}  {:<7}  {} running / {} total",
            n.name,
            n.subnet,
            n.gateway,
            status,
            running,
            members.len()
        );
    }
    Ok(())
}

/// `bsdkrun network rm <name>…` — stop a network's gvproxy + delete it. Refuses a
/// network with running members unless `force`.
pub fn cmd_rm(names: &[String], force: bool) -> Result<()> {
    let db = db::Db::open()?;
    let mut failed = false;
    for name in names {
        let Some(net_row) = db.find_network(name)? else {
            eprintln!("Error: no such network: {name}");
            failed = true;
            continue;
        };
        let running = db
            .network_members(name)?
            .into_iter()
            .filter(|(_, _, pid)| pid.map(db::pid_alive).unwrap_or(false))
            .count();
        if running > 0 && !force {
            eprintln!(
                "Error: network {name:?} has {running} running member(s) — stop them or use -f"
            );
            failed = true;
            continue;
        }
        if let Some(pid) = net_row.pid {
            if db::pid_alive(pid) {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }
        }
        crate::host::remove_dir_all_detached(&PathBuf::from(&net_row.dir));
        db.remove_network(name).ok();
        println!("{name}");
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// Spawn a detached, long-lived gvproxy for a network: a control socket that
/// serves the `/connect` switch (members bridge into it) + the DNS/forwarder API.
/// No VM listener — members join via `/connect`. Survives this process exiting.
fn spawn_gvproxy(dir: &Path, control: &Path) -> Result<u32> {
    use std::os::unix::process::CommandExt;
    let bin = net::locate()?;
    let ssh = net::free_local_port()?; // gvproxy insists on a real ssh-port
    let _ = std::fs::remove_file(control);
    let log = std::fs::File::create(dir.join("gvproxy.log"))
        .with_context(|| format!("creating {}", dir.join("gvproxy.log").display()))?;
    let mut cmd = Command::new(&bin);
    cmd.arg("-ssh-port")
        .arg(ssh.to_string())
        .arg("-listen")
        .arg(format!("unix://{}", control.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // Detach into its own session so it outlives the bsdkrun invocation.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawning the network gvproxy ({})", bin.display()))?;
    let pid = child.id();
    std::mem::forget(child); // a daemon we track by pid — don't reap/kill on drop

    for _ in 0..50 {
        if control.exists() {
            return Ok(pid);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("the network gvproxy didn't create its control socket in time");
}

/// Ensure a network's gvproxy is running (respawn if its pid died), returning the
/// control socket path.
fn ensure_running(db: &db::Db, net_row: &db::NetworkRow) -> Result<PathBuf> {
    let control = PathBuf::from(&net_row.control_socket);
    let alive = net_row.pid.map(db::pid_alive).unwrap_or(false) && control.exists();
    if alive {
        return Ok(control);
    }
    let dir = PathBuf::from(&net_row.dir);
    let pid = spawn_gvproxy(&dir, &control)?;
    db.set_network_pid(&net_row.name, Some(pid as i64))?;
    info!(network = %net_row.name, "restarted the network gvproxy");
    Ok(control)
}

/// Allocate the next free member IP (`.2`..`.254`) on a network.
fn allocate_ip(db: &db::Db, network: &str) -> Result<String> {
    let used: std::collections::HashSet<String> = db
        .network_members(network)?
        .into_iter()
        .map(|(_, ip, _)| ip)
        .collect();
    for last in 2u8..=254 {
        let ip = format!("192.168.127.{last}");
        if !used.contains(&ip) {
            return Ok(ip);
        }
    }
    anyhow::bail!("network {network} is full (no free IPs in {SUBNET})");
}

/// A stable, distinct MAC for a network member (derived from its name — every
/// member on the shared switch needs a unique MAC, and gvproxy's DHCP keys leases
/// by MAC). Locally-administered (`5a:…`).
fn member_mac(member: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    member.hash(&mut h);
    let b = h.finish().to_le_bytes();
    format!("5a:94:ef:{:02x}:{:02x}:{:02x}", b[0], b[1], b[2])
}

/// Join a machine (named `member`) to `network` before it boots: ensure the
/// gvproxy is up and set the env the boot path reads (`BSDKRUN_NET_CONTROL`/
/// `_NAME`/`_MAC`). A **static** guest (Linux) also gets an allocated IP
/// (`BSDKRUN_NET_IP`) + DNS record here; a **dhcp** guest (BSD) discovers its IP
/// after boot (see [`finalize_dhcp`]).
pub fn join(network: &str, member: &str, dhcp: bool) -> Result<()> {
    let db = db::Db::open()?;
    let net_row = db.find_network(network)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no such network: {network} — create it with `bsdkrun network create {network}`"
        )
    })?;
    let control = ensure_running(&db, &net_row)?;

    std::env::set_var("BSDKRUN_NET_CONTROL", &control);
    std::env::set_var("BSDKRUN_NET_NAME", network);
    std::env::set_var("BSDKRUN_NET_MAC", member_mac(member));

    if dhcp {
        // BSD DHCPs its IP; DNS + forwards are wired post-boot from the lease.
        std::env::remove_var("BSDKRUN_NET_IP");
        info!(network, member, "joining network (DHCP)");
    } else {
        let ip = allocate_ip(&db, network)?;
        std::env::set_var("BSDKRUN_NET_IP", &ip);
        if let Err(e) = net::dns_add(&control, network, member, &ip) {
            warn!("couldn't register {member} in {network} DNS: {e:#}");
        }
        info!(network, %ip, member, "joining network");
    }
    Ok(())
}

/// Finalize a **DHCP** (BSD) member once it's booted: discover its leased IP,
/// forward its agent (and any `--port`s) to it, register it in DNS, and record
/// membership. Blocks briefly while the guest DHCPs.
pub fn finalize_dhcp(
    network: &str,
    member: &str,
    machine_id: &str,
    agent_dir: &Path,
    ports: &[PortForward],
) -> Result<()> {
    let db = db::Db::open()?;
    let net_row = db
        .find_network(network)?
        .ok_or_else(|| anyhow::anyhow!("network {network} vanished"))?;
    let control = PathBuf::from(&net_row.control_socket);
    let mac = std::env::var("BSDKRUN_NET_MAC").unwrap_or_default();

    // Wait for the guest to DHCP a lease (BSD reaches this a few seconds in).
    let mut leased = None;
    for _ in 0..80 {
        if let Ok(Some(ip)) = net::lease_ip_for_mac(&control, &mac) {
            leased = Some(ip);
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let ip = leased
        .ok_or_else(|| anyhow::anyhow!("{member} didn't get a DHCP lease on {network} in time"))?;

    // Forward the agent (for exec/shell) + any user ports to the leased IP.
    let host = net::free_local_port()?;
    net::expose_on_control(&control, host, &ip, agent::GUEST_PORT)
        .context("forwarding the agent port on the network")?;
    let _ = std::fs::write(agent::port_file(agent_dir), host.to_string());
    for pf in ports {
        let _ = net::expose_on_control(&control, pf.host, &ip, pf.guest);
    }
    if let Err(e) = net::dns_add(&control, network, member, &ip) {
        warn!("couldn't register {member} in {network} DNS: {e:#}");
    }
    let _ = db.set_machine_network(machine_id, network, &ip);
    info!(network, %ip, member, "BSD member joined the network");
    Ok(())
}
