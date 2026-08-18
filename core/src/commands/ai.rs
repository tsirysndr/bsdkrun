//! `bsdkrun ai` and the per-agent aliases (`bsdkrun claude`, `bsdkrun codex`, …).
//!
//! The engine lives in [`crate::ai`]; this prints, and the half that ends in a
//! boot lives in [`crate::commands::boot`] — the same split the flavor and
//! docker commands use.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::ai::{self, upload, Agent};
use crate::db;

/// `bsdkrun ai agents` — the agents, and whether each one is ready to boot.
#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_agents(json: bool) -> Result<()> {
    let agents = ai::agents()?;
    if json {
        println!("{}", serde_json::to_string(&agents)?);
        return Ok(());
    }
    println!(
        "{:<10}  {:<18}  {:<10}  {:<8}  {}",
        "ID", "AGENT", "INSTALLED", "RUNNING", "DESCRIPTION"
    );
    for a in &agents {
        println!(
            "{:<10}  {:<18}  {:<10}  {:<8}  {}",
            a.id,
            super::truncate(&a.label, 18),
            if a.installed { "yes" } else { "no" },
            a.running,
            a.description
        );
    }
    println!();
    println!("Start one with `bsdkrun <id>` (e.g. `bsdkrun claude`) — it shares the");
    println!("directory you run it in, and drops you into the agent's TUI.");
    Ok(())
}

/// `bsdkrun ai ls` — the sandboxes.
#[allow(clippy::print_literal)]
pub(crate) fn cmd_sessions(json: bool) -> Result<()> {
    let sessions = ai::sessions()?;
    if json {
        println!("{}", serde_json::to_string(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("No agent sandboxes. Start one with `bsdkrun claude`.");
        return Ok(());
    }
    // Grouped by project, because that is how they are worked on: several
    // sessions against one codebase belong together, whatever they are named.
    let mut projects: BTreeMap<String, Vec<&ai::Session>> = BTreeMap::new();
    for s in &sessions {
        projects
            .entry(s.project.clone().unwrap_or_else(|| "(no project)".into()))
            .or_default()
            .push(s);
    }
    for (project, rows) in &projects {
        println!("{project}");
        for s in rows {
            println!(
                "  {:<14}  {:<22}  {:<10}  {:<9}  {}",
                s.id,
                super::truncate(s.label.as_deref().unwrap_or(&s.name), 22),
                s.agent,
                if s.running { "running" } else { "stopped" },
                s.workspace.as_deref().unwrap_or("—")
            );
        }
    }
    Ok(())
}

/// `bsdkrun ai stop <agent>` — stop an agent's sandboxes. Its login survives:
/// that lives on the agent's home volume, not in the machine.
pub(crate) fn cmd_stop(agent: &str) -> Result<()> {
    let agent = ai::require(agent)?;
    let running: Vec<_> = ai::sessions_for(agent.id)?
        .into_iter()
        .filter(|s| s.running)
        .collect();
    if running.is_empty() {
        println!("No running {} sandbox.", agent.label);
        return Ok(());
    }
    for s in running {
        println!("{}", super::machines::stop(&s.id)?);
    }
    Ok(())
}

/// `bsdkrun ai rm <agent>` — remove an agent's sandboxes, and (unless kept) the
/// home volume holding its login.
pub(crate) fn cmd_rm(agent: &str, keep_home: bool) -> Result<()> {
    let agent = ai::require(agent)?;
    for s in ai::sessions_for(agent.id)? {
        super::machines::remove_machine(&s.id, true)?;
        println!("removed sandbox {}", s.name);
    }
    if keep_home {
        println!("kept {}'s saved login", agent.label);
        return Ok(());
    }
    match super::volumes::remove_volume(&ai::home_volume(agent.id), true) {
        Ok(_) => println!("removed {}'s saved login", agent.label),
        // Nothing to remove is the common case (the agent was never launched);
        // it is not worth an error.
        Err(e) => tracing::debug!("no home volume to remove for {}: {e:#}", agent.id),
    }
    Ok(())
}

/// Report what a sandbox is about to do, before a boot that may take a while.
pub(crate) fn announce(agent: &Agent, workspace: Option<&std::path::Path>, fresh: bool) {
    if fresh && !ai::flavor_built(agent) {
        println!(
            "Installing {} (first run — this builds the sandbox image once)…",
            agent.label
        );
    }
    match workspace {
        Some(w) => println!("Sharing {} into the sandbox.", w.display()),
        None => println!("No folder shared — the sandbox cannot see your files."),
    }
}

/// The exec argv the CLI and the UIs both attach with.
pub fn attach_argv(agent: &Agent, workspace: Option<&str>) -> Vec<String> {
    ai::tui_argv(agent, workspace)
}

/// `bsdkrun ai __shell-command <agent> <machine>` — the argv, as JSON.
pub(crate) fn cmd_shell_command(agent: &str, machine: &str) -> Result<()> {
    let agent = ai::require(agent)?;
    let workspace = db::Db::open()
        .and_then(|db| db.find_machine(machine))
        .ok()
        .and_then(|m| ai::workspace_of(std::path::Path::new(&m.state_dir)));
    println!(
        "{}",
        serde_json::to_string(&ai::tui_argv(agent, workspace.as_deref()))?
    );
    Ok(())
}

/// Find the machine to attach to for `agent`, if one is running.
pub fn running_machine(agent: &str) -> Result<Option<db::MachineRow>> {
    let Some(session) = ai::running_session(agent)? else {
        return Ok(None);
    };
    Ok(db::Db::open()?.find_machine(&session.id).ok())
}

/// `bsdkrun ai upload` — send local files to a sandbox on the engine's host.
///
/// The CLI talks to a local engine, so here both ends are the same machine and
/// this is a copy rather than a transfer. It exists at this layer anyway
/// because the daemon's mutation and the desktop's uploader call the same
/// [`ai::upload`] functions — one definition of what lands where, and one set
/// of refusals guarding it.
pub(crate) fn cmd_upload(
    what: &str,
    agent: &str,
    dir: Option<&str>,
    name: Option<&str>,
    all: bool,
    json: bool,
) -> Result<()> {
    let kind = upload::Kind::parse(what)?;
    let agent = ai::require(agent)?;

    // `git` is synthesized from `git config` rather than read off disk — see
    // `pack_git_identity` for why it is two values and not `~/.gitconfig`.
    let (packed, source) = if kind == upload::Kind::Git {
        (upload::pack_git_identity()?, None)
    } else {
        let source = kind.local_source(dir.map(std::path::Path::new))?;
        if !source.is_dir() {
            anyhow::bail!(
                "{} does not exist — there is nothing to upload",
                source.display()
            );
        }
        (upload::pack(&source, all)?, Some(source))
    };

    // The name has to be settled before the upload, not after: it decides the
    // destination directory, and re-deriving it on the far side from a tar
    // would be a second, silently different answer.
    let name = name.map(|n| n.to_string()).or_else(|| {
        source
            .as_ref()
            .and_then(|s| s.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });
    let dest = upload::receive(kind, agent.id, name.as_deref(), &packed.bytes)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "kind": kind.as_str(),
                "agent": agent.id,
                "path": dest.display().to_string(),
                "files": packed.files,
                "bytes": packed.size,
                "skipped": packed.skipped,
            })
        );
        return Ok(());
    }

    if kind == upload::Kind::Git {
        let (name, email) = ai::git_identity();
        println!(
            "uploaded git identity to {}: {} <{}>",
            dest.join(".gitconfig").display(),
            name.as_deref().unwrap_or("(no name)"),
            email.as_deref().unwrap_or("(no email)")
        );
        println!(
            "every {} sandbox on this engine commits as that.",
            agent.label
        );
        return Ok(());
    }

    println!("uploaded {} file(s) to {}", packed.files, dest.display());
    if !all {
        // Said out loud because the alternative is discovering a missing file
        // inside the sandbox, where the reason is not visible.
        println!("honouring .gitignore and .dockerignore (pass --all to send everything)");
    }
    if !packed.skipped.is_empty() {
        println!("also skipped build output: {}", packed.skipped.join(", "));
    }
    if kind == upload::Kind::Workspace {
        // Without this the upload is inert: nothing mounts a directory just
        // because it exists.
        println!(
            "\nstart a sandbox on it with:\n  bsdkrun ai start --agent {} --workspace {}",
            agent.id,
            dest.display()
        );
    }
    Ok(())
}

