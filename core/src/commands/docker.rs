//! `bsdkrun docker` — the Docker Desktop replacement, as commands.
//!
//! The engine lives in [`crate::docker`]; this prints. The one exception is
//! `start`, which ends in a boot and therefore lives in
//! [`crate::commands::boot`], the same split the flavor commands use.

use anyhow::Result;

use crate::docker::{self, Container, Status};

/// `bsdkrun docker status` — is the engine up, and how do I talk to it?
pub(crate) fn cmd_status(json: bool) -> Result<()> {
    let s = docker::status()?;
    if json {
        println!("{}", serde_json::to_string(&s)?);
        return Ok(());
    }
    if !s.running {
        println!("Docker engine: not running");
        if s.machine_id.is_some() && !s.machine_running {
            println!("  the VM exists but is stopped — `bsdkrun docker start` resumes it");
        } else if s.machine_id.is_none() {
            println!("  start one with: bsdkrun docker start");
        } else {
            println!("  the VM is up but dockerd is not answering yet — try again in a moment");
        }
        return Ok(());
    }
    println!("Docker engine: running");
    println!("  version    {}", s.version.as_deref().unwrap_or("?"));
    println!(
        "  machine    {} ({} containers, {} images)",
        s.machine_id.as_deref().unwrap_or("?"),
        s.containers.unwrap_or(0),
        s.images.unwrap_or(0)
    );
    println!("  socket     {}", s.socket);
    if let Some(p) = s.api_port {
        println!(
            "  api port   127.0.0.1:{p} → guest {}",
            docker::GUEST_API_PORT
        );
    }
    if !s.mounts.is_empty() {
        println!("  shared     {}", s.mounts.join(", "));
    }
    println!(
        "  context    {}",
        match (s.context, s.context_active) {
            (true, true) => format!("{} (active)", docker::CONTEXT),
            (true, false) => format!(
                "{} (run `docker context use {}`)",
                docker::CONTEXT,
                docker::CONTEXT
            ),
            _ => format!("not configured — export DOCKER_HOST=unix://{}", s.socket),
        }
    );
    if s.system_socket {
        println!("  {} → this engine", docker::SYSTEM_SOCKET);
    }
    Ok(())
}

/// `bsdkrun docker stop` — stop the proxy and power the VM off.
///
/// The VM is stopped, not removed: images, volumes and containers live on its
/// disk, and a `start` brings them all back.
pub(crate) fn cmd_stop() -> Result<()> {
    docker::stop_proxy()?;
    let socket = docker::socket_path()?;
    docker::release_system_socket(&socket);
    match docker::machine()? {
        Some(vm) => {
            println!("{}", super::machines::stop(&vm.id)?);
            println!("Docker engine stopped (`bsdkrun docker start` resumes it).");
        }
        None => println!("No Docker VM to stop."),
    }
    Ok(())
}

/// `bsdkrun docker rm` — remove the VM, its images/containers, and the context.
pub(crate) fn cmd_rm(force: bool) -> Result<()> {
    let Some(vm) = docker::machine()? else {
        println!("No Docker VM to remove.");
        return Ok(());
    };
    let running = vm.status == "running" && vm.pid.map(crate::db::pid_alive).unwrap_or(false);
    if running && !force {
        anyhow::bail!(
            "the Docker VM is running (stop it first with `bsdkrun docker stop`, or use -f)"
        );
    }
    docker::stop_proxy()?;
    docker::release_system_socket(&docker::socket_path()?);
    docker::remove_context();
    super::machines::remove_machine(&vm.id, true)?;
    // The volume is where every pulled image and container lives — removing
    // the machine but keeping it would leave gigabytes nobody can reach.
    match super::volumes::remove_volume(docker::VOLUME, true) {
        Ok(_) => println!("removed the Docker VM and its image store"),
        Err(e) => println!("removed the Docker VM (its volume is still there: {e})"),
    }
    Ok(())
}

