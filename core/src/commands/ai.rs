//! `bsdkrun ai` and the per-agent aliases (`bsdkrun claude`, `bsdkrun codex`, …).
//!
//! The engine lives in [`crate::ai`]; this prints, and the half that ends in a
//! boot lives in [`crate::commands::boot`] — the same split the flavor and
//! docker commands use.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::ai::{self, Agent};
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
