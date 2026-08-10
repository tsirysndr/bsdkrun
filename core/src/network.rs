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
    println!("{}", create(name)?);
    Ok(())
}

/// Create a network and start its shared gvproxy switch, returning its name.
///
/// The result is returned rather than printed so the daemon can put it on its
/// own transport instead of the daemon process's stdout.
pub fn create(name: &str) -> Result<String> {
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
    Ok(name.to_string())
}

/// `bsdkrun network ls` — list networks + their running member count.
#[allow(clippy::print_literal)]
pub fn cmd_ls(json: bool) -> Result<()> {
    let nets = crate::api::list_networks()?;
    if json {
        println!("{}", serde_json::to_string(&nets)?);
        return Ok(());
    }
    println!(
        "{:<18}  {:<20}  {:<9}  {:<7}  MEMBERS",
        "NAME", "SUBNET", "GATEWAY", "STATUS"
    );
    for n in &nets {
        let status = if n.up { "up" } else { "down" };
        println!(
            "{:<18}  {:<20}  {:<9}  {:<7}  {} running / {} total",
            n.name, n.subnet, n.gateway, status, n.running, n.members
        );
    }
    Ok(())
}

/// `bsdkrun network rm <name>…` — stop a network's gvproxy + delete it. Refuses a
/// network with running members unless `force`.
pub fn cmd_rm(names: &[String], force: bool) -> Result<()> {
    let mut failed = false;
    for name in names {
        match remove(name, force) {
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

/// Stop a network's gvproxy and delete it. Refuses one with running members
/// unless `force`.
pub fn remove(name: &str, force: bool) -> Result<String> {
    let db = db::Db::open()?;
    let Some(net_row) = db.find_network(name)? else {
        anyhow::bail!("no such network: {name}");
    };
    let running = db
        .network_members(name)?
        .into_iter()
        .filter(|(_, _, pid)| pid.map(db::pid_alive).unwrap_or(false))
        .count();
    if running > 0 && !force {
        anyhow::bail!("network {name:?} has {running} running member(s) — stop them or use -f");
    }
    if let Some(pid) = net_row.pid {
        if db::pid_alive(pid) {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
    }
    crate::host::remove_dir_all_detached(&PathBuf::from(&net_row.dir));
    db.remove_network(name).ok();
    Ok(name.to_string())
}

/// `bsdkrun network connect <machine> <network>` — join/switch a machine to a
/// network. Records membership + clears any prior IP; takes effect on the next
/// `start` (libkrun fixes a VM's devices at boot, so a running machine can't
/// hot-swap its NIC).
pub fn cmd_connect(machine: &str, network: &str) -> Result<()> {
    println!("{}", connect(machine, network)?);
    Ok(())
}

/// Join a machine to a network. Applies on the machine's next start, which the
/// returned message says when it is already running.
pub fn connect(machine: &str, network: &str) -> Result<String> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;
    if db.find_network(network)?.is_none() {
        anyhow::bail!(
            "no such network: {network} — create it with `bsdkrun network create {network}`"
        );
    }
    db.update_machine_network(&vm.id, Some(network))?;
    let running = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
    info!(machine = %vm.id, network, "connected machine to network");
    if running {
        return Ok(format!(
            "{}\nrestart the machine (`bsdkrun start {}`) to join {network}",
            vm.id, vm.id
        ));
    }
    Ok(vm.id)
}

/// Push a managed `/etc/hosts` block (every member's name → IP) to each running
/// **BSD** member of `network`, so peers resolve by name regardless of the DNS.
///
/// Why: `ping`/`getaddrinfo` do an A *and* AAAA lookup. gvproxy's DNS answers
/// the A but returns NXDOMAIN for the AAAA of an A-only name (should be NODATA);
/// NetBSD's resolver treats that as authoritative and fails the whole lookup, so
/// bare-name ping fails on NetBSD (Linux/FreeBSD are lenient). `/etc/hosts`
/// (nsswitch `files` before `dns`) sidesteps it. Best-effort per member.
pub fn sync_hosts(network: &str) -> Result<()> {
    let db = db::Db::open()?;
    let members = db.network_member_hosts(network)?;
    if members.is_empty() {
        return Ok(());
    }
    // Tag every managed line so the block can be replaced idempotently with just
    // grep/printf/cp (portable — no sed/awk needed on minimal guests).
    let tag = format!("# bsdkrun-net-{network}");
    let mut appends = String::new();
    for (name, ip, _, _, _) in &members {
        // name/ip/network are constrained to safe chars, so plain shell words.
        appends.push_str(&format!(
            "printf '%s %s %s.%s {tag}\\n' {ip} {name} {name} {network} >> /etc/hosts\n"
        ));
    }
    let script = format!(
        "tmp=/etc/hosts.bsdkrun.$$\n\
         grep -v '{tag}$' /etc/hosts > \"$tmp\" 2>/dev/null\n\
         [ -s \"$tmp\" ] && cat \"$tmp\" > /etc/hosts\n\
         rm -f \"$tmp\"\n\
         {appends}"
    );
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];

    let mut pushed = 0;
    for (name, _ip, state_dir, kind, pid) in &members {
        // Only BSD guests need this; Linux resolves fine via DNS (and a minimal
        // guest may lack the tools). Push to all BSD members so existing ones
        // also learn newly-joined peers.
        let is_bsd = matches!(kind.as_str(), "netbsd" | "kernel" | "freebsd" | "firmware");
        if !is_bsd || !pid.map(db::pid_alive).unwrap_or(false) {
            continue;
        }
        let Some(port) = agent::read_port(Path::new(state_dir)) else {
            continue;
        };
        match agent::exec_quiet(port, &argv) {
            Ok(_) => pushed += 1,
            Err(e) => warn!("hosts sync to {name} failed: {e:#}"),
        }
    }
    info!(
        network,
        members = members.len(),
        pushed,
        "synced /etc/hosts on BSD members"
    );
    Ok(())
}

/// `bsdkrun network sync <network>` — refresh every running member's `/etc/hosts`
/// with the current membership (fixes name resolution without restarting them).
pub fn cmd_sync(network: &str) -> Result<()> {
    println!("{}", sync(network)?);
    Ok(())
}

/// Rewrite a network's hosts file from its current membership.
pub fn sync(network: &str) -> Result<String> {
    let db = db::Db::open()?;
    if db.find_network(network)?.is_none() {
        anyhow::bail!("no such network: {network}");
    }
    sync_hosts(network)?;
    Ok(network.to_string())
}

/// `bsdkrun network disconnect <machine>` — detach a machine back to the default
/// isolated stack. Takes effect on the next `start`.
pub fn cmd_disconnect(machine: &str) -> Result<()> {
    println!("{}", disconnect(machine)?);
    Ok(())
}

/// Remove a machine from its network; applies on its next start.
pub fn disconnect(machine: &str) -> Result<String> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;
    db.update_machine_network(&vm.id, None)?;
    let running = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
    info!(machine = %vm.id, "disconnected machine from its network");
    if running {
        return Ok(format!(
            "{}\nrestart the machine (`bsdkrun start {}`) to leave the network",
            vm.id, vm.id
        ));
    }
    Ok(vm.id)
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
        // On a plain restart, `cmd_start` hints the previously-assigned IP so the
        // machine keeps its address; otherwise allocate the next free one.
        let ip = match std::env::var("BSDKRUN_NET_PREF_IP")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(pref) => pref,
            None => allocate_ip(&db, network)?,
        };
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
    net::expose_on_control(
        &control,
        std::net::Ipv4Addr::LOCALHOST.into(),
        host,
        &ip,
        agent::GUEST_PORT,
    )
    .context("forwarding the agent port on the network")?;
    let _ = std::fs::write(agent::port_file(agent_dir), host.to_string());
    for pf in ports {
        let _ = net::expose_on_control(&control, pf.bind, pf.host, &ip, pf.guest);
    }
    if let Err(e) = net::dns_add(&control, network, member, &ip) {
        warn!("couldn't register {member} in {network} DNS: {e:#}");
    }

    // Add a `search <network>` domain to the guest's resolv.conf so bare peer
    // names (e.g. `ping db`) resolve against the network's DNS zone — the BSD
    // side of the symmetry Linux members already get (see `linux::init_script`).
    // The lease appears (DHCP) before the guest's agent is serving, so wait for
    // agent readiness first — otherwise the exec lands on gvproxy's accepted-but-
    // unanswered socket and silently no-ops. Best-effort: FQDNs work regardless.
    for _ in 0..60 {
        if agent::ping(host) {
            inject_search_domain(host, network);
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let _ = db.set_machine_network(machine_id, network, &ip);
    info!(network, %ip, member, "BSD member joined the network");
    Ok(())
}

/// Ensure `search <network>` is in the guest's `/etc/resolv.conf`, over the
/// forwarded agent port. Idempotent and best-effort — logs on failure rather
/// than failing the boot (FQDN resolution works without it).
fn inject_search_domain(agent_port: u16, network: &str) {
    // `network` is validated to `[A-Za-z0-9._-]` (see `valid_name`), so it's
    // safe to interpolate into this shell snippet.
    //
    // FreeBSD (and NetBSD/dhcpcd) manage /etc/resolv.conf through resolvconf(8),
    // which regenerates the file on every DHCP/tailscale event — a plain append
    // gets clobbered. So when resolvconf is present, register the domain in
    // /etc/resolvconf.conf (prepending to any existing `search_domains`, which
    // wins as the last assignment when the file is sourced) and regenerate.
    // Otherwise fall back to appending directly. resolvconf lives in /sbin, which
    // isn't on the agent's early-boot PATH, so probe absolute paths + call by path.
    let script = format!(
        "set -e\n\
         net={network}\n\
         rc=\n\
         for p in /sbin/resolvconf /usr/sbin/resolvconf; do [ -x \"$p\" ] && rc=$p && break; done\n\
         if [ -n \"$rc\" ]; then\n\
         \tgrep -q \"$net\" /etc/resolvconf.conf 2>/dev/null || \
         printf 'search_domains=\"%s ${{search_domains}}\"\\n' \"$net\" >> /etc/resolvconf.conf\n\
         \t\"$rc\" -u 2>/dev/null || true\n\
         else\n\
         \tgrep -q \"^search $net\\$\" /etc/resolv.conf 2>/dev/null || \
         printf 'search %s\\n' \"$net\" >> /etc/resolv.conf\n\
         fi"
    );
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];
    match agent::exec_quiet(agent_port, &argv) {
        Ok(0) => info!(network, "added search domain to guest resolv.conf"),
        Ok(code) => warn!("resolv.conf search-domain command exited {code}"),
        Err(e) => warn!("couldn't set search domain in {network}: {e:#}"),
    }
}