/// `bsdkrun ai __receive` — the engine side, reading a tar from stdin.
///
/// Hidden, and deliberately dumb: it chooses nothing. The kind decides the
/// destination ([`upload::destination`]), so a caller on the far end of a
/// socket cannot name a path on this host.
pub(crate) fn cmd_receive(what: &str, agent: &str, name: Option<&str>, json: bool) -> Result<()> {
    let kind = upload::Kind::parse(what)?;
    let agent = ai::require(agent)?;
    let bytes = upload::read_all(std::io::stdin().lock())?;
    if bytes.is_empty() {
        anyhow::bail!("no upload on stdin");
    }
    let dest = upload::receive(kind, agent.id, name, &bytes)?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "path": dest.display().to_string() })
        );
    } else {
        println!("{}", dest.display());
    }
    Ok(())
}

/// `bsdkrun ai disk` — the shared stores, and what each sandbox occupies.
#[allow(clippy::print_literal)]
pub(crate) fn cmd_disk_ls(json: bool, watch: Option<u64>) -> Result<()> {
    let Some(secs) = watch else {
        return disk_report(json);
    };
    // Monitoring: reprint on an interval until interrupted. Cleared each time
    // rather than scrolled, so the numbers stay in one place to watch.
    let secs = secs.max(1);
    loop {
        print!("\x1b[2J\x1b[H");
        disk_report(json)?;
        println!("\nwatching every {secs}s — ctrl-c to stop");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}

#[allow(clippy::print_literal)]
fn disk_report(json: bool) -> Result<()> {
    // Quoted from the source of truth, so the help cannot drift from what a
    // disk is actually created at.
    let (docker_default, nix_default) = (
        ai::disk::Shared::Docker.default_size(),
        ai::disk::Shared::Nix.default_size(),
    );
    let shared = ai::disk::status()?;
    let rows = ai::disk::usage()?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "shared": shared, "sandboxes": rows })
        );
        return Ok(());
    }

    println!("Shared stores");
    println!(
        "  {:<8}  {:<18}  {:<10}  {:<10}  {:<10}  {}",
        "DISK", "MOUNT", "SIZE", "USED", "FREE", "HELD BY"
    );
    for d in &shared {
        let or_dash = |n: u64| {
            if d.exists {
                crate::oci::human_size(n)
            } else {
                "—".into()
            }
        };
        println!(
            "  {:<8}  {:<18}  {:<10}  {:<10}  {:<10}  {}",
            d.name,
            d.guest_path,
            or_dash(d.size),
            or_dash(d.used),
            // The effective figure, not the disk's own headroom: a sparse
            // image cannot grow past what the host still has, and the ceiling
            // is the number that misleads.
            or_dash(d.effective_free),
            d.held_by.as_deref().unwrap_or("—")
        );
    }

    if let Some((free, total)) = ai::disk::host_free() {
        let pct = free
            .checked_mul(100)
            .and_then(|n| n.checked_div(total))
            .unwrap_or(0);
        println!(
            "\n  Host: {} free of {} ({pct}%).",
            crate::oci::human_size(free),
            crate::oci::human_size(total)
        );
        // Everything here is ultimately host-backed, so this is the number
        // that runs out first — including inside a sparse disk that still
        // reports terabytes of its own headroom.
        if pct < 10 || free < 5 * 1024 * 1024 * 1024 {
            // One println per line: a wrapped string literal carries this
            // file's own indentation into the output, which is how the note
            // below it came out ragged.
            println!("  WARNING: the host is nearly full. Sandboxes write into this space —");
            println!("  the rootfs and agent homes directly, and the shared disks as they fill.");
            println!(
                "  Reclaim with `bsdkrun ai rm <agent>`, or `docker system prune` in a sandbox."
            );
        }
    }
    // One println per line: a wrapped string literal carries this file's own
    // indentation into the output, which is what made this note ragged.
    println!();
    println!("  Sizes are sparse: SIZE is the ceiling, USED is what it costs on this host,");
    println!("  FREE is what it can still grow into — capped by the host, not the ceiling.");
    println!("  Defaults: docker {docker_default}, nix {nix_default}.");
    println!("  One running sandbox holds a disk at a time: two guests writing one ext4");
    println!("  image corrupts it, so a second sandbox boots with an empty store instead.");
    println!("  Grow one with `bsdkrun ai disk grow docker --size 200G`.");

    if rows.is_empty() {
        println!("\nNo agent sandboxes. Start one with `bsdkrun claude`.");
        return Ok(());
    }

    println!("\nSandboxes");
    println!(
        "  {:<14}  {:<22}  {:<9}  {:<10}  {}",
        "ID", "SANDBOX", "STATUS", "ROOTFS", "HOME"
    );
    for r in &rows {
        println!(
            "  {:<14}  {:<22}  {:<9}  {:<10}  {}",
            r.id,
            super::truncate(&r.name, 22),
            if r.running { "running" } else { "stopped" },
            crate::oci::human_size(r.rootfs),
            crate::oci::human_size(r.home)
        );
    }

    // The home volume is per agent, so summing the column would count one
    // agent's login once per session it has.
    let rootfs: u64 = rows.iter().map(|r| r.rootfs).sum();
    let mut homes: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    for r in &rows {
        homes.insert(&r.agent, r.home);
    }
    println!();
    println!(
        "  {} sandbox(es): {} of rootfs, {} of agent homes.",
        rows.len(),
        crate::oci::human_size(rootfs),
        crate::oci::human_size(homes.values().sum::<u64>())
    );
    println!("  Both are host directories shared into the guest: no size of their own to");
    println!("  raise, and they grow into the host's free space above.");
    Ok(())
}

/// `bsdkrun ai disk grow <docker|nix> --size N`.
pub(crate) fn cmd_disk_grow(disk: &str, size: &str) -> Result<()> {
    let what = ai::disk::Shared::parse(disk)?;
    let existed = what.image()?.exists();
    let path = ai::disk::ensure(what, size)?;
    let now = path.metadata().map(|m| m.len()).unwrap_or(0);

    println!(
        "{} the shared {} disk: {} at {}",
        if existed { "grew" } else { "created" },
        what.as_str(),
        crate::oci::human_size(now),
        path.display()
    );
    println!("Sandboxes mount it at {}.", what.guest_path());

    // virtio-blk fixes a device's size when the VM attaches it, so a running
    // guest cannot see the growth and `resize2fs` inside it would resize to the
    // *old* size. Say so rather than imply it took effect.
    if let Some(id) = ai::disk::holder(what) {
        println!(
            "\n{id} is running and holds this disk. A virtio-blk device's size is fixed when\n\
             the VM attaches it, so that guest still sees the old size. Restart it to pick\n\
             this up:\n  bsdkrun stop {id} && bsdkrun ai resume {id}"
        );
    }
    Ok(())
}