/// `bsdkrun docker ps` — containers, as the engine reports them.
#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_ps(all: bool, json: bool) -> Result<()> {
    let rows = docker::containers(all)?;
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "No {}containers. Run one with `docker run`.",
            if all { "" } else { "running " }
        );
        return Ok(());
    }
    println!(
        "{:<14}  {:<22}  {:<24}  {:<18}  {}",
        "CONTAINER ID", "NAME", "IMAGE", "STATUS", "PORTS"
    );
    for c in rows {
        println!(
            "{:<14}  {:<22}  {:<24}  {:<18}  {}",
            c.id,
            super::truncate(&c.name, 22),
            super::truncate(&c.image, 24),
            super::truncate(&c.status, 18),
            if c.ports.is_empty() {
                "-".to_string()
            } else {
                c.ports.join(", ")
            }
        );
    }
    Ok(())
}

/// `bsdkrun docker container <action> <id>` — the actions the UIs drive.
pub(crate) fn cmd_container(action: &str, ids: &[String]) -> Result<()> {
    let mut failed = false;
    for id in ids {
        match docker::container_action(id, action) {
            Ok(_) => println!("{id}"),
            Err(e) => {
                eprintln!("Error: {e:#}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// `bsdkrun docker logs <container>`.
pub(crate) fn cmd_logs(id: &str, tail: u32) -> Result<()> {
    print!("{}", docker::container_logs(id, tail)?);
    Ok(())
}

/// `bsdkrun docker disk [--size SIZE]` — show or grow the image store.
pub(crate) fn cmd_disk(size: Option<&str>, json: bool) -> Result<()> {
    if let Some(size) = size {
        docker::ensure_data_disk(size)?;
        println!("image store grown to {size}");
        if docker::engine_running() {
            // Not a caveat we can engineer away: virtio-blk pins the device
            // size at attach time, so the guest keeps seeing the old disk
            // until it re-attaches at boot (where the init resizes the fs).
            println!(
                "  the running engine still sees the old size — \
                 `bsdkrun docker stop && bsdkrun docker start` applies it"
            );
        }
    }
    let s = docker::status()?;
    if json {
        println!("{}", serde_json::to_string(&s)?);
        return Ok(());
    }
    match (&s.disk, s.disk_size) {
        (Some(path), Some(bytes)) => {
            println!("image store: {path}");
            println!("  size       {}", crate::oci::human_size(bytes));
            println!("  grow with  bsdkrun docker disk --size <SIZE>");
        }
        _ => {
            println!("image store: the VM's rootfs (host-backed, no fixed size)");
            println!(
                "  a dedicated disk is opt-in: `bsdkrun docker rm -f` then \
                 `bsdkrun docker start --disk-size 60G`"
            );
        }
    }
    Ok(())
}

/// `bsdkrun docker env` — the two lines a shell needs when it would rather not
/// use a docker context (CI, a stray `sudo docker`, fish/nushell setups).
pub(crate) fn cmd_env() -> Result<()> {
    let socket = docker::socket_path()?;
    println!("export DOCKER_HOST=unix://{}", socket.display());
    println!("# eval \"$(bsdkrun docker env)\"");
    Ok(())
}

/// `bsdkrun docker shell` — a shell *in the VM*, not in a container.
pub(crate) fn cmd_shell() -> Result<()> {
    let vm = docker::machine()?
        .ok_or_else(|| anyhow::anyhow!("no Docker VM yet — `bsdkrun docker start`"))?;
    super::guest::cmd_shell(&vm.id)
}

/// The detached proxy process (`docker __serve`). Never returns.
pub(crate) fn cmd_serve(port: u16, machine: &str, publish_bind: &str) -> Result<()> {
    docker::serve(port, machine, docker::PublishBind::parse(publish_bind)?)
}

/// Print what a fresh `start` should tell the user: where the socket is, and
/// how to point a CLI at it. Shared by `start` (in `boot.rs`) and the tests.
pub(crate) fn report_started(s: &Status, containers: &[Container]) {
    println!("Docker engine ready.");
    println!("  socket   {}", s.socket);
    if s.context_active {
        println!(
            "  context  {} (active) — `docker ps` just works",
            docker::CONTEXT
        );
    } else if s.context {
        println!(
            "  context  {ctx} — select it with `docker context use {ctx}`",
            ctx = docker::CONTEXT
        );
    } else {
        println!("  export DOCKER_HOST=unix://{}", s.socket);
    }
    if !s.mounts.is_empty() {
        println!("  shared   {}", s.mounts.join(", "));
    }
    if !containers.is_empty() {
        println!("  {} container(s) already running", containers.len());
    }
}
